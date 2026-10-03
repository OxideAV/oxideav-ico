//! Decode-side limits / entry selection ([`DecodeOptions`]) and the
//! encoder knobs ([`EncodeOptions`]).

use crate::error::{IcoError, Result};
use crate::types::{BmpBitDepth, IconType};

// ---------------------------------------------------------------------------
// DecodeOptions
// ---------------------------------------------------------------------------

/// Which directory entry [`decode`](crate::decode) returns from a
/// multi-resolution icon.
///
/// The selectors are the same heuristics the `select_*` family offers
/// (`select_largest`, `select_best_fit`, `select_by_dimensions`), run
/// over the directory rows *before* any payload is decoded — the order
/// Windows' `LookupIconIdFromDirectoryEx` resolves an icon in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum EntrySelection {
    /// The largest entry by pixel area; the highest bit depth breaks a
    /// size tie. The default — the highest-fidelity rendition, what a
    /// thumbnailer wants.
    #[default]
    Largest,
    /// The entry at this directory index (0-based). Out of range is an
    /// [`IcoError::InvalidData`].
    Index(u32),
    /// The smallest entry whose longer side is at least this many
    /// pixels (downscale-only fit), falling back to the largest entry
    /// when every row is smaller. Highest bit depth breaks ties.
    BestFit(u32),
    /// The entry whose stored size is exactly `width × height`
    /// (highest bit depth breaks ties). No match is an
    /// [`IcoError::InvalidData`].
    Dimensions(u32, u32),
}

/// Limits and strictness for the decode path. `None` means unlimited;
/// the defaults are finite.
///
/// The limits apply to each decoded sub-image's geometry (`max_width` /
/// `max_height` / `max_pixels`) and to the total decoded RGBA bytes a
/// call produces (`max_bytes`: one sub-image for `decode`, the sum for
/// `decode_all`). They are checked against the directory rows and
/// payload headers *before* any pixel buffer is allocated, and
/// forwarded to the `oxideav-png` / `oxideav-bmp` sub-image decoders so
/// an entry whose PNG `IHDR` disagrees with its directory row cannot
/// bypass them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodeOptions {
    /// Reject a sub-image wider than this.
    pub max_width: Option<u32>,
    /// Reject a sub-image taller than this.
    pub max_height: Option<u32>,
    /// Reject a sub-image with more pixels than this.
    pub max_pixels: Option<u64>,
    /// Reject a call whose decoded RGBA output would exceed this many
    /// bytes.
    pub max_bytes: Option<u64>,
    /// Strict mode: refuse bytes after the last payload (trailing
    /// garbage) and a payload whose declared `dwBytesInRes` extends
    /// past the sub-image it holds. Default `false` tolerates both,
    /// as Windows does.
    pub strict: bool,
    /// Which entry [`decode`](crate::decode) returns (ignored by
    /// [`decode_all`](crate::decode_all)).
    pub entry: EntrySelection,
}

impl DecodeOptions {
    /// Default `max_bytes`: 1 GiB of decoded RGBA.
    pub const DEFAULT_MAX_BYTES: u64 = 1 << 30;

    /// The defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set (or clear) the width limit.
    pub fn with_max_width(mut self, max_width: impl Into<Option<u32>>) -> Self {
        self.max_width = max_width.into();
        self
    }

    /// Set (or clear) the height limit.
    pub fn with_max_height(mut self, max_height: impl Into<Option<u32>>) -> Self {
        self.max_height = max_height.into();
        self
    }

    /// Set (or clear) the pixel-count limit.
    pub fn with_max_pixels(mut self, max_pixels: impl Into<Option<u64>>) -> Self {
        self.max_pixels = max_pixels.into();
        self
    }

    /// Set (or clear) the decoded-bytes limit.
    pub fn with_max_bytes(mut self, max_bytes: impl Into<Option<u64>>) -> Self {
        self.max_bytes = max_bytes.into();
        self
    }

    /// Set strict mode.
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Choose which entry [`decode`](crate::decode) returns.
    pub fn with_entry(mut self, entry: EntrySelection) -> Self {
        self.entry = entry;
        self
    }

    /// Clear every limit.
    pub fn unlimited(mut self) -> Self {
        self.max_width = None;
        self.max_height = None;
        self.max_pixels = None;
        self.max_bytes = None;
        self
    }

    /// Check one sub-image's geometry against the limits; `bytes` is the
    /// running decoded-output total the call has committed to so far
    /// (including this sub-image).
    pub(crate) fn check(&self, width: u32, height: u32, bytes: u64) -> Result<()> {
        if let Some(m) = self.max_width {
            if width > m {
                return Err(IcoError::limit(format!(
                    "ICO: sub-image width {width} exceeds max_width {m}"
                )));
            }
        }
        if let Some(m) = self.max_height {
            if height > m {
                return Err(IcoError::limit(format!(
                    "ICO: sub-image height {height} exceeds max_height {m}"
                )));
            }
        }
        let pixels = u64::from(width) * u64::from(height);
        if let Some(m) = self.max_pixels {
            if pixels > m {
                return Err(IcoError::limit(format!(
                    "ICO: sub-image of {pixels} pixels exceeds max_pixels {m}"
                )));
            }
        }
        if let Some(m) = self.max_bytes {
            if bytes > m {
                return Err(IcoError::limit(format!(
                    "ICO: decoded output of {bytes} bytes exceeds max_bytes {m}"
                )));
            }
        }
        Ok(())
    }

    /// The same limits as `oxideav-png` options, so a PNG payload is
    /// bounded by its own decoder as well.
    pub(crate) fn png(&self) -> oxideav_png::DecodeOptions {
        oxideav_png::DecodeOptions::default()
            .with_max_width(self.max_width)
            .with_max_height(self.max_height)
            .with_max_pixels(self.max_pixels)
            .with_max_bytes(self.max_bytes)
            .with_strict(self.strict)
    }

    /// The same limits as `oxideav-bmp` options for a DIB payload.
    pub(crate) fn bmp(&self) -> oxideav_bmp::DecodeOptions {
        oxideav_bmp::DecodeOptions::default()
            .with_max_width(self.max_width)
            .with_max_height(self.max_height)
            .with_max_pixels(self.max_pixels)
            .with_max_bytes(self.max_bytes)
            .with_strict(self.strict)
    }
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            max_width: None,
            max_height: None,
            max_pixels: None,
            max_bytes: Some(Self::DEFAULT_MAX_BYTES),
            strict: false,
            entry: EntrySelection::Largest,
        }
    }
}

// ---------------------------------------------------------------------------
// EncodeOptions
// ---------------------------------------------------------------------------

/// Encoder knobs. Behaviour variants are fields, never function
/// suffixes.
///
/// Defaults follow the convention the format reference records
/// (`docs/image/ico/ico-cur-format.md`: PNG payloads are "used for large
/// icons (typically 256×256)"): an entry whose smaller side is at least
/// [`png_size_threshold`](Self::png_size_threshold) (`256`) is written
/// as an embedded PNG, everything smaller as a 32-bpp BGRA DIB with an
/// alpha-derived AND mask — the lossless form every Windows version
/// renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct EncodeOptions {
    /// Directory type to write: [`IconType::Ico`] (default) or
    /// [`IconType::Cur`]. A CUR entry's hotspot comes from
    /// [`crate::IcoImage::hotspot`] (`(0, 0)` when `None`).
    pub icon_type: IconType,
    /// When `Some(n)`, use PNG for any sub-image whose smaller
    /// dimension is `>= n`; else a DIB. `None` forces a DIB on every
    /// sub-image (legacy / maximum-compat write). `Some(1)` forces PNG
    /// everywhere.
    ///
    /// Default `Some(256)`.
    pub png_size_threshold: Option<u32>,
    /// Bit depth for the DIB path. Default [`BmpBitDepth::Bgra32`]
    /// (lossless). The lower depths quantise each DIB-bound sub-image
    /// by exact-colour collection and fail with
    /// [`IcoError::Unsupported`] when the image needs more colours
    /// than the depth holds. Ignored for PNG-routed entries, and
    /// overridden per image when [`per_image_bit_depth`](Self::per_image_bit_depth)
    /// is set.
    pub bmp_bit_depth: BmpBitDepth,
    /// When `true`, each DIB-bound sub-image is encoded at the depth
    /// its own [`crate::IcoImage::bit_depth`] names (`1 / 4 / 8 / 24 /
    /// 32`), so one call emits a faithful mixed-depth icon — what a
    /// decode → edit → re-encode cycle wants. A depth the writer has no
    /// encoder for (`16`, or anything else) falls back to
    /// [`bmp_bit_depth`](Self::bmp_bit_depth). Default `false`.
    pub per_image_bit_depth: bool,
    /// Embed the image's ICC profile / Exif / XMP in PNG-routed entries
    /// (DIB entries cannot carry them). Default `true`.
    pub embed_metadata: bool,
}

impl EncodeOptions {
    /// Default PNG routing threshold (smaller side, pixels).
    pub const DEFAULT_PNG_SIZE_THRESHOLD: u32 = 256;

    /// The defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the directory type.
    pub fn with_icon_type(mut self, icon_type: IconType) -> Self {
        self.icon_type = icon_type;
        self
    }

    /// Set (or clear) the PNG routing threshold.
    pub fn with_png_size_threshold(mut self, threshold: impl Into<Option<u32>>) -> Self {
        self.png_size_threshold = threshold.into();
        self
    }

    /// Set the DIB bit depth.
    pub fn with_bmp_bit_depth(mut self, depth: BmpBitDepth) -> Self {
        self.bmp_bit_depth = depth;
        self
    }

    /// Set per-image depth routing.
    pub fn with_per_image_bit_depth(mut self, per_image: bool) -> Self {
        self.per_image_bit_depth = per_image;
        self
    }

    /// Set metadata embedding for PNG entries.
    pub fn with_embed_metadata(mut self, embed: bool) -> Self {
        self.embed_metadata = embed;
        self
    }

    /// `true` when a `width × height` sub-image routes to PNG.
    pub(crate) fn routes_to_png(&self, width: u32, height: u32) -> bool {
        match self.png_size_threshold {
            None => false,
            Some(t) => width.min(height) >= t,
        }
    }
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            icon_type: IconType::Ico,
            png_size_threshold: Some(Self::DEFAULT_PNG_SIZE_THRESHOLD),
            bmp_bit_depth: BmpBitDepth::Bgra32,
            per_image_bit_depth: false,
            embed_metadata: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_limits_default_and_check() {
        let d = DecodeOptions::default();
        assert_eq!(d.max_bytes, Some(1 << 30));
        assert!(d.max_width.is_none());
        assert!(!d.strict);
        assert_eq!(d.entry, EntrySelection::Largest);
        assert!(d.check(256, 256, 256 * 256 * 4).is_ok());
        let tight = DecodeOptions::new().with_max_width(16);
        assert!(matches!(
            tight.check(17, 1, 0),
            Err(IcoError::LimitExceeded(_))
        ));
        let bytes = DecodeOptions::new().with_max_bytes(10);
        assert!(matches!(
            bytes.check(1, 1, 11),
            Err(IcoError::LimitExceeded(_))
        ));
        assert!(DecodeOptions::new()
            .unlimited()
            .check(u32::MAX, u32::MAX, u64::MAX)
            .is_ok());
    }

    #[test]
    fn encode_defaults_follow_the_256_convention() {
        let e = EncodeOptions::default();
        assert_eq!(e.png_size_threshold, Some(256));
        assert!(e.routes_to_png(256, 256));
        assert!(!e.routes_to_png(255, 256));
        assert!(!e.with_png_size_threshold(None).routes_to_png(256, 256));
        assert!(EncodeOptions::new()
            .with_png_size_threshold(1)
            .routes_to_png(1, 1));
        assert_eq!(e.icon_type, IconType::Ico);
        assert_eq!(e.bmp_bit_depth, BmpBitDepth::Bgra32);
    }
}
