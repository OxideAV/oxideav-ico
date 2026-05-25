# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
