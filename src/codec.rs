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

use oxideav_core::{
    CodecId, CodecParameters, Error, Frame, Packet, PixelFormat, Result, TimeBase, VideoFrame,
};
use oxideav_core::{Decoder, Encoder};

const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

pub fn make_decoder(_params: &CodecParameters) -> Result<Box<dyn Decoder>> {
    Ok(Box::new(IcoDecoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        pending: None,
        eof: false,
    }))
}

pub fn make_encoder(params: &CodecParameters) -> Result<Box<dyn Encoder>> {
    let mut out_params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
    out_params.width = params.width;
    out_params.height = params.height;
    out_params.pixel_format = params.pixel_format.or(Some(PixelFormat::Rgba));
    Ok(Box::new(IcoEncoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        out_params,
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
        let width = self
            .out_params
            .width
            .ok_or_else(|| Error::invalid("ICO encoder: missing width in CodecParameters"))?;
        let height = self
            .out_params
            .height
            .ok_or_else(|| Error::invalid("ICO encoder: missing height in CodecParameters"))?;
        // Default to PNG for large sub-images, BMP for small — mirrors
        // the standalone `WriteOptions::default()` heuristic.
        let use_png = width.min(height) >= 64;
        let bytes = if use_png {
            oxideav_png::encode_single(vf, width, height, PixelFormat::Rgba, &[])?
        } else {
            oxideav_bmp::encode_dib_videoframe(
                vf,
                PixelFormat::Rgba,
                width,
                height,
                /* doubled */ true,
            )?
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
        let mut f = oxideav_png::decode_png_to_frame(payload, pts)?;
        // PNG is typically RGBA already; normalise for downstream so
        // the rest of the pipeline can count on a stable format.
        f.pts = pts;
        Ok(f)
    } else {
        let mut f = oxideav_bmp::decode_dib_videoframe(payload, /* doubled */ true)?;
        f.pts = pts;
        Ok(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxideav_core::{MediaType, VectorFrame, VideoPlane};

    /// A solid-colour RGBA `VideoFrame` (`stride = w * 4`, top-down).
    fn rgba_frame(w: u32, h: u32, rgba: [u8; 4]) -> VideoFrame {
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            data.extend_from_slice(&rgba);
        }
        VideoFrame {
            pts: None,
            planes: vec![VideoPlane {
                stride: (w * 4) as usize,
                data,
            }],
        }
    }

    fn params(w: u32, h: u32) -> CodecParameters {
        let mut p = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
        p.width = Some(w);
        p.height = Some(h);
        p.pixel_format = Some(PixelFormat::Rgba);
        p
    }

    #[test]
    fn make_encoder_defaults_pixel_format_and_carries_dims() {
        let mut p = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
        p.width = Some(16);
        p.height = Some(16);
        // pixel_format left None — the encoder must default it to Rgba.
        let enc = make_encoder(&p).unwrap();
        let out = enc.output_params();
        assert_eq!(out.media_type, MediaType::Video);
        assert_eq!(out.width, Some(16));
        assert_eq!(out.height, Some(16));
        assert_eq!(out.pixel_format, Some(PixelFormat::Rgba));
        assert_eq!(out.codec_id.as_str(), crate::CODEC_ID_STR);
    }

    #[test]
    fn encoder_round_trips_bmp_sub_image_through_decoder() {
        // 16×16 is below the encoder's PNG threshold (64), so it emits a
        // BMP-DIB body. Encode a frame → packet → decode → frame, and
        // confirm the RGBA pixels survive the BMP-inside-ICO path exactly
        // (BMP is lossless, unlike a potential PNG colour conversion).
        let w = 16;
        let h = 16;
        let src = rgba_frame(w, h, [200, 40, 10, 255]);

        let mut enc = make_encoder(&params(w, h)).unwrap();
        enc.send_frame(&Frame::Video(src.clone())).unwrap();
        let pkt = enc.receive_packet().unwrap();
        assert!(pkt.flags.keyframe);
        // BMP body, not PNG (no PNG magic at the front).
        assert_ne!(&pkt.data[..PNG_MAGIC.len()], &PNG_MAGIC);

        let mut dec = make_decoder(&params(w, h)).unwrap();
        dec.send_packet(&pkt).unwrap();
        let frame = dec.receive_frame().unwrap();
        let vf = match frame {
            Frame::Video(v) => v,
            _ => panic!("expected a video frame"),
        };
        // Compare the decoded top-down RGBA rows against the source.
        let stride = vf.planes[0].stride;
        for y in 0..h as usize {
            let row = &vf.planes[0].data[y * stride..y * stride + (w as usize) * 4];
            let exp = &src.planes[0].data[y * (w as usize) * 4..(y + 1) * (w as usize) * 4];
            assert_eq!(row, exp, "row {y} must round-trip exactly");
        }
    }

    #[test]
    fn encoder_emits_png_body_at_or_above_threshold() {
        // 64×64 is at the PNG threshold → a PNG body (PNG magic present).
        let w = 64;
        let h = 64;
        let mut enc = make_encoder(&params(w, h)).unwrap();
        enc.send_frame(&Frame::Video(rgba_frame(w, h, [10, 20, 30, 255])))
            .unwrap();
        let pkt = enc.receive_packet().unwrap();
        assert_eq!(&pkt.data[..PNG_MAGIC.len()], &PNG_MAGIC);

        // …and the decoder accepts it back (PNG branch of the sniff).
        let mut dec = make_decoder(&params(w, h)).unwrap();
        dec.send_packet(&pkt).unwrap();
        assert!(matches!(dec.receive_frame().unwrap(), Frame::Video(_)));
    }

    #[test]
    fn decoder_signals_need_more_then_eof() {
        let mut dec = make_decoder(&params(8, 8)).unwrap();
        // No packet sent yet → NeedMore, not Eof.
        assert!(matches!(dec.receive_frame(), Err(Error::NeedMore)));
        // After flush with nothing pending → Eof.
        dec.flush().unwrap();
        assert!(matches!(dec.receive_frame(), Err(Error::Eof)));
    }

    #[test]
    fn encoder_signals_need_more_then_eof() {
        let mut enc = make_encoder(&params(8, 8)).unwrap();
        assert!(matches!(enc.receive_packet(), Err(Error::NeedMore)));
        enc.flush().unwrap();
        assert!(matches!(enc.receive_packet(), Err(Error::Eof)));
    }

    #[test]
    fn encoder_rejects_missing_width() {
        // No width in params → send_frame errors rather than panicking.
        let mut p = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
        p.height = Some(8);
        let mut enc = make_encoder(&p).unwrap();
        let err = enc
            .send_frame(&Frame::Video(rgba_frame(8, 8, [1, 2, 3, 255])))
            .unwrap_err();
        assert!(err.to_string().contains("width"));
    }

    #[test]
    fn encoder_rejects_non_video_frame() {
        let mut enc = make_encoder(&params(8, 8)).unwrap();
        // A vector frame is the wrong shape for the ICO encoder.
        let err = enc
            .send_frame(&Frame::Vector(VectorFrame::default()))
            .unwrap_err();
        assert!(err.to_string().contains("video frame"));
    }
}
