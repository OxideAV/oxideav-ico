//! The image-crate contract vocabulary (`IMAGE_CRATE_API`) at the crate
//! root: `probe` / `info` / `decode*` / `encode*`, plus the two
//! entry-level depth functions ([`decode_entry`], [`encode_entry`]) the
//! framework codec adapter and the ANI reader / writer are built on.
//!
//! Everything here is framework-free: it works with
//! `default-features = false` because the sub-image codecs
//! (`oxideav-bmp` for DIB payloads, `oxideav-png` for embedded PNG
//! payloads) are themselves image crates with a standalone layer, and
//! are plain (non-optional) dependencies of this crate.

use std::io::{Read, Write};

use crate::error::{IcoError, Result};
use crate::image::{
    ColorInfo, ColorRange, Frame, IcoEntryInfo, IcoImage, ImageInfo, Metadata, PixelFormat,
    RgbImage, RgbaImage,
};
use crate::options::{DecodeOptions, EncodeOptions, EntrySelection};
use crate::raw::{
    encode_indexed_dib_body, encode_rgb24_dib_body, quantise_rgba_to_indexed, read_ico_raw,
    write_ico_raw, IconEntryRaw,
};
use crate::types::{
    select_best_fit_raw, select_by_dimensions_raw, select_largest_raw, BmpBitDepth, HotSpot,
    IconSubFormat, IconType,
};

const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

// ---------------------------------------------------------------------------
// probe / info
// ---------------------------------------------------------------------------

/// `true` when `bytes` plausibly start an ICO / CUR file.
///
/// ICO has no magic number, so this is a header plausibility test on the
/// 6-byte `ICONDIR` and the first 16-byte `ICONDIRENTRY`: `idReserved`
/// is `0`, `idType` is `1` (ICO) or `2` (CUR), `idCount` is at least 1,
/// the first entry's `bReserved` is `0`, its payload is non-empty and
/// its offset lies past the directory. RIFF / `ACON` animated cursors
/// (`.ani`) are **not** ICO files and probe `false`. Total,
/// allocation-free, `false` on short input.
pub fn probe(bytes: &[u8]) -> bool {
    if bytes.len() < 6 + 16 {
        return false;
    }
    if &bytes[..4] == b"RIFF" {
        return false;
    }
    let reserved = u16::from_le_bytes([bytes[0], bytes[1]]);
    let id_type = u16::from_le_bytes([bytes[2], bytes[3]]);
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as u64;
    if reserved != 0 || !(id_type == 1 || id_type == 2) || count == 0 {
        return false;
    }
    let e = &bytes[6..22];
    let entry_reserved = e[3];
    let data_size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as u64;
    let data_offset = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as u64;
    let dir_end = 6 + count * 16;
    entry_reserved == 0 && data_size != 0 && data_offset >= dir_end
}

/// Header-only description: the directory walk plus the primary
/// (largest, then deepest) entry's geometry and payload-header facts.
/// No pixel is decoded. Fails on a malformed directory exactly like
/// [`decode`] would.
pub fn info(bytes: &[u8]) -> Result<ImageInfo> {
    let (icon_type, entries) = read_ico_raw(bytes)?;
    let primary = select_entry(&entries, EntrySelection::Largest)?;
    let rows: Vec<IcoEntryInfo> = entries
        .iter()
        .map(|e| {
            IcoEntryInfo::new(
                e.width,
                e.height,
                e.bit_depth,
                e.sub_format,
                e.hotspot,
                e.data.len() as u32,
            )
        })
        .collect();
    let p = &entries[primary];
    let mut out = ImageInfo::new(p.width, p.height, PixelFormat::Rgba);
    out.frames = entries.len() as u32;
    out.icon_type = icon_type;
    out.primary = primary as u32;
    out.bit_depth = p.bit_depth;
    out.sub_format = p.sub_format;
    out.hotspot = p.hotspot;
    match p.sub_format {
        IconSubFormat::Png => {
            if let Ok(pi) = oxideav_png::info(&p.data) {
                out.color = color_from_png(&pi.color);
                out.has_icc = pi.has_icc;
                out.has_exif = pi.has_exif;
                out.has_xmp = pi.has_xmp;
            }
        }
        IconSubFormat::Bmp => {
            // Only a V4 / V5 header can carry colour facts; the classic
            // 40-byte header (every real-world icon) has none.
            let bi_size = p
                .data
                .get(..4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .unwrap_or(0);
            if bi_size >= 108 {
                if let Ok(meta) = oxideav_bmp::BmpMetadata::from_dib(&p.data) {
                    if matches!(
                        meta.color_space,
                        Some(oxideav_bmp::BmpColorSpace::SRgb)
                            | Some(oxideav_bmp::BmpColorSpace::Windows)
                    ) {
                        out.color = ColorInfo::srgb();
                    }
                    out.has_icc = meta.icc_profile.is_some();
                }
            }
        }
    }
    out.entries = rows;
    Ok(out)
}

// ---------------------------------------------------------------------------
// decode
// ---------------------------------------------------------------------------

/// Decode the primary sub-image — the largest entry by area, the
/// highest bit depth breaking a tie ([`EntrySelection::Largest`]) —
/// in its native `Rgba` layout with colour and metadata filled from the
/// payload. Equivalent to [`decode_with`] under
/// [`DecodeOptions::default`].
pub fn decode(bytes: &[u8]) -> Result<IcoImage> {
    decode_with(bytes, &DecodeOptions::default())
}

/// [`decode`] with explicit limits, strictness and
/// [entry selection](DecodeOptions::entry).
pub fn decode_with(bytes: &[u8], opts: &DecodeOptions) -> Result<IcoImage> {
    let (_, entries) = read_ico_raw(bytes)?;
    if opts.strict {
        reject_trailing_bytes(bytes)?;
    }
    let idx = select_entry(&entries, opts.entry)?;
    let entry = &entries[idx];
    let bytes_out = u64::from(entry.width) * u64::from(entry.height) * 4;
    opts.check(entry.width, entry.height, bytes_out)?;
    decode_entry(entry, opts)
}

/// Decode the primary sub-image to tightly packed RGB (alpha dropped).
pub fn decode_rgb8(bytes: &[u8]) -> Result<RgbImage> {
    let img = decode(bytes)?;
    Ok(RgbImage::new(img.width, img.height, img.to_rgb8()))
}

/// Decode the primary sub-image to tightly packed RGBA.
pub fn decode_rgba8(bytes: &[u8]) -> Result<RgbaImage> {
    let img = decode(bytes)?;
    Ok(RgbaImage::new(img.width, img.height, img.to_rgba8()))
}

/// Decode every directory entry, in file order, as a [`Frame`] whose
/// `index` is the entry's position. Equivalent to [`decode_all_with`]
/// under [`DecodeOptions::default`].
pub fn decode_all(bytes: &[u8]) -> Result<Vec<Frame>> {
    decode_all_with(bytes, &DecodeOptions::default())
}

/// [`decode_all`] with explicit limits and strictness. The per-image
/// limits apply to each entry; `max_bytes` bounds the sum of every
/// decoded sub-image. `opts.entry` is ignored.
pub fn decode_all_with(bytes: &[u8], opts: &DecodeOptions) -> Result<Vec<Frame>> {
    let (_, entries) = read_ico_raw(bytes)?;
    if opts.strict {
        reject_trailing_bytes(bytes)?;
    }
    // Every limit is checked over the directory before the first
    // payload is decoded, so a hostile 65 535-entry file cannot allocate
    // its way past `max_bytes` one entry at a time.
    let mut total: u64 = 0;
    for e in &entries {
        total = total.saturating_add(u64::from(e.width) * u64::from(e.height) * 4);
        opts.check(e.width, e.height, total)?;
    }
    let mut frames = Vec::with_capacity(entries.len());
    for (i, e) in entries.iter().enumerate() {
        frames.push(Frame::new(decode_entry(e, opts)?, i as u32));
    }
    Ok(frames)
}

/// Read `r` to the end and [`decode`] it.
pub fn decode_from<R: Read>(mut r: R) -> Result<IcoImage> {
    let mut buf = Vec::new();
    r.read_to_end(&mut buf)?;
    decode(&buf)
}

/// Decode one directory entry's payload (an embedded PNG or a
/// doubled-height DIB with its AND mask) into an [`IcoImage`], carrying
/// the entry's bit depth, encoding and hotspot. This is the depth entry
/// point beneath [`decode`] / [`decode_all`]: pick an entry from
/// [`crate::read_ico_raw`] (e.g. with [`crate::select_best_fit_raw`])
/// and decode only that one.
///
/// The directory's width / height (already cross-checked against the
/// payload header by `read_ico_raw`) must match the decoded payload, or
/// the entry is [`IcoError::InvalidData`].
pub fn decode_entry(entry: &IconEntryRaw, opts: &DecodeOptions) -> Result<IcoImage> {
    let mut img = decode_payload(&entry.data, opts)?;
    if img.width != entry.width || img.height != entry.height {
        return Err(IcoError::invalid(format!(
            "ICO: sub-image payload decodes to {}×{} but the directory says {}×{}",
            img.width, img.height, entry.width, entry.height
        )));
    }
    img.bit_depth = entry.bit_depth;
    img.sub_format = entry.sub_format;
    img.hotspot = entry.hotspot;
    Ok(img)
}

/// Decode a bare sub-image payload — the bytes at `dwImageOffset`,
/// sniffed as PNG (8-byte signature) or DIB — into an `Rgba`
/// [`IcoImage`]. The directory extras (`bit_depth`, `hotspot`) are not
/// known here: `bit_depth` is the payload's own depth (PNG: 32), the
/// hotspot `None`. The framework `Decoder` uses this on each demuxed
/// packet.
pub fn decode_payload(payload: &[u8], opts: &DecodeOptions) -> Result<IcoImage> {
    if payload.starts_with(&PNG_MAGIC) {
        let png = oxideav_png::decode_with(payload, &opts.png())?;
        let (w, h) = (png.width(), png.height());
        opts.check(w, h, u64::from(w) * u64::from(h) * 4)?;
        let color = color_from_png(&png.color);
        let metadata = metadata_from_png(&png.metadata);
        let rgba = png.to_rgba8();
        let mut img = IcoImage::packed(w, h, PixelFormat::Rgba, w as usize * 4, rgba)?;
        img.color = color;
        img.metadata = metadata;
        img.bit_depth = 32;
        img.sub_format = IconSubFormat::Png;
        Ok(img)
    } else {
        let bit_depth = payload
            .get(14..16)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .unwrap_or(0);
        let dib = oxideav_bmp::decode_dib_with(payload, /* doubled */ true, &opts.bmp())?;
        let (w, h) = (dib.width(), dib.height());
        opts.check(w, h, u64::from(w) * u64::from(h) * 4)?;
        // `decode_dib` with the mask flag always composes to RGBA (the
        // XOR colour widened, the AND mask folded into alpha).
        let color = color_from_bmp(&dib.color);
        let metadata = metadata_from_bmp(&dib.metadata);
        let rgba = dib.to_rgba8();
        let mut img = IcoImage::packed(w, h, PixelFormat::Rgba, w as usize * 4, rgba)?;
        img.color = color;
        img.metadata = metadata;
        img.bit_depth = u8::try_from(bit_depth).unwrap_or(0);
        img.sub_format = IconSubFormat::Bmp;
        Ok(img)
    }
}

/// Resolve an [`EntrySelection`] over the directory rows.
pub(crate) fn select_entry(entries: &[IconEntryRaw], sel: EntrySelection) -> Result<usize> {
    if entries.is_empty() {
        return Err(IcoError::invalid("ICO: no directory entries"));
    }
    match sel {
        EntrySelection::Largest => {
            select_largest_raw(entries).ok_or_else(|| IcoError::invalid("ICO: no entries"))
        }
        EntrySelection::Index(i) => {
            if (i as usize) < entries.len() {
                Ok(i as usize)
            } else {
                Err(IcoError::invalid(format!(
                    "ICO: entry index {i} out of range (directory has {} entries)",
                    entries.len()
                )))
            }
        }
        EntrySelection::BestFit(target) => {
            select_best_fit_raw(entries, target).ok_or_else(|| IcoError::invalid("ICO: no entries"))
        }
        EntrySelection::Dimensions(w, h) => {
            select_by_dimensions_raw(entries, w, h).ok_or_else(|| {
                IcoError::invalid(format!("ICO: no directory entry is exactly {w}×{h}"))
            })
        }
    }
}

/// Strict mode: the file must end where its last payload ends.
fn reject_trailing_bytes(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 6 {
        return Ok(());
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    let mut end = 6 + count * 16;
    for i in 0..count {
        let e = &bytes[6 + i * 16..6 + i * 16 + 16];
        let size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let off = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
        end = end.max(off.saturating_add(size));
    }
    if bytes.len() > end {
        return Err(IcoError::invalid(format!(
            "ICO: {} trailing bytes after the last payload (strict)",
            bytes.len() - end
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// encode
// ---------------------------------------------------------------------------

/// Write `image` as a single-entry ICO / CUR file ([`EncodeOptions::icon_type`]).
///
/// The payload is an embedded PNG when the image's smaller side is at
/// least [`EncodeOptions::png_size_threshold`], otherwise a DIB at
/// [`EncodeOptions::bmp_bit_depth`] (32-bpp BGRA + alpha-derived AND
/// mask by default — lossless). A dimension outside `1..=256` is
/// [`IcoError::Unsupported`] (the directory stores sizes in one byte);
/// an indexed depth that cannot hold the image's colours is
/// [`IcoError::Unsupported`] too. ICO has only the one native layout
/// (`Rgba`), so nothing is converted silently.
pub fn encode(image: &IcoImage, opts: &EncodeOptions) -> Result<Vec<u8>> {
    encode_images(std::slice::from_ref(image), opts)
}

/// Write every frame's image as one entry of a multi-resolution ICO /
/// CUR, in slice order — the mirror of [`decode_all`]. `Frame::delay`
/// and `Frame::index` are ignored (entries have no timing; the
/// directory order is the slice order).
pub fn encode_all(frames: &[Frame], opts: &EncodeOptions) -> Result<Vec<u8>> {
    let images: Vec<&IcoImage> = frames.iter().map(|f| &f.image).collect();
    encode_image_refs(&images, opts)
}

/// Write several images as one multi-resolution ICO / CUR — the same as
/// [`encode_all`] without wrapping each image in a [`Frame`].
pub fn encode_images(images: &[IcoImage], opts: &EncodeOptions) -> Result<Vec<u8>> {
    let refs: Vec<&IcoImage> = images.iter().collect();
    encode_image_refs(&refs, opts)
}

fn encode_image_refs(images: &[&IcoImage], opts: &EncodeOptions) -> Result<Vec<u8>> {
    if images.is_empty() {
        return Err(IcoError::invalid("ICO: must have at least one sub-image"));
    }
    let mut entries = Vec::with_capacity(images.len());
    for (i, im) in images.iter().enumerate() {
        let entry = encode_entry(im, opts).map_err(|e| match e {
            IcoError::InvalidData(s) => IcoError::InvalidData(format!("ICO: entry {i}: {s}")),
            IcoError::Unsupported(s) => IcoError::Unsupported(format!("ICO: entry {i}: {s}")),
            other => other,
        })?;
        entries.push(entry);
    }
    write_ico_raw(opts.icon_type, &entries)
}

/// Encode tightly packed 8-bit RGB (`3 × width × height` bytes) as a
/// single-entry icon; alpha is opaque.
pub fn encode_rgb8(width: u32, height: u32, rgb: &[u8], opts: &EncodeOptions) -> Result<Vec<u8>> {
    encode(&IcoImage::from_rgb8(width, height, rgb.to_vec())?, opts)
}

/// Encode tightly packed 8-bit RGBA (`4 × width × height` bytes) as a
/// single-entry icon.
pub fn encode_rgba8(width: u32, height: u32, rgba: &[u8], opts: &EncodeOptions) -> Result<Vec<u8>> {
    encode(&IcoImage::from_rgba8(width, height, rgba.to_vec())?, opts)
}

/// [`encode`] straight into a writer.
pub fn encode_to<W: Write>(image: &IcoImage, opts: &EncodeOptions, mut w: W) -> Result<()> {
    let bytes = encode(image, opts)?;
    w.write_all(&bytes)?;
    Ok(())
}

/// Encode one sub-image to its directory entry: the payload bytes
/// (PNG or DIB per the size rule and depth options) plus the directory
/// facts (`width`, `height`, the depth actually written, the encoding,
/// the hotspot). The depth entry point beneath [`encode`] /
/// [`encode_all`]; hand the entries to [`crate::write_ico_raw`] to lay
/// out a file, or use the payload alone (the framework `Encoder` emits
/// it as a packet).
pub fn encode_entry(im: &IcoImage, opts: &EncodeOptions) -> Result<IconEntryRaw> {
    im.check_geometry()?;
    if im.width > 256 || im.height > 256 {
        return Err(IcoError::unsupported(format!(
            "dimensions {}×{} out of 1..=256 (ICONDIRENTRY width/height are single bytes, 0 == 256)",
            im.width, im.height
        )));
    }
    let rgba = im.rgba_cow();
    let (data, sub_format, bit_depth) = if opts.routes_to_png(im.width, im.height) {
        (encode_png_payload(im, &rgba, opts)?, IconSubFormat::Png, 32)
    } else {
        let depth = if opts.per_image_bit_depth {
            BmpBitDepth::from_bits(im.bit_depth).unwrap_or(opts.bmp_bit_depth)
        } else {
            opts.bmp_bit_depth
        };
        (
            encode_dib_payload(im.width, im.height, &rgba, depth)?,
            IconSubFormat::Bmp,
            depth.bits(),
        )
    };
    Ok(IconEntryRaw {
        width: im.width,
        height: im.height,
        bit_depth,
        sub_format,
        hotspot: if opts.icon_type == IconType::Cur {
            Some(im.hotspot.unwrap_or(HotSpot { x: 0, y: 0 }))
        } else {
            im.hotspot
        },
        data,
    })
}

fn encode_png_payload(im: &IcoImage, rgba: &[u8], opts: &EncodeOptions) -> Result<Vec<u8>> {
    let mut png = oxideav_png::PngImage::new(
        im.width,
        im.height,
        oxideav_png::PixelFormat::Rgba,
        vec![oxideav_png::Plane::new(
            im.width as usize * 4,
            rgba.to_vec(),
        )],
    );
    if im.color.is_specified() {
        png = png.with_color(color_to_png(&im.color));
    }
    if opts.embed_metadata && !im.metadata.is_empty() {
        png = png.with_metadata(
            oxideav_png::Metadata::new()
                .with_icc(im.metadata.icc.clone())
                .with_exif(im.metadata.exif.clone())
                .with_xmp(im.metadata.xmp.clone())
                .with_gamma(im.metadata.gamma),
        );
    }
    Ok(oxideav_png::encode(
        &png,
        &oxideav_png::EncodeOptions::default(),
    )?)
}

/// Encode one DIB sub-image body at the requested bit depth. The 32-bpp
/// path delegates to `oxideav-bmp` (BGRA + alpha-derived AND mask); the
/// indexed / 24-bpp paths use the in-crate `raw` encoders that build the
/// palette + XOR rows + AND mask.
fn encode_dib_payload(width: u32, height: u32, rgba: &[u8], depth: BmpBitDepth) -> Result<Vec<u8>> {
    match depth {
        BmpBitDepth::Bgra32 => {
            let bmp = oxideav_bmp::BmpImage::new(
                width,
                height,
                oxideav_bmp::PixelFormat::Rgba,
                vec![oxideav_bmp::Plane::new(width as usize * 4, rgba.to_vec())],
            )?;
            Ok(oxideav_bmp::encode_dib(&bmp, /* doubled */ true)?)
        }
        BmpBitDepth::Rgb24 => {
            let pixels = width as usize * height as usize;
            let mut rgb = Vec::with_capacity(pixels * 3);
            let mut transparent = Vec::with_capacity(pixels);
            for px in rgba.chunks_exact(4) {
                rgb.extend_from_slice(&px[..3]);
                transparent.push(px[3] == 0);
            }
            encode_rgb24_dib_body(width, height, &rgb, &transparent)
        }
        BmpBitDepth::Indexed8 | BmpBitDepth::Indexed4 | BmpBitDepth::Indexed1 => {
            let bpp = depth
                .indexed_bpp()
                .expect("indexed variant has an indexed bpp");
            // Too many colours for the depth is `Unsupported` (a
            // representability limit of the requested layout).
            let (palette, indices, transparent) =
                quantise_rgba_to_indexed(width, height, rgba, bpp)?;
            encode_indexed_dib_body(width, height, bpp, &palette, &indices, &transparent)
        }
    }
}

// ---------------------------------------------------------------------------
// Sibling-crate record mapping (identical shapes, distinct types)
// ---------------------------------------------------------------------------

pub(crate) fn color_from_png(c: &oxideav_png::ColorInfo) -> ColorInfo {
    let range = match c.range {
        oxideav_png::ColorRange::Limited => ColorRange::Limited,
        oxideav_png::ColorRange::Full => ColorRange::Full,
        _ => ColorRange::Unspecified,
    };
    ColorInfo::new(range, c.primaries, c.transfer, c.matrix)
}

pub(crate) fn color_to_png(c: &ColorInfo) -> oxideav_png::ColorInfo {
    let range = match c.range {
        ColorRange::Limited => oxideav_png::ColorRange::Limited,
        ColorRange::Full => oxideav_png::ColorRange::Full,
        ColorRange::Unspecified => oxideav_png::ColorRange::Unspecified,
    };
    oxideav_png::ColorInfo::new(range, c.primaries, c.transfer, c.matrix)
}

pub(crate) fn color_from_bmp(c: &oxideav_bmp::ColorInfo) -> ColorInfo {
    let range = match c.range {
        oxideav_bmp::ColorRange::Limited => ColorRange::Limited,
        oxideav_bmp::ColorRange::Full => ColorRange::Full,
        _ => ColorRange::Unspecified,
    };
    ColorInfo::new(range, c.primaries, c.transfer, c.matrix)
}

pub(crate) fn metadata_from_png(m: &oxideav_png::Metadata) -> Metadata {
    Metadata::new()
        .with_icc(m.icc.clone())
        .with_exif(m.exif.clone())
        .with_xmp(m.xmp.clone())
        .with_gamma(m.gamma)
}

pub(crate) fn metadata_from_bmp(m: &oxideav_bmp::Metadata) -> Metadata {
    Metadata::new()
        .with_icc(m.icc.clone())
        .with_exif(m.exif.clone())
        .with_xmp(m.xmp.clone())
        .with_gamma(m.gamma)
}
