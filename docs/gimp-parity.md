# GIMP parity: filters, layer modes, and what a GIMP-made PSD carries

> **Last reviewed:** 2026-10-16 · **Reference:** GIMP master (3.3.1, toward 3.4), `app/actions/filters-actions.c`
> and `plug-ins/file-psd/` · Status marks are a first read against the Filter menu and `crates/algo`,
> not yet checked by running each filter; fix a row when you verify it.

## What a PSD from GIMP actually contains

- **Layer modes.** GIMP's PSD export (`plug-ins/file-psd/psd-util.c`, `layer_mode_map`) maps every GIMP
  mode onto a Photoshop key (`norm`, `mul `, `scrn`, `over`, `diff`, `lddg`, `fsub`, `dark`, `lite`, `hue `,
  `sat `, `colr`, `lum `, `fdiv`, `div `, …) and writes anything else as Normal. A GIMP-made PSD therefore
  never needs a mode PhotoCraft lacks: `BlendMode::LAYER_MODES` covers all of Photoshop's. GIMP-only modes
  (Grain Extract/Merge, Luma/Luminance, LCh Hue/Chroma/Color/Lightness, Addition's non-legacy maths) only
  matter when reading an **XCF**.
- **Layer effects.** GIMP writes no `lfx2` block, so it has no Photoshop layer effects to be missing.
  GIMP's non-destructive filters are GEGL operations on the layer; its PSD export merges them into pixels.
- **So the real gaps are** (1) the GEGL filter set below and (2) reading `.xcf` (filters and modes survive
  there as data).

## Filters still to do

72 GIMP operations had no PhotoCraft counterpart when this list was made. 31 are done (the
table below), `rgb-clip` is not applicable (integer documents never leave 0–1; 32-bit documents
clamp on display), and **40 are left**:

- **Blur and enhance (9):** variable-blur, mean-curvature-blur, snn-mean, wavelet-decompose,
  antialias, stress, fattal02, mantiuk06, reinhard05 (the last three are tone mappers).
- **Distort and map (15):** apply-lens, panorama-projection, stereographic-projection, tile-glass,
  tile-paper, fractal-trace, illusion, recursive-transform, video-degradation, engrave, bump-map,
  normal-map, tile-seamless, shift, curve-bend.
- **Artistic and light (8):** cartoon, cubism, photocopy, softglow, bloom, supernova, sparkle,
  gradient-flare.
- **Edge detect (1):** edge-neon.
- **Colours (1):** color-rotate.
- **Render (2):** diffraction-patterns, fractal-explorer.
- **Misc (4):** distance-transform, value-propagate, waterpixels, slic.

Operations GIMP has that PhotoCraft already covers under a Photoshop name (blurs, sharpens,
Median, High Pass, Unsharp Mask, Lens Correction, Polar Coordinates, Spherize, Twirl/Pinch, Ripple,
Wave, Wind, Emboss, Find Edges, Add Noise, Custom, Color to Alpha, Channel Mixer, Black & White,
Photo Filter, Invert, Posterize, Threshold, Shadows/Highlights, Lens Flare, Clouds, Fibers…) are not
listed.

## Suggested order

1. **Cheap, self-contained maths first:** the remaining distorts (shift, curve-bend, tile-glass,
   tile-paper, illusion, engrave, video-degradation), colour-rotate, edge-neon, the artistic filters.
2. **Heavier:** tone mappers (fattal02, mantiuk06, reinhard05), variable/curvature blurs,
   projections, the segmentation filters (slic, waterpixels).

## Licence note

GIMP is GPL-3.0-or-later and GEGL is LGPL-3.0-or-later (many operations GPL-3.0-or-later); this repository is
`MIT OR Apache-2.0` and its rules are clean-room (`AGENTS.md`). Use GIMP as a *behaviour reference*; do not
copy its source or link GEGL/babl while the repository keeps its current licence. Relicensing the fork to
GPL-3.0-or-later would lift that restriction; decide before porting any operation.
