# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
