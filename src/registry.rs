//! `oxideav-core` integration layer for `oxideav-ico`.
//!
//! Gated behind the default-on `registry` feature so image-library
//! consumers can depend on `oxideav-ico` with `default-features = false`
//! and skip the framework dependency tree (and the `oxideav-bmp` /
//! `oxideav-png` codec deps that the registry side uses) entirely.
//!
//! The module exposes:
//! * [`register`] / [`register_codecs`] / [`register_containers`] — the
//!   `CodecRegistry` / `ContainerRegistry` entry points the umbrella
//!   `oxideav` crate calls during framework initialisation.
//! * The `From<IcoError> for oxideav_core::Error` conversion that lets
//!   the trait-side `Decoder` / `Encoder` impls (still living in
//!   `codec.rs`) bubble container errors up through the framework
//!   error type.

use oxideav_core::ContainerRegistry;
use oxideav_core::{CodecCapabilities, CodecId, PixelFormat};
use oxideav_core::{CodecInfo, CodecRegistry};

use crate::container;
use crate::error::IcoError;

/// Convert an [`IcoError`] into the framework-shared
/// `oxideav_core::Error` so trait impls in this crate can use `?` on
/// errors returned by the framework-free `read_ico_raw` /
/// `write_ico_raw` functions.
impl From<IcoError> for oxideav_core::Error {
    fn from(e: IcoError) -> Self {
        match e {
            IcoError::InvalidData(s) => oxideav_core::Error::InvalidData(s),
            IcoError::Unsupported(s) => oxideav_core::Error::Unsupported(s),
        }
    }
}

/// Register the ICO codec into the supplied [`CodecRegistry`].
pub fn register_codecs(reg: &mut CodecRegistry) {
    let caps = CodecCapabilities::video("ico_sw")
        .with_intra_only(true)
        .with_lossless(true)
        // `bIcon*` width/height are u8 → max 256. Larger entries
        // aren't legally representable in the directory.
        .with_max_size(256, 256)
        .with_pixel_formats(vec![PixelFormat::Rgba]);
    reg.register(
        CodecInfo::new(CodecId::new(crate::CODEC_ID_STR))
            .capabilities(caps)
            .decoder(crate::codec::make_decoder)
            .encoder(crate::codec::make_encoder),
    );
}

/// Register the ICO container demuxer + muxer + extension + probe
/// into the supplied [`ContainerRegistry`].
pub fn register_containers(reg: &mut ContainerRegistry) {
    container::register(reg);
}

/// Combined registration for callers that just want everything wired up
/// in one call.
pub fn register(codecs: &mut CodecRegistry, containers: &mut ContainerRegistry) {
    register_codecs(codecs);
    register_containers(containers);
}
