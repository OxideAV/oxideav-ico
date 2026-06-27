//! ICO / CUR file encoder (registry-side, oxideav-core-using path).
//!
//! Picks PNG or BMP per sub-image according to [`WriteOptions`] (default:
//! PNG for sizes ≥ 64, BMP otherwise), encodes each [`IconImage`] to
//! its packed payload via `oxideav-png` / `oxideav-bmp`, then hands
//! the resulting `IconEntryRaw` batch to the framework-free
//! [`crate::raw::write_ico_raw`] for the directory layout.

use oxideav_core::{Error, PixelFormat, Result, VideoFrame, VideoPlane};

use crate::ani::{write_ani_raw, AniFile, AniHeader, AniInfo, AF_ICON, AF_SEQUENCE};
use crate::raw::{
    encode_indexed_dib_body, encode_rgb24_dib_body, quantise_rgba_to_indexed, write_ico_raw,
    IconEntryRaw,
};
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
        // The `ICONDIRENTRY` width / height fields are single bytes
        // (`0` encodes the 256 case), so the directory physically
        // cannot describe a sub-image outside `1..=256` in either
        // axis. Reject oversized inputs here — before the (potentially
        // expensive) PNG / BMP encode — so callers get the dimension
        // error up front rather than after wasting an encode pass.
        // `write_ico_raw` re-checks the same bound as a backstop.
        if im.width == 0 || im.height == 0 || im.width > 256 || im.height > 256 {
            return Err(oxideav_core::Error::invalid(format!(
                "ICO: entry {i} dimensions {}×{} out of 1..=256 \
                 (ICONDIRENTRY width/height are single bytes, 0 == 256)",
                im.width, im.height
            )));
        }
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
    for (i, im) in images.iter().enumerate() {
        let chosen = choose_sub_format(im, &opts);
        let bytes = encode_sub_image(im, chosen, opts.bmp_bit_depth)
            .map_err(|e| Error::invalid(format!("ICO: entry {i}: {e}")))?;
        // PNG bodies always carry full RGBA; BMP bodies carry the depth
        // the writer actually emitted so the directory `wBitCount` and
        // the body's `biBitCount` agree (read_ico_raw cross-checks them).
        let bit_depth = match chosen {
            SubFormatChosen::Png => 32,
            SubFormatChosen::Bmp => opts.bmp_bit_depth.bits(),
        };
        entries.push(IconEntryRaw {
            width: im.width,
            height: im.height,
            bit_depth,
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

fn encode_sub_image(
    im: &IconImage,
    fmt: SubFormatChosen,
    bmp_depth: BmpBitDepth,
) -> Result<Vec<u8>> {
    match fmt {
        SubFormatChosen::Png => {
            let frame = iconimage_to_frame(im);
            oxideav_png::encode_single(&frame, im.width, im.height, PixelFormat::Rgba, &[])
        }
        SubFormatChosen::Bmp => encode_bmp_sub_image(im, bmp_depth),
    }
}

/// Encode one BMP-DIB sub-image body at the requested bit depth. The
/// 32-bpp path delegates to `oxideav-bmp` (BGRA + alpha-derived AND
/// mask); the indexed / 24-bpp paths use the framework-free `raw`
/// encoders that build the palette + XOR rows + AND mask in-crate.
fn encode_bmp_sub_image(im: &IconImage, depth: BmpBitDepth) -> Result<Vec<u8>> {
    match depth {
        BmpBitDepth::Bgra32 => {
            // The BMP-inside-ICO convention is doubled height + AND
            // mask appended; oxideav-bmp handles both via the
            // `double_height_for_ico_mask` flag on the registry-gated
            // VideoFrame-shaped wrapper.
            let frame = iconimage_to_frame(im);
            oxideav_bmp::encode_dib_videoframe(
                &frame,
                PixelFormat::Rgba,
                im.width,
                im.height,
                /* doubled */ true,
            )
        }
        BmpBitDepth::Rgb24 => {
            // Drop the RGBA buffer to RGB triples; transparency goes to
            // the AND mask (alpha 0 ⇒ transparent).
            let pixels = im.width as usize * im.height as usize;
            let mut rgb = Vec::with_capacity(pixels * 3);
            let mut transparent = Vec::with_capacity(pixels);
            for p in 0..pixels {
                rgb.push(im.pixels[p * 4]);
                rgb.push(im.pixels[p * 4 + 1]);
                rgb.push(im.pixels[p * 4 + 2]);
                transparent.push(im.pixels[p * 4 + 3] == 0);
            }
            Ok(encode_rgb24_dib_body(
                im.width,
                im.height,
                &rgb,
                &transparent,
            )?)
        }
        BmpBitDepth::Indexed8 | BmpBitDepth::Indexed4 | BmpBitDepth::Indexed1 => {
            let bpp = depth
                .indexed_bpp()
                .expect("indexed variant has an indexed bpp");
            let (palette, indices, transparent) =
                quantise_rgba_to_indexed(im.width, im.height, &im.pixels, bpp)?;
            Ok(encode_indexed_dib_body(
                im.width,
                im.height,
                bpp,
                &palette,
                &indices,
                &transparent,
            )?)
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

// ---------------------------------------------------------------------------
// ANI (animated cursor) high-level encoder — the RGBA-side counterpart to
// [`crate::read_ani`].
// ---------------------------------------------------------------------------

/// One frame of an animation to encode — a complete ICO/CUR resource's
/// worth of sub-images plus the container type the frame should declare.
///
/// This is the encode-side counterpart of [`crate::AniFrame`] (what
/// [`crate::read_ani`] produces): where the reader hands back one
/// `AniFrame` per stored `LIST 'fram'` `icon` chunk (decoded to RGBA),
/// the writer takes one `AniWriteFrame` per chunk and serialises it back
/// to a complete ICO/CUR resource via [`write_ico`]. A frame may carry
/// several resolutions exactly as a `.ico` does — they all share the
/// frame's `icon_type` and become one `icon` chunk.
#[derive(Debug, Clone)]
pub struct AniWriteFrame {
    /// The container type to write this frame as (`Ico` or `Cur`). The
    /// ACON spec permits mixing ICO and CUR frames in one file.
    pub icon_type: IconType,
    /// The frame's sub-images, in directory order. Must be non-empty —
    /// every `icon` chunk needs at least one sub-image, and the
    /// underlying [`write_ico`] rejects an empty batch.
    pub images: Vec<IconImage>,
}

/// Encode-time knobs for [`write_ani`] — the animation-level metadata and
/// timeline that sit alongside the per-frame pixel data.
///
/// The defaults produce the simplest valid animation: identity playback
/// order (each stored frame shown once, in order), every step held for
/// `default_jiffies`, no `LIST 'INFO'` metadata, and per-sub-image PNG /
/// BMP selection per [`WriteOptions::default`].
#[derive(Debug, Clone)]
pub struct AniWriteOptions {
    /// Optional `LIST 'INFO'` title (`INAM`) / author (`IART`) payload
    /// bytes, written verbatim. `None` omits the chunk entirely (no empty
    /// `LIST 'INFO'`). Pair with the raw bytes the caller wants on disk —
    /// the common convention is a NUL-terminated Latin-1 / Windows-1252
    /// string, but this layer writes whatever bytes it is handed.
    pub info: AniInfo,
    /// Optional `seq ` playback order — one zero-based frame index per
    /// step. `None` plays the frames in identity order
    /// (`0, 1, …, n_frames-1`). When `Some`, its length defines the step
    /// count (`nSteps`), the `AF_SEQUENCE` attribute bit is set, and every
    /// index must be `< frames.len()`.
    pub sequence: Option<Vec<u32>>,
    /// Optional `rate` per-step durations in 1/60-second jiffies — one per
    /// playback step. `None` holds every step for `default_jiffies`. When
    /// `Some`, its length must match the resolved step count (the `seq `
    /// length when a sequence is present, else `frames.len()`).
    pub rates: Option<Vec<u32>>,
    /// `anih.iDispRate` — the default per-step duration in 1/60-second
    /// jiffies, used for any step the `rate` chunk doesn't override (and
    /// for every step when `rates` is `None`). Must be non-zero: a
    /// zero-jiffy step has no defined display behaviour and
    /// [`crate::AniFile::playback_steps`] rejects it on the read side.
    pub default_jiffies: u32,
    /// Per-sub-image PNG / BMP selection forwarded to [`write_ico`] for
    /// every frame.
    pub ico: WriteOptions,
}

impl Default for AniWriteOptions {
    fn default() -> Self {
        Self {
            info: AniInfo::default(),
            sequence: None,
            rates: None,
            // 10 jiffies ≈ 1/6 s is a common cursor-animation cadence; any
            // non-zero value is valid. The caller overrides per animation.
            default_jiffies: 10,
            ico: WriteOptions::default(),
        }
    }
}

/// Encode a set of RGBA animation frames into an ANI (RIFF/`ACON`) byte
/// stream — the high-level, pixel-side counterpart to [`crate::read_ani`].
///
/// Where [`write_ico`] serialises one icon resource's sub-images,
/// `write_ani` serialises a whole animation: each [`AniWriteFrame`] is
/// encoded to a complete ICO/CUR resource via [`write_ico`], the resolved
/// `anih` header / `seq ` / `rate` / `LIST 'INFO'` chunks are assembled,
/// and the lot is serialised through [`write_ani_raw`]. The result parses
/// back through [`crate::read_ani`] to an equivalent animation.
///
/// Only the common `AF_ICON`-set path is produced: every frame is a
/// complete ICO/CUR resource (the headerless-raw-BMP `AF_ICON`-clear path
/// has no ICO directory and is out of scope for this pixel-level encoder).
///
/// Errors:
/// * `frames` is empty — an animation needs at least one frame.
/// * `default_jiffies` is `0` — a zero-jiffy step is rejected on read
///   ([`crate::AniFile::playback_steps`]); refusing it here keeps the
///   writer from emitting a file its own reader would reject.
/// * A `rates` entry is `0`, same rationale.
/// * `sequence` carries an index `>= frames.len()`.
/// * `rates.len()` doesn't match the resolved step count (the `sequence`
///   length when present, else `frames.len()`).
/// * Any per-frame [`write_ico`] error (empty sub-image batch, a
///   sub-image outside `1..=256`, a bad pixel-buffer length).
pub fn write_ani(frames: &[AniWriteFrame], opts: &AniWriteOptions) -> Result<Vec<u8>> {
    if frames.is_empty() {
        return Err(Error::invalid(
            "ANI: write_ani: must have at least one frame",
        ));
    }
    let n_frames = frames.len() as u32;

    if opts.default_jiffies == 0 {
        return Err(Error::invalid(
            "ANI: write_ani: default_jiffies = 0 — a zero-jiffy step has no \
             defined display behaviour and read_ani would reject it",
        ));
    }

    // Resolved step count: the seq length drives it when a sequence is
    // present (frames may repeat / play out of order); otherwise every
    // stored frame is shown once, so the step count is the frame count.
    let step_count = match &opts.sequence {
        Some(seq) => seq.len(),
        None => frames.len(),
    };

    // Validate the sequence indices up front so a caller gets a clear
    // error here rather than from the lower-level write_ani_raw.
    if let Some(seq) = &opts.sequence {
        for (i, &idx) in seq.iter().enumerate() {
            if idx >= n_frames {
                return Err(Error::invalid(format!(
                    "ANI: write_ani: sequence[{i}] = {idx} out of range \
                     (frames.len() = {n_frames})"
                )));
            }
        }
    }

    // Resolve the per-step rate table. A zero rate is rejected for the
    // same reason default_jiffies = 0 is.
    if let Some(rates) = &opts.rates {
        if rates.len() != step_count {
            return Err(Error::invalid(format!(
                "ANI: write_ani: rates len {} != resolved step count {step_count}",
                rates.len()
            )));
        }
        for (i, &r) in rates.iter().enumerate() {
            if r == 0 {
                return Err(Error::invalid(format!(
                    "ANI: write_ani: rates[{i}] = 0 — a zero-jiffy step has no \
                     defined display behaviour"
                )));
            }
        }
    }

    // Encode each frame to a complete ICO/CUR resource.
    let mut frame_payloads: Vec<Vec<u8>> = Vec::with_capacity(frames.len());
    for (i, frame) in frames.iter().enumerate() {
        let bytes = write_ico(frame.icon_type, &frame.images, opts.ico)
            .map_err(|e| Error::invalid(format!("ANI: write_ani: frame {i}: {e}")))?;
        frame_payloads.push(bytes);
    }

    // Set nSteps only when a sequence overrides identity order — leaving it
    // 0 lets the reader apply its "nSteps = nFrames" default for the common
    // identity case, exactly as a minimal real-world encoder does.
    let n_steps = if opts.sequence.is_some() {
        step_count as u32
    } else {
        0
    };
    let attrs = AF_ICON
        | if opts.sequence.is_some() {
            AF_SEQUENCE
        } else {
            0
        };

    let header = AniHeader {
        cb_size: 36,
        n_frames,
        n_steps,
        // iWidth / iHeight / iBitCount are the AF_ICON-clear raw-BMP
        // geometry; for the AF_ICON path each frame's own headers are
        // authoritative, so leave the advisory fields at the spec's
        // "take from frame" sentinel (0).
        i_width: 0,
        i_height: 0,
        i_bit_count: 0,
        n_planes: 1,
        i_disp_rate: opts.default_jiffies,
        bf_attributes: attrs,
    };

    let ani = AniFile {
        header,
        info: opts.info.clone(),
        sequence: opts.sequence.clone(),
        rates: opts.rates.clone(),
        frames: frame_payloads,
    };

    Ok(write_ani_raw(&ani)?)
}

#[cfg(test)]
mod bmp_depth_tests {
    use super::*;
    use crate::reader::read_ico;

    /// 2×2 RGBA with `n` distinct opaque colours, cycling palette slots.
    fn rgba_palette(colours: &[[u8; 4]]) -> Vec<u8> {
        let mut v = Vec::new();
        for c in colours {
            v.extend_from_slice(c);
        }
        v
    }

    fn opts_bmp(depth: BmpBitDepth) -> WriteOptions {
        WriteOptions {
            png_size_threshold: None, // force BMP everywhere
            bmp_bit_depth: depth,
        }
    }

    #[test]
    fn write_indexed8_round_trips_pixels_and_directory_depth() {
        let colours = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 0, 255],
        ];
        let img = IconImage::from_rgba(2, 2, rgba_palette(&colours));
        let bytes = write_ico(IconType::Ico, &[img], opts_bmp(BmpBitDepth::Indexed8)).unwrap();

        // Directory wBitCount (entry 0, offset 6+6 = 12) must read 8.
        assert_eq!(u16::from_le_bytes([bytes[12], bytes[13]]), 8);

        let (_, decoded) = read_ico(&bytes).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].bit_depth, 8);
        assert_eq!(decoded[0].sub_format, IconSubFormat::Bmp);
        for (i, c) in colours.iter().enumerate() {
            assert_eq!(&decoded[0].pixels[i * 4..i * 4 + 4], c, "pixel {i}");
        }
    }

    #[test]
    fn write_indexed4_round_trips_pixels_and_directory_depth() {
        // 4×1 row, four distinct colours — fits the 16-entry 4-bpp table.
        let colours = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 0, 255],
        ];
        let img = IconImage::from_rgba(4, 1, rgba_palette(&colours));
        let bytes = write_ico(IconType::Ico, &[img], opts_bmp(BmpBitDepth::Indexed4)).unwrap();
        assert_eq!(u16::from_le_bytes([bytes[12], bytes[13]]), 4);
        let (_, decoded) = read_ico(&bytes).unwrap();
        assert_eq!(decoded[0].bit_depth, 4);
        for (i, c) in colours.iter().enumerate() {
            assert_eq!(&decoded[0].pixels[i * 4..i * 4 + 4], c, "pixel {i}");
        }
    }

    #[test]
    fn write_indexed1_monochrome_round_trips() {
        let colours = [
            [255, 255, 255, 255],
            [0, 0, 0, 255],
            [0, 0, 0, 255],
            [255, 255, 255, 255],
        ];
        let img = IconImage::from_rgba(2, 2, rgba_palette(&colours));
        let bytes = write_ico(IconType::Ico, &[img], opts_bmp(BmpBitDepth::Indexed1)).unwrap();
        assert_eq!(u16::from_le_bytes([bytes[12], bytes[13]]), 1);
        let (_, decoded) = read_ico(&bytes).unwrap();
        assert_eq!(decoded[0].bit_depth, 1);
        for (i, c) in colours.iter().enumerate() {
            assert_eq!(&decoded[0].pixels[i * 4..i * 4 + 4], c, "pixel {i}");
        }
    }

    #[test]
    fn write_rgb24_round_trips_colour_and_mask() {
        // (1,0) transparent via alpha 0.
        let colours = [
            [11, 22, 33, 255],
            [44, 55, 66, 0],
            [77, 88, 99, 255],
            [100, 110, 120, 255],
        ];
        let img = IconImage::from_rgba(2, 2, rgba_palette(&colours));
        let bytes = write_ico(IconType::Ico, &[img], opts_bmp(BmpBitDepth::Rgb24)).unwrap();
        assert_eq!(u16::from_le_bytes([bytes[12], bytes[13]]), 24);
        let (_, decoded) = read_ico(&bytes).unwrap();
        assert_eq!(decoded[0].bit_depth, 24);
        assert_eq!(&decoded[0].pixels[0..4], &[11, 22, 33, 255]);
        assert_eq!(decoded[0].pixels[7], 0, "(1,0) transparent");
        assert_eq!(&decoded[0].pixels[8..12], &[77, 88, 99, 255]);
    }

    #[test]
    fn write_indexed_errors_when_too_many_colours() {
        // 3 distinct colours can't fit a 1-bpp (2-entry) palette.
        let colours = [
            [1, 0, 0, 255],
            [0, 2, 0, 255],
            [0, 0, 3, 255],
            [1, 0, 0, 255],
        ];
        let img = IconImage::from_rgba(2, 2, rgba_palette(&colours));
        let err = write_ico(IconType::Ico, &[img], opts_bmp(BmpBitDepth::Indexed1)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("entry 0"), "error names the entry: {msg}");
        assert!(msg.contains("more than 2 colours"), "got: {msg}");
    }

    #[test]
    fn indexed_depth_ignored_for_png_routed_entries() {
        // With a size threshold the 64×64 entry routes to PNG; the
        // indexed depth only governs the BMP path, so the PNG entry
        // still carries full RGBA at directory bit-depth 32.
        let big = IconImage::from_rgba(64, 64, vec![128u8; 64 * 64 * 4]);
        let opts = WriteOptions {
            png_size_threshold: Some(64),
            bmp_bit_depth: BmpBitDepth::Indexed8,
        };
        let bytes = write_ico(IconType::Ico, &[big], opts).unwrap();
        let (_, decoded) = read_ico(&bytes).unwrap();
        assert_eq!(decoded[0].sub_format, IconSubFormat::Png);
        assert_eq!(decoded[0].bit_depth, 32);
    }
}

#[cfg(test)]
mod ani_write_tests {
    use super::*;
    use crate::ani::AniInfo;
    use crate::reader::read_ani;

    fn solid_rgba(n: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity((n * n * 4) as usize);
        for _ in 0..(n * n) {
            v.extend_from_slice(&rgba);
        }
        v
    }

    fn frame(n: u32, rgba: [u8; 4]) -> AniWriteFrame {
        AniWriteFrame {
            icon_type: IconType::Ico,
            images: vec![IconImage::from_rgba(n, n, solid_rgba(n, rgba))],
        }
    }

    /// All-BMP sub-images so read_ani decodes pixels back exactly without
    /// any lossy PNG involvement.
    fn all_bmp_opts() -> AniWriteOptions {
        AniWriteOptions {
            ico: WriteOptions {
                png_size_threshold: None,
                ..Default::default()
            },
            ..AniWriteOptions::default()
        }
    }

    #[test]
    fn write_ani_identity_round_trips_through_read_ani() {
        let frames = [frame(8, [200, 10, 10, 255]), frame(8, [10, 200, 10, 255])];
        let opts = AniWriteOptions {
            default_jiffies: 12,
            ..all_bmp_opts()
        };
        let bytes = write_ani(&frames, &opts).unwrap();

        let anim = read_ani(&bytes).unwrap();
        assert_eq!(anim.frames.len(), 2);
        // Each frame is one 8×8 sub-image; pixels survive the BMP path.
        assert_eq!(anim.frames[0].images.len(), 1);
        assert_eq!(anim.frames[0].images[0].width, 8);
        assert_eq!(
            anim.frames[0].images[0].pixels,
            solid_rgba(8, [200, 10, 10, 255])
        );
        assert_eq!(
            anim.frames[1].images[0].pixels,
            solid_rgba(8, [10, 200, 10, 255])
        );
        // Identity timeline: each frame once, default 12 jiffies.
        assert_eq!(anim.steps.len(), 2);
        assert_eq!(anim.steps[0].frame_index, 0);
        assert_eq!(anim.steps[1].frame_index, 1);
        assert!(anim.steps.iter().all(|s| s.jiffies == 12));
        assert_eq!(anim.total_jiffies(), 24);
    }

    #[test]
    fn write_ani_indexed8_frames_round_trip() {
        // Drive the whole ANI encode/decode chain through the indexed
        // BMP path: every frame's icon resource is an 8-bpp indexed DIB
        // with a palette + AND mask, which read_ani must decode back to
        // the exact source pixels.
        let frames = [frame(8, [200, 10, 10, 255]), frame(8, [10, 200, 10, 255])];
        let opts = AniWriteOptions {
            default_jiffies: 8,
            ico: WriteOptions {
                png_size_threshold: None,
                bmp_bit_depth: BmpBitDepth::Indexed8,
            },
            ..AniWriteOptions::default()
        };
        let bytes = write_ani(&frames, &opts).unwrap();
        let anim = read_ani(&bytes).unwrap();
        assert_eq!(anim.frames.len(), 2);
        assert_eq!(anim.frames[0].images[0].bit_depth, 8);
        assert_eq!(
            anim.frames[0].images[0].pixels,
            solid_rgba(8, [200, 10, 10, 255])
        );
        assert_eq!(
            anim.frames[1].images[0].pixels,
            solid_rgba(8, [10, 200, 10, 255])
        );
    }

    #[test]
    fn write_ani_indexed1_monochrome_frames_round_trip() {
        // 1-bpp (monochrome) ANI frames — a single solid colour per
        // frame fits a 1-entry palette comfortably.
        let frames = [frame(8, [255, 255, 255, 255]), frame(8, [0, 0, 0, 255])];
        let opts = AniWriteOptions {
            default_jiffies: 6,
            ico: WriteOptions {
                png_size_threshold: None,
                bmp_bit_depth: BmpBitDepth::Indexed1,
            },
            ..AniWriteOptions::default()
        };
        let bytes = write_ani(&frames, &opts).unwrap();
        let anim = read_ani(&bytes).unwrap();
        assert_eq!(anim.frames[0].images[0].bit_depth, 1);
        assert_eq!(
            anim.frames[0].images[0].pixels,
            solid_rgba(8, [255, 255, 255, 255])
        );
        assert_eq!(
            anim.frames[1].images[0].pixels,
            solid_rgba(8, [0, 0, 0, 255])
        );
    }

    #[test]
    fn write_ani_seq_rate_and_info_round_trip() {
        let frames = [frame(8, [1, 2, 3, 255]), frame(8, [4, 5, 6, 255])];
        let opts = AniWriteOptions {
            info: AniInfo {
                title: Some(b"My Cursor\0".to_vec()),
                author: Some(b"Me\0".to_vec()),
            },
            sequence: Some(vec![0, 1, 1, 0]),
            rates: Some(vec![5, 6, 7, 8]),
            default_jiffies: 10,
            ..all_bmp_opts()
        };
        let bytes = write_ani(&frames, &opts).unwrap();

        let anim = read_ani(&bytes).unwrap();
        assert_eq!(anim.frames.len(), 2);
        let idx: Vec<u32> = anim.steps.iter().map(|s| s.frame_index).collect();
        let jif: Vec<u32> = anim.steps.iter().map(|s| s.jiffies).collect();
        assert_eq!(idx, vec![0, 1, 1, 0]);
        assert_eq!(jif, vec![5, 6, 7, 8]);
        assert_eq!(anim.info.title_str().as_deref(), Some("My Cursor"));
        assert_eq!(anim.info.author_str().as_deref(), Some("Me"));
        assert_eq!(anim.total_jiffies(), 26);
    }

    #[test]
    fn write_ani_multi_resolution_frame_round_trips() {
        // One frame carrying two resolutions — both must come back, in
        // directory order, sharing the frame's icon_type.
        let multi = AniWriteFrame {
            icon_type: IconType::Ico,
            images: vec![
                IconImage::from_rgba(8, 8, solid_rgba(8, [9, 8, 7, 255])),
                IconImage::from_rgba(16, 16, solid_rgba(16, [1, 1, 1, 255])),
            ],
        };
        let bytes = write_ani(std::slice::from_ref(&multi), &all_bmp_opts()).unwrap();
        let anim = read_ani(&bytes).unwrap();
        assert_eq!(anim.frames.len(), 1);
        assert_eq!(anim.frames[0].icon_type, IconType::Ico);
        assert_eq!(anim.frames[0].images.len(), 2);
        assert_eq!(anim.frames[0].images[0].width, 8);
        assert_eq!(anim.frames[0].images[1].width, 16);
    }

    #[test]
    fn write_ani_cur_frame_round_trips_hotspot() {
        let mut img = IconImage::from_rgba(16, 16, solid_rgba(16, [3, 3, 3, 255]));
        img.hotspot = Some(HotSpot { x: 4, y: 5 });
        let cur = AniWriteFrame {
            icon_type: IconType::Cur,
            images: vec![img],
        };
        let bytes = write_ani(std::slice::from_ref(&cur), &all_bmp_opts()).unwrap();
        let anim = read_ani(&bytes).unwrap();
        assert_eq!(anim.frames[0].icon_type, IconType::Cur);
        assert_eq!(
            anim.frames[0].images[0].hotspot,
            Some(HotSpot { x: 4, y: 5 })
        );
    }

    #[test]
    fn write_ani_rejects_empty_frames() {
        let err = write_ani(&[], &AniWriteOptions::default()).unwrap_err();
        assert!(err.to_string().contains("at least one frame"));
    }

    #[test]
    fn write_ani_rejects_zero_default_jiffies() {
        let frames = [frame(8, [1, 1, 1, 255])];
        let opts = AniWriteOptions {
            default_jiffies: 0,
            ..all_bmp_opts()
        };
        let err = write_ani(&frames, &opts).unwrap_err();
        assert!(err.to_string().contains("default_jiffies = 0"));
    }

    #[test]
    fn write_ani_rejects_out_of_range_seq_index() {
        let frames = [frame(8, [1, 1, 1, 255])];
        let opts = AniWriteOptions {
            sequence: Some(vec![0, 1]), // index 1 with only 1 frame
            default_jiffies: 5,
            ..all_bmp_opts()
        };
        let err = write_ani(&frames, &opts).unwrap_err();
        assert!(err.to_string().contains("out of range"));
    }

    #[test]
    fn write_ani_rejects_rate_length_mismatch() {
        let frames = [frame(8, [1, 1, 1, 255]), frame(8, [2, 2, 2, 255])];
        let opts = AniWriteOptions {
            rates: Some(vec![5]), // 2 frames, identity → 2 steps expected
            default_jiffies: 5,
            ..all_bmp_opts()
        };
        let err = write_ani(&frames, &opts).unwrap_err();
        assert!(err.to_string().contains("resolved step count"));
    }

    #[test]
    fn write_ani_rejects_zero_rate_entry() {
        let frames = [frame(8, [1, 1, 1, 255]), frame(8, [2, 2, 2, 255])];
        let opts = AniWriteOptions {
            rates: Some(vec![5, 0]),
            default_jiffies: 5,
            ..all_bmp_opts()
        };
        let err = write_ani(&frames, &opts).unwrap_err();
        assert!(err.to_string().contains("zero-jiffy"));
    }
}
