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

/// `RIFF` magic — every `.ani` animated-cursor file starts with this,
/// followed by a 4-byte size and the `ACON` form type. Recognised
/// up-front so callers get a clear "this is a different container"
/// error instead of a generic "bad idType" once we'd read past the
/// `RIFF`-as-little-endian-u16 garbage.
const RIFF_MAGIC: &[u8; 4] = b"RIFF";
/// `.ani` (animated cursor) form type, found at byte offset 8 of a
/// valid RIFF file. Together with [`RIFF_MAGIC`] this uniquely tags
/// an animated cursor.
const ACON_FORM: &[u8; 4] = b"ACON";

/// Legal `ICONDIRENTRY.wBitCount` values for ICO entries. `0` means
/// "unspecified — look at the DIB header", which is what most
/// PNG-payload entries store. The other values mirror what
/// `BITMAPINFOHEADER` itself accepts.
const VALID_ICO_BIT_DEPTHS: [u16; 7] = [0, 1, 4, 8, 16, 24, 32];

/// `BITMAPINFOHEADER.biCompression = BI_RGB` — uncompressed RGB. The
/// spec mandates this for ICO sub-images; the BMP body's colour bits
/// follow the header verbatim with no per-row state.
const BI_RGB: u32 = 0;
/// `BITMAPINFOHEADER.biCompression = BI_BITFIELDS` — uncompressed RGB
/// with explicit per-channel masks. Valid for 16-bpp and 32-bpp DIBs
/// per the BITMAPINFOHEADER reference; tolerated for ICO sub-images
/// since the wider Windows ecosystem occasionally produces it (a
/// 32-bpp ARGB layout declared via masks rather than the implied
/// 8-8-8-8 packing) and the rest of the body still parses with the
/// same row-stride rules.
const BI_BITFIELDS: u32 = 3;

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
    // Animated cursors share the `.ani` extension's user expectation
    // but are a completely different RIFF-based container. Recognise
    // the RIFF/ACON tag pair before we mistakenly read "RIFF" as the
    // ICONDIR header (which would produce a misleading "idType 0x4952"
    // error). 12 bytes covers `RIFF`+u32 size+`ACON`.
    if input.len() >= 12 && &input[..4] == RIFF_MAGIC && &input[8..12] == ACON_FORM {
        return Err(Error::unsupported(
            "ICO: input is a .ani animated cursor (RIFF/ACON); \
             use oxideav_ico::read_ani_raw for animated cursors",
        ));
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
    if count == 0 {
        return Err(Error::invalid(
            "ICO: ICONDIR.idCount = 0 (need at least one sub-image)",
        ));
    }
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
    // Track each entry's [data_offset, data_offset+data_size) range
    // so we can flag overlapping sub-image payloads (see the
    // cross-entry check inside the loop below).
    let mut payload_ranges: Vec<(usize, usize)> = Vec::with_capacity(count);
    for i in 0..count {
        let e = &input[6 + i * 16..6 + i * 16 + 16];
        let declared_width = normalise_dim(e[0]);
        let declared_height = normalise_dim(e[1]);
        let color_count = e[2];
        let reserved_byte = e[3];
        let planes_or_hotx = u16::from_le_bytes([e[4], e[5]]);
        let bits_or_hoty = u16::from_le_bytes([e[6], e[7]]);
        let data_size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let data_offset = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;

        if reserved_byte != 0 {
            return Err(Error::invalid(format!(
                "ICO: entry {i} bReserved = {reserved_byte} (must be 0)"
            )));
        }
        if data_size == 0 {
            return Err(Error::invalid(format!(
                "ICO: entry {i} dwBytesInRes = 0 (empty payload)"
            )));
        }
        if data_offset < dir_end {
            return Err(Error::invalid(format!(
                "ICO: entry {i} dwImageOffset {data_offset} overlaps directory (ends at {dir_end})"
            )));
        }
        // `data_offset + data_size` overflow + EOF check in one go.
        let data_end = data_offset
            .checked_add(data_size)
            .ok_or_else(|| Error::invalid(format!("ICO: entry {i} payload extent overflows")))?;
        if data_end > input.len() {
            return Err(Error::invalid(format!(
                "ICO: entry {i} payload spans {data_offset}..{data_end} past input ({} bytes)",
                input.len()
            )));
        }
        // Cross-entry payload overlap: every legit ICO/CUR writer
        // assigns each sub-image a private byte range. Overlapping
        // ranges have been used by attackers to smuggle two different
        // PNG/BMP bodies through the same offset window (so e.g. the
        // probe sees a benign image but the renderer parses a malicious
        // one). We reject the entire file rather than try to guess
        // which interpretation the producer intended.
        for (j, prev) in payload_ranges.iter().enumerate() {
            let &(prev_start, prev_end) = prev;
            if data_offset < prev_end && prev_start < data_end {
                return Err(Error::invalid(format!(
                    "ICO: entry {i} payload {data_offset}..{data_end} overlaps \
                     entry {j} payload {prev_start}..{prev_end}"
                )));
            }
        }
        payload_ranges.push((data_offset, data_end));
        let payload = input[data_offset..data_end].to_vec();

        let hotspot = if icon_type == IconType::Cur {
            // Per the CUR convention the hotspot must lie within the
            // sub-image. Real cursors regularly write `(0, 0)` so
            // accept that even when the dimensions are also 0 (e.g.
            // unparseable BMP body — the caller will fail later).
            //
            // Note: we re-check the hotspot against the body-derived
            // dimensions below, after the BMP/PNG header has been
            // parsed. This first pass uses the directory-declared
            // dimensions so a clearly-broken hotspot (e.g. > 256) is
            // rejected before we even sniff the body.
            if (planes_or_hotx as u32) >= declared_width.max(1)
                || (bits_or_hoty as u32) >= declared_height.max(1)
            {
                return Err(Error::invalid(format!(
                    "CUR: entry {i} hotspot ({planes_or_hotx},{bits_or_hoty}) \
                     outside directory sub-image {declared_width}×{declared_height}"
                )));
            }
            Some(HotSpot {
                x: planes_or_hotx,
                y: bits_or_hoty,
            })
        } else {
            // ICO path: `wPlanes` is legally 0 ("unspecified") or 1.
            // Real-world writers always emit 1; defenders against
            // garbage inputs should reject anything else.
            if planes_or_hotx > 1 {
                return Err(Error::invalid(format!(
                    "ICO: entry {i} wPlanes = {planes_or_hotx} (must be 0 or 1)"
                )));
            }
            if !VALID_ICO_BIT_DEPTHS.contains(&bits_or_hoty) {
                return Err(Error::invalid(format!(
                    "ICO: entry {i} wBitCount = {bits_or_hoty} \
                     (must be one of 0/1/4/8/16/24/32)"
                )));
            }
            None
        };

        let sub_format = sniff_sub_format(&payload);
        let (width, height) = match sub_format {
            IconSubFormat::Png => {
                parse_png_dims(&payload).unwrap_or((declared_width, declared_height))
            }
            IconSubFormat::Bmp => parse_dib_dims(&payload, declared_width, declared_height),
        };
        // The ICO/CUR directory width/height are single bytes (with the
        // `0 == 256` convention) so a legal entry's true dimensions
        // always fall inside `1..=256`. A body whose IHDR / DIB header
        // claims something outside that range is either corrupt or a
        // probe-vs-render mismatch attack (the directory says one size
        // but the body decodes to another); reject the file rather
        // than emit a sub-image the directory physically can't
        // describe. Same fuzz-harness invariant the lower-level
        // `(0, 256]` assertion checks — promoted here so the parser
        // refuses the input cleanly instead of producing a value the
        // harness then panics on.
        if width == 0 || width > 256 {
            return Err(Error::invalid(format!(
                "ICO: entry {i} sub-image header claims width {width} (must be 1..=256)"
            )));
        }
        if height == 0 || height > 256 {
            return Err(Error::invalid(format!(
                "ICO: entry {i} sub-image header claims height {height} (must be 1..=256)"
            )));
        }
        // Directory-vs-body dimension consistency.
        //
        // The ICONDIRENTRY's `bWidth` / `bHeight` are `u8` fields with
        // the `0 == 256` convention. When the raw byte is non-zero,
        // it's an exact assertion of the sub-image dimension; the
        // body's PNG IHDR or BMP `biWidth` / halved-`biHeight` MUST
        // agree. A body that disagrees is the same probe-vs-render
        // shape the body-dim range check (entry size > 256), the
        // CUR hotspot body-derived check, and the BMP `biBitCount`
        // body check already close for adjacent fields: the
        // directory walker advertises one value, the renderer
        // decodes a different one. Reject the file rather than
        // emit an `IconEntryRaw` whose `width` / `height` silently
        // contradict the directory the caller just inspected.
        //
        // The `0 == 256` carve-out: when the raw `bWidth` byte is
        // `0`, the directory physically cannot encode a literal
        // dimension other than 256, so the body's value is taken
        // as authoritative (any value already inside the validated
        // `1..=256` range is accepted). Same for `bHeight`.
        let raw_dir_width = e[0];
        let raw_dir_height = e[1];
        if raw_dir_width != 0 && width != raw_dir_width as u32 {
            return Err(Error::invalid(format!(
                "ICO: entry {i} directory width {raw_dir_width} disagrees \
                 with body sub-image width {width} (probe-vs-render mismatch)"
            )));
        }
        if raw_dir_height != 0 && height != raw_dir_height as u32 {
            return Err(Error::invalid(format!(
                "ICO: entry {i} directory height {raw_dir_height} disagrees \
                 with body sub-image height {height} (probe-vs-render mismatch)"
            )));
        }
        // CUR hotspot probe-vs-render check: the directory's u8 fields
        // can encode 256×256 by convention (the `0 == 256` rule), but
        // the body's BMP / PNG header may describe a much smaller
        // sub-image — at which point a hotspot legal against the
        // directory dimensions (e.g. (0, 128) on a 256×256 declared
        // canvas) is *outside* the actual sub-image (e.g. 2×33 BMP).
        // That's the same probe-vs-render shape r178 caught for the
        // body-dim range; here we re-validate the hotspot against the
        // dims the body actually decodes to. Caught by fuzz crash
        // `10593ac8…` 2026-05-29: directory said 256×256, body was a
        // BMP with biWidth=2 + biHeight=66 (doubled → 33), and the
        // hotspot (0, 128) silently slipped through the first-pass
        // check.
        if let Some(ref h) = hotspot {
            if (h.x as u32) >= width || (h.y as u32) >= height {
                return Err(Error::invalid(format!(
                    "CUR: entry {i} hotspot ({},{}) outside body sub-image {width}×{height} \
                     (probe-vs-render: directory declared {declared_width}×{declared_height})",
                    h.x, h.y
                )));
            }
        }
        let bit_depth = sniff_bpp(&payload);

        // Body bit-depth must be one of the legal `wBitCount` values
        // (0/1/4/8/16/24/32). The directory's `wBitCount` field is
        // already validated above, but the BMP body's `biBitCount`
        // can carry arbitrary values that the writer would otherwise
        // dutifully fold back into a fresh directory — producing a
        // file that fails its *own* `wBitCount` re-read check, i.e.
        // breaking the parser/writer fixpoint. Reject any body whose
        // `biBitCount` falls outside the legal set, with the same
        // wording as the directory-side check so a triage grep maps
        // both reports to the same root cause. Caught by
        // `ico_raw_parser` fuzz crash `591dc2ca…` (BMP body with
        // `biBitCount = 72`).
        if !VALID_ICO_BIT_DEPTHS.contains(&(bit_depth as u16)) {
            return Err(Error::invalid(format!(
                "ICO: entry {i} body biBitCount = {bit_depth} \
                 (must be one of 0/1/4/8/16/24/32)"
            )));
        }

        // BMP body `biPlanes` consistency. The ICO/CUR spec mandates
        // `biPlanes = 1` in the BITMAPINFOHEADER (a single colour plane
        // — multi-plane DIBs were a planar-video relic that ICO never
        // used). The directory's `wPlanes` is already validated against
        // {0, 1} above, but the BMP body's `biPlanes` is independently
        // present and previously trusted verbatim — same probe-vs-render
        // shape as the r198 `biBitCount` fix and r210 `bWidth`/`bHeight`
        // mismatch fix: a probe inspecting the directory sees one value,
        // the renderer parses the body and sees another. A body claiming
        // e.g. `biPlanes = 7` is a malformed DIB the writer would
        // otherwise round-trip into a directory whose `wPlanes` then
        // fails the very check above (broken parser/writer fixpoint).
        // Reject body bodies whose `biPlanes` falls outside {0, 1} up
        // front with the same wording as the directory-side check so a
        // triage grep maps both reports to the same root cause.
        //
        // The legal carve-out mirrors the directory side: `0` is
        // accepted ("unspecified — defer to a different field") for
        // tolerance with the wider ecosystem; `1` is the canonical
        // value. PNG entries don't have a `biPlanes` to validate.
        if sub_format == IconSubFormat::Bmp {
            if let Some(planes) = parse_dib_planes(&payload) {
                if planes > 1 {
                    return Err(Error::invalid(format!(
                        "ICO: entry {i} body biPlanes = {planes} \
                         (must be 0 or 1)"
                    )));
                }
            }
        }

        // BMP body `biCompression` consistency. The ICO spec narrows
        // the wider DIB `biCompression` field: an ICO sub-image's DIB
        // header carries `BI_RGB = 0` (uncompressed). `BI_BITFIELDS =
        // 3` is the only practical alternative — it lets 16-bpp and
        // 32-bpp DIBs declare per-channel masks instead of the implied
        // 5-5-5 / 8-8-8-8 layout — and the wider Windows imaging
        // ecosystem produces it in the wild. Every other value
        // (`BI_RLE8 = 1`, `BI_RLE4 = 2`, `BI_JPEG = 4`, `BI_PNG = 5`,
        // `BI_ALPHABITFIELDS = 6`, opaque FOURCC video codes) is
        // explicitly excluded by the spec for icon sub-images — RLE
        // codecs need a per-row state machine no ICO renderer
        // implements, and `BI_JPEG` / `BI_PNG` would smuggle a second
        // codec body through the BMP-DIB code path while the magic
        // sniff already routes proper PNG bodies via the PNG branch.
        //
        // Same probe-vs-render shape as the r198 `biBitCount`, r210
        // `bWidth` / `bHeight`, and the `biPlanes` body check just
        // above: the directory advertises an icon, the body carries a
        // header field that no icon renderer can honour, and a probe
        // inspecting the directory before rendering would miss the
        // mismatch entirely. Reject `biCompression` outside the legal
        // `{BI_RGB, BI_BITFIELDS}` set up front rather than emit an
        // `IconEntryRaw` the harness (or any downstream BMP decoder)
        // then chokes on.
        //
        // The 0-byte tolerance carve-out mirrors the rest of the BMP
        // checks: when the DIB header is too short to carry a
        // `biCompression` field (`< 20` bytes), the value is
        // unobservable and we don't flag it — earlier checks have
        // already taken responsibility for "this isn't a DIB".
        if sub_format == IconSubFormat::Bmp {
            if let Some(compression) = parse_dib_compression(&payload) {
                if compression != BI_RGB && compression != BI_BITFIELDS {
                    return Err(Error::invalid(format!(
                        "ICO: entry {i} body biCompression = {compression} \
                         (must be 0 = BI_RGB or 3 = BI_BITFIELDS)"
                    )));
                }
            }
        }

        // `bColorCount` consistency: must be 0 for ≥ 8 bpp (the palette
        // is too large to fit in a single byte). For ≤ 8 bpp the value
        // is the palette entry count, or 0 to mean "use the default for
        // this bit depth" — both are legal so we only error on the
        // clearly-impossible high-bpp case.
        if bit_depth >= 16 && color_count != 0 {
            return Err(Error::invalid(format!(
                "ICO: entry {i} bColorCount = {color_count} \
                 contradicts {bit_depth}-bpp payload (must be 0)"
            )));
        }

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
        if e.data.is_empty() {
            return Err(Error::invalid(format!("ICO: entry {i} has empty payload")));
        }
        // CUR hotspot must fall inside the sub-image. We mirror
        // `read_ico_raw`'s tolerance: `(0, 0)` is always accepted.
        if icon_type == IconType::Cur {
            if let Some(h) = e.hotspot {
                if (h.x as u32) >= e.width || (h.y as u32) >= e.height {
                    return Err(Error::invalid(format!(
                        "CUR: entry {i} hotspot ({},{}) outside sub-image {}×{}",
                        h.x, h.y, e.width, e.height
                    )));
                }
            }
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

/// Read the BMP DIB body's `biPlanes` (WORD at offset 12). Returns
/// `None` for headers shorter than 14 bytes — those already fail an
/// earlier check and don't need a second error.
fn parse_dib_planes(body: &[u8]) -> Option<u16> {
    if body.len() >= 14 {
        Some(u16::from_le_bytes([body[12], body[13]]))
    } else {
        None
    }
}

/// Read the BMP DIB body's `biCompression` (DWORD at offset 16).
/// Returns `None` for headers shorter than 20 bytes — short DIBs are
/// rejected by earlier dimension / bit-depth checks, so we don't
/// double-flag the same body. Layout: `biSize` (4) + `biWidth` (4) +
/// `biHeight` (4) + `biPlanes` (2) + `biBitCount` (2) =
/// 16 bytes ahead of the `biCompression` field.
fn parse_dib_compression(body: &[u8]) -> Option<u32> {
    if body.len() >= 20 {
        Some(u32::from_le_bytes([body[16], body[17], body[18], body[19]]))
    } else {
        None
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

    /// Build a minimal valid-looking ICO with one fake-PNG entry, so
    /// the validation tests below can mutate one byte at a time and
    /// assert the parser flags the tampered field.
    fn build_minimal_ico() -> Vec<u8> {
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            data: PNG_MAGIC.to_vec(),
        };
        write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap()
    }

    #[test]
    fn read_rejects_ani_riff_acon() {
        // RIFF???? ACON ???? — animated cursor magic.
        let mut bytes = vec![0u8; 12];
        bytes[..4].copy_from_slice(b"RIFF");
        bytes[8..12].copy_from_slice(b"ACON");
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::Unsupported(msg) => assert!(msg.contains(".ani"), "{msg}"),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn read_rejects_zero_count_dir() {
        // Reserved=0, idType=1 (ICO), count=0 — no entries.
        let bytes = [0, 0, 1, 0, 0, 0];
        assert!(read_ico_raw(&bytes).is_err());
    }

    #[test]
    fn read_rejects_non_zero_breserved() {
        let mut bytes = build_minimal_ico();
        // bReserved sits at offset 9 (header 6 + entry byte 3).
        bytes[6 + 3] = 1;
        let err = read_ico_raw(&bytes).unwrap_err();
        assert!(matches!(err, Error::InvalidData(_)));
    }

    #[test]
    fn read_rejects_offset_into_directory() {
        let mut bytes = build_minimal_ico();
        // Rewrite the dwImageOffset to point inside the directory (0)
        // — entry byte 12..16 holds dwImageOffset LE.
        bytes[6 + 12..6 + 16].copy_from_slice(&0u32.to_le_bytes());
        assert!(read_ico_raw(&bytes).is_err());
    }

    #[test]
    fn read_rejects_zero_bytes_in_res() {
        let mut bytes = build_minimal_ico();
        // dwBytesInRes lives at entry bytes 8..12.
        bytes[6 + 8..6 + 12].copy_from_slice(&0u32.to_le_bytes());
        assert!(read_ico_raw(&bytes).is_err());
    }

    #[test]
    fn read_rejects_overflowing_payload_extent() {
        let mut bytes = build_minimal_ico();
        // dwImageOffset huge → offset + size overflows usize.
        bytes[6 + 12..6 + 16].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[6 + 8..6 + 12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_ico_raw(&bytes).is_err());
    }

    #[test]
    fn read_rejects_ico_planes_field_above_one() {
        let mut bytes = build_minimal_ico();
        // wPlanes lives at entry bytes 4..6.
        bytes[6 + 4..6 + 6].copy_from_slice(&7u16.to_le_bytes());
        let err = read_ico_raw(&bytes).unwrap_err();
        assert!(matches!(err, Error::InvalidData(_)));
    }

    #[test]
    fn read_rejects_ico_invalid_bit_count() {
        let mut bytes = build_minimal_ico();
        // wBitCount lives at entry bytes 6..8. 17 is not a valid BMP
        // bit depth.
        bytes[6 + 6..6 + 8].copy_from_slice(&17u16.to_le_bytes());
        assert!(read_ico_raw(&bytes).is_err());
    }

    #[test]
    fn read_accepts_ico_wbitcount_zero() {
        // 0 is "unspecified — defer to the DIB / PNG header" and is
        // common in real files; must not be rejected.
        let mut bytes = build_minimal_ico();
        bytes[6 + 6..6 + 8].copy_from_slice(&0u16.to_le_bytes());
        assert!(read_ico_raw(&bytes).is_ok());
    }

    #[test]
    fn read_rejects_cur_hotspot_outside_image() {
        // Build a 16×16 CUR with hotspot (100, 100) — way out of bounds.
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: Some(HotSpot { x: 0, y: 0 }),
            data: PNG_MAGIC.to_vec(),
        };
        let mut bytes = write_ico_raw(IconType::Cur, std::slice::from_ref(&entry)).unwrap();
        // Overwrite the hotspot at entry bytes 4..8.
        bytes[6 + 4..6 + 6].copy_from_slice(&100u16.to_le_bytes());
        bytes[6 + 6..6 + 8].copy_from_slice(&100u16.to_le_bytes());
        assert!(read_ico_raw(&bytes).is_err());
    }

    #[test]
    fn read_accepts_cur_hotspot_zero_zero() {
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: Some(HotSpot { x: 0, y: 0 }),
            data: PNG_MAGIC.to_vec(),
        };
        let bytes = write_ico_raw(IconType::Cur, std::slice::from_ref(&entry)).unwrap();
        let (ty, got) = read_ico_raw(&bytes).unwrap();
        assert_eq!(ty, IconType::Cur);
        assert_eq!(got[0].hotspot, Some(HotSpot { x: 0, y: 0 }));
    }

    #[test]
    fn write_rejects_cur_hotspot_outside_image() {
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: Some(HotSpot { x: 50, y: 0 }),
            data: PNG_MAGIC.to_vec(),
        };
        assert!(write_ico_raw(IconType::Cur, std::slice::from_ref(&entry)).is_err());
    }

    #[test]
    fn write_rejects_empty_payload() {
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            data: Vec::new(),
        };
        assert!(write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).is_err());
    }

    #[test]
    fn read_rejects_overlapping_entry_payloads() {
        // Build a 2-entry ICO. Both entries are 8 bytes long (one PNG
        // magic each) and the writer naturally lays them out
        // back-to-back, so first we use `write_ico_raw` to get a valid
        // file then we rewrite entry 1's dwImageOffset to point inside
        // entry 0's payload window.
        let payload = PNG_MAGIC.to_vec();
        let entry_a = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            data: payload.clone(),
        };
        let entry_b = IconEntryRaw {
            width: 32,
            height: 32,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            data: payload,
        };
        let mut bytes = write_ico_raw(IconType::Ico, &[entry_a, entry_b]).unwrap();
        // Directory: header (6) + 2 × 16 = 38; entry 0 payload starts
        // at offset 38, entry 1's at offset 46. Rewrite entry 1's
        // dwImageOffset to 40 — that sits 2 bytes into entry 0's
        // payload window, an unambiguous overlap.
        // Entry 1 starts at byte (6 header + 16 first entry) = 22;
        // its dwImageOffset field lives 12 bytes into that entry = 34.
        let entry1_dwoffset = 6 + 16 + 12;
        bytes[entry1_dwoffset..entry1_dwoffset + 4].copy_from_slice(&40u32.to_le_bytes());
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => {
                assert!(msg.contains("overlaps"), "expected overlap msg, got {msg}")
            }
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn read_accepts_adjacent_non_overlapping_payloads() {
        // Two entries with payloads laid out exactly back-to-back —
        // the writer's natural output. Must not be flagged as overlap
        // because the ranges are `[38, 46)` and `[46, 54)`, sharing
        // only the boundary.
        let payload = PNG_MAGIC.to_vec();
        let entry_a = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            data: payload.clone(),
        };
        let entry_b = IconEntryRaw {
            width: 32,
            height: 32,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            data: payload,
        };
        let bytes = write_ico_raw(IconType::Ico, &[entry_a, entry_b]).unwrap();
        let (_, got) = read_ico_raw(&bytes).unwrap();
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn read_write_read_identity_for_mixed_two_entries() {
        // Parse → write → re-parse must be a pure identity for every
        // observable field on a valid 2-entry file. Locks the
        // round-trip contract that callers depend on for "load an ICO,
        // edit one entry, save it back" workflows.
        let entry_a = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            data: PNG_MAGIC.to_vec(),
        };
        // A synthetic 32×32 BMP DIB body (BITMAPINFOHEADER header + a
        // few payload bytes, doubled height for the ICO mask
        // convention).
        let mut dib = vec![0u8; 48];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes()); // biSize
        dib[4..8].copy_from_slice(&32u32.to_le_bytes()); // biWidth
        dib[8..12].copy_from_slice(&64u32.to_le_bytes()); // biHeight (doubled)
        dib[12..14].copy_from_slice(&1u16.to_le_bytes()); // biPlanes
        dib[14..16].copy_from_slice(&32u16.to_le_bytes()); // biBitCount
        let entry_b = IconEntryRaw {
            width: 32,
            height: 32,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib,
        };
        let pass1 = write_ico_raw(IconType::Ico, &[entry_a, entry_b]).unwrap();
        let (ty1, parsed1) = read_ico_raw(&pass1).unwrap();
        // Round-trip the parsed entries through the writer again — the
        // bytes must converge to the same file.
        let pass2 = write_ico_raw(ty1, &parsed1).unwrap();
        assert_eq!(
            pass1, pass2,
            "read→write→read should be a byte-identical fixed point"
        );
        let (ty2, parsed2) = read_ico_raw(&pass2).unwrap();
        assert_eq!(ty1, ty2);
        assert_eq!(parsed1.len(), parsed2.len());
        for (a, b) in parsed1.iter().zip(parsed2.iter()) {
            assert_eq!(a.width, b.width);
            assert_eq!(a.height, b.height);
            assert_eq!(a.sub_format, b.sub_format);
            assert_eq!(a.hotspot, b.hotspot);
            assert_eq!(a.data, b.data);
        }
    }

    #[test]
    fn read_accepts_single_entry_without_overlap_check_trip() {
        // The overlap detector iterates over prior entries; on a
        // single-entry file the inner loop should never execute and
        // the entry must be accepted unconditionally. Regression for a
        // hypothetical refactor that swaps the seed `Vec::new()` for
        // something with a synthetic first range.
        let entry = IconEntryRaw {
            width: 8,
            height: 8,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            data: PNG_MAGIC.to_vec(),
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let (_, got) = read_ico_raw(&bytes).unwrap();
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn read_rejects_truncated_payload_one_byte_short() {
        // Entry declares dwBytesInRes = N but the file ends N-1 bytes
        // into the payload. The off-by-one is the most common
        // truncation case for partial downloads / interrupted writes.
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            data: PNG_MAGIC.to_vec(),
        };
        let mut bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        bytes.pop();
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("past input"),
                "expected truncation msg, got {msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    #[test]
    fn read_rejects_dir_count_overflow_via_u16_max() {
        // idCount = 0xFFFF — directory itself is 6 + 65535*16 ≈ 1 MB,
        // which is fine to size-check but truncated input must produce
        // a clean "directory truncated" error rather than a panic.
        let bytes = [0, 0, 1, 0, 0xff, 0xff];
        let err = read_ico_raw(&bytes).unwrap_err();
        assert!(matches!(err, Error::InvalidData(_)));
    }

    /// Regression for fuzz crash `eacf46ac…`: a CUR file whose
    /// directory declares the canonical `256×256` sub-image (`bWidth`
    /// = `bHeight` = 0) but whose BMP DIB body's `biHeight` decodes to
    /// `2 × 2_097_152` (so the halved height is 2_097_152), well
    /// outside the ICO `(0, 256]` invariant. The fuzz harness flagged
    /// this because the directory walker propagated the body-derived
    /// dimensions without re-checking them against the directory's
    /// `u8` range — meaning probe-vs-render attacks could disagree on
    /// what the sub-image dimensions are. Parser must now reject the
    /// file on the body-derived dimension overflow rather than emit a
    /// rogue `IconEntryRaw` for the harness (or any caller) to choke
    /// on later.
    #[test]
    fn read_rejects_bmp_body_height_above_256() {
        // 256×256 CUR directory + hotspot (64, 2). Body is a synthetic
        // DIB whose biHeight = 0x00400000 (doubled), recovering to
        // 2_097_152 once halved — way outside 256.
        let mut dib = vec![0u8; 18];
        // biSize doesn't matter for the dim check; biWidth at body[4..8]
        // = 2 keeps the legit-width side simple.
        dib[4..8].copy_from_slice(&2u32.to_le_bytes());
        // biHeight (doubled) = 0x00400000 → halved = 2_097_152. That's
        // the value the fuzz crash file landed on.
        dib[8..12].copy_from_slice(&0x00400000u32.to_le_bytes());
        let entry = IconEntryRaw {
            width: 256,
            height: 256,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: Some(HotSpot { x: 64, y: 2 }),
            data: dib,
        };
        let bytes = write_ico_raw(IconType::Cur, std::slice::from_ref(&entry)).unwrap();
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("height") && msg.contains("1..=256"),
                "expected height-out-of-range msg, got {msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    /// Same probe-vs-render attack but on the PNG path: the directory
    /// describes a tiny sub-image (16×16 width/height fields), but the
    /// PNG body's IHDR encodes (1_000_000, 1_000_000). A probe reading
    /// the directory sees one size, a PNG-aware renderer reading the
    /// payload sees another — the parser must reject rather than pick.
    #[test]
    fn read_rejects_png_body_dims_above_256() {
        // 8-byte PNG magic + IHDR with width = height = 1_000_000.
        let mut png = Vec::with_capacity(24);
        png.extend_from_slice(&PNG_MAGIC);
        // PNG IHDR length+type are at bytes 8..16; `parse_png_dims`
        // reads the two BE u32s at bytes 16..24 unconditionally.
        png.extend_from_slice(&[0; 8]);
        png.extend_from_slice(&1_000_000u32.to_be_bytes());
        png.extend_from_slice(&1_000_000u32.to_be_bytes());
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            data: png,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("1..=256"),
                "expected oversized dim msg, got {msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    /// Body claims a zero dimension — the BMP DIB header can encode
    /// `biWidth = 0` or `biHeight = 0` (which the doubled-height
    /// convention halves to 0). Either way it's outside the `(0, 256]`
    /// ICO range and must be rejected, just like the >256 case above.
    #[test]
    fn read_rejects_bmp_body_zero_width() {
        let mut dib = vec![0u8; 18];
        // biWidth = 0 — invalid for a real sub-image.
        dib[4..8].copy_from_slice(&0u32.to_le_bytes());
        // biHeight (doubled) = 32 → halved = 16, plausible.
        dib[8..12].copy_from_slice(&32u32.to_le_bytes());
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("width") && msg.contains("1..=256"),
                "expected width-out-of-range msg, got {msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    /// Regression for fuzz crash `591dc2ca…` (2026-05-31): a BMP DIB
    /// body whose `biBitCount` is 72 (i.e. anything outside the legal
    /// {0,1,4,8,16,24,32} set). The parser previously validated
    /// `wBitCount` in the directory but trusted the body's
    /// `biBitCount` verbatim — fine for the read path, but the writer
    /// would dutifully fold the rogue value back into a fresh
    /// directory, producing a file that fails its own re-read check
    /// (`wBitCount = 72 must be one of 0/1/4/8/16/24/32`). That broke
    /// the parser/writer fixpoint the fuzz harness asserts. The
    /// parser now rejects body bit-depths outside the legal set up
    /// front, with the same error wording as the directory-side
    /// check so triage maps both reports to the same root cause.
    #[test]
    fn read_rejects_bmp_body_invalid_bit_count() {
        // 16×16 BMP DIB body with biBitCount = 72. Width / height
        // are set to plausible values so the dim range check passes
        // and the bit-depth check is the one that triggers.
        let mut dib = vec![0u8; 18];
        dib[4..8].copy_from_slice(&16u32.to_le_bytes()); // biWidth
        dib[8..12].copy_from_slice(&32u32.to_le_bytes()); // doubled biHeight → 16
        dib[14..16].copy_from_slice(&72u16.to_le_bytes()); // biBitCount = 72 (illegal)
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            // wBitCount in the directory stays at 0 ("defer to body
            // header") so this entry only fails on the body-side
            // check we just added.
            bit_depth: 0,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("biBitCount") && msg.contains("72"),
                "expected biBitCount-out-of-range msg, got {msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    /// Regression for fuzz crash `10593ac8…` (2026-05-29): a CUR file
    /// whose directory declares 256×256 (canonical `bWidth = bHeight =
    /// 0`) with a hotspot of (0, 128), but whose BMP DIB body decodes
    /// to a 2×33 sub-image (biWidth = 2, biHeight = 66 doubled →
    /// halved = 33). The first-pass hotspot check used the directory's
    /// declared dims (`256 × 256`) and let the entry through, so the
    /// parser emitted an `IconEntryRaw` whose `hotspot.y = 128` was
    /// outside the body-derived `height = 33` — and the fuzz
    /// harness's invariant check then panicked.
    ///
    /// Same probe-vs-render mismatch shape r178 caught for the body
    /// dim range — the directory says one thing, the body says
    /// another, and a hotspot validated against only the directory
    /// silently passes a sub-image where the renderer would crash.
    /// Parser must now re-check the hotspot against the body-derived
    /// dimensions, rejecting the file rather than emitting a rogue
    /// entry.
    #[test]
    fn read_rejects_cur_hotspot_outside_body_after_dim_recovery() {
        // 2×33 BMP DIB body: biWidth = 2 at body[4..8], biHeight = 66
        // (doubled) at body[8..12]. The doubled-height convention
        // halves it back to 33.
        let mut dib = vec![0u8; 18];
        dib[4..8].copy_from_slice(&2u32.to_le_bytes());
        dib[8..12].copy_from_slice(&66u32.to_le_bytes());
        let entry = IconEntryRaw {
            // Directory declares 256×256 (the `0 → 256` convention)
            // even though the body is 2×33. The writer doesn't
            // validate this mismatch (it serialises width/height
            // verbatim), so we drive it directly.
            width: 256,
            height: 256,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            // Hotspot (0, 128): legal against 256×256, *illegal*
            // against the body's 2×33.
            hotspot: Some(HotSpot { x: 0, y: 128 }),
            data: dib,
        };
        let bytes = write_ico_raw(IconType::Cur, std::slice::from_ref(&entry)).unwrap();
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("hotspot") && msg.contains("outside body"),
                "expected body-hotspot rejection, got {msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    /// Same probe-vs-render shape as above, but for the PNG-body path:
    /// directory claims 256×256, PNG IHDR claims 8×8, hotspot (16, 0)
    /// is outside the body-derived dims. Symmetric coverage with the
    /// BMP case so a future refactor that fixes one path but not the
    /// other gets caught.
    #[test]
    fn read_rejects_cur_hotspot_outside_png_body_after_dim_recovery() {
        // PNG magic + 8-byte filler + (width, height) = (8, 8) BE.
        let mut png = Vec::with_capacity(24);
        png.extend_from_slice(&PNG_MAGIC);
        png.extend_from_slice(&[0; 8]);
        png.extend_from_slice(&8u32.to_be_bytes());
        png.extend_from_slice(&8u32.to_be_bytes());
        let entry = IconEntryRaw {
            width: 256,
            height: 256,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            // hotspot.x = 16 sits inside 256 but outside the body's 8.
            hotspot: Some(HotSpot { x: 16, y: 0 }),
            data: png,
        };
        let bytes = write_ico_raw(IconType::Cur, std::slice::from_ref(&entry)).unwrap();
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("hotspot") && msg.contains("outside body"),
                "expected body-hotspot rejection, got {msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    /// CUR hotspot that's legal against *both* the directory and the
    /// body-derived dims must still be accepted — confirm the new
    /// second-pass check doesn't over-reject on the happy path.
    #[test]
    fn read_accepts_cur_hotspot_inside_body_dims() {
        // 16×16 BMP body, hotspot (5, 7) — well inside.
        let mut dib = vec![0u8; 18];
        dib[4..8].copy_from_slice(&16u32.to_le_bytes());
        dib[8..12].copy_from_slice(&32u32.to_le_bytes()); // doubled → 16
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: Some(HotSpot { x: 5, y: 7 }),
            data: dib,
        };
        let bytes = write_ico_raw(IconType::Cur, std::slice::from_ref(&entry)).unwrap();
        let (ty, got) = read_ico_raw(&bytes).unwrap();
        assert_eq!(ty, IconType::Cur);
        assert_eq!(got[0].hotspot, Some(HotSpot { x: 5, y: 7 }));
    }

    #[test]
    fn read_rejects_bcolorcount_for_highbpp_bmp() {
        // Build a synthetic 16×16 minimal BMP DIB payload claiming
        // 32 bpp, then flip bColorCount to non-zero — the parser must
        // reject the contradiction.
        let mut dib = vec![0u8; 64];
        // biSize = 40 (BITMAPINFOHEADER)
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        // biWidth = 16, biHeight = 32 (doubled), biPlanes = 1
        dib[4..8].copy_from_slice(&16u32.to_le_bytes());
        dib[8..12].copy_from_slice(&32u32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        // biBitCount = 32
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib,
        };
        let mut bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        // bColorCount sits at entry byte 2 of the directory entry.
        bytes[6 + 2] = 16;
        assert!(read_ico_raw(&bytes).is_err());
    }

    /// Directory-vs-body width mismatch on the BMP path. The directory
    /// entry advertises `bWidth = 16`, but the BMP DIB body's
    /// `biWidth` decodes to a different value — the same shape as
    /// the r178 body-dim range check, the r184 CUR hotspot
    /// body-derived check, and the r198 `biBitCount` body check,
    /// applied to the dim *value* (not its range). A probe that
    /// inspected the directory before deciding to render would see
    /// one size; the renderer reading the BMP body would see
    /// another. Reject the file rather than emit an `IconEntryRaw`
    /// whose `width` silently contradicts the directory.
    #[test]
    fn read_rejects_directory_width_mismatch_bmp() {
        let mut dib = vec![0u8; 18];
        // biWidth = 32 (body says 32) — disagrees with directory.
        dib[4..8].copy_from_slice(&32u32.to_le_bytes());
        // doubled biHeight = 32 → halved = 16 (directory's claim).
        dib[8..12].copy_from_slice(&32u32.to_le_bytes());
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("directory width 16")
                    && msg.contains("body sub-image width 32")
                    && msg.contains("probe-vs-render mismatch"),
                "expected dir/body width mismatch msg, got {msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    /// Directory-vs-body height mismatch on the BMP path. Same shape
    /// as the width mismatch above, exercising the second axis.
    #[test]
    fn read_rejects_directory_height_mismatch_bmp() {
        let mut dib = vec![0u8; 18];
        // biWidth = 16 (matches directory).
        dib[4..8].copy_from_slice(&16u32.to_le_bytes());
        // doubled biHeight = 64 → halved = 32 (disagrees with directory).
        dib[8..12].copy_from_slice(&64u32.to_le_bytes());
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("directory height 16")
                    && msg.contains("body sub-image height 32")
                    && msg.contains("probe-vs-render mismatch"),
                "expected dir/body height mismatch msg, got {msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    /// Directory-vs-body width mismatch on the PNG path. Same probe-
    /// vs-render attack: directory says 16×16, PNG IHDR says 64×16.
    #[test]
    fn read_rejects_directory_width_mismatch_png() {
        let mut png = Vec::with_capacity(24);
        png.extend_from_slice(&PNG_MAGIC);
        // PNG length+type bytes 8..16 are ignored by parse_png_dims.
        png.extend_from_slice(&[0; 8]);
        // PNG IHDR width = 64 (disagrees), height = 16 (matches).
        png.extend_from_slice(&64u32.to_be_bytes());
        png.extend_from_slice(&16u32.to_be_bytes());
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            data: png,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("directory width 16")
                    && msg.contains("body sub-image width 64")
                    && msg.contains("probe-vs-render mismatch"),
                "expected dir/body width mismatch msg, got {msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    /// Canonical 256-encoding carve-out. The directory's `bWidth` /
    /// `bHeight` bytes are `0` (the `0 == 256` convention, because a
    /// literal 256 doesn't fit in a `u8`); the body's PNG IHDR
    /// reports `(256, 256)`. The new dir-vs-body consistency check
    /// MUST accept this — the directory cannot physically encode a
    /// disagreeing dim, so the body is authoritative for the 256
    /// case (still subject to the `1..=256` body-dim range check
    /// already enforced).
    #[test]
    fn read_accepts_256_canonical_directory_zero_with_body_256() {
        let mut png = Vec::with_capacity(24);
        png.extend_from_slice(&PNG_MAGIC);
        png.extend_from_slice(&[0; 8]);
        png.extend_from_slice(&256u32.to_be_bytes());
        png.extend_from_slice(&256u32.to_be_bytes());
        let entry = IconEntryRaw {
            width: 256,
            height: 256,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            data: png,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        // Sanity-check: the directory bytes are physically `0` for
        // both axes (the writer applied the canonical encoding).
        assert_eq!(bytes[6], 0, "bWidth byte must be 0 for the 256 case");
        assert_eq!(bytes[7], 0, "bHeight byte must be 0 for the 256 case");
        let (_, got) = read_ico_raw(&bytes).unwrap();
        assert_eq!(got[0].width, 256);
        assert_eq!(got[0].height, 256);
    }

    /// BMP body `biPlanes` outside {0, 1} is the same probe-vs-render
    /// shape as the r198 `biBitCount` body check: the directory
    /// `wPlanes` is already validated against {0, 1}, but the body's
    /// `biPlanes` was previously trusted verbatim. A body claiming
    /// `biPlanes = 7` is a malformed DIB; the writer would otherwise
    /// fold it into a fresh directory whose `wPlanes = 7` then fails
    /// the existing `wPlanes > 1` check on re-read — broken
    /// parser/writer fixpoint. Same wording as the directory-side
    /// `wPlanes` check so a triage grep maps both reports to the
    /// same root cause.
    #[test]
    fn read_rejects_bmp_body_planes_above_one() {
        let mut dib = vec![0u8; 18];
        dib[4..8].copy_from_slice(&16u32.to_le_bytes()); // biWidth
        dib[8..12].copy_from_slice(&32u32.to_le_bytes()); // doubled biHeight → 16
                                                          // biPlanes = 7 — outside the legal {0, 1} set.
        dib[12..14].copy_from_slice(&7u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes()); // biBitCount
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("biPlanes") && msg.contains("7") && msg.contains("must be 0 or 1"),
                "expected biPlanes-out-of-range msg, got {msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    /// `biPlanes = 0` carve-out: real-world writers occasionally emit
    /// 0 ("unspecified") and the directory side accepts it (the
    /// existing `wPlanes > 1` directory check also tolerates 0). The
    /// body check must mirror that — reject anything > 1, accept 0
    /// and 1.
    #[test]
    fn read_accepts_bmp_body_planes_zero() {
        let mut dib = vec![0u8; 18];
        dib[4..8].copy_from_slice(&16u32.to_le_bytes()); // biWidth
        dib[8..12].copy_from_slice(&32u32.to_le_bytes()); // doubled biHeight → 16
        dib[12..14].copy_from_slice(&0u16.to_le_bytes()); // biPlanes = 0
        dib[14..16].copy_from_slice(&32u16.to_le_bytes()); // biBitCount
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        assert!(read_ico_raw(&bytes).is_ok());
    }

    /// The canonical `biPlanes = 1` case must continue to round-trip
    /// cleanly — the happy-path regression for the new check.
    #[test]
    fn read_accepts_bmp_body_planes_one() {
        let mut dib = vec![0u8; 18];
        dib[4..8].copy_from_slice(&16u32.to_le_bytes());
        dib[8..12].copy_from_slice(&32u32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes()); // biPlanes = 1
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let (_, got) = read_ico_raw(&bytes).unwrap();
        assert_eq!(got.len(), 1);
    }

    /// The biPlanes check is BMP-specific — a PNG entry must not
    /// trip on bytes 12..14 of its payload (those are part of the
    /// PNG `IHDR` length / type, not a DIB header).
    #[test]
    fn read_png_body_does_not_validate_biplanes() {
        // Craft a PNG body whose bytes 12..14 happen to contain the
        // value 7 (the same value the BMP test rejects on). The PNG
        // path must ignore it because PNG bodies have no `biPlanes`.
        let mut png = Vec::with_capacity(24);
        png.extend_from_slice(&PNG_MAGIC);
        // PNG bytes 8..16 are the IHDR length (BE u32) + chunk type
        // ("IHDR"). The dim parser reads bytes 16..24 unconditionally.
        // Place `7, 0` at bytes 12..14 so a confused BMP-style read
        // would flag it.
        png.extend_from_slice(&[0, 0, 0, 0, 7, 0, 0, 0]);
        // PNG IHDR width/height (BE) at 16..24.
        png.extend_from_slice(&16u32.to_be_bytes());
        png.extend_from_slice(&16u32.to_be_bytes());
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            data: png,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        // Must NOT trip the biPlanes check — PNG bodies are exempt.
        let (_, got) = read_ico_raw(&bytes).unwrap();
        assert_eq!(got.len(), 1);
    }

    /// Build a 16×16 BMP DIB body with the supplied `biCompression`
    /// value, all other fields filled to plausible defaults. Used by
    /// the biCompression test cluster below.
    fn dib_with_compression(compression: u32) -> Vec<u8> {
        let mut dib = vec![0u8; 40];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes()); // biSize
        dib[4..8].copy_from_slice(&16u32.to_le_bytes()); // biWidth
        dib[8..12].copy_from_slice(&32u32.to_le_bytes()); // biHeight (doubled)
        dib[12..14].copy_from_slice(&1u16.to_le_bytes()); // biPlanes
        dib[14..16].copy_from_slice(&32u16.to_le_bytes()); // biBitCount
        dib[16..20].copy_from_slice(&compression.to_le_bytes()); // biCompression
        dib
    }

    /// `biCompression = BI_RLE8` (= 1) is run-length-encoded 8-bpp —
    /// no ICO renderer handles RLE bodies and the spec explicitly
    /// excludes it. Same probe-vs-render rejection as the `biPlanes`
    /// check above: directory advertises an icon, body carries a
    /// header field no renderer can honour.
    #[test]
    fn read_rejects_bmp_body_compression_rle8() {
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib_with_compression(1), // BI_RLE8
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("biCompression = 1") && msg.contains("BI_RGB"),
                "expected biCompression rejection msg, got {msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    /// `biCompression = BI_RLE4` (= 2) — same rejection class as
    /// BI_RLE8; covers the 4-bpp RLE variant.
    #[test]
    fn read_rejects_bmp_body_compression_rle4() {
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib_with_compression(2), // BI_RLE4
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("biCompression = 2"),
                "expected biCompression rejection msg, got {msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    /// `biCompression = BI_JPEG` (= 4) — would smuggle a JPEG body
    /// through the BMP-DIB code path. The PNG-magic sniff routes
    /// proper PNG payloads via the PNG branch up front; BI_JPEG /
    /// BI_PNG are explicitly excluded from the BMP-side carve-out.
    #[test]
    fn read_rejects_bmp_body_compression_jpeg() {
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib_with_compression(4), // BI_JPEG
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("biCompression = 4"),
                "expected biCompression rejection msg, got {msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    /// `biCompression = BI_PNG` (= 5) — companion to the BI_JPEG case
    /// above. Same rejection: PNG bodies are routed via the PNG
    /// magic-sniff branch, not by trusting a BMP-DIB header field.
    #[test]
    fn read_rejects_bmp_body_compression_png() {
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib_with_compression(5), // BI_PNG
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("biCompression = 5"),
                "expected biCompression rejection msg, got {msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    /// `biCompression = BI_ALPHABITFIELDS` (= 6) — also explicitly
    /// excluded for ICO sub-images even though it's a near-cousin of
    /// the tolerated BI_BITFIELDS case. ICO writers don't produce it
    /// and no ICO renderer parses the trailing alpha mask.
    #[test]
    fn read_rejects_bmp_body_compression_alphabitfields() {
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib_with_compression(6), // BI_ALPHABITFIELDS
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let err = read_ico_raw(&bytes).unwrap_err();
        match err {
            Error::InvalidData(msg) => assert!(
                msg.contains("biCompression = 6"),
                "expected biCompression rejection msg, got {msg}"
            ),
            other => panic!("expected InvalidData, got {other:?}"),
        }
    }

    /// `biCompression = BI_RGB` (= 0) is the canonical, mandated
    /// value — happy-path regression so a future refactor that swaps
    /// the constant or flips the comparison gets caught.
    #[test]
    fn read_accepts_bmp_body_compression_bi_rgb() {
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib_with_compression(0), // BI_RGB
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let (_, got) = read_ico_raw(&bytes).unwrap();
        assert_eq!(got.len(), 1);
    }

    /// `biCompression = BI_BITFIELDS` (= 3) is tolerated for 16-bpp /
    /// 32-bpp DIBs: the body still parses with the same row-stride
    /// rules, the channel masks just live in the bytes after the
    /// fixed BITMAPINFOHEADER. Must be accepted (this is the
    /// carve-out the rejection branch above guards).
    #[test]
    fn read_accepts_bmp_body_compression_bi_bitfields() {
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib_with_compression(3), // BI_BITFIELDS
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let (_, got) = read_ico_raw(&bytes).unwrap();
        assert_eq!(got.len(), 1);
    }

    /// The biCompression check is BMP-specific — a PNG entry whose
    /// IHDR happens to place a non-zero `u32` at bytes 16..20 (which
    /// is the PNG IHDR width / height, big-endian) must not trip on
    /// the new check. Same exemption shape as the biPlanes PNG-body
    /// test above.
    #[test]
    fn read_png_body_does_not_validate_bicompression() {
        // PNG: magic + 8-byte chunk header filler + IHDR width/height
        // (BE) at bytes 16..24. Width = 16 LE-decoded as a u32 at
        // bytes 16..20 would be `0x00000010`, which equals 16 (one of
        // the rejection values if the BMP path mis-fired).
        let mut png = Vec::with_capacity(24);
        png.extend_from_slice(&PNG_MAGIC);
        png.extend_from_slice(&[0; 8]);
        png.extend_from_slice(&16u32.to_be_bytes());
        png.extend_from_slice(&16u32.to_be_bytes());
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            data: png,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        let (_, got) = read_ico_raw(&bytes).unwrap();
        assert_eq!(got.len(), 1);
    }

    /// Short DIB bodies (< 20 bytes — the `biCompression` field sits
    /// at offset 16..20) are exempt from the new check. They've
    /// already failed earlier dim / bit-depth checks; we don't
    /// double-flag the same body, and the helper must return `None`
    /// rather than indexing past the slice. Belt-and-braces sanity:
    /// the existing 18-byte synthetic-body tests must still parse to
    /// their existing failure modes.
    #[test]
    fn read_short_dib_skips_bicompression_check() {
        // 18-byte body — `biCompression` would live at 16..20 but the
        // body is too short. The body still has valid `biWidth = 16`
        // and doubled `biHeight = 32` so the dim / bit-depth checks
        // pass, and the parser falls through to the bColorCount /
        // overlap checks just like a normal short body.
        let mut dib = vec![0u8; 18];
        dib[4..8].copy_from_slice(&16u32.to_le_bytes());
        dib[8..12].copy_from_slice(&32u32.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes()); // biBitCount = 32
        let entry = IconEntryRaw {
            width: 16,
            height: 16,
            bit_depth: 32,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: dib,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        // Must NOT trip the biCompression check on the truncated body.
        let (_, got) = read_ico_raw(&bytes).unwrap();
        assert_eq!(got.len(), 1);
    }

    /// A second carve-out test: directory byte `0` paired with a body
    /// reporting an in-range non-256 dim is still accepted, because
    /// the directory's `0` doesn't actually constrain the body's
    /// dim (the writer wouldn't normally produce this, but a
    /// hand-rolled file can; the spec is silent on which side wins
    /// when the directory writes 0 for a non-256 image, so be
    /// permissive — the body's dim is what the renderer uses).
    #[test]
    fn read_accepts_256_canonical_directory_zero_with_smaller_body() {
        let mut png = Vec::with_capacity(24);
        png.extend_from_slice(&PNG_MAGIC);
        png.extend_from_slice(&[0; 8]);
        // Body claims 128×128 — directory bytes still 0 (canonical).
        png.extend_from_slice(&128u32.to_be_bytes());
        png.extend_from_slice(&128u32.to_be_bytes());
        // Build a directory by hand: byte 6 = bWidth, byte 7 = bHeight,
        // both set to 0; the writer would normally tighten them to
        // match `e.width` / `e.height`, but we want to force the
        // canonical case here.
        let entry = IconEntryRaw {
            width: 256, // tells write_ico_raw to emit 0 for the dir byte
            height: 256,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            data: png,
        };
        let bytes = write_ico_raw(IconType::Ico, std::slice::from_ref(&entry)).unwrap();
        assert_eq!(bytes[6], 0);
        assert_eq!(bytes[7], 0);
        let (_, got) = read_ico_raw(&bytes).unwrap();
        // The renderer takes the body's value (128) because the
        // directory's 0 imposes no constraint.
        assert_eq!(got[0].width, 128);
        assert_eq!(got[0].height, 128);
    }
}
