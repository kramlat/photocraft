//! 5/3 reversible discrete wavelet transform (lifting scheme).
//!
//! Implements the forward and inverse DWT as specified in ITU-T T.800
//! Annex F, using the lifting-based factorisation of the Le Gall 5/3 filter.

// ---------------------------------------------------------------------------
// 1-D transforms
// ---------------------------------------------------------------------------

/// Forward 1-D 5/3 wavelet transform (in-place, lifting scheme).
///
/// After the transform the first `(n+1)/2` samples contain the low-pass
/// coefficients and the remaining `n/2` samples contain the high-pass
/// coefficients.
pub fn forward_1d(signal: &mut [i32]) {
    let n = signal.len();
    if n < 2 {
        return;
    }

    let half = n.div_ceil(2);
    let num_high = n - half;

    // Split into even (low) and odd (high) samples.
    let mut low = vec![0i32; half];
    let mut high = vec![0i32; num_high];

    for i in 0..half {
        low[i] = signal[2 * i];
    }
    for i in 0..num_high {
        high[i] = signal[2 * i + 1];
    }

    // Predict step  (high-pass):
    //   d[i] = x[2i+1] - floor((x[2i] + x[2i+2]) / 2)
    // Arithmetic right shift gives floor for negative operands (T.800 F.4.8.2).
    for i in 0..num_high {
        let left = low[i];
        let right = if i + 1 < half { low[i + 1] } else { left }; // symmetric extension
        high[i] -= (left + right) >> 1;
    }

    // Update step  (low-pass):
    //   s[i] = x[2i] + floor((d[i-1] + d[i] + 2) / 4)
    for i in 0..half {
        let left = if i > 0 {
            high[i - 1]
        } else if !high.is_empty() {
            high[0] // symmetric extension
        } else {
            0
        };
        let right = if i < num_high { high[i] } else { left };
        low[i] += (left + right + 2) >> 2;
    }

    // Pack: low first, then high.
    signal[..half].copy_from_slice(&low);
    signal[half..].copy_from_slice(&high);
}

/// Inverse 1-D 5/3 wavelet transform (in-place, lifting scheme).
///
/// Expects low-pass coefficients in `signal[..half]` and high-pass in
/// `signal[half..]` where `half = (n+1)/2`.
pub fn inverse_1d(signal: &mut [i32]) {
    let n = signal.len();
    if n < 2 {
        return;
    }

    let half = n.div_ceil(2);
    let num_high = n - half;

    let mut low = vec![0i32; half];
    let mut high = vec![0i32; num_high];

    low.copy_from_slice(&signal[..half]);
    high.copy_from_slice(&signal[half..]);

    // Inverse update step.
    for i in 0..half {
        let left = if i > 0 {
            high[i - 1]
        } else if !high.is_empty() {
            high[0]
        } else {
            0
        };
        let right = if i < num_high { high[i] } else { left };
        low[i] -= (left + right + 2) >> 2;
    }

    // Inverse predict step.
    for i in 0..num_high {
        let left = low[i];
        let right = if i + 1 < half { low[i + 1] } else { left };
        high[i] += (left + right) >> 1;
    }

    // Interleave.
    for i in 0..half {
        signal[2 * i] = low[i];
    }
    for i in 0..num_high {
        signal[2 * i + 1] = high[i];
    }
}

// ---------------------------------------------------------------------------
// Legacy 1-D transforms (truncating division)
// ---------------------------------------------------------------------------
//
// The 1.0.0 raw-DWT pipeline (still wired through `encode`/`decode` via
// `forward_multi_level`/`inverse_multi_level`) used truncating integer
// division (`/2`, `/4`), which rounds toward zero rather than flooring. The
// checked-in legacy fixtures pin those exact bytes, so the legacy multi-level
// entry points MUST keep using these functions verbatim. Do not "fix" them.

/// Legacy forward 1-D 5/3 transform (truncating division). Frozen for byte
/// compatibility with 1.0.0 codestreams -- see `tests/legacy_fixtures.rs`.
#[allow(dead_code)] // frozen legacy (v1.0.0) DWT helper; live only via `legacy` decode
fn legacy_forward_1d(signal: &mut [i32]) {
    let n = signal.len();
    if n < 2 {
        return;
    }

    let half = n.div_ceil(2);
    let num_high = n - half;

    let mut low = vec![0i32; half];
    let mut high = vec![0i32; num_high];

    for i in 0..half {
        low[i] = signal[2 * i];
    }
    for i in 0..num_high {
        high[i] = signal[2 * i + 1];
    }

    for i in 0..num_high {
        let left = low[i];
        let right = if i + 1 < half { low[i + 1] } else { left };
        high[i] -= (left + right) / 2;
    }

    for i in 0..half {
        let left = if i > 0 {
            high[i - 1]
        } else if !high.is_empty() {
            high[0]
        } else {
            0
        };
        let right = if i < num_high { high[i] } else { left };
        low[i] += (left + right + 2) / 4;
    }

    signal[..half].copy_from_slice(&low);
    signal[half..].copy_from_slice(&high);
}

/// Legacy inverse 1-D 5/3 transform (truncating division). Frozen for byte
/// compatibility with 1.0.0 codestreams -- see `tests/legacy_fixtures.rs`.
#[allow(dead_code)] // frozen legacy (v1.0.0) DWT helper; live only via `legacy` decode
fn legacy_inverse_1d(signal: &mut [i32]) {
    let n = signal.len();
    if n < 2 {
        return;
    }

    let half = n.div_ceil(2);
    let num_high = n - half;

    let mut low = vec![0i32; half];
    let mut high = vec![0i32; num_high];

    low.copy_from_slice(&signal[..half]);
    high.copy_from_slice(&signal[half..]);

    for i in 0..half {
        let left = if i > 0 {
            high[i - 1]
        } else if !high.is_empty() {
            high[0]
        } else {
            0
        };
        let right = if i < num_high { high[i] } else { left };
        low[i] -= (left + right + 2) / 4;
    }

    for i in 0..num_high {
        let left = low[i];
        let right = if i + 1 < half { low[i + 1] } else { left };
        high[i] += (left + right) / 2;
    }

    for i in 0..half {
        signal[2 * i] = low[i];
    }
    for i in 0..num_high {
        signal[2 * i + 1] = high[i];
    }
}

// ---------------------------------------------------------------------------
// 2-D transforms (single level)
// ---------------------------------------------------------------------------

/// Forward 2-D 5/3 DWT -- produces LL, HL, LH, HH subbands (in-place).
#[allow(dead_code)] // TODO(t800): single-level 2-D helper, retained for tests
pub fn forward_2d(data: &mut [i32], width: usize, height: usize) {
    if width < 2 || height < 2 {
        return;
    }

    // Transform rows.
    let mut row = vec![0i32; width];
    for y in 0..height {
        let off = y * width;
        row.copy_from_slice(&data[off..off + width]);
        forward_1d(&mut row);
        data[off..off + width].copy_from_slice(&row);
    }

    // Transform columns.
    let mut col = vec![0i32; height];
    for x in 0..width {
        for y in 0..height {
            col[y] = data[y * width + x];
        }
        forward_1d(&mut col);
        for y in 0..height {
            data[y * width + x] = col[y];
        }
    }
}

/// Inverse 2-D 5/3 DWT -- reconstructs from LL, HL, LH, HH subbands (in-place).
#[allow(dead_code)] // TODO(t800): single-level 2-D helper, retained for tests
pub fn inverse_2d(data: &mut [i32], width: usize, height: usize) {
    if width < 2 || height < 2 {
        return;
    }

    // Inverse columns first.
    let mut col = vec![0i32; height];
    for x in 0..width {
        for y in 0..height {
            col[y] = data[y * width + x];
        }
        inverse_1d(&mut col);
        for y in 0..height {
            data[y * width + x] = col[y];
        }
    }

    // Inverse rows.
    let mut row = vec![0i32; width];
    for y in 0..height {
        let off = y * width;
        row.copy_from_slice(&data[off..off + width]);
        inverse_1d(&mut row);
        data[off..off + width].copy_from_slice(&row);
    }
}

// ---------------------------------------------------------------------------
// Multi-level transforms (operates on LL region of previous level)
// ---------------------------------------------------------------------------

/// Forward multi-level 2-D DWT (LEGACY truncating pipeline).
///
/// Each level transforms the LL subband from the previous level.
/// Returns the (width, height) of the final LL subband.
///
/// This is the path wired through `encode`; it uses [`legacy_forward_1d`] and
/// stops early on degenerate sub-regions, matching 1.0.0 byte output. Do not
/// change its behaviour -- use the `*_conformant` entry points for new work.
#[allow(dead_code)] // frozen legacy (v1.0.0) DWT helper; live only via `legacy` decode
pub fn forward_multi_level(
    data: &mut [i32],
    width: usize,
    height: usize,
    levels: usize,
) -> (usize, usize) {
    let mut ll_w = width;
    let mut ll_h = height;

    for _ in 0..levels {
        if ll_w < 2 || ll_h < 2 {
            break;
        }
        forward_ll_region(data, width, ll_w, ll_h);
        ll_w = ll_w.div_ceil(2);
        ll_h = ll_h.div_ceil(2);
    }
    (ll_w, ll_h)
}

/// Inverse multi-level 2-D DWT.
///
/// Levels are processed in reverse order, from smallest to largest.
#[allow(dead_code)] // frozen legacy (v1.0.0) DWT helper; live only via `legacy` decode
pub fn inverse_multi_level(data: &mut [i32], width: usize, height: usize, levels: usize) {
    // Pre-calculate LL dimensions at each level.
    let mut dims = vec![(0usize, 0usize); levels + 1];
    dims[0] = (width, height);
    for i in 1..=levels {
        dims[i] = (dims[i - 1].0.div_ceil(2), dims[i - 1].1.div_ceil(2));
    }

    // Reconstruct from smallest to largest.
    for level in (0..levels).rev() {
        let (ll_w, ll_h) = dims[level];
        if ll_w < 2 || ll_h < 2 {
            continue;
        }
        inverse_ll_region(data, width, ll_w, ll_h);
    }
}

// ---------------------------------------------------------------------------
// Helpers -- forward/inverse on the LL sub-region
// ---------------------------------------------------------------------------

#[allow(dead_code)] // frozen legacy (v1.0.0) DWT helper; live only via `legacy` decode
fn forward_ll_region(data: &mut [i32], stride: usize, width: usize, height: usize) {
    if width < 2 || height < 2 {
        return;
    }

    // Transform rows in the region (legacy truncating 1-D).
    let mut row = vec![0i32; width];
    for y in 0..height {
        let off = y * stride;
        row.copy_from_slice(&data[off..off + width]);
        legacy_forward_1d(&mut row);
        data[off..off + width].copy_from_slice(&row);
    }

    // Transform columns in the region (legacy truncating 1-D).
    let mut col = vec![0i32; height];
    for x in 0..width {
        for y in 0..height {
            col[y] = data[y * stride + x];
        }
        legacy_forward_1d(&mut col);
        for y in 0..height {
            data[y * stride + x] = col[y];
        }
    }
}

#[allow(dead_code)] // frozen legacy (v1.0.0) DWT helper; live only via `legacy` decode
fn inverse_ll_region(data: &mut [i32], stride: usize, width: usize, height: usize) {
    if width < 2 || height < 2 {
        return;
    }

    // Inverse columns first (legacy truncating 1-D).
    let mut col = vec![0i32; height];
    for x in 0..width {
        for y in 0..height {
            col[y] = data[y * stride + x];
        }
        legacy_inverse_1d(&mut col);
        for y in 0..height {
            data[y * stride + x] = col[y];
        }
    }

    // Inverse rows (legacy truncating 1-D).
    let mut row = vec![0i32; width];
    for y in 0..height {
        let off = y * stride;
        row.copy_from_slice(&data[off..off + width]);
        legacy_inverse_1d(&mut row);
        data[off..off + width].copy_from_slice(&row);
    }
}

// ---------------------------------------------------------------------------
// Conformant multi-level transforms (T.800 F.4.8 -- floor lifting)
// ---------------------------------------------------------------------------
//
// These are the spec-conformant entry points the future tier-1/tier-2 pipeline
// will use. They differ from the legacy path in three ways:
//   * they call the floor-lifting `forward_1d`/`inverse_1d`;
//   * they transform degenerate 1xN / Nx1 sub-regions along the dimension of
//     length >= 2 (a 1x1 level is a no-op) rather than stopping early;
//   * they always perform exactly `levels` levels;
//   * they hoist one row-sized and one column-sized scratch buffer per call,
//     reused across every row, column, and level (no per-row allocation).

/// Forward multi-level 2-D 5/3 DWT (conformant, floor lifting).
///
/// Returns the `(width, height)` of the final LL sub-band.
// TODO(t800): unwire when tier-1 lands
#[allow(dead_code)]
pub fn forward_multi_level_conformant(
    data: &mut [i32],
    width: usize,
    height: usize,
    levels: usize,
) -> (usize, usize) {
    let mut row_scratch = vec![0i32; width];
    let mut col_scratch = vec![0i32; height];

    let mut ll_w = width;
    let mut ll_h = height;
    for _ in 0..levels {
        forward_ll_region_conformant(data, width, ll_w, ll_h, &mut row_scratch, &mut col_scratch);
        ll_w = ll_w.div_ceil(2);
        ll_h = ll_h.div_ceil(2);
    }
    (ll_w, ll_h)
}

/// Inverse multi-level 2-D 5/3 DWT (conformant, floor lifting).
// TODO(t800): unwire when tier-1 lands
#[allow(dead_code)]
pub fn inverse_multi_level_conformant(
    data: &mut [i32],
    width: usize,
    height: usize,
    levels: usize,
) {
    // LL dimensions at each level.
    let mut dims = vec![(0usize, 0usize); levels + 1];
    dims[0] = (width, height);
    for i in 1..=levels {
        dims[i] = (dims[i - 1].0.div_ceil(2), dims[i - 1].1.div_ceil(2));
    }

    let mut row_scratch = vec![0i32; width];
    let mut col_scratch = vec![0i32; height];

    for level in (0..levels).rev() {
        let (ll_w, ll_h) = dims[level];
        inverse_ll_region_conformant(data, width, ll_w, ll_h, &mut row_scratch, &mut col_scratch);
    }
}

/// Forward transform of one LL sub-region (conformant). Degenerate rows or
/// columns (length < 2) are left unchanged by the 1-D transform.
#[allow(dead_code)]
fn forward_ll_region_conformant(
    data: &mut [i32],
    stride: usize,
    width: usize,
    height: usize,
    row_scratch: &mut [i32],
    col_scratch: &mut [i32],
) {
    // Columns first, then rows. The reversible 5/3 lifting uses floor rounding,
    // so the 2-D pass order is observable in the integer coefficients; T.800 /
    // OpenJPEG transform columns (vertical) before rows (horizontal) on the
    // forward transform (the inverse undoes them in the opposite order). Coding
    // rows first here produced off-by-one coefficients versus OpenJPEG.
    let col = &mut col_scratch[..height];
    for x in 0..width {
        for (y, slot) in col.iter_mut().enumerate() {
            *slot = data[y * stride + x];
        }
        forward_1d(col);
        for (y, &v) in col.iter().enumerate() {
            data[y * stride + x] = v;
        }
    }

    for y in 0..height {
        let off = y * stride;
        let row = &mut row_scratch[..width];
        row.copy_from_slice(&data[off..off + width]);
        forward_1d(row);
        data[off..off + width].copy_from_slice(row);
    }
}

/// Inverse transform of one LL sub-region (conformant).
#[allow(dead_code)]
fn inverse_ll_region_conformant(
    data: &mut [i32],
    stride: usize,
    width: usize,
    height: usize,
    row_scratch: &mut [i32],
    col_scratch: &mut [i32],
) {
    // Rows first, then columns -- the exact reverse of the forward pass order
    // (columns-then-rows), so this is its true inverse and matches OpenJPEG's
    // inverse transform.
    for y in 0..height {
        let off = y * stride;
        let row = &mut row_scratch[..width];
        row.copy_from_slice(&data[off..off + width]);
        inverse_1d(row);
        data[off..off + width].copy_from_slice(row);
    }

    let col = &mut col_scratch[..height];
    for x in 0..width {
        for (y, slot) in col.iter_mut().enumerate() {
            *slot = data[y * stride + x];
        }
        inverse_1d(col);
        for (y, &v) in col.iter().enumerate() {
            data[y * stride + x] = v;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
