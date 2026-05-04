//! ICO / CUR file encoder (registry-side, oxideav-core-using path).
//!
//! Picks PNG or BMP per sub-image according to [`WriteOptions`] (default:
//! PNG for sizes ≥ 64, BMP otherwise), encodes each [`IconImage`] to
//! its packed payload via `oxideav-png` / `oxideav-bmp`, then hands
//! the resulting `IconEntryRaw` batch to the framework-free
//! [`crate::raw::write_ico_raw`] for the directory layout.

use oxideav_core::{PixelFormat, Result, VideoFrame, VideoPlane};

use crate::raw::{write_ico_raw, IconEntryRaw};
use crate::types::*;

/// Serialize a batch of images into a single `.ico` / `.cur` byte
/// stream. The caller is responsible for ensuring every image fits
/// within 1 ≤ dim ≤ 256 (the format's hard limit — stored as `u8` in
/// the directory entry, with 0 meaning 256).
pub fn write_ico(icon_type: IconType, images: &[IconImage], opts: WriteOptions) -> Result<Vec<u8>> {
    if images.is_empty() {
        return Err(oxideav_core::Error::invalid(
            "ICO: must have at least one sub-image",
        ));
    }
    for (i, im) in images.iter().enumerate() {
        if im.pixels.len() != (im.width as usize * im.height as usize * 4) {
            return Err(oxideav_core::Error::invalid(format!(
                "ICO: entry {i} pixel buffer size {} != {}×{}×4",
                im.pixels.len(),
                im.width,
                im.height
            )));
        }
    }

    let mut entries: Vec<IconEntryRaw> = Vec::with_capacity(images.len());
    for im in images {
        let chosen = choose_sub_format(im, &opts);
        let bytes = encode_sub_image(im, chosen)?;
        entries.push(IconEntryRaw {
            width: im.width,
            height: im.height,
            bit_depth: 32, // we only ever emit 32-bpp today
            sub_format: match chosen {
                SubFormatChosen::Png => IconSubFormat::Png,
                SubFormatChosen::Bmp => IconSubFormat::Bmp,
            },
            hotspot: im.hotspot,
            data: bytes,
        });
    }

    Ok(write_ico_raw(icon_type, &entries)?)
}

/// Alias of [`IconSubFormat`] used at encode time, so we don't confuse
/// the "caller's hint" (which we may override) with "what we actually
/// wrote".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubFormatChosen {
    Png,
    Bmp,
}

fn choose_sub_format(im: &IconImage, opts: &WriteOptions) -> SubFormatChosen {
    match opts.png_size_threshold {
        None => SubFormatChosen::Bmp,
        Some(threshold) => {
            if im.width.min(im.height) >= threshold {
                SubFormatChosen::Png
            } else {
                SubFormatChosen::Bmp
            }
        }
    }
}

fn encode_sub_image(im: &IconImage, fmt: SubFormatChosen) -> Result<Vec<u8>> {
    let frame = iconimage_to_frame(im);
    match fmt {
        SubFormatChosen::Png => {
            oxideav_png::encode_single(&frame, im.width, im.height, PixelFormat::Rgba, &[])
        }
        SubFormatChosen::Bmp => {
            // The BMP-inside-ICO convention is doubled height + AND
            // mask appended; oxideav-bmp handles both via the
            // `double_height_for_ico_mask` flag on the registry-gated
            // VideoFrame-shaped wrapper.
            oxideav_bmp::encode_dib_videoframe(
                &frame,
                PixelFormat::Rgba,
                im.width,
                im.height,
                /* doubled */ true,
            )
        }
    }
}

fn iconimage_to_frame(im: &IconImage) -> VideoFrame {
    VideoFrame {
        pts: None,
        planes: vec![VideoPlane {
            stride: im.width as usize * 4,
            data: im.pixels.clone(),
        }],
    }
}
