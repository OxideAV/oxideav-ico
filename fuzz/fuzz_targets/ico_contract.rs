#![no_main]

//! Contract-surface fuzz harness: arbitrary bytes through the standalone
//! image-crate API (`probe` / `info` / `decode` / `decode_all` /
//! `decode_rgba8` / `decode_rgb8`), with no `oxideav-core` involved.
//!
//! Invariants checked on every input:
//!
//! * nothing panics, whatever the bytes;
//! * `probe` is total and `info` / `decode` agree on the primary entry's
//!   geometry whenever both succeed;
//! * every decoded sub-image is a tightly packed `Rgba` plane of exactly
//!   `4 × width × height` bytes with `1..=256` dimensions, and
//!   `to_rgba8()` / `to_rgb8()` have the matching lengths;
//! * `decode_all` yields exactly `info().frames` frames with ascending
//!   `index`;
//! * an accepted file re-encodes (`encode_all`, DIB everywhere, per-image
//!   depth) and decodes back to the same pixels — the lossless pin on
//!   fuzz-discovered inputs;
//! * tight `DecodeOptions` limits fail with `LimitExceeded`, never a
//!   panic or an oversized allocation.

use libfuzzer_sys::fuzz_target;
use oxideav_ico::{
    decode, decode_all, decode_all_with, decode_rgb8, decode_rgba8, decode_with, encode_all, info,
    probe, DecodeOptions, EncodeOptions, EntrySelection, IcoError, PixelFormat,
};

fuzz_target!(|data: &[u8]| {
    let _ = probe(data);

    let info = match info(data) {
        Ok(i) => i,
        Err(_) => {
            // A directory the walker rejects must be rejected everywhere.
            assert!(decode(data).is_err());
            assert!(decode_all(data).is_err());
            return;
        }
    };
    assert!(info.frames >= 1);
    assert_eq!(info.entries.len() as u32, info.frames);
    assert!((1..=256).contains(&info.width) && (1..=256).contains(&info.height));

    // Tight limits: never a panic, always LimitExceeded when they bite.
    let tight = DecodeOptions::new().with_max_width(1).with_max_height(1);
    if info.width > 1 || info.height > 1 {
        assert!(matches!(
            decode_with(data, &tight),
            Err(IcoError::LimitExceeded(_))
        ));
    }
    let _ = decode_all_with(data, &DecodeOptions::new().with_max_bytes(16));

    let frames = match decode_all(data) {
        Ok(f) => f,
        Err(_) => {
            // Payload corruption: the primary may still decode or not,
            // but it must not panic.
            let _ = decode(data);
            let _ = decode_rgba8(data);
            return;
        }
    };
    assert_eq!(frames.len() as u32, info.frames);
    for (i, f) in frames.iter().enumerate() {
        assert_eq!(f.index as usize, i);
        assert!(f.delay.is_none());
        let im = &f.image;
        assert_eq!(im.format, PixelFormat::Rgba);
        assert_eq!(im.planes.len(), 1);
        let n = im.width as usize * im.height as usize;
        assert_eq!(im.planes[0].stride, im.width as usize * 4);
        assert_eq!(im.planes[0].data.len(), n * 4);
        assert_eq!(im.to_rgba8().len(), n * 4);
        assert_eq!(im.to_rgb8().len(), n * 3);
        let row = &info.entries[i];
        assert_eq!((row.width, row.height), (im.width, im.height));
        assert_eq!(row.hotspot, im.hotspot);
    }

    let primary = decode(data).expect("decode_all succeeded, so decode must");
    assert_eq!((primary.width, primary.height), (info.width, info.height));
    assert_eq!(primary.planes, frames[info.primary as usize].image.planes);
    let rgba = decode_rgba8(data).unwrap();
    assert_eq!(rgba.data, primary.to_rgba8());
    let rgb = decode_rgb8(data).unwrap();
    assert_eq!(rgb.data.len(), rgba.data.len() / 4 * 3);
    let by_index = decode_with(
        data,
        &DecodeOptions::new().with_entry(EntrySelection::Index(info.primary)),
    )
    .unwrap();
    assert_eq!(by_index.planes, primary.planes);

    // Lossless re-encode of everything the decoder accepted: 32-bpp DIBs
    // keep every RGBA byte (colour under alpha 0 included).
    let opts = EncodeOptions::new()
        .with_icon_type(info.icon_type)
        .with_png_size_threshold(None);
    let bytes = encode_all(&frames, &opts).expect("re-encode of a decoded icon");
    let again = decode_all(&bytes).expect("re-decode of our own output");
    assert_eq!(again.len(), frames.len());
    for (a, b) in again.iter().zip(&frames) {
        assert_eq!(a.image.planes, b.image.planes, "pixels survive re-encode");
        assert_eq!(a.image.hotspot, b.image.hotspot);
    }

    // Faithful-depth re-encode: an indexed / 24-bpp DIB cannot keep the
    // colour of a fully transparent pixel (the AND mask hides it), so
    // compare alpha everywhere and colour only where alpha != 0.
    let bytes = encode_all(&frames, &opts.with_per_image_bit_depth(true))
        .expect("per-depth re-encode of a decoded icon");
    let again = decode_all(&bytes).expect("re-decode of per-depth output");
    for (a, b) in again.iter().zip(&frames) {
        let (pa, pb) = (a.image.to_rgba8(), b.image.to_rgba8());
        assert_eq!(pa.len(), pb.len());
        for (x, y) in pa.chunks_exact(4).zip(pb.chunks_exact(4)) {
            assert_eq!(x[3], y[3], "alpha survives per-depth re-encode");
            if y[3] != 0 {
                assert_eq!(x, y, "opaque colour survives per-depth re-encode");
            }
        }
    }
});
