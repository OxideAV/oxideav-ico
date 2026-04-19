//! `Decoder` / `Encoder` implementations for the `"ico"` codec id.
//!
//! A packet hands the decoder one sub-image (PNG or BMP-DIB) exactly
//! as it appears inside the containing `.ico` / `.cur`, and the
//! decoder returns an RGBA [`VideoFrame`]. Each packet maps to one
//! frame — ICO sub-images are intra-only.
//!
//! The encoder accepts an RGBA [`VideoFrame`] and produces either a
//! PNG or a BMP DIB (with the doubled-height + AND-mask layout), so
//! the muxer can splice the bytes straight into a file.

use oxideav_codec::{Decoder, Encoder};
use oxideav_core::{
    CodecId, CodecParameters, Error, Frame, Packet, PixelFormat, Result, TimeBase, VideoFrame,
};

const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

pub fn make_decoder(_params: &CodecParameters) -> Result<Box<dyn Decoder>> {
    Ok(Box::new(IcoDecoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        pending: None,
        eof: false,
    }))
}

pub fn make_encoder(_params: &CodecParameters) -> Result<Box<dyn Encoder>> {
    Ok(Box::new(IcoEncoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        out_params: CodecParameters::video(CodecId::new(crate::CODEC_ID_STR)),
        pending: None,
        eof: false,
    }))
}

struct IcoDecoder {
    codec_id: CodecId,
    pending: Option<VideoFrame>,
    eof: bool,
}

impl Decoder for IcoDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        let frame = decode_sub_image_bytes(&packet.data, packet.pts)?;
        self.pending = Some(frame);
        Ok(())
    }
    fn receive_frame(&mut self) -> Result<Frame> {
        match self.pending.take() {
            Some(f) => Ok(Frame::Video(f)),
            None => {
                if self.eof {
                    Err(Error::Eof)
                } else {
                    Err(Error::NeedMore)
                }
            }
        }
    }
    fn flush(&mut self) -> Result<()> {
        self.eof = true;
        Ok(())
    }
}

struct IcoEncoder {
    codec_id: CodecId,
    out_params: CodecParameters,
    pending: Option<Vec<u8>>,
    eof: bool,
}

impl Encoder for IcoEncoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn output_params(&self) -> &CodecParameters {
        &self.out_params
    }
    fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        let vf = match frame {
            Frame::Video(v) => v,
            _ => return Err(Error::invalid("ICO encoder: expected video frame")),
        };
        // Default to PNG for large sub-images, BMP for small — mirrors
        // the standalone `WriteOptions::default()` heuristic.
        let use_png = vf.width.min(vf.height) >= 64;
        let bytes = if use_png {
            oxideav_png::encoder::encode_single(vf, PixelFormat::Rgba, &[])?
        } else {
            oxideav_bmp::encode_dib(vf, /* doubled */ true)?
        };
        self.pending = Some(bytes);
        Ok(())
    }
    fn receive_packet(&mut self) -> Result<Packet> {
        match self.pending.take() {
            Some(bytes) => {
                let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes);
                pkt.flags.keyframe = true;
                Ok(pkt)
            }
            None => {
                if self.eof {
                    Err(Error::Eof)
                } else {
                    Err(Error::NeedMore)
                }
            }
        }
    }
    fn flush(&mut self) -> Result<()> {
        self.eof = true;
        Ok(())
    }
}

pub(crate) fn decode_sub_image_bytes(payload: &[u8], pts: Option<i64>) -> Result<VideoFrame> {
    if payload.len() >= PNG_MAGIC.len() && payload[..PNG_MAGIC.len()] == PNG_MAGIC {
        let mut f = oxideav_png::decoder::decode_png_to_frame(payload, pts, TimeBase::new(1, 1))?;
        // PNG is typically RGBA already; normalise for downstream so
        // the rest of the pipeline can count on a stable format.
        f.pts = pts;
        Ok(f)
    } else {
        let mut f = oxideav_bmp::decode_dib(payload, /* doubled */ true)?;
        f.pts = pts;
        Ok(f)
    }
}
