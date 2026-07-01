//! ICO / CUR container: one [`Packet`] per directory entry. Each
//! packet's `data` is the raw sub-image bytes (PNG or BMP DIB) exactly
//! as they appear in the file, so downstream consumers (the ICO
//! codec or any PNG / BMP codec that can probe the magic) can decode
//! them directly.
//!
//! The demuxer exposes one `StreamInfo` per sub-image. `pts = index`
//! (0-based directory order); `width` / `height` carry the sub-image's
//! resolved dimensions (the directory's `0 == 256` convention applied
//! and cross-validated against the body's PNG-IHDR / BMP-DIB header by
//! the shared [`crate::read_ico_raw`] parser, so the demuxer can never
//! diverge from the standalone API on geometry). Hotspots from CUR
//! entries are surfaced in `StreamInfo::params.extradata` as a 4-byte
//! `u16 x`, `u16 y` little-endian pair (empty for ICO entries).

use std::io::{Read, SeekFrom, Write};

use oxideav_core::{
    CodecId, CodecParameters, CodecResolver, Error, MediaType, Packet, PixelFormat, Result,
    StreamInfo, TimeBase,
};
use oxideav_core::{
    ContainerRegistry, Demuxer, Muxer, ProbeData, ProbeScore, ReadSeek, WriteSeek, MAX_PROBE_SCORE,
};

pub fn register(reg: &mut ContainerRegistry) {
    reg.register_demuxer("ico", open_demuxer);
    reg.register_muxer("ico", open_muxer);
    reg.register_extension("ico", "ico");
    reg.register_extension("cur", "ico");
    reg.register_probe("ico", probe);

    // ANI (RIFF/`ACON` animated cursor) is a distinct container — its
    // own demuxer, probe, and `.ani` extension. Demux-only: an ANI is a
    // playback timeline (frames + `seq `/`rate`), which the round-trip
    // muxer surface in `ani::write_ani` already covers at the RGBA
    // level; there is no framework `Muxer` for it.
    reg.register_demuxer("ani", open_ani_demuxer);
    reg.register_extension("ani", "ani");
    reg.register_probe("ani", probe_ani);
}

/// Recognise ICO + CUR files by their 6-byte `ICONDIR` header.
fn probe(data: &ProbeData) -> ProbeScore {
    if data.buf.len() >= 6
        && data.buf[0] == 0
        && data.buf[1] == 0
        && (data.buf[2] == 1 || data.buf[2] == 2)
        && data.buf[3] == 0
    {
        MAX_PROBE_SCORE
    } else if matches!(data.ext, Some("ico") | Some("cur")) {
        oxideav_core::PROBE_SCORE_EXTENSION
    } else {
        0
    }
}

fn open_demuxer(
    mut input: Box<dyn ReadSeek>,
    _codecs: &dyn CodecResolver,
) -> Result<Box<dyn Demuxer>> {
    input.seek(SeekFrom::Start(0))?;
    let mut buf = Vec::new();
    input.read_to_end(&mut buf)?;

    // Delegate the directory walk to the hardened standalone parser
    // ([`crate::read_ico_raw`]) rather than re-implementing a thinner,
    // looser copy here. The previous in-line walk only checked
    // payload-extent bounds; `read_ico_raw` additionally rejects the
    // whole CVE surface this container otherwise inherited verbatim —
    // overlapping sub-image payload ranges, a non-zero `bReserved`
    // byte, an out-of-range `wPlanes` / `wBitCount`, a CUR hotspot
    // outside the (body-derived) sub-image, a directory-vs-body
    // dimension / bit-depth disagreement, and a body whose
    // `biSize` / `biPlanes` / `biCompression` / `biBitCount` fall
    // outside the legal ICO set. It also surfaces the `.ani`
    // RIFF/ACON case as a clean `Unsupported` (the same refusal the
    // old in-line check produced). One parse, one set of rules, so the
    // demuxer and the standalone API can never diverge on what a
    // well-formed file is.
    let (icon_type, entries) = crate::read_ico_raw(&buf).map_err(Error::from)?;

    let count = entries.len();
    let mut streams = Vec::with_capacity(count);
    let mut packets = Vec::with_capacity(count);
    for (i, entry) in entries.into_iter().enumerate() {
        let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
        // `read_ico_raw` resolves the directory's `0 == 256` convention
        // *and* cross-validates the body's PNG-IHDR / BMP-DIB dimensions
        // against the directory, so these are the authoritative, agreed
        // sub-image dimensions — not just the directory's `u8` self-claim.
        params.width = Some(entry.width);
        params.height = Some(entry.height);
        params.pixel_format = Some(PixelFormat::Rgba);
        if icon_type == crate::IconType::Cur {
            // CUR — surface the hotspot in extradata so callers that
            // need it don't have to re-parse the directory. A CUR entry
            // always carries a hotspot (`read_ico_raw` populates it,
            // defaulting to `(0, 0)`), but guard with a match anyway so
            // an `Ico`-vs-`Cur` mismatch can't panic.
            if let Some(h) = entry.hotspot {
                let mut ed = Vec::with_capacity(4);
                ed.extend_from_slice(&h.x.to_le_bytes());
                ed.extend_from_slice(&h.y.to_le_bytes());
                params.extradata = ed;
            }
        }
        streams.push(StreamInfo {
            index: i as u32,
            params,
            time_base: TimeBase::new(1, 1),
            start_time: Some(0),
            duration: None,
        });

        let mut pkt = Packet::new(i as u32, TimeBase::new(1, 1), entry.data);
        pkt.pts = Some(i as i64);
        pkt.dts = Some(i as i64);
        pkt.flags.keyframe = true;
        packets.push(pkt);
    }

    Ok(Box::new(IcoDemuxer {
        streams,
        pending: packets,
    }))
}

struct IcoDemuxer {
    streams: Vec<StreamInfo>,
    /// Remaining packets, in directory order. Drained FIFO.
    pending: Vec<Packet>,
}

impl Demuxer for IcoDemuxer {
    fn format_name(&self) -> &str {
        "ico"
    }
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }
    fn next_packet(&mut self) -> Result<Packet> {
        if self.pending.is_empty() {
            Err(Error::Eof)
        } else {
            Ok(self.pending.remove(0))
        }
    }
}

fn open_muxer(output: Box<dyn WriteSeek>, streams: &[StreamInfo]) -> Result<Box<dyn Muxer>> {
    if streams.is_empty() {
        return Err(Error::invalid("ICO muxer: need at least one stream"));
    }
    // All streams must claim `codec_id = "ico"` (the muxer carries
    // pre-encoded sub-image bytes in packet data, so the stream's
    // nominal codec identifies the container format itself).
    for s in streams {
        if s.params.media_type != MediaType::Video {
            return Err(Error::invalid("ICO muxer: all streams must be video"));
        }
    }
    // Assume ICO unless the first stream's extradata contains a 4-byte
    // hotspot — conventional hint that the producer built the packets
    // with CUR semantics in mind.
    let is_cur = !streams[0].params.extradata.is_empty();
    Ok(Box::new(IcoMuxer {
        output,
        is_cur,
        stream_entries: streams
            .iter()
            .map(|s| StreamEntry {
                width: s.params.width.unwrap_or(0),
                height: s.params.height.unwrap_or(0),
                hotspot: if s.params.extradata.len() >= 4 {
                    Some((
                        u16::from_le_bytes([s.params.extradata[0], s.params.extradata[1]]),
                        u16::from_le_bytes([s.params.extradata[2], s.params.extradata[3]]),
                    ))
                } else {
                    None
                },
            })
            .collect(),
        packet_bodies: Vec::new(),
    }))
}

struct StreamEntry {
    width: u32,
    height: u32,
    /// Only populated for CUR streams.
    hotspot: Option<(u16, u16)>,
}

struct IcoMuxer {
    output: Box<dyn WriteSeek>,
    is_cur: bool,
    stream_entries: Vec<StreamEntry>,
    /// One `Vec<u8>` per `write_packet` call, collected in arrival
    /// order. `write_trailer` flushes the header + directory +
    /// payloads in one go because we need to know every payload's
    /// length before the directory can be laid out.
    packet_bodies: Vec<Vec<u8>>,
}

impl Muxer for IcoMuxer {
    fn format_name(&self) -> &str {
        "ico"
    }
    fn write_header(&mut self) -> Result<()> {
        Ok(())
    }
    fn write_packet(&mut self, packet: &Packet) -> Result<()> {
        self.packet_bodies.push(packet.data.clone());
        Ok(())
    }
    fn write_trailer(&mut self) -> Result<()> {
        if self.packet_bodies.is_empty() {
            return Err(Error::invalid("ICO muxer: no packets"));
        }
        if self.packet_bodies.len() != self.stream_entries.len() {
            return Err(Error::invalid("ICO muxer: packet count != stream count"));
        }
        let count = self.packet_bodies.len();
        let dir_size = 6 + 16 * count;
        let mut total = dir_size;
        let mut offsets = Vec::with_capacity(count);
        for body in &self.packet_bodies {
            offsets.push(total as u32);
            total += body.len();
        }
        let id_type: u16 = if self.is_cur { 2 } else { 1 };
        self.output.write_all(&0u16.to_le_bytes())?;
        self.output.write_all(&id_type.to_le_bytes())?;
        self.output.write_all(&(count as u16).to_le_bytes())?;
        for (i, entry) in self.stream_entries.iter().enumerate() {
            let body = &self.packet_bodies[i];
            // `bits_per_pixel` — sniff from the payload so we write a
            // value that matches what's actually in the body.
            let bpp = sniff_bpp(body);
            let w_byte = if entry.width == 256 {
                0
            } else {
                entry.width as u8
            };
            let h_byte = if entry.height == 256 {
                0
            } else {
                entry.height as u8
            };
            self.output.write_all(&[w_byte, h_byte, 0, 0])?;
            let (planes, bits) = match (self.is_cur, entry.hotspot) {
                (true, Some((x, y))) => (x, y),
                (true, None) => (0, 0),
                (false, _) => (1, bpp),
            };
            self.output.write_all(&planes.to_le_bytes())?;
            self.output.write_all(&bits.to_le_bytes())?;
            self.output.write_all(&(body.len() as u32).to_le_bytes())?;
            self.output.write_all(&offsets[i].to_le_bytes())?;
        }
        for body in &self.packet_bodies {
            self.output.write_all(body)?;
        }
        Ok(())
    }
}

/// Peek at a sub-image body and guess its bits-per-pixel for the
/// `ICONDIRENTRY.wBitCount` field. PNG is treated as 32 bpp RGBA;
/// BMP reads from the header's `biBitCount`.
fn sniff_bpp(body: &[u8]) -> u16 {
    const PNG_MAGIC: &[u8; 8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    if body.len() >= 8 && &body[..8] == PNG_MAGIC {
        32
    } else if body.len() >= 16 {
        // BITMAPINFOHEADER: biBitCount at offset 14 (size + w + h +
        // planes = 4 + 4 + 4 + 2 bytes).
        u16::from_le_bytes([body[14], body[15]])
    } else {
        32
    }
}

// ───────────────────────── ANI (RIFF/ACON) demuxer ─────────────────────────

/// Recognise a Windows ANI animated cursor by its `RIFF`…`ACON` header.
///
/// Mirrors the ICO probe's two-tier scoring: an unambiguous magic match
/// (`RIFF` at offset 0 + `ACON` form-type at offset 8) scores
/// [`MAX_PROBE_SCORE`]; a bare `.ani` extension with no readable header
/// scores [`PROBE_SCORE_EXTENSION`]. The ICO probe deliberately does
/// *not* claim ANI (it inspects `idType` at offset 2, which for a RIFF
/// header reads as the ASCII of `FF`), so the two probes never both fire
/// at full confidence on the same input.
fn probe_ani(data: &ProbeData) -> ProbeScore {
    if data.buf.len() >= 12 && &data.buf[..4] == b"RIFF" && &data.buf[8..12] == b"ACON" {
        MAX_PROBE_SCORE
    } else if matches!(data.ext, Some("ani")) {
        oxideav_core::PROBE_SCORE_EXTENSION
    } else {
        0
    }
}

/// Open an ANI animated cursor as a framework [`Demuxer`].
///
/// The ANI is walked once (via [`crate::read_ani_raw`]) and its
/// `seq `/`rate` timeline resolved (via
/// [`crate::AniFile::playback_steps`]) into a flat playback table. The
/// demuxer then presents the animation as a **single video stream**
/// whose packets are the playback *steps* in display order: each packet
/// carries the chosen frame's raw `icon` payload (a complete ICO/CUR
/// resource on the common `AF_ICON`-set path, ready for the ICO codec or
/// any PNG/BMP probe to decode) with a presentation timestamp and
/// duration drawn from the jiffy timeline.
///
/// Timing uses the ACON-native 1/60-second jiffy as the stream time
/// base (`TimeBase::new(1, 60)`), so a packet's `pts` is the cumulative
/// jiffy offset of its step and `duration` is the step's own jiffy
/// count — no lossy conversion to another rate. A step that repeats a
/// stored frame (via `seq `) re-emits that frame's bytes, so a consumer
/// that just plays packets in order reproduces the full animation,
/// including out-of-storage-order and repeated frames, without
/// consulting `seq ` itself.
///
/// The single-cycle timeline is emitted once (an ANI cursor loops
/// indefinitely when shown live, but a demuxer yields one pass and lets
/// the consumer loop). `AF_ICON`-clear (raw headerless BMP) frames are
/// carried byte-for-byte too — the geometry needed to decode them lives
/// in the stream's [`CodecParameters`], populated from `anih`.
fn open_ani_demuxer(
    mut input: Box<dyn ReadSeek>,
    _codecs: &dyn CodecResolver,
) -> Result<Box<dyn Demuxer>> {
    input.seek(SeekFrom::Start(0))?;
    let mut buf = Vec::new();
    input.read_to_end(&mut buf)?;

    let ani = crate::read_ani_raw(&buf).map_err(oxideav_core::Error::from)?;
    let steps = ani.playback_steps().map_err(oxideav_core::Error::from)?;

    // Per-frame `icon` payloads, indexed by `AniStep::frame_index`. The
    // timeline resolver already guaranteed every index is in range.
    let frames = &ani.frames;

    // One video stream describing the animation. Width/height come from
    // `anih` when the encoder filled them; otherwise they stay `None`
    // (the spec's `0` = "take from frame" sentinel — the per-frame
    // ICO/CUR directory carries the authoritative geometry on the
    // `AF_ICON` path).
    let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
    if ani.header.i_width != 0 {
        params.width = Some(ani.header.i_width);
    }
    if ani.header.i_height != 0 {
        params.height = Some(ani.header.i_height);
    }
    // `iBitCount` is the third piece of the `AF_ICON`-clear raw-BMP
    // descriptor: the frame bodies are headerless DIB pixel rows, so a
    // consumer needs the bit-depth (alongside width / height) to decode
    // them. Surface it as a 4-byte little-endian `extradata` value when
    // the header filled it in (the raw path otherwise carries no
    // extradata, so a present 4-byte block unambiguously means "raw-BMP
    // `iBitCount`"). On the `AF_ICON`-set path it stays the "take from
    // frame" sentinel (`0`) — the per-frame ICO/CUR directory is
    // authoritative there, exactly as with width / height above — so
    // leave `extradata` empty.
    if ani.header.i_bit_count != 0 {
        params.extradata = ani.header.i_bit_count.to_le_bytes().to_vec();
    }
    params.pixel_format = Some(PixelFormat::Rgba);
    // The 1/60-s jiffy is the animation's native cadence; expose it as
    // the stream's nominal frame rate (steps are not uniform, so this is
    // the timeline tick, not a fixed fps).
    params.frame_rate = Some(oxideav_core::Rational::new(60, 1));

    // Resolve INFO metadata (title / author) into the demuxer's
    // container-level metadata table.
    let mut metadata = Vec::new();
    if let Some(title) = ani.info.title_str() {
        metadata.push(("title".to_owned(), title));
    }
    if let Some(author) = ani.info.author_str() {
        metadata.push(("artist".to_owned(), author));
    }

    // Build the packet timeline: one packet per playback step, in
    // display order, with cumulative-jiffy `pts` and per-step
    // `duration`. `time_base` is 1/60 s so `pts`/`duration` are jiffies.
    let tb = TimeBase::new(1, 60);
    let mut packets = Vec::with_capacity(steps.len());
    let mut cumulative: i64 = 0;
    for step in &steps {
        let body = frames
            .get(step.frame_index as usize)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "ANI: step frame index {} out of range ({} frames)",
                    step.frame_index,
                    frames.len()
                ))
            })?
            .clone();
        let mut pkt = Packet::new(0, tb, body);
        pkt.pts = Some(cumulative);
        pkt.dts = Some(cumulative);
        pkt.duration = Some(step.jiffies as i64);
        pkt.flags.keyframe = true;
        packets.push(pkt);
        cumulative += step.jiffies as i64;
    }

    // Stream duration is the full single-cycle length in jiffies.
    let total_jiffies = cumulative;
    let stream = StreamInfo {
        index: 0,
        params,
        time_base: tb,
        start_time: Some(0),
        duration: Some(total_jiffies),
    };

    // Container duration in microseconds: jiffies × 1_000_000 / 60.
    let duration_micros = total_jiffies.saturating_mul(1_000_000) / 60;

    Ok(Box::new(AniDemuxer {
        streams: vec![stream],
        pending: packets,
        metadata,
        duration_micros,
    }))
}

struct AniDemuxer {
    streams: Vec<StreamInfo>,
    /// Remaining packets, in display order. Drained FIFO.
    pending: Vec<Packet>,
    /// Container-level metadata (title / artist) from the `INFO` list.
    metadata: Vec<(String, String)>,
    /// Single-cycle duration in microseconds.
    duration_micros: i64,
}

impl Demuxer for AniDemuxer {
    fn format_name(&self) -> &str {
        "ani"
    }
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }
    fn next_packet(&mut self) -> Result<Packet> {
        if self.pending.is_empty() {
            Err(Error::Eof)
        } else {
            Ok(self.pending.remove(0))
        }
    }
    fn metadata(&self) -> &[(String, String)] {
        &self.metadata
    }
    fn duration_micros(&self) -> Option<i64> {
        Some(self.duration_micros)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ani::{AF_ICON, AF_SEQUENCE};
    use crate::{write_ico, IconImage, IconType, WriteOptions};
    use oxideav_core::NullCodecResolver;
    use std::io::{Cursor, Seek, Write};
    use std::sync::{Arc, Mutex};

    /// A `Write + Seek + Send` sink backed by a shared `Cursor<Vec<u8>>`,
    /// so a test can hand a boxed muxer its output and still read the
    /// written bytes back afterwards (a plain `Cursor` is consumed by the
    /// `Box<dyn WriteSeek>`).
    #[derive(Clone)]
    struct SharedSink(Arc<Mutex<Cursor<Vec<u8>>>>);

    impl SharedSink {
        fn new() -> Self {
            SharedSink(Arc::new(Mutex::new(Cursor::new(Vec::new()))))
        }
        fn into_bytes(self) -> Vec<u8> {
            self.0.lock().unwrap().get_ref().clone()
        }
    }

    impl Write for SharedSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0.lock().unwrap().flush()
        }
    }

    impl Seek for SharedSink {
        fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
            self.0.lock().unwrap().seek(pos)
        }
    }

    /// A solid-colour single-sub-image ICO byte stream, forced all-BMP.
    fn ico_frame(n: u32, rgba: [u8; 4]) -> Vec<u8> {
        let pixels: Vec<u8> = std::iter::repeat(rgba)
            .take((n * n) as usize)
            .flatten()
            .collect();
        let img = IconImage::from_rgba(n, n, pixels);
        write_ico(
            IconType::Ico,
            &[img],
            WriteOptions {
                png_size_threshold: None,
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn push_chunk(buf: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
        buf.extend_from_slice(tag);
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            buf.push(0);
        }
    }

    /// Hand-assemble an `AF_ICON`-set ANI from encoded ICO frame bodies.
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
        put(&mut anih, 24, 1);
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

    fn open_ani(bytes: &[u8]) -> Box<dyn Demuxer> {
        open_ani_demuxer(Box::new(Cursor::new(bytes.to_vec())), &NullCodecResolver).unwrap()
    }

    fn open_ico(bytes: &[u8]) -> Result<Box<dyn Demuxer>> {
        open_demuxer(Box::new(Cursor::new(bytes.to_vec())), &NullCodecResolver)
    }

    #[test]
    fn ico_demuxer_emits_one_stream_and_packet_per_sub_image() {
        // Two-resolution all-BMP ICO. The demuxer presents one stream +
        // one packet per sub-image, in directory order, each packet
        // carrying that entry's raw payload bytes.
        let imgs = vec![
            IconImage::from_rgba(
                8,
                8,
                std::iter::repeat([1u8, 2, 3, 255])
                    .take(64)
                    .flatten()
                    .collect(),
            ),
            IconImage::from_rgba(
                16,
                16,
                std::iter::repeat([9u8, 9, 9, 255])
                    .take(256)
                    .flatten()
                    .collect(),
            ),
        ];
        let bytes = write_ico(
            IconType::Ico,
            &imgs,
            WriteOptions {
                png_size_threshold: None,
                ..Default::default()
            },
        )
        .unwrap();

        let mut dx = open_ico(&bytes).unwrap();
        assert_eq!(dx.format_name(), "ico");
        assert_eq!(dx.streams().len(), 2);
        // Body-derived (and directory-cross-validated) dimensions.
        assert_eq!(dx.streams()[0].params.width, Some(8));
        assert_eq!(dx.streams()[0].params.height, Some(8));
        assert_eq!(dx.streams()[1].params.width, Some(16));
        assert_eq!(dx.streams()[1].params.height, Some(16));
        // ICO entries carry no hotspot extradata.
        assert!(dx.streams()[0].params.extradata.is_empty());

        let p0 = dx.next_packet().unwrap();
        assert_eq!(p0.pts, Some(0));
        assert!(p0.flags.keyframe);
        let p1 = dx.next_packet().unwrap();
        assert_eq!(p1.pts, Some(1));
        assert!(matches!(dx.next_packet(), Err(Error::Eof)));
        // Each packet's bytes are a decodable BMP-DIB body.
        assert!(!p0.data.is_empty());
        assert!(!p1.data.is_empty());
    }

    #[test]
    fn ico_demuxer_surfaces_cur_hotspot_in_extradata() {
        let mut img = IconImage::from_rgba(
            16,
            16,
            std::iter::repeat([7u8, 7, 7, 255])
                .take(256)
                .flatten()
                .collect(),
        );
        img.hotspot = Some(crate::HotSpot { x: 5, y: 9 });
        let bytes = write_ico(
            IconType::Cur,
            &[img],
            WriteOptions {
                png_size_threshold: None,
                ..Default::default()
            },
        )
        .unwrap();
        let dx = open_ico(&bytes).unwrap();
        let ed = &dx.streams()[0].params.extradata;
        assert_eq!(ed.len(), 4);
        assert_eq!(u16::from_le_bytes([ed[0], ed[1]]), 5);
        assert_eq!(u16::from_le_bytes([ed[2], ed[3]]), 9);
    }

    #[test]
    fn ico_demuxer_inherits_overlap_hardening() {
        // Two adjacent entries, then rewrite entry 1's dwImageOffset to
        // overlap entry 0's payload window. The old in-line directory
        // walk only bounds-checked payload extents and would have
        // accepted this; delegating to `read_ico_raw` inherits its
        // cross-entry overlap rejection (a known icon-parser CVE shape).
        let body = std::iter::repeat([1u8, 2, 3, 255])
            .take(64)
            .flatten()
            .collect::<Vec<_>>();
        let a = IconImage::from_rgba(8, 8, body.clone());
        let b = IconImage::from_rgba(8, 8, body);
        let mut bytes = write_ico(
            IconType::Ico,
            &[a, b],
            WriteOptions {
                png_size_threshold: None,
                ..Default::default()
            },
        )
        .unwrap();
        // Entry 1's dwImageOffset lives at file offset 6 + 16 + 12 = 34.
        // Point it at entry 0's payload start (just past the directory).
        let dir_end = (6 + 16 * 2) as u32;
        bytes[34..38].copy_from_slice(&dir_end.to_le_bytes());
        match open_ico(&bytes) {
            Ok(_) => panic!("expected overlap rejection, got a demuxer"),
            Err(e) => assert!(
                e.to_string().to_lowercase().contains("overlap"),
                "expected overlap rejection, got: {e}"
            ),
        }
    }

    #[test]
    fn ico_muxer_round_trips_through_demuxer() {
        // Build an ICO via write_ico, demux it into streams + packets,
        // then re-mux those through the framework IcoMuxer and confirm
        // the muxed bytes parse back to the same two sub-images. Covers
        // open_muxer / write_header / write_packet / write_trailer, which
        // had no test.
        let imgs = vec![
            IconImage::from_rgba(
                8,
                8,
                std::iter::repeat([1u8, 2, 3, 255])
                    .take(64)
                    .flatten()
                    .collect(),
            ),
            IconImage::from_rgba(
                16,
                16,
                std::iter::repeat([9u8, 8, 7, 255])
                    .take(256)
                    .flatten()
                    .collect(),
            ),
        ];
        let src = write_ico(
            IconType::Ico,
            &imgs,
            WriteOptions {
                png_size_threshold: None,
                ..Default::default()
            },
        )
        .unwrap();

        let mut dx = open_ico(&src).unwrap();
        let streams: Vec<StreamInfo> = dx.streams().to_vec();
        let mut packets = Vec::new();
        while let Ok(p) = dx.next_packet() {
            packets.push(p);
        }
        assert_eq!(packets.len(), 2);

        let sink = SharedSink::new();
        {
            let mut mux = open_muxer(Box::new(sink.clone()), &streams).unwrap();
            assert_eq!(mux.format_name(), "ico");
            mux.write_header().unwrap();
            for p in &packets {
                mux.write_packet(p).unwrap();
            }
            mux.write_trailer().unwrap();
        }
        let muxed = sink.into_bytes();

        let (ty, entries) = crate::read_ico_raw(&muxed).unwrap();
        assert_eq!(ty, IconType::Ico);
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[0].width, entries[0].height), (8, 8));
        assert_eq!((entries[1].width, entries[1].height), (16, 16));
        // Payload bytes survive the mux verbatim.
        assert_eq!(entries[0].data, packets[0].data);
        assert_eq!(entries[1].data, packets[1].data);
    }

    #[test]
    fn ico_muxer_rejects_empty_and_mismatched_packet_count() {
        let mut p = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
        p.width = Some(8);
        p.height = Some(8);
        let stream = StreamInfo {
            index: 0,
            params: p,
            time_base: TimeBase::new(1, 1),
            start_time: Some(0),
            duration: None,
        };
        // No packets written → write_trailer errors.
        let buf = Cursor::new(Vec::<u8>::new());
        let mut m = open_muxer(Box::new(buf), std::slice::from_ref(&stream)).unwrap();
        m.write_header().unwrap();
        match m.write_trailer() {
            Ok(()) => panic!("expected no-packets error"),
            Err(e) => assert!(e.to_string().contains("no packets")),
        }

        // One stream but two packets → count-mismatch error.
        let buf = Cursor::new(Vec::<u8>::new());
        let mut m = open_muxer(Box::new(buf), std::slice::from_ref(&stream)).unwrap();
        m.write_header().unwrap();
        let body = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        m.write_packet(&Packet::new(0, TimeBase::new(1, 1), body.clone()))
            .unwrap();
        m.write_packet(&Packet::new(0, TimeBase::new(1, 1), body))
            .unwrap();
        match m.write_trailer() {
            Ok(()) => panic!("expected count-mismatch error"),
            Err(e) => assert!(e.to_string().contains("packet count")),
        }
    }

    #[test]
    fn ico_demuxer_refuses_ani_input() {
        // A RIFF/ACON stream reaches the ICO demuxer (e.g. mis-probed by
        // extension) and must be refused cleanly via the delegated
        // parser's `.ani` recognition rather than a cryptic idType error.
        let red = ico_frame(8, [200, 10, 10, 255]);
        let ani = build_ani(&[red], 0, 12, None, None, None);
        match open_ico(&ani) {
            Ok(_) => panic!("expected .ani refusal, got a demuxer"),
            Err(e) => assert!(e.to_string().contains(".ani"), "{e}"),
        }
    }

    #[test]
    fn ani_probe_recognises_acon_magic() {
        let red = ico_frame(8, [200, 10, 10, 255]);
        let ani = build_ani(&[red], 0, 12, None, None, None);
        let score = probe_ani(&ProbeData {
            buf: &ani,
            ext: None,
        });
        assert_eq!(score, MAX_PROBE_SCORE);
    }

    #[test]
    fn ani_probe_extension_only_fallback() {
        let score = probe_ani(&ProbeData {
            buf: &[0u8; 4],
            ext: Some("ani"),
        });
        assert_eq!(score, oxideav_core::PROBE_SCORE_EXTENSION);
        let none = probe_ani(&ProbeData {
            buf: &[0u8; 4],
            ext: Some("ico"),
        });
        assert_eq!(none, 0);
    }

    #[test]
    fn ico_probe_does_not_claim_ani_magic() {
        // A RIFF/ACON header must not score full confidence on the ICO
        // probe — the two containers stay disjoint.
        let red = ico_frame(8, [1, 2, 3, 255]);
        let ani = build_ani(&[red], 0, 12, None, None, None);
        let ico_score = probe(&ProbeData {
            buf: &ani,
            ext: None,
        });
        assert_ne!(ico_score, MAX_PROBE_SCORE);
        let ani_score = probe_ani(&ProbeData {
            buf: &ani,
            ext: None,
        });
        assert_eq!(ani_score, MAX_PROBE_SCORE);
    }

    #[test]
    fn ani_demuxer_single_stream_identity_timeline() {
        let red = ico_frame(8, [200, 10, 10, 255]);
        let green = ico_frame(8, [10, 200, 10, 255]);
        let bodies = [red.clone(), green.clone()];
        let ani = build_ani(&bodies, 0, 12, None, None, None);

        let mut dx = open_ani(&ani);
        assert_eq!(dx.format_name(), "ani");
        // Exactly one video stream regardless of frame/step count.
        assert_eq!(dx.streams().len(), 1);
        let s = &dx.streams()[0];
        assert_eq!(s.params.media_type, MediaType::Video);
        assert_eq!(s.time_base, TimeBase::new(1, 60));
        // 2 identity steps × 12 jiffies = 24 jiffies of duration.
        assert_eq!(s.duration, Some(24));

        // Two packets in display order: frame 0 then frame 1, each
        // carrying that frame's raw ICO bytes, with jiffy-domain pts.
        let p0 = dx.next_packet().unwrap();
        assert_eq!(p0.stream_index, 0);
        assert_eq!(p0.pts, Some(0));
        assert_eq!(p0.duration, Some(12));
        assert!(p0.flags.keyframe);
        assert_eq!(p0.data, red);

        let p1 = dx.next_packet().unwrap();
        assert_eq!(p1.pts, Some(12));
        assert_eq!(p1.duration, Some(12));
        assert_eq!(p1.data, green);

        assert!(matches!(dx.next_packet(), Err(Error::Eof)));
    }

    #[test]
    fn ani_demuxer_seq_rate_drive_packet_timeline() {
        let a = ico_frame(8, [1, 2, 3, 255]);
        let b = ico_frame(8, [4, 5, 6, 255]);
        // 2 stored frames, 4 steps playing 0,1,1,0 with per-step rates.
        let ani = build_ani(
            &[a.clone(), b.clone()],
            4,
            10,
            Some(&[0, 1, 1, 0]),
            Some(&[5, 6, 7, 8]),
            Some((b"My Cursor", b"Me")),
        );

        let mut dx = open_ani(&ani);
        // Four packets — one per playback step, not per stored frame.
        let pkts: Vec<Packet> = std::iter::from_fn(|| dx.next_packet().ok()).collect();
        assert_eq!(pkts.len(), 4);

        // seq drives which frame body each step carries.
        let want_bodies = [&a, &b, &b, &a];
        let want_durs = [5i64, 6, 7, 8];
        let mut pts = 0i64;
        for (i, p) in pkts.iter().enumerate() {
            assert_eq!(&p.data, want_bodies[i], "step {i} frame body");
            assert_eq!(p.duration, Some(want_durs[i]), "step {i} duration");
            assert_eq!(p.pts, Some(pts), "step {i} pts");
            pts += want_durs[i];
        }
        // Cumulative timeline: 5+6+7+8 = 26 jiffies total.
        assert_eq!(pts, 26);
    }

    #[test]
    fn ani_demuxer_surfaces_info_and_duration() {
        let a = ico_frame(8, [1, 2, 3, 255]);
        let ani = build_ani(&[a], 0, 30, None, None, Some((b"Spin\0", b"Artist\0")));
        let dx = open_ani(&ani);
        let md: Vec<(&str, &str)> = dx
            .metadata()
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert!(md.contains(&("title", "Spin")));
        assert!(md.contains(&("artist", "Artist")));
        // One 30-jiffy step → 0.5 s → 500_000 µs.
        assert_eq!(dx.duration_micros(), Some(500_000));
    }

    /// Hand-assemble an `AF_ICON`-clear ANI (raw headerless BMP frames)
    /// with geometry in `anih`.
    fn build_raw_ani(
        frames: &[Vec<u8>],
        width: u32,
        height: u32,
        bit_count: u32,
        i_disp_rate: u32,
    ) -> Vec<u8> {
        let mut anih = [0u8; 36];
        let put =
            |b: &mut [u8; 36], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut anih, 0, 36);
        put(&mut anih, 4, frames.len() as u32);
        put(&mut anih, 12, width);
        put(&mut anih, 16, height);
        put(&mut anih, 20, bit_count);
        put(&mut anih, 24, 1);
        put(&mut anih, 28, i_disp_rate);
        put(&mut anih, 32, 0); // AF_ICON clear

        let mut body = Vec::new();
        body.extend_from_slice(b"ACON");
        push_chunk(&mut body, b"anih", &anih);
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
    fn ani_demuxer_carries_af_icon_clear_raw_frames_with_anih_geometry() {
        // AF_ICON clear: two raw 4×4 32-bpp BMP bodies (no header). The
        // demuxer passes the raw bytes through and surfaces the anih
        // geometry on the stream params (the raw bodies are headerless,
        // so the geometry has nowhere else to live).
        let f0: Vec<u8> = std::iter::repeat([0u8, 0, 200, 255])
            .take(16)
            .flatten()
            .collect();
        let f1: Vec<u8> = std::iter::repeat([0u8, 200, 0, 255])
            .take(16)
            .flatten()
            .collect();
        let ani = build_raw_ani(&[f0.clone(), f1.clone()], 4, 4, 32, 7);

        let mut dx = open_ani(&ani);
        let s = &dx.streams()[0];
        // anih geometry is authoritative for the raw path → surfaced,
        // including the bit-depth a consumer needs to decode the
        // headerless bodies.
        assert_eq!(s.params.width, Some(4));
        assert_eq!(s.params.height, Some(4));
        // Bit-depth surfaced as a 4-byte LE extradata block.
        assert_eq!(s.params.extradata, 32u32.to_le_bytes().to_vec());

        let p0 = dx.next_packet().unwrap();
        assert_eq!(p0.data, f0, "raw frame 0 bytes pass through verbatim");
        assert_eq!(p0.duration, Some(7));
        let p1 = dx.next_packet().unwrap();
        assert_eq!(p1.data, f1, "raw frame 1 bytes pass through verbatim");
        assert!(matches!(dx.next_packet(), Err(Error::Eof)));
    }

    #[test]
    fn ani_demuxer_omits_unset_anih_geometry() {
        // AF_ICON set with anih width/height = 0 (the "take from frame"
        // sentinel): the demuxer leaves stream width/height as None
        // rather than claiming a bogus 0×0.
        let a = ico_frame(8, [1, 2, 3, 255]);
        let ani = build_ani(&[a], 0, 12, None, None, None);
        let dx = open_ani(&ani);
        let s = &dx.streams()[0];
        assert_eq!(s.params.width, None);
        assert_eq!(s.params.height, None);
        // iBitCount is the "take from frame" sentinel on the AF_ICON
        // path too — the per-frame directory is authoritative, so the
        // stream doesn't fabricate a bit-depth (no extradata block).
        assert!(s.params.extradata.is_empty());
    }

    #[test]
    fn ani_demuxer_rejects_non_ani_input() {
        let ico = ico_frame(8, [9, 9, 9, 255]);
        let err = open_ani_demuxer(Box::new(Cursor::new(ico)), &NullCodecResolver);
        assert!(err.is_err());
    }
}
