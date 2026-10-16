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

72 GIMP operations had no PhotoCraft counterpart when this list was made. All are now done or
covered (the table below, plus the existing features named under it), except `rgb-clip`, which is
not applicable: integer documents never leave 0–1 and 32-bit documents clamp on display. **None are
left.** What remains is testing against GIMP (see [parity-testing.md](parity-testing.md)) and fixing
what that finds.

Found to be covered already by existing filters (so not new work): color-rotate (Hue/Saturation's
colour ranges), edge-neon (Filter Gallery › Glowing Edges), cartoon (Poster Edges), photocopy
(Photocopy), softglow and bloom (Diffuse Glow), invert-gamma (Invert), gradient-flare (Lens
Flare, Supernova and Sparkle), variable-blur (Lens Blur with a layer-mask depth map, and Field Blur
pins).

Operations GIMP has that PhotoCraft already covers under a Photoshop name (blurs, sharpens,
Median, High Pass, Unsharp Mask, Lens Correction, Polar Coordinates, Spherize, Twirl/Pinch, Ripple,
Wave, Wind, Emboss, Find Edges, Add Noise, Custom, Color to Alpha, Channel Mixer, Black & White,
Photo Filter, Invert, Posterize, Threshold, Shadows/Highlights, Lens Flare, Clouds, Fibers…) are not
listed.

## File formats

GIMP's `plug-ins/` file loaders and savers against PhotoCraft. ✅ = read and write done (or the
direction GIMP offers), ⬜ = to do. Adobe column: whether Photoshop has it too (see
[file-format-parity.md](file-format-parity.md)).

| GIMP format | Status | Adobe |
|---|---|---|
| PNG, JPEG, TIFF, WebP, GIF, BMP, TGA, ICO, QOI, PNM, EXR, HDR, FITS, DDS (r), HEIF (r), XPM (r), PSD, SVG (r), OpenRaster, XCF, PCX | ✅ | most |
| `.gbr`, `.gih`, `.pat` | ✅ (import) | `.pat` only |
| Sun Raster, SGI, XBM, Farbfeld, XWD, Alias PIX | ✅ | no |
| WBMP | ✅ | yes |
| Cineon (r) | ✅ | yes |
| PDF (load), PostScript/EPS | ⬜ | yes (PDF, EPS) |
| JPEG XL (lossless write), JPEG 2000 (lossless write) | ✅ | yes, yes |
| DICOM, Amiga IFF | ✅ | yes |
| Photoshop Raw / raw data | ⬜ | yes |
| ICNS (read) | ✅ | no |
| PSP, TIM, PVR, FLI/FLC, CEL, PIX (Dr. Halo), MacPaint, WMF, XMC, FAX G3, Seattle FilmWorks, MNG | ⬜ | no |
| Export-only: C source, HTML table, ASCII art | ⬜ | no |

## Next

The filter list is complete. Next: the testing in [parity-testing.md](parity-testing.md), and the
remaining file formats in the table above.

## Licence note

GIMP is GPL-3.0-or-later and GEGL is LGPL-3.0-or-later (many operations GPL-3.0-or-later); this repository is
`MIT OR Apache-2.0` and its rules are clean-room (`AGENTS.md`). Use GIMP as a *behaviour reference*; do not
copy its source or link GEGL/babl while the repository keeps its current licence. Relicensing the fork to
GPL-3.0-or-later would lift that restriction; decide before porting any operation.
