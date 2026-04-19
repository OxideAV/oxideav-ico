//! ICO / CUR file parser.
//!
//! Walks the 6-byte `ICONDIR` header, then each 16-byte `ICONDIRENTRY`,
//! and decodes the pointed-at payload (either PNG or BMP-DIB) into an
//! RGBA [`IconImage`].

use oxideav_core::{Error, Result, TimeBase};

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
        let frame = oxideav_png::decoder::decode_png_to_frame(payload, None, TimeBase::new(1, 1))?;
        let rgba = frame_to_rgba_bytes(&frame)?;
        Ok(IconImage {
            width: frame.width,
            height: frame.height,
            pixels: rgba,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot,
        })
    } else {
        // BMP-inside-ICO: headerless DIB with doubled height + AND mask.
        let frame = oxideav_bmp::decode_dib(payload, /* doubled */ true)?;
        // Sanity-check against the directory entry. Mostly harmless if
        // they disagree; we trust the DIB header's dimensions because
        // it's what was actually used to lay out pixels.
        let (w, h) = (frame.width, frame.height);
        if (declared_w != w && declared_w != 256) || (declared_h != h && declared_h != 256) {
            // Not fatal — just note that the ICONDIRENTRY header
            // lied. Real-world icon files do this.
        }
        let rgba = frame_to_rgba_bytes(&frame)?;
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

/// Copy a `VideoFrame` (produced by either oxideav-png or oxideav-bmp,
/// always in `Rgba`) into a tightly-packed top-down RGBA byte Vec.
fn frame_to_rgba_bytes(frame: &oxideav_core::VideoFrame) -> Result<Vec<u8>> {
    use oxideav_core::PixelFormat;
    if frame.format != PixelFormat::Rgba {
        return Err(Error::invalid(format!(
            "ICO: sub-image decoder returned {:?}; expected Rgba",
            frame.format
        )));
    }
    let w = frame.width as usize;
    let h = frame.height as usize;
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
