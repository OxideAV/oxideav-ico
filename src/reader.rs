//! ICO / CUR file parser (registry-side, oxideav-core-using path).
//!
//! Walks the 6-byte `ICONDIR` header, then each 16-byte `ICONDIRENTRY`,
//! and decodes the pointed-at payload (either PNG or BMP-DIB) into an
//! RGBA [`IconImage`]. The lower-level container layout walk lives in
//! [`crate::raw::read_ico_raw`] which returns raw payload bytes;
//! this layer adds the PNG / BMP-DIB decode on top.

use oxideav_core::{Error, Result};

use crate::raw::{read_ico_raw, IconEntryRaw};
use crate::types::*;

/// Parse an ICO / CUR byte stream. Returns the container type and one
/// [`IconImage`] per directory entry, in directory order.
pub fn read_ico(input: &[u8]) -> Result<(IconType, Vec<IconImage>)> {
    let (icon_type, entries) = read_ico_raw(input)?;
    let mut images = Vec::with_capacity(entries.len());
    for entry in entries {
        images.push(decode_entry(entry)?);
    }
    Ok((icon_type, images))
}

fn decode_entry(entry: IconEntryRaw) -> Result<IconImage> {
    match entry.sub_format {
        IconSubFormat::Png => {
            let frame = oxideav_png::decode_png_to_frame(&entry.data, None)?;
            let rgba = frame_to_rgba_bytes(&frame, entry.width, entry.height)?;
            Ok(IconImage {
                width: entry.width,
                height: entry.height,
                pixels: rgba,
                bit_depth: entry.bit_depth,
                sub_format: IconSubFormat::Png,
                hotspot: entry.hotspot,
            })
        }
        IconSubFormat::Bmp => {
            // BMP-inside-ICO: headerless DIB with doubled height + AND
            // mask. `decode_dib_videoframe` is the registry-gated
            // VideoFrame-shaped wrapper around `oxideav_bmp::decode_dib`
            // (which now returns the standalone `BmpImage` shape).
            let frame = oxideav_bmp::decode_dib_videoframe(&entry.data, /* doubled */ true)?;
            let rgba = frame_to_rgba_bytes(&frame, entry.width, entry.height)?;
            Ok(IconImage {
                width: entry.width,
                height: entry.height,
                pixels: rgba,
                bit_depth: entry.bit_depth,
                sub_format: IconSubFormat::Bmp,
                hotspot: entry.hotspot,
            })
        }
    }
}

/// Copy a `VideoFrame` (produced by either oxideav-png or oxideav-bmp,
/// always in `Rgba`) into a tightly-packed top-down RGBA byte Vec.
fn frame_to_rgba_bytes(frame: &oxideav_core::VideoFrame, w: u32, h: u32) -> Result<Vec<u8>> {
    let w = w as usize;
    let h = h as usize;
    if frame.planes.is_empty() {
        return Err(Error::invalid("ICO: sub-image frame has no planes"));
    }
    let src_stride = frame.planes[0].stride;
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let src = &frame.planes[0].data[y * src_stride..y * src_stride + w * 4];
        out.extend_from_slice(src);
    }
    Ok(out)
}
