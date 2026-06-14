//! ICO / CUR file parser (registry-side, oxideav-core-using path).
//!
//! Walks the 6-byte `ICONDIR` header, then each 16-byte `ICONDIRENTRY`,
//! and decodes the pointed-at payload (either PNG or BMP-DIB) into an
//! RGBA [`IconImage`]. The lower-level container layout walk lives in
//! [`crate::raw::read_ico_raw`] which returns raw payload bytes;
//! this layer adds the PNG / BMP-DIB decode on top.

use oxideav_core::{Error, Result};

use crate::ani::{read_ani_raw, AniInfo, AniStep};
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

/// One stored frame of a decoded ANI animation.
///
/// Each `LIST 'fram'` `icon` chunk is a *complete* ICO/CUR resource, so
/// it may itself carry several sub-images at different resolutions (the
/// way a `.ico` does). [`read_ani`] decodes every sub-image of every
/// frame; this struct groups one frame's sub-images together, tagged
/// with the container type the frame declared (`Ico` or `Cur`).
#[derive(Debug, Clone)]
pub struct AniFrame {
    /// The frame's own ICO/CUR container type (frames may be mixed —
    /// the ACON spec permits ICO and CUR frames in one file).
    pub icon_type: IconType,
    /// The frame's decoded sub-images, in directory order. Always at
    /// least one (the parser rejects an empty directory).
    pub images: Vec<IconImage>,
}

/// A fully decoded ANI animated cursor: every stored frame decoded to
/// RGBA, plus the resolved playback timeline.
///
/// This is the ANI-side counterpart of [`read_ico`]'s
/// `(IconType, Vec<IconImage>)`: where `read_ico` decodes one icon
/// resource's sub-images, [`read_ani`] decodes a whole animation —
/// every frame's sub-images *and* the `seq ` / `rate` timeline merged
/// into a flat step table a renderer can drive directly.
#[derive(Debug, Clone)]
pub struct AniAnimation {
    /// Optional `LIST 'INFO'` metadata (title / author).
    pub info: AniInfo,
    /// The stored frames, in `LIST 'fram'` order. Index with
    /// [`AniStep::frame_index`] from [`Self::steps`].
    pub frames: Vec<AniFrame>,
    /// The resolved playback steps — `(frame_index, jiffies)` pairs with
    /// the `seq ` / `rate` / `iDispRate` defaulting already applied (see
    /// [`crate::AniFile::playback_steps`]). Each `frame_index` is a
    /// valid index into [`Self::frames`].
    pub steps: Vec<AniStep>,
}

/// Parse and fully decode an ANI animated-cursor byte stream.
///
/// Walks the RIFF/`ACON` tree (via [`read_ani_raw`]), decodes every
/// stored frame's sub-images to RGBA (via [`read_ico`] per frame), and
/// resolves the `seq ` / `rate` chunks into a flat playback step table
/// (via [`crate::AniFile::playback_steps`]).
///
/// Only the common `AF_ICON`-set path is decodable here: each frame is
/// a complete ICO/CUR resource carrying its own headers. When `AF_ICON`
/// is *clear* the frames are headerless raw BMP data whose geometry
/// lives only in `anih` — there is no ICO directory to walk — so this
/// function returns an error directing the caller to
/// [`crate::AniFile::raw_bmp_descriptor`] + a BMP-DIB decoder for that
/// path. (`read_ani_raw` still parses such files structurally.)
pub fn read_ani(input: &[u8]) -> Result<AniAnimation> {
    let ani = read_ani_raw(input).map_err(|e| Error::invalid(e.to_string()))?;

    if !ani.header.frames_are_icons() {
        return Err(Error::invalid(
            "ANI: read_ani: AF_ICON is clear — frames are headerless raw BMP data \
             with no ICO directory to decode. Use read_ani_raw + \
             AniFile::raw_bmp_descriptor with a BMP-DIB decoder instead.",
        ));
    }

    // Resolve the timeline first: this re-validates the seq / rate
    // lengths and frame-index ranges, so a frame_index emitted here is
    // guaranteed to be in range for the decoded `frames` vec below.
    let steps = ani
        .playback_steps()
        .map_err(|e| Error::invalid(e.to_string()))?;

    let mut frames = Vec::with_capacity(ani.frames.len());
    for (i, frame_bytes) in ani.frames.iter().enumerate() {
        let (icon_type, images) =
            read_ico(frame_bytes).map_err(|e| Error::invalid(format!("ANI: frame {i}: {e}")))?;
        frames.push(AniFrame { icon_type, images });
    }

    Ok(AniAnimation {
        info: ani.info,
        frames,
        steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ani::{AF_ICON, AF_SEQUENCE};
    use crate::write_ico;

    /// Solid-colour RGBA buffer for an `n`×`n` sub-image.
    fn solid_rgba(n: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity((n * n * 4) as usize);
        for _ in 0..(n * n) {
            v.extend_from_slice(&rgba);
        }
        v
    }

    /// A complete single-sub-image ICO byte stream, forced all-BMP so
    /// `read_ico` decodes it without PNG involvement.
    fn ico_frame(n: u32, rgba: [u8; 4]) -> Vec<u8> {
        let img = IconImage::from_rgba(n, n, solid_rgba(n, rgba));
        let opts = WriteOptions {
            png_size_threshold: None,
        };
        write_ico(IconType::Ico, &[img], opts).unwrap()
    }

    /// Append a RIFF chunk (tag + LE u32 len + payload, even-padded).
    fn push_chunk(buf: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
        buf.extend_from_slice(tag);
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            buf.push(0);
        }
    }

    /// Hand-assemble an ANI file from already-encoded ICO frame bodies,
    /// with optional seq / rate chunks and an INFO list.
    fn build_ani(
        frames: &[Vec<u8>],
        n_steps: u32,
        i_disp_rate: u32,
        seq: Option<&[u32]>,
        rate: Option<&[u32]>,
        info: Option<(&[u8], &[u8])>,
    ) -> Vec<u8> {
        let mut anih = [0u8; 36];
        let put =
            |b: &mut [u8; 36], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut anih, 0, 36);
        put(&mut anih, 4, frames.len() as u32);
        put(&mut anih, 8, n_steps);
        put(&mut anih, 24, 1); // nPlanes
        put(&mut anih, 28, i_disp_rate);
        let attrs = AF_ICON | if seq.is_some() { AF_SEQUENCE } else { 0 };
        put(&mut anih, 32, attrs);

        let mut body = Vec::new();
        body.extend_from_slice(b"ACON");
        push_chunk(&mut body, b"anih", &anih);

        if let Some((title, author)) = info {
            let mut info_body = Vec::new();
            info_body.extend_from_slice(b"INFO");
            push_chunk(&mut info_body, b"INAM", title);
            push_chunk(&mut info_body, b"IART", author);
            push_chunk(&mut body, b"LIST", &info_body);
        }
        if let Some(seq) = seq {
            let p: Vec<u8> = seq.iter().flat_map(|v| v.to_le_bytes()).collect();
            push_chunk(&mut body, b"seq ", &p);
        }
        if let Some(rate) = rate {
            let p: Vec<u8> = rate.iter().flat_map(|v| v.to_le_bytes()).collect();
            push_chunk(&mut body, b"rate", &p);
        }
        let mut fram = Vec::new();
        fram.extend_from_slice(b"fram");
        for f in frames {
            push_chunk(&mut fram, b"icon", f);
        }
        push_chunk(&mut body, b"LIST", &fram);

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn read_ani_decodes_frames_and_identity_timeline() {
        let red = ico_frame(8, [200, 10, 10, 255]);
        let green = ico_frame(8, [10, 200, 10, 255]);
        let ani = build_ani(&[red, green], 0, 12, None, None, None);

        let anim = read_ani(&ani).unwrap();
        assert_eq!(anim.frames.len(), 2);
        // Each frame is a single 8×8 BMP sub-image decoded to RGBA.
        for frame in &anim.frames {
            assert_eq!(frame.icon_type, IconType::Ico);
            assert_eq!(frame.images.len(), 1);
            assert_eq!(frame.images[0].width, 8);
            assert_eq!(frame.images[0].height, 8);
            assert_eq!(frame.images[0].pixels.len(), 8 * 8 * 4);
        }
        // No seq chunk → identity timeline, iDispRate for every step.
        assert_eq!(anim.steps.len(), 2);
        assert_eq!(anim.steps[0].frame_index, 0);
        assert_eq!(anim.steps[1].frame_index, 1);
        assert!(anim.steps.iter().all(|s| s.jiffies == 12));
    }

    #[test]
    fn read_ani_applies_seq_and_rate_and_info() {
        let a = ico_frame(8, [1, 2, 3, 255]);
        let b = ico_frame(8, [4, 5, 6, 255]);
        // 2 stored frames, 4 playback steps playing 0,1,1,0 with
        // distinct per-step durations.
        let ani = build_ani(
            &[a, b],
            4,
            10,
            Some(&[0, 1, 1, 0]),
            Some(&[5, 6, 7, 8]),
            Some((b"My Cursor", b"Me")),
        );

        let anim = read_ani(&ani).unwrap();
        assert_eq!(anim.frames.len(), 2);
        let idx: Vec<u32> = anim.steps.iter().map(|s| s.frame_index).collect();
        let jif: Vec<u32> = anim.steps.iter().map(|s| s.jiffies).collect();
        assert_eq!(idx, vec![0, 1, 1, 0]);
        assert_eq!(jif, vec![5, 6, 7, 8]);
        assert_eq!(anim.info.title_str().as_deref(), Some("My Cursor"));
        assert_eq!(anim.info.author_str().as_deref(), Some("Me"));
        // Every resolved step indexes a real decoded frame.
        for s in &anim.steps {
            assert!((s.frame_index as usize) < anim.frames.len());
        }
    }

    #[test]
    fn read_ani_rejects_af_icon_clear() {
        // Build an AF_ICON-clear header (raw headerless BMP frames).
        let mut anih = [0u8; 36];
        let put =
            |b: &mut [u8; 36], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut anih, 0, 36);
        put(&mut anih, 4, 1); // nFrames
        put(&mut anih, 12, 8); // iWidth
        put(&mut anih, 16, 8); // iHeight
        put(&mut anih, 20, 32); // iBitCount
        put(&mut anih, 24, 1); // nPlanes
        put(&mut anih, 28, 10); // iDispRate
        put(&mut anih, 32, 0); // bfAttributes: AF_ICON clear

        let mut body = Vec::new();
        body.extend_from_slice(b"ACON");
        push_chunk(&mut body, b"anih", &anih);
        let mut fram = Vec::new();
        fram.extend_from_slice(b"fram");
        push_chunk(&mut fram, b"icon", &vec![0u8; 8 * 8 * 4]);
        push_chunk(&mut body, b"LIST", &fram);
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);

        let err = read_ani(&out).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("AF_ICON is clear"), "{msg}");
        assert!(msg.contains("raw_bmp_descriptor"), "{msg}");
    }

    #[test]
    fn read_ani_rejects_non_ani_input() {
        // A plain ICO file is not a RIFF/ACON stream.
        let ico = ico_frame(8, [9, 9, 9, 255]);
        assert!(read_ani(&ico).is_err());
    }
}
