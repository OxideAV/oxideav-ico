# oxideav-ico

[![CI](https://github.com/OxideAV/oxideav-ico/actions/workflows/ci.yml/badge.svg)](https://github.com/OxideAV/oxideav-ico/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/oxideav-ico.svg)](https://crates.io/crates/oxideav-ico) [![docs.rs](https://docs.rs/oxideav-ico/badge.svg)](https://docs.rs/oxideav-ico) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Pure-Rust **ICO** + **CUR** (Windows icon / cursor) and **ANI**
(animated cursor) reader and writer for the
[`oxideav`](https://github.com/OxideAV/oxideav) framework, following
the OxideAV image-crate API contract (`IMAGE_CRATE_API.md` in the
umbrella). Handles multi-resolution icons with mixed DIB + PNG
sub-images exactly the way modern Windows produces them.

- `ICONDIR` (`idType = 1` for `.ico`, `2` for `.cur`)
- N × `ICONDIRENTRY` → PNG body (sniffed by magic) or DIB body
  (`BITMAPINFOHEADER`, doubled `biHeight`, 1-bpp AND mask)
- DIB sub-images at 1/4/8-bpp indexed (palette + AND mask), 16-bpp,
  24-bpp BGR and 32-bpp BGRA — read **and** (except 16-bpp) write, with
  mixed depths in a single multi-resolution file
- CUR hotspot round-tripped via the `planes` / `bit_count` fields
- ANI (RIFF/`ACON`) animated cursors: raw model, decoded playback,
  encoders, framework demuxer

## Standalone use

```toml
oxideav-ico = { version = "0.0", default-features = false }
```

```rust
let bytes = std::fs::read("app.ico")?;
if oxideav_ico::probe(&bytes) {
    let info = oxideav_ico::info(&bytes)?;     // directory walk only: no pixels decoded
    println!("{} entries, primary {}×{} ({:?})",
             info.frames, info.width, info.height, info.icon_type);

    let img = oxideav_ico::decode(&bytes)?;    // the largest entry, native Rgba
    let rgba: Vec<u8> = img.to_rgba8();        // tightly packed, 4 × width bytes per row
    let (w, h) = (img.width(), img.height());

    // Every entry, in directory order.
    for frame in oxideav_ico::decode_all(&bytes)? {
        let im = &frame.image;
        println!("#{} {}×{} {}-bpp {:?}", frame.index, im.width, im.height, im.bit_depth, im.sub_format);
    }

    let opts = oxideav_ico::EncodeOptions::default();
    let out = oxideav_ico::encode_rgba8(w, h, &rgba, &opts)?;   // single-entry icon
    std::fs::write("copy.ico", out)?;
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

The whole contract surface works with `default-features = false`: the
sub-image codecs (`oxideav-bmp` for DIB payloads, `oxideav-png` for
embedded PNG payloads) are image crates with their own standalone layer
and are plain dependencies of this crate, so **every** ICO / CUR entry
decodes standalone — no `oxideav-core` needed.

| Item | Notes |
|---|---|
| `probe(&[u8]) -> bool` | ICO has no magic: `ICONDIR` + first `ICONDIRENTRY` plausibility (reserved `0`, type `1`/`2`, count ≥ 1, entry reserved `0`, non-empty payload past the directory). RIFF/`ACON` is `false`. Allocation-free. |
| `info(&[u8]) -> ImageInfo` | Directory walk, no pixels. Contract fields describe the primary entry; `frames` = entry count. Extras: `icon_type`, `primary` (index), `bit_depth`, `sub_format`, `hotspot`, `entries: Vec<IcoEntryInfo>` (every row). |
| `decode(&[u8]) -> IcoImage` | The **primary** entry: largest by area, highest bit depth breaking a tie (`EntrySelection::Largest`). |
| `decode_with(&[u8], &DecodeOptions)` | Limits, `strict`, and `entry: EntrySelection` (`Largest` / `Index(i)` / `BestFit(px)` / `Dimensions(w, h)`). |
| `decode_rgb8` / `decode_rgba8` | The primary entry as packed `RgbImage` / `RgbaImage`. |
| `decode_all(&[u8]) -> Vec<Frame>` | Every entry in directory order; `Frame { image, delay: None, index }`. `decode_all_with` takes options (`max_bytes` bounds the sum). |
| `decode_from<R: Read>` | Reads to end, then `decode`. |
| `encode(&IcoImage, &EncodeOptions)` | One-entry ICO / CUR (`icon_type` option). PNG or DIB by the size rule below. |
| `encode_all(&[Frame], &EncodeOptions)` | Multi-entry icon, slice order = directory order (mirror of `decode_all`). `encode_images(&[IcoImage], …)` is the same without the `Frame` wrapper. |
| `encode_rgb8` / `encode_rgba8` | One-call raw paths (RGB is widened to opaque RGBA). |
| `encode_to<W: Write>` | Streaming `encode`. |
| `IcoImage` | `width, height, format: PixelFormat, planes: Vec<Plane>, color: ColorInfo, metadata: Metadata` + extras `bit_depth`, `sub_format`, `hotspot`. `new` / `packed` / `from_rgb8` / `from_rgba8` return `Result`. |
| `IcoError` (`Error`) | `InvalidData`, `Unsupported`, `LimitExceeded`, `Io(std::io::Error)`. |

Beneath the floor: `decode_entry(&IconEntryRaw, &DecodeOptions)` and
`encode_entry(&IcoImage, &EncodeOptions) -> IconEntryRaw` work on one
directory entry (pick with the `select_*_raw` family, lay out with
`write_ico_raw`); `decode_payload` decodes bare sub-image bytes.

### Why every sub-image is `Rgba`

An ICO / CUR sub-image is *defined* as the composition of a colour (XOR)
bitmap with a 1-bpp transparency (AND) mask ("Compose RGBA: where the
AND bit is set, output transparent; otherwise use the XOR colour" —
`docs/image/ico/ico-cur-format.md`, decoder checklist step 6). The
per-pixel mask cannot ride on an indexed `Pal8` or alpha-less `Bgr24`
plane, so the one layout that carries every entry completely — whatever
its on-disk depth, DIB or PNG — is packed `Rgba`. The on-disk depth and
encoding survive as `IcoImage::bit_depth` / `sub_format`, and
`EncodeOptions::per_image_bit_depth` re-encodes each entry at that
depth, so a decode → edit → re-encode cycle stays faithful.

## Framework use

```rust
let mut ctx = oxideav_core::RuntimeContext::new();
oxideav_ico::register(&mut ctx);
// "ico" codec (one sub-image payload per packet), "ico" demuxer + muxer
// (.ico / .cur), "ani" demuxer are now available to the pipeline / CLI.
```

With the default-on `registry` feature: `register(&mut RuntimeContext)`
(also `register_codecs` / `register_containers` / `register_registries`
for separately held registries), `make_decoder` / `make_encoder`,
`From<IcoImage> for VideoFrame` and `IcoImage::from_video_frame(&frame,
&params)` / `TryFrom<(&VideoFrame, &CodecParameters)>`. The framework
`Decoder` / `Encoder` call the standalone `decode_payload` /
`encode_entry` — one implementation.

The `"ico"` demuxer emits one stream + one packet per directory entry
(the raw PNG / DIB payload; CUR hotspots as 4-byte extradata); the
`"ico"` codec decodes a payload to an `Rgba` frame and encodes an
`Rgba` / `Bgra` / `Rgb24` / `Bgr24` frame to a payload, with
`EncodeOptions` exposed as codec options (`png_size_threshold`,
`bmp_bit_depth`, `per_image_bit_depth`, `embed_metadata`, `icon_type`).
Registry frames carry a colour-signal side-channel **only** when the
payload signalled colour (an embedded PNG's `sRGB` / `cICP` / `iCCP`, a
V4 / V5 DIB tag); the crate's `ico_default` convention is never stamped.

The framework `Demuxer` shares the `read_ico_raw` directory walk with
the standalone API, so it inherits the full validation surface below.

## Supported layouts

Decode (every entry composes to one native layout):

| On-disk entry | Native `PixelFormat` | Notes |
|---|---|---|
| DIB 1 / 4 / 8-bpp indexed + AND mask | `Rgba` | palette expanded, mask → alpha 0 |
| DIB 16-bpp (`BI_RGB` 555 / `BI_BITFIELDS`) + AND mask | `Rgba` | mask → alpha 0 |
| DIB 24-bpp BGR + AND mask | `Rgba` | mask → alpha 0 |
| DIB 32-bpp BGRA + AND mask | `Rgba` | XOR alpha kept, mask bits force alpha 0 |
| Embedded PNG (any PNG colour type / depth) | `Rgba` | through `oxideav_png::decode` + `to_rgba8` |

Encode (`IcoImage` is always `Rgba`; the options pick the payload):

| Payload | Selected by | Lossless? |
|---|---|---|
| Embedded PNG (RGBA) | smaller side ≥ `png_size_threshold` (default **256**) | yes |
| DIB 32-bpp BGRA + alpha-derived AND mask | default below the threshold (`BmpBitDepth::Bgra32`) | yes |
| DIB 24-bpp BGR + AND mask | `BmpBitDepth::Rgb24` | alpha collapses to on/off; colour under alpha 0 lost |
| DIB 8 / 4 / 1-bpp indexed + AND mask | `BmpBitDepth::Indexed8/4/1` | exact-colour palette (≤ 256 / 16 / 2 opaque colours, else `Unsupported`); colour under alpha 0 lost |

`decode(encode(img)) == img` holds for planes, colour and metadata on
the PNG and 32-bpp DIB paths (pinned by tests and the `ico_contract`
fuzz target). Unrepresentable input is `Error::Unsupported`: a
dimension outside `1..=256` (the directory stores sizes in one byte,
`0 == 256`), or more colours than an indexed depth holds. Nothing is
converted silently — ICO has only the one native layout.

## Options

`EncodeOptions` (`#[non_exhaustive]`, `Default`, `with_*`):

| Field | Default | Meaning |
|---|---|---|
| `icon_type: IconType` | `Ico` | directory type; CUR writes each image's `hotspot` (`(0, 0)` when `None`) |
| `png_size_threshold: Option<u32>` | `Some(256)` | PNG when the smaller side is ≥ this; `None` = DIB everywhere; `Some(1)` = PNG everywhere. Follows the format reference ("PNG … used for large icons, typically 256×256") |
| `bmp_bit_depth: BmpBitDepth` | `Bgra32` | DIB depth: `Bgra32`, `Rgb24`, `Indexed8`, `Indexed4`, `Indexed1` |
| `per_image_bit_depth: bool` | `false` | encode each DIB entry at its own `IcoImage::bit_depth` (`1/4/8/24/32`; anything else falls back to `bmp_bit_depth`) — faithful mixed-depth re-encode |
| `embed_metadata: bool` | `true` | write ICC / Exif / XMP / gamma into PNG-routed entries (DIBs cannot carry them) |

`DecodeOptions` (`#[non_exhaustive]`, `Default`, `with_*`):

| Field | Default | Meaning |
|---|---|---|
| `max_width` / `max_height: Option<u32>` | `None` | per-entry geometry limits (the format caps both at 256 anyway) |
| `max_pixels: Option<u64>` | `None` | per-entry pixel limit |
| `max_bytes: Option<u64>` | `Some(1 GiB)` | decoded RGBA bytes a call may produce (one entry for `decode`, the sum for `decode_all`) |
| `strict: bool` | `false` | refuse trailing bytes after the last payload; forwarded to the PNG / DIB decoders |
| `entry: EntrySelection` | `Largest` | which entry `decode` returns (ignored by `decode_all`) |

Limits are checked against the directory rows and payload headers
before any pixel buffer is allocated, and forwarded to the `oxideav-png`
/ `oxideav-bmp` decoders so a payload header that disagrees with its
directory row cannot bypass them. A hit is `Error::LimitExceeded`.

## Metadata and colour

The ICO / CUR container carries no colour information and no metadata
of its own; both come from the sub-image payload:

- **Embedded PNG**: `color` from `sRGB` / `cICP` / `iCCP` / `gAMA` +
  `cHRM` as `oxideav-png` resolves them; `metadata.icc` / `exif` / `xmp`
  from `iCCP` / `eXIf` / XMP `iTXt`, `metadata.gamma` from `gAMA` (the
  PNG file-gamma exponent, e.g. `0.45455`). On encode the same chunks
  are written back (`embed_metadata`).
- **DIB**: a V4 / V5 header's `LCS_sRGB` / `LCS_WINDOWS_COLOR_SPACE` tag
  decodes to `ColorInfo::srgb()`, a V5 embedded ICC profile to
  `metadata.icc`. The classic 40-byte `BITMAPINFOHEADER` — every
  real-world icon — has neither.
- **Default** when the payload says nothing: `ColorInfo::ico_default()`
  = full-range device RGB (`range: Full`, `primaries: 2`, `transfer: 2`,
  `matrix: 0`). This is a convention of the crate, not a definition of
  the format, so it is reported on the standalone `IcoImage` but **not**
  stamped on registry frames.

`info` reports `color` / `has_icc` / `has_exif` / `has_xmp` for the
primary entry from the payload header alone (PNG chunk walk, DIB V4/V5
header), without decoding pixels.

## Limits

- Dimensions `1..=256` per axis (the directory format), checked on read
  and on write; up to 65 535 entries per file.
- Default decode budget 1 GiB of RGBA output per call; every entry is
  bounded before its payload is touched.
- `decode_all` on a hostile 65 535-entry directory checks the whole
  directory against the limits first, so it cannot allocate its way past
  `max_bytes` one entry at a time.
- Hostile input never panics: `probe` / `info` / `decode` /
  `decode_all` / `decode_rgba8` are fuzzed (`ico_contract`), the raw
  directory and ANI walkers separately (`ico_raw_parser`,
  `ani_raw_parser`), the codec round trip too (`ico_self_roundtrip`).

## Scope

- Read: ICO + CUR, PNG + DIB sub-images at 1/4/8/16/24/32-bpp
  (indexed + direct), 1..=256 px in each axis.
- Write: RGBA inputs; PNG, 32-bpp BGRA, 24-bpp BGR, or 1/4/8-bpp
  indexed DIB output per entry (mixed depths in one file via
  `per_image_bit_depth`). No 16-bpp DIB writer.
- Not implemented: the Windows Vista-era "PNG inside a BMP-claiming
  directory entry" quirk is handled implicitly — the reader sniffs the
  body bytes, never the directory's claim.
- ANI (Windows animated cursor, RIFF/ACON) has its own model (see
  "ANI" below). `read_ico_raw` refuses ANI input cleanly — its error
  message points the caller at `read_ani_raw`; `probe` returns `false`.

## Deprecated pre-contract names

Kept for one release as thin wrappers: `read_ico(&[u8]) -> (IconType,
Vec<IconImage>)` (use `info` + `decode_all`), `write_ico(IconType,
&[IconImage], WriteOptions)` (use `encode_all` / `encode_images`),
`IconImage` (RGBA `pixels` + extras; converts to / from `IcoImage`) and
`WriteOptions` (keeps its historical `png_size_threshold = Some(64)`
default; `into_encode_options(icon_type)` maps it). The ANI depth API
(`AniFrame`, `AniWriteFrame`, `AniWriteOptions::ico`) now speaks
`IcoImage` / `EncodeOptions` directly.

## Picking a sub-image

For multi-resolution `.ico` files where the caller wants a single best
match for a given render size:

```rust
use oxideav_ico::{decode_all, select_best_fit, select_by_dimensions, select_largest, IcoImage};

# let bytes = std::fs::read("app.ico")?;
let images: Vec<IcoImage> = decode_all(&bytes)?.into_iter().map(|f| f.image).collect();
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
# Ok::<(), Box<dyn std::error::Error>>(())
```

`select_best_fit` / `select_largest` match the spirit of Windows'
`LookupIconIdFromDirectoryEx`; `select_by_dimensions` is the strict
equality variant for callers that want a specific size or nothing. The
same three heuristics drive `decode_with` through
`DecodeOptions::entry` (`EntrySelection::BestFit(px)` / `Largest` /
`Dimensions(w, h)`, plus `Index(i)`), which picks the entry from the
directory rows and decodes only that one.

### Directory-level selection (before any decode)

The three `select_*` functions above operate on decoded `IcoImage`s —
i.e. **after** every sub-image has been decoded to RGBA. Windows'
`LookupIconIdFromDirectoryEx` works the other way around: it picks a
*directory entry* from its `bWidth` / `bHeight` / `wBitCount` first,
then decodes only the chosen body. The `select_*_raw` family mirrors
that order, running the same heuristics over the undecoded
`IconEntryRaw` rows from `read_ico_raw`:

```rust
use oxideav_ico::{read_ico_raw, select_best_fit_raw};

# let bytes = std::fs::read("app.ico")?;
let (_ty, entries) = read_ico_raw(&bytes)?;
// Closest fit for a 32-px slot — chosen from directory metadata only,
// no PNG / BMP body decoded yet.
let idx = select_best_fit_raw(&entries, 32).unwrap();
let chosen = &entries[idx];           // now decode just this one:
let img = oxideav_ico::decode_entry(chosen, &oxideav_ico::DecodeOptions::default())?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`select_largest_raw` and `select_by_dimensions_raw` are the directory
counterparts of `select_largest` / `select_by_dimensions`. All three
share the decoded family's exact tie-break rules (highest bit depth
wins at equal size), and — like `read_ico_raw` itself — are available
with `default-features = false` (no `oxideav-core` dependency). A
caller that only needs one resolution from a multi-entry `.ico` saves
N-1 sub-image decodes.

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

### Framework demuxer (`"ani"` container)

With the default `registry` feature, `register` wires ANI into the
`ContainerRegistry` as a distinct container named `"ani"` (separate
from `"ico"`): a probe that scores RIFF+`ACON` magic at full
confidence and a bare `.ani` extension at the extension tier, plus a
demuxer factory. The demuxer presents the animation as a **single
video stream** whose packets are the *resolved playback timeline* —
one `Packet` per `seq `/`rate` step, in display order, each carrying
the chosen frame's raw `icon` bytes (a complete ICO/CUR resource on
the `AF_ICON` path, ready for the ICO codec or a PNG/BMP probe).
Timestamps use the ACON-native 1/60-second jiffy as the stream time
base, so a packet's `pts` is the cumulative jiffy offset and
`duration` is the step's jiffy count — no lossy rate conversion. A
step that repeats a stored frame re-emits that frame's bytes, so a
consumer that just plays packets in order reproduces the full
animation (including out-of-order and repeated frames) without
consulting `seq ` itself. INFO `title`/`artist` surface via
`Demuxer::metadata`; the single-cycle length via
`Demuxer::duration_micros`. The `"ico"` demuxer continues to refuse
ANI input, so the two containers stay disjoint at probe time.

### Decoded playback (`read_ani`)

`read_ani` is the ANI-side counterpart of `decode_all`: where
`decode_all` decodes one icon resource's sub-images, `read_ani` decodes a
whole animation — every stored frame's sub-images **and** the resolved
playback timeline — in one call.

```rust
use oxideav_ico::read_ani;

let anim = read_ani(&std::fs::read("cursor.ani")?)?;
if let Some(title) = anim.info.title_str() {
    println!("title: {title}");
}
// Drive the animation loop straight off the resolved step table.
for step in &anim.steps {
    let frame = &anim.frames[step.frame_index as usize];
    let largest = &frame.images[frame.images.len() - 1];
    println!(
        "show {}x{} ({:?}) for {} jiffies",
        largest.width, largest.height, frame.icon_type, step.jiffies
    );
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

It walks the RIFF/`ACON` tree (via `read_ani_raw`), decodes each
`LIST 'fram'` `icon` frame to RGBA (via `decode_all` — each frame is a
complete ICO/CUR resource, so it may itself carry several resolutions,
grouped per frame in `AniFrame { icon_type, images }`), and resolves
the `seq ` / `rate` chunks into the same flat `Vec<AniStep>` timeline
`AniFile::playback_steps` produces — every `frame_index` guaranteed in
range for `AniAnimation::frames`. The timeline is resolved *before* the
frame decode, so the seq/rate validation surfaces ahead of any pixel
work.

For a cursor renderer, `AniFrame::primary_image()` returns the
sub-image a frame would actually display (the largest, via the same
`select_largest` heuristic the static API uses) and `AniFrame::hotspot()`
its click point (`None` for an `Ico`-typed frame). Since an animated
cursor's hotspot can change frame to frame — each frame is a full CUR
resource with its own directory — `AniAnimation::frame_at_step(i)` /
`hotspot_at_step(i)` resolve the `seq ` indirection so a timeline-driven
loop gets the displayed frame and its active hotspot per step directly:

```rust
# use oxideav_ico::read_ani;
# let elapsed: u64 = 0;
let anim = read_ani(&std::fs::read("cursor.ani")?)?;
// At some elapsed jiffy offset into one cycle:
let step = anim.step_at_jiffy(elapsed % anim.total_jiffies())?;
let frame = anim.frame_at_step(step).unwrap();
let hot = anim.hotspot_at_step(step);   // where to anchor the cursor now
let display = frame.primary_image().unwrap();
# Ok::<(), Box<dyn std::error::Error>>(())
```

`AniAnimation` also carries the same wall-clock helpers `AniFile` does,
computed straight off its already-resolved `steps` table (no re-running
the defaulting rules): `total_jiffies()` (cycle length in 1/60-second
jiffies), `cycle_seconds()` (the same in wall-clock seconds),
`step_at_jiffy(jiffy)` and `step_at_second(seconds)` (the inverse
lookup — "which step is on screen at this offset into the cycle?"). A
renderer driving the decoded RGBA frames gets cycle length and the
active step without going back to the raw `AniFile`. Interval semantics
match `AniFile::step_at_jiffy` exactly: step `i` owns the half-open
`[start_i, start_i + jiffies)` interval, a boundary jiffy lands on the
next step, and a `jiffy >= total` (or non-finite / negative `seconds`)
is rejected rather than silently clamped — the caller applies
`offset % cycle` before the lookup. Both frame layouts decode: the
common `AF_ICON`-set path (each frame carries its own ICO directory)
and the `AF_ICON`-clear path (each frame is a single headerless BMP
whose geometry lives in `anih`). For the clear path `read_ani`
synthesises a `BITMAPINFOHEADER` from `raw_bmp_descriptor` and feeds
the raw pixel rows through the same BMP-DIB decoder, yielding one
`Cur`-tagged sub-image per frame (no AND mask — raw frames decode
opaque). Only the non-indexed depths `{16, 24, 32}` are decodable: at
`iBitCount <= 8` the ACON reference leaves the raw colour-table layout
*undefined*, so those frames are refused (the raw bytes stay reachable
via `read_ani_raw` + `raw_bmp_descriptor`). Like the rest of the
decode surface, `read_ani` works with `default-features = false`.

The `LIST 'INFO'` metadata is surfaced both raw and decoded. The
`AniInfo::title` / `author` fields hold the verbatim `INAM` / `IART`
payload bytes (terminator and padding included); `AniInfo::title_str()`
/ `author_str()` decode those bytes to a `String` for the common case:

```rust
use oxideav_ico::read_ani_raw;

let ani = read_ani_raw(&std::fs::read("cursor.ani")?)?;
if let Some(title) = ani.info.title_str() {
    println!("title: {title}");
}
if let Some(author) = ani.info.author_str() {
    println!("author: {author}");
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

The accessors interpret the payload as Latin-1 (every byte
`0x00..=0xFF` maps to `U+0000..=U+00FF`, so the decode is total and
never fails) and trim the trailing NUL terminator plus any even-length
padding NUL these legacy cursor tools append — `b"My Cursor\0"` becomes
`"My Cursor"`. Interior NULs are preserved (no C-string truncation at
the first NUL), and a present-but-empty field decodes to `Some("")`
rather than `None` (the chunk *was* there). Latin-1 is the lossless
lower half of the Windows-1252 charset these tools actually wrote;
callers needing byte-exact Windows-1252 punctuation keep the raw
`Vec<u8>` field and run their own table.

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

// Or drive the same lookup from a seconds-based wall clock — the
// seconds-domain counterpart of `step_at_jiffy`, folding the spec's
// 60-jiffies-per-second conversion in so the `60` literal stays out of
// the call site (loop with `seconds % cycle_seconds`).
let elapsed_seconds: f64 = 0.28 % cycle_seconds;
let active_step = ani.step_at_second(elapsed_seconds)?;
# Ok::<(), Box<dyn std::error::Error>>(())
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

`step_at_second` is the seconds-domain counterpart of `step_at_jiffy`,
standing in the same relation to it as `cycle_seconds` stands to
`total_jiffies`. A renderer driving playback from a seconds-based wall
clock (clock-side schedulers, video-clip timelines, UI that thinks in
seconds rather than 1/60-second jiffies) gets the active step directly
instead of re-deriving the spec's 60-jiffies-per-second conversion and
handing off to `step_at_jiffy` by hand — the `60` literal is fixed in
the function name so it can't drift across call sites. The conversion
is `floor(seconds * 60)` jiffies: the floor is the correct rounding
direction for the half-open `[start, end)` step intervals, since a
fractional jiffy offset has not yet crossed into the next whole-jiffy
bucket, so a wall-clock instant resolves to the step whose interval
contains its whole-jiffy floor. A non-finite or negative `seconds` is
rejected up front (a wall-clock offset is physically non-negative and
finite; NaN especially must be caught, since every `<` jiffy-boundary
comparison against a NaN-derived value is false and would otherwise
misreport as a "past total" error), as is a `seconds` so large that
`floor(seconds * 60)` exceeds `u64::MAX` (caught before the `as u64`
cast, which would otherwise saturate silently). Otherwise it delegates
to `step_at_jiffy`, inheriting its full error contract verbatim.

`write_ani_raw` is the symmetric encoder — the ANI-side counterpart
to `write_ico_raw`. It serialises an `AniFile` back into a RIFF/`ACON`
byte stream that `read_ani_raw` parses to an equal value, emitting the
spec's canonical chunk order (`anih`, then optional `LIST 'INFO'` /
`seq ` / `rate`, then `LIST 'fram'`) and RIFF-padding odd-length
payloads with one zero byte. Frame payload bytes are written verbatim
(this layer never looks inside an `icon` body — the caller assembles
each ICO/CUR resource with `write_ico_raw` first). It mirrors the
reader's strictness up front so a caller can never produce a file the
reader would later reject: `n_frames` must equal `frames.len()` and
sit in `1..=65_536`; `n_planes ∈ {0, 1}`; `i_width` / `i_height ∈
{0} ∪ 1..=256`; `i_bit_count ∈ {0, 1, 4, 8, 16, 24, 32}`; every frame
payload non-empty; and a present `seq ` / `rate` array must match the
resolved step count (`n_steps`, or `n_frames` when `n_steps == 0`)
with every `seq ` index `< n_frames`; and the `bfAttributes`
`AF_SEQUENCE` bit must agree with whether a `seq ` chunk will be
emitted — the spec fixes bit 1 as "file contains a `seq ` sequence
chunk", so a flag that contradicts `sequence.is_some()` would produce a
file whose header advertises a chunk the body lacks (or carries a
`seq ` body the header doesn't announce). The byte parser stays lenient
about a flag-without-chunk on *read* (it falls back to identity step
order), but the writer has no reason to emit the inconsistency. Absent
optional chunks are omitted entirely (no empty `LIST 'INFO'` / `seq ` /
`rate`).

```rust
use oxideav_ico::{read_ani_raw, write_ani_raw};

let ani = read_ani_raw(&std::fs::read("cursor.ani")?)?;
let bytes = write_ani_raw(&ani)?;        // value-stable round-trip
assert_eq!(read_ani_raw(&bytes)?, ani);
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Encoded playback (`write_ani`)

`write_ani` is the RGBA-side counterpart to `read_ani`: where `read_ani`
decodes a whole `.ani` into RGBA frames plus a resolved timeline,
`write_ani` takes RGBA frames plus the timeline and serialises a complete
RIFF/`ACON` byte stream that `read_ani` parses back to an equivalent
animation. Each frame is one `AniWriteFrame` (a complete ICO/CUR
resource's worth of sub-images, mixed-resolution allowed, with its own
`icon_type` — ICO and CUR frames may be mixed); every sub-image is encoded
to its `icon` chunk via `encode_all`, and `AniWriteOptions` carries the
animation-level metadata (`LIST 'INFO'` title / author), the optional
`seq ` playback order, the optional per-step `rate` table, the default
per-step duration (`anih.iDispRate`), and the per-sub-image PNG / DIB
`EncodeOptions` (its `icon_type` is overridden by each frame's own).
`write_ani` produces the common `AF_ICON`-set path (each
frame is a full ICO/CUR resource); the compact `AF_ICON`-clear raw-BMP
path has its own encoder, `write_ani_raw_frames` (below). It rejects up
front anything `read_ani` would
later refuse: an empty frame list, a zero `default_jiffies` or zero `rate`
entry (a zero-jiffy step has no defined display behaviour), a `seq ` index
`>= frames.len()`, and a `rate` length that doesn't match the resolved step
count (the `seq ` length when present, else the frame count). The `anih`
advisory `iWidth` / `iHeight` / `iBitCount` are left at the spec's
"take from frame" sentinel (`0`) — each frame's own headers are
authoritative for the `AF_ICON` path; `nSteps` is left `0` for the identity
case (the reader applies its `nSteps = nFrames` default) and set to the
`seq ` length when a sequence is present.

```rust
use oxideav_ico::{
    read_ani, write_ani, AniInfo, AniWriteFrame, AniWriteOptions,
    EncodeOptions, IcoImage, IconType,
};
# let rgba_a = vec![0u8; 32 * 32 * 4];
# let rgba_b = rgba_a.clone();

let frames = vec![
    AniWriteFrame { icon_type: IconType::Cur, images: vec![IcoImage::from_rgba8(32, 32, rgba_a)?] },
    AniWriteFrame { icon_type: IconType::Cur, images: vec![IcoImage::from_rgba8(32, 32, rgba_b)?] },
];
let opts = AniWriteOptions {
    info: AniInfo { title: Some(b"Spinner\0".to_vec()), author: None },
    sequence: Some(vec![0, 1, 0, 1]),   // play A,B,A,B
    rates: Some(vec![6, 6, 6, 6]),      // 6 jiffies each
    default_jiffies: 6,
    ico: EncodeOptions::new().with_png_size_threshold(None), // all-DIB
};
let bytes = write_ani(&frames, &opts)?;
let anim = read_ani(&bytes)?;            // decodes back to an equivalent animation
assert_eq!(anim.steps.len(), 4);
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Encoding `AF_ICON`-clear raw-BMP frames (`write_ani_raw_frames`)

`write_ani_raw_frames` is the encode-side counterpart of `read_ani`'s
`AF_ICON`-clear decode: it takes RGBA frames plus a timeline and emits the
compact raw form where every frame is a bare bottom-up DIB pixel array
(**no** per-frame header) and the shared geometry lives once in `anih`
(`iWidth` / `iHeight` / `iBitCount`, `AF_ICON` cleared). The result parses
back through `read_ani` to an equivalent animation (one `Cur`-tagged opaque
sub-image per frame).

```rust
use oxideav_ico::{
    read_ani, write_ani_raw_frames, AniRawWriteOptions, RawFrameBitDepth,
};
# let rgba_a = vec![0u8; 32 * 32 * 4];
# let rgba_b = rgba_a.clone();

// Every frame shares one geometry — the raw path has a single anih
// descriptor for the whole file. Each is (width, height, RGBA bytes).
let frames = vec![
    (32, 32, rgba_a),
    (32, 32, rgba_b),
];
let opts = AniRawWriteOptions {
    default_jiffies: 6,
    bit_depth: RawFrameBitDepth::Bgra32,   // or Rgb24
    ..Default::default()
};
let bytes = write_ani_raw_frames(&frames, &opts)?;
let anim = read_ani(&bytes)?;
assert_eq!(anim.frames.len(), 2);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Only the two direct-colour depths `read_ani` can decode on the raw path
are offered (`RawFrameBitDepth::{Bgra32, Rgb24}`): a raw frame carries no
colour table, and the ACON format leaves the indexed (`iBitCount <= 8`)
colour-table layout undefined, so an indexed raw frame could not be
decoded back unambiguously. `Rgb24` drops alpha (a raw frame has no AND
mask, so every pixel decodes opaque). The encoder mirrors `write_ani`'s
timeline validation (empty frame list, zero `default_jiffies` / `rate`
entry, out-of-range `seq ` index, `rate`-length mismatch) plus a per-file
single-geometry check (all frames must share one size — the raw path can't
describe differing dimensions) and the `1..=256` axis range the `anih`
geometry inherits from the ICO/CUR sub-image limit. The framework `"ani"`
demuxer consumes the output too, surfacing the raw-path bit-depth as a
4-byte little-endian `extradata` block alongside the stream's width /
height (the `AF_ICON`-set path leaves `extradata` empty — the per-frame
directory is authoritative there).

### Raw-image (`AF_ICON`-clear) frames

The common ANI carries icon/cursor frames (`bfAttributes & AF_ICON`
set): each `LIST 'fram'` `icon` chunk is a complete ICO/CUR resource
whose own headers describe its geometry, so it feeds straight into
`read_ico_raw`. When `AF_ICON` is *clear*, each frame is instead a
**headerless** BMP — pure pixel data whose width / height / bit-depth /
plane count live in `anih`, not in the frame bytes. A caller therefore
can't decode such a frame from its bytes alone. `raw_bmp_descriptor`
surfaces exactly the four `anih` fields that path needs:

```rust
use oxideav_ico::read_ani_raw;
# fn decode_headerless_bmp(_frame: &[u8], _w: u32, _h: u32, _bpp: u32, _planes: u32) {}

let ani = read_ani_raw(&std::fs::read("raw-cursor.ani")?)?;
match ani.raw_bmp_descriptor()? {
    None => {
        // AF_ICON set — each frame is a full ICO/CUR resource.
        for frame in &ani.frames {
            let (_ty, _entries) = oxideav_ico::read_ico_raw(frame)?;
        }
    }
    Some(desc) => {
        // AF_ICON clear — every frame is a headerless BMP of these dims.
        for frame in &ani.frames {
            decode_headerless_bmp(frame, desc.width, desc.height,
                                  desc.bit_count, desc.planes);
        }
    }
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

It returns `None` for the icon/cursor path (the `anih` advisory geometry
isn't authoritative there) and, on the raw path, rejects an unset
`iWidth` / `iHeight` / `iBitCount` — the spec's `0` = "take from frame"
sentinel has no meaning when there's no per-frame header to defer to —
while normalising `nPlanes` to the single-plane BMP value `1`.

`read_ani` uses this descriptor internally to decode the `AF_ICON`-clear
path for the non-indexed depths `{16, 24, 32}` (it synthesises a
`BITMAPINFOHEADER` and runs the BMP-DIB decoder), so most callers never
touch `raw_bmp_descriptor` directly. The accessor stays public for the
indexed (`iBitCount <= 8`) case `read_ani` refuses — there the ACON
reference leaves the colour-table layout undefined, so a caller that
knows its own files' palette convention can hand the raw bytes to a
BMP-crate decoder itself.

The parser is hardened against the usual cursor-file CVE surface:
truncated declared RIFF size, missing or out-of-order `anih`,
oversized `nFrames` (capped at 65_536 to bound allocator pressure),
stray non-`icon` chunks inside `LIST 'fram'`, child chunks that
declare a length running past their parent, `seq ` / `rate`
appearing before `anih`, **duplicate `seq ` / `rate` chunks** (a
second occurrence is ambiguous — a probe reading the first and a
renderer reading the last would resolve different playback orders /
durations; rejected up front, mirroring the duplicate-`anih` /
duplicate-`LIST 'fram'` strictness), **`seq ` step indices `>= nFrames`** —
a renderer reaches `frames[seq[i]]` directly, so an out-of-range
entry (the classic `seq[k] = 0xFFFFFFFF` adversarial value) would
panic / out-of-bounds-read downstream — and **`anih.nPlanes` outside
`{0, 1}`**: the ACON spec fixes `nPlanes = 1` (multi-plane DIBs were
a planar-video relic that never reached cursor animation), mirroring
the ICO-path BMP-body `biPlanes ∈ {0, 1}` strictness; `0` is
tolerated as the wider-ecosystem "unspecified" sentinel. The walker
rejects the file up front rather than emit a sequence array or
multi-plane assertion a caller can't safely act on.

`anih.bfAttributes` is likewise range-checked: only bit 0 (`AF_ICON`)
and bit 1 (`AF_SEQUENCE`) are defined, and the ACON spec fixes bits
31..2 as "reserved, unused = 0". A header carrying any reserved bit is
rejected up front rather than silently round-tripped — the two
accessors that read the field (`frames_are_icons()` /
`has_sequence_flag()`) each mask down to their single bit, so a stray
high bit would otherwise survive a parse → re-emit cycle as a non-spec
value a strict consumer would later flag. `write_ani_raw` mirrors the
same check, so a `bfAttributes` value the reader refuses to accept is
also one the writer refuses to emit (closing the round-trip asymmetry).

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

`anih.cbSize` is validated too: the field repeats the 36-byte chunk
length, and the spec's §'anih' note directs the decoder to "validate
`cbSize`". The nine ANIHEADER fields occupy 36 bytes, so a self-reported
`cbSize < 36` cannot describe the structure and is rejected; a larger
value is tolerated (the documented "some encoders write a slightly
different cbSize" caveat — the RIFF chunk length, already validated, is
the authoritative bound). `write_ani_raw` mirrors the reject.

## Fuzzing

The `fuzz/` crate ships four complementary cargo-fuzz targets:

- `ico_contract` — arbitrary fuzz bytes → the standalone image-crate
  API (`probe` / `info` / `decode` / `decode_all` / `decode_rgba8` /
  `decode_rgb8` / `decode_with` + `EntrySelection`). Asserts the
  geometry invariants every decoded entry must satisfy (one packed
  `Rgba` plane of `4 × w × h` bytes, `1..=256` dimensions, `info` and
  `decode` agreeing, `decode_all` yielding `info.frames` frames with
  ascending `index`), that tight `DecodeOptions` fail with
  `LimitExceeded`, and that an accepted file re-encodes through
  `encode_all` (32-bpp DIB: byte-exact; per-image depth: alpha exact,
  opaque colour exact) and decodes back.
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
- `ani_raw_parser` — arbitrary fuzz bytes → standalone `read_ani_raw`
  RIFF/`ACON` walker plus every playback accessor (`playback_steps`,
  `total_jiffies`, `cycle_seconds`, `step_at_jiffy`, `step_at_second`,
  `raw_bmp_descriptor`). On accepted inputs it asserts the parser's
  structural guarantees (frame count matches `nFrames`, header ranges
  in-spec, `seq ` indices in range, optional-array lengths matched) and
  round-trips through `write_ani_raw` to assert value-stability. This
  drives the second CVE-prone surface: the declared RIFF size, the
  per-chunk lengths, the `nFrames` / `nSteps` counts, and the `seq `
  step indices all feed offset arithmetic and allocation sizing.

Run with `cargo fuzz run ico_contract` (or `ico_raw_parser` /
`ico_self_roundtrip` / `ani_raw_parser`).
