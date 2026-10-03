//! The image-crate contract records (`IMAGE_CRATE_API`): [`IcoImage`],
//! [`Plane`], [`ColorInfo`], [`Metadata`], [`RgbImage`] / [`RgbaImage`],
//! [`ImageInfo`] and [`Frame`].
//!
//! Every field shape here is identical across the OxideAV image crates
//! (a copy, not a shared dependency), so a consumer can move between
//! `oxideav_png::PngImage`, `oxideav_bmp::BmpImage` and
//! [`IcoImage`] without relearning anything.
//!
//! ## Why every ICO sub-image is `Rgba`
//!
//! An ICO / CUR sub-image is *defined* as the composition of a colour
//! (XOR) bitmap with a 1-bpp transparency (AND) mask (spec
//! `docs/image/ico/ico-cur-format.md`, "Decoder checklist" step 6:
//! "Compose RGBA: where the AND bit is set, output transparent;
//! otherwise use the XOR colour (and its alpha for 32-bpp)"). The
//! per-pixel mask cannot be expressed on top of an indexed (`Pal8`) or
//! alpha-less (`Bgr24`) plane, so the one layout that carries every
//! sub-image completely — whatever its on-disk depth or whether it is a
//! DIB or an embedded PNG — is packed `Rgba`. The on-disk depth and
//! encoding survive as the [`IcoImage::bit_depth`] /
//! [`IcoImage::sub_format`] extras so a faithful re-encode is still
//! possible.

use core::time::Duration;

use crate::error::{IcoError, Result};
use crate::types::{HotSpot, IconSubFormat, IconType};

// ---------------------------------------------------------------------------
// Pixel format
// ---------------------------------------------------------------------------

/// Native pixel layouts an [`IcoImage`] can carry. Variant names mirror
/// `oxideav_core::PixelFormat` exactly.
///
/// Only `Rgba` exists today: see the module docs for why every ICO
/// sub-image composes to RGBA. The enum is `#[non_exhaustive]` so a
/// future native layout can be added without a breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum IcoPixelFormat {
    /// Packed `R, G, B, A`, 8 bits per channel, 4 bytes per pixel.
    #[default]
    Rgba,
}

/// Contract alias of [`IcoPixelFormat`].
pub type PixelFormat = IcoPixelFormat;

impl IcoPixelFormat {
    /// Bytes per pixel of the packed layout.
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba => 4,
        }
    }

    /// Bits per pixel of the packed layout.
    pub const fn bits_per_pixel(self) -> u16 {
        match self {
            Self::Rgba => 32,
        }
    }

    /// `true` when the layout carries an alpha channel.
    pub const fn has_alpha(self) -> bool {
        match self {
            Self::Rgba => true,
        }
    }
}

// ---------------------------------------------------------------------------
// Plane
// ---------------------------------------------------------------------------

/// One pixel plane: `stride` bytes per row, `data` holding at least
/// `stride × height` bytes. Every ICO layout is packed, so an
/// [`IcoImage`] has exactly one.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Plane {
    /// Bytes per row.
    pub stride: usize,
    /// Row-major bytes, at least `stride × height` long.
    pub data: Vec<u8>,
}

impl Plane {
    /// Wrap a plane buffer with its row stride.
    pub fn new(stride: usize, data: Vec<u8>) -> Self {
        Self { stride, data }
    }
}

// ---------------------------------------------------------------------------
// Colour
// ---------------------------------------------------------------------------

/// Nominal sample range (H.273 `VideoFullRangeFlag`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ColorRange {
    /// No range was signalled.
    #[default]
    Unspecified,
    /// Limited (video / studio) range.
    Limited,
    /// Full (PC) range.
    Full,
}

/// Colour signalling of a sub-image: the sample range plus the H.273
/// `ColourPrimaries` / `TransferCharacteristics` / `MatrixCoefficients`
/// code points (`2` = unspecified).
///
/// The ICO / CUR container itself carries no colour information. A
/// sub-image's colour comes from its payload: an embedded PNG's `sRGB`
/// / `cICP` / `iCCP` / `gAMA` chunks (through `oxideav-png`), or a DIB's
/// V4 / V5 colour-space tag (through `oxideav-bmp`). A classic
/// `BITMAPINFOHEADER` DIB — the overwhelmingly common case — has none,
/// and decodes to [`ColorInfo::ico_default`]: full-range device RGB with
/// unspecified primaries and transfer. That default is a convention of
/// this crate, not a definition of the format, so it is **not** stamped
/// on registry frames (only specified colour is).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ColorInfo {
    /// Sample range.
    pub range: ColorRange,
    /// H.273 `ColourPrimaries` code point (`1` = BT.709 / sRGB, `2` =
    /// unspecified).
    pub primaries: u8,
    /// H.273 `TransferCharacteristics` code point (`13` = sRGB, `2` =
    /// unspecified).
    pub transfer: u8,
    /// H.273 `MatrixCoefficients` code point (`0` = identity / RGB).
    pub matrix: u8,
}

impl ColorInfo {
    /// H.273 "unspecified" code point.
    pub const UNSPECIFIED: u8 = 2;
    /// H.273 `MatrixCoefficients` identity (RGB) code point.
    pub const MATRIX_IDENTITY: u8 = 0;
    /// H.273 `ColourPrimaries` BT.709 / sRGB code point.
    pub const PRIMARIES_BT709: u8 = 1;
    /// H.273 `TransferCharacteristics` IEC 61966-2-1 sRGB code point.
    pub const TRANSFER_SRGB: u8 = 13;

    /// Build a description from its four parts.
    pub const fn new(range: ColorRange, primaries: u8, transfer: u8, matrix: u8) -> Self {
        Self {
            range,
            primaries,
            transfer,
            matrix,
        }
    }

    /// Every field unspecified.
    pub const fn unspecified() -> Self {
        Self::new(
            ColorRange::Unspecified,
            Self::UNSPECIFIED,
            Self::UNSPECIFIED,
            Self::UNSPECIFIED,
        )
    }

    /// The crate's convention for a sub-image whose payload carries no
    /// colour signalling (every classic `BITMAPINFOHEADER` DIB): full
    /// range device RGB (`matrix` 0), primaries and transfer unspecified.
    pub const fn ico_default() -> Self {
        Self::new(
            ColorRange::Full,
            Self::UNSPECIFIED,
            Self::UNSPECIFIED,
            Self::MATRIX_IDENTITY,
        )
    }

    /// sRGB (IEC 61966-2-1): BT.709 primaries, sRGB transfer, identity
    /// matrix, full range.
    pub const fn srgb() -> Self {
        Self::new(
            ColorRange::Full,
            Self::PRIMARIES_BT709,
            Self::TRANSFER_SRGB,
            Self::MATRIX_IDENTITY,
        )
    }

    /// Set the range.
    pub fn with_range(mut self, range: ColorRange) -> Self {
        self.range = range;
        self
    }

    /// Set the primaries code point.
    pub fn with_primaries(mut self, primaries: u8) -> Self {
        self.primaries = primaries;
        self
    }

    /// Set the transfer code point.
    pub fn with_transfer(mut self, transfer: u8) -> Self {
        self.transfer = transfer;
        self
    }

    /// Set the matrix code point.
    pub fn with_matrix(mut self, matrix: u8) -> Self {
        self.matrix = matrix;
        self
    }

    /// `true` when either primaries or transfer is specified (`!= 2`) or
    /// the range is `Limited` — i.e. the payload said something the
    /// crate's default does not already imply.
    pub fn is_specified(&self) -> bool {
        self.primaries != Self::UNSPECIFIED
            || self.transfer != Self::UNSPECIFIED
            || self.range == ColorRange::Limited
    }
}

impl Default for ColorInfo {
    /// [`ColorInfo::ico_default`].
    fn default() -> Self {
        Self::ico_default()
    }
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

/// The metadata blobs every image crate surfaces. The ICO container has
/// none of its own; an embedded PNG sub-image contributes its `iCCP`
/// (`icc`), `eXIf` (`exif`), XMP `iTXt` (`xmp`) and `gAMA` (`gamma`, the
/// PNG file-gamma exponent, e.g. `0.45455`); a V5 DIB contributes an
/// embedded ICC profile. Classic DIB entries leave every field `None`.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct Metadata {
    /// Embedded ICC profile bytes.
    pub icc: Option<Vec<u8>>,
    /// Exif payload (TIFF header onwards).
    pub exif: Option<Vec<u8>>,
    /// XMP packet (UTF-8 XML).
    pub xmp: Option<Vec<u8>>,
    /// The single file-gamma exponent (PNG `gAMA` semantics).
    pub gamma: Option<f32>,
}

impl Metadata {
    /// Empty metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set (or clear) the ICC profile.
    pub fn with_icc(mut self, icc: impl Into<Option<Vec<u8>>>) -> Self {
        self.icc = icc.into();
        self
    }

    /// Set (or clear) the Exif payload.
    pub fn with_exif(mut self, exif: impl Into<Option<Vec<u8>>>) -> Self {
        self.exif = exif.into();
        self
    }

    /// Set (or clear) the XMP packet.
    pub fn with_xmp(mut self, xmp: impl Into<Option<Vec<u8>>>) -> Self {
        self.xmp = xmp.into();
        self
    }

    /// Set (or clear) the file gamma.
    pub fn with_gamma(mut self, gamma: impl Into<Option<f32>>) -> Self {
        self.gamma = gamma.into();
        self
    }

    /// `true` when no field is set.
    pub fn is_empty(&self) -> bool {
        self.icc.is_none() && self.exif.is_none() && self.xmp.is_none() && self.gamma.is_none()
    }
}

// ---------------------------------------------------------------------------
// IcoImage
// ---------------------------------------------------------------------------

/// One ICO / CUR sub-image in its native layout (always packed `Rgba`,
/// rows top-down — see the module docs), or an input image for the
/// encoder.
///
/// The contract fields (`width`, `height`, `format`, `planes`, `color`,
/// `metadata`) are shared with every OxideAV image crate. The three
/// extras record the sub-image's provenance so a decode → re-encode
/// cycle can stay faithful:
///
/// * [`bit_depth`](Self::bit_depth) — the on-disk bits per pixel (1 / 4
///   / 8 / 16 / 24 / 32; `32` for PNG entries). The encoder honours it
///   through [`crate::EncodeOptions::per_image_bit_depth`].
/// * [`sub_format`](Self::sub_format) — whether the entry was a DIB or
///   an embedded PNG. Advisory on encode (the size rule in
///   [`crate::EncodeOptions`] decides).
/// * [`hotspot`](Self::hotspot) — the cursor click point for CUR
///   entries, `None` for ICO entries.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct IcoImage {
    /// Sub-image width in pixels (`1..=256` for anything the encoder
    /// accepts).
    pub width: u32,
    /// Sub-image height in pixels (`1..=256` for anything the encoder
    /// accepts).
    pub height: u32,
    /// Native pixel layout — [`IcoPixelFormat::Rgba`].
    pub format: PixelFormat,
    /// Pixel planes — exactly one for ICO.
    pub planes: Vec<Plane>,
    /// Colour signalling of the sub-image payload (see [`ColorInfo`]).
    pub color: ColorInfo,
    /// Metadata the sub-image payload carried (see [`Metadata`]).
    pub metadata: Metadata,
    /// On-disk bits per pixel of the source entry (or the depth the
    /// encoder should target when `per_image_bit_depth` is set).
    pub bit_depth: u8,
    /// The source entry's encoding (DIB or PNG). Advisory on encode.
    pub sub_format: IconSubFormat,
    /// Cursor hotspot — `Some` for CUR entries, `None` for ICO entries.
    pub hotspot: Option<HotSpot>,
}

impl IcoImage {
    /// Assemble an image from its geometry, layout and planes (exactly
    /// one for ICO), validating the geometry: non-zero dimensions, one
    /// plane whose `stride` covers `width × bytes_per_pixel` and whose
    /// `data` covers `stride × height`. Colour is
    /// [`ColorInfo::ico_default`], metadata empty, `bit_depth` 32,
    /// `sub_format` [`IconSubFormat::Png`], no hotspot; the `with_*`
    /// builders fill those in.
    ///
    /// Dimensions above 256 are accepted here (an image may be built
    /// for other purposes); the encoder rejects them with
    /// [`IcoError::Unsupported`].
    pub fn new(width: u32, height: u32, format: PixelFormat, planes: Vec<Plane>) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(IcoError::invalid("ICO image: zero dimension"));
        }
        if planes.len() != 1 {
            return Err(IcoError::invalid(format!(
                "ICO image: expected exactly one plane, got {}",
                planes.len()
            )));
        }
        let plane = &planes[0];
        let min_stride = (width as usize).saturating_mul(format.bytes_per_pixel());
        if plane.stride < min_stride {
            return Err(IcoError::invalid(format!(
                "ICO image: stride {} is below the {} bytes a {}-pixel row of {:?} needs",
                plane.stride, min_stride, width, format
            )));
        }
        let needed = plane.stride.saturating_mul(height as usize);
        if plane.data.len() < needed {
            return Err(IcoError::invalid(format!(
                "ICO image: plane holds {} bytes, {} × {} rows need {}",
                plane.data.len(),
                plane.stride,
                height,
                needed
            )));
        }
        Ok(Self {
            width,
            height,
            format,
            planes,
            color: ColorInfo::ico_default(),
            metadata: Metadata::default(),
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
        })
    }

    /// One packed plane with an explicit row stride (validated like
    /// [`Self::new`]).
    pub fn packed(
        width: u32,
        height: u32,
        format: PixelFormat,
        stride: usize,
        data: Vec<u8>,
    ) -> Result<Self> {
        Self::new(width, height, format, vec![Plane::new(stride, data)])
    }

    /// Tightly packed `Rgb24` input: exactly `3 × width × height` bytes.
    /// ICO has no alpha-less native layout, so the pixels are widened
    /// to [`IcoPixelFormat::Rgba`] with opaque alpha (an exact, lossless
    /// step). Rejects a length / geometry mismatch with
    /// [`IcoError::InvalidData`].
    pub fn from_rgb8(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        let pixels = (width as usize).saturating_mul(height as usize);
        if width == 0 || height == 0 || data.len() != pixels.saturating_mul(3) {
            return Err(IcoError::invalid(format!(
                "ICO image: from_rgb8 expects 3 × {width} × {height} = {} bytes, got {}",
                pixels.saturating_mul(3),
                data.len()
            )));
        }
        let mut rgba = Vec::with_capacity(pixels * 4);
        for px in data.chunks_exact(3) {
            rgba.extend_from_slice(&[px[0], px[1], px[2], 0xFF]);
        }
        Self::packed(width, height, PixelFormat::Rgba, width as usize * 4, rgba)
    }

    /// Tightly packed `Rgba` from exactly `4 × width × height` bytes.
    /// Rejects a length / geometry mismatch with [`IcoError::InvalidData`].
    pub fn from_rgba8(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        let pixels = (width as usize).saturating_mul(height as usize);
        if width == 0 || height == 0 || data.len() != pixels.saturating_mul(4) {
            return Err(IcoError::invalid(format!(
                "ICO image: from_rgba8 expects 4 × {width} × {height} = {} bytes, got {}",
                pixels.saturating_mul(4),
                data.len()
            )));
        }
        Self::packed(width, height, PixelFormat::Rgba, width as usize * 4, data)
    }

    /// Set the colour signalling.
    pub fn with_color(mut self, color: ColorInfo) -> Self {
        self.color = color;
        self
    }

    /// Set the metadata blobs.
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Set the on-disk / target bit depth (see [`Self::bit_depth`]).
    pub fn with_bit_depth(mut self, bit_depth: u8) -> Self {
        self.bit_depth = bit_depth;
        self
    }

    /// Set the sub-format hint (see [`Self::sub_format`]).
    pub fn with_sub_format(mut self, sub_format: IconSubFormat) -> Self {
        self.sub_format = sub_format;
        self
    }

    /// Set (or clear) the cursor hotspot.
    pub fn with_hotspot(mut self, hotspot: impl Into<Option<HotSpot>>) -> Self {
        self.hotspot = hotspot.into();
        self
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Native layout.
    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// Bytes per pixel of the native layout.
    pub fn bytes_per_pixel(&self) -> usize {
        self.format.bytes_per_pixel()
    }

    /// Row stride of the single plane in bytes.
    pub fn stride(&self) -> usize {
        self.planes.first().map(|p| p.stride).unwrap_or(0)
    }

    /// The single packed plane's bytes (`Some` for every layout the
    /// crate has, since all are packed).
    pub fn as_bytes(&self) -> Option<&[u8]> {
        self.planes.first().map(|p| p.data.as_slice())
    }

    /// Consume the image and return the plane bytes (planes concatenated
    /// in order; one plane for ICO).
    pub fn into_raw(self) -> Vec<u8> {
        let mut planes = self.planes.into_iter();
        let mut out = planes.next().map(|p| p.data).unwrap_or_default();
        for p in planes {
            out.extend_from_slice(&p.data);
        }
        out
    }

    /// `true` — every ICO sub-image composes to RGBA.
    pub fn has_alpha(&self) -> bool {
        self.format.has_alpha()
    }

    /// Tightly packed `R, G, B` (3 bytes per pixel, rows top-down),
    /// alpha dropped. Exact for the native layout.
    pub fn to_rgb8(&self) -> Vec<u8> {
        let w = self.width as usize;
        let h = self.height as usize;
        let mut out = Vec::with_capacity(w * h * 3);
        if let Some(plane) = self.planes.first() {
            for y in 0..h {
                let row = &plane.data[y * plane.stride..y * plane.stride + w * 4];
                for px in row.chunks_exact(4) {
                    out.extend_from_slice(&px[..3]);
                }
            }
        }
        out
    }

    /// Tightly packed `R, G, B, A` (4 bytes per pixel, rows top-down).
    /// Exact for the native layout — a copy with any row padding
    /// removed.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let w = self.width as usize;
        let h = self.height as usize;
        let mut out = Vec::with_capacity(w * h * 4);
        if let Some(plane) = self.planes.first() {
            if plane.stride == w * 4 && plane.data.len() == w * h * 4 {
                return plane.data.clone();
            }
            for y in 0..h {
                out.extend_from_slice(&plane.data[y * plane.stride..y * plane.stride + w * 4]);
            }
        }
        out
    }

    /// Fallible [`Self::to_rgb8`] for defensive callers (always `Ok`
    /// on an image built through the validating constructors).
    pub fn try_to_rgb8(&self) -> Result<Vec<u8>> {
        self.check_geometry()?;
        Ok(self.to_rgb8())
    }

    /// Fallible [`Self::to_rgba8`] for defensive callers (always `Ok`
    /// on an image built through the validating constructors).
    pub fn try_to_rgba8(&self) -> Result<Vec<u8>> {
        self.check_geometry()?;
        Ok(self.to_rgba8())
    }

    /// Re-run the [`Self::new`] geometry checks on an existing value.
    pub(crate) fn check_geometry(&self) -> Result<()> {
        let plane = self
            .planes
            .first()
            .ok_or_else(|| IcoError::invalid("ICO image: no plane"))?;
        if self.planes.len() != 1 {
            return Err(IcoError::invalid("ICO image: expected exactly one plane"));
        }
        let min_stride = (self.width as usize).saturating_mul(self.format.bytes_per_pixel());
        if self.width == 0 || self.height == 0 || plane.stride < min_stride {
            return Err(IcoError::invalid("ICO image: invalid geometry"));
        }
        if plane.data.len() < plane.stride.saturating_mul(self.height as usize) {
            return Err(IcoError::invalid("ICO image: plane too short"));
        }
        Ok(())
    }

    /// The tightly packed RGBA bytes of a decoder-produced image
    /// (stride `4 × width`); falls back to a repacked copy otherwise.
    pub(crate) fn rgba_cow(&self) -> std::borrow::Cow<'_, [u8]> {
        let w = self.width as usize;
        let h = self.height as usize;
        match self.planes.first() {
            Some(p) if p.stride == w * 4 && p.data.len() == w * h * 4 => {
                std::borrow::Cow::Borrowed(&p.data)
            }
            _ => std::borrow::Cow::Owned(self.to_rgba8()),
        }
    }
}

// ---------------------------------------------------------------------------
// RgbImage / RgbaImage
// ---------------------------------------------------------------------------

/// Tightly packed 8-bit RGB (3 bytes per pixel, rows top-down).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RgbImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `3 × width × height` bytes.
    pub data: Vec<u8>,
}

impl RgbImage {
    /// Wrap a packed RGB buffer.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Self {
        Self {
            width,
            height,
            data,
        }
    }

    /// The pixel bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Consume and return the pixel bytes.
    pub fn into_raw(self) -> Vec<u8> {
        self.data
    }

    /// Bytes per row (`3 × width`).
    pub fn stride(&self) -> usize {
        self.width as usize * 3
    }
}

/// Tightly packed 8-bit RGBA (4 bytes per pixel, rows top-down).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RgbaImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `4 × width × height` bytes.
    pub data: Vec<u8>,
}

impl RgbaImage {
    /// Wrap a packed RGBA buffer.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Self {
        Self {
            width,
            height,
            data,
        }
    }

    /// The pixel bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Consume and return the pixel bytes.
    pub fn into_raw(self) -> Vec<u8> {
        self.data
    }

    /// Bytes per row (`4 × width`).
    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }
}

// ---------------------------------------------------------------------------
// ImageInfo
// ---------------------------------------------------------------------------

/// One directory row as [`info`](crate::info) reports it — the facts
/// [`crate::read_ico_raw`] recovers from the `ICONDIRENTRY` and the
/// payload header, without decoding any pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct IcoEntryInfo {
    /// Width in pixels (`0 → 256` convention already applied, PNG IHDR
    /// / DIB header cross-checked).
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bits per pixel the entry advertises.
    pub bit_depth: u8,
    /// Payload encoding.
    pub sub_format: IconSubFormat,
    /// Cursor hotspot (CUR files only).
    pub hotspot: Option<HotSpot>,
    /// Payload size in bytes (`dwBytesInRes`).
    pub size: u32,
}

impl IcoEntryInfo {
    /// Build a row description.
    pub fn new(
        width: u32,
        height: u32,
        bit_depth: u8,
        sub_format: IconSubFormat,
        hotspot: Option<HotSpot>,
        size: u32,
    ) -> Self {
        Self {
            width,
            height,
            bit_depth,
            sub_format,
            hotspot,
            size,
        }
    }
}

/// Header-level facts about an ICO / CUR file, from
/// [`info`](crate::info): the directory walk plus the primary
/// ("best") entry's geometry. No pixel is decoded.
///
/// The contract fields describe the primary entry (the one
/// [`decode`](crate::decode) returns under default options:
/// [`crate::EntrySelection::Largest`]); `frames` is the number of
/// directory entries. ICO-specific extras: the container type, the
/// primary entry's index / depth / encoding / hotspot, and every row.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ImageInfo {
    /// Primary entry width in pixels.
    pub width: u32,
    /// Primary entry height in pixels.
    pub height: u32,
    /// Native layout of a decoded sub-image — always `Rgba`.
    pub format: PixelFormat,
    /// Number of directory entries (what [`crate::decode_all`] returns).
    pub frames: u32,
    /// `true` — every ICO sub-image composes to RGBA.
    pub has_alpha: bool,
    /// Colour signalling of the primary entry's payload (header only:
    /// an embedded PNG's chunks are parsed, a DIB reports the
    /// [`ColorInfo::ico_default`]).
    pub color: ColorInfo,
    /// Primary entry payload carries an ICC profile.
    pub has_icc: bool,
    /// Primary entry payload carries Exif.
    pub has_exif: bool,
    /// Primary entry payload carries XMP.
    pub has_xmp: bool,
    /// Icon (`1`) or cursor (`2`) directory.
    pub icon_type: IconType,
    /// Index of the primary entry in `entries` (and in
    /// [`crate::decode_all`]'s output).
    pub primary: u32,
    /// Primary entry bits per pixel.
    pub bit_depth: u8,
    /// Primary entry encoding.
    pub sub_format: IconSubFormat,
    /// Primary entry hotspot (CUR only).
    pub hotspot: Option<HotSpot>,
    /// Every directory row, in file order.
    pub entries: Vec<IcoEntryInfo>,
}

impl ImageInfo {
    /// A description with the geometry set and every other field at its
    /// default (`frames` 1, no metadata, icon type `Ico`, no rows).
    pub fn new(width: u32, height: u32, format: PixelFormat) -> Self {
        Self {
            width,
            height,
            format,
            frames: 1,
            has_alpha: format.has_alpha(),
            color: ColorInfo::ico_default(),
            has_icc: false,
            has_exif: false,
            has_xmp: false,
            icon_type: IconType::Ico,
            primary: 0,
            bit_depth: 32,
            sub_format: IconSubFormat::Png,
            hotspot: None,
            entries: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Frame
// ---------------------------------------------------------------------------

/// One directory entry as [`decode_all`](crate::decode_all) returns it
/// (and as [`encode_all`](crate::encode_all) consumes it).
///
/// ICO entries are alternative renditions, not an animation, so
/// `delay` is always `None` on decode and ignored on encode. The
/// `index` extra is the entry's position in the directory; the
/// hotspot, bit depth and encoding live on the image.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Frame {
    /// The decoded sub-image.
    pub image: IcoImage,
    /// Always `None` — ICO entries have no timing.
    pub delay: Option<Duration>,
    /// Position of the entry in the directory (0-based).
    pub index: u32,
}

impl Frame {
    /// Wrap a sub-image as directory entry `index`.
    pub fn new(image: IcoImage, index: u32) -> Self {
        Self {
            image,
            delay: None,
            index,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_validates_geometry() {
        assert!(IcoImage::new(0, 1, PixelFormat::Rgba, vec![Plane::new(4, vec![0; 4])]).is_err());
        assert!(IcoImage::new(1, 1, PixelFormat::Rgba, vec![]).is_err());
        assert!(IcoImage::new(2, 1, PixelFormat::Rgba, vec![Plane::new(4, vec![0; 4])]).is_err());
        assert!(IcoImage::new(1, 2, PixelFormat::Rgba, vec![Plane::new(4, vec![0; 4])]).is_err());
        let ok = IcoImage::new(
            1,
            1,
            PixelFormat::Rgba,
            vec![Plane::new(4, vec![1, 2, 3, 4])],
        );
        let ok = ok.unwrap();
        assert_eq!(ok.as_bytes(), Some(&[1u8, 2, 3, 4][..]));
        assert_eq!(ok.into_raw(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn from_rgb8_widens_with_opaque_alpha() {
        let im = IcoImage::from_rgb8(2, 1, vec![1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!(im.format(), PixelFormat::Rgba);
        assert_eq!(im.to_rgba8(), vec![1, 2, 3, 255, 4, 5, 6, 255]);
        assert_eq!(im.to_rgb8(), vec![1, 2, 3, 4, 5, 6]);
        assert!(IcoImage::from_rgb8(2, 1, vec![0; 5]).is_err());
        assert!(IcoImage::from_rgba8(2, 1, vec![0; 7]).is_err());
    }

    #[test]
    fn padded_stride_is_removed_by_to_rgba8() {
        // 1-pixel row with 4 bytes of padding per row.
        let data = vec![9, 8, 7, 6, 0, 0, 0, 0, 5, 4, 3, 2, 0, 0, 0, 0];
        let im = IcoImage::packed(1, 2, PixelFormat::Rgba, 8, data).unwrap();
        assert_eq!(im.to_rgba8(), vec![9, 8, 7, 6, 5, 4, 3, 2]);
        assert_eq!(im.to_rgb8(), vec![9, 8, 7, 5, 4, 3]);
        assert_eq!(im.try_to_rgba8().unwrap().len(), 8);
    }

    #[test]
    fn color_defaults_and_specified() {
        assert!(!ColorInfo::ico_default().is_specified());
        assert!(ColorInfo::srgb().is_specified());
        assert!(!ColorInfo::unspecified().is_specified());
        assert!(ColorInfo::unspecified()
            .with_range(ColorRange::Limited)
            .is_specified());
        assert_eq!(ColorInfo::default(), ColorInfo::ico_default());
        assert!(Metadata::new().is_empty());
        assert!(!Metadata::new().with_gamma(0.45455).is_empty());
    }
}
