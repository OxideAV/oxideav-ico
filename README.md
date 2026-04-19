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
