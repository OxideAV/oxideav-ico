//! Stub crate root for the cargo-fuzz harness.
//!
//! ICO is self-roundtrip only — there's no system library to dlopen,
//! so unlike e.g. `oxideav-webp-fuzz` this `lib.rs` carries no
//! interop. cargo-fuzz still requires a valid library target for the
//! `[[bin]]` entries to link against, hence this placeholder.

pub fn _placeholder() {}
