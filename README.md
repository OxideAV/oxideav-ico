# oxideav-ico

Pure-Rust **ICO** + **CUR** (Windows icon / cursor) reader and writer
for the [`oxideav`](https://github.com/OxideAV/oxideav) framework.
Handles multi-resolution icons with mixed BMP + PNG sub-images exactly
the way modern Windows produces them.

- `ICONDIR` (`idType = 1` for `.ico`, `2` for `.cur`)
- N × `ICONDIRENTRY` → PNG body (sniffed by magic) or BMP DIB body
  (doubled `biHeight` + 1-bpp AND mask)
- CUR hotspot round-tripped via the `planes` / `bit_count` fields

## Read

```rust
use oxideav_ico::{read_ico, IconType};

let bytes = std::fs::read("app.ico")?;
let (ty, images) = read_ico(&bytes)?;
assert!(matches!(ty, IconType::Ico));
for img in images {
    println!("{}x{} ({:?}) {} bytes", img.width, img.height, img.sub_format, img.pixels.len());
}
```

Each `IconImage` carries pixels as **top-down RGBA**, tightly packed.
`sub_format` records whether the source entry was BMP or PNG so
callers who want a faithful roundtrip can preserve that.

## Write

```rust
use oxideav_ico::{write_ico, IconImage, IconType, WriteOptions};

let imgs = vec![
    IconImage::from_rgba(16,  16,  rgba_16),
    IconImage::from_rgba(32,  32,  rgba_32),
    IconImage::from_rgba(128, 128, rgba_128),
    IconImage::from_rgba(256, 256, rgba_256),
];
let bytes = write_ico(IconType::Ico, &imgs, WriteOptions::default())?;
std::fs::write("out.ico", bytes)?;
```

`WriteOptions::default()` switches sub-images ≥ 64 px to PNG and
keeps smaller ones as BMP — matches what Windows 10+ ships. Set
`png_size_threshold = None` to force all-BMP (maximum legacy
compatibility).

A 256×256 sub-image is the canonical large-icon case: the directory's
single-byte width/height fields can't hold 256, so they serialise as
`0` (the `0 == 256` convention) and the true size is recovered from
the PNG body's IHDR on read. `write_ico` rejects any sub-image outside
`1..=256` in either axis up front — before the encode pass — since the
directory physically cannot describe it.

## CUR

```rust
use oxideav_ico::{write_ico, HotSpot, IconImage, IconType, WriteOptions};

let mut cur = IconImage::from_rgba(32, 32, rgba_32);
cur.hotspot = Some(HotSpot { x: 10, y: 12 });
let bytes = write_ico(IconType::Cur, &[cur], WriteOptions::default())?;
```

## Registration

```rust
let mut codecs = oxideav_codec::CodecRegistry::new();
let mut containers = oxideav_container::ContainerRegistry::new();
oxideav_ico::register(&mut codecs, &mut containers);
// "ico" codec + container are now available to the pipeline / CLI.
```

## Scope

- Read: ICO + CUR, PNG + BMP sub-images, 1..=256 px in each axis.
- Write: 32-bpp RGBA inputs, PNG or BMP output per entry.
- Not implemented: Windows Vista-era `PNG-inside-BMP-header` quirk
  (where the directory entry claims BMP but the body is secretly
  PNG). Nobody writes this; the reader already handles it because it
  sniffs the body bytes.
- ANI (Windows animated cursor, RIFF/ACON) is parsed by the
  separate `read_ani_raw` helper (see "ANI" below). `read_ico_raw`
  still refuses ANI input cleanly — its error message points the
  caller at `read_ani_raw`.

## Picking a sub-image

For multi-resolution `.ico` files where the caller wants a single best
match for a given render size:

```rust
use oxideav_ico::{read_ico, select_best_fit, select_by_dimensions, select_largest};

let (_, images) = read_ico(&bytes)?;
// Closest fit for a 32×32 slot. Prefers the smallest entry ≥ 32,
// falls back to the largest available when every entry is smaller.
// Bit-depth breaks ties (32-bpp beats 1-bpp at the same resolution).
let idx = select_best_fit(&images, 32).unwrap();
let chosen = &images[idx];

// Or just the highest-fidelity entry, irrespective of target size.
let idx = select_largest(&images).unwrap();

// Or a strict, pixel-exact lookup — `None` if no entry is exactly
// that size (no nearest-fit substitution). Bit-depth breaks ties when
// the same size appears at several depths.
let idx = select_by_dimensions(&images, 256, 256);
```

`select_best_fit` / `select_largest` match the spirit of Windows'
`LookupIconIdFromDirectoryEx`; `select_by_dimensions` is the strict
equality variant for callers that want a specific size or nothing.

## Validation surface

`read_ico_raw` rejects malformed directories before they reach a
sub-image decoder:

- `RIFF/ACON` magic — caller passed an `.ani` animated cursor.
- `idCount = 0`, `idType` not in {1, 2}, `idReserved != 0`.
- Per entry: `bReserved != 0`, `dwBytesInRes = 0`, `dwImageOffset`
  pointing into the directory, `offset + size` overflowing usize or
  running past EOF.
- ICO entries: `wPlanes` not in {0, 1}, `wBitCount` not in
  {0, 1, 4, 8, 16, 24, 32}, `bColorCount != 0` for >= 16-bpp.
- BMP body: `biBitCount` outside {0, 1, 4, 8, 16, 24, 32}; `biPlanes`
  outside {0, 1} (the spec mandates `biPlanes = 1`; `0` is accepted
  as the "unspecified" tolerance the directory side also allows);
  `biCompression` outside {`BI_RGB = 0`, `BI_BITFIELDS = 3`} — the
  ICO spec mandates uncompressed RGB for sub-images, and the
  `BI_BITFIELDS` carve-out covers 16-bpp / 32-bpp DIBs that declare
  explicit per-channel masks (the wider ecosystem produces those).
  `BI_RLE4` / `BI_RLE8` / `BI_JPEG` / `BI_PNG` / `BI_ALPHABITFIELDS`
  bodies are rejected up front rather than silently routed to a
  BMP-DIB renderer that doesn't implement them.
  `biSize` outside {40 (`BITMAPINFOHEADER`), 108 (`BITMAPV4HEADER`),
  124 (`BITMAPV5HEADER`)} — the 1995 ICO spec mandates v3
  (`BITMAPINFOHEADER`, 40 bytes); v4 / v5 are accepted as drop-in
  successors whose extra colour-space cells sit after the v3
  layout. The OS/2 `BITMAPCOREHEADER` (12) is rejected — its
  16-bit `bcWidth` / `bcHeight` fields can't carry the
  doubled-height ICO convention; the Adobe-Photoshop
  `BITMAPV2INFOHEADER` (52) / `BITMAPV3INFOHEADER` (56)
  extensions are also rejected (not part of Microsoft's
  documented BITMAPINFOHEADER family).
- CUR entries: hotspot `(x, y)` outside `width × height`.
- Cross-entry: no two sub-image payloads may overlap. Overlapping
  ranges have been used to smuggle a second body through the same
  offset window (probe sees one image, renderer parses another); the
  parser rejects the whole file rather than picking a side.
- CUR hotspot probe-vs-render: the hotspot is re-checked against the
  **body-derived** dimensions (PNG IHDR or DIB header) after the
  initial directory-declared check. A directory that claims 256×256
  (the canonical `bWidth = bHeight = 0` encoding) with a body that
  decodes to 2×33 can no longer slip a (0, 128) hotspot through —
  what the directory probe sees and what the PNG/BMP renderer sees
  must agree.
- Directory-vs-body **dimension** probe-vs-render: when the directory's
  `bWidth` / `bHeight` byte is non-zero, the value is an exact
  assertion of the sub-image dimension; the body's PNG IHDR
  width/height or BMP `biWidth` / halved-`biHeight` must agree. A
  directory that says `bWidth = 16` shipping a body whose IHDR
  reports 64 is rejected up front rather than emitting an
  `IconEntryRaw { width: 64, … }` that silently contradicts the
  directory the caller just inspected. The `bWidth = 0` (canonical
  256-encoding) case is the only carve-out: the directory cannot
  physically encode a literal dimension other than 256, so the
  body is authoritative for that case (still subject to the
  `1..=256` body-dim range check).
- Directory-vs-body **bit-depth** probe-vs-render (BMP path, ICO
  type): when both the directory `wBitCount` and the BMP body's
  `biBitCount` are non-zero, they must agree. A directory
  advertising `wBitCount = 8` shipping a body whose `biBitCount`
  decodes to 32 is rejected up front. Either side being `0`
  ("unspecified — defer to the other header") makes the check
  vacuous, mirroring the existing `wBitCount = 0` tolerance. CUR
  files are exempt (the directory WORD at offset 6 is the
  hotspot Y, not a bit-depth assertion), and PNG bodies are
  exempt (no `biBitCount` field for the directory to agree with).

`write_ico_raw` mirrors the CUR-hotspot and empty-payload checks so
emitted files always round-trip through the parser.

## ANI (animated cursors)

`.ani` is a RIFF container whose form-type is `ACON`. Each file
carries an `anih` ANIHEADER, an optional `LIST 'INFO'` (title /
author), optional `seq ` / `rate` chunks (step-sequence override
and per-step jiffy durations), and a `LIST 'fram'` containing N
`icon` chunks — each `icon` is a complete ICO or CUR resource when
`bfAttributes & AF_ICON` is set (the common case), or a raw
headerless BMP otherwise.

```rust
use oxideav_ico::{read_ani_raw, read_ico_raw};

let bytes = std::fs::read("cursor.ani")?;
let ani = read_ani_raw(&bytes)?;

println!(
    "{} frames, {} steps, default {}/60s, AF_ICON={}",
    ani.header.n_frames,
    ani.header.n_steps,
    ani.header.i_disp_rate,
    ani.header.frames_are_icons(),
);

if ani.header.frames_are_icons() {
    for (i, frame_bytes) in ani.frames.iter().enumerate() {
        let (ty, entries) = read_ico_raw(frame_bytes)?;
        println!("frame {i}: {ty:?} with {} sub-image(s)", entries.len());
    }
}

// `seq` / `rate` are `None` when the chunk was absent — fall back
// to identity step order / `header.i_disp_rate` respectively.
let step_order: Vec<u32> = ani.sequence.clone().unwrap_or_else(
    || (0..ani.header.n_frames).collect(),
);
let durations: Vec<u32> = ani.rates.clone().unwrap_or_else(
    || vec![ani.header.i_disp_rate; step_order.len()],
);

// Or skip the per-chunk defaulting and let `playback_steps()` merge
// `seq` / `rate` / `iDispRate` / `nSteps` into a typed table of
// `(frame_index, jiffies)` tuples the animation loop drives directly:
let steps = ani.playback_steps()?;
for step in &steps {
    let frame_bytes = &ani.frames[step.frame_index as usize];
    println!("show {frame_bytes:p} for {} jiffies", step.jiffies);
}

// One full animation cycle's length, in 1/60-second jiffies. Returns a
// u64 so the sum can't overflow on adversarial input (65_536 steps ×
// u32::MAX rate ≈ 2.8e14, which fits a u64 with room to spare).
let cycle_jiffies = ani.total_jiffies()?;

// Or, the same cycle in wall-clock seconds, folding the spec's
// "1/60 of a second per jiffy" conversion into the type system so
// the `60` literal doesn't drift across call sites and the unit is
// fixed in the function name. Exact in f64 for every parser-accepted
// input (worst case ~2.8e14 jiffies, well under f64's 2^53 integer
// boundary).
let cycle_seconds = ani.cycle_seconds()?;

// Wall-clock → step inverse: given a jiffy offset into one cycle,
// locate the active playback step. A renderer driven by a
// wall-clock-like elapsed counter typically does `elapsed % total`
// and feeds the result here to find "what step is on screen now?".
let elapsed_jiffies: u64 = 17 % cycle_jiffies;
let active_step = ani.step_at_jiffy(elapsed_jiffies)?;
let frame_bytes = &ani.frames[steps[active_step].frame_index as usize];
```

`playback_steps` resolves the spec's defaulting rules — `nSteps = nFrames`
when the field is zero; identity `i` when no `seq ` chunk is present;
`header.i_disp_rate` when no `rate` chunk is present — and refuses any
step whose resolved duration is `0` (a zero-jiffy step has no defined
display behaviour and would either burn 100% CPU in a poll-based
renderer or divide-by-zero in a frame-rate normaliser). Identity steps
past `nFrames` are also refused (only reachable when the header pairs
`nSteps > nFrames` with no `seq ` chunk — the spec is silent on this
combination and the accessor refuses rather than fabricate out-of-range
indices that would panic downstream).

`total_jiffies` returns one full animation cycle's length as a `u64`,
folding the same `rate` / `iDispRate` / `nSteps` / `nFrames` defaulting
rules into a single number. The `u32 → u64` widening is load-bearing:
a worst-case file (the 65_536-step cap × `u32::MAX` per-step rate)
sums to roughly `2.8e14`, which exceeds `u32::MAX` by a factor of
65_536. The accessor mirrors `playback_steps`'s zero-jiffy rejection
contract (the cycle length of a malformed file is meaningless, and
returning a smaller-than-real total would mask the bug). The accessor
deliberately does not consult the `seq ` chunk: per-step duration in
the ACON spec depends only on the step index, not on which frame
the step picks, so two files with the same rate table and different
sequences yield the same total.

`cycle_seconds` is the wall-clock counterpart — the same total, divided
by the spec's 60-jiffies-per-second conversion factor, returned as an
`f64`. A renderer wiring the result into clock-side scheduling (sleep
timers, video-clip lengths, "1.5 s loop" UI labels) gets the unit
fixed in the function name rather than carrying the `60.0` literal
across call sites. The conversion is exact for every cycle length
the parser can produce: the 65_536-step × `u32::MAX` worst case sums
to roughly `2.8e14` jiffies, well under `f64`'s `2^53 ≈ 9.0e15`
integer-precision boundary. The accessor reuses `total_jiffies`'s
error contract verbatim (`n_frames = 0`, mismatched `rates` length,
any zero-jiffy step), so hand-constructed `AniFile`s that the byte
parser can't reach still surface the same rejection paths.

`step_at_jiffy` is the inverse mapping a wall-clock-driven renderer
actually needs at every frame: given a jiffy offset into one cycle,
return the step index that's currently active. Step `i` claims the
half-open interval `[start_i, start_i + step.jiffies)` where `start_i`
is the cumulative sum of every preceding step's duration, so step `0`
spans `[0, step_0.jiffies)`, step `1` spans `[step_0.jiffies,
step_0.jiffies + step_1.jiffies)`, and so on. A `jiffy` exactly equal
to a step boundary lands on the next step (matching the spec's "show
frame, then advance" edge semantics); a `jiffy >= total_jiffies` is
rejected up front so a renderer with a buggy wall-clock counter (one
that wrapped past cycle end or never reset) sees a deterministic error
rather than getting silently stuck on the last frame forever. The
caller is responsible for applying `jiffy % total_jiffies` before the
lookup — looping is a renderer-level concern, not the accessor's.
Parameter type is `u64` to match `total_jiffies`'s return type (a
cycle whose total exceeds `u32::MAX` can produce a per-cycle elapsed
offset that doesn't fit a `u32`, so the accessor doesn't force the
caller to pre-truncate). The accessor delegates to `playback_steps`
up front so a malformed file (zero-jiffy step, identity-fallback past
nFrames, mismatched-length sequence / rates) surfaces a single
deterministic error rather than an ambiguous "active step = ?" answer.

The parser is hardened against the usual cursor-file CVE surface:
truncated declared RIFF size, missing or out-of-order `anih`,
oversized `nFrames` (capped at 65_536 to bound allocator pressure),
stray non-`icon` chunks inside `LIST 'fram'`, child chunks that
declare a length running past their parent, `seq ` / `rate`
appearing before `anih`, **`seq ` step indices `>= nFrames`** —
a renderer reaches `frames[seq[i]]` directly, so an out-of-range
entry (the classic `seq[k] = 0xFFFFFFFF` adversarial value) would
panic / out-of-bounds-read downstream — and **`anih.nPlanes` outside
`{0, 1}`**: the ACON spec fixes `nPlanes = 1` (multi-plane DIBs were
a planar-video relic that never reached cursor animation), mirroring
the ICO-path BMP-body `biPlanes ∈ {0, 1}` strictness; `0` is
tolerated as the wider-ecosystem "unspecified" sentinel. The walker
rejects the file up front rather than emit a sequence array or
multi-plane assertion a caller can't safely act on.

The advisory `anih.iWidth` / `iHeight` / `iBitCount` fields are
also range-checked: dimensions must be in `1..=256` (the ICO/CUR
sub-image limit — a value of `0` retains its spec-mandated "take
from frame" sentinel), and bit-depth must be in
`{0, 1, 4, 8, 16, 24, 32}` (the BMP/ICO sub-image bit-depth set;
`0` again carries the "take from frame" meaning). An
adversarial `iWidth = 0xFFFF_FFFF` is the classic "size pulled
from user-controlled bytes" smuggling shape that would size a
raw-BMP-path renderer allocation past anything real; an
`iBitCount = 7` doesn't correspond to any renderable DIB layout.

## Fuzzing

The `fuzz/` crate ships two complementary cargo-fuzz targets:

- `ico_self_roundtrip` — RGBA → `make_encoder` → packet → `make_decoder`
  → RGBA pixel-equality. Catches encoder bugs that emit corrupt
  sub-images and decoder bugs that mis-parse legitimate output.
- `ico_raw_parser` — arbitrary fuzz bytes → standalone `read_ico_raw`
  directory walker (no codec / PNG / BMP-DIB decode in scope). On
  inputs the parser accepts, round-trips through `write_ico_raw` and
  re-parses to assert byte-stability. This is where icon parsers
  historically take CVE hits — adversarial input goes after the
  offset arithmetic, the payload-overlap detector, the RIFF/ACON
  detection, and the `planes` / `bit_count` range checks.

Run with `cargo fuzz run ico_raw_parser` (or `ico_self_roundtrip`).
