//! ICO / CUR file parser (registry-side, oxideav-core-using path).
//!
//! Walks the 6-byte `ICONDIR` header, then each 16-byte `ICONDIRENTRY`,
//! and decodes the pointed-at payload (either PNG or BMP-DIB) into an
//! RGBA [`IconImage`]. The lower-level container layout walk lives in
//! [`crate::raw::read_ico_raw`] which returns raw payload bytes;
//! this layer adds the PNG / BMP-DIB decode on top.

use oxideav_core::{Error, Result};

use crate::ani::{read_ani_raw, AniInfo, AniStep};
use crate::raw::{read_ico_raw, IconEntryRaw};
use crate::types::*;

/// Parse an ICO / CUR byte stream. Returns the container type and one
/// [`IconImage`] per directory entry, in directory order.
pub fn read_ico(input: &[u8]) -> Result<(IconType, Vec<IconImage>)> {
    let (icon_type, entries) = read_ico_raw(input)?;
    let mut images = Vec::with_capacity(entries.len());
    for entry in entries {
        images.push(decode_entry(entry)?);
    }
    Ok((icon_type, images))
}

fn decode_entry(entry: IconEntryRaw) -> Result<IconImage> {
    match entry.sub_format {
        IconSubFormat::Png => {
            let frame = oxideav_png::decode_png_to_frame(&entry.data, None)?;
            let rgba = frame_to_rgba_bytes(&frame, entry.width, entry.height)?;
            Ok(IconImage {
                width: entry.width,
                height: entry.height,
                pixels: rgba,
                bit_depth: entry.bit_depth,
                sub_format: IconSubFormat::Png,
                hotspot: entry.hotspot,
            })
        }
        IconSubFormat::Bmp => {
            // BMP-inside-ICO: headerless DIB with doubled height + AND
            // mask. `decode_dib_videoframe` is the registry-gated
            // VideoFrame-shaped wrapper around `oxideav_bmp::decode_dib`
            // (which now returns the standalone `BmpImage` shape).
            let frame = oxideav_bmp::decode_dib_videoframe(&entry.data, /* doubled */ true)?;
            let rgba = frame_to_rgba_bytes(&frame, entry.width, entry.height)?;
            Ok(IconImage {
                width: entry.width,
                height: entry.height,
                pixels: rgba,
                bit_depth: entry.bit_depth,
                sub_format: IconSubFormat::Bmp,
                hotspot: entry.hotspot,
            })
        }
    }
}

/// Copy a `VideoFrame` (produced by either oxideav-png or oxideav-bmp,
/// always in `Rgba`) into a tightly-packed top-down RGBA byte Vec.
fn frame_to_rgba_bytes(frame: &oxideav_core::VideoFrame, w: u32, h: u32) -> Result<Vec<u8>> {
    let w = w as usize;
    let h = h as usize;
    if frame.planes.is_empty() {
        return Err(Error::invalid("ICO: sub-image frame has no planes"));
    }
    let src_stride = frame.planes[0].stride;
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let src = &frame.planes[0].data[y * src_stride..y * src_stride + w * 4];
        out.extend_from_slice(src);
    }
    Ok(out)
}

/// One stored frame of a decoded ANI animation.
///
/// Each `LIST 'fram'` `icon` chunk is a *complete* ICO/CUR resource, so
/// it may itself carry several sub-images at different resolutions (the
/// way a `.ico` does). [`read_ani`] decodes every sub-image of every
/// frame; this struct groups one frame's sub-images together, tagged
/// with the container type the frame declared (`Ico` or `Cur`).
#[derive(Debug, Clone)]
pub struct AniFrame {
    /// The frame's own ICO/CUR container type (frames may be mixed —
    /// the ACON spec permits ICO and CUR frames in one file).
    pub icon_type: IconType,
    /// The frame's decoded sub-images, in directory order. Always at
    /// least one (the parser rejects an empty directory).
    pub images: Vec<IconImage>,
}

impl AniFrame {
    /// The sub-image a renderer would actually display for this frame:
    /// the largest by area (highest bit depth breaks a size tie), via
    /// the same [`select_largest`] heuristic the static-ICO API uses.
    ///
    /// Never `None` for a frame [`read_ani`] produces — every frame
    /// carries at least one sub-image (the parser rejects an empty
    /// directory) — but returns `Option` so a hand-built `AniFrame` with
    /// an empty `images` doesn't panic.
    pub fn primary_image(&self) -> Option<&IconImage> {
        select_largest(&self.images).map(|i| &self.images[i])
    }

    /// The cursor hotspot to use when this frame is on screen: the
    /// hotspot of the frame's [`primary_image`](Self::primary_image).
    ///
    /// `None` for an `Ico`-typed frame (icons have no click point) and
    /// for any `Cur` frame whose primary sub-image didn't carry one. An
    /// animated cursor's hotspot can change frame to frame (each frame is
    /// a full CUR resource with its own directory), so a renderer reads
    /// it per displayed frame rather than once for the whole animation.
    pub fn hotspot(&self) -> Option<HotSpot> {
        if self.icon_type != IconType::Cur {
            return None;
        }
        self.primary_image().and_then(|im| im.hotspot)
    }
}

/// A fully decoded ANI animated cursor: every stored frame decoded to
/// RGBA, plus the resolved playback timeline.
///
/// This is the ANI-side counterpart of [`read_ico`]'s
/// `(IconType, Vec<IconImage>)`: where `read_ico` decodes one icon
/// resource's sub-images, [`read_ani`] decodes a whole animation —
/// every frame's sub-images *and* the `seq ` / `rate` timeline merged
/// into a flat step table a renderer can drive directly.
#[derive(Debug, Clone)]
pub struct AniAnimation {
    /// Optional `LIST 'INFO'` metadata (title / author).
    pub info: AniInfo,
    /// The stored frames, in `LIST 'fram'` order. Index with
    /// [`AniStep::frame_index`] from [`Self::steps`].
    pub frames: Vec<AniFrame>,
    /// The resolved playback steps — `(frame_index, jiffies)` pairs with
    /// the `seq ` / `rate` / `iDispRate` defaulting already applied (see
    /// [`crate::AniFile::playback_steps`]). Each `frame_index` is a
    /// valid index into [`Self::frames`].
    pub steps: Vec<AniStep>,
}

impl AniAnimation {
    /// Total animation cycle length in 1/60-second jiffies — the sum of
    /// every resolved step's `jiffies`.
    ///
    /// This is the decoded-animation counterpart of
    /// [`crate::AniFile::total_jiffies`]. The difference: an
    /// [`AniAnimation`] already holds a fully-resolved [`Self::steps`]
    /// table (the `seq ` / `rate` / `iDispRate` defaulting was applied
    /// when [`read_ani`] built it, and every step's `jiffies` is
    /// guaranteed non-zero by that resolution), so this accessor is a
    /// straight sum over `steps` rather than a re-derivation of the
    /// defaulting rules. A renderer driving the decoded RGBA frames gets
    /// the cycle length without going back to the raw [`crate::AniFile`].
    ///
    /// The `u32 → u64` widening matches [`crate::AniFile::total_jiffies`]:
    /// a worst-case file (the 65_536-step cap × `u32::MAX` per-step rate)
    /// sums to roughly `2.8e14`, which exceeds `u32::MAX` by a factor of
    /// 65_536. `steps` is non-empty for any animation [`read_ani`]
    /// produces (the parser rejects a zero-frame file), so the sum is
    /// always `>= 1`.
    pub fn total_jiffies(&self) -> u64 {
        // Each `step.jiffies` is at most u32::MAX and the step count is
        // bounded by the parser's 65_536 cap, so the u64 sum has 14+ bits
        // of headroom — no `checked_add` needed.
        self.steps.iter().map(|s| s.jiffies as u64).sum()
    }

    /// Total animation cycle length in wall-clock seconds — the
    /// [`Self::total_jiffies`] result divided by the spec's
    /// 60-jiffies-per-second conversion factor.
    ///
    /// The decoded-animation counterpart of
    /// [`crate::AniFile::cycle_seconds`]: it folds the spec's "1 jiffy =
    /// 1/60 of a second" conversion into the type system so a renderer
    /// wiring the cycle length into clock-side scheduling (sleep timers,
    /// video-clip lengths, "1.5 s loop" UI labels) keeps the `60` literal
    /// out of its own call sites.
    ///
    /// Exact in `f64` for every animation [`read_ani`] can produce: the
    /// worst-case ~`2.8e14` jiffies (see [`Self::total_jiffies`]) sits
    /// well under `f64`'s `2^53 ≈ 9.0e15` integer-precision boundary.
    pub fn cycle_seconds(&self) -> f64 {
        // Spec: 1 second = 60 jiffies. The `as f64` widening is exact for
        // the parser's bounded input range (see the precision argument on
        // `total_jiffies`).
        self.total_jiffies() as f64 / 60.0
    }

    /// Locate the playback step active at a given jiffy offset into one
    /// animation cycle. Returns the step index, suitable as a subscript
    /// into [`Self::steps`].
    ///
    /// The decoded-animation counterpart of
    /// [`crate::AniFile::step_at_jiffy`], with identical interval
    /// semantics: step `i` claims the half-open jiffy interval
    /// `[start_i, start_i + step.jiffies)` where `start_i` is the
    /// cumulative sum of every preceding step's duration. A `jiffy`
    /// exactly equal to a step boundary lands on the next step (the
    /// spec's "show frame, then advance" edge rule).
    ///
    /// `jiffy` is expected to be the elapsed offset **inside one cycle**;
    /// the caller applies `jiffy % total_jiffies` before the lookup
    /// (looping is a renderer concern, not the accessor's). The `u64`
    /// parameter type matches [`Self::total_jiffies`]'s return type so a
    /// cycle longer than `u32::MAX` jiffies needs no caller pre-truncation.
    ///
    /// Errors when `jiffy >= total_jiffies` — the elapsed offset has
    /// wrapped past the end of one cycle (the caller forgot to modulo, or
    /// a wall-clock counter drifted). Surfacing an error rather than
    /// silently clamping to the last step catches the renderer bug at its
    /// source, matching [`crate::AniFile::step_at_jiffy`].
    pub fn step_at_jiffy(&self, jiffy: u64) -> Result<usize> {
        // `steps` is non-empty for any AniAnimation read_ani produces:
        // the parser rejects a zero-frame file and the resolved step
        // count never falls below 1 once there is at least one frame.
        let mut cumulative: u64 = 0;
        for (i, step) in self.steps.iter().enumerate() {
            // Compare against the exclusive upper bound so step `i` owns
            // `[start, end)` and `jiffy == end` flips cleanly to step
            // `i+1`. u64 accumulation can't overflow on parser-accepted
            // input (worst case ~2.8e14; see `total_jiffies`).
            cumulative += step.jiffies as u64;
            if jiffy < cumulative {
                return Ok(i);
            }
        }
        // Fell off the end — `jiffy >= total_jiffies`. `cumulative` is now
        // the full cycle length, so reuse it in the message (no second
        // `total_jiffies` pass).
        Err(Error::invalid(format!(
            "ANI: step_at_jiffy: jiffy {jiffy} >= total cycle length {cumulative} \
             (caller must apply modulo `jiffy % total_jiffies` before lookup)"
        )))
    }

    /// Locate the playback step active at a given wall-clock **seconds**
    /// offset into one animation cycle — the seconds-domain counterpart
    /// of [`Self::step_at_jiffy`], standing in the same relation to it as
    /// [`Self::cycle_seconds`] stands to [`Self::total_jiffies`].
    ///
    /// The decoded-animation counterpart of
    /// [`crate::AniFile::step_at_second`]. A renderer driving playback
    /// from a seconds-based wall clock gets the active step directly
    /// instead of re-deriving the spec's 60-jiffies-per-second conversion
    /// and handing off to [`Self::step_at_jiffy`] by hand — the `60`
    /// literal is fixed in the function name so it can't drift across
    /// call sites.
    ///
    /// Conversion is `floor(seconds * 60)` jiffies: the floor is the
    /// correct direction for the half-open `[start, end)` step intervals,
    /// since a fractional jiffy offset has not yet crossed into the next
    /// whole-jiffy bucket.
    ///
    /// Errors:
    ///
    /// * `seconds` is negative, NaN, or infinite — a wall-clock offset is
    ///   physically non-negative and finite (NaN especially must be
    ///   caught: every `<` boundary comparison against a NaN-derived jiffy
    ///   is false, so it would otherwise walk off the end and misreport as
    ///   a "past total" error).
    /// * `floor(seconds * 60)` exceeds `u64::MAX` — caught before the
    ///   `as u64` cast, which would otherwise saturate silently.
    /// * Everything [`Self::step_at_jiffy`] rejects (resolved jiffy offset
    ///   `>= total_jiffies`).
    pub fn step_at_second(&self, seconds: f64) -> Result<usize> {
        // A wall-clock offset must be finite and non-negative. NaN is
        // load-bearing to reject up front (see `step_at_jiffy`'s `<`
        // comparison behaviour against a NaN-derived jiffy).
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(Error::invalid(format!(
                "ANI: step_at_second: seconds {seconds} is not a finite, \
                 non-negative wall-clock offset"
            )));
        }
        // Spec: 1 second = 60 jiffies. Floor maps a wall-clock instant to
        // the step whose interval contains its whole-jiffy floor.
        let jiffies_f = (seconds * 60.0).floor();
        if jiffies_f > u64::MAX as f64 {
            return Err(Error::invalid(format!(
                "ANI: step_at_second: seconds {seconds} converts to a jiffy \
                 offset beyond u64 range"
            )));
        }
        self.step_at_jiffy(jiffies_f as u64)
    }

    /// The stored frame displayed at playback step `step_index` — i.e.
    /// `frames[steps[step_index].frame_index]`, resolving the `seq `
    /// indirection so the caller doesn't index two tables by hand.
    ///
    /// `step_index` is a subscript into [`Self::steps`] (as returned by
    /// [`Self::step_at_jiffy`] / [`Self::step_at_second`]). Returns `None`
    /// when `step_index` is out of range for `steps`. The inner
    /// `frame_index` is always valid for [`Self::frames`] in any
    /// [`AniAnimation`] [`read_ani`] produces — the timeline resolver
    /// guarantees it — so this only `None`s on a bad `step_index`.
    pub fn frame_at_step(&self, step_index: usize) -> Option<&AniFrame> {
        let step = self.steps.get(step_index)?;
        self.frames.get(step.frame_index as usize)
    }

    /// The cursor hotspot active at playback step `step_index`: the
    /// [`hotspot`](AniFrame::hotspot) of the frame shown at that step.
    ///
    /// `None` when `step_index` is out of range, when the displayed frame
    /// is `Ico`-typed, or when its primary sub-image carried no hotspot.
    /// Combine with [`Self::step_at_jiffy`] for the per-tick query a
    /// cursor renderer needs: "which step is on screen, and where is its
    /// click point?".
    pub fn hotspot_at_step(&self, step_index: usize) -> Option<HotSpot> {
        self.frame_at_step(step_index).and_then(AniFrame::hotspot)
    }
}

/// Parse and fully decode an ANI animated-cursor byte stream.
///
/// Walks the RIFF/`ACON` tree (via [`read_ani_raw`]), decodes every
/// stored frame's sub-images to RGBA, and resolves the `seq ` / `rate`
/// chunks into a flat playback step table (via
/// [`crate::AniFile::playback_steps`]).
///
/// Two frame layouts are decoded, selected by the `anih` `AF_ICON` flag:
///
/// * **`AF_ICON` set** (the common case): each frame is a complete
///   ICO/CUR resource carrying its own headers, decoded via [`read_ico`].
///   A frame may carry several sub-images at different resolutions.
///
/// * **`AF_ICON` clear**: each frame is a single headerless BMP whose
///   geometry lives only in `anih` (`iWidth` / `iHeight` / `iBitCount` /
///   `nPlanes`) — there is no per-frame DIB header and no ICO directory.
///   This path is decoded by synthesising a `BITMAPINFOHEADER` from the
///   `anih` descriptor (via [`crate::AniFile::raw_bmp_descriptor`]) and
///   feeding the raw pixel rows through the same BMP-DIB decoder the
///   icon path uses. Per the ACON format reference the raw frames carry
///   **no AND mask** (they are plain bottom-up DIB pixel rows padded to a
///   4-byte boundary), so they decode opaque.
///
///   Only the non-indexed depths `{16, 24, 32}` are decodable: at
///   `iBitCount <= 8` the format reference explicitly leaves it
///   *unknown* whether the frame includes a colour table, so this
///   function returns an error rather than guess a palette layout. Such
///   files still parse structurally via [`read_ani_raw`].
pub fn read_ani(input: &[u8]) -> Result<AniAnimation> {
    let ani = read_ani_raw(input).map_err(|e| Error::invalid(e.to_string()))?;

    // Resolve the timeline first: this re-validates the seq / rate
    // lengths and frame-index ranges, so a frame_index emitted here is
    // guaranteed to be in range for the decoded `frames` vec below.
    let steps = ani
        .playback_steps()
        .map_err(|e| Error::invalid(e.to_string()))?;

    let frames = if ani.header.frames_are_icons() {
        let mut frames = Vec::with_capacity(ani.frames.len());
        for (i, frame_bytes) in ani.frames.iter().enumerate() {
            let (icon_type, images) = read_ico(frame_bytes)
                .map_err(|e| Error::invalid(format!("ANI: frame {i}: {e}")))?;
            frames.push(AniFrame { icon_type, images });
        }
        frames
    } else {
        decode_raw_bmp_frames(&ani)?
    };

    Ok(AniAnimation {
        info: ani.info,
        frames,
        steps,
    })
}

/// Decode the `AF_ICON`-clear path: every `LIST 'fram'` frame is a
/// single headerless BMP whose geometry comes from `anih`. Each decoded
/// frame becomes an [`AniFrame`] carrying exactly one [`IconImage`],
/// tagged `Cur` (an ANI is always a cursor resource).
fn decode_raw_bmp_frames(ani: &crate::AniFile) -> Result<Vec<AniFrame>> {
    // `raw_bmp_descriptor` validates the geometry is fully specified
    // (non-zero width / height / bit_count) for this path and normalises
    // planes; it returns `None` only when AF_ICON is set, which this
    // branch already excluded.
    let desc = ani
        .raw_bmp_descriptor()
        .map_err(|e| Error::invalid(e.to_string()))?
        .ok_or_else(|| Error::invalid("ANI: read_ani: internal: raw frame path on AF_ICON file"))?;

    // The format reference (docs/image/ico/ani-acon-format.md) records
    // that for an indexed raw frame (iBitCount <= 8) it is *unknown*
    // whether a colour table precedes the pixels. Refuse those rather
    // than guess a palette layout that could mis-decode every pixel.
    if desc.bit_count <= 8 {
        return Err(Error::invalid(format!(
            "ANI: read_ani: AF_ICON-clear raw frame at iBitCount = {} (indexed) — the \
             ACON format leaves the raw colour-table layout undefined for depths <= 8, \
             so it cannot be decoded unambiguously. Use read_ani_raw + \
             AniFile::raw_bmp_descriptor to access the raw bytes.",
            desc.bit_count
        )));
    }

    let mut frames = Vec::with_capacity(ani.frames.len());
    for (i, pixels) in ani.frames.iter().enumerate() {
        let dib = synthesize_headerless_dib(&desc, pixels)?;
        // doubled = false: a raw ANI frame is a single image with no
        // doubled-height AND-mask region (that is an ICO/CUR-only layout).
        let frame = oxideav_bmp::decode_dib_videoframe(&dib, /* doubled */ false)
            .map_err(|e| Error::invalid(format!("ANI: raw frame {i}: {e}")))?;
        let rgba = frame_to_rgba_bytes(&frame, desc.width, desc.height)?;
        let image = IconImage {
            width: desc.width,
            height: desc.height,
            pixels: rgba,
            bit_depth: desc.bit_count as u8,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
        };
        frames.push(AniFrame {
            icon_type: IconType::Cur,
            images: vec![image],
        });
    }
    Ok(frames)
}

/// Build a single-image headerless DIB (`BITMAPINFOHEADER` + pixel rows)
/// from an `anih` raw-frame [`crate::RawBmpDescriptor`] and the raw
/// frame's pixel bytes, so the standard BMP-DIB decoder can consume it.
///
/// The synthesised header is a canonical 40-byte V3 `BITMAPINFOHEADER`:
/// `biWidth = width`, `biHeight = +height` (positive = bottom-up rows,
/// the BMP default — the raw ANI frame is *not* the doubled-height
/// XOR+AND layout an ICO uses), `biPlanes = 1`, `biBitCount` from the
/// descriptor, `biCompression = BI_RGB (0)`, all remaining fields zero.
/// The descriptor already guarantees `bit_count ∈ {16, 24, 32}` here, so
/// the decoder needs no colour table.
fn synthesize_headerless_dib(desc: &crate::RawBmpDescriptor, pixels: &[u8]) -> Result<Vec<u8>> {
    // 4-byte-aligned row stride for an uncompressed top-of-{16,24,32}-bpp
    // DIB, matching the BMP pixel-array padding rule.
    let bits_per_row = desc.width as u64 * desc.bit_count as u64;
    let row_bytes = bits_per_row.div_ceil(8);
    let stride = (row_bytes + 3) & !3;
    let expected = stride
        .checked_mul(desc.height as u64)
        .ok_or_else(|| Error::invalid("ANI: raw frame geometry overflows"))?;
    if (pixels.len() as u64) < expected {
        return Err(Error::invalid(format!(
            "ANI: raw frame is {} bytes but anih geometry ({}x{} @ {} bpp) needs {} \
             ({}-byte stride x {} rows)",
            pixels.len(),
            desc.width,
            desc.height,
            desc.bit_count,
            expected,
            stride,
            desc.height,
        )));
    }

    let mut dib = Vec::with_capacity(40 + pixels.len());
    dib.extend_from_slice(&40u32.to_le_bytes()); // biSize
    dib.extend_from_slice(&(desc.width as i32).to_le_bytes()); // biWidth
    dib.extend_from_slice(&(desc.height as i32).to_le_bytes()); // biHeight (+ = bottom-up)
    dib.extend_from_slice(&(desc.planes as u16).to_le_bytes()); // biPlanes
    dib.extend_from_slice(&(desc.bit_count as u16).to_le_bytes()); // biBitCount
    dib.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    dib.extend_from_slice(&0u32.to_le_bytes()); // biSizeImage (0 = derive)
    dib.extend_from_slice(&0i32.to_le_bytes()); // biXPelsPerMeter
    dib.extend_from_slice(&0i32.to_le_bytes()); // biYPelsPerMeter
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant
    dib.extend_from_slice(pixels);
    Ok(dib)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ani::{AF_ICON, AF_SEQUENCE};
    use crate::write_ico;

    /// Solid-colour RGBA buffer for an `n`×`n` sub-image.
    fn solid_rgba(n: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity((n * n * 4) as usize);
        for _ in 0..(n * n) {
            v.extend_from_slice(&rgba);
        }
        v
    }

    /// A complete single-sub-image ICO byte stream, forced all-BMP so
    /// `read_ico` decodes it without PNG involvement.
    fn ico_frame(n: u32, rgba: [u8; 4]) -> Vec<u8> {
        let img = IconImage::from_rgba(n, n, solid_rgba(n, rgba));
        let opts = WriteOptions {
            png_size_threshold: None,
            ..Default::default()
        };
        write_ico(IconType::Ico, &[img], opts).unwrap()
    }

    /// Build a complete single-sub-image ICO carrying one hand-built
    /// headerless **indexed** DIB body (palette + XOR rows + 1-bpp AND
    /// mask), exercising the low-bit-depth BMP-inside-ICO decode path
    /// that `IconImage::from_rgba` + `write_ico` (which only emit 32-bpp)
    /// can't reach. Spec §"DIB form structure": `BITMAPINFOHEADER`,
    /// then `2^biBitCount` RGBQUAD palette entries, then the bottom-up
    /// XOR colour rows at `biBitCount` bpp, then the bottom-up 1-bpp AND
    /// mask (a set bit = transparent). `biHeight` is the doubled
    /// (XOR + AND) height the parser halves back.
    ///
    /// `palette` is `(B, G, R)` triples written into the leading RGBQUAD
    /// slots (reserved byte 0). `xor_indices` / `and_bits` are given in
    /// **top-down** row-major order (row 0 = top); the builder flips both
    /// to the bottom-up on-disk layout. `and_bits[i] != 0` marks pixel
    /// `i` transparent.
    fn indexed_dib_ico(
        w: u32,
        h: u32,
        bpp: u16,
        palette: &[[u8; 3]],
        xor_indices: &[u8],
        and_bits: &[bool],
    ) -> Vec<u8> {
        assert!(matches!(bpp, 1 | 4 | 8));
        assert_eq!(xor_indices.len(), (w * h) as usize);
        assert_eq!(and_bits.len(), (w * h) as usize);

        let mut dib: Vec<u8> = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes()); // biSize
        dib.extend_from_slice(&(w as i32).to_le_bytes()); // biWidth
        dib.extend_from_slice(&((2 * h) as i32).to_le_bytes()); // biHeight doubled
        dib.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
        dib.extend_from_slice(&bpp.to_le_bytes()); // biBitCount
        dib.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        dib.extend_from_slice(&0u32.to_le_bytes()); // biSizeImage
        dib.extend_from_slice(&0i32.to_le_bytes()); // biXPelsPerMeter
        dib.extend_from_slice(&0i32.to_le_bytes()); // biYPelsPerMeter
        dib.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed (0 -> 2^bpp)
        dib.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant

        // Palette: 2^bpp RGBQUAD (B, G, R, reserved) entries.
        let pal_entries = 1usize << bpp;
        let mut pal = vec![0u8; pal_entries * 4];
        for (i, &[b, g, r]) in palette.iter().enumerate() {
            pal[i * 4..i * 4 + 4].copy_from_slice(&[b, g, r, 0]);
        }
        dib.extend_from_slice(&pal);

        // XOR colour rows: bottom-up, 4-byte-aligned stride at `bpp`.
        let xor_stride = ((w * bpp as u32).div_ceil(32) * 4) as usize;
        let mut xor = vec![0u8; xor_stride * h as usize];
        for y in 0..h {
            let storage_row = (h - 1 - y) as usize; // top-down y -> bottom-up storage
            for x in 0..w {
                let idx = xor_indices[(y * w + x) as usize];
                let bit_pos = (x * bpp as u32) as usize;
                let byte = storage_row * xor_stride + bit_pos / 8;
                let shift = 8 - bpp as usize - (bit_pos % 8);
                let mask: u16 = (1u16 << bpp) - 1;
                xor[byte] |= ((idx as u16 & mask) as u8) << shift;
            }
        }
        dib.extend_from_slice(&xor);

        // AND mask: 1-bpp, bottom-up, 4-byte-aligned stride.
        let and_stride = (w.div_ceil(32) * 4) as usize;
        let mut and = vec![0u8; and_stride * h as usize];
        for y in 0..h {
            let storage_row = (h - 1 - y) as usize;
            for x in 0..w {
                if and_bits[(y * w + x) as usize] {
                    let byte = storage_row * and_stride + (x as usize) / 8;
                    let shift = 7 - (x as usize % 8);
                    and[byte] |= 1 << shift;
                }
            }
        }
        dib.extend_from_slice(&and);

        let entry = crate::raw::IconEntryRaw {
            width: w,
            height: h,
            bit_depth: bpp as u8,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib,
        };
        crate::raw::write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap()
    }

    #[test]
    fn read_ico_decodes_8bpp_indexed_with_palette_and_mask() {
        // 2×2 8-bpp icon. Palette: idx0 = red, idx1 = green. Image (top-down):
        //   (0,0)=idx0 red   (1,0)=idx1 green, transparent via AND mask
        //   (0,1)=idx1 green (1,1)=idx1 green
        let bytes = indexed_dib_ico(
            2,
            2,
            8,
            &[[0, 0, 255], [0, 255, 0]], // BGR: red, green
            &[0, 1, 1, 1],
            &[false, true, false, false],
        );
        let (ty, imgs) = read_ico(&bytes).unwrap();
        assert_eq!(ty, IconType::Ico);
        assert_eq!(imgs.len(), 1);
        let im = &imgs[0];
        assert_eq!((im.width, im.height), (2, 2));
        assert_eq!(im.bit_depth, 8);
        assert_eq!(im.sub_format, IconSubFormat::Bmp);
        // Palette lookup + bottom-up flip + AND-mask transparency.
        assert_eq!(&im.pixels[0..4], &[255, 0, 0, 255], "(0,0) red opaque");
        assert_eq!(&im.pixels[4..8], &[0, 255, 0, 0], "(1,0) green transparent");
        assert_eq!(&im.pixels[8..12], &[0, 255, 0, 255], "(0,1) green opaque");
        assert_eq!(&im.pixels[12..16], &[0, 255, 0, 255], "(1,1) green opaque");
    }

    #[test]
    fn read_ico_decodes_1bpp_monochrome_with_mask() {
        // 2×2 1-bpp icon. Palette: idx0 = black, idx1 = white. Image:
        //   (0,0)=white (1,0)=black
        //   (0,1)=black (1,1)=white, transparent
        let bytes = indexed_dib_ico(
            2,
            2,
            1,
            &[[0, 0, 0], [255, 255, 255]],
            &[1, 0, 0, 1],
            &[false, false, false, true],
        );
        let (_, imgs) = read_ico(&bytes).unwrap();
        let im = &imgs[0];
        assert_eq!(im.bit_depth, 1);
        assert_eq!(&im.pixels[0..4], &[255, 255, 255, 255], "(0,0) white");
        assert_eq!(&im.pixels[4..8], &[0, 0, 0, 255], "(1,0) black");
        assert_eq!(&im.pixels[8..12], &[0, 0, 0, 255], "(0,1) black");
        assert_eq!(
            &im.pixels[12..16],
            &[255, 255, 255, 0],
            "(1,1) white transparent"
        );
    }

    #[test]
    fn read_ico_decodes_4bpp_indexed_palette() {
        // 4×1 4-bpp row using four distinct palette slots, no transparency.
        let bytes = indexed_dib_ico(
            4,
            1,
            4,
            &[[0, 0, 255], [0, 255, 0], [255, 0, 0], [0, 255, 255]], // r, g, b, yellow
            &[0, 1, 2, 3],
            &[false, false, false, false],
        );
        let (_, imgs) = read_ico(&bytes).unwrap();
        let im = &imgs[0];
        assert_eq!(im.bit_depth, 4);
        assert_eq!(&im.pixels[0..4], &[255, 0, 0, 255], "idx0 red");
        assert_eq!(&im.pixels[4..8], &[0, 255, 0, 255], "idx1 green");
        assert_eq!(&im.pixels[8..12], &[0, 0, 255, 255], "idx2 blue");
        assert_eq!(&im.pixels[12..16], &[255, 255, 0, 255], "idx3 yellow");
    }

    /// Append a RIFF chunk (tag + LE u32 len + payload, even-padded).
    fn push_chunk(buf: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
        buf.extend_from_slice(tag);
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            buf.push(0);
        }
    }

    /// Hand-assemble an ANI file from already-encoded ICO frame bodies,
    /// with optional seq / rate chunks and an INFO list.
    fn build_ani(
        frames: &[Vec<u8>],
        n_steps: u32,
        i_disp_rate: u32,
        seq: Option<&[u32]>,
        rate: Option<&[u32]>,
        info: Option<(&[u8], &[u8])>,
    ) -> Vec<u8> {
        let mut anih = [0u8; 36];
        let put =
            |b: &mut [u8; 36], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut anih, 0, 36);
        put(&mut anih, 4, frames.len() as u32);
        put(&mut anih, 8, n_steps);
        put(&mut anih, 24, 1); // nPlanes
        put(&mut anih, 28, i_disp_rate);
        let attrs = AF_ICON | if seq.is_some() { AF_SEQUENCE } else { 0 };
        put(&mut anih, 32, attrs);

        let mut body = Vec::new();
        body.extend_from_slice(b"ACON");
        push_chunk(&mut body, b"anih", &anih);

        if let Some((title, author)) = info {
            let mut info_body = Vec::new();
            info_body.extend_from_slice(b"INFO");
            push_chunk(&mut info_body, b"INAM", title);
            push_chunk(&mut info_body, b"IART", author);
            push_chunk(&mut body, b"LIST", &info_body);
        }
        if let Some(seq) = seq {
            let p: Vec<u8> = seq.iter().flat_map(|v| v.to_le_bytes()).collect();
            push_chunk(&mut body, b"seq ", &p);
        }
        if let Some(rate) = rate {
            let p: Vec<u8> = rate.iter().flat_map(|v| v.to_le_bytes()).collect();
            push_chunk(&mut body, b"rate", &p);
        }
        let mut fram = Vec::new();
        fram.extend_from_slice(b"fram");
        for f in frames {
            push_chunk(&mut fram, b"icon", f);
        }
        push_chunk(&mut body, b"LIST", &fram);

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn read_ani_decodes_frames_and_identity_timeline() {
        let red = ico_frame(8, [200, 10, 10, 255]);
        let green = ico_frame(8, [10, 200, 10, 255]);
        let ani = build_ani(&[red, green], 0, 12, None, None, None);

        let anim = read_ani(&ani).unwrap();
        assert_eq!(anim.frames.len(), 2);
        // Each frame is a single 8×8 BMP sub-image decoded to RGBA.
        for frame in &anim.frames {
            assert_eq!(frame.icon_type, IconType::Ico);
            assert_eq!(frame.images.len(), 1);
            assert_eq!(frame.images[0].width, 8);
            assert_eq!(frame.images[0].height, 8);
            assert_eq!(frame.images[0].pixels.len(), 8 * 8 * 4);
        }
        // No seq chunk → identity timeline, iDispRate for every step.
        assert_eq!(anim.steps.len(), 2);
        assert_eq!(anim.steps[0].frame_index, 0);
        assert_eq!(anim.steps[1].frame_index, 1);
        assert!(anim.steps.iter().all(|s| s.jiffies == 12));
    }

    #[test]
    fn read_ani_applies_seq_and_rate_and_info() {
        let a = ico_frame(8, [1, 2, 3, 255]);
        let b = ico_frame(8, [4, 5, 6, 255]);
        // 2 stored frames, 4 playback steps playing 0,1,1,0 with
        // distinct per-step durations.
        let ani = build_ani(
            &[a, b],
            4,
            10,
            Some(&[0, 1, 1, 0]),
            Some(&[5, 6, 7, 8]),
            Some((b"My Cursor", b"Me")),
        );

        let anim = read_ani(&ani).unwrap();
        assert_eq!(anim.frames.len(), 2);
        let idx: Vec<u32> = anim.steps.iter().map(|s| s.frame_index).collect();
        let jif: Vec<u32> = anim.steps.iter().map(|s| s.jiffies).collect();
        assert_eq!(idx, vec![0, 1, 1, 0]);
        assert_eq!(jif, vec![5, 6, 7, 8]);
        assert_eq!(anim.info.title_str().as_deref(), Some("My Cursor"));
        assert_eq!(anim.info.author_str().as_deref(), Some("Me"));
        // Every resolved step indexes a real decoded frame.
        for s in &anim.steps {
            assert!((s.frame_index as usize) < anim.frames.len());
        }
    }

    /// Hand-assemble an `AF_ICON`-clear ANI (raw headerless BMP frames)
    /// from already-built raw frame pixel bodies, with geometry baked
    /// into the `anih` header. `n_frames` is taken from `frames.len()`.
    fn build_raw_ani(
        frames: &[Vec<u8>],
        width: u32,
        height: u32,
        bit_count: u32,
        i_disp_rate: u32,
    ) -> Vec<u8> {
        let mut anih = [0u8; 36];
        let put =
            |b: &mut [u8; 36], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut anih, 0, 36);
        put(&mut anih, 4, frames.len() as u32); // nFrames
        put(&mut anih, 12, width); // iWidth
        put(&mut anih, 16, height); // iHeight
        put(&mut anih, 20, bit_count); // iBitCount
        put(&mut anih, 24, 1); // nPlanes
        put(&mut anih, 28, i_disp_rate); // iDispRate
        put(&mut anih, 32, 0); // bfAttributes: AF_ICON clear

        let mut body = Vec::new();
        body.extend_from_slice(b"ACON");
        push_chunk(&mut body, b"anih", &anih);
        let mut fram = Vec::new();
        fram.extend_from_slice(b"fram");
        for f in frames {
            push_chunk(&mut fram, b"icon", f);
        }
        push_chunk(&mut body, b"LIST", &fram);
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// A bottom-up 32-bpp DIB pixel array (no header) for a `w`×`h` frame
    /// where every pixel is BGRA `(b, g, r, a)`. Stride is `w*4`, already
    /// 4-byte aligned at 32 bpp, so no padding is needed.
    fn raw_bgra32(w: u32, h: u32, bgra: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            v.extend_from_slice(&bgra);
        }
        v
    }

    #[test]
    fn read_ani_decodes_af_icon_clear_raw_bmp_32bpp() {
        // Two raw 4×4 32-bpp frames: BGRA in storage → RGBA on decode.
        // Frame 0 = pure red (B=0 G=0 R=200 A=255), frame 1 = pure green.
        let f0 = raw_bgra32(4, 4, [0, 0, 200, 255]);
        let f1 = raw_bgra32(4, 4, [0, 200, 0, 255]);
        let ani = build_raw_ani(&[f0, f1], 4, 4, 32, 9);

        let anim = read_ani(&ani).unwrap();
        assert_eq!(anim.frames.len(), 2);
        // Each raw frame becomes one Cur sub-image, no hotspot.
        for frame in &anim.frames {
            assert_eq!(frame.icon_type, IconType::Cur);
            assert_eq!(frame.images.len(), 1);
            let img = &frame.images[0];
            assert_eq!((img.width, img.height), (4, 4));
            assert_eq!(img.sub_format, IconSubFormat::Bmp);
            assert_eq!(img.bit_depth, 32);
            assert!(img.hotspot.is_none());
            assert_eq!(img.pixels.len(), 4 * 4 * 4);
        }
        // BGRA red stored → top-left RGBA pixel is (200, 0, 0, 255).
        assert_eq!(&anim.frames[0].images[0].pixels[0..4], &[200, 0, 0, 255]);
        assert_eq!(&anim.frames[1].images[0].pixels[0..4], &[0, 200, 0, 255]);
        // Identity timeline (no seq), iDispRate per step.
        assert_eq!(anim.steps.len(), 2);
        assert!(anim.steps.iter().all(|s| s.jiffies == 9));
    }

    #[test]
    fn read_ani_rejects_indexed_raw_bmp() {
        // iBitCount <= 8: colour-table presence is undefined per the
        // ACON reference, so the raw path must refuse rather than guess.
        let frame = vec![0u8; 8 * 8]; // 8 bpp pixel bytes (1 byte/pixel)
        let ani = build_raw_ani(&[frame], 8, 8, 8, 10);
        let err = read_ani(&ani).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("indexed"), "{msg}");
        assert!(msg.contains("iBitCount = 8"), "{msg}");
    }

    #[test]
    fn read_ani_rejects_truncated_raw_bmp_frame() {
        // anih claims 8×8 @ 32 bpp = 256 bytes, but the frame is short.
        let frame = vec![0u8; 100];
        let ani = build_raw_ani(&[frame], 8, 8, 32, 10);
        let err = read_ani(&ani).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("needs"), "{msg}");
    }

    #[test]
    fn read_ani_rejects_raw_bmp_unset_geometry() {
        // AF_ICON clear but iWidth = 0: raw_bmp_descriptor flags the
        // missing geometry (no per-frame header to fall back on).
        let frame = vec![0u8; 8 * 8 * 4];
        let ani = build_raw_ani(&[frame], 0, 8, 32, 10);
        let err = read_ani(&ani).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("dimension") || msg.contains("iWidth"),
            "{msg}"
        );
    }

    #[test]
    fn read_ani_rejects_non_ani_input() {
        // A plain ICO file is not a RIFF/ACON stream.
        let ico = ico_frame(8, [9, 9, 9, 255]);
        assert!(read_ani(&ico).is_err());
    }

    #[test]
    fn write_ani_raw_frames_output_decodes_through_read_ani() {
        // Closes the encoder↔decoder loop on the AF_ICON-clear raw path:
        // write_ani_raw_frames produces a headerless-BMP ANI, and both
        // read_ani (pixel decode) and read_ani_raw + raw_bmp_descriptor
        // (geometry) consume its output.
        use crate::{read_ani_raw, write_ani_raw_frames, AniRawWriteOptions, RawFrameBitDepth};
        let red = [190, 30, 40, 255];
        let blue = [40, 30, 190, 255];
        let mkframe = |c: [u8; 4]| -> (u32, u32, Vec<u8>) {
            let mut v = Vec::new();
            for _ in 0..(5 * 3) {
                v.extend_from_slice(&c);
            }
            (5, 3, v) // 5×3 exercises the 4-byte row pad at 32 bpp too
        };
        let opts = AniRawWriteOptions {
            default_jiffies: 8,
            bit_depth: RawFrameBitDepth::Bgra32,
            ..Default::default()
        };
        let bytes = write_ani_raw_frames(&[mkframe(red), mkframe(blue)], &opts).unwrap();

        // raw_bmp_descriptor round-trips the anih geometry.
        let raw = read_ani_raw(&bytes).unwrap();
        assert!(!raw.header.frames_are_icons());
        let desc = raw.raw_bmp_descriptor().unwrap().unwrap();
        assert_eq!((desc.width, desc.height, desc.bit_count), (5, 3, 32));

        // read_ani decodes the bodies back to the exact source RGBA.
        let anim = read_ani(&bytes).unwrap();
        assert_eq!(anim.frames.len(), 2);
        let mut expect_red = Vec::new();
        for _ in 0..15 {
            expect_red.extend_from_slice(&red);
        }
        assert_eq!(anim.frames[0].images[0].pixels, expect_red);
        assert_eq!(anim.frames[0].images[0].bit_depth, 32);
    }

    #[test]
    fn animation_total_and_cycle_seconds_from_resolved_steps() {
        let a = ico_frame(8, [1, 2, 3, 255]);
        let b = ico_frame(8, [4, 5, 6, 255]);
        // 4 steps, durations 5,6,7,8 → 26 jiffies total, 26/60 s.
        let ani = build_ani(
            &[a, b],
            4,
            10,
            Some(&[0, 1, 1, 0]),
            Some(&[5, 6, 7, 8]),
            None,
        );
        let anim = read_ani(&ani).unwrap();
        assert_eq!(anim.total_jiffies(), 26);
        assert!((anim.cycle_seconds() - 26.0 / 60.0).abs() < 1e-12);
    }

    #[test]
    fn animation_identity_timeline_total() {
        let red = ico_frame(8, [200, 10, 10, 255]);
        let green = ico_frame(8, [10, 200, 10, 255]);
        // No seq / rate → identity timeline, iDispRate (12) per step.
        let ani = build_ani(&[red, green], 0, 12, None, None, None);
        let anim = read_ani(&ani).unwrap();
        assert_eq!(anim.total_jiffies(), 24);
        assert!((anim.cycle_seconds() - 24.0 / 60.0).abs() < 1e-12);
    }

    #[test]
    fn animation_step_at_jiffy_interval_boundaries() {
        let a = ico_frame(8, [1, 2, 3, 255]);
        let b = ico_frame(8, [4, 5, 6, 255]);
        // Durations 5,6,7,8 → cumulative boundaries 5,11,18,26.
        let ani = build_ani(
            &[a, b],
            4,
            10,
            Some(&[0, 1, 1, 0]),
            Some(&[5, 6, 7, 8]),
            None,
        );
        let anim = read_ani(&ani).unwrap();
        // Step 0 owns [0,5), step 1 [5,11), step 2 [11,18), step 3 [18,26).
        assert_eq!(anim.step_at_jiffy(0).unwrap(), 0);
        assert_eq!(anim.step_at_jiffy(4).unwrap(), 0);
        // Boundary lands on the next step ("show frame, then advance").
        assert_eq!(anim.step_at_jiffy(5).unwrap(), 1);
        assert_eq!(anim.step_at_jiffy(10).unwrap(), 1);
        assert_eq!(anim.step_at_jiffy(11).unwrap(), 2);
        assert_eq!(anim.step_at_jiffy(17).unwrap(), 2);
        assert_eq!(anim.step_at_jiffy(18).unwrap(), 3);
        assert_eq!(anim.step_at_jiffy(25).unwrap(), 3);
        // jiffy == total and beyond are rejected (caller forgot modulo).
        let err = anim.step_at_jiffy(26).unwrap_err().to_string();
        assert!(err.contains("total cycle length 26"), "{err}");
        assert!(anim.step_at_jiffy(1000).is_err());
    }

    #[test]
    fn animation_step_at_second_floor_and_rejections() {
        let a = ico_frame(8, [1, 2, 3, 255]);
        let b = ico_frame(8, [4, 5, 6, 255]);
        let ani = build_ani(
            &[a, b],
            4,
            10,
            Some(&[0, 1, 1, 0]),
            Some(&[5, 6, 7, 8]),
            None,
        );
        let anim = read_ani(&ani).unwrap();
        // floor(0.0*60)=0 → step 0; floor(5/60*60)=5 → step 1 (boundary);
        // floor(4.9/60*60)=4 → step 0.
        assert_eq!(anim.step_at_second(0.0).unwrap(), 0);
        assert_eq!(anim.step_at_second(5.0 / 60.0).unwrap(), 1);
        assert_eq!(anim.step_at_second(4.9 / 60.0).unwrap(), 0);
        assert_eq!(anim.step_at_second(18.0 / 60.0).unwrap(), 3);
        // Non-finite / negative are rejected.
        assert!(anim.step_at_second(-0.1).is_err());
        assert!(anim.step_at_second(f64::NAN).is_err());
        assert!(anim.step_at_second(f64::INFINITY).is_err());
        // Past the cycle end delegates to step_at_jiffy's rejection.
        assert!(anim.step_at_second(26.0 / 60.0).is_err());
    }

    /// A complete single-sub-image CUR byte stream carrying a hotspot,
    /// forced all-BMP so `read_ico` decodes it without PNG involvement.
    fn cur_frame(n: u32, rgba: [u8; 4], hot: HotSpot) -> Vec<u8> {
        let mut img = IconImage::from_rgba(n, n, solid_rgba(n, rgba));
        img.hotspot = Some(hot);
        let opts = WriteOptions {
            png_size_threshold: None,
            ..Default::default()
        };
        write_ico(IconType::Cur, &[img], opts).unwrap()
    }

    #[test]
    fn ani_frame_primary_image_picks_largest_sub_image() {
        // A frame carrying 8×8 and 16×16 sub-images: the primary is the
        // 16×16 (largest area). Build a two-resolution ICO frame.
        let imgs = vec![
            IconImage::from_rgba(8, 8, solid_rgba(8, [1, 2, 3, 255])),
            IconImage::from_rgba(16, 16, solid_rgba(16, [9, 9, 9, 255])),
        ];
        let frame_bytes = write_ico(
            IconType::Ico,
            &imgs,
            WriteOptions {
                png_size_threshold: None,
                ..Default::default()
            },
        )
        .unwrap();
        let ani = build_ani(&[frame_bytes], 0, 12, None, None, None);
        let anim = read_ani(&ani).unwrap();
        let primary = anim.frames[0].primary_image().unwrap();
        assert_eq!((primary.width, primary.height), (16, 16));
        // Ico frame → no hotspot regardless of sub-images.
        assert_eq!(anim.frames[0].hotspot(), None);
    }

    #[test]
    fn ani_cur_frame_hotspot_surfaces_per_frame() {
        // Two CUR frames with *different* hotspots — a renderer must read
        // the hotspot of whichever frame is on screen, not a single
        // animation-wide value.
        let f0 = cur_frame(16, [200, 0, 0, 255], HotSpot { x: 3, y: 4 });
        let f1 = cur_frame(16, [0, 200, 0, 255], HotSpot { x: 11, y: 12 });
        let ani = build_ani(&[f0, f1], 0, 12, None, None, None);
        let anim = read_ani(&ani).unwrap();

        assert_eq!(anim.frames[0].icon_type, IconType::Cur);
        assert_eq!(anim.frames[0].hotspot(), Some(HotSpot { x: 3, y: 4 }));
        assert_eq!(anim.frames[1].hotspot(), Some(HotSpot { x: 11, y: 12 }));
    }

    #[test]
    fn ani_frame_at_step_resolves_seq_indirection() {
        // seq plays 1,0,1 — frame_at_step must follow the seq index into
        // the frame table, returning the *displayed* frame per step.
        let f0 = cur_frame(16, [200, 0, 0, 255], HotSpot { x: 3, y: 4 });
        let f1 = cur_frame(16, [0, 200, 0, 255], HotSpot { x: 11, y: 12 });
        let ani = build_ani(&[f0, f1], 3, 12, Some(&[1, 0, 1]), Some(&[5, 6, 7]), None);
        let anim = read_ani(&ani).unwrap();

        // Step 0 shows frame 1, step 1 shows frame 0, step 2 shows frame 1.
        assert_eq!(anim.hotspot_at_step(0), Some(HotSpot { x: 11, y: 12 }));
        assert_eq!(anim.hotspot_at_step(1), Some(HotSpot { x: 3, y: 4 }));
        assert_eq!(anim.hotspot_at_step(2), Some(HotSpot { x: 11, y: 12 }));

        // frame_at_step ties to step_at_jiffy: jiffy 6 is in step 1's
        // interval [5,11), which displays frame 0 (hotspot 3,4).
        let step = anim.step_at_jiffy(6).unwrap();
        assert_eq!(step, 1);
        let frame = anim.frame_at_step(step).unwrap();
        assert_eq!(frame.hotspot(), Some(HotSpot { x: 3, y: 4 }));

        // Out-of-range step → None, no panic.
        assert!(anim.frame_at_step(3).is_none());
        assert_eq!(anim.hotspot_at_step(99), None);
    }
}
