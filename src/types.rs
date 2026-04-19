//! Public value types for the ICO / CUR API.

/// File type — distinguishes a static icon from an animated cursor.
/// `.ico` carries `IconType::Ico`; `.cur` carries `IconType::Cur` and
/// stashes a per-image hotspot in the directory entry's `planes` /
/// `bit_count` fields instead of plane count + bits per pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconType {
    /// Static icon — `idType == 1` in the `ICONDIR` header.
    Ico,
    /// Mouse cursor — `idType == 2`. Adds a per-image `HotSpot`.
    Cur,
}

/// On-disk encoding for a single image inside an ICO / CUR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSubFormat {
    /// PNG-encoded sub-image (PNG magic at the start of the entry).
    /// Modern 256×256 entries virtually always use this.
    Png,
    /// Classic BITMAPINFOHEADER DIB sub-image. Height field is 2× the
    /// real height; a 1-bpp AND mask follows the XOR pixels.
    Bmp,
}

/// CUR-only: the click point inside the cursor. `(0, 0)` is top-left.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HotSpot {
    pub x: u16,
    pub y: u16,
}

/// One decoded sub-image from an ICO / CUR, or an input image for the
/// writer. Always stores pixels as `Rgba` in top-down order (row 0 =
/// top), regardless of what the on-disk encoding was.
///
/// `sub_format` is purely advisory on the decode path — it records
/// what the original container entry used so callers can decide
/// whether to re-encode faithfully. On the write path, it's a hint
/// the writer may override based on `WriteOptions` (e.g. force all
/// images to PNG for compactness).
#[derive(Debug, Clone)]
pub struct IconImage {
    pub width: u32,
    pub height: u32,
    /// Pixels in top-down RGBA order, tightly packed, stride =
    /// `width * 4`.
    pub pixels: Vec<u8>,
    /// Bits per pixel the source entry claimed. Useful when roundtripping
    /// (so 1-bpp icons stay 1-bpp, 32-bpp stay 32-bpp). On encode paths
    /// we only produce 32-bpp BMP / 32-bpp PNG today, so this is
    /// ignored for writes.
    pub bit_depth: u8,
    pub sub_format: IconSubFormat,
    /// `Some` for CUR entries, `None` for ICO entries (or when the
    /// caller doesn't care). Ignored unless the containing file type
    /// is `Cur`.
    pub hotspot: Option<HotSpot>,
}

impl IconImage {
    /// Build an `IconImage` from top-down RGBA pixels.
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            width,
            height,
            pixels,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
        }
    }
}

/// Options for the writer. Defaults favour modern icons (PNG for
/// larger sub-images, BMP for smaller ones), matching what the
/// Windows 10+ icon tooling produces.
#[derive(Debug, Clone, Copy)]
pub struct WriteOptions {
    /// When `Some(n)`, use PNG for any sub-image whose smaller
    /// dimension is ≥ `n`; else BMP. When `None`, force BMP on every
    /// sub-image (legacy / maximum-compat write).
    ///
    /// Default: `Some(64)` — 64×64 and up go PNG, smaller ones stay
    /// BMP so they still render on Windows XP-era loaders that don't
    /// understand PNG-in-ICO.
    pub png_size_threshold: Option<u32>,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            png_size_threshold: Some(64),
        }
    }
}
