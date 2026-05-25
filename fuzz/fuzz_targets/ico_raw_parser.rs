#![no_main]

//! Panic-free fuzz harness for the standalone ICO / CUR parser.
//!
//! Drives [`read_ico_raw`] on arbitrary fuzz bytes (no codec / PNG /
//! BMP-DIB decode involved) — the lowest-level directory walker is
//! the most likely place for adversarial input to trigger an
//! arithmetic overflow, an OOB index, or a divergence between the
//! validator and the actual layout. On success, the harness also
//! round-trips through [`write_ico_raw`] and re-parses the output to
//! assert the writer's emit matches what we just parsed (no
//! information loss across read → write → read).
//!
//! Why this exists separately from `ico_self_roundtrip`:
//!
//! * `ico_self_roundtrip` covers the *codec* path (BMP / PNG encode +
//!   decode). It only exercises bit-streams the encoder generates —
//!   nothing adversarial.
//! * `ico_raw_parser` covers the *container* path on **arbitrary**
//!   bytes — every directory entry validation path, the RIFF/ACON
//!   detection, the payload-overlap detector, the planes/bit_count
//!   range checks, the offset-arithmetic overflow guards. This is
//!   where icon parsers historically take CVE hits.
//!
//! Wall: standalone `read_ico_raw` / `write_ico_raw` only — no
//! `oxideav-bmp` / `oxideav-png` / `oxideav-core` reachable from this
//! harness, so a panic anywhere here is a bug in the directory
//! walker itself.

use libfuzzer_sys::fuzz_target;
use oxideav_ico::{read_ico_raw, write_ico_raw, IconType};

fuzz_target!(|data: &[u8]| {
    // First pass: arbitrary bytes → parser. We expect Err for
    // virtually everything (the input is rarely a valid ICO), but
    // **never** a panic / overflow / OOB index. That's the property
    // under test.
    let Ok((icon_type, entries)) = read_ico_raw(data) else {
        return;
    };

    // The parser accepted the input. A few cheap invariants the
    // parser itself guarantees — assert them so a future regression
    // that silently weakens the validator gets caught here.
    assert!(
        matches!(icon_type, IconType::Ico | IconType::Cur),
        "read_ico_raw returned an icon type outside the {{Ico, Cur}} set: {icon_type:?}"
    );
    assert!(
        !entries.is_empty(),
        "read_ico_raw accepted a zero-entry file"
    );
    for (i, e) in entries.iter().enumerate() {
        // PNG IHDR or DIB header recovery, with the directory's
        // `0 → 256` fallback. `width = 0` would mean both the body
        // and the directory disagreed and the recovery fell through —
        // shouldn't happen with the current parser.
        assert!(
            e.width != 0 && e.width <= 256,
            "entry {i} parsed width {} outside (0, 256]",
            e.width
        );
        assert!(
            e.height != 0 && e.height <= 256,
            "entry {i} parsed height {} outside (0, 256]",
            e.height
        );
        if icon_type == IconType::Cur {
            // CUR hotspots must lie inside the sub-image — the
            // parser enforces this on read.
            if let Some(h) = e.hotspot {
                assert!(
                    (h.x as u32) < e.width.max(1) && (h.y as u32) < e.height.max(1),
                    "entry {i} CUR hotspot ({},{}) outside {}×{}",
                    h.x,
                    h.y,
                    e.width,
                    e.height
                );
            }
        } else {
            assert!(
                e.hotspot.is_none(),
                "entry {i} ICO must not carry a hotspot"
            );
        }
        assert!(
            !e.data.is_empty(),
            "entry {i} payload should be non-empty (dwBytesInRes > 0 enforced on read)"
        );
    }

    // Second pass: round-trip the parsed entries through the writer
    // and re-parse. The writer's output must be byte-stable on a
    // second pass (write→read→same `IconEntryRaw` fields) — that's
    // the contract that makes "read the icon, tweak one entry, write
    // it back" a safe edit pattern for callers.
    let Ok(round_a) = write_ico_raw(icon_type, &entries) else {
        // The writer rejecting a parser-accepted input is itself a
        // bug worth surfacing, but only when the rejection isn't
        // structural (parsed entries may legitimately violate writer
        // invariants the parser is more lenient about — e.g. payload
        // ranges < 1, which the parser blocks but a hand-crafted
        // input could still slip through if a future change relaxes
        // the parser). Skip rather than panic to avoid flagging
        // intentional asymmetry.
        return;
    };
    let Ok((round_type, round_entries)) = read_ico_raw(&round_a) else {
        panic!(
            "re-parsing writer output failed: file is {} bytes",
            round_a.len()
        );
    };
    assert_eq!(round_type, icon_type, "round-trip flipped the icon type");
    assert_eq!(
        round_entries.len(),
        entries.len(),
        "round-trip lost or duplicated entries"
    );
    for (i, (a, b)) in entries.iter().zip(round_entries.iter()).enumerate() {
        assert_eq!(a.width, b.width, "entry {i} width drift");
        assert_eq!(a.height, b.height, "entry {i} height drift");
        assert_eq!(a.sub_format, b.sub_format, "entry {i} sub_format drift");
        assert_eq!(a.hotspot, b.hotspot, "entry {i} hotspot drift");
        assert_eq!(a.data, b.data, "entry {i} payload drift");
    }
});
