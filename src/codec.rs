//! Framework `Decoder` / `Encoder` adapters for the `"ico"` codec id —
//! one ICO / CUR sub-image payload per packet.
//!
//! The `"ico"` demuxer emits one packet per directory entry carrying the
//! entry's raw payload (an embedded PNG or a doubled-height DIB with its
//! AND mask); the decoder here turns each into an `Rgba` `VideoFrame`
//! through the standalone [`crate::decode_payload`], and the encoder
//! turns one `VideoFrame` into one payload through
//! [`crate::encode_entry`] — a thin adapter, one implementation.

use oxideav_core::{
    parse_options, CodecId, CodecParameters, Error, Frame, Packet, PixelFormat, Result, TimeBase,
    VideoFrame,
};
use oxideav_core::{Decoder, Encoder};

use crate::image::IcoImage;
use crate::options::{DecodeOptions, EncodeOptions};

/// Factory registered with the codec registry: one packet (a sub-image
/// payload) becomes one `Rgba` frame. Sub-images are independent, so
/// `flush()` just drains the pending frame.
pub fn make_decoder(_params: &CodecParameters) -> Result<Box<dyn Decoder>> {
    Ok(Box::new(IcoDecoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        pending: None,
        eof: false,
    }))
}

/// Factory registered with the codec registry: one `VideoFrame`
/// (`Rgba` / `Rgb24` / `Bgra` / `Bgr24` per
/// `CodecParameters::pixel_format`, default `Rgba`) becomes one
/// sub-image payload packet. `CodecParameters::options` is parsed as
/// [`EncodeOptions`] (`png_size_threshold`, `bmp_bit_depth`,
/// `per_image_bit_depth`, `embed_metadata`, `icon_type`).
pub fn make_encoder(params: &CodecParameters) -> Result<Box<dyn Encoder>> {
    let opts = parse_options::<EncodeOptions>(&params.options)?;
    let mut out_params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
    out_params.width = params.width;
    out_params.height = params.height;
    out_params.pixel_format = params.pixel_format.or(Some(PixelFormat::Rgba));
    Ok(Box::new(IcoEncoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        out_params,
        opts,
        pending: None,
        eof: false,
    }))
}

/// ICO sub-image `Decoder` (see [`make_decoder`]).
pub struct IcoDecoder {
    codec_id: CodecId,
    pending: Option<VideoFrame>,
    eof: bool,
}

impl Decoder for IcoDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        let image = crate::decode_payload(&packet.data, &DecodeOptions::default())?;
        let mut frame = VideoFrame::from(image);
        frame.pts = packet.pts;
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

/// ICO sub-image `Encoder` (see [`make_encoder`]).
pub struct IcoEncoder {
    codec_id: CodecId,
    out_params: CodecParameters,
    opts: EncodeOptions,
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
        let image = IcoImage::from_video_frame(vf, &self.out_params)?;
        let entry = crate::encode_entry(&image, &self.opts)?;
        self.pending = Some(entry.data);
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

#[cfg(test)]
mod tests {
    use super::*;
    use oxideav_core::{MediaType, VectorFrame, VideoPlane};

    const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

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
        let enc = make_encoder(&p).unwrap();
        let out = enc.output_params();
        assert_eq!(out.media_type, MediaType::Video);
        assert_eq!(out.width, Some(16));
        assert_eq!(out.height, Some(16));
        assert_eq!(out.pixel_format, Some(PixelFormat::Rgba));
        assert_eq!(out.codec_id.as_str(), crate::CODEC_ID_STR);
    }

    #[test]
    fn encoder_round_trips_dib_sub_image_through_decoder() {
        let w = 16;
        let h = 16;
        let src = rgba_frame(w, h, [200, 40, 10, 255]);

        let mut enc = make_encoder(&params(w, h)).unwrap();
        enc.send_frame(&Frame::Video(src.clone())).unwrap();
        let pkt = enc.receive_packet().unwrap();
        assert!(pkt.flags.keyframe);
        assert_ne!(&pkt.data[..PNG_MAGIC.len()], &PNG_MAGIC);

        let mut dec = make_decoder(&params(w, h)).unwrap();
        dec.send_packet(&pkt).unwrap();
        let frame = dec.receive_frame().unwrap();
        let vf = match frame {
            Frame::Video(v) => v,
            _ => panic!("expected a video frame"),
        };
        let stride = vf.planes[0].stride;
        for y in 0..h as usize {
            let row = &vf.planes[0].data[y * stride..y * stride + (w as usize) * 4];
            let exp = &src.planes[0].data[y * (w as usize) * 4..(y + 1) * (w as usize) * 4];
            assert_eq!(row, exp, "row {y} must round-trip exactly");
        }
        // A classic DIB carries no colour signalling: nothing is stamped.
        assert!(vf.color_signal().is_none());
    }

    #[test]
    fn encoder_emits_png_body_at_or_above_threshold() {
        // The default threshold is 256; a 64 px frame is a DIB by default
        // and a PNG once the option lowers the threshold.
        let w = 64;
        let h = 64;
        let mut enc = make_encoder(&params(w, h)).unwrap();
        enc.send_frame(&Frame::Video(rgba_frame(w, h, [10, 20, 30, 255])))
            .unwrap();
        let pkt = enc.receive_packet().unwrap();
        assert_ne!(
            &pkt.data[..PNG_MAGIC.len()],
            &PNG_MAGIC,
            "64 px is a DIB by default"
        );

        let mut p = params(w, h);
        p.options.insert("png_size_threshold", "64");
        let mut enc = make_encoder(&p).unwrap();
        enc.send_frame(&Frame::Video(rgba_frame(w, h, [10, 20, 30, 255])))
            .unwrap();
        let pkt = enc.receive_packet().unwrap();
        assert_eq!(&pkt.data[..PNG_MAGIC.len()], &PNG_MAGIC);

        let mut dec = make_decoder(&params(w, h)).unwrap();
        dec.send_packet(&pkt).unwrap();
        assert!(matches!(dec.receive_frame().unwrap(), Frame::Video(_)));
    }

    #[test]
    fn encoder_options_select_dib_depth_and_reject_unknown_keys() {
        let mut p = params(4, 4);
        p.options.insert("bmp_bit_depth", "indexed8");
        let mut enc = make_encoder(&p).unwrap();
        enc.send_frame(&Frame::Video(rgba_frame(4, 4, [1, 2, 3, 255])))
            .unwrap();
        let pkt = enc.receive_packet().unwrap();
        // biBitCount at DIB offset 14.
        assert_eq!(u16::from_le_bytes([pkt.data[14], pkt.data[15]]), 8);

        let mut bad = params(4, 4);
        bad.options.insert("no_such_option", "1");
        assert!(make_encoder(&bad).is_err());
        let mut bad = params(4, 4);
        bad.options.insert("bmp_bit_depth", "indexed3");
        assert!(make_encoder(&bad).is_err());
    }

    #[test]
    fn decoder_signals_need_more_then_eof() {
        let mut dec = make_decoder(&params(8, 8)).unwrap();
        assert!(matches!(dec.receive_frame(), Err(Error::NeedMore)));
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
        let err = enc
            .send_frame(&Frame::Vector(VectorFrame::default()))
            .unwrap_err();
        assert!(err.to_string().contains("video frame"));
    }

    #[test]
    fn decoder_rejects_garbage_payload() {
        let mut dec = make_decoder(&params(8, 8)).unwrap();
        let pkt = Packet::new(0, TimeBase::new(1, 1), vec![0u8; 20]);
        assert!(dec.send_packet(&pkt).is_err());
    }
}
