//! ICO / CUR container: one [`Packet`] per directory entry. Each
//! packet's `data` is the raw sub-image bytes (PNG or BMP DIB) exactly
//! as they appear in the file, so downstream consumers (the ICO
//! codec or any PNG / BMP codec that can probe the magic) can decode
//! them directly.
//!
//! The demuxer exposes one `StreamInfo` per sub-image. `pts = index`
//! (0-based directory order); `width` / `height` carry the
//! `ICONDIRENTRY`-declared dimensions. Hotspots from CUR entries are
//! surfaced in `StreamInfo::params.extradata` as a 4-byte `u16 x`,
//! `u16 y` little-endian pair (empty for ICO entries).

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
    if buf.len() < 6 {
        return Err(Error::invalid("ICO: file shorter than ICONDIR"));
    }
    let id_type = u16::from_le_bytes([buf[2], buf[3]]);
    if !(id_type == 1 || id_type == 2) {
        return Err(Error::invalid(format!("ICO: unknown idType {id_type}")));
    }
    let count = u16::from_le_bytes([buf[4], buf[5]]) as usize;
    let dir_end = 6 + count * 16;
    if buf.len() < dir_end {
        return Err(Error::invalid("ICO: directory truncated"));
    }

    let mut streams = Vec::with_capacity(count);
    let mut packets = Vec::with_capacity(count);
    for i in 0..count {
        let e = &buf[6 + i * 16..6 + i * 16 + 16];
        let declared_w = if e[0] == 0 { 256 } else { e[0] as u32 };
        let declared_h = if e[1] == 0 { 256 } else { e[1] as u32 };
        let planes_or_hotx = u16::from_le_bytes([e[4], e[5]]);
        let bits_or_hoty = u16::from_le_bytes([e[6], e[7]]);
        let data_size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let data_offset = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
        if buf.len() < data_offset.saturating_add(data_size) {
            return Err(Error::invalid(format!("ICO: entry {i} payload OOB")));
        }

        let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
        params.width = Some(declared_w);
        params.height = Some(declared_h);
        params.pixel_format = Some(PixelFormat::Rgba);
        if id_type == 2 {
            // CUR — surface the hotspot in extradata so callers that
            // need it don't have to re-parse the directory.
            let mut ed = Vec::with_capacity(4);
            ed.extend_from_slice(&planes_or_hotx.to_le_bytes());
            ed.extend_from_slice(&bits_or_hoty.to_le_bytes());
            params.extradata = ed;
        }
        streams.push(StreamInfo {
            index: i as u32,
            params,
            time_base: TimeBase::new(1, 1),
            start_time: Some(0),
            duration: None,
        });

        let payload = buf[data_offset..data_offset + data_size].to_vec();
        let mut pkt = Packet::new(i as u32, TimeBase::new(1, 1), payload);
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
