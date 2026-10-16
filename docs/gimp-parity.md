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

## Filters GIMP offers that PhotoCraft has no counterpart for

Names are GEGL operation names. "Has" means a PhotoCraft filter does the same job under another name.

**Blur / enhance:** variable-blur, mean-curvature-blur, snn-mean, bilateral-style selective gaussian
(has: Surface Blur), wavelet-decompose (plug-in), antialias, stress, noise-reduction (has: Reduce Noise),
high-pass (has), unsharp-mask (has), local-contrast / retinex-style tone ops: fattal02, mantiuk06, reinhard05.

**Distorts / map:** apply-lens, lens-distortion (has: Lens Correction), mirrors (kaleidoscope),
panorama-projection, stereographic-projection (little planet), polar-coordinates (has), spherize (has),
whirl-pinch (has: Twirl, Pinch), ripple (has), waves (has), tile-glass, tile-paper, fractal-trace, illusion,
recursive-transform, video-degradation, engrave, newsprint (has: Color Halftone, partly), bump-map,
normal-map, tile-seamless, displace (has), shift, wind (has), curve-bend.

**Artistic / light:** cartoon, cubism, photocopy, softglow, bloom, long-shadow, vignette (has, Lens
Correction), supernova, lens-flare (has), sparkle, gradient-flare.

**Noise:** noise-rgb / noise-hsv / noise-cie-lch variants (has: Add Noise), noise-hurl, noise-pick,
noise-slur, noise-spread.

**Edge detect:** edge-sobel, edge-laplace, edge-neon, edge (generic), difference-of-gaussians
(has: Find Edges).

**Colours:** alien-map, color-exchange, color-rotate, color-enhance, component-extract, value-invert,
invert-gamma, invert-linear, rgb-clip, stretch-contrast, stretch-contrast-hsv, mono-mixer (has: Black &
White), dither, bayer-matrix, color-temperature (has: Camera Raw), channel-mixer (has), saturation (has),
hue-chroma (has), sepia (has: Photo Filter).

**Render:** checkerboard, grid, maze, plasma, sinus, spiral, linear-sinusoid, diffraction-patterns,
cell-noise, simplex-noise, perlin-noise, noise-solid (has: Clouds, Fibers), fractal-explorer.

**Misc:** distance-transform, value-propagate, waterpixels, slic, long-shadow, convolution-matrix
(has: Custom), gaussian-blur-selective (has).

## Naming policy

The UI speaks Photoshop. A GIMP-only operation gets the name and parameter wording a Photoshop user
would expect (Fuzziness, Amount, Radius, Channel), never GIMP's operation name. Script procedures keep
their GIMP names (`gimp-image-new`) because existing scripts depend on them, but each maps onto a
PhotoCraft command.

## Added so far (clean-room, from published definitions)

| PhotoCraft | Menu | Covers |
|---|---|---|
| Invert Brightness | Filter › Other | value-invert |
| Invert (Linear Light) | Filter › Other | invert-linear |
| Replace Color Range… | Filter › Other | color-exchange |
| Channel Waves… | Filter › Other | alien-map |
| Extract Channel… | Filter › Other | component-extract |
| Gaussian Difference… | Filter › Other | difference-of-gaussians |
| Edge Detection… (Sobel, Prewitt, Roberts, Gradient, Differential, Laplace) | Filter › Stylize | edge-sobel, edge-laplace, edge |

## Suggested order

1. **XCF import** (`photocraft-io`): layers, groups, masks, channels, paths, modes; the data the filters
   above would otherwise be applied to.
2. **Cheap, self-contained maths first:** the Colours and Edge-detect groups, Render patterns, Noise variants.
3. **Heavier:** tone mappers (fattal02, mantiuk06, reinhard05), variable/curvature blurs, projections.

## Licence note

GIMP is GPL-3.0-or-later and GEGL is LGPL-3.0-or-later (many operations GPL-3.0-or-later); this repository is
`MIT OR Apache-2.0` and its rules are clean-room (`AGENTS.md`). Use GIMP as a *behaviour reference*; do not
copy its source or link GEGL/babl while the repository keeps its current licence. Relicensing the fork to
GPL-3.0-or-later would lift that restriction; decide before porting any operation.
