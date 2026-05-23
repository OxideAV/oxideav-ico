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
pub fn select_best_fit(images: &[IconImage], target: u32) -> Option<usize> {
    if images.is_empty() {
        return None;
    }
    // First pass: smallest entry whose max-dim is ≥ target.
    let mut best: Option<usize> = None;
    for (i, im) in images.iter().enumerate() {
        let max_dim = im.width.max(im.height);
        if max_dim < target {
            continue;
        }
        let better = match best {
            None => true,
            Some(j) => {
                let cur = images[j].width.max(images[j].height);
                match max_dim.cmp(&cur) {
                    core::cmp::Ordering::Less => true,
                    core::cmp::Ordering::Equal => im.bit_depth > images[j].bit_depth,
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
    Some(select_largest(images).expect("non-empty slice has a largest entry"))
}

/// Pick the largest sub-image by pixel area. Ties broken by bit depth
/// (higher wins). Returns `None` only when the slice is empty.
///
/// Useful when the caller wants the highest-fidelity entry regardless
/// of render size — e.g. extracting the canonical "main" icon from a
/// multi-resolution `.ico` to feed a thumbnail pipeline.
pub fn select_largest(images: &[IconImage]) -> Option<usize> {
    if images.is_empty() {
        return None;
    }
    let mut best = 0;
    for i in 1..images.len() {
        let im = &images[i];
        let cur = &images[best];
        let im_area = (im.width as u64) * (im.height as u64);
        let cur_area = (cur.width as u64) * (cur.height as u64);
        let pick = match im_area.cmp(&cur_area) {
            core::cmp::Ordering::Greater => true,
            core::cmp::Ordering::Equal => im.bit_depth > cur.bit_depth,
            core::cmp::Ordering::Less => false,
        };
        if pick {
            best = i;
        }
    }
    Some(best)
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
pub fn select_by_dimensions(images: &[IconImage], width: u32, height: u32) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, im) in images.iter().enumerate() {
        if im.width != width || im.height != height {
            continue;
        }
        let better = match best {
            None => true,
            Some(j) => im.bit_depth > images[j].bit_depth,
        };
        if better {
            best = Some(i);
        }
    }
    best
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
}
