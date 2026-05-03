#![no_main]

//! Self-roundtrip fuzz harness for the `oxideav-ico` codec.
//!
//! Drives the trait-based [`make_encoder`] / [`make_decoder`] path:
//! synthesise an RGBA [`VideoFrame`], push it through the encoder to
//! get a packet (one PNG or BMP-DIB sub-image — whichever the encoder
//! chose), feed that packet's bytes back into a fresh decoder, and
//! assert the round-tripped pixels are byte-identical.
//!
//! ICO is a container that wraps BMP or PNG sub-images, but the
//! `"ico"` codec id operates on a single sub-image at a time (one
//! `Packet` = one entry in the icon directory). That's the unit the
//! decoder consumes when the container demuxer hands it work, so it's
//! the right surface to fuzz here.
//!
//! No external library oracle: ICO has no canonical system decoder
//! worth pulling in (libgdiplus etc.). Self-roundtrip catches encoder
//! bugs that produce corrupt sub-images and decoder bugs that
//! mis-parse legitimate output — the cross-product is what differential
//! cross-decode would also have caught for these two halves.

use libfuzzer_sys::fuzz_target;
use oxideav_core::{
    CodecId, CodecParameters, Frame, Packet, PixelFormat, TimeBase, VideoFrame, VideoPlane,
};
use oxideav_ico::codec::{make_decoder, make_encoder};

const MAX_WIDTH: usize = 64;
const MAX_PIXELS: usize = 2048;

fuzz_target!(|data: &[u8]| {
    let Some((width, height, rgba)) = image_from_fuzz_input(data) else {
        return;
    };

    // Build CodecParameters mirroring what a muxer would hand the
    // encoder factory: video, RGBA, fixed dimensions.
    let mut enc_params = CodecParameters::video(CodecId::new(oxideav_ico::CODEC_ID_STR));
    enc_params.width = Some(width);
    enc_params.height = Some(height);
    enc_params.pixel_format = Some(PixelFormat::Rgba);

    let frame = Frame::Video(VideoFrame {
        pts: None,
        planes: vec![VideoPlane {
            stride: (width as usize) * 4,
            data: rgba.to_vec(),
        }],
    });

    let mut encoder = make_encoder(&enc_params).expect("ICO encoder construction failed");
    encoder
        .send_frame(&frame)
        .expect("ICO encoder send_frame failed");
    let packet = encoder
        .receive_packet()
        .expect("ICO encoder receive_packet failed");

    // Decoder takes the same single-sub-image bytes back. Pass through
    // a fresh packet (drop the encoder's `pts == 0` so the assertion
    // surface stays focused on pixel equality, not pts plumbing).
    let dec_params = CodecParameters::video(CodecId::new(oxideav_ico::CODEC_ID_STR));
    let mut decoder = make_decoder(&dec_params).expect("ICO decoder construction failed");

    let in_packet = Packet::new(0, TimeBase::new(1, 1), packet.data);
    decoder
        .send_packet(&in_packet)
        .expect("ICO decoder send_packet failed");
    let decoded = match decoder
        .receive_frame()
        .expect("ICO decoder receive_frame failed")
    {
        Frame::Video(v) => v,
        other => panic!("ICO decoder returned non-video frame: {other:?}"),
    };

    assert_eq!(
        decoded.planes.len(),
        1,
        "ICO decoder must emit a single RGBA plane"
    );
    let plane = &decoded.planes[0];
    let expected_stride = (width as usize) * 4;
    assert_eq!(
        plane.stride, expected_stride,
        "ICO decoder stride mismatch: got {} expected {}",
        plane.stride, expected_stride
    );
    assert_eq!(
        plane.data.len(),
        (width as usize) * (height as usize) * 4,
        "ICO decoder pixel buffer wrong length"
    );
    assert_eq!(
        plane.data.as_slice(),
        rgba,
        "ICO self-roundtrip pixel mismatch ({width}×{height})"
    );
});

fn image_from_fuzz_input(data: &[u8]) -> Option<(u32, u32, &[u8])> {
    let (&shape, rgba) = data.split_first()?;

    let pixel_count = (rgba.len() / 4).min(MAX_PIXELS);
    if pixel_count == 0 {
        return None;
    }

    let width = ((shape as usize) % MAX_WIDTH) + 1;
    let width = width.min(pixel_count);
    let height = pixel_count / width;
    if height == 0 {
        return None;
    }
    // ICO directory entries store width/height in u8 (0 means 256), so
    // the on-disk format physically can't carry > 256. We're safely
    // under that with MAX_WIDTH=64 and MAX_PIXELS=2048 (max height
    // 2048/1 = 2048 → clamp).
    if height > 256 {
        return None;
    }
    let used_len = width * height * 4;
    let rgba = &rgba[..used_len];

    Some((width as u32, height as u32, rgba))
}
