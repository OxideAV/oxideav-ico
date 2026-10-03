//! Pure-Rust ICO + CUR (Windows icon / cursor) and ANI (animated
//! cursor) reader / writer, following the OxideAV image-crate API
//! contract (`IMAGE_CRATE_API`).
//!
//! Handles the full modern icon / cursor layout:
//!
//! * Multi-resolution files (N sub-images inside one `.ico` / `.cur`).
//! * Both **DIB** and **PNG** sub-image encodings. PNG-inside-ICO is
//!   the convention for 256×256 entries; DIB-inside-ICO (a
//!   `BITMAPINFOHEADER`, colour table, XOR colour bits and a 1-bpp AND
//!   mask) is what smaller sizes and every pre-Vista loader use.
//! * `ICO` (`idType == 1`) and `CUR` (`idType == 2`), the latter
//!   carrying a per-image hotspot.
//! * Every sub-image decodes to a packed `Rgba` [`IcoImage`] — the
//!   composition of XOR colour and AND mask the format defines — with
//!   its on-disk depth, encoding and hotspot kept as extras for a
//!   faithful re-encode.
//!
//! ## Standalone use
//!
//! ```no_run
//! let bytes = std::fs::read("app.ico")?;
//! if oxideav_ico::probe(&bytes) {
//!     let info = oxideav_ico::info(&bytes)?;      // directory walk, no pixels
//!     let img = oxideav_ico::decode(&bytes)?;     // largest entry, Rgba
//!     let rgba: Vec<u8> = img.to_rgba8();
//!     let out = oxideav_ico::encode_rgba8(img.width(), img.height(), &rgba,
//!         &oxideav_ico::EncodeOptions::default())?;
//!     std::fs::write("copy.ico", out)?;
//!     for frame in oxideav_ico::decode_all(&bytes)? { let _ = frame.image; }
//!     let _ = info.frames;
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The whole contract surface — `probe`, `info`, `decode*`, `encode*`,
//! [`IcoImage`], [`DecodeOptions`], [`EncodeOptions`] — works with
//! `default-features = false`: the sub-image codecs (`oxideav-bmp` for
//! DIB payloads, `oxideav-png` for embedded PNG payloads) are image
//! crates with their own standalone layer and are plain dependencies
//! here. The default-on `registry` feature adds `oxideav-core` and the
//! framework adapters ([`register`], [`make_decoder`], [`make_encoder`],
//! the `"ico"` / `"ani"` containers, the `VideoFrame` bridge).
//!
//! ## Depth API
//!
//! Beneath the contract floor: [`read_ico_raw`] / [`write_ico_raw`] (the
//! directory and raw payloads, no decode), [`decode_entry`] /
//! [`encode_entry`] (one payload), the `select_*` entry pickers, the
//! low-depth DIB encoders, and the ANI animated-cursor model
//! ([`read_ani`] / [`write_ani`] / [`read_ani_raw`] / [`write_ani_raw`]).

pub mod ani;
mod api;
#[cfg(feature = "registry")]
pub mod codec;
#[cfg(feature = "registry")]
pub mod container;
pub mod error;
pub mod image;
mod options;
pub mod raw;
pub mod reader;
#[cfg(feature = "registry")]
pub mod registry;
pub mod types;
pub mod writer;

/// Codec id for individual ICO / CUR sub-image frames.
pub const CODEC_ID_STR: &str = "ico";

// ---- The image-crate contract (IMAGE_CRATE_API) ---------------------------
// Root vocabulary, identical across every oxideav image crate; works
// with `default-features = false`.
pub use api::{
    decode, decode_all, decode_all_with, decode_from, decode_rgb8, decode_rgba8, decode_with,
    encode, encode_all, encode_rgb8, encode_rgba8, encode_to, info, probe,
};
pub use error::{Error, IcoError, Result};
pub use image::{
    ColorInfo, ColorRange, Frame, IcoEntryInfo, IcoImage, IcoPixelFormat, ImageInfo, Metadata,
    PixelFormat, Plane, RgbImage, RgbaImage,
};
pub use options::{DecodeOptions, EncodeOptions, EntrySelection};

// ---- ICO-specific depth (the contract is a floor, not a ceiling) ----------
pub use ani::{
    read_ani_raw, write_ani_raw, AniFile, AniHeader, AniInfo, AniStep, RawBmpDescriptor, AF_ICON,
    AF_SEQUENCE,
};
pub use api::{decode_entry, decode_payload, encode_entry, encode_images};
pub use raw::{
    encode_indexed_dib_body, encode_rgb24_dib_body, quantise_rgba_to_indexed, read_ico_raw,
    write_ico_raw, IconEntryRaw, PaletteEntry,
};
pub use reader::{read_ani, AniAnimation, AniFrame};
pub use types::{
    select_best_fit, select_best_fit_raw, select_by_dimensions, select_by_dimensions_raw,
    select_largest, select_largest_raw, BmpBitDepth, HotSpot, IconSubFormat, IconType,
};
pub use writer::{
    write_ani, write_ani_raw_frames, AniRawWriteOptions, AniWriteFrame, AniWriteOptions,
    RawFrameBitDepth,
};

// ---- Deprecated pre-contract names (one release) --------------------------
#[allow(deprecated)]
pub use reader::read_ico;
#[allow(deprecated)]
pub use types::{IconImage, WriteOptions};
#[allow(deprecated)]
pub use writer::write_ico;

// ---- Registry-gated framework surface -------------------------------------
#[cfg(feature = "registry")]
pub use codec::{make_decoder, make_encoder, IcoDecoder, IcoEncoder};
#[cfg(feature = "registry")]
pub use registry::{
    __oxideav_entry, from_color_signal, register, register_codecs, register_containers,
    register_registries, to_color_signal, to_core_pixel_format,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn checker_rgba(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let q = ((x & 1) + 2 * (y & 1)) as usize;
                let rgba = [
                    [255u8, 0, 0, 255],
                    [0, 255, 0, 255],
                    [0, 0, 255, 200],
                    [255, 255, 255, 128],
                ][q];
                v.extend_from_slice(&rgba);
            }
        }
        v
    }

    fn checker(w: u32, h: u32) -> IcoImage {
        IcoImage::from_rgba8(w, h, checker_rgba(w, h)).unwrap()
    }

    fn dib_only() -> EncodeOptions {
        EncodeOptions::new().with_png_size_threshold(None)
    }

    fn images(bytes: &[u8]) -> Vec<IcoImage> {
        decode_all(bytes)
            .unwrap()
            .into_iter()
            .map(|f| f.image)
            .collect()
    }

    #[test]
    fn roundtrip_multi_resolution_ico_mixed_dib_png() {
        // 16×16 (below threshold → DIB), 64×64 (at threshold → PNG),
        // 128×128 (above → PNG).
        let sizes = [16u32, 64, 128];
        let imgs: Vec<IcoImage> = sizes.iter().map(|&s| checker(s, s)).collect();
        let opts = EncodeOptions::new().with_png_size_threshold(64);
        let bytes = encode_images(&imgs, &opts).unwrap();
        assert!(probe(&bytes));
        let meta = info(&bytes).unwrap();
        assert_eq!(meta.frames, 3);
        assert_eq!(meta.icon_type, IconType::Ico);
        assert_eq!((meta.width, meta.height), (128, 128), "primary = largest");
        assert_eq!(meta.primary, 2);
        assert_eq!(meta.entries.len(), 3);
        assert_eq!(meta.entries[0].sub_format, IconSubFormat::Bmp);
        assert_eq!(meta.entries[1].sub_format, IconSubFormat::Png);

        let decoded = images(&bytes);
        assert_eq!(decoded.len(), 3);
        for (i, (got, exp)) in decoded.iter().zip(imgs.iter()).enumerate() {
            assert_eq!(
                (got.width, got.height),
                (exp.width, exp.height),
                "entry {i}"
            );
            assert_eq!(
                got.planes, exp.planes,
                "entry {i} pixels round-trip exactly"
            );
            assert_eq!(got.format, PixelFormat::Rgba);
            let expected_fmt = if exp.width.min(exp.height) >= 64 {
                IconSubFormat::Png
            } else {
                IconSubFormat::Bmp
            };
            assert_eq!(got.sub_format, expected_fmt, "entry {i} sub-format");
            assert_eq!(got.bit_depth, 32);
        }
        // `decode` is the largest entry.
        let primary = decode(&bytes).unwrap();
        assert_eq!((primary.width, primary.height), (128, 128));
        assert_eq!(primary.sub_format, IconSubFormat::Png);
    }

    #[test]
    fn default_threshold_routes_only_256_to_png() {
        let bytes = encode_images(
            &[checker(255, 8), checker(256, 256)],
            &EncodeOptions::default(),
        )
        .unwrap();
        let meta = info(&bytes).unwrap();
        assert_eq!(meta.entries[0].sub_format, IconSubFormat::Bmp);
        assert_eq!(meta.entries[1].sub_format, IconSubFormat::Png);
    }

    #[test]
    fn roundtrip_256x256_png_entry() {
        // The directory's single-byte width/height fields can't hold
        // 256, so they're stored as `0` and recovered via the `0 == 256`
        // convention; the PNG body's IHDR carries the true dimensions.
        let img = checker(256, 256);
        let bytes = encode(&img, &EncodeOptions::default()).unwrap();
        assert_eq!(bytes[6], 0, "256 width must serialise as the 0 byte");
        assert_eq!(bytes[7], 0, "256 height must serialise as the 0 byte");
        let got = decode(&bytes).unwrap();
        assert_eq!((got.width, got.height), (256, 256));
        assert_eq!(got.sub_format, IconSubFormat::Png);
        assert_eq!(got.planes, img.planes, "256×256 pixels round-trip exactly");
        let meta = info(&bytes).unwrap();
        assert_eq!((meta.width, meta.height, meta.frames), (256, 256, 1));
    }

    #[test]
    fn encode_rejects_dimension_above_256_as_unsupported() {
        let img = checker(300, 300);
        let err = encode(&img, &EncodeOptions::default()).unwrap_err();
        assert!(matches!(err, IcoError::Unsupported(_)), "{err}");
        assert!(err.to_string().contains("1..=256"), "{err}");
        let err = encode_rgb8(300, 1, &[0; 900], &EncodeOptions::default()).unwrap_err();
        assert!(matches!(err, IcoError::Unsupported(_)));
    }

    #[test]
    fn encode_raw_paths_validate_lengths() {
        assert!(matches!(
            encode_rgba8(2, 2, &[0; 15], &EncodeOptions::default()),
            Err(IcoError::InvalidData(_))
        ));
        assert!(matches!(
            encode_rgb8(2, 2, &[0; 13], &EncodeOptions::default()),
            Err(IcoError::InvalidData(_))
        ));
        let bytes =
            encode_rgb8(2, 2, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12], &dib_only()).unwrap();
        let rgb = decode_rgb8(&bytes).unwrap();
        assert_eq!(rgb.data, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        let rgba = decode_rgba8(&bytes).unwrap();
        assert_eq!(rgba.stride(), 8);
        assert_eq!(&rgba.data[..4], &[1, 2, 3, 255]);
    }

    #[test]
    fn cur_hotspot_roundtrip() {
        let img = checker(32, 32).with_hotspot(HotSpot { x: 10, y: 12 });
        let opts = EncodeOptions::new().with_icon_type(IconType::Cur);
        let bytes = encode(&img, &opts).unwrap();
        let meta = info(&bytes).unwrap();
        assert_eq!(meta.icon_type, IconType::Cur);
        assert_eq!(meta.hotspot, Some(HotSpot { x: 10, y: 12 }));
        let got = decode(&bytes).unwrap();
        assert_eq!(got.hotspot, img.hotspot);
        // A CUR entry without a hotspot gets (0, 0).
        let bytes = encode(&checker(8, 8), &opts).unwrap();
        assert_eq!(
            decode(&bytes).unwrap().hotspot,
            Some(HotSpot { x: 0, y: 0 })
        );
        // An ICO entry never carries one.
        let bytes = encode(&img, &EncodeOptions::default()).unwrap();
        assert_eq!(decode(&bytes).unwrap().hotspot, None);
    }

    #[test]
    fn probe_and_decode_reject_non_ico_input() {
        assert!(!probe(&[]));
        assert!(!probe(&[1, 2, 3, 4, 5, 6]));
        assert!(!probe(&[0u8; 22]), "idType 0 / idCount 0");
        let mut riff = vec![0u8; 24];
        riff[..4].copy_from_slice(b"RIFF");
        assert!(!probe(&riff));
        assert!(matches!(
            decode(&[1, 2, 3, 4, 5, 6]),
            Err(IcoError::InvalidData(_))
        ));
        assert!(info(&[0u8; 5]).is_err());
        assert!(decode_all(&[0u8; 40]).is_err());
        // A plausible header whose directory is truncated still probes
        // (probe is a sniff) but fails to decode.
        let mut hdr = vec![0u8, 0, 1, 0, 1, 0];
        hdr.extend_from_slice(&[16, 16, 0, 0, 1, 0, 32, 0, 100, 0, 0, 0, 22, 0, 0, 0]);
        assert!(probe(&hdr));
        assert!(decode(&hdr).is_err());
    }

    #[test]
    fn force_all_dib_write() {
        let img = checker(128, 128);
        let bytes = encode(&img, &dib_only()).unwrap();
        let got = decode(&bytes).unwrap();
        assert_eq!(got.sub_format, IconSubFormat::Bmp);
        assert_eq!(got.planes, img.planes);
    }

    #[test]
    fn encode_all_mirrors_decode_all() {
        let frames = vec![Frame::new(checker(8, 8), 0), Frame::new(checker(16, 16), 1)];
        let bytes = encode_all(&frames, &dib_only()).unwrap();
        let back = decode_all(&bytes).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].index, 0);
        assert_eq!(back[1].index, 1);
        assert!(back[0].delay.is_none());
        for (b, f) in back.iter().zip(&frames) {
            // Planes, colour and metadata round-trip; `sub_format` records
            // the encoding actually written (DIB here), not the hint.
            assert_eq!(b.image.planes, f.image.planes);
            assert_eq!(b.image.color, f.image.color);
            assert_eq!(b.image.metadata, f.image.metadata);
            assert_eq!(b.image.sub_format, IconSubFormat::Bmp);
        }
        assert!(matches!(
            encode_all(&[], &dib_only()),
            Err(IcoError::InvalidData(_))
        ));
    }

    #[test]
    fn decode_from_and_encode_to_stream() {
        let img = checker(8, 8);
        let mut buf = Vec::new();
        encode_to(&img, &dib_only(), &mut buf).unwrap();
        let back = decode_from(std::io::Cursor::new(&buf)).unwrap();
        assert_eq!(back.planes, img.planes);
    }

    #[test]
    fn entry_selection_options() {
        let imgs = [
            checker(16, 16).with_bit_depth(1),
            checker(16, 16),
            checker(48, 48),
            checker(32, 32),
        ];
        let bytes = encode_images(&imgs, &dib_only()).unwrap();
        let sel =
            |e: EntrySelection| decode_with(&bytes, &DecodeOptions::new().with_entry(e)).unwrap();
        assert_eq!(sel(EntrySelection::Largest).width, 48);
        assert_eq!(sel(EntrySelection::Index(3)).width, 32);
        assert_eq!(sel(EntrySelection::BestFit(20)).width, 32);
        assert_eq!(
            sel(EntrySelection::BestFit(100)).width,
            48,
            "fallback to largest"
        );
        assert_eq!(sel(EntrySelection::Dimensions(16, 16)).width, 16);
        // Both 16×16 entries are 32-bpp DIBs on disk (per_image_bit_depth
        // off), so the decoded depth is 32 either way.
        assert_eq!(sel(EntrySelection::Dimensions(16, 16)).bit_depth, 32);
        assert!(matches!(
            decode_with(
                &bytes,
                &DecodeOptions::new().with_entry(EntrySelection::Index(4))
            ),
            Err(IcoError::InvalidData(_))
        ));
        assert!(matches!(
            decode_with(
                &bytes,
                &DecodeOptions::new().with_entry(EntrySelection::Dimensions(20, 20))
            ),
            Err(IcoError::InvalidData(_))
        ));
    }

    #[test]
    fn limits_are_enforced_before_decoding() {
        let bytes = encode_images(&[checker(8, 8), checker(32, 32)], &dib_only()).unwrap();
        let w = DecodeOptions::new().with_max_width(16);
        assert!(matches!(
            decode_with(&bytes, &w),
            Err(IcoError::LimitExceeded(_))
        ));
        assert!(matches!(
            decode_all_with(&bytes, &w),
            Err(IcoError::LimitExceeded(_))
        ));
        // The 8×8 entry alone passes the width limit.
        let small = w.with_entry(EntrySelection::Index(0));
        assert_eq!(decode_with(&bytes, &small).unwrap().width, 8);
        let px = DecodeOptions::new().with_max_pixels(100);
        assert!(matches!(
            decode_with(&bytes, &px),
            Err(IcoError::LimitExceeded(_))
        ));
        // max_bytes bounds the sum over decode_all (8*8*4 + 32*32*4 = 4352).
        let sum = DecodeOptions::new().with_max_bytes(4351);
        assert!(matches!(
            decode_all_with(&bytes, &sum),
            Err(IcoError::LimitExceeded(_))
        ));
        assert_eq!(
            decode_all_with(&bytes, &DecodeOptions::new().with_max_bytes(4352))
                .unwrap()
                .len(),
            2
        );
        // The largest entry alone (4096 bytes) still decodes under 4351.
        assert_eq!(decode_with(&bytes, &sum).unwrap().width, 32);
        let h = DecodeOptions::new().with_max_height(8);
        assert!(matches!(
            decode_with(&bytes, &h),
            Err(IcoError::LimitExceeded(_))
        ));
        assert!(decode_with(&bytes, &DecodeOptions::new().unlimited()).is_ok());
    }

    #[test]
    fn strict_rejects_trailing_bytes() {
        let mut bytes = encode(&checker(8, 8), &dib_only()).unwrap();
        bytes.extend_from_slice(b"junk");
        assert!(decode(&bytes).is_ok(), "lenient by default");
        let strict = DecodeOptions::new().with_strict(true);
        assert!(matches!(
            decode_with(&bytes, &strict),
            Err(IcoError::InvalidData(ref s)) if s.contains("trailing")
        ));
        assert!(decode_all_with(&bytes, &strict).is_err());
        bytes.truncate(bytes.len() - 4);
        assert!(decode_with(&bytes, &strict).is_ok());
    }

    #[test]
    fn png_entry_carries_colour_and_metadata() {
        let img = checker(256, 256)
            .with_color(ColorInfo::srgb())
            .with_metadata(
                Metadata::new()
                    .with_gamma(0.45455)
                    .with_xmp(b"<x/>".to_vec()),
            );
        let bytes = encode(&img, &EncodeOptions::default()).unwrap();
        let meta = info(&bytes).unwrap();
        assert_eq!(meta.sub_format, IconSubFormat::Png);
        assert!(meta.has_xmp);
        assert!(!meta.has_icc);
        assert_eq!(meta.color, ColorInfo::srgb());
        let got = decode(&bytes).unwrap();
        assert_eq!(got.color, ColorInfo::srgb());
        assert_eq!(got.metadata.xmp.as_deref(), Some(&b"<x/>"[..]));
        assert!(got.metadata.gamma.is_some());
        assert_eq!(got.planes, img.planes);
        // embed_metadata = false drops the blobs; a DIB cannot carry them.
        let bytes = encode(&img, &EncodeOptions::default().with_embed_metadata(false)).unwrap();
        assert!(decode(&bytes).unwrap().metadata.xmp.is_none());
        let bytes = encode(&img, &dib_only()).unwrap();
        let got = decode(&bytes).unwrap();
        assert!(got.metadata.is_empty());
        assert_eq!(got.color, ColorInfo::ico_default());
        let meta = info(&bytes).unwrap();
        assert_eq!(meta.color, ColorInfo::ico_default());
        assert!(!meta.has_xmp);
    }

    /// Indexed (8-bpp) DIB sub-image, end to end through the raw
    /// encoders and `write_ico_raw`, decoded by `decode_all`.
    #[test]
    fn indexed_8bpp_dib_pixel_exact_roundtrip() {
        let rgba = vec![
            255, 0, 0, 255, // (0,0) red
            0, 255, 0, 255, // (1,0) green
            0, 0, 255, 255, // (0,1) blue
            12, 34, 56, 0, // (1,1) transparent — colour discarded
        ];
        let (pal, idx, transp) = quantise_rgba_to_indexed(2, 2, &rgba, 8).unwrap();
        assert_eq!(pal.len(), 3, "three opaque colours collected");
        let dib = encode_indexed_dib_body(2, 2, 8, &pal, &idx, &transp).unwrap();
        let entry = IconEntryRaw {
            width: 2,
            height: 2,
            bit_depth: 8,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let im = decode(&bytes).unwrap();
        assert_eq!((im.width, im.height), (2, 2));
        assert_eq!(im.bit_depth, 8);
        assert_eq!(im.sub_format, IconSubFormat::Bmp);
        let px = im.to_rgba8();
        assert_eq!(&px[0..4], &[255, 0, 0, 255], "(0,0) red opaque");
        assert_eq!(&px[4..8], &[0, 255, 0, 255], "(1,0) green opaque");
        assert_eq!(&px[8..12], &[0, 0, 255, 255], "(0,1) blue opaque");
        assert_eq!(px[15], 0, "(1,1) transparent (alpha 0)");
        // The same entry decodes identically through decode_entry.
        let direct = decode_entry(&entry, &DecodeOptions::default()).unwrap();
        assert_eq!(direct, im);
    }

    #[test]
    fn indexed_1bpp_dib_pixel_exact_roundtrip() {
        let rgba = vec![
            255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255,
        ];
        let (pal, idx, transp) = quantise_rgba_to_indexed(2, 2, &rgba, 1).unwrap();
        assert_eq!(pal.len(), 2);
        let dib = encode_indexed_dib_body(2, 2, 1, &pal, &idx, &transp).unwrap();
        let entry = IconEntryRaw {
            width: 2,
            height: 2,
            bit_depth: 1,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let im = decode(&bytes).unwrap();
        assert_eq!(im.bit_depth, 1);
        assert_eq!(im.to_rgba8(), rgba);
    }

    #[test]
    fn rgb24_dib_pixel_exact_roundtrip() {
        let rgb = vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120];
        let transp = vec![false, true, false, false];
        let dib = encode_rgb24_dib_body(2, 2, &rgb, &transp).unwrap();
        let entry = IconEntryRaw {
            width: 2,
            height: 2,
            bit_depth: 24,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let im = decode(&bytes).unwrap();
        assert_eq!(im.bit_depth, 24);
        let px = im.to_rgba8();
        assert_eq!(&px[0..4], &[10, 20, 30, 255], "(0,0) opaque");
        assert_eq!(px[7], 0, "(1,0) transparent via AND mask");
        assert_eq!(&px[8..12], &[70, 80, 90, 255], "(0,1) opaque");
        assert_eq!(&px[12..16], &[100, 110, 120, 255], "(1,1) opaque");
    }

    #[test]
    fn indexed_encode_refuses_too_many_colours_as_unsupported() {
        let img = IcoImage::from_rgba8(
            2,
            2,
            vec![1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255],
        )
        .unwrap();
        let err = encode(&img, &dib_only().with_bmp_bit_depth(BmpBitDepth::Indexed1)).unwrap_err();
        assert!(matches!(err, IcoError::Unsupported(_)), "{err}");
    }

    /// A decoded mixed-depth icon re-encodes faithfully under
    /// `per_image_bit_depth`.
    #[test]
    fn decode_then_reencode_preserves_per_entry_depth() {
        let mono = IcoImage::from_rgba8(
            2,
            2,
            vec![
                0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255,
            ],
        )
        .unwrap()
        .with_bit_depth(1);
        let idx8 = IcoImage::from_rgba8(
            2,
            2,
            vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        )
        .unwrap()
        .with_bit_depth(8);
        let opts = dib_only().with_per_image_bit_depth(true);
        let src = encode_images(&[mono, idx8], &opts).unwrap();
        let decoded = images(&src);
        assert_eq!(decoded[0].bit_depth, 1);
        assert_eq!(decoded[1].bit_depth, 8);
        let reencoded = encode_images(&decoded, &opts).unwrap();
        let again = images(&reencoded);
        assert_eq!(again[0].bit_depth, 1, "1-bpp entry stays 1-bpp");
        assert_eq!(again[1].bit_depth, 8, "8-bpp entry stays 8-bpp");
        assert_eq!(again[0].planes, decoded[0].planes);
        assert_eq!(again[1].planes, decoded[1].planes);
    }

    #[test]
    fn raw_parser_rejects_non_ico_magic() {
        let bytes = [1, 2, 3, 4, 5, 6];
        assert!(read_ico_raw(&bytes).is_err());
    }

    #[test]
    fn hostile_directory_counts_never_allocate_past_limits() {
        // 65535 claimed entries, 22 bytes of input: fails cleanly.
        let mut bytes = vec![0u8, 0, 1, 0, 0xFF, 0xFF];
        bytes.extend_from_slice(&[0u8; 16]);
        let _ = probe(&bytes); // total, never panics
        assert!(info(&bytes).is_err());
        assert!(decode(&bytes).is_err());
        assert!(decode_all(&bytes).is_err());
    }

    #[test]
    #[allow(deprecated)]
    fn deprecated_wrappers_still_work() {
        let legacy = IconImage::from_rgba(8, 8, checker_rgba(8, 8)).with_bit_depth(32);
        let bytes = write_ico(
            IconType::Ico,
            std::slice::from_ref(&legacy),
            WriteOptions {
                png_size_threshold: None,
                ..Default::default()
            },
        )
        .unwrap();
        let (ty, got) = read_ico(&bytes).unwrap();
        assert_eq!(ty, IconType::Ico);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].pixels, legacy.pixels);
        assert_eq!(got[0].sub_format, IconSubFormat::Bmp);
        // The legacy default threshold (64) is preserved on the wrapper.
        let big = IconImage::from_rgba(64, 64, checker_rgba(64, 64));
        let bytes = write_ico(IconType::Ico, &[big], WriteOptions::default()).unwrap();
        assert_eq!(
            read_ico(&bytes).unwrap().1[0].sub_format,
            IconSubFormat::Png
        );
        // Conversions both ways.
        let modern = IcoImage::try_from(legacy.clone()).unwrap();
        assert_eq!(modern.to_rgba8(), legacy.pixels);
        let back = IconImage::from(modern);
        assert_eq!(back.pixels, legacy.pixels);
        assert!(write_ico(IconType::Ico, &[], WriteOptions::default()).is_err());
        let bad = IconImage::from_rgba(4, 4, vec![0; 3]);
        assert!(write_ico(IconType::Ico, &[bad], WriteOptions::default()).is_err());
    }
}
