//! EBCOT (Embedded Block Coding with Optimal Truncation) -- ITU-T T.800 Annex D.
//!
//! Conformant tier-1 coder for lossless (reversible) code-blocks. A single
//! shared [`Tier1`] state machine drives BOTH encoding and decoding: the scan
//! order, flag grid, and all context-formation logic (zero-coding,
//! sign-coding, magnitude-refinement) are written exactly once and dispatch to
//! the MQ arithmetic coder through one `code()` primitive. This kills the
//! former encoder/decoder duplication that could silently drift out of sync.
//!
//! Coding is organized in 4-row stripes (Figure D.2). Within a stripe the scan
//! visits every column left-to-right and, within a column, every row
//! top-to-bottom. Each bit-plane is coded with up to three passes:
//! significance propagation (SPP, D.3.1), magnitude refinement (MRP, D.3.3),
//! and cleanup (CU, D.4) with run-length mode. The most-significant bit-plane
//! carries a cleanup pass only, giving `num_passes = 3*nb - 2` where `nb` is
//! the number of magnitude bit-planes.

use super::error::CodecError;
use super::geometry::BandKind;
use super::mq::{
    setup_default_contexts, MqDecoder, MqEncoder, MqState, CTX_MR_START, CTX_RUN_LENGTH,
    CTX_SC_START, CTX_UNIFORM, CTX_ZC_START,
};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// A tier-1 coded code-block: the MQ codeword segment plus the metadata a
/// tier-2 packet header must signal to make it decodable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodedBlock {
    /// The single MQ codeword segment (empty for an all-zero block).
    pub data: Vec<u8>,
    /// Total number of coding passes emitted (`3*num_bitplanes - 2`, or 0).
    pub num_passes: u32,
    /// Number of magnitude bit-planes (0 for an all-zero block).
    pub num_bitplanes: u32,
}

/// Encode a single code-block of signed subband samples.
///
/// `coeffs` holds `w * h` samples in row-major order. Sign-magnitude coding is
/// internal. An all-zero block returns `CodedBlock { data: vec![], num_passes:
/// 0, num_bitplanes: 0 }`.
pub fn encode_code_block(coeffs: &[i32], w: usize, h: usize, band: BandKind) -> CodedBlock {
    debug_assert_eq!(coeffs.len(), w * h, "coeffs length must equal w*h");

    let max_mag = coeffs.iter().map(|v| v.unsigned_abs()).max().unwrap_or(0);
    if max_mag == 0 {
        return CodedBlock {
            data: Vec::new(),
            num_passes: 0,
            num_bitplanes: 0,
        };
    }
    let nb = 32 - max_mag.leading_zeros();

    let mut t = Tier1::new_encoder(w, h, band);
    // Pre-populate magnitudes and sign flags. `sc_context` only ever consults
    // the SIGN of a *significant* neighbour, and a neighbour's sign is coded at
    // the exact moment it becomes significant, so pre-seeding every negative
    // sample's SIGN bit is safe and matches what the decoder reconstructs.
    for y in 0..h {
        for x in 0..w {
            let v = coeffs[y * w + x];
            t.mag[y * w + x] = v.unsigned_abs();
            if v < 0 {
                let idx = t.flags.idx(x, y);
                t.flags.set(idx, SIGN);
            }
        }
    }

    let mut passes = 0u32;
    // Most-significant plane: cleanup only.
    t.cleanup_pass(nb - 1);
    passes += 1;
    // Remaining planes: SPP, MRP, CU.
    for p in (0..nb - 1).rev() {
        t.sig_prop_pass(p);
        t.mag_ref_pass(p);
        t.cleanup_pass(p);
        passes += 3;
    }

    let data = t.finish_encoder();
    CodedBlock {
        data,
        num_passes: passes,
        num_bitplanes: nb,
    }
}

/// Decode a single code-block back into signed subband samples.
///
/// `num_bitplanes` and `num_passes` come from the tier-2 packet header. Our
/// encoder always emits all `3*nb - 2` passes, but the decoder honours any
/// prefix `1..=3*nb-2` (tier-2 truncation) and never panics on malformed input:
/// the MQ decoder feeds synthetic 1-bits past end-of-data.
pub fn decode_code_block(
    data: &[u8],
    w: usize,
    h: usize,
    band: BandKind,
    num_bitplanes: u32,
    num_passes: u32,
) -> Result<Vec<i32>, CodecError> {
    if num_bitplanes == 0 {
        if num_passes != 0 {
            return Err(CodecError::InvalidData(
                "num_bitplanes == 0 but num_passes != 0".into(),
            ));
        }
        return Ok(vec![0i32; w * h]);
    }

    let nb = num_bitplanes;
    // Defensive shift guard: the coding passes shift `1 << (nb - 1)` down to
    // `1 << 0`. With `nb >= 32` the most-significant plane shift is `1 << 31`
    // or worse (a debug panic / release corruption), and a magnitude with bit
    // 31 set overflows the signed i32 reconstruction. Callers (the codestream
    // QCD validator) already reject this, but re-check here so no path can
    // reach the shift with an out-of-range plane count.
    if nb >= 32 {
        return Err(CodecError::InvalidData(format!(
            "num_bitplanes {nb} >= 32 exceeds the i32 coefficient range"
        )));
    }
    let max_passes = 3 * nb - 2;
    if num_passes == 0 || num_passes > max_passes {
        return Err(CodecError::InvalidData(format!(
            "num_passes {num_passes} out of range 1..={max_passes} for num_bitplanes {nb}"
        )));
    }

    let mut t = Tier1::new_decoder(data, w, h, band);

    let mut done = 0u32;
    // Pass 0 is always the most-significant-plane cleanup.
    t.cleanup_pass(nb - 1);
    done += 1;
    for p in (0..nb - 1).rev() {
        if done >= num_passes {
            break;
        }
        t.sig_prop_pass(p);
        done += 1;
        if done >= num_passes {
            break;
        }
        t.mag_ref_pass(p);
        done += 1;
        if done >= num_passes {
            break;
        }
        t.cleanup_pass(p);
        done += 1;
    }

    let mut out = vec![0i32; w * h];
    for y in 0..h {
        for x in 0..w {
            let ci = y * w + x;
            let m = t.mag[ci] as i32;
            let idx = t.flags.idx(x, y);
            out[ci] = if t.flags.get(idx, SIGN) { -m } else { m };
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Shared flag grid
// ---------------------------------------------------------------------------

/// Significant (σ): the sample has a coded 1 magnitude bit.
const SIG: u16 = 1 << 0;
/// Visited (π): coded in this bit-plane's significance-propagation pass.
const VISITED: u16 = 1 << 1;
/// Refined (σ'): has had at least one magnitude-refinement bit coded.
const REFINED: u16 = 1 << 2;
/// Sign: set when the sample is negative.
const SIGN: u16 = 1 << 3;

/// Per-sample coding state with a one-cell insignificant border all around, so
/// neighbourhood queries never need edge tests.
struct BlockFlags {
    flags: Vec<u16>,
    stride: usize,
}

impl BlockFlags {
    fn new(w: usize, h: usize) -> Self {
        let stride = w + 2;
        Self {
            flags: vec![0u16; stride * (h + 2)],
            stride,
        }
    }

    /// Linear index of sample `(x, y)` inside the bordered grid.
    #[inline]
    fn idx(&self, x: usize, y: usize) -> usize {
        (y + 1) * self.stride + (x + 1)
    }

    #[inline]
    fn get(&self, idx: usize, bit: u16) -> bool {
        self.flags[idx] & bit != 0
    }

    #[inline]
    fn set(&mut self, idx: usize, bit: u16) {
        self.flags[idx] |= bit;
    }

    /// Clear the π (VISITED) flag everywhere; run at the end of each cleanup.
    fn clear_visited(&mut self) {
        for f in &mut self.flags {
            *f &= !VISITED;
        }
    }
}

// ---------------------------------------------------------------------------
// Context formation (shared, ITU-T T.800 Tables D.1-D.4)
// ---------------------------------------------------------------------------

/// Zero-coding context for the LL/LH orientation ("H-major" table, T.800
/// Table D.1). `h`/`v`/`d` are significant horizontal/vertical/diagonal
/// neighbour counts. Returns a label in `0..=8`.
fn zc_ll(h: u32, v: u32, d: u32) -> u8 {
    if h == 2 {
        8
    } else if h == 1 {
        if v >= 1 {
            7
        } else if d >= 1 {
            6
        } else {
            5
        }
    } else if v == 2 {
        4
    } else if v == 1 {
        3
    } else if d >= 2 {
        2
    } else if d == 1 {
        1
    } else {
        0
    }
}

/// Zero-coding context for the HH orientation ("D-major" table, T.800
/// Table D.1). Returns a label in `0..=8`.
fn zc_hh(h: u32, v: u32, d: u32) -> u8 {
    let hv = h + v;
    if d >= 3 {
        8
    } else if d == 2 {
        if hv >= 1 {
            7
        } else {
            6
        }
    } else if d == 1 {
        if hv >= 2 {
            5
        } else if hv == 1 {
            4
        } else {
            3
        }
    } else if hv >= 2 {
        2
    } else if hv == 1 {
        1
    } else {
        0
    }
}

/// Orientation-aware zero-coding context (T.800 Table D.1). LL and LH use the
/// H-major table directly; HL swaps the horizontal and vertical roles; HH uses
/// the diagonal-major table.
fn zc_context(h: u32, v: u32, d: u32, band: BandKind) -> u8 {
    match band {
        BandKind::LL | BandKind::LH => zc_ll(h, v, d),
        BandKind::HL => zc_ll(v, h, d),
        BandKind::HH => zc_hh(h, v, d),
    }
}

// ---------------------------------------------------------------------------
// Tier-1 state machine (drives both directions)
// ---------------------------------------------------------------------------

enum Mq<'d> {
    Enc(MqEncoder),
    Dec(MqDecoder<'d>),
}

struct Tier1<'d> {
    mq: Mq<'d>,
    contexts: Vec<MqState>,
    flags: BlockFlags,
    /// Accumulated magnitudes: source data on encode, reconstruction on decode.
    mag: Vec<u32>,
    w: usize,
    h: usize,
    band: BandKind,
}

impl Tier1<'static> {
    fn new_encoder(w: usize, h: usize, band: BandKind) -> Self {
        Tier1 {
            mq: Mq::Enc(MqEncoder::new()),
            contexts: setup_default_contexts(),
            flags: BlockFlags::new(w, h),
            mag: vec![0u32; w * h],
            w,
            h,
            band,
        }
    }
}

impl<'d> Tier1<'d> {
    fn new_decoder(data: &'d [u8], w: usize, h: usize, band: BandKind) -> Self {
        Tier1 {
            mq: Mq::Dec(MqDecoder::new(data)),
            contexts: setup_default_contexts(),
            flags: BlockFlags::new(w, h),
            mag: vec![0u32; w * h],
            w,
            h,
            band,
        }
    }

    #[inline]
    fn encoding(&self) -> bool {
        matches!(self.mq, Mq::Enc(_))
    }

    /// The single coding primitive. On encode, `known` is coded in `ctx` and
    /// returned unchanged; on decode, `known` is ignored and the decoded bit is
    /// returned. Every pass routes through here, so encoder and decoder share a
    /// single control flow.
    #[inline]
    fn code(&mut self, ctx: usize, known: u8) -> u8 {
        match &mut self.mq {
            Mq::Enc(e) => {
                e.encode(known, &mut self.contexts[ctx]);
                known
            }
            Mq::Dec(dc) => dc.decode(&mut self.contexts[ctx]),
        }
    }

    fn finish_encoder(&mut self) -> Vec<u8> {
        match &mut self.mq {
            Mq::Enc(e) => {
                e.flush();
                e.bytes().to_vec()
            }
            Mq::Dec(_) => Vec::new(),
        }
    }

    // -- neighbourhood queries ----------------------------------------------

    /// `(h, v, d)` significant-neighbour counts for the sample at `idx`.
    #[inline]
    fn neighbor_counts(&self, idx: usize) -> (u32, u32, u32) {
        let s = self.flags.stride;
        let sig = |i: usize| (self.flags.flags[i] & SIG != 0) as u32;
        let h = sig(idx - 1) + sig(idx + 1);
        let v = sig(idx - s) + sig(idx + s);
        let d = sig(idx - s - 1) + sig(idx - s + 1) + sig(idx + s - 1) + sig(idx + s + 1);
        (h, v, d)
    }

    #[inline]
    fn has_sig_neighbor(&self, idx: usize) -> bool {
        let (h, v, d) = self.neighbor_counts(idx);
        h + v + d > 0
    }

    /// Signed sign contribution of the neighbour at `idx`: 0 if insignificant,
    /// +1 if significant-positive, -1 if significant-negative.
    #[inline]
    fn sign_contrib(&self, idx: usize) -> i32 {
        if self.flags.get(idx, SIG) {
            if self.flags.get(idx, SIGN) {
                -1
            } else {
                1
            }
        } else {
            0
        }
    }

    /// Sign-coding context and XOR bit (T.800 Tables D.2/D.3).
    fn sc_context_and_xor(&self, idx: usize) -> (u8, u8) {
        let s = self.flags.stride;
        let hc = (self.sign_contrib(idx - 1) + self.sign_contrib(idx + 1)).clamp(-1, 1);
        let vc = (self.sign_contrib(idx - s) + self.sign_contrib(idx + s)).clamp(-1, 1);
        match (hc, vc) {
            (1, 1) => (4, 0),
            (1, 0) => (3, 0),
            (1, -1) => (2, 0),
            (0, 1) => (1, 0),
            (0, 0) => (0, 0),
            (0, -1) => (1, 1),
            (-1, 1) => (2, 1),
            (-1, 0) => (3, 1),
            (-1, -1) => (4, 1),
            _ => unreachable!("clamped contributions are in -1..=1"),
        }
    }

    /// Magnitude-refinement context (T.800 Table D.4).
    fn mr_context(&self, idx: usize) -> u8 {
        if self.flags.get(idx, REFINED) {
            2
        } else if self.has_sig_neighbor(idx) {
            1
        } else {
            0
        }
    }

    // -- shared coding primitives -------------------------------------------

    /// Code the sign of the (just-significant) sample at `(idx, ci)` and record
    /// it in the SIGN flag.
    fn code_sign(&mut self, idx: usize) {
        let (sc_ctx, xor) = self.sc_context_and_xor(idx);
        let known_sym = if self.encoding() {
            (self.flags.get(idx, SIGN) as u8) ^ xor
        } else {
            0
        };
        let sym = self.code(CTX_SC_START + sc_ctx as usize, known_sym);
        if (sym ^ xor) == 1 {
            self.flags.set(idx, SIGN);
        }
    }

    /// Zero-coding of the plane-`p` significance bit for a not-yet-significant
    /// sample, followed by sign coding on a 1. Used by SPP and by cleanup
    /// normal mode.
    fn code_significance(&mut self, x: usize, y: usize, p: u32, zc_ctx: u8) {
        let idx = self.flags.idx(x, y);
        let ci = y * self.w + x;
        let known = ((self.mag[ci] >> p) & 1) as u8;
        let bit = self.code(CTX_ZC_START + zc_ctx as usize, known);
        if bit == 1 {
            self.flags.set(idx, SIG);
            self.mag[ci] |= 1u32 << p;
            self.code_sign(idx);
        }
    }

    // -- coding passes ------------------------------------------------------

    /// Significance-propagation pass (T.800 D.3.1). A sample is a candidate iff
    /// it is not yet significant and has at least one significant neighbour
    /// (LIVE state -- no snapshot). Every candidate is marked VISITED.
    fn sig_prop_pass(&mut self, p: u32) {
        let mut y0 = 0;
        while y0 < self.h {
            let rows = (self.h - y0).min(4);
            for x in 0..self.w {
                for r in 0..rows {
                    let y = y0 + r;
                    let idx = self.flags.idx(x, y);
                    if self.flags.get(idx, SIG) {
                        continue;
                    }
                    let (h, v, d) = self.neighbor_counts(idx);
                    let zc = zc_context(h, v, d, self.band);
                    if zc == 0 {
                        continue;
                    }
                    self.flags.set(idx, VISITED);
                    self.code_significance(x, y, p, zc);
                }
            }
            y0 += 4;
        }
    }

    /// Magnitude-refinement pass (T.800 D.3.3). Refines samples that are
    /// significant but were not coded in this plane's SPP (π clear).
    fn mag_ref_pass(&mut self, p: u32) {
        let mut y0 = 0;
        while y0 < self.h {
            let rows = (self.h - y0).min(4);
            for x in 0..self.w {
                for r in 0..rows {
                    let y = y0 + r;
                    let idx = self.flags.idx(x, y);
                    if !self.flags.get(idx, SIG) || self.flags.get(idx, VISITED) {
                        continue;
                    }
                    let mrc = self.mr_context(idx);
                    let ci = y * self.w + x;
                    let known = ((self.mag[ci] >> p) & 1) as u8;
                    let bit = self.code(CTX_MR_START + mrc as usize, known);
                    self.mag[ci] |= (bit as u32) << p;
                    self.flags.set(idx, REFINED);
                }
            }
            y0 += 4;
        }
    }

    /// Cleanup pass (T.800 D.4) with run-length mode. Codes samples that are
    /// still insignificant and were not visited by SPP, then clears all π flags.
    fn cleanup_pass(&mut self, p: u32) {
        let mut y0 = 0;
        while y0 < self.h {
            let rows = (self.h - y0).min(4);
            for x in 0..self.w {
                if rows == 4 && self.rl_eligible(x, y0) {
                    self.cleanup_rl(x, y0, p);
                } else {
                    for r in 0..rows {
                        let y = y0 + r;
                        let idx = self.flags.idx(x, y);
                        if self.flags.get(idx, SIG) || self.flags.get(idx, VISITED) {
                            continue;
                        }
                        let (h, v, d) = self.neighbor_counts(idx);
                        let zc = zc_context(h, v, d, self.band);
                        self.code_significance(x, y, p, zc);
                    }
                }
            }
            y0 += 4;
        }
        self.flags.clear_visited();
    }

    /// Whether the 4 samples of a full stripe column all qualify for run-length
    /// mode: not significant, not visited, and zero zero-coding context.
    fn rl_eligible(&self, x: usize, y0: usize) -> bool {
        (0..4).all(|r| {
            let idx = self.flags.idx(x, y0 + r);
            if self.flags.get(idx, SIG) || self.flags.get(idx, VISITED) {
                return false;
            }
            let (h, v, d) = self.neighbor_counts(idx);
            zc_context(h, v, d, self.band) == 0
        })
    }

    /// Run-length coding of one eligible full stripe column (T.800 D.4.2).
    fn cleanup_rl(&mut self, x: usize, y0: usize, p: u32) {
        // Encoder derives the run bit and first-significant index from the
        // magnitudes; the decoder receives (0, 0) and reads them back.
        let (run_known, r_known) = if self.encoding() {
            let mut first = None;
            for r in 0..4 {
                let ci = (y0 + r) * self.w + x;
                if (self.mag[ci] >> p) & 1 == 1 {
                    first = Some(r as u8);
                    break;
                }
            }
            match first {
                Some(r) => (1u8, r),
                None => (0, 0),
            }
        } else {
            (0, 0)
        };

        let run = self.code(CTX_RUN_LENGTH, run_known);
        if run == 0 {
            return;
        }

        // Two uniform-context bits, MSB first, give the first-significant row.
        let b1 = self.code(CTX_UNIFORM, (r_known >> 1) & 1);
        let b0 = self.code(CTX_UNIFORM, r_known & 1);
        let r = ((b1 << 1) | b0) as usize;

        // Sample r becomes significant now; code its sign.
        let idx = self.flags.idx(x, y0 + r);
        let ci = (y0 + r) * self.w + x;
        self.flags.set(idx, SIG);
        self.mag[ci] |= 1u32 << p;
        self.code_sign(idx);

        // Samples r+1..4 are coded normally (rows 0..r stay insignificant).
        for rr in (r + 1)..4 {
            let y = y0 + rr;
            let nidx = self.flags.idx(x, y);
            let (h, v, d) = self.neighbor_counts(nidx);
            let zc = zc_context(h, v, d, self.band);
            self.code_significance(x, y, p, zc);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
