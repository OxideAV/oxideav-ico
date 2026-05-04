//! Standalone (framework-free) ICO / CUR container parser.
//!
//! Produces / consumes [`IconEntryRaw`] entries — directory rows + the
//! raw sub-image payload bytes (PNG or BMP-DIB) exactly as they appear
//! in the file. The actual sub-image decode/encode is the caller's
//! job; pull in `oxideav-png` and `oxideav-bmp` (or any other PNG /
//! BMP-DIB implementation) to materialise pixels.
//!
//! The registry-side [`crate::reader::read_ico`] /
//! [`crate::writer::write_ico`] helpers wrap this with the
//! [`crate::types::IconImage`] (decoded RGBA) shape and pull
//! `oxideav-bmp` / `oxideav-png` in for the actual decoding /
//! encoding. They live behind the default-on `registry` feature.

use crate::error::{IcoError as Error, Result};
use crate::types::{HotSpot, IconSubFormat, IconType};

const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// One ICO / CUR sub-image entry as stored on disk: directory metadata
/// (width, height, optional hotspot, source encoding) plus the raw
/// payload bytes.
///
/// Width/height are read from the on-disk `bWidth`/`bHeight` fields
/// (with the `0 → 256` convention applied). For PNG entries, the
/// payload is exactly what's between the `data_offset` and
/// `data_offset + data_size` for that directory entry — i.e. a
/// complete PNG file (`89 50 4E 47 …`). For BMP entries, the payload
/// is a headerless DIB with the doubled-height + 1-bpp AND mask
/// layout per the ICO convention.
#[derive(Debug, Clone)]
pub struct IconEntryRaw {
    /// Width as read from the directory entry (0 normalised to 256).
    pub width: u32,
    /// Height as read from the directory entry (0 normalised to 256).
    pub height: u32,
    /// Bits-per-pixel as the directory advertised it. For PNG entries
    /// this is what the `wBitCount` field claims (32 in practice);
    /// for BMP entries it matches the DIB header's `biBitCount`.
    pub bit_depth: u8,
    /// What the entry encodes: PNG file or headerless BMP DIB.
    pub sub_format: IconSubFormat,
    /// Hotspot — populated when `IconType::Cur`, ignored otherwise.
    pub hotspot: Option<HotSpot>,
    /// Raw sub-image bytes — PNG file or BMP DIB, exactly as they
    /// appear in the source file.
    pub data: Vec<u8>,
}

/// Parse the directory of an ICO / CUR file and return one
/// [`IconEntryRaw`] per directory entry. The returned `data` slices
/// are owned `Vec<u8>` copies of the on-disk bytes; the caller is
/// free to drop the source buffer.
///
/// This parser does **not** decode the sub-image payloads — it just
/// validates the directory structure (offsets in range, magic ok,
/// `idType` either ICO or CUR). Pull in `oxideav-png` /
/// `oxideav-bmp` (or use the registry-side [`crate::read_ico`]
/// wrapper) to turn each entry's bytes into pixels.
pub fn read_ico_raw(input: &[u8]) -> Result<(IconType, Vec<IconEntryRaw>)> {
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

    let mut entries = Vec::with_capacity(count);
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
        let payload = input[data_offset..data_offset + data_size].to_vec();

        let hotspot = if icon_type == IconType::Cur {
            Some(HotSpot {
                x: planes_or_hotx,
                y: bits_or_hoty,
            })
        } else {
            None
        };

        let sub_format = sniff_sub_format(&payload);
        let (width, height) = match sub_format {
            IconSubFormat::Png => {
                parse_png_dims(&payload).unwrap_or((declared_width, declared_height))
            }
            IconSubFormat::Bmp => parse_dib_dims(&payload, declared_width, declared_height),
        };
        let bit_depth = sniff_bpp(&payload);

        entries.push(IconEntryRaw {
            width,
            height,
            bit_depth,
            sub_format,
            hotspot,
            data: payload,
        });
    }

    Ok((icon_type, entries))
}

/// Build an ICO / CUR file from a batch of pre-encoded sub-image
/// payloads. The caller is responsible for producing valid PNG /
/// BMP-DIB bytes for each entry's `data`. Width / height / hotspot /
/// bit_depth are written into the directory verbatim.
///
/// Single fits-in-`u8` directory check: `1..=256` for width and
/// height; the directory format physically can't carry larger.
pub fn write_ico_raw(icon_type: IconType, entries: &[IconEntryRaw]) -> Result<Vec<u8>> {
    if entries.is_empty() {
        return Err(Error::invalid("ICO: must have at least one sub-image"));
    }
    if entries.len() > u16::MAX as usize {
        return Err(Error::invalid("ICO: too many sub-images (> 65535)"));
    }
    for (i, e) in entries.iter().enumerate() {
        if e.width == 0 || e.height == 0 || e.width > 256 || e.height > 256 {
            return Err(Error::invalid(format!(
                "ICO: entry {i} dimensions {}×{} out of 1..=256",
                e.width, e.height
            )));
        }
    }

    let count = entries.len();
    let dir_size = 6 + 16 * count;
    let mut total = dir_size;
    let mut offsets = Vec::with_capacity(count);
    for e in entries {
        offsets.push(total as u32);
        total += e.data.len();
    }
    let mut out = Vec::with_capacity(total);

    // ICONDIR.
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    let id_type: u16 = match icon_type {
        IconType::Ico => 1,
        IconType::Cur => 2,
    };
    out.extend_from_slice(&id_type.to_le_bytes());
    out.extend_from_slice(&(count as u16).to_le_bytes());

    for (i, e) in entries.iter().enumerate() {
        let w_byte = if e.width == 256 { 0 } else { e.width as u8 };
        let h_byte = if e.height == 256 { 0 } else { e.height as u8 };
        out.push(w_byte);
        out.push(h_byte);
        // `bColorCount` — 0 for ≥ 8 bpp, which is always our case today.
        out.push(0);
        // `bReserved` — must be zero per the format.
        out.push(0);
        let (planes, bits) = match (icon_type, e.hotspot) {
            (IconType::Cur, Some(h)) => (h.x, h.y),
            (IconType::Cur, None) => (0, 0),
            (IconType::Ico, _) => (1, e.bit_depth as u16),
        };
        out.extend_from_slice(&planes.to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(&(e.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offsets[i].to_le_bytes());
    }

    for e in entries {
        out.extend_from_slice(&e.data);
    }

    Ok(out)
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

/// Recognise the sub-image encoding by sniffing the first bytes of
/// the payload.
fn sniff_sub_format(payload: &[u8]) -> IconSubFormat {
    if payload.len() >= PNG_MAGIC.len() && payload[..PNG_MAGIC.len()] == PNG_MAGIC {
        IconSubFormat::Png
    } else {
        IconSubFormat::Bmp
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

/// Peek at a sub-image body and guess its bits-per-pixel. PNG is
/// treated as 32 bpp RGBA; BMP reads from the header's `biBitCount`.
fn sniff_bpp(body: &[u8]) -> u8 {
    if body.len() >= 8 && body[..8] == PNG_MAGIC {
        32
    } else if body.len() >= 16 {
        // BITMAPINFOHEADER: biBitCount at offset 14 (size + w + h +
        // planes = 4 + 4 + 4 + 2 bytes).
        u16::from_le_bytes([body[14], body[15]]) as u8
    } else {
        32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_rejects_short_input() {
        assert!(read_ico_raw(&[]).is_err());
        assert!(read_ico_raw(&[0; 5]).is_err());
    }

    #[test]
    fn read_rejects_bad_id_type() {
        // reserved=0, idType=42, count=0
        let bytes = [0, 0, 42, 0, 0, 0];
        assert!(read_ico_raw(&bytes).is_err());
    }

    #[test]
    fn write_then_read_raw_roundtrip_zero_payload() {
        // 1×1 fake-PNG payload (8 magic bytes — too short to parse but
        // the parser still tolerates "raw payload bytes here").
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            data: PNG_MAGIC.to_vec(),
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let (ty, got) = read_ico_raw(&bytes).unwrap();
        assert_eq!(ty, IconType::Ico);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].width, 16);
        assert_eq!(got[0].height, 16);
        assert_eq!(got[0].sub_format, IconSubFormat::Png);
        assert_eq!(got[0].data, entry.data);
    }
}
