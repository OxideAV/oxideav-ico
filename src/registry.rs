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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{write_ico, IconImage, IconType, WriteOptions};
    use oxideav_core::{CodecParameters, NullCodecResolver};
    use std::io::Cursor;

    fn registries() -> (CodecRegistry, ContainerRegistry) {
        let mut codecs = CodecRegistry::new();
        let mut containers = ContainerRegistry::new();
        register(&mut codecs, &mut containers);
        (codecs, containers)
    }

    fn small_ico() -> Vec<u8> {
        let img = IconImage::from_rgba(
            8,
            8,
            std::iter::repeat([3u8, 4, 5, 255])
                .take(64)
                .flatten()
                .collect(),
        );
        write_ico(
            IconType::Ico,
            &[img],
            WriteOptions {
                png_size_threshold: None,
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn register_codecs_wires_decoder_and_encoder() {
        let (codecs, _) = registries();
        let id = CodecId::new(crate::CODEC_ID_STR);
        assert!(codecs.has_decoder(&id), "ico decoder must be registered");
        assert!(codecs.has_encoder(&id), "ico encoder must be registered");

        // The registered factories must build working codecs.
        let mut params = CodecParameters::video(id.clone());
        params.width = Some(8);
        params.height = Some(8);
        assert!(codecs.first_decoder(&params).is_ok());
        assert!(codecs.first_encoder(&params).is_ok());
    }

    #[test]
    fn register_containers_wires_ico_and_ani_names() {
        let (_, containers) = registries();
        let dmx: Vec<&str> = containers.demuxer_names().collect();
        assert!(dmx.contains(&"ico"), "ico demuxer registered: {dmx:?}");
        assert!(dmx.contains(&"ani"), "ani demuxer registered: {dmx:?}");
        let mux: Vec<&str> = containers.muxer_names().collect();
        assert!(mux.contains(&"ico"), "ico muxer registered: {mux:?}");
        // ANI is demux-only (no framework muxer), so it must NOT appear.
        assert!(!mux.contains(&"ani"), "ani must be demux-only: {mux:?}");
    }

    #[test]
    fn extensions_route_to_the_right_container() {
        let (_, containers) = registries();
        assert_eq!(containers.container_for_extension("ico"), Some("ico"));
        assert_eq!(containers.container_for_extension("cur"), Some("ico"));
        assert_eq!(containers.container_for_extension("ani"), Some("ani"));
        // Case-insensitive per the registry's lowercasing.
        assert_eq!(containers.container_for_extension("ICO"), Some("ico"));
    }

    #[test]
    fn probe_input_routes_ico_magic_to_ico_container() {
        let (_, containers) = registries();
        let bytes = small_ico();
        let mut input = Cursor::new(bytes);
        let name = containers.probe_input(&mut input, None).unwrap();
        assert_eq!(name, "ico");
    }

    #[test]
    fn probe_input_routes_ani_magic_to_ani_container() {
        let (_, containers) = registries();
        // Minimal RIFF/ACON shell (probe only inspects the 12-byte magic).
        let mut bytes = vec![0u8; 12];
        bytes[..4].copy_from_slice(b"RIFF");
        bytes[8..12].copy_from_slice(b"ACON");
        let mut input = Cursor::new(bytes);
        let name = containers.probe_input(&mut input, None).unwrap();
        assert_eq!(name, "ani");
    }

    #[test]
    fn open_demuxer_via_registry_yields_packets() {
        let (_, containers) = registries();
        let bytes = small_ico();
        let mut dx = containers
            .open_demuxer("ico", Box::new(Cursor::new(bytes)), &NullCodecResolver)
            .unwrap();
        assert_eq!(dx.streams().len(), 1);
        assert!(dx.next_packet().is_ok());
    }

    #[test]
    fn error_conversion_preserves_variant_and_message() {
        // The From<IcoError> bridge the trait impls rely on must map
        // variants 1:1 and keep the message verbatim.
        let inv: oxideav_core::Error = IcoError::invalid("boom").into();
        match inv {
            oxideav_core::Error::InvalidData(s) => assert_eq!(s, "boom"),
            other => panic!("expected InvalidData, got {other:?}"),
        }
        let uns: oxideav_core::Error = IcoError::unsupported("nope").into();
        match uns {
            oxideav_core::Error::Unsupported(s) => assert_eq!(s, "nope"),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
