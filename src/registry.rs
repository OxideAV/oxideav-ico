//! `oxideav-core` integration layer for `oxideav-ico`.
//!
//! Gated behind the default-on `registry` feature so image-library
//! consumers can depend on `oxideav-ico` with `default-features = false`
//! and skip the `oxideav-core` dependency entirely.
//!
//! The module exposes:
//! * [`register`] / [`register_codecs`] / [`register_containers`] /
//!   [`register_registries`] — the `RuntimeContext` / `CodecRegistry` /
//!   `ContainerRegistry` entry points.
//! * `From<IcoImage> for VideoFrame` and [`IcoImage::from_video_frame`]
//!   — the `Rgba` plane plus the colour-signal side-channel (stamped
//!   only when the sub-image payload actually signalled colour).
//! * The `From<IcoError> for oxideav_core::Error` conversion and the
//!   `CodecOptionsStruct` impl for [`EncodeOptions`].
//!
//! The `Decoder` / `Encoder` adapters and their factories live in
//! [`crate::codec`]; the demuxers / muxer in [`crate::container`].

use oxideav_core::{
    CodecCapabilities, CodecId, CodecInfo, CodecOptionsStruct, CodecParameters, CodecRegistry,
    ColorPrimaries, ColorSignal, ContainerRegistry, MatrixCoefficients, OptionField, OptionKind,
    OptionValue, PixelFormat, RuntimeContext, TransferCharacteristics, VideoFrame, VideoPlane,
};

use crate::container;
use crate::error::IcoError;
use crate::image::{ColorInfo, ColorRange, IcoImage, IcoPixelFormat, Plane};
use crate::options::EncodeOptions;
use crate::types::{BmpBitDepth, IconType};

impl From<IcoError> for oxideav_core::Error {
    fn from(e: IcoError) -> Self {
        match e {
            IcoError::InvalidData(s) => oxideav_core::Error::InvalidData(s),
            IcoError::Unsupported(s) => oxideav_core::Error::Unsupported(s),
            IcoError::LimitExceeded(s) => oxideav_core::Error::ResourceExhausted(s),
            IcoError::Io(e) => oxideav_core::Error::Io(e),
        }
    }
}

// ---- Pixel-format + colour-signal mapping ----

/// The framework pixel format an [`IcoPixelFormat`] maps to by name.
pub fn to_core_pixel_format(pf: IcoPixelFormat) -> PixelFormat {
    match pf {
        IcoPixelFormat::Rgba => PixelFormat::Rgba,
    }
}

/// [`ColorInfo`] as the framework's [`ColorSignal`] (code points map
/// 1:1; `Unspecified` range stays unspecified).
pub fn to_color_signal(c: &ColorInfo) -> ColorSignal {
    let range = match c.range {
        ColorRange::Unspecified => oxideav_core::ColorRange::Unspecified,
        ColorRange::Limited => oxideav_core::ColorRange::Limited,
        ColorRange::Full => oxideav_core::ColorRange::Full,
    };
    ColorSignal::new(
        range,
        ColorPrimaries(c.primaries),
        TransferCharacteristics(c.transfer),
        MatrixCoefficients(c.matrix),
    )
}

/// The inverse of [`to_color_signal`].
pub fn from_color_signal(s: &ColorSignal) -> ColorInfo {
    let range = match s.range {
        oxideav_core::ColorRange::Limited => ColorRange::Limited,
        oxideav_core::ColorRange::Full => ColorRange::Full,
        _ => ColorRange::Unspecified,
    };
    ColorInfo::new(range, s.primaries.0, s.transfer.0, s.matrix.0)
}

// ---- Frame bridge ----

fn image_into_video_frame(mut image: IcoImage) -> VideoFrame {
    let stride = image.stride();
    let data = if image.planes.is_empty() {
        Vec::new()
    } else {
        std::mem::take(&mut image.planes[0].data)
    };
    let mut frame = VideoFrame {
        pts: None,
        planes: vec![VideoPlane { stride, data }],
    };
    // Stamp colour only when the payload signalled it (an embedded PNG's
    // sRGB / cICP / iCCP, a V4 / V5 DIB tag). The crate's own
    // `ico_default` is a convention, not a format definition.
    if image.color.is_specified() {
        frame.set_color_signal(to_color_signal(&image.color));
    }
    frame
}

impl From<IcoImage> for VideoFrame {
    /// The `Rgba` plane (`pts` `None`), plus the colour-signal
    /// side-channel when the sub-image payload signalled a colour space.
    fn from(image: IcoImage) -> Self {
        image_into_video_frame(image)
    }
}

impl From<&IcoImage> for VideoFrame {
    fn from(image: &IcoImage) -> Self {
        image_into_video_frame(image.clone())
    }
}

impl IcoImage {
    /// Rebuild a sub-image from a framework frame and the stream
    /// parameters that describe it (`width` and `height` are required;
    /// `pixel_format` defaults to `Rgba`). `Rgb24` / `Bgra` / `Bgr24`
    /// frames are widened / swizzled to the crate's one native layout
    /// (exact); any other layout is [`IcoError::Unsupported`]. The
    /// frame's colour-signal side-channel, when attached, becomes
    /// `color`.
    pub fn from_video_frame(
        frame: &VideoFrame,
        params: &CodecParameters,
    ) -> crate::error::Result<Self> {
        let width = params
            .width
            .ok_or_else(|| IcoError::invalid("ICO: missing width in CodecParameters"))?;
        let height = params
            .height
            .ok_or_else(|| IcoError::invalid("ICO: missing height in CodecParameters"))?;
        let pf = params.pixel_format.unwrap_or(PixelFormat::Rgba);
        let plane = frame
            .image_planes()
            .first()
            .ok_or_else(|| IcoError::invalid("ICO: frame has no planes"))?;
        let w = width as usize;
        let h = height as usize;
        let need = |bpp: usize| -> crate::error::Result<()> {
            if plane.stride < w * bpp || plane.data.len() < plane.stride * h {
                return Err(IcoError::invalid(format!(
                    "ICO: {width}×{height} {pf:?} frame needs a {}-byte stride and {} bytes, \
                     got stride {} and {} bytes",
                    w * bpp,
                    plane.stride * h,
                    plane.stride,
                    plane.data.len()
                )));
            }
            Ok(())
        };
        let rgba: Vec<u8> = match pf {
            PixelFormat::Rgba => {
                need(4)?;
                let mut out = Vec::with_capacity(w * h * 4);
                for y in 0..h {
                    out.extend_from_slice(&plane.data[y * plane.stride..y * plane.stride + w * 4]);
                }
                out
            }
            PixelFormat::Bgra => {
                need(4)?;
                let mut out = Vec::with_capacity(w * h * 4);
                for y in 0..h {
                    for px in plane.data[y * plane.stride..y * plane.stride + w * 4].chunks_exact(4)
                    {
                        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                    }
                }
                out
            }
            PixelFormat::Rgb24 => {
                need(3)?;
                let mut out = Vec::with_capacity(w * h * 4);
                for y in 0..h {
                    for px in plane.data[y * plane.stride..y * plane.stride + w * 3].chunks_exact(3)
                    {
                        out.extend_from_slice(&[px[0], px[1], px[2], 0xFF]);
                    }
                }
                out
            }
            PixelFormat::Bgr24 => {
                need(3)?;
                let mut out = Vec::with_capacity(w * h * 4);
                for y in 0..h {
                    for px in plane.data[y * plane.stride..y * plane.stride + w * 3].chunks_exact(3)
                    {
                        out.extend_from_slice(&[px[2], px[1], px[0], 0xFF]);
                    }
                }
                out
            }
            other => {
                return Err(IcoError::unsupported(format!(
                    "ICO: pixel format {other:?} (expected Rgba, Bgra, Rgb24 or Bgr24)"
                )))
            }
        };
        let mut img = IcoImage::new(
            width,
            height,
            IcoPixelFormat::Rgba,
            vec![Plane::new(w * 4, rgba)],
        )?;
        if let Some(sig) = frame.color_signal() {
            img.color = from_color_signal(&sig);
        }
        Ok(img)
    }
}

impl TryFrom<(&VideoFrame, &CodecParameters)> for IcoImage {
    type Error = IcoError;
    fn try_from((frame, params): (&VideoFrame, &CodecParameters)) -> crate::error::Result<Self> {
        IcoImage::from_video_frame(frame, params)
    }
}

// ---- CodecOptionsStruct (registry-only schema for EncodeOptions) ----

const BIT_DEPTH_NAMES: &[&str] = &["bgra32", "rgb24", "indexed8", "indexed4", "indexed1"];
const ICON_TYPE_NAMES: &[&str] = &["ico", "cur"];

impl CodecOptionsStruct for EncodeOptions {
    const SCHEMA: &'static [OptionField] = &[
        OptionField {
            name: "png_size_threshold",
            kind: OptionKind::U32,
            default: OptionValue::U32(EncodeOptions::DEFAULT_PNG_SIZE_THRESHOLD),
            help: "Write a sub-image whose smaller side is >= this many pixels as an \
                   embedded PNG, smaller ones as a DIB. 0 = never PNG (DIB everywhere).",
        },
        OptionField {
            name: "bmp_bit_depth",
            kind: OptionKind::Enum(BIT_DEPTH_NAMES),
            default: OptionValue::String(String::new()),
            help: "DIB depth: bgra32 (default, lossless), rgb24, indexed8, indexed4, indexed1.",
        },
        OptionField {
            name: "per_image_bit_depth",
            kind: OptionKind::Bool,
            default: OptionValue::Bool(false),
            help: "Encode each DIB sub-image at its own source bit depth.",
        },
        OptionField {
            name: "embed_metadata",
            kind: OptionKind::Bool,
            default: OptionValue::Bool(true),
            help: "Embed ICC / Exif / XMP in PNG-routed sub-images.",
        },
        OptionField {
            name: "icon_type",
            kind: OptionKind::Enum(ICON_TYPE_NAMES),
            default: OptionValue::String(String::new()),
            help: "Directory type for whole-file writes: ico (default) or cur.",
        },
    ];
    fn apply(&mut self, key: &str, v: &OptionValue) -> oxideav_core::Result<()> {
        match key {
            "png_size_threshold" => {
                let n = v.as_u32()?;
                self.png_size_threshold = if n == 0 { None } else { Some(n) };
            }
            "bmp_bit_depth" => {
                self.bmp_bit_depth = match v.as_str()? {
                    "bgra32" => BmpBitDepth::Bgra32,
                    "rgb24" => BmpBitDepth::Rgb24,
                    "indexed8" => BmpBitDepth::Indexed8,
                    "indexed4" => BmpBitDepth::Indexed4,
                    "indexed1" => BmpBitDepth::Indexed1,
                    _ => unreachable!("guarded by SCHEMA"),
                }
            }
            "per_image_bit_depth" => self.per_image_bit_depth = v.as_bool()?,
            "embed_metadata" => self.embed_metadata = v.as_bool()?,
            "icon_type" => {
                self.icon_type = match v.as_str()? {
                    "ico" => IconType::Ico,
                    "cur" => IconType::Cur,
                    _ => unreachable!("guarded by SCHEMA"),
                }
            }
            _ => unreachable!("guarded by SCHEMA"),
        }
        Ok(())
    }
}

// ---- Registration ----

/// Register the `"ico"` sub-image codec into the supplied [`CodecRegistry`].
pub fn register_codecs(reg: &mut CodecRegistry) {
    let caps = CodecCapabilities::video("ico_sw")
        .with_intra_only(true)
        .with_lossless(true)
        .with_max_size(256, 256)
        .with_pixel_formats(vec![
            PixelFormat::Rgba,
            PixelFormat::Bgra,
            PixelFormat::Rgb24,
            PixelFormat::Bgr24,
        ]);
    reg.register(
        CodecInfo::new(CodecId::new(crate::CODEC_ID_STR))
            .capabilities(caps)
            .decoder(crate::codec::make_decoder)
            .encoder(crate::codec::make_encoder)
            .encoder_options::<EncodeOptions>(),
    );
}

/// Register the `"ico"` demuxer / muxer and the `"ani"` demuxer (with
/// their probes and extensions) into the supplied [`ContainerRegistry`].
pub fn register_containers(reg: &mut ContainerRegistry) {
    container::register(reg);
}

/// Unified registration entry point — installs the codec into the codec
/// sub-registry and the containers into the container sub-registry of
/// the supplied [`RuntimeContext`].
pub fn register(ctx: &mut RuntimeContext) {
    register_codecs(&mut ctx.codecs);
    register_containers(&mut ctx.containers);
}

/// Register into two separately held registries — the pre-contract
/// two-argument shape `register` used to have.
pub fn register_registries(codecs: &mut CodecRegistry, containers: &mut ContainerRegistry) {
    register_codecs(codecs);
    register_containers(containers);
}

oxideav_core::register!("ico", register);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{encode_images, EncodeOptions, IconType};
    use oxideav_core::{CodecParameters, NullCodecResolver};
    use std::io::Cursor;

    fn context() -> RuntimeContext {
        let mut ctx = RuntimeContext::new();
        register(&mut ctx);
        ctx
    }

    fn small_ico() -> Vec<u8> {
        let img = IcoImage::from_rgba8(
            8,
            8,
            std::iter::repeat([3u8, 4, 5, 255])
                .take(64)
                .flatten()
                .collect(),
        )
        .unwrap();
        encode_images(&[img], &EncodeOptions::new().with_png_size_threshold(None)).unwrap()
    }

    #[test]
    fn register_codecs_wires_decoder_and_encoder() {
        let ctx = context();
        let id = CodecId::new(crate::CODEC_ID_STR);
        assert!(
            ctx.codecs.has_decoder(&id),
            "ico decoder must be registered"
        );
        assert!(
            ctx.codecs.has_encoder(&id),
            "ico encoder must be registered"
        );

        let mut params = CodecParameters::video(id.clone());
        params.width = Some(8);
        params.height = Some(8);
        assert!(ctx.codecs.first_decoder(&params).is_ok());
        assert!(ctx.codecs.first_encoder(&params).is_ok());
    }

    #[test]
    fn register_registries_matches_register() {
        let mut codecs = CodecRegistry::new();
        let mut containers = ContainerRegistry::new();
        register_registries(&mut codecs, &mut containers);
        assert!(codecs.has_decoder(&CodecId::new(crate::CODEC_ID_STR)));
        let dmx: Vec<&str> = containers.demuxer_names().collect();
        assert!(dmx.contains(&"ico") && dmx.contains(&"ani"));
        let mut ctx = RuntimeContext::new();
        crate::__oxideav_entry(&mut ctx);
        assert!(ctx.codecs.has_encoder(&CodecId::new(crate::CODEC_ID_STR)));
    }

    #[test]
    fn register_containers_wires_ico_and_ani_names() {
        let ctx = context();
        let dmx: Vec<&str> = ctx.containers.demuxer_names().collect();
        assert!(dmx.contains(&"ico"), "ico demuxer registered: {dmx:?}");
        assert!(dmx.contains(&"ani"), "ani demuxer registered: {dmx:?}");
        let mux: Vec<&str> = ctx.containers.muxer_names().collect();
        assert!(mux.contains(&"ico"), "ico muxer registered: {mux:?}");
        assert!(!mux.contains(&"ani"), "ani must be demux-only: {mux:?}");
    }

    #[test]
    fn extensions_route_to_the_right_container() {
        let ctx = context();
        assert_eq!(ctx.containers.container_for_extension("ico"), Some("ico"));
        assert_eq!(ctx.containers.container_for_extension("cur"), Some("ico"));
        assert_eq!(ctx.containers.container_for_extension("ani"), Some("ani"));
        assert_eq!(ctx.containers.container_for_extension("ICO"), Some("ico"));
    }

    #[test]
    fn probe_input_routes_ico_magic_to_ico_container() {
        let ctx = context();
        let bytes = small_ico();
        let mut input = Cursor::new(bytes);
        let name = ctx.containers.probe_input(&mut input, None).unwrap();
        assert_eq!(name, "ico");
    }

    #[test]
    fn probe_input_routes_ani_magic_to_ani_container() {
        let ctx = context();
        let mut bytes = vec![0u8; 12];
        bytes[..4].copy_from_slice(b"RIFF");
        bytes[8..12].copy_from_slice(b"ACON");
        let mut input = Cursor::new(bytes);
        let name = ctx.containers.probe_input(&mut input, None).unwrap();
        assert_eq!(name, "ani");
    }

    #[test]
    fn open_demuxer_via_registry_yields_packets() {
        let ctx = context();
        let bytes = small_ico();
        let mut dx = ctx
            .containers
            .open_demuxer("ico", Box::new(Cursor::new(bytes)), &NullCodecResolver)
            .unwrap();
        assert_eq!(dx.streams().len(), 1);
        assert!(dx.next_packet().is_ok());
    }

    #[test]
    fn error_conversion_preserves_variant_and_message() {
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
        let lim: oxideav_core::Error = IcoError::limit("big").into();
        assert!(matches!(lim, oxideav_core::Error::ResourceExhausted(_)));
        let io: oxideav_core::Error = IcoError::from(std::io::Error::other("x")).into();
        assert!(matches!(io, oxideav_core::Error::Io(_)));
    }

    #[test]
    fn frame_bridge_round_trips_and_stamps_only_specified_colour() {
        let img = IcoImage::from_rgba8(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        let frame = VideoFrame::from(img.clone());
        assert_eq!(frame.planes[0].stride, 8);
        assert!(frame.color_signal().is_none(), "ico_default is not stamped");

        let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
        params.width = Some(2);
        params.height = Some(1);
        params.pixel_format = Some(PixelFormat::Rgba);
        let back = IcoImage::from_video_frame(&frame, &params).unwrap();
        assert_eq!(back.to_rgba8(), img.to_rgba8());
        let back2 = IcoImage::try_from((&frame, &params)).unwrap();
        assert_eq!(back2, back);

        let srgb = img.clone().with_color(ColorInfo::srgb());
        let frame = VideoFrame::from(&srgb);
        let sig = frame.color_signal().expect("sRGB is stamped");
        assert_eq!(sig.primaries.0, 1);
        assert_eq!(sig.transfer.0, 13);
        let back = IcoImage::from_video_frame(&frame, &params).unwrap();
        assert_eq!(back.color, ColorInfo::srgb());
    }

    #[test]
    fn from_video_frame_swizzles_other_rgb_layouts() {
        let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
        params.width = Some(1);
        params.height = Some(1);
        let frame = |stride: usize, data: Vec<u8>| VideoFrame {
            pts: None,
            planes: vec![VideoPlane { stride, data }],
        };
        params.pixel_format = Some(PixelFormat::Bgra);
        let im = IcoImage::from_video_frame(&frame(4, vec![1, 2, 3, 4]), &params).unwrap();
        assert_eq!(im.to_rgba8(), vec![3, 2, 1, 4]);
        params.pixel_format = Some(PixelFormat::Rgb24);
        let im = IcoImage::from_video_frame(&frame(3, vec![1, 2, 3]), &params).unwrap();
        assert_eq!(im.to_rgba8(), vec![1, 2, 3, 255]);
        params.pixel_format = Some(PixelFormat::Bgr24);
        let im = IcoImage::from_video_frame(&frame(3, vec![1, 2, 3]), &params).unwrap();
        assert_eq!(im.to_rgba8(), vec![3, 2, 1, 255]);
        params.pixel_format = Some(PixelFormat::Gray8);
        assert!(matches!(
            IcoImage::from_video_frame(&frame(1, vec![1]), &params),
            Err(IcoError::Unsupported(_))
        ));
        params.pixel_format = Some(PixelFormat::Rgba);
        assert!(IcoImage::from_video_frame(&frame(4, vec![1, 2]), &params).is_err());
        params.width = None;
        assert!(IcoImage::from_video_frame(&frame(4, vec![1, 2, 3, 4]), &params).is_err());
    }

    #[test]
    fn encode_options_schema_round_trips_every_key() {
        let mut bag = oxideav_core::CodecOptions::default();
        bag.insert("png_size_threshold", "0");
        bag.insert("bmp_bit_depth", "indexed4");
        bag.insert("per_image_bit_depth", "true");
        bag.insert("embed_metadata", "false");
        bag.insert("icon_type", "cur");
        let o = oxideav_core::parse_options::<EncodeOptions>(&bag).unwrap();
        assert_eq!(o.png_size_threshold, None);
        assert_eq!(o.bmp_bit_depth, BmpBitDepth::Indexed4);
        assert!(o.per_image_bit_depth);
        assert!(!o.embed_metadata);
        assert_eq!(o.icon_type, IconType::Cur);
        let mut bag = oxideav_core::CodecOptions::default();
        bag.insert("png_size_threshold", "48");
        let o = oxideav_core::parse_options::<EncodeOptions>(&bag).unwrap();
        assert_eq!(o.png_size_threshold, Some(48));
    }
}
