# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
