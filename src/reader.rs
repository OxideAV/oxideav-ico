//! ICO / CUR file parser.
//!
//! Walks the 6-byte `ICONDIR` header, then each 16-byte `ICONDIRENTRY`,
//! and decodes the pointed-at payload (either PNG or BMP-DIB) into an
//! RGBA [`IconImage`].

use oxideav_core::{Error, Result};

use crate::types::*;

const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// Parse an ICO / CUR byte stream. Returns the container type and one
/// [`IconImage`] per directory entry, in directory order.
pub fn read_ico(input: &[u8]) -> Result<(IconType, Vec<IconImage>)> {
    if input.len() < 6 {
        return Err(Error::invalid("ICO: too short for ICONDIR"));
    }
    let reserved = u16::from_le_bytes([input[0], input[1]]);
    let id_type = u16::from_le_bytes([input[2], input[3]]);
    let count = u16::from_le_bytes([input[4], input[5]]) as usize;
    if reserved != 0 {
        return Err(Error::invalid(format!(
            "ICO: ICONDIR.idReserved = {reserved} (must be 0)"
        )));
    }
    let icon_type = match id_type {
        1 => IconType::Ico,
        2 => IconType::Cur,
        other => {
            return Err(Error::invalid(format!(
                "ICO: unknown idType {other} (expected 1=ICO or 2=CUR)"
            )))
        }
    };
    let dir_end = 6usize
        .checked_add(
            count
                .checked_mul(16)
                .ok_or_else(|| Error::invalid("ICO: directory entry count overflows usize"))?,
        )
        .ok_or_else(|| Error::invalid("ICO: directory extends past usize"))?;
    if input.len() < dir_end {
        return Err(Error::invalid("ICO: directory truncated"));
    }

    let mut images = Vec::with_capacity(count);
    for i in 0..count {
        let e = &input[6 + i * 16..6 + i * 16 + 16];
        let declared_width = normalise_dim(e[0]);
        let declared_height = normalise_dim(e[1]);
        // `e[2]` = bColorCount, `e[3]` = bReserved.
        let planes_or_hotx = u16::from_le_bytes([e[4], e[5]]);
        let bits_or_hoty = u16::from_le_bytes([e[6], e[7]]);
        let data_size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let data_offset = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;

        if input.len() < data_offset.saturating_add(data_size) {
            return Err(Error::invalid(format!(
                "ICO: entry {i} payload spans {data_offset}..{} past input",
                data_offset + data_size
            )));
        }
        let payload = &input[data_offset..data_offset + data_size];

        let hotspot = if icon_type == IconType::Cur {
            Some(HotSpot {
                x: planes_or_hotx,
                y: bits_or_hoty,
            })
        } else {
            None
        };

        let image = decode_entry_payload(payload, declared_width, declared_height, hotspot)?;
        images.push(image);
    }

    Ok((icon_type, images))
}

/// The `ICONDIRENTRY` width / height fields are `u8`s; `0` encodes
/// the 256 case (since a literal `256` doesn't fit).
fn normalise_dim(byte: u8) -> u32 {
    if byte == 0 {
        256
    } else {
        byte as u32
    }
}

fn decode_entry_payload(
    payload: &[u8],
    declared_w: u32,
    declared_h: u32,
    hotspot: Option<HotSpot>,
) -> Result<IconImage> {
    let is_png = payload.len() >= PNG_MAGIC.len() && payload[..PNG_MAGIC.len()] == PNG_MAGIC;
    if is_png {
        let frame = oxideav_png::decode_png_to_frame(payload, None)?;
        // PNG path: dimensions come from the embedded IHDR chunk
        // (width/height are at bytes 16..24 of every PNG: 8-byte magic
        // + 8-byte chunk header). The directory entry's declared dims
        // are only a hint and may be 0 (encoded as 256).
        let (w, h) = parse_png_dims(payload).unwrap_or((declared_w, declared_h));
        let rgba = frame_to_rgba_bytes(&frame, w, h)?;
        Ok(IconImage {
            width: w,
            height: h,
            pixels: rgba,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot,
        })
    } else {
        // BMP-inside-ICO: headerless DIB with doubled height + AND mask.
        // The DIB header itself carries the true dimensions; parse them
        // from the header so a lying directory entry doesn't mislead us.
        let frame = oxideav_bmp::decode_dib(payload, /* doubled */ true)?;
        let (w, h) = parse_dib_dims(payload, declared_w, declared_h);
        let rgba = frame_to_rgba_bytes(&frame, w, h)?;
        // Read back the BMP bit-depth from the header so callers can
        // preserve it on roundtrip.
        let bpp = if payload.len() >= 16 {
            u16::from_le_bytes([payload[14], payload[15]]) as u8
        } else {
            32
        };
        Ok(IconImage {
            width: w,
            height: h,
            pixels: rgba,
            bit_depth: bpp,
            sub_format: IconSubFormat::Bmp,
            hotspot,
        })
    }
}

/// Pull (width, height) from a PNG IHDR chunk. PNG layout: 8-byte
/// magic, then a 4-byte length, 4-byte chunk type ("IHDR"), then the
/// IHDR payload starting with two big-endian u32s (width, height).
fn parse_png_dims(payload: &[u8]) -> Option<(u32, u32)> {
    if payload.len() < 24 {
        return None;
    }
    let w = u32::from_be_bytes([payload[16], payload[17], payload[18], payload[19]]);
    let h = u32::from_be_bytes([payload[20], payload[21], payload[22], payload[23]]);
    Some((w, h))
}

/// Pull (width, height) from a headerless DIB (BITMAPINFOHEADER). The
/// height field is doubled (image + AND-mask) for the ICO sub-image
/// convention; we halve it back. Falls back to the directory entry's
/// declared dims when the header is too short to parse.
fn parse_dib_dims(payload: &[u8], declared_w: u32, declared_h: u32) -> (u32, u32) {
    if payload.len() < 12 {
        return (declared_w, declared_h);
    }
    let w = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
    let h_signed = i32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
    let h_abs = h_signed.unsigned_abs();
    // Doubled-height ICO convention: the stored value is 2× the real height.
    (w, h_abs / 2)
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
