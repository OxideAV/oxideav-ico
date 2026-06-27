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

    /// Builder-style setter for [`IconImage::bit_depth`] — the BMP depth
    /// the writer should target for this sub-image when
    /// [`WriteOptions::per_image_bit_depth`] is enabled. Returns `self`
    /// so it chains off [`IconImage::from_rgba`]:
    ///
    /// ```
    /// # use oxideav_ico::IconImage;
    /// let im = IconImage::from_rgba(16, 16, vec![0; 16 * 16 * 4]).with_bit_depth(8);
    /// assert_eq!(im.bit_depth, 8);
    /// ```
    pub fn with_bit_depth(mut self, bit_depth: u8) -> Self {
        self.bit_depth = bit_depth;
        self
    }

    /// Builder-style setter for [`IconImage::hotspot`] (CUR entries).
    /// Returns `self` so it chains off [`IconImage::from_rgba`].
    pub fn with_hotspot(mut self, hotspot: HotSpot) -> Self {
        self.hotspot = Some(hotspot);
        self
    }
}

/// Bit depth to use when the writer emits a BMP-DIB sub-image. The
/// default `Bgra32` matches what `write_ico` historically produced; the
/// lower depths drive the classic indexed / true-colour `ICONIMAGE` DIB
/// forms (palette + XOR + AND mask) for compact, legacy-faithful icons.
///
/// PNG sub-images always carry full RGBA regardless of this setting —
/// it only governs the BMP path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BmpBitDepth {
    /// 32-bpp BGRA — the modern, lossless default (alpha in the colour
    /// bits, all-zero AND mask). No colour or transparency limits.
    #[default]
    Bgra32,
    /// 24-bpp true-colour BGR with a 1-bpp AND mask for transparency.
    /// Drops the alpha channel to a hard on/off mask; full colour range.
    Rgb24,
    /// 8-bpp indexed (≤ 256 distinct opaque colours) with a palette and
    /// 1-bpp AND mask. The writer builds the palette by exact-colour
    /// collection and errors if the image needs more than 256 colours.
    Indexed8,
    /// 4-bpp indexed (≤ 16 distinct opaque colours).
    Indexed4,
    /// 1-bpp indexed (≤ 2 distinct opaque colours — classic monochrome).
    Indexed1,
}

impl BmpBitDepth {
    /// Bits-per-pixel value this depth writes into the directory + DIB
    /// header.
    pub fn bits(self) -> u8 {
        match self {
            BmpBitDepth::Bgra32 => 32,
            BmpBitDepth::Rgb24 => 24,
            BmpBitDepth::Indexed8 => 8,
            BmpBitDepth::Indexed4 => 4,
            BmpBitDepth::Indexed1 => 1,
        }
    }

    /// The indexed bit depth (1/4/8) when this is an indexed variant,
    /// else `None` (32/24-bpp are direct-colour).
    pub fn indexed_bpp(self) -> Option<u8> {
        match self {
            BmpBitDepth::Indexed8 => Some(8),
            BmpBitDepth::Indexed4 => Some(4),
            BmpBitDepth::Indexed1 => Some(1),
            _ => None,
        }
    }

    /// Map a directory/DIB `biBitCount` value to a [`BmpBitDepth`].
    /// Recognises the legal ICO depths `1/4/8/24/32`; `16` (which the
    /// reader accepts but the writer has no encoder for) and any other
    /// value map to `None`. Used by the per-image-depth write path to
    /// turn a decoded [`IconImage::bit_depth`] back into an encode
    /// choice for a faithful round-trip.
    pub fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            1 => Some(BmpBitDepth::Indexed1),
            4 => Some(BmpBitDepth::Indexed4),
            8 => Some(BmpBitDepth::Indexed8),
            24 => Some(BmpBitDepth::Rgb24),
            32 => Some(BmpBitDepth::Bgra32),
            _ => None,
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
    /// Bit depth for the BMP-DIB path. Default [`BmpBitDepth::Bgra32`]
    /// (the historical 32-bpp output). Set to an indexed or 24-bpp
    /// variant to emit compact, legacy-faithful sub-images; the writer
    /// quantises each BMP-bound sub-image to the requested depth and
    /// errors if an indexed depth can't hold the image's colour count.
    /// Ignored for any sub-image the size threshold routes to PNG, and
    /// overridden per-image when [`WriteOptions::per_image_bit_depth`]
    /// is set.
    pub bmp_bit_depth: BmpBitDepth,
    /// When `true`, each BMP-bound sub-image is encoded at the depth its
    /// own [`IconImage::bit_depth`] field names (via
    /// [`BmpBitDepth::from_bits`]) instead of the single
    /// [`WriteOptions::bmp_bit_depth`]. This lets one `write_ico` call
    /// emit a faithful **mixed-depth** multi-resolution icon — e.g. a
    /// decoded `.ico` carrying a legacy 1-bpp 16×16 next to a 32-bpp
    /// 32×32 re-encodes each entry at its original depth. A
    /// `bit_depth` the writer can't encode (`16`, or anything outside
    /// `1/4/8/24/32`) falls back to [`WriteOptions::bmp_bit_depth`].
    ///
    /// Default `false` — every BMP sub-image uses the single
    /// `bmp_bit_depth`, preserving the historical behaviour.
    pub per_image_bit_depth: bool,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            png_size_threshold: Some(64),
            bmp_bit_depth: BmpBitDepth::Bgra32,
            per_image_bit_depth: false,
        }
    }
}

/// The three directory-row facts the sub-image selectors compare on:
/// pixel dimensions plus advertised bit depth. Both [`IconImage`]
/// (decoded RGBA) and [`IconEntryRaw`] (undecoded directory row +
/// payload bytes) expose these, so the selection heuristics work over
/// either without forcing the caller to decode every body just to pick
/// one. The free `select_*` functions take `&[IconImage]`; the
/// `select_*_raw` functions take `&[IconEntryRaw]` and share the exact
/// same tie-break rules through this trait.
pub(crate) trait Selectable {
    fn sel_width(&self) -> u32;
    fn sel_height(&self) -> u32;
    fn sel_bit_depth(&self) -> u8;
}

impl Selectable for IconImage {
    fn sel_width(&self) -> u32 {
        self.width
    }
    fn sel_height(&self) -> u32 {
        self.height
    }
    fn sel_bit_depth(&self) -> u8 {
        self.bit_depth
    }
}

impl Selectable for crate::raw::IconEntryRaw {
    fn sel_width(&self) -> u32 {
        self.width
    }
    fn sel_height(&self) -> u32 {
        self.height
    }
    fn sel_bit_depth(&self) -> u8 {
        self.bit_depth
    }
}

/// Generic `select_best_fit` over any [`Selectable`] slice. The public
/// `select_best_fit` / `select_best_fit_raw` are thin wrappers.
pub(crate) fn select_best_fit_impl<T: Selectable>(items: &[T], target: u32) -> Option<usize> {
    if items.is_empty() {
        return None;
    }
    // First pass: smallest entry whose max-dim is ≥ target.
    let mut best: Option<usize> = None;
    for (i, im) in items.iter().enumerate() {
        let max_dim = im.sel_width().max(im.sel_height());
        if max_dim < target {
            continue;
        }
        let better = match best {
            None => true,
            Some(j) => {
                let cur = items[j].sel_width().max(items[j].sel_height());
                match max_dim.cmp(&cur) {
                    core::cmp::Ordering::Less => true,
                    core::cmp::Ordering::Equal => im.sel_bit_depth() > items[j].sel_bit_depth(),
                    core::cmp::Ordering::Greater => false,
                }
            }
        };
        if better {
            best = Some(i);
        }
    }
    if best.is_some() {
        return best;
    }
    // Fallback: every entry is smaller than `target`. Return the
    // largest one — the closest we have without making the user
    // upscale a tiny entry.
    Some(select_largest_impl(items).expect("non-empty slice has a largest entry"))
}

/// Generic `select_largest` over any [`Selectable`] slice.
pub(crate) fn select_largest_impl<T: Selectable>(items: &[T]) -> Option<usize> {
    if items.is_empty() {
        return None;
    }
    let mut best = 0;
    for i in 1..items.len() {
        let im = &items[i];
        let cur = &items[best];
        let im_area = (im.sel_width() as u64) * (im.sel_height() as u64);
        let cur_area = (cur.sel_width() as u64) * (cur.sel_height() as u64);
        let pick = match im_area.cmp(&cur_area) {
            core::cmp::Ordering::Greater => true,
            core::cmp::Ordering::Equal => im.sel_bit_depth() > cur.sel_bit_depth(),
            core::cmp::Ordering::Less => false,
        };
        if pick {
            best = i;
        }
    }
    Some(best)
}

/// Generic `select_by_dimensions` over any [`Selectable`] slice.
pub(crate) fn select_by_dimensions_impl<T: Selectable>(
    items: &[T],
    width: u32,
    height: u32,
) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, im) in items.iter().enumerate() {
        if im.sel_width() != width || im.sel_height() != height {
            continue;
        }
        let better = match best {
            None => true,
            Some(j) => im.sel_bit_depth() > items[j].sel_bit_depth(),
        };
        if better {
            best = Some(i);
        }
    }
    best
}

/// Pick the sub-image that best fits a target render size, returning
/// its index in `images`. Returns `None` only when the slice is empty.
///
/// The heuristic matches the spirit of Windows' `LookupIconIdFromDirectoryEx`:
///
/// 1. Prefer the smallest entry whose largest dimension is **≥**
///    `target`. That gives a sub-image that can be downscaled to fit
///    without ever needing to upscale — always sharper than blowing up
///    a smaller entry. Among ties, the highest bit depth wins (so a
///    32-bpp entry beats a 1-bpp entry at the same resolution).
/// 2. If every entry is smaller than `target`, fall back to the
///    largest entry (closest to the target without overshooting in
///    the "no bigger entry exists" sense). Same bit-depth tiebreaker.
///
/// `target` is in pixels and applies to the longer side of each
/// candidate; this matches how shell icons are sized (a 32×32 entry
/// fits any target up to 32 px).
///
/// See [`select_best_fit_raw`] for the directory-level variant that
/// runs the same heuristic over undecoded [`IconEntryRaw`] rows — so a
/// caller can pick a sub-image *before* spending a PNG / BMP decode on
/// it (exactly the order Windows' `LookupIconIdFromDirectoryEx` works
/// in).
pub fn select_best_fit(images: &[IconImage], target: u32) -> Option<usize> {
    select_best_fit_impl(images, target)
}

/// Pick the largest sub-image by pixel area. Ties broken by bit depth
/// (higher wins). Returns `None` only when the slice is empty.
///
/// Useful when the caller wants the highest-fidelity entry regardless
/// of render size — e.g. extracting the canonical "main" icon from a
/// multi-resolution `.ico` to feed a thumbnail pipeline.
///
/// See [`select_largest_raw`] for the directory-level variant over
/// undecoded [`IconEntryRaw`] rows.
pub fn select_largest(images: &[IconImage]) -> Option<usize> {
    select_largest_impl(images)
}

/// Find the sub-image whose stored dimensions are **exactly**
/// `width × height`, returning its index in `images`. Returns `None`
/// when no entry matches that size.
///
/// Multi-resolution `.ico` files routinely carry the same nominal
/// size at more than one bit depth (e.g. a legacy 1-bpp 32×32 next to
/// a 32-bpp 32×32). When several entries share the requested size the
/// highest bit depth wins — the same tiebreaker [`select_best_fit`]
/// and [`select_largest`] use — so callers asking for "the 32×32"
/// get the modern entry, not the legacy one.
///
/// Unlike [`select_best_fit`], this is a strict equality lookup: it
/// never substitutes a larger entry to be downscaled. Use it when the
/// caller needs a pixel-exact sub-image (a UI slot that wants
/// precisely 256×256 and would rather fail than rescale) and
/// [`select_best_fit`] when a nearest-fit-with-downscale is
/// acceptable.
///
/// See [`select_by_dimensions_raw`] for the directory-level variant
/// over undecoded [`IconEntryRaw`] rows.
pub fn select_by_dimensions(images: &[IconImage], width: u32, height: u32) -> Option<usize> {
    select_by_dimensions_impl(images, width, height)
}

// ---------------------------------------------------------------------------
// Directory-level (raw) selection.
//
// These mirror the decoded `select_*` family but run against the
// undecoded [`IconEntryRaw`] directory rows `read_ico_raw` produces.
// Windows' own `LookupIconIdFromDirectoryEx` picks a directory entry
// from its `bWidth` / `bHeight` / `wBitCount` *before* the sub-image
// body is ever decoded; these helpers let a caller follow that order —
// select the entry, then decode only the chosen body — instead of
// decoding every PNG / BMP sub-image just to call the decoded
// `select_*` family.
//
// The dimension and bit-depth values come straight from the directory
// row (with the `bWidth`/`bHeight` `0 → 256` convention already applied
// by `read_ico_raw`), so the selection is byte-cheap: no payload is
// touched. The tie-break rules are identical to the decoded variants
// (highest bit depth wins at equal size), since both delegate to the
// same generic core.
// ---------------------------------------------------------------------------

/// Directory-level [`select_best_fit`]: pick the [`IconEntryRaw`] whose
/// stored size best fits `target`, returning its index in `entries`.
///
/// Identical heuristic to [`select_best_fit`] — smallest entry whose
/// longer side is `≥ target`, falling back to the largest entry when
/// every row is smaller, with the highest-bit-depth tie-break — but
/// run over the undecoded directory rows from [`read_ico_raw`](crate::read_ico_raw).
/// A shell-style caller that knows its render size can pick the right
/// sub-image first and then decode only that one entry's payload bytes,
/// matching the order Windows' `LookupIconIdFromDirectoryEx` resolves
/// an icon in.
///
/// Returns `None` only when `entries` is empty.
pub fn select_best_fit_raw(entries: &[crate::raw::IconEntryRaw], target: u32) -> Option<usize> {
    select_best_fit_impl(entries, target)
}

/// Directory-level [`select_largest`]: pick the largest-area
/// [`IconEntryRaw`] (highest-bit-depth tie-break) without decoding any
/// payload. Returns `None` only when `entries` is empty.
pub fn select_largest_raw(entries: &[crate::raw::IconEntryRaw]) -> Option<usize> {
    select_largest_impl(entries)
}

/// Directory-level [`select_by_dimensions`]: strict pixel-exact lookup
/// over the undecoded [`IconEntryRaw`] rows. Returns the index of the
/// row whose stored `width × height` equals the request (highest bit
/// depth wins when several rows share that size), or `None` when no row
/// matches — no nearest-fit substitution (that's [`select_best_fit_raw`]).
pub fn select_by_dimensions_raw(
    entries: &[crate::raw::IconEntryRaw],
    width: u32,
    height: u32,
) -> Option<usize> {
    select_by_dimensions_impl(entries, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32, bpp: u8) -> IconImage {
        let mut im = IconImage::from_rgba(w, h, vec![0u8; (w * h * 4) as usize]);
        im.bit_depth = bpp;
        im
    }

    #[test]
    fn select_largest_empty() {
        assert!(select_largest(&[]).is_none());
    }

    #[test]
    fn select_largest_picks_max_area() {
        let images = [img(16, 16, 32), img(64, 64, 32), img(32, 32, 32)];
        assert_eq!(select_largest(&images), Some(1));
    }

    #[test]
    fn select_largest_breaks_tie_on_bpp() {
        // Two 32×32 entries — one 1-bpp legacy, one 32-bpp modern.
        // Higher bit depth wins.
        let images = [img(32, 32, 1), img(32, 32, 32)];
        assert_eq!(select_largest(&images), Some(1));
    }

    #[test]
    fn select_best_fit_empty() {
        assert!(select_best_fit(&[], 32).is_none());
    }

    #[test]
    fn select_best_fit_smallest_entry_at_or_above_target() {
        // Target 32; entries 16, 32, 64, 128. Picks 32 — smallest
        // entry that's ≥ target.
        let images = [
            img(16, 16, 32),
            img(32, 32, 32),
            img(64, 64, 32),
            img(128, 128, 32),
        ];
        assert_eq!(select_best_fit(&images, 32), Some(1));
    }

    #[test]
    fn select_best_fit_falls_back_to_largest_when_all_smaller() {
        // Target 256; entries 16, 32, 48. None are ≥ target, so the
        // largest one (48) wins.
        let images = [img(16, 16, 32), img(32, 32, 32), img(48, 48, 32)];
        assert_eq!(select_best_fit(&images, 256), Some(2));
    }

    #[test]
    fn select_best_fit_exact_target_match_wins() {
        // Target 32 with both a 32-px entry and a 64-px entry; the
        // 32-px exact match should beat the larger entry.
        let images = [img(32, 32, 32), img(64, 64, 32)];
        assert_eq!(select_best_fit(&images, 32), Some(0));
    }

    #[test]
    fn select_best_fit_breaks_tie_on_bpp() {
        // Two 32×32 entries at different bit depths. Target 32.
        // 32-bpp wins.
        let images = [img(32, 32, 1), img(32, 32, 32)];
        assert_eq!(select_best_fit(&images, 32), Some(1));
    }

    #[test]
    fn select_best_fit_handles_non_square_entries() {
        // Non-square entries — `max(width, height)` is what counts.
        // A 16×48 entry has max-dim 48, so target 32 should pick it
        // over a square 16×16.
        let images = [img(16, 16, 32), img(16, 48, 32), img(64, 64, 32)];
        assert_eq!(select_best_fit(&images, 32), Some(1));
    }

    #[test]
    fn select_by_dimensions_empty() {
        assert!(select_by_dimensions(&[], 32, 32).is_none());
    }

    #[test]
    fn select_by_dimensions_exact_match() {
        let images = [img(16, 16, 32), img(32, 32, 32), img(256, 256, 32)];
        assert_eq!(select_by_dimensions(&images, 32, 32), Some(1));
        assert_eq!(select_by_dimensions(&images, 256, 256), Some(2));
    }

    #[test]
    fn select_by_dimensions_no_match_returns_none() {
        // 48×48 isn't present — strict equality, no nearest-fit
        // substitution (that's `select_best_fit`'s job).
        let images = [img(16, 16, 32), img(32, 32, 32), img(64, 64, 32)];
        assert!(select_by_dimensions(&images, 48, 48).is_none());
    }

    #[test]
    fn select_by_dimensions_breaks_tie_on_bpp() {
        // Two 32×32 entries, legacy 1-bpp and modern 32-bpp. The exact
        // lookup returns the higher-bit-depth one.
        let images = [img(32, 32, 1), img(32, 32, 32)];
        assert_eq!(select_by_dimensions(&images, 32, 32), Some(1));
    }

    #[test]
    fn select_by_dimensions_respects_non_square() {
        // 16×48 must not match a request for 48×16 — order matters.
        let images = [img(16, 48, 32), img(48, 16, 32)];
        assert_eq!(select_by_dimensions(&images, 16, 48), Some(0));
        assert_eq!(select_by_dimensions(&images, 48, 16), Some(1));
    }

    // ───────────────────────── directory-level (raw) ─────────────────────────

    /// A directory row with no payload bytes — selection never touches
    /// `data`, so an empty body is fine for these tests.
    fn entry(w: u32, h: u32, bpp: u8) -> crate::raw::IconEntryRaw {
        crate::raw::IconEntryRaw {
            width: w,
            height: h,
            bit_depth: bpp,
            sub_format: IconSubFormat::Bmp,
            hotspot: None,
            data: Vec::new(),
        }
    }

    #[test]
    fn select_largest_raw_empty() {
        assert!(select_largest_raw(&[]).is_none());
    }

    #[test]
    fn select_largest_raw_picks_max_area() {
        let entries = [entry(16, 16, 32), entry(64, 64, 32), entry(32, 32, 32)];
        assert_eq!(select_largest_raw(&entries), Some(1));
    }

    #[test]
    fn select_largest_raw_breaks_tie_on_bpp() {
        let entries = [entry(32, 32, 1), entry(32, 32, 32)];
        assert_eq!(select_largest_raw(&entries), Some(1));
    }

    #[test]
    fn select_best_fit_raw_empty() {
        assert!(select_best_fit_raw(&[], 32).is_none());
    }

    #[test]
    fn select_best_fit_raw_smallest_entry_at_or_above_target() {
        let entries = [
            entry(16, 16, 32),
            entry(32, 32, 32),
            entry(64, 64, 32),
            entry(128, 128, 32),
        ];
        assert_eq!(select_best_fit_raw(&entries, 32), Some(1));
    }

    #[test]
    fn select_best_fit_raw_falls_back_to_largest_when_all_smaller() {
        let entries = [entry(16, 16, 32), entry(32, 32, 32), entry(48, 48, 32)];
        assert_eq!(select_best_fit_raw(&entries, 256), Some(2));
    }

    #[test]
    fn select_best_fit_raw_breaks_tie_on_bpp() {
        let entries = [entry(32, 32, 1), entry(32, 32, 32)];
        assert_eq!(select_best_fit_raw(&entries, 32), Some(1));
    }

    #[test]
    fn select_best_fit_raw_handles_non_square_entries() {
        let entries = [entry(16, 16, 32), entry(16, 48, 32), entry(64, 64, 32)];
        assert_eq!(select_best_fit_raw(&entries, 32), Some(1));
    }

    #[test]
    fn select_by_dimensions_raw_exact_match() {
        let entries = [entry(16, 16, 32), entry(32, 32, 32), entry(256, 256, 32)];
        assert_eq!(select_by_dimensions_raw(&entries, 32, 32), Some(1));
        assert_eq!(select_by_dimensions_raw(&entries, 256, 256), Some(2));
    }

    #[test]
    fn select_by_dimensions_raw_no_match_returns_none() {
        let entries = [entry(16, 16, 32), entry(32, 32, 32), entry(64, 64, 32)];
        assert!(select_by_dimensions_raw(&entries, 48, 48).is_none());
    }

    #[test]
    fn select_by_dimensions_raw_breaks_tie_on_bpp() {
        let entries = [entry(32, 32, 1), entry(32, 32, 32)];
        assert_eq!(select_by_dimensions_raw(&entries, 32, 32), Some(1));
    }

    #[test]
    fn raw_and_decoded_selectors_agree() {
        // The raw and decoded families must resolve to the same index
        // when fed the same (width, height, bit-depth) facts — the
        // whole point of sharing the generic core. Mixed sizes + a
        // legacy/modern bit-depth tie at 32×32.
        let facts = [(16u32, 16u32, 1u8), (32, 32, 1), (32, 32, 32), (64, 64, 32)];
        let images: Vec<IconImage> = facts.iter().map(|&(w, h, b)| img(w, h, b)).collect();
        let entries: Vec<crate::raw::IconEntryRaw> =
            facts.iter().map(|&(w, h, b)| entry(w, h, b)).collect();

        for target in [1u32, 16, 24, 32, 48, 64, 128, 256] {
            assert_eq!(
                select_best_fit(&images, target),
                select_best_fit_raw(&entries, target),
                "best_fit disagreed at target {target}"
            );
        }
        assert_eq!(select_largest(&images), select_largest_raw(&entries));
        // The 32×32 tie must resolve to the 32-bpp row (index 2) in both.
        assert_eq!(select_by_dimensions(&images, 32, 32), Some(2));
        assert_eq!(select_by_dimensions_raw(&entries, 32, 32), Some(2));
    }
}
