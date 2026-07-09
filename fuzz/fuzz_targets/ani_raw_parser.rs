#![no_main]

//! Panic-free fuzz harness for the standalone ANI (RIFF/`ACON`
//! animated-cursor) parser.
//!
//! Drives [`read_ani_raw`] on arbitrary fuzz bytes — the RIFF chunk
//! walker, the `anih` header validator, the `seq ` / `rate` array
//! parsers, the `LIST 'fram'` frame extractor, and every downstream
//! playback accessor. Animated-cursor parsers are a classic
//! CVE surface: the declared RIFF size, the per-chunk length fields,
//! the `nFrames` / `nSteps` counts, and the `seq ` step indices are all
//! attacker-controlled and feed directly into offset arithmetic and
//! allocation sizing. The property under test is simple: **no input,
//! however malformed, may panic / overflow / out-of-bounds-index the
//! parser or any of its accessors.**
//!
//! On the inputs the parser accepts, the harness additionally:
//!
//! * asserts the structural invariants the parser guarantees (frame
//!   count matches `nFrames`, header ranges are in-spec, `seq ` indices
//!   are in range, optional-array lengths match the resolved step
//!   count);
//! * exercises every playback accessor (`resolved_step_count`,
//!   `playback_steps`, `total_jiffies`, `cycle_seconds`,
//!   `step_at_jiffy`, `step_at_second`, `raw_bmp_descriptor`) — none may
//!   panic regardless of what the (possibly hand-adversarial-looking)
//!   parsed header claims;
//! * round-trips the parsed value through [`write_ani_raw`] and
//!   re-parses, asserting the writer emits a byte stream the reader maps
//!   back to an **equal** `AniFile` (the value-stability contract that
//!   makes "parse an ANI, tweak a field, write it back" a safe edit).
//!
//! Why this exists separately from `ico_raw_parser`: that target covers
//! the ICO/CUR directory walker; ANI is a wholly different RIFF-based
//! container with its own header, chunk tree, and timeline resolution.
//! `read_ico_raw` deliberately *refuses* ANI input, so the ICO fuzzer
//! never reaches any of this code.
//!
//! Wall: standalone `read_ani_raw` / `write_ani_raw` + the `AniFile`
//! accessors only — no `oxideav-bmp` / `oxideav-png` / `oxideav-core`
//! decode reachable from here, so a panic anywhere is a bug in the ANI
//! container parser itself.

use libfuzzer_sys::fuzz_target;
use oxideav_ico::{read_ani_raw, write_ani_raw, AF_ICON, AF_SEQUENCE};

/// Every `bfAttributes` bit the parser leaves defined.
const AF_DEFINED: u32 = AF_ICON | AF_SEQUENCE;

/// Sanity cap the parser enforces on `nFrames` / `nSteps`.
const MAX_FRAMES_OR_STEPS: u32 = 65_536;

fuzz_target!(|data: &[u8]| {
    let Ok(ani) = read_ani_raw(data) else {
        // Overwhelmingly the common case — arbitrary bytes are rarely a
        // valid ANI. The point is that we returned Err instead of
        // panicking.
        return;
    };

    // --- structural invariants the parser guarantees on accept --------
    let h = &ani.header;
    assert!(
        h.n_frames >= 1 && h.n_frames <= MAX_FRAMES_OR_STEPS,
        "n_frames {} out of accepted range [1, {MAX_FRAMES_OR_STEPS}]",
        h.n_frames
    );
    assert!(
        h.n_steps <= MAX_FRAMES_OR_STEPS,
        "n_steps {} exceeds cap",
        h.n_steps
    );
    assert_eq!(
        ani.frames.len(),
        h.n_frames as usize,
        "frames.len() must equal header.n_frames"
    );
    assert!(h.cb_size >= 36, "cb_size {} below 36", h.cb_size);
    assert!(h.n_planes <= 1, "n_planes {} above 1", h.n_planes);
    assert!(h.i_width <= 256, "i_width {} above 256", h.i_width);
    assert!(h.i_height <= 256, "i_height {} above 256", h.i_height);
    assert!(
        matches!(h.i_bit_count, 0 | 1 | 4 | 8 | 16 | 24 | 32),
        "i_bit_count {} outside {{0,1,4,8,16,24,32}}",
        h.i_bit_count
    );
    assert_eq!(
        h.bf_attributes & !AF_DEFINED,
        0,
        "bf_attributes {:#010x} sets reserved bits",
        h.bf_attributes
    );

    // Resolved step count is what the optional `seq ` / `rate` arrays are
    // sized against.
    let step_count = ani.resolved_step_count() as usize;
    if let Some(seq) = &ani.sequence {
        assert_eq!(seq.len(), step_count, "sequence len != resolved step count");
        for (i, &idx) in seq.iter().enumerate() {
            assert!(
                idx < h.n_frames,
                "seq[{i}] = {idx} out of range (n_frames = {})",
                h.n_frames
            );
        }
    }
    if let Some(rates) = &ani.rates {
        assert_eq!(rates.len(), step_count, "rates len != resolved step count");
    }

    // --- accessors must never panic on parser-accepted input ----------
    // `raw_bmp_descriptor` may Err (AF_ICON-clear with unset geometry),
    // but must not panic.
    let _ = ani.raw_bmp_descriptor();

    // The timeline accessors may legitimately Err (e.g. a zero-jiffy
    // step, or nSteps > nFrames with no seq chunk — both parser-accepted
    // header shapes). None may panic.
    if let Ok(steps) = ani.playback_steps() {
        assert_eq!(steps.len(), step_count, "playback_steps len != step count");
        for (i, s) in steps.iter().enumerate() {
            assert!(
                s.frame_index < h.n_frames,
                "step {i} frame_index {} out of range",
                s.frame_index
            );
            assert!(s.jiffies != 0, "step {i} resolved to zero jiffies");
        }

        // When playback_steps succeeds, total_jiffies must too, and be
        // the sum of the step durations.
        let total = ani
            .total_jiffies()
            .expect("total_jiffies must succeed when playback_steps does");
        let expect: u64 = steps.iter().map(|s| s.jiffies as u64).sum();
        assert_eq!(total, expect, "total_jiffies != sum of step jiffies");
        assert!(total >= 1, "a valid timeline has a non-zero cycle length");

        // cycle_seconds is total/60 and must succeed under the same
        // conditions.
        let secs = ani.cycle_seconds().expect("cycle_seconds must succeed");
        assert!((secs - total as f64 / 60.0).abs() < 1e-9);

        // Every in-cycle jiffy offset resolves to a valid step; the
        // boundary value `total` must be rejected (caller forgot modulo).
        assert!(
            ani.step_at_jiffy(total).is_err(),
            "step_at_jiffy(total) must reject the past-cycle offset"
        );
        let probe = total - 1;
        let last = ani
            .step_at_jiffy(probe)
            .expect("in-cycle jiffy must resolve to a step");
        assert!(last < steps.len(), "step_at_jiffy returned an OOB index");

        // step_at_second on a finite non-negative offset must not panic;
        // it may Err if the converted jiffy offset lands past the cycle.
        let _ = ani.step_at_second(probe as f64 / 60.0);
        // Adversarial float inputs are rejected, never panic.
        assert!(ani.step_at_second(-1.0).is_err());
        assert!(ani.step_at_second(f64::NAN).is_err());
        assert!(ani.step_at_second(f64::INFINITY).is_err());
    } else {
        // If playback_steps Errs, total_jiffies / cycle_seconds share
        // its error contract — they must Err too, not panic.
        let _ = ani.total_jiffies();
        let _ = ani.cycle_seconds();
        let _ = ani.step_at_jiffy(0);
        let _ = ani.step_at_second(0.0);
    }

    // --- write → read value-stability round-trip ----------------------
    // The writer legitimately refuses some parser-accepted values (an
    // AF_SEQUENCE flag that disagrees with `sequence.is_some()`, or an
    // empty frame payload the reader tolerates but the writer rejects).
    // A refusal is intentional asymmetry, not a bug — skip rather than
    // panic. On accept, the re-parsed value must equal the original.
    let Ok(bytes) = write_ani_raw(&ani) else {
        return;
    };
    let round = read_ani_raw(&bytes)
        .expect("re-parsing write_ani_raw output must succeed");
    assert_eq!(round, ani, "write → read must be value-stable");
});
