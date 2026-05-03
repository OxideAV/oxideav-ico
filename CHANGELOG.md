# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.5](https://github.com/OxideAV/oxideav-ico/compare/v0.0.4...v0.0.5) - 2026-05-03

### Other

- add self-roundtrip cargo-fuzz harness

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
