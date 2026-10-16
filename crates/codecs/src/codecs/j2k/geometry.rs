//! Resolution / band / precinct / code-block geometry (ITU-T T.800 B.5–B.7).
//!
//! Everything here is expressed in *tile-component* space. This crate pins all
//! grid origins at zero (the codestream layer rejects non-zero
//! `XOsiz`/`YOsiz`/`XTOsiz`/`YTOsiz` with `Unsupported`), so the tile-component
//! coordinates equal the image coordinates and every rectangle used below is
//! anchored at `(0, 0)`. The math is nonetheless written in the general,
//! spec-shaped ceil-division form so it reads against T.800 directly.
//!
//! Layout note (verified against `dwt.rs`): the conformant multi-level DWT
//! (`forward_multi_level_conformant`) transforms, at each level, the LL region
//! left by the previous level, in place, at the full tile stride. A single 2-D
//! transform of a `W×H` region deinterleaves into
//!   * LL at `(0, 0)`             size `⌈W/2⌉ × ⌈H/2⌉`
//!   * HL at `(⌈W/2⌉, 0)`         (horizontal detail)
//!   * LH at `(0, ⌈H/2⌉)`         (vertical detail)
//!   * HH at `(⌈W/2⌉, ⌈H/2⌉)`     (diagonal detail)
//!
//! So at decomposition level `n` the region size is `⌈w/2ⁿ⁻¹⌉ × ⌈h/2ⁿ⁻¹⌉` and
//! `LLₙ` (size `⌈w/2ⁿ⌉ × ⌈h/2ⁿ⌉`) sits at its top-left. [`MallatPlacement`]
//! records where each band lands in that packed tile buffer.

// BandKind is exactly the four DWT sub-bands already modelled by
// `markers::Subband` (LL/HL/LH/HH), so we reuse that type rather than defining a
// parallel enum.
pub use super::markers::Subband as BandKind;

// PPx = PPy default precinct exponent (T.800 A.6.1 default: 2^15 precincts).
const PP_DEFAULT: u32 = 15;

// ---------------------------------------------------------------------------
// Rect
// ---------------------------------------------------------------------------

/// Half-open rectangle `[x0, x1) × [y0, y1)` in tile-component / band space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Rect {
    /// Width `x1 - x0` (saturating; an inverted rect reads as empty).
    pub fn width(&self) -> u32 {
        self.x1.saturating_sub(self.x0)
    }

    /// Height `y1 - y0` (saturating).
    pub fn height(&self) -> u32 {
        self.y1.saturating_sub(self.y0)
    }

    /// Number of samples covered.
    #[allow(dead_code)] // used by geometry/packet tests and invariant checks
    pub fn area(&self) -> u64 {
        self.width() as u64 * self.height() as u64
    }

    /// `true` when the rect covers no samples.
    #[allow(dead_code)] // used by geometry/packet tests
    pub fn is_empty(&self) -> bool {
        self.width() == 0 || self.height() == 0
    }
}

// ---------------------------------------------------------------------------
// Bands / resolutions / tile
// ---------------------------------------------------------------------------

/// Position of a band inside the level's packed (Mallat) tile buffer.
///
/// Distinct from the band's own coordinate origin (which is `(0, 0)` in
/// band-native space): this is the physical top-left offset within the full
/// tile buffer produced by the multi-level DWT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MallatPlacement {
    pub x_off: u32,
    pub y_off: u32,
}

/// A single code-block within a band (band-native coordinates).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlockGeom {
    /// Clipped code-block rectangle in band-native space.
    pub rect: Rect,
    /// Index of the owning precinct (0 for the single-precinct default).
    pub precinct_idx: usize,
}

/// One DWT sub-band of a resolution level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Band {
    pub kind: BandKind,
    /// Band-native rectangle, anchored at `(0, 0)`.
    pub rect: Rect,
    /// Log2 nominal gain: 0 (LL), 1 (HL), 1 (LH), 2 (HH). (T.800 E.1 / Table F.3)
    pub gain_log2: u8,
    /// Where this band sits in the packed Mallat tile buffer.
    pub placement: MallatPlacement,
    /// Code-blocks partitioning the band (raster order, clipped, non-empty).
    pub cbs: Vec<CodeBlockGeom>,
    /// Number of code-block columns (0 if the band is empty).
    pub cb_grid_w: u32,
    /// Number of code-block rows (0 if the band is empty).
    pub cb_grid_h: u32,
}

/// A resolution level `r` (`0..=NL`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// Resolution index (`0` = coarsest LL, `NL` = full tile).
    pub r: u8,
    /// Resolution rectangle, anchored at `(0, 0)`.
    pub rect: Rect,
    /// Sub-bands: `[LL_NL]` at `r == 0`, else `[HL, LH, HH]`.
    pub bands: Vec<Band>,
    pub num_precincts_w: u32,
    pub num_precincts_h: u32,
}

/// Full geometry of one tile-component: one entry per resolution `0..=NL`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileGeometry {
    pub resolutions: Vec<Resolution>,
}

// ---------------------------------------------------------------------------
// ceil-division helpers
// ---------------------------------------------------------------------------

/// `⌈v / 2^k⌉` for unsigned `v` (widening to avoid `1 << 32` overflow).
fn ceil_shift(v: u32, k: u32) -> u32 {
    if k == 0 {
        return v;
    }
    let d = 1u64 << k;
    (((v as u64) + d - 1) >> k) as u32
}

/// `⌈a / b⌉` for `b > 0`, valid for negative `a` (T.800 uses `⌈·⌉` throughout,
/// and the HL/HH band offsets make the numerator go negative for tiny tiles).
fn ceil_div_i64(a: i64, b: i64) -> i64 {
    debug_assert!(b > 0);
    let q = a / b;
    let r = a % b;
    if r > 0 {
        q + 1
    } else {
        q
    }
}

/// Sub-band rectangle per T.800 Eq. B-15, in tile-component coordinates.
///
/// `nb` is the sub-band decomposition level; `xob`/`yob` are the sub-band
/// partition offsets (0 or 1): LL=(0,0), HL=(1,0), LH=(0,1), HH=(1,1).
fn subband_rect_b15(
    tcx0: i64,
    tcy0: i64,
    tcx1: i64,
    tcy1: i64,
    nb: u32,
    xob: i64,
    yob: i64,
) -> Rect {
    let d = 1i64 << nb;
    // 2^(nb-1) is only referenced when the offset is 1, and HL/HH (offset 1)
    // always have nb >= 1, so the shift never underflows.
    let half = if nb > 0 { 1i64 << (nb - 1) } else { 0 };
    let off_x = xob * half;
    let off_y = yob * half;

    let bx0 = ceil_div_i64(tcx0 - off_x, d);
    let bx1 = ceil_div_i64(tcx1 - off_x, d);
    let by0 = ceil_div_i64(tcy0 - off_y, d);
    let by1 = ceil_div_i64(tcy1 - off_y, d);

    Rect {
        x0: bx0.max(0) as u32,
        y0: by0.max(0) as u32,
        x1: bx1.max(0) as u32,
        y1: by1.max(0) as u32,
    }
}

fn gain_log2(kind: BandKind) -> u8 {
    match kind {
        BandKind::LL => 0,
        BandKind::HL | BandKind::LH => 1,
        BandKind::HH => 2,
    }
}

fn xob_yob(kind: BandKind) -> (i64, i64) {
    match kind {
        BandKind::LL => (0, 0),
        BandKind::HL => (1, 0),
        BandKind::LH => (0, 1),
        BandKind::HH => (1, 1),
    }
}

// ---------------------------------------------------------------------------
// Code-block partition (T.800 B.7)
// ---------------------------------------------------------------------------

/// Partition a band into code-blocks on the `2^xcb_eff × 2^ycb_eff` grid
/// anchored at the band origin. Returns `(blocks, grid_w, grid_h)`.
fn build_code_blocks(
    band: &Rect,
    xcb_eff: u32,
    ycb_eff: u32,
    ppx_band: u32,
    ppy_band: u32,
    num_precincts_w: u32,
) -> (Vec<CodeBlockGeom>, u32, u32) {
    let w = band.width();
    let h = band.height();
    if w == 0 || h == 0 {
        return (Vec::new(), 0, 0);
    }

    let cbw = 1u32 << xcb_eff;
    let cbh = 1u32 << ycb_eff;
    let grid_w = ceil_shift(w, xcb_eff);
    let grid_h = ceil_shift(h, ycb_eff);

    let mut cbs = Vec::with_capacity((grid_w as usize) * (grid_h as usize));
    for row in 0..grid_h {
        let by0 = row * cbh;
        let by1 = ((row + 1) * cbh).min(h);
        let prow = by0 >> ppy_band;
        for col in 0..grid_w {
            let bx0 = col * cbw;
            let bx1 = ((col + 1) * cbw).min(w);
            let pcol = bx0 >> ppx_band;
            let precinct_idx = (prow * num_precincts_w + pcol) as usize;
            cbs.push(CodeBlockGeom {
                rect: Rect {
                    x0: bx0,
                    y0: by0,
                    x1: bx1,
                    y1: by1,
                },
                precinct_idx,
            });
        }
    }
    (cbs, grid_w, grid_h)
}

/// Assemble one band: B-15 rect, gain, Mallat placement, code-blocks.
#[allow(clippy::too_many_arguments)]
fn build_band(
    kind: BandKind,
    tile_w: u32,
    tile_h: u32,
    nb: u32,
    placement: MallatPlacement,
    xcb_eff: u32,
    ycb_eff: u32,
    ppx_band: u32,
    ppy_band: u32,
    num_precincts_w: u32,
) -> Band {
    let (xob, yob) = xob_yob(kind);
    let rect = subband_rect_b15(0, 0, tile_w as i64, tile_h as i64, nb, xob, yob);
    let (cbs, cb_grid_w, cb_grid_h) =
        build_code_blocks(&rect, xcb_eff, ycb_eff, ppx_band, ppy_band, num_precincts_w);
    Band {
        kind,
        rect,
        gain_log2: gain_log2(kind),
        placement,
        cbs,
        cb_grid_w,
        cb_grid_h,
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Build the full resolution / band / precinct / code-block geometry for one
/// tile-component of size `tile_w × tile_h` decomposed `num_levels` times.
///
/// `xcb`/`ycb` are the code-block width/height exponents (block size
/// `2^xcb × 2^ycb`). Grid origins are pinned at zero.
pub fn build_geometry(tile_w: u32, tile_h: u32, num_levels: u8, xcb: u8, ycb: u8) -> TileGeometry {
    let nl = num_levels as u32;
    let mut resolutions = Vec::with_capacity(num_levels as usize + 1);

    for r in 0..=num_levels {
        let ru = r as u32;
        // Resolution rect = ⌈tile / 2^(NL-r)⌉ (T.800 B.5 with zero origin).
        let shift = nl - ru;
        let res_w = ceil_shift(tile_w, shift);
        let res_h = ceil_shift(tile_h, shift);
        let rect = Rect {
            x0: 0,
            y0: 0,
            x1: res_w,
            y1: res_h,
        };

        // Precincts over the resolution rect (B.6). Empty resolution → 0.
        let (num_precincts_w, num_precincts_h) = if res_w == 0 || res_h == 0 {
            (0, 0)
        } else {
            (ceil_shift(res_w, PP_DEFAULT), ceil_shift(res_h, PP_DEFAULT))
        };

        // Effective code-block / precinct exponents (B.7): capped by the
        // precinct exponent, which is reduced by one at resolutions r >= 1.
        let (ppx_band, ppy_band) = if r == 0 {
            (PP_DEFAULT, PP_DEFAULT)
        } else {
            (PP_DEFAULT - 1, PP_DEFAULT - 1)
        };
        let xcb_eff = (xcb as u32).min(ppx_band);
        let ycb_eff = (ycb as u32).min(ppy_band);

        let bands = if r == 0 {
            // Resolution 0 carries the single LL_NL sub-band.
            vec![build_band(
                BandKind::LL,
                tile_w,
                tile_h,
                nl, // LL at the coarsest decomposition level
                MallatPlacement { x_off: 0, y_off: 0 },
                xcb_eff,
                ycb_eff,
                ppx_band,
                ppy_band,
                num_precincts_w,
            )]
        } else {
            // Detail bands from decomposition level n = NL - r + 1.
            let n = nl - ru + 1;
            // LLₙ size = previous resolution rect = ⌈tile / 2^n⌉; the packed
            // region containing these three bands is ⌈tile / 2^(n-1)⌉.
            let ll_w = ceil_shift(tile_w, n);
            let ll_h = ceil_shift(tile_h, n);
            vec![
                build_band(
                    BandKind::HL,
                    tile_w,
                    tile_h,
                    n,
                    MallatPlacement {
                        x_off: ll_w,
                        y_off: 0,
                    },
                    xcb_eff,
                    ycb_eff,
                    ppx_band,
                    ppy_band,
                    num_precincts_w,
                ),
                build_band(
                    BandKind::LH,
                    tile_w,
                    tile_h,
                    n,
                    MallatPlacement {
                        x_off: 0,
                        y_off: ll_h,
                    },
                    xcb_eff,
                    ycb_eff,
                    ppx_band,
                    ppy_band,
                    num_precincts_w,
                ),
                build_band(
                    BandKind::HH,
                    tile_w,
                    tile_h,
                    n,
                    MallatPlacement {
                        x_off: ll_w,
                        y_off: ll_h,
                    },
                    xcb_eff,
                    ycb_eff,
                    ppx_band,
                    ppy_band,
                    num_precincts_w,
                ),
            ]
        };

        resolutions.push(Resolution {
            r,
            rect,
            bands,
            num_precincts_w,
            num_precincts_h,
        });
    }

    TileGeometry { resolutions }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
