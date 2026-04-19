//! ICO / CUR file encoder.
//!
//! Picks PNG or BMP per sub-image according to [`WriteOptions`] (default:
//! PNG for sizes ≥ 64, BMP otherwise), lays out the `ICONDIR` header,
//! the contiguous `ICONDIRENTRY` table, and the packed payloads.

use oxideav_core::{Error, PixelFormat, Result, TimeBase, VideoFrame, VideoPlane};

use crate::types::*;

/// Serialize a batch of images into a single `.ico` / `.cur` byte
/// stream. The caller is responsible for ensuring every image fits
/// within 1 ≤ dim ≤ 256 (the format's hard limit — stored as `u8` in
/// the directory entry, with 0 meaning 256).
pub fn write_ico(icon_type: IconType, images: &[IconImage], opts: WriteOptions) -> Result<Vec<u8>> {
    if images.is_empty() {
        return Err(Error::invalid("ICO: must have at least one sub-image"));
    }
    if images.len() > u16::MAX as usize {
        return Err(Error::invalid("ICO: too many sub-images (> 65535)"));
    }
    for (i, im) in images.iter().enumerate() {
        if im.width == 0 || im.height == 0 || im.width > 256 || im.height > 256 {
            return Err(Error::invalid(format!(
                "ICO: entry {i} dimensions {}×{} out of 1..=256",
                im.width, im.height
            )));
        }
        if im.pixels.len() != (im.width as usize * im.height as usize * 4) {
            return Err(Error::invalid(format!(
                "ICO: entry {i} pixel buffer size {} != {}×{}×4",
                im.pixels.len(),
                im.width,
                im.height
            )));
        }
    }

    // 1. Encode each sub-image to its packed payload.
    let mut payloads: Vec<(SubFormatChosen, Vec<u8>)> = Vec::with_capacity(images.len());
    for im in images {
        let chosen = choose_sub_format(im, &opts);
        let bytes = encode_sub_image(im, chosen)?;
        payloads.push((chosen, bytes));
    }

    // 2. Lay out: ICONDIR (6 B) + N × ICONDIRENTRY (16 B) + payloads.
    let dir_size = 6 + 16 * payloads.len();
    let mut total = dir_size;
    let mut offsets = Vec::with_capacity(payloads.len());
    for (_, body) in &payloads {
        offsets.push(total as u32);
        total += body.len();
    }
    let mut out = Vec::with_capacity(total);

    // ICONDIR.
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    let id_type: u16 = match icon_type {
        IconType::Ico => 1,
        IconType::Cur => 2,
    };
    out.extend_from_slice(&id_type.to_le_bytes());
    out.extend_from_slice(&(payloads.len() as u16).to_le_bytes());

    // ICONDIRENTRY × N. `planes` / `bit_count` are overridden with
    // the CUR hotspot when applicable.
    for (i, im) in images.iter().enumerate() {
        let (chosen, body) = &payloads[i];
        let w_byte = if im.width == 256 { 0 } else { im.width as u8 };
        let h_byte = if im.height == 256 { 0 } else { im.height as u8 };
        out.push(w_byte);
        out.push(h_byte);
        // `bColorCount` — 0 for ≥ 8 bpp or when no palette is used,
        // which is always our case today.
        out.push(0);
        // `bReserved` — must be zero per the format.
        out.push(0);
        let (planes, bits) = match (icon_type, im.hotspot) {
            (IconType::Cur, Some(h)) => (h.x, h.y),
            (IconType::Cur, None) => (0, 0),
            (IconType::Ico, _) => (1, bits_per_pixel_for(*chosen)),
        };
        out.extend_from_slice(&planes.to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&offsets[i].to_le_bytes());
    }

    // Payloads in the same order as the directory entries.
    for (_, body) in &payloads {
        out.extend_from_slice(body);
    }

    Ok(out)
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

fn bits_per_pixel_for(fmt: SubFormatChosen) -> u16 {
    match fmt {
        // We always write 32bpp for both encodings.
        SubFormatChosen::Png | SubFormatChosen::Bmp => 32,
    }
}

fn encode_sub_image(im: &IconImage, fmt: SubFormatChosen) -> Result<Vec<u8>> {
    let frame = iconimage_to_frame(im);
    match fmt {
        SubFormatChosen::Png => oxideav_png::encoder::encode_single(&frame, PixelFormat::Rgba, &[]),
        SubFormatChosen::Bmp => {
            // The BMP-inside-ICO convention is doubled height + AND
            // mask appended; oxideav-bmp handles both via the
            // `double_height_for_ico_mask` flag.
            oxideav_bmp::encode_dib(&frame, /* doubled */ true)
        }
    }
}

fn iconimage_to_frame(im: &IconImage) -> VideoFrame {
    VideoFrame {
        format: PixelFormat::Rgba,
        width: im.width,
        height: im.height,
        pts: None,
        time_base: TimeBase::new(1, 1),
        planes: vec![VideoPlane {
            stride: im.width as usize * 4,
            data: im.pixels.clone(),
        }],
    }
}
