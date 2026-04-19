//! Pure-Rust ICO + CUR (Windows icon / cursor) codec and container.
//!
//! Handles the full modern icon / cursor layout:
//!
//! * Multi-resolution files (N sub-images inside one `.ico` /
//!   `.cur`).
//! * Both **BMP** and **PNG** sub-image encodings. PNG-inside-ICO is
//!   standard for 256×256 entries and common at 128×128; BMP-inside-ICO
//!   is what smaller sizes use (and everything below Windows XP).
//! * `ICO` (`idType == 1`) and `CUR` (`idType == 2`), the latter
//!   carrying a per-image hotspot.
//! * Read → [`IconImage`]s in RGBA, irrespective of the on-disk
//!   encoding. Write ← same, with per-image choice of PNG or BMP via
//!   [`WriteOptions`].
//!
//! Depends on:
//! * [`oxideav_bmp`] for the BMP-inside-ICO path (and the headerless
//!   `DIB` variant with the doubled-height + 1bpp AND-mask layout
//!   that icons require).
//! * [`oxideav_png`] for the PNG-inside-ICO path.
//!
//! The crate registers a single `"ico"` codec (one sub-image per
//! `Packet`) and a single `"ico"` container, matching the
//! `register` shape the rest of the workspace uses.

pub mod codec;
pub mod container;
pub mod reader;
pub mod types;
pub mod writer;

use oxideav_codec::{CodecInfo, CodecRegistry};
use oxideav_container::ContainerRegistry;
use oxideav_core::{CodecCapabilities, CodecId, PixelFormat};

/// Codec id for individual ICO / CUR sub-image frames.
pub const CODEC_ID_STR: &str = "ico";

pub fn register_codecs(reg: &mut CodecRegistry) {
    let caps = CodecCapabilities::video("ico_sw")
        .with_intra_only(true)
        .with_lossless(true)
        // `bIcon*` width/height are u8 → max 256. Larger entries
        // aren't legally representable in the directory.
        .with_max_size(256, 256)
        .with_pixel_formats(vec![PixelFormat::Rgba]);
    reg.register(
        CodecInfo::new(CodecId::new(CODEC_ID_STR))
            .capabilities(caps)
            .decoder(codec::make_decoder)
            .encoder(codec::make_encoder),
    );
}

pub fn register_containers(reg: &mut ContainerRegistry) {
    container::register(reg);
}

pub fn register(codecs: &mut CodecRegistry, containers: &mut ContainerRegistry) {
    register_codecs(codecs);
    register_containers(containers);
}

// ---------------------------------------------------------------------------
// Public standalone surface — read_ico / write_ico for callers that
// don't want to plumb through the codec / container registries.
// ---------------------------------------------------------------------------

pub use reader::read_ico;
pub use types::{HotSpot, IconImage, IconSubFormat, IconType, WriteOptions};
pub use writer::write_ico;

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

    #[test]
    fn roundtrip_multi_resolution_ico_mixed_bmp_png() {
        // 16×16 (below threshold → BMP), 64×64 (at threshold → PNG),
        // 128×128 (above → PNG).
        let sizes = [16u32, 64, 128];
        let images: Vec<IconImage> = sizes
            .iter()
            .map(|&s| IconImage::from_rgba(s, s, checker_rgba(s, s)))
            .collect();

        let bytes = write_ico(IconType::Ico, &images, WriteOptions::default()).unwrap();
        let (ty, decoded) = read_ico(&bytes).unwrap();
        assert_eq!(ty, IconType::Ico);
        assert_eq!(decoded.len(), 3);

        for (i, (got, exp)) in decoded.iter().zip(images.iter()).enumerate() {
            assert_eq!(got.width, exp.width, "entry {i} width");
            assert_eq!(got.height, exp.height, "entry {i} height");
            assert_eq!(
                got.pixels, exp.pixels,
                "entry {i} pixels must roundtrip exactly"
            );
            let expected_fmt = if exp.width.min(exp.height) >= 64 {
                IconSubFormat::Png
            } else {
                IconSubFormat::Bmp
            };
            assert_eq!(got.sub_format, expected_fmt, "entry {i} sub-format");
        }
    }

    #[test]
    fn cur_hotspot_roundtrip() {
        let mut img = IconImage::from_rgba(32, 32, checker_rgba(32, 32));
        img.hotspot = Some(HotSpot { x: 10, y: 12 });
        let bytes = write_ico(IconType::Cur, &[img.clone()], WriteOptions::default()).unwrap();
        let (ty, got) = read_ico(&bytes).unwrap();
        assert_eq!(ty, IconType::Cur);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].hotspot, img.hotspot);
    }

    #[test]
    fn read_rejects_non_ico_magic() {
        let bytes = [1, 2, 3, 4, 5, 6];
        assert!(read_ico(&bytes).is_err());
    }

    #[test]
    fn force_all_bmp_write() {
        let img = IconImage::from_rgba(128, 128, checker_rgba(128, 128));
        let bytes = write_ico(
            IconType::Ico,
            &[img],
            WriteOptions {
                png_size_threshold: None,
            },
        )
        .unwrap();
        let (_, got) = read_ico(&bytes).unwrap();
        assert_eq!(got[0].sub_format, IconSubFormat::Bmp);
    }
}
