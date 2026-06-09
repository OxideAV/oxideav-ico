//! Standalone (framework-free) Windows ANI animated-cursor parser.
//!
//! The ANI container is a RIFF file whose form type is `ACON`. It
//! carries an animation header (`anih` — 36-byte ANIHEADER), an
//! optional `LIST 'INFO'` block (title / author), an optional `seq `
//! frame-sequence override, an optional `rate` per-step duration
//! table, and a `LIST 'fram'` containing N `icon` chunks. Each
//! `icon` chunk is either a complete ICO/CUR resource (when
//! `bfAttributes & AF_ICON`) or a raw headerless BMP (when not).
//!
//! `oxideav-ico`'s [`crate::read_ico_raw`] entry point continues to
//! refuse `.ani` input with a clear "this is a different container"
//! error. This module is the explicit ANI-side counterpart: callers
//! that *want* to walk an ANI file get a structured view of the
//! RIFF tree, with each frame's raw bytes ready to feed back into
//! [`crate::read_ico_raw`] (or a BMP-DIB decoder) frame by frame.
//!
//! Wall: derived strictly from `docs/image/ico/ani-acon-format.md`
//! (the cleanroom-staged ACON spec), not from any animated-cursor
//! library source.

use crate::error::{IcoError as Error, Result};

/// `bfAttributes` bit 0 — when set, each frame in `LIST 'fram'` is
/// a complete ICO/CUR resource (parseable with
/// [`crate::read_ico_raw`]). When clear, each frame is a raw
/// headerless BMP described by `anih`'s `iWidth` / `iHeight` /
/// `iBitCount` / `nPlanes`.
pub const AF_ICON: u32 = 0x0000_0001;

/// `bfAttributes` bit 1 — when set, the file is expected to carry a
/// `seq ` chunk overriding the default identity step sequence.
/// Pure metadata: a missing `seq ` chunk with this bit set is still
/// recoverable (we fall back to identity), so this parser warns
/// rather than rejects.
pub const AF_SEQUENCE: u32 = 0x0000_0002;

/// Maximum number of frames / steps we'll accept up-front, before
/// any per-chunk parsing. Cursor files in the wild rarely exceed a
/// few dozen frames; pathological values (a header claiming
/// `nFrames = 0xFFFF_FFFF`) would otherwise drive an allocator into
/// the ground before we ever inspected the `LIST 'fram'`. Mirrors
/// the spirit of the `1..=256` clamp on ICO directory entry counts:
/// a defensive upper bound that's still 1000× larger than anything
/// real.
const MAX_FRAMES_OR_STEPS: u32 = 65_536;

/// `ANIHEADER` chunk payload — 36 bytes of fixed-layout LE `u32`s.
/// All fields named verbatim from the ACON spec.
///
/// `cb_size` repeats the chunk's declared length (36) per spec, but
/// per the spec note "some encoders write a slightly different
/// cbSize" the parser already validated the chunk length itself and
/// `cb_size` here is informational only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AniHeader {
    /// `cbSize` — size of this header structure. Per spec the
    /// canonical value is 36; the parser doesn't reject other
    /// values since the wider ecosystem includes encoders that
    /// disagree.
    pub cb_size: u32,
    /// `nFrames` — number of `icon` chunks stored in `LIST 'fram'`.
    pub n_frames: u32,
    /// `nSteps` — number of display steps. Equals `nFrames` when
    /// no `seq ` chunk overrides the order.
    pub n_steps: u32,
    /// `iWidth` — width in pixels (`0` = "take from frame").
    pub i_width: u32,
    /// `iHeight` — height in pixels (`0` = "take from frame").
    pub i_height: u32,
    /// `iBitCount` — bit-depth (`0` = "take from frame").
    pub i_bit_count: u32,
    /// `nPlanes` — number of colour planes (canonically 1).
    pub n_planes: u32,
    /// `iDispRate` — default display rate per step, in 1/60-second
    /// units (jiffies). Overridden per-step by the `rate` chunk
    /// when present.
    pub i_disp_rate: u32,
    /// `bfAttributes` — flags. See [`AF_ICON`] / [`AF_SEQUENCE`].
    pub bf_attributes: u32,
}

impl AniHeader {
    /// `true` when `bfAttributes & AF_ICON` is set — frames carry
    /// full ICO/CUR resources rather than raw headerless BMPs.
    pub fn frames_are_icons(&self) -> bool {
        self.bf_attributes & AF_ICON != 0
    }

    /// `true` when `bfAttributes & AF_SEQUENCE` is set — the
    /// encoder expects a `seq ` chunk to override identity step
    /// order. (A missing chunk with this bit set is still
    /// recoverable by falling back to identity; this is a hint, not
    /// a hard requirement.)
    pub fn has_sequence_flag(&self) -> bool {
        self.bf_attributes & AF_SEQUENCE != 0
    }
}

/// One ACON metadata string from a `LIST 'INFO'` sub-chunk
/// (`INAM` = title, `IART` = author). Stored as raw bytes — ACON
/// pre-dates a settled charset convention; callers that want a
/// `String` apply their own decoder (commonly Windows-1252 or
/// UTF-8).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AniInfo {
    /// `INAM` payload — animation title.
    pub title: Option<Vec<u8>>,
    /// `IART` payload — author.
    pub author: Option<Vec<u8>>,
}

/// Parsed ANI file: header + optional metadata + frame payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AniFile {
    /// `anih` — animation header.
    pub header: AniHeader,
    /// `LIST 'INFO'` — optional metadata.
    pub info: AniInfo,
    /// `seq ` — `nSteps` zero-based indices into [`Self::frames`].
    /// `None` when the chunk is absent (caller falls back to
    /// identity `0..n_frames`).
    pub sequence: Option<Vec<u32>>,
    /// `rate` — `nSteps` per-step durations in 1/60-second jiffies.
    /// `None` when the chunk is absent (caller falls back to
    /// `header.i_disp_rate`).
    pub rates: Option<Vec<u32>>,
    /// `LIST 'fram'` payload — N `icon` chunks. Each entry is the
    /// raw chunk-payload bytes, ready to feed to
    /// [`crate::read_ico_raw`] when `header.frames_are_icons()` is
    /// true (the common case).
    pub frames: Vec<Vec<u8>>,
}

/// One resolved playback step from an ANI file — the merged result of
/// the per-step `seq ` index and per-step `rate` duration, with the
/// header-level `nSteps` / `iDispRate` defaults already applied. A
/// playback engine that holds a `Vec<AniStep>` can drive the animation
/// loop directly without re-deriving any defaulting rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AniStep {
    /// Zero-based index into [`AniFile::frames`] — guaranteed to be
    /// `< header.n_frames` (the parser already enforced this on the
    /// raw [`AniFile::sequence`] array, and the identity fallback
    /// only produces in-range indices by construction).
    pub frame_index: u32,
    /// Per-step duration in 1/60-second jiffies — `rate[i]` when the
    /// optional `rate` chunk was present, else `header.i_disp_rate`.
    /// Guaranteed non-zero: [`AniFile::playback_steps`] rejects any
    /// zero-jiffy step rather than emit a value that would either
    /// burn CPU in a poll-based renderer or divide-by-zero in a
    /// frame-rate normaliser.
    pub jiffies: u32,
}

impl AniFile {
    /// Number of playback steps the file declares, with the spec's
    /// "`nSteps = nFrames` when no `seq ` chunk" default already
    /// applied. Returns `header.n_steps` when the field is non-zero,
    /// else `header.n_frames`.
    ///
    /// Note `n_steps` and `n_frames` aren't always equal — `seq ` lets
    /// a 2-frame file play 4 (or 100) steps by repeating frames out of
    /// storage order. Callers driving the animation loop should size
    /// arrays against this method, not against `frames.len()`.
    pub fn resolved_step_count(&self) -> u32 {
        if self.header.n_steps == 0 {
            self.header.n_frames
        } else {
            self.header.n_steps
        }
    }

    /// Resolve the file's `seq ` / `rate` chunks into the concrete
    /// `(frame_index, jiffies)` playback table a renderer needs.
    ///
    /// Per the ACON spec:
    /// * Step count is `header.n_steps` (or `header.n_frames` when the
    ///   header field is zero — the spec's "= nFrames if no `seq`
    ///   chunk" rule).
    /// * Step `i`'s frame index is `sequence[i]` when the `seq ` chunk
    ///   is present, else identity `i` (clamped to `n_frames - 1`
    ///   would lose the playback intent — instead, identity is only
    ///   produced when the spec guarantees `n_steps == n_frames`).
    /// * Step `i`'s duration is `rates[i]` when the `rate` chunk is
    ///   present, else `header.i_disp_rate`.
    ///
    /// Errors:
    /// * No frames declared (`n_frames == 0` — the parser already
    ///   rejects this at read time, so this branch only triggers on
    ///   hand-constructed `AniFile`s).
    /// * Identity-fallback step `i` would index past `n_frames` — only
    ///   reachable when `n_steps > n_frames` *and* no `seq ` chunk is
    ///   present (the spec is silent on this combination; an identity
    ///   mapping is undefined past the frame array). The parser
    ///   accepts the raw header (`n_steps` legitimately exceeds
    ///   `n_frames` when paired with a `seq ` chunk), but this
    ///   accessor refuses to fabricate identity indices that would
    ///   panic downstream.
    /// * Any resolved `jiffies` value is zero — neither `rate[i]` nor
    ///   the `i_disp_rate` fallback may be `0`, since a zero-duration
    ///   step has no defined display behaviour and would either burn
    ///   100% CPU in a poll-based renderer (`if elapsed >= rate[i]`
    ///   advances instantly) or divide-by-zero in a frame-rate
    ///   normaliser.
    pub fn playback_steps(&self) -> Result<Vec<AniStep>> {
        if self.header.n_frames == 0 {
            return Err(Error::invalid(
                "ANI: playback_steps: header.n_frames = 0 (no frames to play)",
            ));
        }
        let step_count = self.resolved_step_count() as usize;
        let default_jiffies = self.header.i_disp_rate;
        let n_frames = self.header.n_frames;

        // Cross-check the optional chunks' lengths against the
        // resolved step count. The parser already aligned them, but
        // a caller constructing `AniFile` by hand could pass an
        // unexpected length; the accessor catches it rather than
        // silently truncating / panicking on out-of-bounds index.
        if let Some(seq) = &self.sequence {
            if seq.len() != step_count {
                return Err(Error::invalid(format!(
                    "ANI: playback_steps: sequence len {} != step count {step_count}",
                    seq.len()
                )));
            }
        }
        if let Some(rates) = &self.rates {
            if rates.len() != step_count {
                return Err(Error::invalid(format!(
                    "ANI: playback_steps: rates len {} != step count {step_count}",
                    rates.len()
                )));
            }
        }

        let mut out = Vec::with_capacity(step_count);
        for i in 0..step_count {
            let frame_index = match &self.sequence {
                Some(seq) => seq[i],
                None => {
                    // Identity fallback — only well-defined when
                    // i < n_frames. The spec's "nSteps = nFrames if
                    // no seq" default keeps this in range for the
                    // common case; refuse when the header pairs a
                    // larger nSteps with no seq chunk.
                    let idx = i as u32;
                    if idx >= n_frames {
                        return Err(Error::invalid(format!(
                            "ANI: playback_steps: identity step {i} \
                             out of range (nFrames = {n_frames}) — file \
                             declares nSteps > nFrames without a seq chunk"
                        )));
                    }
                    idx
                }
            };
            let jiffies = match &self.rates {
                Some(rates) => rates[i],
                None => default_jiffies,
            };
            if jiffies == 0 {
                return Err(Error::invalid(format!(
                    "ANI: playback_steps: step {i} resolved to 0 jiffies \
                     (zero-duration step is undefined)"
                )));
            }
            out.push(AniStep {
                frame_index,
                jiffies,
            });
        }
        Ok(out)
    }

    /// Total animation cycle length in 1/60-second jiffies — the sum of
    /// every step's resolved duration.
    ///
    /// The ACON spec's playback rule is "for each step `i`, show frame
    /// `seq[i]` for `rate[i]` jiffies; loop". A renderer that needs to
    /// know how long one full pass takes (e.g. to schedule the next
    /// wake-up, to convert to wall-clock seconds via `total / 60`, or to
    /// pre-size a frame-cycle buffer) currently has to call
    /// [`Self::playback_steps`] and sum the result by hand. This typed
    /// accessor folds that sum into one call and returns a `u64` so the
    /// arithmetic can't overflow on adversarial input: a file with the
    /// 65_536-step cap and every per-step rate at `u32::MAX` sums to
    /// roughly `2.8e14`, which fits a `u64` comfortably but overflows a
    /// `u32` by a factor of 65_536.
    ///
    /// Defaulting rules match [`Self::playback_steps`]:
    /// * Step count is `header.n_steps` (or `header.n_frames` when the
    ///   header field is zero — the spec's "= nFrames if no `seq` chunk"
    ///   rule, surfaced through [`Self::resolved_step_count`]).
    /// * Step `i`'s duration is `rates[i]` when the `rate` chunk is
    ///   present, else `header.i_disp_rate`.
    ///
    /// Errors:
    /// * No frames declared (`n_frames == 0`) — the byte parser already
    ///   rejects this, so this branch only triggers on hand-constructed
    ///   `AniFile`s.
    /// * The optional `rates` `Vec` length disagrees with the resolved
    ///   step count (same hand-constructed-only branch
    ///   [`Self::playback_steps`] guards).
    /// * Any per-step duration resolves to `0` — same reasoning as
    ///   [`Self::playback_steps`]: a zero-jiffy step has no defined
    ///   display behaviour, and folding it into the total would mask
    ///   the bug.
    ///
    /// Note this accessor does not need to consult the `sequence`
    /// chunk: the spec's per-step duration depends only on the step
    /// index, not on the frame the step picks. A 2-frame file with a
    /// 4-step `seq ` runs through `rate[0..4]` (or four copies of
    /// `i_disp_rate`) regardless of which frame each step lands on.
    /// The accessor still surfaces a `sequence` / step-count mismatch
    /// indirectly via [`Self::playback_steps`]'s contract — call that
    /// first if you need the per-step frame indices alongside the
    /// total.
    pub fn total_jiffies(&self) -> Result<u64> {
        if self.header.n_frames == 0 {
            return Err(Error::invalid(
                "ANI: total_jiffies: header.n_frames = 0 (no frames to play)",
            ));
        }
        let step_count = self.resolved_step_count() as usize;
        if let Some(rates) = &self.rates {
            if rates.len() != step_count {
                return Err(Error::invalid(format!(
                    "ANI: total_jiffies: rates len {} != step count {step_count}",
                    rates.len()
                )));
            }
        }
        let default_jiffies = self.header.i_disp_rate;
        let mut total: u64 = 0;
        for i in 0..step_count {
            let jiffies = match &self.rates {
                Some(rates) => rates[i],
                None => default_jiffies,
            };
            if jiffies == 0 {
                return Err(Error::invalid(format!(
                    "ANI: total_jiffies: step {i} resolved to 0 jiffies \
                     (zero-duration step is undefined)"
                )));
            }
            // u32 → u64 widening: the per-step jiffies value is at most
            // u32::MAX, and step_count is bounded by MAX_FRAMES_OR_STEPS
            // (65_536). The sum therefore fits a u64 by 14+ bits of
            // headroom, with no `checked_add` needed.
            total += jiffies as u64;
        }
        Ok(total)
    }

    /// Total animation cycle length in wall-clock seconds — the
    /// [`Self::total_jiffies`] result divided by the spec's 60-jiffies-per-second
    /// conversion factor.
    ///
    /// The ACON spec fixes the per-step duration unit as "1/60 of a second"
    /// (`anih.iDispRate` and per-step `rate[i]` both carry jiffies). A
    /// renderer that needs seconds for clock-side scheduling — sleep
    /// timers, video-clip lengths, UI labels reading "1.5 s loop" — would
    /// currently call [`Self::total_jiffies`] and divide by `60.0` by hand;
    /// this accessor folds that conversion into the type system so the
    /// `60` literal can't drift across call sites and the unit is fixed in
    /// the function name.
    ///
    /// The conversion is exact in `f64` for every cycle length this parser
    /// can produce: the bounded-input cap of 65_536 steps × `u32::MAX`
    /// jiffies sums to roughly `2.8e14`, which sits well under the `f64`
    /// integer-precision boundary at `2^53 ≈ 9.0e15`. No precision loss is
    /// possible on parser-accepted input.
    ///
    /// Errors: same conditions as [`Self::total_jiffies`] (hand-constructed
    /// `n_frames = 0`, mismatched `rates` length, any zero-jiffy step).
    /// The accessor is a thin wrapper that reuses that contract verbatim
    /// rather than re-deriving the rate / step-count defaulting rules.
    pub fn cycle_seconds(&self) -> Result<f64> {
        // Spec: 1 second = 60 jiffies (`anih.iDispRate` units). The `as f64`
        // widening is exact for the parser's bounded input range (see the
        // doc-comment for the precision argument); no `try_from` / fallible
        // conversion is appropriate here.
        Ok(self.total_jiffies()? as f64 / 60.0)
    }

    /// Locate the playback step that is **active** at a given jiffy offset
    /// into one animation cycle. Returns the step index, suitable as a
    /// subscript into the [`Self::playback_steps`] table.
    ///
    /// The ACON spec's playback rule is "for each step `i`, show frame
    /// `seq[i]` for `rate[i]` jiffies; loop". A renderer driven by a
    /// wall-clock-like jiffy counter (`elapsed = now - start_of_cycle`)
    /// needs the inverse mapping — given an elapsed-jiffy value, which step
    /// is currently on screen? This accessor folds that lookup into one
    /// call instead of forcing every renderer to re-derive a cumulative-sum
    /// walk over the rate table.
    ///
    /// Semantics:
    ///
    /// * Step `i` spans the half-open jiffy interval
    ///   `[start_i, start_i + step.jiffies)`, where `start_i` is the sum of
    ///   `step.jiffies` for every preceding step.
    /// * Step `0` is therefore active for `jiffy ∈ [0, step_0.jiffies)`,
    ///   step `1` for `jiffy ∈ [step_0.jiffies, step_0.jiffies + step_1.jiffies)`,
    ///   and so on.
    /// * `jiffy` is expected to be the elapsed offset **inside one cycle**.
    ///   The caller is responsible for `jiffy %= total_jiffies` before
    ///   calling — looping is a renderer-level concern, not the accessor's.
    /// * The `u64` parameter type matches [`Self::total_jiffies`]'s return
    ///   type — a cycle whose total exceeds `u32::MAX` can produce a
    ///   per-cycle elapsed offset that doesn't fit a `u32`, so the
    ///   accessor parameterises on `u64` to avoid forcing the caller to
    ///   pre-truncate.
    ///
    /// Errors:
    ///
    /// * Same conditions as [`Self::playback_steps`] (`n_frames = 0`,
    ///   mismatched `sequence` / `rates` length, any zero-jiffy step,
    ///   identity-fallback past `n_frames`). The accessor delegates to
    ///   [`Self::playback_steps`] up front so a malformed file produces a
    ///   single deterministic error rather than an ambiguous "active step
    ///   = ?" answer.
    /// * `jiffy >= total_jiffies` — the elapsed offset has wrapped past
    ///   the end of one cycle. The caller forgot to modulo. Reporting an
    ///   error rather than silently clamping catches the renderer bug at
    ///   the source: a wrap-past-cycle-end value either means the
    ///   wall-clock counter has drifted (timer bug) or the cycle length
    ///   has changed under the renderer (file swap during playback);
    ///   both deserve a deliberate caller fix-up rather than a silent
    ///   "you're seeing the last frame forever" outcome.
    pub fn step_at_jiffy(&self, jiffy: u64) -> Result<usize> {
        // Resolve the full step table first — this also surfaces every
        // zero-jiffy / mismatched-length / identity-past-nframes rejection
        // path before any lookup arithmetic runs, so the caller sees the
        // same error contract `playback_steps` documents.
        let steps = self.playback_steps()?;
        // `steps` is non-empty by construction: `playback_steps` rejects
        // `n_frames = 0` up front and the resolved-step-count never falls
        // below 1 once `n_frames >= 1`.
        debug_assert!(!steps.is_empty(), "playback_steps must be non-empty");

        // Walk the cumulative-jiffy boundaries. We compare against `end`
        // (exclusive upper bound for step `i`) so step `i` claims the
        // half-open interval `[start, end)`; `jiffy == end` flips to step
        // `i+1` cleanly, matching the spec's "show frame, then advance"
        // edge semantics. `u64` accumulation can't overflow on
        // parser-accepted input — the precision argument from
        // `total_jiffies` (worst case ~2.8e14) applies verbatim.
        let mut cumulative: u64 = 0;
        for (i, step) in steps.iter().enumerate() {
            cumulative += step.jiffies as u64;
            if jiffy < cumulative {
                return Ok(i);
            }
        }

        // Fell off the end — `jiffy >= total_jiffies`. The caller forgot
        // to modulo by the cycle length. `cumulative` at this point is
        // the total cycle length, so it's the natural value to surface in
        // the error message (and avoids a second `total_jiffies` call).
        Err(Error::invalid(format!(
            "ANI: step_at_jiffy: jiffy {jiffy} >= total cycle length {cumulative} \
             (caller must apply modulo `jiffy % total_jiffies` before lookup)"
        )))
    }
}

/// Parse a Windows ANI animated-cursor file.
///
/// Returns the animation header, any metadata, sequence + rate
/// overrides, and the raw bytes of each frame. The caller is
/// responsible for decoding individual frames (typically by handing
/// each `frames[i]` back to [`crate::read_ico_raw`] when
/// `header.frames_are_icons()` is `true`).
///
/// Strictness:
///
/// * The file must start with `RIFF`, declare a non-zero size that
///   fits inside `input`, and form-type `ACON`.
/// * The `anih` chunk must appear and decode to a 36-byte
///   `AniHeader` payload.
/// * `LIST 'fram'` must appear and contain exactly
///   `header.n_frames` `icon` chunks.
/// * `nFrames` and `nSteps` are clamped to [`MAX_FRAMES_OR_STEPS`]
///   to bound allocator pressure on adversarial input.
/// * Unknown chunks at the top level are tolerated (skipped) so
///   forward-compatible files don't fail; the spec doesn't fix the
///   chunk order strictly.
pub fn read_ani_raw(input: &[u8]) -> Result<AniFile> {
    // RIFF outer wrapper: "RIFF" + LE u32 size + "ACON" + payload.
    if input.len() < 12 {
        return Err(Error::invalid("ANI: input shorter than RIFF/ACON header"));
    }
    if &input[..4] != b"RIFF" {
        return Err(Error::invalid("ANI: missing RIFF magic"));
    }
    let declared = u32::from_le_bytes([input[4], input[5], input[6], input[7]]) as usize;
    // The RIFF length covers form-type + child chunks (i.e. bytes
    // 8..8+declared in the source file). The full file is therefore
    // 8 + declared bytes long; accept a longer buffer (extra
    // trailing data) but reject a shorter one — truncation can
    // smuggle a malformed inner tree past per-chunk bounds checks.
    let body_end = 8usize
        .checked_add(declared)
        .ok_or_else(|| Error::invalid("ANI: RIFF declared size overflows usize"))?;
    if body_end > input.len() {
        return Err(Error::invalid(format!(
            "ANI: RIFF declared size {declared} exceeds available bytes {}",
            input.len() - 8
        )));
    }
    if &input[8..12] != b"ACON" {
        return Err(Error::invalid("ANI: form-type is not ACON"));
    }

    let mut header: Option<AniHeader> = None;
    let mut info = AniInfo::default();
    let mut sequence: Option<Vec<u32>> = None;
    let mut rates: Option<Vec<u32>> = None;
    let mut frames: Option<Vec<Vec<u8>>> = None;

    // Walk children of the outer RIFF chunk. Body is `input[12..body_end]`.
    let mut cursor = 12usize;
    while cursor < body_end {
        let (tag, payload, next) = read_chunk(input, cursor, body_end)?;
        match tag {
            b"anih" => {
                if header.is_some() {
                    return Err(Error::invalid("ANI: duplicate anih chunk"));
                }
                header = Some(parse_anih(payload)?);
            }
            b"seq " => {
                let count = expected_step_count(&header)?;
                let indices = parse_u32_array(payload, count, "seq")?;
                // Bounds-check each step index against `nFrames` — a
                // renderer reaches `frames[seq[i]]` directly, so an
                // out-of-range entry would panic / out-of-bounds-read
                // downstream. Same probe-vs-render shape as the CUR
                // hotspot body-dim check: the directory walker sees
                // the indices, the renderer dereferences them.
                let n_frames = header
                    .as_ref()
                    .map(|h| h.n_frames)
                    .ok_or_else(|| Error::invalid("ANI: seq chunk appeared before anih"))?;
                for (i, &idx) in indices.iter().enumerate() {
                    if idx >= n_frames {
                        return Err(Error::invalid(format!(
                            "ANI: seq[{i}] = {idx} out of range (nFrames = {n_frames})"
                        )));
                    }
                }
                sequence = Some(indices);
            }
            b"rate" => {
                let count = expected_step_count(&header)?;
                rates = Some(parse_u32_array(payload, count, "rate")?);
            }
            b"LIST" => {
                if payload.len() < 4 {
                    return Err(Error::invalid("ANI: LIST chunk shorter than 4-byte type"));
                }
                let list_type: &[u8; 4] = payload[..4].try_into().unwrap();
                let list_body = &payload[4..];
                match list_type {
                    b"INFO" => {
                        info = parse_info_list(list_body)?;
                    }
                    b"fram" => {
                        if frames.is_some() {
                            return Err(Error::invalid("ANI: duplicate LIST 'fram'"));
                        }
                        let expected = match &header {
                            Some(h) => h.n_frames as usize,
                            None => {
                                return Err(Error::invalid(
                                    "ANI: LIST 'fram' appeared before anih — \
                                     frame count is unknown",
                                ))
                            }
                        };
                        frames = Some(parse_fram_list(list_body, expected)?);
                    }
                    _ => {
                        // Unknown LIST sub-type — skip (forward compat).
                    }
                }
            }
            _ => {
                // Unknown top-level chunk — skip (forward compat).
            }
        }
        cursor = next;
    }

    let header = header.ok_or_else(|| Error::invalid("ANI: missing anih chunk"))?;
    let frames =
        frames.ok_or_else(|| Error::invalid("ANI: missing LIST 'fram' / frame payload"))?;

    Ok(AniFile {
        header,
        info,
        sequence,
        rates,
        frames,
    })
}

/// Read one RIFF chunk at `pos` (inside `[..end]`) — returns
/// `(tag, payload, next_pos)` where `next_pos` already includes the
/// padding byte that follows an odd-length payload.
fn read_chunk(input: &[u8], pos: usize, end: usize) -> Result<(&[u8; 4], &[u8], usize)> {
    if pos + 8 > end {
        return Err(Error::invalid("ANI: chunk header runs past end of parent"));
    }
    let tag: &[u8; 4] = input[pos..pos + 4].try_into().unwrap();
    let size = u32::from_le_bytes([
        input[pos + 4],
        input[pos + 5],
        input[pos + 6],
        input[pos + 7],
    ]) as usize;
    let payload_start = pos + 8;
    let payload_end = payload_start
        .checked_add(size)
        .ok_or_else(|| Error::invalid("ANI: chunk size overflows usize"))?;
    if payload_end > end {
        return Err(Error::invalid(format!(
            "ANI: chunk '{}' size {size} runs past parent end",
            tag_str(tag)
        )));
    }
    // RIFF pads chunks to even length with one zero byte.
    let mut next = payload_end;
    if size % 2 == 1 && next < end {
        next += 1;
    }
    Ok((tag, &input[payload_start..payload_end], next))
}

/// Parse the 36-byte `anih` payload into an [`AniHeader`].
fn parse_anih(payload: &[u8]) -> Result<AniHeader> {
    // Per spec, `anih` is 36 bytes; some encoders pad with garbage
    // but the canonical layout fits in 36. Reject anything shorter
    // (we'd be reading uninitialised bytes); accept longer (treat
    // tail as opaque, the same way we treat unknown chunks).
    if payload.len() < 36 {
        return Err(Error::invalid(format!(
            "ANI: anih payload too short ({} < 36)",
            payload.len()
        )));
    }
    let r =
        |o: usize| u32::from_le_bytes([payload[o], payload[o + 1], payload[o + 2], payload[o + 3]]);
    let header = AniHeader {
        cb_size: r(0),
        n_frames: r(4),
        n_steps: r(8),
        i_width: r(12),
        i_height: r(16),
        i_bit_count: r(20),
        n_planes: r(24),
        i_disp_rate: r(28),
        bf_attributes: r(32),
    };
    if header.n_frames == 0 {
        return Err(Error::invalid(
            "ANI: anih.nFrames = 0 (need at least one frame)",
        ));
    }
    if header.n_frames > MAX_FRAMES_OR_STEPS {
        return Err(Error::invalid(format!(
            "ANI: anih.nFrames {} exceeds sanity cap {}",
            header.n_frames, MAX_FRAMES_OR_STEPS
        )));
    }
    if header.n_steps > MAX_FRAMES_OR_STEPS {
        return Err(Error::invalid(format!(
            "ANI: anih.nSteps {} exceeds sanity cap {}",
            header.n_steps, MAX_FRAMES_OR_STEPS
        )));
    }
    // `nPlanes` consistency. The ACON spec fixes `nPlanes = 1` for
    // every animated cursor (multi-plane DIBs were a planar-video
    // relic that never reached cursor animation). Same probe-vs-render
    // shape as the BMP body's `biPlanes ∈ {0, 1}` check on the ICO
    // path: a probe that read the header and decided "this is a
    // single-plane animation" must agree with the renderer that's
    // about to walk the frame payloads. A header claiming e.g.
    // `nPlanes = 7` would either be silently ignored (and the file
    // round-tripped into a non-spec value) or interpreted by some
    // future planar-mode renderer the spec doesn't describe — neither
    // outcome is what the caller asked for.
    //
    // Carve-out mirrors the BMP-side strictness: `0` is tolerated
    // ("unspecified — defer to a different field") to match the
    // wider ecosystem's tolerance for an absent value; `1` is the
    // canonical spec-mandated value. Any other value is a probe-vs-
    // render mismatch up front. The `frames_are_icons()` (AF_ICON)
    // path doesn't use `nPlanes` directly (each ICO/CUR frame
    // carries its own planes assertion inside the inner DIB), but
    // the raw-BMP path (`AF_ICON` clear) does — and the spec note
    // "= 1" is unconditional, so the check applies to both.
    if header.n_planes > 1 {
        return Err(Error::invalid(format!(
            "ANI: anih.nPlanes = {} (must be 0 or 1)",
            header.n_planes
        )));
    }
    // `iWidth` / `iHeight` range. Per spec these are "Width / Height in
    // pixels (0 = take from frame)" — they describe a cursor pixel
    // dimension, which the ICO/CUR layer constrains to `1..=256` (the
    // directory's `bWidth` / `bHeight` byte fields physically can't
    // describe anything outside that range, with the `0 == 256`
    // convention pinning the upper bound). Same probe-vs-render shape
    // as the BMP-body dim range check on the ICO path: a probe that
    // read the header and decided "frames are 32×32" must agree with
    // anything a renderer later does with the value. `0` is the
    // spec-mandated "unspecified — take from frame" sentinel and is
    // tolerated; non-zero values outside `1..=256` are a probe-vs-
    // render mismatch that no real cursor file produces (and that an
    // adversarial file might use to smuggle a 4-billion-pixel width
    // into a downstream allocator).
    if header.i_width > 256 {
        return Err(Error::invalid(format!(
            "ANI: anih.iWidth = {} (must be 0 or 1..=256)",
            header.i_width
        )));
    }
    if header.i_height > 256 {
        return Err(Error::invalid(format!(
            "ANI: anih.iHeight = {} (must be 0 or 1..=256)",
            header.i_height
        )));
    }
    // `iBitCount` value set. Per spec "Bits per pixel (color depth =
    // 2^iBitCount; 0 = frame)". The ICO/CUR sub-image bit-depth set is
    // `{0, 1, 4, 8, 16, 24, 32}` — same set the directory `wBitCount`
    // field is constrained to on the ICO path. `0` here means "take
    // from frame" (spec-mandated sentinel); anything outside that set
    // doesn't match a renderable DIB layout and is rejected up front
    // rather than letting a raw-BMP-path decoder choke on an
    // un-decodable `2 bpp` / `7 bpp` claim.
    match header.i_bit_count {
        0 | 1 | 4 | 8 | 16 | 24 | 32 => {}
        n => {
            return Err(Error::invalid(format!(
                "ANI: anih.iBitCount = {n} (must be one of {{0, 1, 4, 8, 16, 24, 32}})"
            )))
        }
    }
    Ok(header)
}

/// Cross-validate the step-count `seq ` / `rate` arrays against the
/// `anih`.nSteps the parser saw earlier. If `nSteps` is zero
/// (defaulted) we use `nFrames` instead, mirroring the spec note
/// "= nFrames if no seq chunk".
fn expected_step_count(header: &Option<AniHeader>) -> Result<usize> {
    let header = header
        .as_ref()
        .ok_or_else(|| Error::invalid("ANI: seq / rate chunk appeared before anih"))?;
    let count = if header.n_steps == 0 {
        header.n_frames
    } else {
        header.n_steps
    };
    Ok(count as usize)
}

/// Parse `count` little-endian `u32` values out of `payload`. Used
/// for both `seq ` and `rate`.
fn parse_u32_array(payload: &[u8], count: usize, what: &str) -> Result<Vec<u32>> {
    let expected = count
        .checked_mul(4)
        .ok_or_else(|| Error::invalid(format!("ANI: {what} count overflows usize")))?;
    if payload.len() < expected {
        return Err(Error::invalid(format!(
            "ANI: {what} chunk payload {} < expected {} ({} steps × 4)",
            payload.len(),
            expected,
            count
        )));
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let o = i * 4;
        out.push(u32::from_le_bytes([
            payload[o],
            payload[o + 1],
            payload[o + 2],
            payload[o + 3],
        ]));
    }
    Ok(out)
}

/// Parse a `LIST 'INFO'` body — extract `INAM` (title) and `IART`
/// (author). Anything else inside the LIST is skipped.
fn parse_info_list(body: &[u8]) -> Result<AniInfo> {
    let mut info = AniInfo::default();
    let mut cursor = 0usize;
    while cursor < body.len() {
        let (tag, payload, next) = read_chunk(body, cursor, body.len())?;
        match tag {
            b"INAM" => info.title = Some(payload.to_vec()),
            b"IART" => info.author = Some(payload.to_vec()),
            _ => {}
        }
        cursor = next;
    }
    Ok(info)
}

/// Parse a `LIST 'fram'` body — expect exactly `expected` `icon`
/// chunks. Non-`icon` chunks inside the list are an error: the spec
/// fixes the list contents, and a stray chunk would change the
/// implied frame-index mapping.
fn parse_fram_list(body: &[u8], expected: usize) -> Result<Vec<Vec<u8>>> {
    let mut frames = Vec::with_capacity(expected);
    let mut cursor = 0usize;
    while cursor < body.len() {
        let (tag, payload, next) = read_chunk(body, cursor, body.len())?;
        if tag != b"icon" {
            return Err(Error::invalid(format!(
                "ANI: LIST 'fram' contains unexpected chunk '{}'",
                tag_str(tag)
            )));
        }
        frames.push(payload.to_vec());
        cursor = next;
    }
    if frames.len() != expected {
        return Err(Error::invalid(format!(
            "ANI: LIST 'fram' carried {} frames; anih declared {}",
            frames.len(),
            expected
        )));
    }
    Ok(frames)
}

/// Lossy ASCII rendering of a 4-byte FOURCC for error messages.
fn tag_str(tag: &[u8; 4]) -> String {
    tag.iter()
        .map(|&b| {
            if (0x20..0x7F).contains(&b) {
                b as char
            } else {
                '.'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid ANI file with `n` frames. Each frame
    /// is a 4-byte placeholder payload — not a real ICO/CUR, but
    /// the parser doesn't try to decode frame payloads itself, so
    /// any non-empty byte sequence is accepted as a chunk payload.
    fn build_minimal_ani(n_frames: u32, n_steps: u32) -> Vec<u8> {
        let mut anih_payload = vec![0u8; 36];
        let put = |buf: &mut [u8], o: usize, v: u32| {
            buf[o..o + 4].copy_from_slice(&v.to_le_bytes());
        };
        put(&mut anih_payload, 0, 36);
        put(&mut anih_payload, 4, n_frames);
        put(&mut anih_payload, 8, n_steps);
        put(&mut anih_payload, 12, 0);
        put(&mut anih_payload, 16, 0);
        put(&mut anih_payload, 20, 0);
        put(&mut anih_payload, 24, 1);
        put(&mut anih_payload, 28, 10);
        put(&mut anih_payload, 32, AF_ICON);

        let mut anih_chunk = Vec::new();
        anih_chunk.extend_from_slice(b"anih");
        anih_chunk.extend_from_slice(&(anih_payload.len() as u32).to_le_bytes());
        anih_chunk.extend_from_slice(&anih_payload);

        // LIST 'fram' { icon[0], icon[1], … }.
        let mut fram_body = Vec::new();
        fram_body.extend_from_slice(b"fram");
        for i in 0..n_frames {
            let frame_payload = [b'F', b'R', b'M', i as u8];
            fram_body.extend_from_slice(b"icon");
            fram_body.extend_from_slice(&(frame_payload.len() as u32).to_le_bytes());
            fram_body.extend_from_slice(&frame_payload);
        }
        let mut fram_chunk = Vec::new();
        fram_chunk.extend_from_slice(b"LIST");
        fram_chunk.extend_from_slice(&(fram_body.len() as u32).to_le_bytes());
        fram_chunk.extend_from_slice(&fram_body);

        // Wrap everything in RIFF/ACON.
        let mut body = Vec::new();
        body.extend_from_slice(b"ACON");
        body.extend_from_slice(&anih_chunk);
        body.extend_from_slice(&fram_chunk);

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn parses_minimal_three_frame_ani() {
        let bytes = build_minimal_ani(3, 3);
        let parsed = read_ani_raw(&bytes).unwrap();
        assert_eq!(parsed.header.n_frames, 3);
        assert_eq!(parsed.header.n_steps, 3);
        assert_eq!(parsed.header.cb_size, 36);
        assert_eq!(parsed.header.i_disp_rate, 10);
        assert!(parsed.header.frames_are_icons());
        assert!(!parsed.header.has_sequence_flag());
        assert_eq!(parsed.frames.len(), 3);
        for (i, f) in parsed.frames.iter().enumerate() {
            assert_eq!(f, &vec![b'F', b'R', b'M', i as u8]);
        }
        assert!(parsed.sequence.is_none());
        assert!(parsed.rates.is_none());
        assert_eq!(parsed.info, AniInfo::default());
    }

    #[test]
    fn rejects_missing_riff_magic() {
        let mut bytes = build_minimal_ani(1, 1);
        bytes[..4].copy_from_slice(b"XXXX");
        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(msg.contains("RIFF"), "{msg}"),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn rejects_wrong_form_type() {
        let mut bytes = build_minimal_ani(1, 1);
        bytes[8..12].copy_from_slice(b"WAVE");
        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(msg.contains("ACON"), "{msg}"),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn rejects_truncated_riff_declared_size() {
        let mut bytes = build_minimal_ani(1, 1);
        // Inflate the RIFF size by 1024 bytes — body claims to be
        // longer than the file actually is.
        let bumped = (bytes.len() - 8 + 1024) as u32;
        bytes[4..8].copy_from_slice(&bumped.to_le_bytes());
        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(msg.contains("exceeds"), "{msg}"),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn rejects_input_shorter_than_riff_header() {
        let bytes = vec![b'R', b'I', b'F', b'F', 0, 0, 0];
        assert!(read_ani_raw(&bytes).is_err());
    }

    #[test]
    fn rejects_missing_anih() {
        // Synthesise a RIFF/ACON wrapper carrying only an empty
        // LIST 'fram' — no anih chunk at all.
        let mut fram_body = Vec::new();
        fram_body.extend_from_slice(b"fram");
        let mut fram_chunk = Vec::new();
        fram_chunk.extend_from_slice(b"LIST");
        fram_chunk.extend_from_slice(&(fram_body.len() as u32).to_le_bytes());
        fram_chunk.extend_from_slice(&fram_body);

        let mut body = Vec::new();
        body.extend_from_slice(b"ACON");
        body.extend_from_slice(&fram_chunk);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&body);

        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("anih") || msg.contains("before anih"), "{msg}")
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn rejects_missing_fram_list() {
        // Build an ACON file with only an `anih` chunk — no LIST 'fram'.
        let mut anih_payload = vec![0u8; 36];
        anih_payload[0..4].copy_from_slice(&36u32.to_le_bytes());
        anih_payload[4..8].copy_from_slice(&2u32.to_le_bytes()); // nFrames
        anih_payload[24..28].copy_from_slice(&1u32.to_le_bytes()); // nPlanes
        let mut anih_chunk = Vec::new();
        anih_chunk.extend_from_slice(b"anih");
        anih_chunk.extend_from_slice(&(anih_payload.len() as u32).to_le_bytes());
        anih_chunk.extend_from_slice(&anih_payload);

        let mut body = Vec::new();
        body.extend_from_slice(b"ACON");
        body.extend_from_slice(&anih_chunk);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&body);

        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(msg.contains("fram"), "{msg}"),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn rejects_frame_count_mismatch() {
        // anih declares 4 frames but LIST 'fram' carries 3.
        let mut bytes = build_minimal_ani(3, 3);
        // anih.nFrames sits at body offset (RIFF8 + ACON4 + anih
        // chunk_header8) + 4 → 8 + 4 + 8 + 4 = 24.
        bytes[24..28].copy_from_slice(&4u32.to_le_bytes());
        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(msg.contains("frames"), "{msg}"),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn rejects_zero_frame_anih() {
        // Build the file then zero out nFrames in the anih payload.
        let mut bytes = build_minimal_ani(1, 1);
        bytes[24..28].copy_from_slice(&0u32.to_le_bytes());
        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(msg.contains("nFrames"), "{msg}"),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn rejects_pathological_frame_count() {
        let mut bytes = build_minimal_ani(1, 1);
        // Bump nFrames to 5 million — well past MAX_FRAMES_OR_STEPS.
        bytes[24..28].copy_from_slice(&5_000_000u32.to_le_bytes());
        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(msg.contains("sanity cap"), "{msg}"),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn rejects_anih_n_planes_above_one() {
        // Per spec the `anih.nPlanes` field is "= 1". Mirror the
        // BMP-body `biPlanes ∈ {0, 1}` strictness on the ICO path:
        // accept 0 (the wider-ecosystem "unspecified" tolerance) and
        // 1 (canonical), reject anything else as a probe-vs-render
        // mismatch.
        //
        // anih.nPlanes sits at anih_payload offset 24, which is file
        // offset (RIFF8 + ACON4 + "anih"4 + size4) + 24 = 44.
        let mut bytes = build_minimal_ani(1, 1);
        bytes[44..48].copy_from_slice(&7u32.to_le_bytes());
        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("nPlanes") && msg.contains("must be 0 or 1"),
                "{msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn accepts_anih_n_planes_zero_tolerance() {
        // The wider ICO/ANI ecosystem tolerates `nPlanes = 0`
        // ("unspecified — defer to the frame headers"). We mirror the
        // BMP-body `biPlanes` carve-out and accept it.
        let mut bytes = build_minimal_ani(1, 1);
        bytes[44..48].copy_from_slice(&0u32.to_le_bytes());
        let parsed = read_ani_raw(&bytes).unwrap();
        assert_eq!(parsed.header.n_planes, 0);
    }

    #[test]
    fn rejects_anih_i_width_above_256() {
        // ICO/CUR sub-images are bounded to 1..=256 in either axis; an
        // `anih.iWidth` outside that range would either be silently
        // ignored or sized a renderer allocation past anything real.
        // anih.iWidth sits at anih_payload offset 12 → file offset
        // (RIFF8 + ACON4 + "anih"4 + size4) + 12 = 32.
        let mut bytes = build_minimal_ani(1, 1);
        bytes[32..36].copy_from_slice(&257u32.to_le_bytes());
        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("iWidth") && msg.contains("1..=256"), "{msg}")
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn rejects_anih_i_width_pathological_u32() {
        // Adversarial `0xFFFF_FFFF` is the classic "size pulled from
        // user-controlled bytes" smuggling shape — make sure we catch
        // it at parse time.
        let mut bytes = build_minimal_ani(1, 1);
        bytes[32..36].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(msg.contains("iWidth"), "{msg}"),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn rejects_anih_i_height_above_256() {
        // anih.iHeight sits at anih_payload offset 16 → file offset
        // 8 + 4 + 4 + 4 + 16 = 36.
        let mut bytes = build_minimal_ani(1, 1);
        bytes[36..40].copy_from_slice(&512u32.to_le_bytes());
        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("iHeight") && msg.contains("1..=256"), "{msg}")
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn accepts_anih_i_width_height_zero_tolerance() {
        // Spec's "0 = take from frame" sentinel — must be accepted as
        // the absent-value case (and is the default in the minimal
        // builder, but we re-assert it here to lock down the contract
        // against future drift of the validator).
        let mut bytes = build_minimal_ani(1, 1);
        bytes[32..36].copy_from_slice(&0u32.to_le_bytes());
        bytes[36..40].copy_from_slice(&0u32.to_le_bytes());
        let parsed = read_ani_raw(&bytes).unwrap();
        assert_eq!(parsed.header.i_width, 0);
        assert_eq!(parsed.header.i_height, 0);
    }

    #[test]
    fn accepts_anih_i_width_height_at_256_boundary() {
        // 256 is the spec maximum — assert the boundary is inclusive
        // rather than exclusive (an off-by-one in the validator would
        // reject the canonical large-cursor case).
        let mut bytes = build_minimal_ani(1, 1);
        bytes[32..36].copy_from_slice(&256u32.to_le_bytes());
        bytes[36..40].copy_from_slice(&256u32.to_le_bytes());
        let parsed = read_ani_raw(&bytes).unwrap();
        assert_eq!(parsed.header.i_width, 256);
        assert_eq!(parsed.header.i_height, 256);
    }

    #[test]
    fn rejects_anih_i_bit_count_outside_canonical_set() {
        // anih.iBitCount sits at anih_payload offset 20 → file offset
        // 8 + 4 + 4 + 4 + 20 = 40. The ICO/CUR sub-image bit-depth set
        // is `{0, 1, 4, 8, 16, 24, 32}`; `7 bpp` doesn't correspond to
        // any renderable DIB layout.
        let mut bytes = build_minimal_ani(1, 1);
        bytes[40..44].copy_from_slice(&7u32.to_le_bytes());
        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("iBitCount") && msg.contains("must be one of"),
                "{msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn rejects_anih_i_bit_count_64() {
        // 64-bpp doesn't exist in the ICO/CUR family — make sure a
        // high-but-plausible-looking value is still rejected.
        let mut bytes = build_minimal_ani(1, 1);
        bytes[40..44].copy_from_slice(&64u32.to_le_bytes());
        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(msg.contains("iBitCount"), "{msg}"),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn accepts_anih_i_bit_count_canonical_values() {
        // Every canonical bit-depth must round-trip the parser.
        for bpp in [0u32, 1, 4, 8, 16, 24, 32] {
            let mut bytes = build_minimal_ani(1, 1);
            bytes[40..44].copy_from_slice(&bpp.to_le_bytes());
            let parsed = read_ani_raw(&bytes).expect("canonical bpp must parse");
            assert_eq!(parsed.header.i_bit_count, bpp);
        }
    }

    #[test]
    fn parses_seq_and_rate_chunks() {
        // Build a 2-frame, 4-step animation with explicit seq / rate.
        let mut anih_payload = vec![0u8; 36];
        anih_payload[0..4].copy_from_slice(&36u32.to_le_bytes());
        anih_payload[4..8].copy_from_slice(&2u32.to_le_bytes()); // nFrames
        anih_payload[8..12].copy_from_slice(&4u32.to_le_bytes()); // nSteps
        anih_payload[24..28].copy_from_slice(&1u32.to_le_bytes()); // nPlanes
        anih_payload[28..32].copy_from_slice(&60u32.to_le_bytes()); // iDispRate
        anih_payload[32..36].copy_from_slice(&(AF_ICON | AF_SEQUENCE).to_le_bytes());
        let mut anih_chunk = Vec::new();
        anih_chunk.extend_from_slice(b"anih");
        anih_chunk.extend_from_slice(&(anih_payload.len() as u32).to_le_bytes());
        anih_chunk.extend_from_slice(&anih_payload);

        // rate: 4 LE u32s.
        let rate_payload: Vec<u8> = [10u32, 20, 30, 40]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let mut rate_chunk = Vec::new();
        rate_chunk.extend_from_slice(b"rate");
        rate_chunk.extend_from_slice(&(rate_payload.len() as u32).to_le_bytes());
        rate_chunk.extend_from_slice(&rate_payload);

        // seq : 4 LE u32s — replay frame 0,1,0,1.
        let seq_payload: Vec<u8> = [0u32, 1, 0, 1]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let mut seq_chunk = Vec::new();
        seq_chunk.extend_from_slice(b"seq ");
        seq_chunk.extend_from_slice(&(seq_payload.len() as u32).to_le_bytes());
        seq_chunk.extend_from_slice(&seq_payload);

        // LIST 'fram' with 2 icon frames.
        let mut fram_body = Vec::new();
        fram_body.extend_from_slice(b"fram");
        for i in 0..2u8 {
            let payload = [b'F', i];
            fram_body.extend_from_slice(b"icon");
            fram_body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            fram_body.extend_from_slice(&payload);
            // Odd-length payload (2 bytes is even) — no pad needed.
        }
        let mut fram_chunk = Vec::new();
        fram_chunk.extend_from_slice(b"LIST");
        fram_chunk.extend_from_slice(&(fram_body.len() as u32).to_le_bytes());
        fram_chunk.extend_from_slice(&fram_body);

        let mut body = Vec::new();
        body.extend_from_slice(b"ACON");
        body.extend_from_slice(&anih_chunk);
        body.extend_from_slice(&rate_chunk);
        body.extend_from_slice(&seq_chunk);
        body.extend_from_slice(&fram_chunk);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&body);

        let parsed = read_ani_raw(&bytes).unwrap();
        assert_eq!(parsed.header.n_frames, 2);
        assert_eq!(parsed.header.n_steps, 4);
        assert!(parsed.header.frames_are_icons());
        assert!(parsed.header.has_sequence_flag());
        assert_eq!(parsed.rates.as_ref().unwrap(), &vec![10u32, 20, 30, 40]);
        assert_eq!(parsed.sequence.as_ref().unwrap(), &vec![0u32, 1, 0, 1]);
        assert_eq!(parsed.frames.len(), 2);
        assert_eq!(parsed.frames[0], vec![b'F', 0]);
        assert_eq!(parsed.frames[1], vec![b'F', 1]);
    }

    #[test]
    fn parses_info_list_inam_iart() {
        // LIST 'INFO' { INAM "Title", IART "Author" } injected
        // between anih and LIST 'fram'.
        let inam_payload = b"Title\0";
        let iart_payload = b"Author\0";
        let mut info_body = Vec::new();
        info_body.extend_from_slice(b"INFO");
        info_body.extend_from_slice(b"INAM");
        info_body.extend_from_slice(&(inam_payload.len() as u32).to_le_bytes());
        info_body.extend_from_slice(inam_payload);
        // INAM is 6 bytes (even) — no pad.
        info_body.extend_from_slice(b"IART");
        info_body.extend_from_slice(&(iart_payload.len() as u32).to_le_bytes());
        info_body.extend_from_slice(iart_payload);
        // IART is 7 bytes (odd) — pad one byte.
        info_body.push(0);

        let mut info_chunk = Vec::new();
        info_chunk.extend_from_slice(b"LIST");
        info_chunk.extend_from_slice(&((info_body.len()) as u32).to_le_bytes());
        info_chunk.extend_from_slice(&info_body);

        // Combine anih + INFO + fram via the minimal builder's RIFF
        // wrapper.
        let mut base = build_minimal_ani(1, 1);
        // Splice the INFO chunk in immediately after the anih chunk.
        // The minimal layout is:
        //   bytes[0..8]  = RIFF + size
        //   bytes[8..12] = ACON
        //   bytes[12..]  = anih chunk (8 + 36 = 44 bytes) then LIST fram
        let anih_end = 12 + 8 + 36;
        base.splice(anih_end..anih_end, info_chunk.iter().copied());
        // Recompute the RIFF outer size.
        let new_size = (base.len() - 8) as u32;
        base[4..8].copy_from_slice(&new_size.to_le_bytes());

        let parsed = read_ani_raw(&base).unwrap();
        assert_eq!(parsed.info.title.as_deref(), Some(inam_payload.as_slice()));
        assert_eq!(parsed.info.author.as_deref(), Some(iart_payload.as_slice()));
    }

    #[test]
    fn rejects_fram_list_with_unexpected_chunk() {
        // Manually craft a LIST 'fram' that contains a stray
        // non-icon chunk.
        let mut anih_payload = vec![0u8; 36];
        anih_payload[0..4].copy_from_slice(&36u32.to_le_bytes());
        anih_payload[4..8].copy_from_slice(&1u32.to_le_bytes());
        anih_payload[24..28].copy_from_slice(&1u32.to_le_bytes());
        let mut anih_chunk = Vec::new();
        anih_chunk.extend_from_slice(b"anih");
        anih_chunk.extend_from_slice(&(anih_payload.len() as u32).to_le_bytes());
        anih_chunk.extend_from_slice(&anih_payload);

        let mut fram_body = Vec::new();
        fram_body.extend_from_slice(b"fram");
        // A "junk" chunk where the spec mandates "icon".
        fram_body.extend_from_slice(b"junk");
        fram_body.extend_from_slice(&4u32.to_le_bytes());
        fram_body.extend_from_slice(&[0, 0, 0, 0]);
        let mut fram_chunk = Vec::new();
        fram_chunk.extend_from_slice(b"LIST");
        fram_chunk.extend_from_slice(&(fram_body.len() as u32).to_le_bytes());
        fram_chunk.extend_from_slice(&fram_body);

        let mut body = Vec::new();
        body.extend_from_slice(b"ACON");
        body.extend_from_slice(&anih_chunk);
        body.extend_from_slice(&fram_chunk);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&body);

        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(msg.contains("unexpected"), "{msg}"),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn rejects_chunk_running_past_parent() {
        // Build a normal file then over-inflate the LIST 'fram'
        // size field so its declared length exceeds what's actually
        // there.
        let mut bytes = build_minimal_ani(2, 2);
        // Locate LIST 'fram' size field. Layout: RIFF8 + ACON4 +
        // anih chunk (8 + 36) = 56; LIST tag at 56, size at 60.
        let list_size_off = 56 + 4;
        bytes[list_size_off..list_size_off + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        let err = read_ani_raw(&bytes).unwrap_err();
        assert!(matches!(err, Error::InvalidData(_)));
    }

    #[test]
    fn handles_odd_length_icon_payload_padding() {
        // Custom-build: 1 frame with a 5-byte payload (odd → RIFF
        // pads with 1 zero byte before the next chunk). Confirms
        // the chunk-walker honours the spec's even-length padding
        // rule and the next chunk lookup doesn't drift by one.
        let mut anih_payload = vec![0u8; 36];
        anih_payload[0..4].copy_from_slice(&36u32.to_le_bytes());
        anih_payload[4..8].copy_from_slice(&1u32.to_le_bytes()); // nFrames=1
        anih_payload[24..28].copy_from_slice(&1u32.to_le_bytes()); // nPlanes
        let mut anih_chunk = Vec::new();
        anih_chunk.extend_from_slice(b"anih");
        anih_chunk.extend_from_slice(&(anih_payload.len() as u32).to_le_bytes());
        anih_chunk.extend_from_slice(&anih_payload);

        // Odd-length (5 bytes) icon payload + pad inside LIST 'fram'.
        let frame_payload = [b'P', b'A', b'D', b'D', b'Y'];
        let mut fram_body = Vec::new();
        fram_body.extend_from_slice(b"fram");
        fram_body.extend_from_slice(b"icon");
        fram_body.extend_from_slice(&(frame_payload.len() as u32).to_le_bytes());
        fram_body.extend_from_slice(&frame_payload);
        fram_body.push(0); // even-pad

        let mut fram_chunk = Vec::new();
        fram_chunk.extend_from_slice(b"LIST");
        fram_chunk.extend_from_slice(&(fram_body.len() as u32).to_le_bytes());
        fram_chunk.extend_from_slice(&fram_body);

        let mut body = Vec::new();
        body.extend_from_slice(b"ACON");
        body.extend_from_slice(&anih_chunk);
        body.extend_from_slice(&fram_chunk);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&body);

        let parsed = read_ani_raw(&bytes).unwrap();
        assert_eq!(parsed.frames.len(), 1);
        assert_eq!(parsed.frames[0], frame_payload.to_vec());
    }

    #[test]
    fn rejects_seq_chunk_before_anih() {
        // seq ahead of anih has no nSteps to validate against; must
        // not be silently accepted.
        let mut seq_chunk = Vec::new();
        seq_chunk.extend_from_slice(b"seq ");
        seq_chunk.extend_from_slice(&4u32.to_le_bytes());
        seq_chunk.extend_from_slice(&0u32.to_le_bytes());

        let mut body = Vec::new();
        body.extend_from_slice(b"ACON");
        body.extend_from_slice(&seq_chunk);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&body);

        let err = read_ani_raw(&bytes).unwrap_err();
        assert!(matches!(err, Error::InvalidData(_)));
    }

    #[test]
    fn ani_frame_payload_round_trips_through_read_ico_raw() {
        // End-to-end: build an ANI whose frames are full ICO files
        // (as the spec mandates when AF_ICON is set), then confirm
        // `read_ico_raw` re-parses each `parsed.frames[i]` as a
        // valid ICO. This is the contract the docstring promises:
        // "ready to feed to read_ico_raw when frames_are_icons()".
        use crate::types::{HotSpot, IconSubFormat, IconType};
        use crate::{read_ico_raw, write_ico_raw, IconEntryRaw};

        // Two distinct ICO frames — one ICO, one CUR with hotspot —
        // each carrying a PNG-magic-prefixed payload (the raw layer
        // doesn't validate PNG body, just the magic-sniff bit).
        let frame0_bytes = write_ico_raw(
            IconType::Ico,
            &[IconEntryRaw {
                width: 16,
                height: 16,
                bit_depth: 32,
                sub_format: IconSubFormat::Png,
                hotspot: None,
                data: vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
            }],
        )
        .unwrap();
        let frame1_bytes = write_ico_raw(
            IconType::Cur,
            &[IconEntryRaw {
                width: 32,
                height: 32,
                bit_depth: 32,
                sub_format: IconSubFormat::Png,
                hotspot: Some(HotSpot { x: 5, y: 7 }),
                data: vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
            }],
        )
        .unwrap();

        // Hand-assemble ACON around those two real ICO frames.
        let mut anih_payload = vec![0u8; 36];
        anih_payload[0..4].copy_from_slice(&36u32.to_le_bytes());
        anih_payload[4..8].copy_from_slice(&2u32.to_le_bytes()); // nFrames=2
        anih_payload[24..28].copy_from_slice(&1u32.to_le_bytes()); // nPlanes
        anih_payload[28..32].copy_from_slice(&8u32.to_le_bytes()); // iDispRate
        anih_payload[32..36].copy_from_slice(&AF_ICON.to_le_bytes());
        let mut anih_chunk = Vec::new();
        anih_chunk.extend_from_slice(b"anih");
        anih_chunk.extend_from_slice(&(anih_payload.len() as u32).to_le_bytes());
        anih_chunk.extend_from_slice(&anih_payload);

        let mut fram_body = Vec::new();
        fram_body.extend_from_slice(b"fram");
        for fb in [&frame0_bytes, &frame1_bytes] {
            fram_body.extend_from_slice(b"icon");
            fram_body.extend_from_slice(&(fb.len() as u32).to_le_bytes());
            fram_body.extend_from_slice(fb);
            if fb.len() % 2 == 1 {
                fram_body.push(0);
            }
        }
        let mut fram_chunk = Vec::new();
        fram_chunk.extend_from_slice(b"LIST");
        fram_chunk.extend_from_slice(&(fram_body.len() as u32).to_le_bytes());
        fram_chunk.extend_from_slice(&fram_body);

        let mut body = Vec::new();
        body.extend_from_slice(b"ACON");
        body.extend_from_slice(&anih_chunk);
        body.extend_from_slice(&fram_chunk);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&body);

        let parsed = read_ani_raw(&bytes).unwrap();
        assert_eq!(parsed.frames.len(), 2);
        assert!(parsed.header.frames_are_icons());

        let (ty0, entries0) = read_ico_raw(&parsed.frames[0]).unwrap();
        assert_eq!(ty0, IconType::Ico);
        assert_eq!(entries0[0].width, 16);

        let (ty1, entries1) = read_ico_raw(&parsed.frames[1]).unwrap();
        assert_eq!(ty1, IconType::Cur);
        assert_eq!(entries1[0].hotspot, Some(HotSpot { x: 5, y: 7 }));
    }

    #[test]
    fn read_ico_raw_pointers_to_ani_helper() {
        // The legacy ICO entry point still rejects ANI input, but
        // the error message now points at the new helper rather
        // than dead-ending the caller. Belt-and-braces check that
        // the cross-reference doesn't drift.
        let bytes = build_minimal_ani(1, 1);
        let err = crate::read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::Unsupported(msg) => {
                assert!(msg.contains(".ani"), "{msg}");
                assert!(msg.contains("read_ani_raw"), "{msg}");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn accepts_trailing_data_after_riff_body() {
        // Some encoders append slack bytes after the declared RIFF
        // body — accept those (don't reject a longer buffer than
        // declared).
        let mut bytes = build_minimal_ani(1, 1);
        bytes.extend_from_slice(b"trailing-slack-bytes");
        let parsed = read_ani_raw(&bytes).unwrap();
        assert_eq!(parsed.header.n_frames, 1);
        assert_eq!(parsed.frames.len(), 1);
    }

    /// Build a 2-frame ANI with an explicit `seq ` chunk carrying the
    /// caller's 4 step indices. Mirrors `parses_seq_and_rate_chunks`
    /// but factored out so the out-of-range tests stay focused.
    fn build_ani_with_seq(n_frames: u32, n_steps: u32, seq_indices: &[u32]) -> Vec<u8> {
        let mut anih_payload = vec![0u8; 36];
        anih_payload[0..4].copy_from_slice(&36u32.to_le_bytes());
        anih_payload[4..8].copy_from_slice(&n_frames.to_le_bytes());
        anih_payload[8..12].copy_from_slice(&n_steps.to_le_bytes());
        anih_payload[24..28].copy_from_slice(&1u32.to_le_bytes()); // nPlanes
        anih_payload[28..32].copy_from_slice(&10u32.to_le_bytes()); // iDispRate
        anih_payload[32..36].copy_from_slice(&(AF_ICON | AF_SEQUENCE).to_le_bytes());
        let mut anih_chunk = Vec::new();
        anih_chunk.extend_from_slice(b"anih");
        anih_chunk.extend_from_slice(&(anih_payload.len() as u32).to_le_bytes());
        anih_chunk.extend_from_slice(&anih_payload);

        let seq_payload: Vec<u8> = seq_indices.iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut seq_chunk = Vec::new();
        seq_chunk.extend_from_slice(b"seq ");
        seq_chunk.extend_from_slice(&(seq_payload.len() as u32).to_le_bytes());
        seq_chunk.extend_from_slice(&seq_payload);

        let mut fram_body = Vec::new();
        fram_body.extend_from_slice(b"fram");
        for i in 0..n_frames {
            let frame_payload = [b'F', b'R', b'M', i as u8];
            fram_body.extend_from_slice(b"icon");
            fram_body.extend_from_slice(&(frame_payload.len() as u32).to_le_bytes());
            fram_body.extend_from_slice(&frame_payload);
        }
        let mut fram_chunk = Vec::new();
        fram_chunk.extend_from_slice(b"LIST");
        fram_chunk.extend_from_slice(&(fram_body.len() as u32).to_le_bytes());
        fram_chunk.extend_from_slice(&fram_body);

        let mut body = Vec::new();
        body.extend_from_slice(b"ACON");
        body.extend_from_slice(&anih_chunk);
        body.extend_from_slice(&seq_chunk);
        body.extend_from_slice(&fram_chunk);

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn rejects_seq_index_equal_to_n_frames() {
        // 2 frames → valid indices are {0, 1}. An entry == n_frames is
        // a one-past-the-end pointer, the classic off-by-one downstream
        // panic source.
        let bytes = build_ani_with_seq(2, 4, &[0, 1, 0, 2]);
        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("seq[3]"), "{msg}");
                assert!(msg.contains("out of range"), "{msg}");
                assert!(msg.contains("nFrames = 2"), "{msg}");
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn rejects_seq_index_far_past_n_frames() {
        // Pathological adversarial value — 0xFFFF_FFFF in the seq array
        // would index gigabytes off the end of the frame vector on any
        // renderer that dereferences blindly.
        let bytes = build_ani_with_seq(2, 4, &[0, 0xFFFF_FFFF, 0, 1]);
        let err = read_ani_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("seq[1]"), "{msg}");
                assert!(msg.contains("out of range"), "{msg}");
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn accepts_seq_with_all_indices_in_range() {
        // Sanity-check the positive path: all entries < n_frames is
        // accepted (and indices may legitimately repeat / play out of
        // storage order — that's the whole point of `seq `).
        let bytes = build_ani_with_seq(2, 4, &[0, 1, 1, 0]);
        let parsed = read_ani_raw(&bytes).unwrap();
        assert_eq!(parsed.sequence.as_ref().unwrap(), &vec![0u32, 1, 1, 0]);
    }

    // ------------------------------------------------------------------
    // `AniFile::playback_steps` — typed multi-step playback accessor.
    // ------------------------------------------------------------------

    /// Build an `AniFile` directly (skipping the byte parser) so the
    /// playback-table tests can exercise corner cases the read-side
    /// validator already rejects (zero-jiffy rates, mismatched
    /// sequence lengths, identity-fallback past nFrames).
    fn ani_with(
        n_frames: u32,
        n_steps: u32,
        i_disp_rate: u32,
        sequence: Option<Vec<u32>>,
        rates: Option<Vec<u32>>,
    ) -> AniFile {
        AniFile {
            header: AniHeader {
                cb_size: 36,
                n_frames,
                n_steps,
                i_width: 0,
                i_height: 0,
                i_bit_count: 0,
                n_planes: 1,
                i_disp_rate,
                bf_attributes: AF_ICON,
            },
            info: AniInfo::default(),
            sequence,
            rates,
            frames: (0..n_frames).map(|i| vec![b'F', i as u8]).collect(),
        }
    }

    #[test]
    fn resolved_step_count_falls_back_to_n_frames_when_zero() {
        // The spec's "nSteps = nFrames if no seq chunk" default —
        // surface it through the typed accessor.
        let a = ani_with(3, 0, 10, None, None);
        assert_eq!(a.resolved_step_count(), 3);
    }

    #[test]
    fn resolved_step_count_uses_n_steps_when_non_zero() {
        // n_steps != n_frames is legitimate when paired with a seq
        // chunk (a 2-frame file playing 5 steps).
        let a = ani_with(2, 5, 10, Some(vec![0, 1, 0, 1, 0]), None);
        assert_eq!(a.resolved_step_count(), 5);
    }

    #[test]
    fn playback_steps_identity_when_seq_absent() {
        // 3-frame file, no seq, no rate — every step is (i, default).
        let a = ani_with(3, 3, 7, None, None);
        let steps = a.playback_steps().unwrap();
        assert_eq!(
            steps,
            vec![
                AniStep {
                    frame_index: 0,
                    jiffies: 7
                },
                AniStep {
                    frame_index: 1,
                    jiffies: 7
                },
                AniStep {
                    frame_index: 2,
                    jiffies: 7
                },
            ]
        );
    }

    #[test]
    fn playback_steps_applies_seq_then_rates() {
        // 2 frames, 4 steps, explicit seq + rate — both override
        // applied at their respective positions.
        let a = ani_with(2, 4, 10, Some(vec![0, 1, 1, 0]), Some(vec![3, 4, 5, 6]));
        let steps = a.playback_steps().unwrap();
        assert_eq!(
            steps,
            vec![
                AniStep {
                    frame_index: 0,
                    jiffies: 3
                },
                AniStep {
                    frame_index: 1,
                    jiffies: 4
                },
                AniStep {
                    frame_index: 1,
                    jiffies: 5
                },
                AniStep {
                    frame_index: 0,
                    jiffies: 6
                },
            ]
        );
    }

    #[test]
    fn playback_steps_seq_only_falls_back_to_disp_rate() {
        // seq present, rate absent — every step gets the header's
        // default jiffies.
        let a = ani_with(2, 4, 12, Some(vec![0, 1, 1, 0]), None);
        let steps = a.playback_steps().unwrap();
        assert_eq!(
            steps.iter().map(|s| s.jiffies).collect::<Vec<_>>(),
            vec![12, 12, 12, 12]
        );
        assert_eq!(
            steps.iter().map(|s| s.frame_index).collect::<Vec<_>>(),
            vec![0, 1, 1, 0]
        );
    }

    #[test]
    fn playback_steps_rate_only_runs_identity_indices() {
        // rate present, seq absent — frame_index = i (identity),
        // jiffies = rates[i].
        let a = ani_with(3, 3, 99, None, Some(vec![1, 2, 3]));
        let steps = a.playback_steps().unwrap();
        assert_eq!(
            steps,
            vec![
                AniStep {
                    frame_index: 0,
                    jiffies: 1
                },
                AniStep {
                    frame_index: 1,
                    jiffies: 2
                },
                AniStep {
                    frame_index: 2,
                    jiffies: 3
                },
            ]
        );
    }

    #[test]
    fn playback_steps_rejects_zero_default_jiffies_no_rate_chunk() {
        // i_disp_rate = 0 and no rate chunk — every step resolves to
        // 0 jiffies. A poll-based renderer that does
        // `if elapsed >= rate[i]` would advance instantly and burn
        // 100% CPU; refuse rather than emit it.
        let a = ani_with(2, 2, 0, None, None);
        let err = a.playback_steps().unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("0 jiffies"), "{msg}");
                assert!(msg.contains("step 0"), "{msg}");
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn playback_steps_rejects_zero_rate_entry() {
        // Even when i_disp_rate is non-zero, a single zero-jiffy entry
        // in the rate chunk poisons the table at that step.
        let a = ani_with(2, 4, 10, None, Some(vec![5, 0, 5, 5]));
        let err = a.playback_steps().unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("step 1"), "{msg}");
                assert!(msg.contains("0 jiffies"), "{msg}");
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn playback_steps_rejects_identity_past_n_frames() {
        // Hand-constructed AniFile with n_steps > n_frames and no seq
        // chunk — identity step n_frames would index past the frame
        // vector. (The byte parser doesn't validate this combination
        // because the spec is silent on it; the accessor catches it
        // up front rather than letting a downstream `frames[i]`
        // panic.)
        let a = ani_with(2, 4, 10, None, None);
        let err = a.playback_steps().unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("identity step 2"), "{msg}");
                assert!(msg.contains("nFrames = 2"), "{msg}");
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn playback_steps_rejects_mismatched_sequence_length() {
        // Hand-constructed AniFile whose sequence array is shorter
        // than n_steps. Parser-produced files can't trip this (the
        // walker aligns the array to the chunk count), but a caller
        // constructing AniFile by hand could; refuse rather than
        // panic on out-of-bounds index.
        let a = ani_with(2, 4, 10, Some(vec![0, 1]), None);
        let err = a.playback_steps().unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("sequence len 2"), "{msg}");
                assert!(msg.contains("step count 4"), "{msg}");
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn playback_steps_rejects_mismatched_rates_length() {
        let a = ani_with(2, 4, 10, Some(vec![0, 1, 0, 1]), Some(vec![5, 6]));
        let err = a.playback_steps().unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("rates len 2"), "{msg}");
                assert!(msg.contains("step count 4"), "{msg}");
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn playback_steps_rejects_zero_n_frames() {
        // Sentinel — hand-constructed AniFile with n_frames = 0
        // would otherwise produce a zero-step table that masks the
        // bug. (Byte parser already rejects n_frames = 0; this
        // covers the hand-constructed path.)
        let a = ani_with(0, 0, 10, None, None);
        let err = a.playback_steps().unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(msg.contains("n_frames = 0"), "{msg}"),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn playback_steps_via_byte_parser_round_trip() {
        // End-to-end: real ANI bytes → read_ani_raw → playback_steps.
        // Confirms the parser's chunk validation is sufficient to
        // produce a typed table that requires no further per-chunk
        // checks at accessor time.
        let bytes = build_ani_with_seq(2, 4, &[0, 1, 1, 0]);
        let parsed = read_ani_raw(&bytes).unwrap();
        let steps = parsed.playback_steps().unwrap();
        assert_eq!(steps.len(), 4);
        assert_eq!(
            steps.iter().map(|s| s.frame_index).collect::<Vec<_>>(),
            vec![0, 1, 1, 0]
        );
        // build_ani_with_seq sets i_disp_rate = 10 and no rate chunk
        // → every step uses 10 jiffies.
        assert!(steps.iter().all(|s| s.jiffies == 10));
    }

    // ------------------------------------------------------------------
    // `AniFile::total_jiffies` — cycle-length typed accessor.
    // ------------------------------------------------------------------

    #[test]
    fn total_jiffies_uses_default_when_rate_absent() {
        // 3 steps × 7 jiffies/default = 21.
        let a = ani_with(3, 3, 7, None, None);
        assert_eq!(a.total_jiffies().unwrap(), 21u64);
    }

    #[test]
    fn total_jiffies_sums_rate_chunk_when_present() {
        // 3 + 4 + 5 + 6 = 18; i_disp_rate is ignored when rate is set.
        let a = ani_with(2, 4, 99, Some(vec![0, 1, 1, 0]), Some(vec![3, 4, 5, 6]));
        assert_eq!(a.total_jiffies().unwrap(), 18u64);
    }

    #[test]
    fn total_jiffies_n_steps_from_n_frames_default() {
        // header.n_steps = 0 → resolved step count = n_frames = 4.
        // 4 × 10 = 40.
        let a = ani_with(4, 0, 10, None, None);
        assert_eq!(a.total_jiffies().unwrap(), 40u64);
    }

    #[test]
    fn total_jiffies_widens_to_u64_without_overflow() {
        // Worst-case headroom: 65_536 steps × u32::MAX jiffies each
        // overflows a u32 by 16+ bits; the u64 sum holds it.
        // Use 4 steps × u32::MAX to keep the test allocation cheap
        // while still asserting the widening (4 × 0xFFFF_FFFF
        // = 0x3_FFFF_FFFC, which exceeds u32::MAX).
        let big = u32::MAX;
        let a = ani_with(4, 4, 0, None, Some(vec![big, big, big, big]));
        let expected = (big as u64) * 4;
        assert_eq!(a.total_jiffies().unwrap(), expected);
        assert!(expected > u32::MAX as u64, "expected exceeds u32 range");
    }

    #[test]
    fn total_jiffies_rejects_zero_default_no_rate_chunk() {
        // i_disp_rate = 0 and no rate chunk → every step is 0 jiffies.
        // Matches the playback_steps zero-jiffy rejection contract.
        let a = ani_with(2, 2, 0, None, None);
        let err = a.total_jiffies().unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("0 jiffies"), "{msg}");
                assert!(msg.contains("step 0"), "{msg}");
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn total_jiffies_rejects_zero_rate_entry() {
        // A single zero-jiffy step in an otherwise-valid rate chunk
        // poisons the cycle: the renderer would either spin or
        // divide-by-zero at that step, and folding the entry into a
        // smaller-than-real total would mask the bug.
        let a = ani_with(2, 4, 10, None, Some(vec![5, 0, 5, 5]));
        let err = a.total_jiffies().unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("step 1"), "{msg}");
                assert!(msg.contains("0 jiffies"), "{msg}");
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn total_jiffies_rejects_mismatched_rates_length() {
        // Hand-constructed AniFile whose rates array is shorter than
        // the resolved step count. Parser-produced files can't trip
        // this (the walker aligns the array to n_steps), but a caller
        // building the struct by hand could.
        let a = ani_with(2, 4, 10, Some(vec![0, 1, 0, 1]), Some(vec![5, 6]));
        let err = a.total_jiffies().unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("rates len 2"), "{msg}");
                assert!(msg.contains("step count 4"), "{msg}");
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn total_jiffies_rejects_zero_n_frames() {
        // Sentinel: hand-constructed AniFile with n_frames = 0.
        let a = ani_with(0, 0, 10, None, None);
        let err = a.total_jiffies().unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(msg.contains("n_frames = 0"), "{msg}"),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn total_jiffies_ignores_sequence_chunk() {
        // The per-step duration depends only on the step index, not on
        // the frame the step picks. Two files with the same rate /
        // default / step_count but different sequence arrays must yield
        // the same total — surface this invariant.
        let a = ani_with(3, 5, 7, Some(vec![0, 1, 2, 0, 1]), None);
        let b = ani_with(3, 5, 7, Some(vec![2, 2, 2, 2, 2]), None);
        assert_eq!(a.total_jiffies().unwrap(), 35);
        assert_eq!(b.total_jiffies().unwrap(), 35);
    }

    #[test]
    fn total_jiffies_matches_playback_steps_sum() {
        // Cross-check: the total returned here must equal the per-step
        // sum derived from playback_steps(). Catches the case where one
        // accessor drifts away from the other under future maintenance.
        let a = ani_with(2, 4, 10, Some(vec![0, 1, 1, 0]), Some(vec![3, 4, 5, 6]));
        let steps = a.playback_steps().unwrap();
        let by_hand: u64 = steps.iter().map(|s| s.jiffies as u64).sum();
        assert_eq!(a.total_jiffies().unwrap(), by_hand);
    }

    #[test]
    fn total_jiffies_via_byte_parser_round_trip() {
        // End-to-end: real ANI bytes → read_ani_raw → total_jiffies.
        // build_ani_with_seq sets i_disp_rate = 10 and no rate chunk →
        // total = 4 steps × 10 = 40.
        let bytes = build_ani_with_seq(2, 4, &[0, 1, 1, 0]);
        let parsed = read_ani_raw(&bytes).unwrap();
        assert_eq!(parsed.total_jiffies().unwrap(), 40u64);
    }

    // ------------------------------------------------------------------
    // `AniFile::cycle_seconds` — wall-clock typed accessor.
    // ------------------------------------------------------------------

    #[test]
    fn cycle_seconds_uses_default_when_rate_absent() {
        // 6 jiffies × 60 = 360, divided by 60.0 = 6.0 s — a clean
        // 1-second-per-step file (i_disp_rate = 60 jiffies = 1 s).
        let a = ani_with(3, 3, 60, None, None);
        // 3 steps × 60 jiffies / 60 = 3.0 s.
        assert_eq!(a.cycle_seconds().unwrap(), 3.0);
    }

    #[test]
    fn cycle_seconds_sums_rate_chunk_when_present() {
        // 30 + 60 + 90 + 120 = 300 jiffies → 5.0 s wall-clock.
        let a = ani_with(
            2,
            4,
            99,
            Some(vec![0, 1, 1, 0]),
            Some(vec![30, 60, 90, 120]),
        );
        assert_eq!(a.cycle_seconds().unwrap(), 5.0);
    }

    #[test]
    fn cycle_seconds_matches_total_jiffies_div_60() {
        // Invariant under future maintenance: the wall-clock accessor must
        // equal total_jiffies / 60.0 by construction. The non-integer
        // jiffy counts here (3+4+5+6 = 18 jiffies → 0.3 s) catch the case
        // where one accessor drifts from the other.
        let a = ani_with(2, 4, 10, None, Some(vec![3, 4, 5, 6]));
        let jiffies = a.total_jiffies().unwrap();
        let seconds = a.cycle_seconds().unwrap();
        assert_eq!(seconds, jiffies as f64 / 60.0);
        assert_eq!(seconds, 0.3);
    }

    #[test]
    fn cycle_seconds_non_integer_jiffies_result() {
        // 7 jiffies / 60 = 0.11666... — surface the case where the
        // jiffy total isn't a clean multiple of 60. The f64 result is
        // the exact rational `7/60`, which round-trips back through
        // multiplication.
        let a = ani_with(1, 1, 7, None, None);
        let seconds = a.cycle_seconds().unwrap();
        assert_eq!(seconds, 7.0 / 60.0);
        // And one full second = 60 jiffies.
        let one_sec = ani_with(1, 1, 60, None, None);
        assert_eq!(one_sec.cycle_seconds().unwrap(), 1.0);
    }

    #[test]
    fn cycle_seconds_widens_exactly_within_f64_precision() {
        // Same widening-bound argument as total_jiffies: 4 × u32::MAX is
        // ~1.7e10, well under 2^53 ≈ 9.0e15. The cycle_seconds f64 must
        // therefore exactly equal `total_jiffies as f64 / 60.0` with no
        // precision loss surfacing at this scale.
        let big = u32::MAX;
        let a = ani_with(4, 4, 0, None, Some(vec![big, big, big, big]));
        let jiffies = a.total_jiffies().unwrap();
        let seconds = a.cycle_seconds().unwrap();
        assert_eq!(seconds, jiffies as f64 / 60.0);
        // And the multiplication round-trip is exact (no surprise rounding):
        // assert that `(seconds * 60.0) as u64` recovers the jiffy total.
        // Cast through u64 since jiffies fits comfortably in the f64
        // integer-precision range.
        assert_eq!((seconds * 60.0) as u64, jiffies);
    }

    #[test]
    fn cycle_seconds_rejects_zero_default_no_rate_chunk() {
        // Reuses total_jiffies' zero-jiffy rejection contract — surface
        // it through the wall-clock accessor so a caller wrapping the
        // file in a renderer can rely on the same up-front guarantee.
        let a = ani_with(2, 2, 0, None, None);
        let err = a.cycle_seconds().unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("0 jiffies"), "{msg}");
                assert!(msg.contains("total_jiffies"), "{msg}");
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn cycle_seconds_rejects_zero_rate_entry() {
        // Same zero-rate-step rejection as total_jiffies — a renderer
        // building "seconds until next cycle" math against a zero step
        // would either spin or divide-by-zero.
        let a = ani_with(2, 4, 10, None, Some(vec![5, 0, 5, 5]));
        let err = a.cycle_seconds().unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("step 1"), "{msg}");
                assert!(msg.contains("0 jiffies"), "{msg}");
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn cycle_seconds_rejects_zero_n_frames() {
        // Sentinel branch: hand-constructed AniFile with no frames.
        let a = ani_with(0, 0, 10, None, None);
        let err = a.cycle_seconds().unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(msg.contains("n_frames = 0"), "{msg}"),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn cycle_seconds_via_byte_parser_round_trip() {
        // End-to-end: real ANI bytes → read_ani_raw → cycle_seconds.
        // build_ani_with_seq sets i_disp_rate = 10 and no rate chunk →
        // total = 4 × 10 = 40 jiffies → 40 / 60 = 2/3 s.
        let bytes = build_ani_with_seq(2, 4, &[0, 1, 1, 0]);
        let parsed = read_ani_raw(&bytes).unwrap();
        assert_eq!(parsed.cycle_seconds().unwrap(), 40.0 / 60.0);
    }

    // ------------------------------------------------------------------
    // `AniFile::step_at_jiffy` — wall-clock-to-step typed lookup.
    // ------------------------------------------------------------------

    #[test]
    fn step_at_jiffy_uniform_rate_buckets_evenly() {
        // 4 steps × 10 jiffies each = 40-jiffy cycle. Boundaries land at
        // 0, 10, 20, 30, with step `i` covering `[10*i, 10*(i+1))`.
        let a = ani_with(4, 4, 10, None, None);
        assert_eq!(a.step_at_jiffy(0).unwrap(), 0);
        assert_eq!(a.step_at_jiffy(9).unwrap(), 0);
        assert_eq!(a.step_at_jiffy(10).unwrap(), 1);
        assert_eq!(a.step_at_jiffy(19).unwrap(), 1);
        assert_eq!(a.step_at_jiffy(20).unwrap(), 2);
        assert_eq!(a.step_at_jiffy(29).unwrap(), 2);
        assert_eq!(a.step_at_jiffy(30).unwrap(), 3);
        assert_eq!(a.step_at_jiffy(39).unwrap(), 3);
    }

    #[test]
    fn step_at_jiffy_variable_rate_buckets_per_step() {
        // 4 steps with rates 3, 4, 5, 6 — cumulative boundaries at
        // 3, 7, 12, 18. Step 0 covers [0,3), step 1 [3,7), step 2 [7,12),
        // step 3 [12,18).
        let a = ani_with(2, 4, 99, Some(vec![0, 1, 1, 0]), Some(vec![3, 4, 5, 6]));
        assert_eq!(a.step_at_jiffy(0).unwrap(), 0);
        assert_eq!(a.step_at_jiffy(2).unwrap(), 0);
        assert_eq!(a.step_at_jiffy(3).unwrap(), 1);
        assert_eq!(a.step_at_jiffy(6).unwrap(), 1);
        assert_eq!(a.step_at_jiffy(7).unwrap(), 2);
        assert_eq!(a.step_at_jiffy(11).unwrap(), 2);
        assert_eq!(a.step_at_jiffy(12).unwrap(), 3);
        assert_eq!(a.step_at_jiffy(17).unwrap(), 3);
    }

    #[test]
    fn step_at_jiffy_boundary_jiffy_value_flips_to_next_step() {
        // The half-open `[start, end)` interval contract: a jiffy value
        // exactly equal to step 0's duration must land on step 1, not
        // step 0. This is the edge that catches an off-by-one
        // `jiffy <= cumulative` check.
        let a = ani_with(3, 3, 5, None, None);
        assert_eq!(a.step_at_jiffy(4).unwrap(), 0);
        assert_eq!(a.step_at_jiffy(5).unwrap(), 1);
        assert_eq!(a.step_at_jiffy(10).unwrap(), 2);
    }

    #[test]
    fn step_at_jiffy_zero_is_step_zero() {
        // Sanity: jiffy = 0 is always step 0 (the cycle just started).
        let a = ani_with(5, 5, 12, None, None);
        assert_eq!(a.step_at_jiffy(0).unwrap(), 0);
    }

    #[test]
    fn step_at_jiffy_rejects_jiffy_equal_to_total() {
        // jiffy == total_jiffies is exactly the wrap-past-cycle-end
        // boundary. The caller forgot to modulo; reporting an error
        // catches the bug at the source rather than silently clamping
        // to the last step (which would leave the renderer "stuck" on
        // the last frame forever).
        let a = ani_with(3, 3, 10, None, None);
        // total = 30; jiffy = 30 must fail.
        let err = a.step_at_jiffy(30).unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("30 >= total cycle length 30"), "{msg}");
                assert!(msg.contains("modulo"), "{msg}");
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn step_at_jiffy_rejects_jiffy_far_past_total() {
        // Pathological adversarial value — `u64::MAX` is the classic
        // "elapsed counter wrapped or never reset" smuggling shape.
        // Must produce a deterministic error rather than a silent
        // last-step answer.
        let a = ani_with(2, 2, 5, None, None);
        let err = a.step_at_jiffy(u64::MAX).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(msg.contains("total cycle length 10"), "{msg}"),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn step_at_jiffy_handles_u64_range_jiffy() {
        // The cycle length can exceed u32::MAX (~2.8e14 worst case in the
        // bounded-input precision argument). A jiffy value above u32::MAX
        // but below the total must still resolve cleanly — this catches a
        // hypothetical implementation that internally truncated to u32.
        let big = u32::MAX;
        let a = ani_with(4, 4, 0, None, Some(vec![big, big, big, big]));
        // Cumulative boundaries at big, 2*big, 3*big, 4*big.
        // Step 1 spans [u32::MAX, 2 * u32::MAX) — pick a jiffy inside that
        // span that doesn't fit a u32.
        let probe = big as u64 + 1;
        assert!(probe > u32::MAX as u64);
        assert_eq!(a.step_at_jiffy(probe).unwrap(), 1);
        // And a probe inside step 3's range (well past u32 reach).
        let probe = 3u64 * big as u64 + 7;
        assert_eq!(a.step_at_jiffy(probe).unwrap(), 3);
    }

    #[test]
    fn step_at_jiffy_inherits_zero_jiffy_rejection() {
        // The accessor delegates to playback_steps up front — a
        // zero-jiffy step poisons the table and the lookup must fail
        // with the same playback_steps error rather than silently
        // collapsing the zero-duration interval.
        let a = ani_with(2, 4, 10, None, Some(vec![5, 0, 5, 5]));
        let err = a.step_at_jiffy(7).unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("step 1"), "{msg}");
                assert!(msg.contains("0 jiffies"), "{msg}");
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn step_at_jiffy_inherits_identity_past_n_frames_rejection() {
        // n_steps > n_frames without a seq chunk — playback_steps
        // refuses to fabricate identity indices past nFrames. The
        // step_at_jiffy lookup must surface that error rather than
        // claim a step it can't validate.
        let a = ani_with(2, 4, 10, None, None);
        let err = a.step_at_jiffy(5).unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("identity step 2"), "{msg}");
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn step_at_jiffy_inherits_zero_n_frames_rejection() {
        // Sentinel: hand-constructed AniFile with no frames. Reuses the
        // playback_steps n_frames = 0 rejection.
        let a = ani_with(0, 0, 10, None, None);
        let err = a.step_at_jiffy(0).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(msg.contains("n_frames = 0"), "{msg}"),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn step_at_jiffy_matches_playback_steps_cumulative_walk() {
        // Cross-check invariant: for every jiffy in the cycle, the result
        // must match a hand-rolled cumulative walk over playback_steps.
        // Catches the case where the accessor drifts away from
        // playback_steps' semantics under future maintenance.
        let a = ani_with(
            3,
            5,
            7,
            Some(vec![0, 1, 2, 1, 0]),
            Some(vec![4, 7, 2, 9, 11]),
        );
        let steps = a.playback_steps().unwrap();
        let total = a.total_jiffies().unwrap();
        // Walk every jiffy and assert the accessor agrees with the by-hand lookup.
        let mut expected = 0usize;
        let mut cumulative: u64 = steps[0].jiffies as u64;
        for j in 0..total {
            while j >= cumulative {
                expected += 1;
                cumulative += steps[expected].jiffies as u64;
            }
            assert_eq!(
                a.step_at_jiffy(j).unwrap(),
                expected,
                "mismatch at jiffy {j}"
            );
        }
    }

    #[test]
    fn step_at_jiffy_via_byte_parser_round_trip() {
        // End-to-end: real ANI bytes → read_ani_raw → step_at_jiffy.
        // build_ani_with_seq: i_disp_rate = 10, no rate chunk, 4 steps.
        // Cumulative boundaries at 10, 20, 30, 40 — step `i` covers
        // `[10*i, 10*(i+1))`.
        let bytes = build_ani_with_seq(2, 4, &[0, 1, 1, 0]);
        let parsed = read_ani_raw(&bytes).unwrap();
        assert_eq!(parsed.step_at_jiffy(0).unwrap(), 0);
        assert_eq!(parsed.step_at_jiffy(9).unwrap(), 0);
        assert_eq!(parsed.step_at_jiffy(10).unwrap(), 1);
        assert_eq!(parsed.step_at_jiffy(25).unwrap(), 2);
        assert_eq!(parsed.step_at_jiffy(39).unwrap(), 3);
        // And the wrap-past-cycle boundary still rejects.
        assert!(parsed.step_at_jiffy(40).is_err());
    }
}
