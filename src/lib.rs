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
//! ## Standalone vs registry-integrated
//!
//! The crate's default `registry` Cargo feature pulls in `oxideav-core`
//! (plus `oxideav-bmp` / `oxideav-png` for sub-image decoding) and
//! exposes the framework `Decoder` / `Encoder` trait surface, the
//! container demuxer / muxer, and the [`read_ico`] / [`write_ico`]
//! helpers that materialise pixels into [`IconImage`]s.
//!
//! Disable the feature (`default-features = false`) for an
//! `oxideav-core`-free build that exposes only the lower-level
//! [`read_ico_raw`] / [`write_ico_raw`] container parser. Those return
//! one [`IconEntryRaw`] per directory entry — directory metadata
//! (width, height, hotspot, source encoding) plus the raw sub-image
//! payload bytes (PNG file or BMP DIB). Bring your own PNG / BMP-DIB
//! implementation to materialise pixels.
//!
//! Depends on (only with `registry`):
//! * [`oxideav_bmp`] for the BMP-inside-ICO path (and the headerless
//!   `DIB` variant with the doubled-height + 1bpp AND-mask layout
//!   that icons require).
//! * [`oxideav_png`] for the PNG-inside-ICO path.

pub mod ani;
#[cfg(feature = "registry")]
pub mod codec;
#[cfg(feature = "registry")]
pub mod container;
pub mod error;
pub mod raw;
#[cfg(feature = "registry")]
pub mod reader;
#[cfg(feature = "registry")]
pub mod registry;
pub mod types;
#[cfg(feature = "registry")]
pub mod writer;

/// Codec id for individual ICO / CUR sub-image frames.
pub const CODEC_ID_STR: &str = "ico";

// ---------------------------------------------------------------------------
// Public standalone surface — always available.
// ---------------------------------------------------------------------------

pub use ani::{
    read_ani_raw, write_ani_raw, AniFile, AniHeader, AniInfo, AniStep, RawBmpDescriptor, AF_ICON,
    AF_SEQUENCE,
};
pub use error::{IcoError, Result};
pub use raw::{read_ico_raw, write_ico_raw, IconEntryRaw};
pub use types::{
    select_best_fit, select_by_dimensions, select_largest, HotSpot, IconImage, IconSubFormat,
    IconType, WriteOptions,
};

// ---------------------------------------------------------------------------
// Registry-side surface — gated behind the default-on `registry` feature.
// ---------------------------------------------------------------------------

#[cfg(feature = "registry")]
pub use reader::{read_ani, read_ico, AniAnimation, AniFrame};
#[cfg(feature = "registry")]
pub use registry::{register, register_codecs, register_containers};
#[cfg(feature = "registry")]
pub use writer::{write_ani, write_ico, AniWriteFrame, AniWriteOptions};

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "registry")]
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

    #[cfg(feature = "registry")]
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

    #[cfg(feature = "registry")]
    #[test]
    fn roundtrip_256x256_png_entry() {
        // The canonical large-icon case: a 256×256 sub-image. The
        // directory's single-byte width/height fields can't hold 256,
        // so they're stored as `0` and recovered on read via the
        // `0 == 256` convention; the PNG body's IHDR carries the true
        // dimensions. This proves the whole 256 path round-trips —
        // directory encode/decode plus PNG payload.
        let img = IconImage::from_rgba(256, 256, checker_rgba(256, 256));
        let bytes = write_ico(
            IconType::Ico,
            std::slice::from_ref(&img),
            WriteOptions::default(),
        )
        .unwrap();

        // The directory byte for width (offset 6) and height (offset 7)
        // must both be 0 — the `256 → 0` directory encoding.
        assert_eq!(bytes[6], 0, "256 width must serialise as the 0 byte");
        assert_eq!(bytes[7], 0, "256 height must serialise as the 0 byte");

        let (ty, decoded) = read_ico(&bytes).unwrap();
        assert_eq!(ty, IconType::Ico);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].width, 256);
        assert_eq!(decoded[0].height, 256);
        assert_eq!(decoded[0].sub_format, IconSubFormat::Png);
        assert_eq!(
            decoded[0].pixels, img.pixels,
            "256×256 pixels must roundtrip exactly"
        );
    }

    #[cfg(feature = "registry")]
    #[test]
    fn write_rejects_dimension_above_256() {
        // 300×300 cannot be expressed in the single-byte directory
        // fields, so the writer must reject it up front with a clear
        // dimension error rather than emitting a corrupt directory.
        let img = IconImage::from_rgba(300, 300, checker_rgba(300, 300));
        let err = write_ico(IconType::Ico, &[img], WriteOptions::default()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("1..=256"),
            "expected a 1..=256 dimension error, got: {msg}"
        );
    }

    #[cfg(feature = "registry")]
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

    #[cfg(feature = "registry")]
    #[test]
    fn read_rejects_non_ico_magic() {
        let bytes = [1, 2, 3, 4, 5, 6];
        assert!(read_ico(&bytes).is_err());
    }

    #[cfg(feature = "registry")]
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

    /// The `read_ico_raw` standalone parser must catch a non-ICO magic
    /// without involving any decoder. Available even with the
    /// `registry` feature off.
    #[test]
    fn raw_parser_rejects_non_ico_magic() {
        let bytes = [1, 2, 3, 4, 5, 6];
        assert!(read_ico_raw(&bytes).is_err());
    }
}
