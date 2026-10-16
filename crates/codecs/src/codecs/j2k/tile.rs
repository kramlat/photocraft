//! Conformant T.800 tile-component pipeline (Workstream 1 step 8).
//!
//! Encodes/decodes a single tile-component through the full lossless pipeline:
//! reversible 5/3 DWT ⇄ resolution/band/code-block geometry ⇄ EBCOT tier-1 ⇄
//! LRCP tier-2 packets. For our profile (1 tile, 1 component, 1 layer, 1
//! precinct per band, LRCP progression) the packet order is simply resolution
//! `r` ascending, one packet per resolution covering that resolution's bands.
//!
//! `Mb` (the maximum number of magnitude bit-planes for a band, `Mb = εb + G −
//! 1`) is supplied per band by the `mb_for(kind, level)` closure, where `level`
//! is the band's *decomposition level* (`NL` for the LL band, `NL..=1` for the
//! detail bands) — the same key the QCD marker uses to order its step sizes.
//! Step 9 (codestream) will pass a closure computed from the parsed `QcdMarker`;
//! the tests here use a constant.
//!
//! This module is not yet wired into the public `encode`/`decode` (still the
//! frozen `legacy` path) — that is Workstream 1 step 9.

use super::dwt;
use super::ebcot::{decode_code_block, encode_code_block, CodedBlock};
use super::error::CodecError;
use super::geometry::{build_geometry, Band, BandKind, CodeBlockGeom, TileGeometry};
use super::packet::{read_packet, write_packet, PrecinctState};

/// Maximum tile area (samples) we will allocate for, guarding against hostile
/// dimensions (matches the codestream-level cap in the plan).
const MAX_TILE_AREA: usize = 1 << 28;

/// Decomposition level of a resolution's bands: `NL` for the coarsest
/// resolution (`r = 0`, the LL band) and for `r = 1`, decreasing to `1` at the
/// finest resolution. This mirrors `geometry`'s `n = NL − r + 1`.
pub(super) fn decomposition_level(r: u8, num_levels: u8) -> u8 {
    if r == 0 {
        num_levels
    } else {
        num_levels - r + 1
    }
}

/// Validate dimensions and return the sample count `w * h`.
pub(super) fn checked_area(w: u32, h: u32) -> Result<usize, CodecError> {
    if w == 0 || h == 0 {
        return Err(CodecError::InvalidData("tile has a zero dimension".into()));
    }
    let n = (w as usize)
        .checked_mul(h as usize)
        .ok_or_else(|| CodecError::InvalidData("tile dimensions overflow usize".into()))?;
    if n > MAX_TILE_AREA {
        return Err(CodecError::InvalidData(format!(
            "tile area {n} exceeds the {MAX_TILE_AREA} sample cap"
        )));
    }
    Ok(n)
}

/// Reject geometries that span more than one precinct per resolution.
///
/// The tier-2 pipeline emits exactly one packet per resolution and holds one
/// [`PrecinctState`] per band, so it only handles the single-precinct case
/// (every realistic tile with the default PPx=PPy=15 precinct exponent). A
/// dimension large enough to cross the 2^15 precinct span (e.g. > 32768 in a
/// resolution) would be silently mispacked; reject it loudly instead, matching
/// the crate's "legal-but-unsupported is rejected" contract.
pub(super) fn ensure_single_precinct(geom: &TileGeometry) -> Result<(), CodecError> {
    for res in &geom.resolutions {
        if res.num_precincts_w as u64 * res.num_precincts_h as u64 > 1 {
            return Err(CodecError::Unsupported(
                "multi-precinct images not supported".into(),
            ));
        }
    }
    Ok(())
}

/// Gather a code-block's coefficients out of the packed Mallat tile buffer.
///
/// The block rectangle is band-native; its physical position in the full tile
/// buffer is `band.placement + cb.rect.(x0, y0)`, indexed at the full tile
/// `stride`.
pub(super) fn gather(buf: &[i32], stride: usize, band: &Band, cb: &CodeBlockGeom) -> Vec<i32> {
    let bw = cb.rect.width() as usize;
    let bh = cb.rect.height() as usize;
    let x_off = band.placement.x_off as usize + cb.rect.x0 as usize;
    let y_off = band.placement.y_off as usize + cb.rect.y0 as usize;
    let mut v = vec![0i32; bw * bh];
    for yy in 0..bh {
        let src = (y_off + yy) * stride + x_off;
        v[yy * bw..yy * bw + bw].copy_from_slice(&buf[src..src + bw]);
    }
    v
}

/// Scatter a code-block's decoded coefficients back into the Mallat buffer.
fn scatter(buf: &mut [i32], stride: usize, band: &Band, cb: &CodeBlockGeom, coeffs: &[i32]) {
    let bw = cb.rect.width() as usize;
    let bh = cb.rect.height() as usize;
    let x_off = band.placement.x_off as usize + cb.rect.x0 as usize;
    let y_off = band.placement.y_off as usize + cb.rect.y0 as usize;
    for yy in 0..bh {
        let dst = (y_off + yy) * stride + x_off;
        buf[dst..dst + bw].copy_from_slice(&coeffs[yy * bw..yy * bw + bw]);
    }
}

/// Encode one tile-component (`w × h` samples, row-major) to a concatenation of
/// LRCP packets.
///
/// `mb_for(kind, level)` supplies `Mb` per band. An `Err` is returned if any
/// code-block needs more magnitude bit-planes than its band's `Mb` allows.
pub fn encode_tile(
    component: &[i32],
    w: u32,
    h: u32,
    num_levels: u8,
    xcb: u8,
    ycb: u8,
    mb_for: impl Fn(BandKind, u8) -> u32,
) -> Result<Vec<u8>, CodecError> {
    let n = checked_area(w, h)?;
    if component.len() != n {
        return Err(CodecError::DimensionMismatch {
            expected: n,
            actual: component.len(),
        });
    }
    let stride = w as usize;

    let geom = build_geometry(w, h, num_levels, xcb, ycb);
    ensure_single_precinct(&geom)?;

    // Forward DWT into a packed Mallat buffer.
    let mut buf = component.to_vec();
    dwt::forward_multi_level_conformant(&mut buf, stride, h as usize, num_levels as usize);

    let mut out = Vec::new();

    for res in &geom.resolutions {
        let level = decomposition_level(res.r, num_levels);
        let bands: Vec<Band> = res.bands.clone();
        let mut states: Vec<PrecinctState> = bands
            .iter()
            .map(|b| PrecinctState::new(b.cb_grid_w, b.cb_grid_h))
            .collect();
        let mut mbs: Vec<u32> = Vec::with_capacity(bands.len());
        let mut all_blocks: Vec<Vec<Option<CodedBlock>>> = Vec::with_capacity(bands.len());

        for band in &bands {
            let mb = mb_for(band.kind, level);
            mbs.push(mb);
            let mut blocks: Vec<Option<CodedBlock>> = Vec::with_capacity(band.cbs.len());
            for cb in &band.cbs {
                let bw = cb.rect.width() as usize;
                let bh = cb.rect.height() as usize;
                let coeffs = gather(&buf, stride, band, cb);
                let coded = encode_code_block(&coeffs, bw, bh, band.kind);
                // `mb` is caller-supplied (from QCD at step 9), so a too-small
                // Mb is a runtime error, not an internal invariant — no
                // debug_assert here, which would mask the clean Err in tests.
                if coded.num_bitplanes > mb {
                    return Err(CodecError::InvalidData(format!(
                        "code-block needs {} bit-planes but Mb is {mb} (band {:?})",
                        coded.num_bitplanes, band.kind
                    )));
                }
                blocks.push(if coded.num_bitplanes == 0 {
                    None
                } else {
                    Some(coded)
                });
            }
            all_blocks.push(blocks);
        }

        let block_refs: Vec<&[Option<CodedBlock>]> =
            all_blocks.iter().map(|v| v.as_slice()).collect();
        write_packet(&mut states, &bands, &block_refs, &mbs, &mut out)?;
    }

    Ok(out)
}

/// Decode a tile-component from a concatenation of LRCP packets.
pub fn decode_tile(
    bitstream: &[u8],
    w: u32,
    h: u32,
    num_levels: u8,
    xcb: u8,
    ycb: u8,
    mb_for: impl Fn(BandKind, u8) -> u32,
) -> Result<Vec<i32>, CodecError> {
    let n = checked_area(w, h)?;
    let stride = w as usize;

    let geom = build_geometry(w, h, num_levels, xcb, ycb);
    ensure_single_precinct(&geom)?;
    let mut buf = vec![0i32; n];
    let mut pos = 0usize;

    for res in &geom.resolutions {
        let level = decomposition_level(res.r, num_levels);
        let bands: Vec<Band> = res.bands.clone();
        let mut states: Vec<PrecinctState> = bands
            .iter()
            .map(|b| PrecinctState::new(b.cb_grid_w, b.cb_grid_h))
            .collect();
        let mbs: Vec<u32> = bands.iter().map(|b| mb_for(b.kind, level)).collect();

        if pos > bitstream.len() {
            return Err(CodecError::InvalidData("tile bitstream truncated".into()));
        }
        let (contribs, header_len) = read_packet(&mut states, &bands, &mbs, &bitstream[pos..])?;
        let mut body_pos = pos
            .checked_add(header_len)
            .ok_or_else(|| CodecError::InvalidData("packet offset overflow".into()))?;

        for (bi, band) in bands.iter().enumerate() {
            let mb = mbs[bi];
            for (k, cb) in band.cbs.iter().enumerate() {
                let bw = cb.rect.width() as usize;
                let bh = cb.rect.height() as usize;
                let coeffs = match &contribs[bi][k] {
                    None => vec![0i32; bw * bh],
                    Some(c) => {
                        if c.zero_bitplanes > mb {
                            return Err(CodecError::InvalidData(format!(
                                "zero-bit-planes {} exceed Mb {mb}",
                                c.zero_bitplanes
                            )));
                        }
                        let num_bitplanes = mb - c.zero_bitplanes;
                        if num_bitplanes == 0 {
                            return Err(CodecError::InvalidData(
                                "included block with zero magnitude bit-planes".into(),
                            ));
                        }
                        let max_passes = 3 * num_bitplanes - 2;
                        if c.num_passes > max_passes {
                            return Err(CodecError::InvalidData(format!(
                                "num_passes {} exceeds 3*{num_bitplanes}-2 = {max_passes}",
                                c.num_passes
                            )));
                        }
                        let end = body_pos.checked_add(c.len).ok_or_else(|| {
                            CodecError::InvalidData("body offset overflow".into())
                        })?;
                        if end > bitstream.len() {
                            return Err(CodecError::InvalidData(
                                "code-block body truncated".into(),
                            ));
                        }
                        let data = &bitstream[body_pos..end];
                        body_pos = end;
                        decode_code_block(data, bw, bh, band.kind, num_bitplanes, c.num_passes)?
                    }
                };
                scatter(&mut buf, stride, band, cb, &coeffs);
            }
        }
        pos = body_pos;
    }

    dwt::inverse_multi_level_conformant(&mut buf, stride, h as usize, num_levels as usize);
    Ok(buf)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
