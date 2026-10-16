//! Multi-component, lossless JPEG 2000 codestream writer.
//!
//! The `j2k` module is a fork of the MIT OR Apache-2.0 `pure_jpeg2k` 2.0.0 crate (JP Fielding,
//! <https://github.com/jpfielding/dicos.rs>): its reversible 5/3 wavelet, EBCOT tier-1 and
//! tier-2 packet coding, MQ coder and marker writers, checked against OpenJPEG upstream. The
//! upstream encoder writes one 16-bit component; this file writes up to four (gray, gray and
//! alpha, RGB, RGBA) in one tile, LRCP order (resolution outermost, then component), one layer,
//! no component transform. The decoder half of the fork is unused: reading is `hayro-jpeg2000`.

use super::bitstream::ByteWriter;
use super::dwt;
use super::ebcot::{CodedBlock, encode_code_block};
use super::error::CodecError;
use super::geometry::{BandKind, build_geometry};
use super::markers::*;
use super::packet::{PrecinctState, write_packet};
use super::tile::{decomposition_level, ensure_single_precinct, gather};

// ---------------------------------------------------------------------------
// Profile constants and the single-source-of-truth quantization helpers
// ---------------------------------------------------------------------------

/// Component bit depth `RI` (unsigned 16-bit). Used as the reversible dynamic
/// range in the E-10 exponent formula.
const BIT_DEPTH: u8 = 16;
/// Guard bits `G` (T.800 E.1) for our profile.
const GUARD_BITS: u8 = 2;
/// DC level shift for unsigned 16-bit samples: `2^(BIT_DEPTH-1) = 32768`.
const DC_SHIFT: i32 = 1 << (BIT_DEPTH - 1);
/// Allocation cap on `Xsiz · Ysiz` (samples), guarding hostile SIZ dimensions.
const MAX_IMAGE_AREA: u64 = 1 << 28;

/// `log2` of the nominal sub-band gain (T.800 E.1 / Table E.1): 0/1/1/2.
fn gain_log2(kind: BandKind) -> u8 {
    match kind {
        BandKind::LL => 0,
        BandKind::HL | BandKind::LH => 1,
        BandKind::HH => 2,
    }
}

/// QCD exponent `εb` for the reversible, no-quantization case (T.800 E-10):
/// `εb = RI + log2(gain_b)`. Level-independent for the reversible transform.
///
/// This is the single source of truth shared by the QCD marker builder and the
/// `mb_for` closure handed to the tile pipeline.
fn qcd_epsilon(kind: BandKind) -> u8 {
    BIT_DEPTH + gain_log2(kind)
}

/// `Mb = εb + G − 1`, the maximum magnitude bit-plane count for a band.
fn mb_from_epsilon(epsilon: u8, guard_bits: u8) -> u32 {
    epsilon as u32 + guard_bits as u32 - 1
}

/// QCD `SPqcd` exponents in T.800 spec order for `num_levels` decompositions:
/// `LL_NL` first, then for `n = NL … 1` the triple `HL_n, LH_n, HH_n`
/// (just `[LL]` when `NL = 0`). Length `3·NL + 1`.
fn qcd_epsilons(num_levels: u8) -> Vec<i16> {
    let mut v = Vec::with_capacity(3 * num_levels as usize + 1);
    v.push(qcd_epsilon(BandKind::LL) as i16);
    for _ in 0..num_levels {
        v.push(qcd_epsilon(BandKind::HL) as i16);
        v.push(qcd_epsilon(BandKind::LH) as i16);
        v.push(qcd_epsilon(BandKind::HH) as i16);
    }
    v
}

/// Index into the spec-order QCD exponent list for band `kind` at decomposition
/// `level` (`1..=NL` for detail bands, `NL` for LL).
fn qcd_band_index(kind: BandKind, level: u8, num_levels: u8) -> usize {
    match kind {
        BandKind::LL => 0,
        BandKind::HL => 1 + 3 * (num_levels - level) as usize,
        BandKind::LH => 2 + 3 * (num_levels - level) as usize,
        BandKind::HH => 3 + 3 * (num_levels - level) as usize,
    }
}

// ---------------------------------------------------------------------------
// Codestream writer (encoder side)
// ---------------------------------------------------------------------------

struct CodestreamWriter {
    bw: ByteWriter,
}

impl CodestreamWriter {
    fn new() -> Self {
        Self {
            bw: ByteWriter::new(),
        }
    }

    fn write_soc(&mut self) {
        self.bw.write_u16(MARKER_SOC);
    }

    fn write_siz(&mut self, siz: &SizMarker) {
        self.bw.write_u16(MARKER_SIZ);
        // Lsiz = 38 + 3 · Csiz.
        let length = 38 + 3 * siz.components.len() as u16;
        self.bw.write_u16(length);
        self.bw.write_u16(siz.rsiz);
        self.bw.write_u32(siz.x_siz);
        self.bw.write_u32(siz.y_siz);
        self.bw.write_u32(siz.x_osiz);
        self.bw.write_u32(siz.y_osiz);
        self.bw.write_u32(siz.x_tsiz);
        self.bw.write_u32(siz.y_tsiz);
        self.bw.write_u32(siz.x_tosiz);
        self.bw.write_u32(siz.y_tosiz);
        self.bw.write_u16(siz.components.len() as u16);
        for comp in &siz.components {
            let mut ssiz = comp.precision - 1;
            if comp.signed {
                ssiz |= 0x80;
            }
            self.bw.write_u8(ssiz);
            self.bw.write_u8(comp.x_rsiz);
            self.bw.write_u8(comp.y_rsiz);
        }
    }

    fn write_cod(&mut self, cod: &CodMarker) {
        self.bw.write_u16(MARKER_COD);
        let mut length = 12u16;
        if cod.scod & CODING_STYLE_PRECINCTS_USER != 0 {
            length += cod.precinct_sizes.len() as u16;
        }
        self.bw.write_u16(length);
        self.bw.write_u8(cod.scod);
        self.bw.write_u8(cod.progression as u8);
        self.bw.write_u16(cod.num_layers);
        self.bw.write_u8(cod.mct);
        self.bw.write_u8(cod.decomp_levels);
        self.bw.write_u8(cod.cb_width_exp);
        self.bw.write_u8(cod.cb_height_exp);
        self.bw.write_u8(cod.cb_style);
        self.bw.write_u8(cod.transform as u8);
        if cod.scod & CODING_STYLE_PRECINCTS_USER != 0 {
            self.bw.write_bytes(&cod.precinct_sizes);
        }
    }

    fn write_qcd(&mut self, qcd: &QcdMarker) {
        self.bw.write_u16(MARKER_QCD);
        let length = 3 + qcd.step_sizes.len() as u16;
        self.bw.write_u16(length);
        let sqcd = (qcd.guard_bits << 5) | (qcd.sqcd & 0x1F);
        self.bw.write_u8(sqcd);
        // Reversible (no-quantization) style: each SPqcd entry is one byte,
        // carrying `εb` in its high 5 bits (`εb << 3`).
        for &eps in &qcd.step_sizes {
            self.bw.write_u8((eps << 3) as u8);
        }
    }

    fn write_sot(&mut self, sot: &SotMarker) {
        self.bw.write_u16(MARKER_SOT);
        self.bw.write_u16(10); // Lsot (fixed for one tile-part)
        self.bw.write_u16(sot.tile_index);
        self.bw.write_u32(sot.tile_part_len);
        self.bw.write_u8(sot.tile_part_idx);
        self.bw.write_u8(sot.num_tile_parts);
    }

    fn write_sod(&mut self) {
        self.bw.write_u16(MARKER_SOD);
    }

    fn write_eoc(&mut self) {
        self.bw.write_u16(MARKER_EOC);
    }

    fn write_bytes(&mut self, data: &[u8]) {
        self.bw.write_bytes(data);
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bw.into_bytes()
    }
}

fn io_err(e: std::io::Error) -> CodecError {
    CodecError::Io(e)
}


/// Encode `planes` (one `w * h` row-major plane of unsigned 16-bit samples per component) into a
/// lossless JPEG 2000 codestream: 5 decomposition levels, 64 x 64 code-blocks.
pub fn encode_planes(planes: &[Vec<u16>], width: u32, height: u32) -> Result<Vec<u8>, CodecError> {
    if planes.is_empty() || planes.len() > 4 {
        return Err(CodecError::Unsupported("1 to 4 components".into()));
    }
    if width == 0 || height == 0 {
        return Err(CodecError::InvalidData("image has zero dimension".into()));
    }
    let n = (width as usize).checked_mul(height as usize).filter(|n| *n <= MAX_IMAGE_AREA as usize).ok_or_else(|| CodecError::InvalidData("image too large".into()))?;
    if planes.iter().any(|p| p.len() != n) {
        return Err(CodecError::DimensionMismatch { expected: n, actual: planes[0].len() });
    }
    let (num_levels, xcb, ycb) = (5u8, 6u8, 6u8);
    let stride = width as usize;
    let geom = build_geometry(width, height, num_levels, xcb, ycb);
    ensure_single_precinct(&geom)?;

    // DC shift and forward transform of every component.
    let bufs: Vec<Vec<i32>> = planes
        .iter()
        .map(|p| {
            let mut buf: Vec<i32> = p.iter().map(|&v| v as i32 - DC_SHIFT).collect();
            dwt::forward_multi_level_conformant(&mut buf, stride, height as usize, num_levels as usize);
            buf
        })
        .collect();

    let mut tile = Vec::new();
    for res in &geom.resolutions {
        let level = decomposition_level(res.r, num_levels);
        let mbs: Vec<u32> = res.bands.iter().map(|b| mb_from_epsilon(qcd_epsilon(b.kind), GUARD_BITS)).collect();
        for buf in &bufs {
            let mut states: Vec<PrecinctState> = res.bands.iter().map(|b| PrecinctState::new(b.cb_grid_w, b.cb_grid_h)).collect();
            let mut all_blocks: Vec<Vec<Option<CodedBlock>>> = Vec::with_capacity(res.bands.len());
            for (band, mb) in res.bands.iter().zip(&mbs) {
                let mut blocks = Vec::with_capacity(band.cbs.len());
                for cb in &band.cbs {
                    let coded = encode_code_block(&gather(buf, stride, band, cb), cb.rect.width() as usize, cb.rect.height() as usize, band.kind);
                    if coded.num_bitplanes > *mb {
                        return Err(CodecError::InvalidData(format!("code-block needs {} bit-planes but Mb is {mb} (level {level})", coded.num_bitplanes)));
                    }
                    blocks.push(if coded.num_bitplanes == 0 { None } else { Some(coded) });
                }
                all_blocks.push(blocks);
            }
            let refs: Vec<&[Option<CodedBlock>]> = all_blocks.iter().map(|v| v.as_slice()).collect();
            write_packet(&mut states, &res.bands, &refs, &mbs, &mut tile)?;
        }
    }

    let comps: Vec<ComponentInfo> = (0..planes.len()).map(|_| ComponentInfo { precision: BIT_DEPTH, signed: false, x_rsiz: 1, y_rsiz: 1 }).collect();
    let siz = build_siz(width, height, comps, 0, 0);
    let cod = build_cod(num_levels, xcb, ycb);
    let qcd = QcdMarker { sqcd: 0, guard_bits: GUARD_BITS, step_sizes: qcd_epsilons(num_levels) };
    let mut cw = CodestreamWriter::new();
    cw.write_soc();
    cw.write_siz(&siz);
    cw.write_cod(&cod);
    cw.write_qcd(&qcd);
    let psot = u32::try_from(tile.len() + 14).map_err(|_| CodecError::InvalidData("tile-part length overflow".into()))?;
    cw.write_sot(&SotMarker { tile_index: 0, tile_part_len: psot, tile_part_idx: 0, num_tile_parts: 1 });
    cw.write_sod();
    cw.write_bytes(&tile);
    cw.write_eoc();
    Ok(cw.into_bytes())
}
