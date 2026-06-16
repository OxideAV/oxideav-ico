# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Other

- write_ani_raw: enforce AF_SEQUENCE flag ⇄ `seq ` chunk coherence
- add write_ani — RGBA-side ANI encoder (the encode counterpart to read_ani)

## [0.0.7](https://github.com/OxideAV/oxideav-ico/compare/v0.0.6...v0.0.7) - 2026-06-14

### Other

- AniAnimation wall-clock accessors (total_jiffies/cycle_seconds/step_at_*)
- add read_ani decoded-animation path (frames→RGBA + resolved seq/rate timeline)
- AniFile::raw_bmp_descriptor for AF_ICON-clear (headerless-BMP) frames
- add write_ani_raw — symmetric RIFF/ACON encoder
- add AniInfo::title_str / author_str Latin-1 INFO decoders
- typed step_at_second() seconds-domain step lookup
- typed step_at_jiffy() wall-clock-to-step lookup
- typed cycle_seconds() wall-clock accessor
- range-check anih.iWidth / iHeight / iBitCount advisory fields
- drop release-plz.toml — use release-plz defaults across the workspace
- typed total_jiffies() cycle-length accessor
- reject anih.nPlanes > 1 per ACON spec
- typed playback_steps() accessor — resolved seq/rate/iDispRate table
- cross-check directory wBitCount vs body biBitCount (ICO+BMP)
- reject BMP body biSize outside {40, 108, 124}
- reject BMP body biCompression outside {BI_RGB, BI_BITFIELDS}
- reject BMP body biPlanes outside {0,1}
- reject directory-vs-body sub-image dim mismatch
- bounds-check seq[] step indices against nFrames
- reject BMP body biBitCount outside {0,1,4,8,16,24,32}
- standalone read_ani_raw RIFF/ACON parser + 19 unit tests

### Added

- `AniAnimation::total_jiffies()` / `cycle_seconds()` / `step_at_jiffy(u64)` /
  `step_at_second(f64)` — the decoded-animation counterparts of the existing
  `AniFile` timeline accessors. An `AniAnimation` already holds a fully
  resolved `steps: Vec<AniStep>` table (the `seq ` / `rate` / `iDispRate`
  defaulting was applied when `read_ani` built it, and every step's `jiffies`
  is guaranteed non-zero), so these are a straight sum / cumulative-interval
  walk over `steps` rather than a re-derivation of the defaulting rules — a
  renderer driving the decoded RGBA frames gets cycle length and
  wall-clock→step lookup without going back to the raw `AniFile`. Interval
  semantics match `AniFile::step_at_jiffy` exactly (half-open
  `[start, start + jiffies)`, boundary lands on the next step;
  `jiffy >= total` and non-finite / negative `seconds` rejected). Truth from
  `docs/image/ico/ani-acon-format.md` (1 jiffy = 1/60 s). Four new unit tests
  cover the identity-timeline total, the `seq ` + `rate` total / seconds,
  step-interval boundaries, and the `step_at_second` floor + rejection paths.

- `read_ani(&[u8]) -> Result<AniAnimation>` — fully decode an ANI animated
  cursor: every stored frame's sub-images decoded to RGBA *and* the
  `seq ` / `rate` timeline resolved into a flat playback step table. This
  is the ANI-side counterpart of `read_ico` (which decodes one icon
  resource's sub-images). It walks the RIFF/`ACON` tree via
  `read_ani_raw`, decodes each `LIST 'fram'` `icon` frame via `read_ico`
  (each frame is a complete ICO/CUR resource, so it may carry several
  resolutions — grouped per frame in the new `AniFrame { icon_type,
  images }`), and resolves timing via `AniFile::playback_steps`. The
  returned `AniAnimation { info, frames, steps }` exposes the INFO
  metadata, the decoded frames, and the resolved `Vec<AniStep>` whose
  every `frame_index` is guaranteed in range for `frames`. Only the
  common `AF_ICON`-set path is decodable (each frame has its own ICO
  directory); an `AF_ICON`-clear file (headerless raw BMP frames) is
  rejected with an error directing the caller to
  `AniFile::raw_bmp_descriptor` + a BMP-DIB decoder. Exported behind the
  default-on `registry` feature alongside `read_ico` (it reuses the same
  BMP/PNG sub-image decode path). Truth from
  `docs/image/ico/ani-acon-format.md`. Four new unit tests cover the
  identity timeline, the `seq ` + `rate` + INFO path, `AF_ICON`-clear
  rejection, and non-ANI input rejection.

- `AniFile::raw_bmp_descriptor() -> Result<Option<RawBmpDescriptor>>` —
  resolves the spec's `AF_ICON`-clear (raw-image) ANI path. When
  `bfAttributes & AF_ICON` is clear, each `LIST 'fram'` `icon` chunk holds
  a **headerless** BMP whose pixel geometry lives in `anih`
  (`iWidth` / `iHeight` / `iBitCount` / `nPlanes`), not in the frame bytes
  (per `docs/image/ico/ani-acon-format.md` §bfAttributes; the daubnet ACON
  reference). A caller cannot decode such a frame without those four
  fields. The new accessor surfaces them as a validated `RawBmpDescriptor`
  (also exported): it returns `Ok(None)` for the icon/cursor path
  (`AF_ICON` set — geometry comes from each frame's own ICO/CUR + DIB
  headers there) and, on the raw path, rejects an unset `iWidth` /
  `iHeight` / `iBitCount` (the spec's `0` = "take from frame" sentinel is
  undefined for a headerless frame, since there is no per-frame header to
  defer to) while normalising `nPlanes ∈ {0, 1}` to the single-plane BMP
  value `1`. Exported from the always-on standalone surface (no `registry`
  feature needed). Seven new unit tests cover the icon-path `None`, the
  raw-path descriptor recovery (including an end-to-end write→parse round
  trip), zero-plane normalisation, and the zero-width / zero-height /
  zero-bit-count rejections.
- `write_ani_raw(&AniFile) -> Result<Vec<u8>>` — the symmetric ANI
  encoder, the container-level counterpart to `write_ico_raw`. Serialises
  an `AniFile` back into a RIFF/`ACON` byte stream that `read_ani_raw`
  parses to an equal value: emits the spec's canonical chunk order
  (`anih`, then optional `LIST 'INFO'` / `seq ` / `rate`, then
  `LIST 'fram'`), RIFF-pads odd-length payloads with one zero byte, and
  writes each frame's `icon` body verbatim (this layer never looks inside
  a frame payload — the caller builds each inner ICO/CUR resource with
  `write_ico_raw` first). Mirrors the reader's strictness up front so the
  output can never be a file the reader would reject: `header.n_frames`
  must equal `frames.len()` and sit in `1..=65_536`; `n_steps <= 65_536`;
  `n_planes ∈ {0, 1}`; `i_width` / `i_height ∈ {0} ∪ 1..=256`;
  `i_bit_count ∈ {0, 1, 4, 8, 16, 24, 32}`; every frame payload non-empty;
  a present `sequence` / `rates` array must match the resolved step count
  (`n_steps`, or `n_frames` when `n_steps == 0`) and every `sequence`
  index must be `< n_frames`. Absent optional chunks are omitted entirely
  (no empty `LIST 'INFO'` / `seq ` / `rate`). Exported from the always-on
  standalone surface (no `registry` feature needed). Thirteen new unit
  tests cover the minimal / seq / full-INFO+seq+rate round-trips, odd
  payload padding, absent-chunk omission, the `n_steps == 0` step-count
  fallback, and each rejection path (frame-count mismatch, empty frame,
  out-of-range / mismatched-length `seq `, mismatched-length `rate`, and
  bad `n_planes` / `i_width` / `i_bit_count` header ranges).

- `AniInfo::title_str()` / `AniInfo::author_str()` — convenience
  accessors that decode the raw `LIST 'INFO'` `INAM` / `IART` payload
  bytes into a `String`. The bytes are interpreted as Latin-1 (every
  byte `0x00..=0xFF` maps to `U+0000..=U+00FF`, so the decode is total
  and can't fail on any input — Latin-1 is the lossless lower half of
  the Windows-1252 charset these legacy cursor tools wrote). The
  trailing NUL terminator plus any even-length padding NUL is trimmed
  (`b"My Cursor\0"` → `"My Cursor"`), while interior NULs are preserved
  as `U+0000` so a deliberately embedded NUL doesn't C-string-truncate
  the value. A field that's absent returns `None`; a present-but-empty
  (or all-NUL) field returns `Some("")` — the chunk *was* present, it
  just carried no visible text. The raw `AniInfo::title` / `author`
  `Option<Vec<u8>>` fields stay available for callers needing
  byte-exact access or a different decoder (e.g. byte-exact
  Windows-1252 punctuation). Seven new unit tests cover the
  terminator / double-NUL-padding trim, the no-terminator-kept-verbatim
  case, the empty-and-all-NUL → `Some("")` case, high-Latin-1 byte
  decoding (`0xE9` → `é`, `0xFF` → `ÿ`), interior-NUL preservation, the
  absent → `None` case, and the end-to-end byte-parser round-trip.

- `AniFile::step_at_second(seconds: f64) -> Result<usize>` — the
  seconds-domain counterpart of `step_at_jiffy`, standing in the same
  relation to it as `cycle_seconds` stands to `total_jiffies`. A
  renderer driving playback from a seconds-based wall clock (clock-side
  schedulers, video-clip timelines, UI that thinks in seconds rather
  than 1/60-second jiffies) computes an elapsed-seconds offset into the
  cycle and gets the active step directly, instead of re-deriving the
  spec's 60-jiffies-per-second conversion and handing off to
  `step_at_jiffy` by hand — the `60` literal is fixed in the function
  name so it can't drift across call sites. Conversion is
  `floor(seconds * 60)` jiffies: the floor is the correct rounding
  direction for the half-open `[start, end)` step intervals
  `step_at_jiffy` uses, since a fractional jiffy offset has not yet
  crossed into the next whole-jiffy bucket, so a wall-clock instant
  resolves to the step whose interval contains its whole-jiffy floor.
  Rejects a non-finite or negative `seconds` (a wall-clock offset is
  physically non-negative and finite; NaN in particular is load-bearing
  to reject up front, since every `<` jiffy-boundary comparison against
  a NaN-derived value is false and would otherwise misreport as a "past
  total" error that hides the real caller bug), and a `seconds` so
  large that `floor(seconds * 60)` exceeds `u64::MAX` (caught before the
  `as u64` cast, which would otherwise saturate silently). Otherwise
  delegates to `step_at_jiffy`, inheriting its full error contract (the
  resolved jiffy offset `>= total_jiffies`, plus the `playback_steps`
  rejections: `n_frames = 0`, mismatched `sequence` / `rates` length,
  any zero-jiffy step, identity-fallback past `n_frames`). Eleven new
  unit tests cover whole-second and sub-jiffy fractional bucketing, the
  floor direction, `seconds = 0`, negative / NaN / +inf rejection, the
  at-or-past-cycle-end rejection, the beyond-u64-jiffy-range rejection,
  the inherited zero-jiffy rejection, a `step_at_second(s)` vs
  `step_at_jiffy(floor(s * 60))` cross-check invariant over a fine grid,
  and the byte-parser round-trip end-to-end.
- `AniFile::step_at_jiffy(jiffy: u64) -> Result<usize>` — the
  wall-clock-to-step inverse a renderer driven by an elapsed-jiffy
  counter actually needs at every frame. Step `i` claims the
  half-open interval `[start_i, start_i + step.jiffies)` where
  `start_i` is the cumulative sum of every preceding step's
  duration; a `jiffy` exactly equal to a step boundary flips to the
  next step (matching the spec's "show frame, then advance" edge
  semantics). The accessor delegates to `playback_steps` up front
  so a malformed file (zero-jiffy step, identity-fallback past
  nFrames, mismatched-length sequence / rates) surfaces a single
  deterministic error rather than an ambiguous "active step = ?"
  answer; `jiffy >= total_jiffies` is also rejected up front so a
  renderer with a buggy wall-clock counter (wrapped past cycle end
  or never reset) sees a deterministic error rather than getting
  silently stuck on the last frame forever (the caller is
  responsible for applying `jiffy % total_jiffies` before the
  lookup — looping is a renderer-level concern). Parameter type is
  `u64` to match `total_jiffies`'s return type (a cycle whose total
  exceeds `u32::MAX` can produce a per-cycle elapsed offset that
  doesn't fit a `u32`, so the accessor doesn't force the caller to
  pre-truncate). Twelve new unit tests cover uniform / variable
  rate bucketing, the half-open boundary contract, jiffy = 0,
  jiffy = total / past total rejection, u64-range probes (jiffy
  inside step intervals that don't fit a u32), inherited
  zero-jiffy / identity-past-nFrames / zero-n_frames rejections,
  the cumulative-walk cross-check invariant against
  `playback_steps`, and the byte-parser round-trip end-to-end.

### Fixed

- `read_ani_raw` now validates the `anih.iWidth` / `iHeight` /
  `iBitCount` advisory fields against the same value sets the
  ICO/CUR layer enforces on directory entries: dimensions in
  `1..=256` (with `0` retained as the spec-mandated "take from
  frame" sentinel), bit-depth in `{0, 1, 4, 8, 16, 24, 32}`
  (with `0` retained as the same "take from frame" sentinel).
  The ACON spec describes these as cursor pixel dimensions and
  bits-per-pixel — the bit-depth interpretation matches the
  BMP/ICO sub-image bit-depth set, and a renderer that consults
  `anih.iWidth` (the raw-BMP path when `AF_ICON` is clear) must
  agree with the ICO/CUR layer's `1..=256` invariant. Same
  probe-vs-render hardening shape as the existing
  directory-vs-body dim / bit-depth cross-check on the ICO
  path: an adversarial `iWidth = 0xFFFF_FFFF` is the classic
  "size pulled from user-controlled bytes" smuggling shape that
  would size a renderer allocation past anything real; an
  `iBitCount = 7` doesn't correspond to any renderable DIB
  layout. Six new unit tests cover the rejection paths (above
  256, pathological `0xFFFF_FFFF`, height beyond 256, bpp
  outside the canonical set, bpp = 64) and the explicit
  acceptance paths (`0` sentinel, 256 boundary, every canonical
  bit-depth round-trip).
- `read_ani_raw` now rejects ANI files whose `anih.nPlanes` field
  is greater than `1`. The ACON spec fixes `nPlanes = 1` for every
  animated cursor — multi-plane DIBs were a planar-video relic that
  never reached cursor animation. Same probe-vs-render hardening
  shape as the ICO-path BMP body `biPlanes ∈ {0, 1}` strictness
  check landed earlier: a probe that read the header and decided
  "this is a single-plane animation" must agree with the renderer
  that's about to walk the frame payloads. A header claiming
  e.g. `nPlanes = 7` would either be silently round-tripped into a
  non-spec value or interpreted by some future planar-mode renderer
  the spec doesn't describe — neither outcome is what the caller
  asked for. The `0` carve-out mirrors the BMP-side strictness
  (the wider ICO/ANI ecosystem produces an "unspecified — defer to
  the frame headers" sentinel that the parser tolerates rather than
  rejects). Two new unit tests cover the rejection of `nPlanes > 1`
  and the explicit acceptance of the `nPlanes = 0` tolerance.
- `read_ico_raw` now rejects ICO sub-image entries whose directory
  `wBitCount` and BMP body `biBitCount` are both non-zero and
  disagree. Both fields are already validated against the legal
  `{0, 1, 4, 8, 16, 24, 32}` set in isolation; the new cross-check
  catches the case where the directory advertises (say) `wBitCount =
  8` while the body decodes to `biBitCount = 32`. Both values are
  individually legal but they contradict each other — a probe that
  inspected the directory and decided "this is an 8-bpp icon" would
  disagree with the renderer that's about to parse the body, and a
  writer round-trip would fold the body's value back into a fresh
  directory, producing a file that disagrees with the original.
  Same probe-vs-render hardening shape as the existing `bWidth` /
  `bHeight` directory-vs-body mismatch check, applied to the
  bit-depth field. The `0 == "unspecified — defer to the other
  header"` carve-out applies to both sides: when either field is
  `0`, it is non-assertive and any agreement check is vacuous (the
  legal-range check already enforces "is this a recognised bit
  depth"). The check is gated on the ICO path — for CUR, the
  directory WORD at offset 6 is the hotspot Y rather than a
  `wBitCount` assertion, so a cursor with hotspot Y = 8 and a
  32-bpp BMP body is still legal; gated on the BMP body path —
  PNG bodies have no `biBitCount` field for the directory to
  agree with, so they're unconditionally accepted. Five new unit
  tests cover the disagreement rejection, the directory-side and
  body-side `0` carve-outs, the CUR hotspot-overlap carve-out, and
  the PNG body exemption.
- `read_ico_raw` now rejects BMP-body entries whose
  `BITMAPINFOHEADER.biSize` field falls outside the legal
  ICO-sub-image set `{40 = BITMAPINFOHEADER, 108 = BITMAPV4HEADER,
  124 = BITMAPV5HEADER}`. The 1995 ICO spec mandates the v3
  `BITMAPINFOHEADER` (40 bytes); later Windows tooling accepts v4
  and v5 as drop-in successors whose extra colour-space / gamma /
  endpoint cells sit after the v3 layout and don't perturb the
  fields the ICO renderer reads. Every other value is corrupt for
  the ICO path: the OS/2 `BITMAPCOREHEADER` (12) has 16-bit
  `bcWidth` / `bcHeight` fields that can't carry the
  doubled-height ICO convention and lacks a `biCompression` cell
  entirely; the Adobe-Photoshop `BITMAPV2INFOHEADER` (52) /
  `BITMAPV3INFOHEADER` (56) extensions are not part of Microsoft's
  documented BITMAPINFOHEADER family. Same probe-vs-render
  hardening shape as the existing `biBitCount` / `biPlanes` /
  `biCompression` body checks: a body claiming `biSize = 12`
  shipped through the BMP-DIB code path would route the next 8
  bytes (BITMAPCOREHEADER's `bcWidth` u16 + `bcHeight` u16) into
  the v3 `biWidth` u32 slot — so a fresh re-read on the writer
  side would see arbitrary garbage in every downstream field. PNG
  entries don't have a `biSize` and are exempt; bodies shorter
  than 4 bytes are also exempt (earlier dim / bit-depth checks
  have already taken responsibility for "this isn't a DIB").
  Nine new unit tests cover the BITMAPCOREHEADER / V2 / V3 /
  garbage rejection paths, the V3 / V4 / V5 acceptance paths, the
  PNG-body exemption (the PNG signature's first 4 bytes
  LE-decode outside the legal set — the test asserts this so the
  exemption is load-bearing), and the short-DIB (< 4 bytes) skip
  path.
- `read_ico_raw` now rejects BMP-body entries whose
  `BITMAPINFOHEADER.biCompression` field falls outside
  `{BI_RGB = 0, BI_BITFIELDS = 3}`. The ICO spec mandates
  uncompressed RGB for sub-images; the `BI_BITFIELDS` carve-out
  covers 16-bpp / 32-bpp DIBs that declare explicit per-channel
  masks (the wider Windows ecosystem produces those). Every other
  value — `BI_RLE8 = 1`, `BI_RLE4 = 2`, `BI_JPEG = 4`,
  `BI_PNG = 5`, `BI_ALPHABITFIELDS = 6`, opaque FOURCC video codes —
  is explicitly excluded by the spec for icon sub-images: RLE codecs
  need a per-row state machine no ICO renderer implements, and
  `BI_JPEG` / `BI_PNG` would smuggle a second codec body through
  the BMP-DIB code path while the PNG-magic sniff already routes
  proper PNG bodies via the PNG branch. Same probe-vs-render
  hardening shape as the `biPlanes` body check, the `biBitCount`
  body check, and the `bWidth` / `bHeight` mismatch fix: the
  directory advertises an icon, the renderer parses a header field
  no icon renderer can honour, and a body that smuggles a
  non-icon codec slips past unless the walker rejects it up
  front. Walker now emits `body biCompression = N (must be
  0 = BI_RGB or 3 = BI_BITFIELDS)` rather than emitting an
  `IconEntryRaw` the harness (or any downstream BMP decoder) then
  chokes on. Eight new unit tests cover the BI_RLE8 / BI_RLE4 /
  BI_JPEG / BI_PNG / BI_ALPHABITFIELDS rejection paths, the
  BI_RGB and BI_BITFIELDS acceptance paths, the PNG-body
  exemption (PNG entries have no `biCompression` field — their
  bytes 16..20 sit inside the IHDR width/height and must not trip
  the new check), and the short-DIB (< 20 bytes) skip path.
- `read_ico_raw` now rejects BMP-body entries whose
  `BITMAPINFOHEADER.biPlanes` field falls outside `{0, 1}`. The ICO
  spec mandates `biPlanes = 1` (multi-plane DIBs were a planar-video
  relic that ICO never used); the directory's `wPlanes` is already
  validated against `{0, 1}` for ICO entries, but the BMP body's
  `biPlanes` was previously trusted verbatim. A body claiming
  `biPlanes = 7` is a malformed DIB that the writer would otherwise
  fold back into a fresh directory whose `wPlanes = 7` then fails
  the existing `wPlanes > 1` check on re-read — broken
  parser/writer fixpoint. Same probe-vs-render hardening shape as
  the r198 `biBitCount` body check and the r210 `bWidth` / `bHeight`
  mismatch fix: a probe inspecting the directory sees one value, the
  renderer parses the body and sees another. Walker now emits
  `body biPlanes = N (must be 0 or 1)` — same wording as the
  directory-side `wPlanes` check so a triage grep maps both reports
  to the same root cause. Four new unit tests cover the `biPlanes > 1`
  rejection path, the `biPlanes = 0` ("unspecified") tolerance, the
  canonical `biPlanes = 1` happy-path acceptance, and the
  PNG-body exemption (PNG entries have no `biPlanes` field — their
  byte 12..14 sits inside the IHDR length/type prefix and must not
  trip the new check).
- `read_ico_raw` now rejects entries whose directory-declared
  `bWidth` / `bHeight` byte disagrees with the body-derived sub-image
  dimension (PNG IHDR width/height or BMP `biWidth` /
  halved-`biHeight`). The directory entry's `u8` width / height
  fields with the `0 == 256` convention are an exact assertion of
  the sub-image dim when the raw byte is non-zero; a body that
  reports a different value is the same probe-vs-render attack the
  body-dim range check (entry size in `(0, 256]`), the CUR hotspot
  body-derived check, and the BMP `biBitCount` body check already
  close for adjacent fields: a probe inspecting the directory before
  rendering sees one value, the renderer reading the payload sees
  another. The walker now produces a clear
  `directory width N disagrees with body sub-image width M
  (probe-vs-render mismatch)` error instead of emitting an
  `IconEntryRaw` whose `width` / `height` silently override the
  directory. The `bWidth = 0` (canonical 256-encoding) carve-out is
  preserved: the directory cannot physically encode a literal
  dimension other than 256, so the body is authoritative for that
  case. Four new unit tests cover BMP-width-mismatch,
  BMP-height-mismatch, PNG-width-mismatch, and the canonical-256
  acceptance path (with a second carve-out test covering
  directory-byte-0 paired with a smaller in-range body dim, for
  hand-rolled files where the writer-side `0 == 256` normalisation
  doesn't apply).
- `read_ani_raw` now bounds-checks every `seq ` step index against
  `anih.nFrames`. The spec defines `seq[i]` as a zero-based index into
  the `LIST 'fram'` frame array, but the previous parser stored the
  raw `u32`s verbatim — a renderer that reaches `frames[seq[i]]`
  directly would panic / out-of-bounds-read on an entry `>= nFrames`
  (e.g. the classic adversarial `seq[k] = 0xFFFFFFFF`). The walker
  now rejects the file with a clear `seq[i] = N out of range
  (nFrames = M)` error rather than emit a sequence array that's
  unsafe to dereference. Same probe-vs-render hardening shape as the
  CUR-hotspot body-dim check (r188) and the BMP-body biBitCount
  check (r198). Three new unit tests cover the off-by-one
  `seq[k] == nFrames` case (most common spec misreading), the
  pathological `0xFFFFFFFF` case (the cargo-fuzz crash shape), and
  the positive in-range repeat-and-reorder acceptance path.
- `read_ico_raw` now rejects entries whose BMP body declares a
  `biBitCount` outside the legal `wBitCount` set
  ({0,1,4,8,16,24,32}). The walker previously only validated the
  directory's `wBitCount` field; a BMP body claiming e.g.
  `biBitCount = 72` would slip past, get sniffed into
  `IconEntryRaw.bit_depth`, and the writer would fold that rogue
  value back into a fresh directory — producing a file that fails
  its own re-read check (broken parser/writer fixpoint). Caught by
  the scheduled `ico_raw_parser` cargo-fuzz target (crash
  `591dc2ca…`). Same error wording as the directory-side check so
  triage maps both reports to the same root cause; new unit test
  covers the BMP-body-bad-biBitCount path.

### Added

- `AniFile::cycle_seconds() -> Result<f64>` — wall-clock typed
  accessor returning the full animation cycle length in seconds,
  folding the ACON spec's "1/60 of a second per jiffy" conversion
  into the type system so the `60` literal can't drift across call
  sites and the unit is fixed in the function name. A renderer
  building clock-side scheduling (sleep timers, video-clip lengths,
  UI labels reading "1.5 s loop") that previously had to call
  `total_jiffies()` and divide by `60.0` by hand now gets a single
  typed call. The conversion is exact in `f64` for every cycle
  length the parser can produce: the 65_536-step × `u32::MAX` worst
  case sums to roughly `2.8e14` jiffies, which sits well under the
  `f64` integer-precision boundary at `2^53 ≈ 9.0e15`, so no
  precision loss is possible on parser-accepted input. Reuses
  `total_jiffies()`'s error contract verbatim (`n_frames = 0`,
  mismatched `rates` length, any zero-jiffy step) rather than
  re-deriving the rate / step-count defaulting rules — the rejection
  paths the byte parser doesn't catch on hand-constructed `AniFile`s
  still surface through this accessor. 9 new unit tests cover the
  rate-absent / rate-present / non-integer / `f64` widening positive
  paths, the three rejection branches (zero default, zero rate
  entry, zero `n_frames`), a `total_jiffies / 60.0` cross-check
  invariant (catches accessor drift under future maintenance), and
  an end-to-end byte-parser → accessor round-trip.
- `AniFile::total_jiffies() -> Result<u64>` — typed cycle-length
  accessor returning the sum of every step's resolved duration in
  1/60-second jiffies. Folds the ACON spec's `rate` / `iDispRate` /
  `nSteps` / `nFrames` defaulting rules into a single `u64` so a
  renderer can schedule the next-cycle wake-up, convert to wall-clock
  seconds (`total / 60`), or size a frame-cycle buffer without
  re-summing the `playback_steps` result by hand. The `u32 → u64`
  widening is load-bearing: a worst-case file (the 65_536-step
  allocator cap × `u32::MAX` per-step rate) sums to roughly `2.8e14`,
  which exceeds `u32::MAX` by a factor of 65_536; the u64 holds it
  with 14+ bits of headroom and no `checked_add` is needed. Rejects
  the same hand-constructed-only branches `playback_steps` already
  guards: `n_frames = 0`, mismatched `rates` `Vec` length vs the
  resolved step count, and any per-step duration resolving to `0`
  (a zero-duration step has no defined display behaviour, and folding
  it into the total would mask the bug). The accessor deliberately
  does not consult the `seq ` chunk — per-step duration in the ACON
  spec depends only on the step index, not on the frame the step
  picks; the test suite asserts this invariant by comparing two
  hand-built files with identical rate tables and different sequence
  arrays. 11 new unit tests cover the rate-absent / rate-present /
  `n_steps = 0 → n_frames` / `u32 → u64` widening positive paths,
  the three rejection branches (zero default, zero rate entry,
  mismatched length), the sentinel `n_frames = 0` path, the
  sequence-invariance check, a cross-check that the total equals the
  hand-summed `playback_steps` output (catches accessor drift under
  future maintenance), and an end-to-end byte-parser → accessor
  round-trip.
- `AniFile::playback_steps() -> Result<Vec<AniStep>>` — typed
  multi-step playback table accessor that resolves the ACON spec's
  `seq ` / `rate` / `iDispRate` / `nSteps` defaulting rules into a
  flat `Vec<AniStep { frame_index, jiffies }>` ready for an
  animation loop. Returns `header.n_steps` (or `header.n_frames`
  when that field is zero, per the spec's "= nFrames if no seq
  chunk" default) entries, each one merging the optional `seq[i]`
  → frame index (identity `i` when absent) and the optional
  `rate[i]` → jiffies (`header.i_disp_rate` when absent). The
  accessor rejects three combinations that the byte-side
  `read_ani_raw` walker doesn't catch: (a) any resolved `jiffies`
  value is zero — neither the per-step `rate[i]` nor the
  `i_disp_rate` fallback may be `0`, since a zero-duration step
  has no defined display behaviour and would either burn 100% CPU
  in a poll-based renderer (`if elapsed >= rate[i]` advances
  instantly) or divide-by-zero in a frame-rate normaliser; (b) an
  identity-fallback step `i >= n_frames` — only reachable when the
  header pairs `nSteps > nFrames` with no `seq ` chunk (the spec
  is silent on this combination and the accessor refuses rather
  than fabricate out-of-range indices that would panic
  downstream); (c) hand-constructed `AniFile`s whose `sequence` /
  `rates` `Vec` lengths don't match the resolved step count
  (parser-produced files can't trip this, but a caller building
  the struct by hand could, and silent truncation would mask the
  bug). Also exposes `AniFile::resolved_step_count()` —
  `header.n_steps` with the `0 → n_frames` defaulting applied —
  for callers sizing their own playback arrays. 12 new unit tests
  cover identity-only / `seq`-only / `rate`-only / both-applied
  positive paths, the three rejection branches, the
  `nSteps = 0 → nFrames` default, the `nSteps != nFrames`
  legitimate combination, and the byte-parser → accessor
  end-to-end round-trip.
- Standalone Windows ANI (animated cursor) RIFF/ACON parser:
  `read_ani_raw(&[u8]) -> Result<AniFile>`. Returns the 36-byte
  `anih` ANIHEADER, optional `LIST 'INFO'` title / author, optional
  `seq ` step-sequence override, optional `rate` per-step jiffy
  durations, and the raw bytes of every `icon` chunk inside
  `LIST 'fram'` — ready to feed back into `read_ico_raw` frame by
  frame when `header.frames_are_icons()` is true (the common case).
  Lives behind the same `#![cfg]`-free standalone surface as
  `read_ico_raw`, so image-library consumers can pull in the ANI
  walker without taking the `oxideav-core` framework dep tree.
- 19 new ANI unit tests covering: minimal 3-frame happy path; full
  `seq ` + `rate` + `AF_SEQUENCE` flag; `LIST 'INFO'` `INAM` / `IART`
  extraction; odd-length payload + RIFF even-padding handling;
  trailing-data-after-RIFF-body tolerance; end-to-end ANI → frame →
  `read_ico_raw` cross-API round-trip with mixed ICO+CUR frames; and
  rejection paths for missing RIFF magic, wrong form type, truncated
  declared size, undersized input, missing `anih`, missing
  `LIST 'fram'`, frame-count mismatch, zero `nFrames`, pathological
  `nFrames` (sanity-cap at 65_536), stray non-`icon` chunk inside
  `LIST 'fram'`, chunk size running past its parent, and `seq `
  before `anih`.
- `read_ico_raw`'s `.ani` rejection message now points callers at
  `oxideav_ico::read_ani_raw` (the new helper) instead of
  dead-ending with "static ICO + CUR only".

## [0.0.6](https://github.com/OxideAV/oxideav-ico/compare/v0.0.5...v0.0.6) - 2026-05-29

### Other

- reject CUR hotspot outside body-derived sub-image dims
- reject sub-image header dims outside ICO 1..=256 range
- standalone-parser fuzz target + read→write→read identity test
- 256×256 PNG round-trip coverage + write dimension guard + select_by_dimensions
- payload-overlap detection + best-fit / largest selection helpers
- harden ICONDIRENTRY validation + detect .ani RIFF/ACON

### Fixed

- `read_ico_raw` now re-validates the CUR hotspot against the
  body-derived sub-image dimensions, not just the directory-declared
  ones. The directory's single-byte width/height fields (with the
  `0 == 256` convention) can legally describe a 256×256 canvas, but
  the actual PNG / BMP body may decode to a much smaller sub-image —
  at which point a hotspot legal against the directory (e.g.
  `(0, 128)` on the 256×256 dummy) is *outside* the real sub-image
  (e.g. 2×33 BMP). Same probe-vs-render shape as r178's body-dim
  fix, but for the hotspot field: a renderer that sees the body's
  dims would crash where a directory-only probe would call the file
  fine. The parser now rejects the file rather than emit an
  `IconEntryRaw` whose `hotspot` falls outside the recovered
  `width × height`. Caught by `ico_raw_parser` cargo-fuzz target
  (crash `10593ac8…`); three new unit tests cover the BMP-body
  case (the fuzz crash itself), the symmetric PNG-IHDR-body case,
  and the happy-path "hotspot legal against both directory and
  body" acceptance.
- `read_ico_raw` no longer accepts entries whose recovered sub-image
  dimensions fall outside the `1..=256` ICO directory range. The
  walker pulls width/height from the payload header (PNG IHDR or DIB
  `biWidth` / doubled `biHeight`) and previously emitted those values
  verbatim, letting a body claim e.g. 2_097_152 px even though the
  `u8` directory fields can only describe `1..=256`. That was the
  classic probe-vs-render shape: a directory walker sees one size, a
  PNG / BMP-aware renderer sees another. The parser now rejects any
  entry whose body-derived dim falls outside `(0, 256]`. Caught by the
  scheduled `ico_raw_parser` cargo-fuzz target. Three new unit tests
  cover the BMP-height-overflow case (the fuzz crash itself), the
  BMP-zero-width case, and the analogous PNG-IHDR oversized-dims case.

### Added

- Second cargo-fuzz target `ico_raw_parser`: drives the standalone
  `read_ico_raw` directory walker on arbitrary fuzz bytes (no codec /
  PNG / BMP-DIB decode in scope) and, on accepted inputs, round-trips
  through `write_ico_raw` + re-parses to assert byte-stability. Sits
  alongside the existing `ico_self_roundtrip` codec-path target —
  together they cover both the validator surface (offset arithmetic,
  payload-overlap detector, RIFF/ACON detection, planes / bit_count
  range checks) and the sub-image encode + decode pair. Asserts every
  parser-guaranteed invariant (icon_type ∈ {Ico, Cur}, non-empty
  entries, dims in `(0, 256]`, CUR-only hotspots, non-empty payloads)
  so a future regression that silently weakens the validator would be
  caught by the harness.
- 3 new unit tests for `read_ico_raw` corner cases: byte-identical
  `read→write→read` fixed-point on a 2-entry mixed-format
  (PNG+BMP-DIB) file; single-entry acceptance (overlap detector's
  inner loop must be a no-op on the first iteration); off-by-one
  payload truncation rejection (most common partial-download failure
  mode); `idCount = 0xFFFF` directory-size-overflow path produces a
  clean truncation error rather than a panic.
- 256×256 PNG sub-image round-trip is now exercised end-to-end:
  `write_ico` serialises the directory width/height as the `0` byte
  (the `0 == 256` convention, since the fields are single bytes) and
  `read_ico` recovers 256 from the PNG body's IHDR. New regression
  test asserts both the `0`-byte directory encoding and a pixel-exact
  256×256 PNG payload round-trip.
- `write_ico` (registry path) now validates each sub-image's
  dimensions are in `1..=256` **before** the PNG / BMP encode, failing
  with a clear `out of 1..=256` error rather than wasting an encode
  pass and relying on the lower-level `write_ico_raw` backstop. New
  test covers a 300×300 rejection.
- `select_by_dimensions(&[IconImage], width, height)` — a strict,
  pixel-exact sub-image lookup that returns the matching entry's index
  or `None` when no entry is exactly that size (no nearest-fit
  substitution; that remains `select_best_fit`'s job). When several
  entries share the requested size the highest bit depth wins, the
  same tiebreaker `select_best_fit` / `select_largest` use. 5 new unit
  tests cover empty, exact-match, no-match, bit-depth tiebreak, and
  order-sensitive non-square cases.
- Cross-entry payload-overlap detection in `read_ico_raw`. Two
  sub-image entries whose `[dwImageOffset, dwImageOffset+dwBytesInRes)`
  byte ranges overlap have been used by attackers to smuggle two
  different bodies through the same offset window (probe sees one
  image, renderer parses another). The parser now rejects any such
  file rather than picking a side.
- `select_best_fit(&[IconImage], target)` and
  `select_largest(&[IconImage])` helpers for picking a sub-image
  from a multi-resolution `.ico`. `select_best_fit` prefers the
  smallest entry whose max-dim is ≥ target, falling back to the
  largest available when every entry is smaller; bit-depth breaks
  ties (32-bpp wins over 1-bpp at the same resolution). Mirrors
  Windows' `LookupIconIdFromDirectoryEx` selection.
- 9 new unit tests covering the overlap rejection, adjacent
  (non-overlapping) payload acceptance, and every selection
  branch (empty, exact target, smallest-above-target, fall-back-
  to-largest, bit-depth tiebreak, non-square entries).
- Directory-entry validation hardening in `read_ico_raw` /
  `write_ico_raw`:
  - `.ani` (RIFF/ACON animated cursor) inputs are detected up front
    and rejected with a clear "different container" error instead of
    a misleading "bad idType 0x4952" downstream failure. Matching
    detection in the registry-side container demuxer.
  - `ICONDIR.idCount = 0` is now rejected.
  - Each `ICONDIRENTRY`'s `bReserved` byte must be zero; `dwBytesInRes`
    must be non-zero; `dwImageOffset` is rejected if it falls inside
    the directory or if `offset + size` overflows / runs past EOF.
  - ICO entries (idType=1): `wPlanes` must be 0 or 1; `wBitCount`
    must be one of {0, 1, 4, 8, 16, 24, 32}. CUR entries (idType=2):
    hotspot `(x, y)` must lie within the declared sub-image bounds
    (both on read and on write).
  - `bColorCount` is rejected when non-zero for >= 16-bpp payloads
    (palette bytes can't fit in `u8`).
- 14 new unit tests covering each rejection path plus the
  always-legal "ICO wBitCount = 0" and "CUR hotspot (0, 0)" tolerance.

## [0.0.5](https://github.com/OxideAV/oxideav-ico/compare/v0.0.4...v0.0.5) - 2026-05-04

### Other

- Standalone-friendly retrofit: gate oxideav-core/bmp/png behind `registry`
- add self-roundtrip cargo-fuzz harness

### Changed

- Standalone-friendly retrofit (#360): `oxideav-core`, `oxideav-bmp`
  and `oxideav-png` are now optional deps behind a default-on
  `registry` cargo feature. Image-library consumers can depend on
  `oxideav-ico` with `default-features = false` to get a framework-free
  build that exposes `read_ico_raw` / `write_ico_raw` plus crate-local
  `IconEntryRaw` / `IcoError` types — directory metadata + raw
  sub-image payload bytes (PNG file or BMP DIB), no decoding. Bring
  your own PNG / BMP-DIB implementation to materialise pixels.
- The `Decoder` / `Encoder` trait surface, the container demuxer /
  muxer, and the `read_ico` / `write_ico` (decoded `IconImage`)
  helpers stay behind the `registry` feature.
- `read_ico` / `write_ico` are now thin wrappers around the new
  `read_ico_raw` / `write_ico_raw` parser plus `oxideav-png` /
  `oxideav-bmp` for the actual sub-image decode + encode. Behaviour
  is unchanged.
- Updated `oxideav-bmp` dep to `0.1.3` for the new
  `decode_dib_videoframe` / `encode_dib_videoframe` compat wrappers.

## [0.0.4](https://github.com/OxideAV/oxideav-ico/compare/v0.0.3...v0.0.4) - 2026-05-03

### Other

- cargo fmt: pending rustfmt cleanup
- replace never-match regex with semver_check = false
- migrate to centralized OxideAV/.github reusable workflows
- adopt slim VideoFrame shape
- pin release-plz to patch-only bumps

## [0.0.3](https://github.com/OxideAV/oxideav-ico/compare/v0.0.2...v0.0.3) - 2026-04-25

### Other

- drop oxideav-codec/oxideav-container shims, import from oxideav-core

## [0.0.2](https://github.com/OxideAV/oxideav-ico/compare/v0.0.1...v0.0.2) - 2026-04-19

### Other

- bump oxideav-bmp + oxideav-png to 0.1
- use the new top-level paths from oxideav-png

### Added

- Initial release: pure-Rust ICO + CUR (Windows icon / cursor) reader
  and writer.
- Multi-resolution icons with mixed BMP and PNG sub-images.
- Read always decodes to top-down RGBA, regardless of on-disk encoding.
- Write lets the caller pick the PNG / BMP boundary via
  `WriteOptions::png_size_threshold` (default 64 px — matches
  Windows 10+ tooling).
- CUR hotspot preserved on both read and write.
- Container + codec registration (`"ico"` codec id, `"ico"`
  container) so ICO files plug into the job-graph / pipeline flow.
- Standalone `read_ico` / `write_ico` API for callers that just want
  bytes in, bytes out.
