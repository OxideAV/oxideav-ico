# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

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
