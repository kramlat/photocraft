# Testing the GIMP-parity work

A checklist for anyone who knows GIMP well and wants to find where PhotoCraft behaves differently.
For each item: do it in GIMP, do the same in PhotoCraft, and note anything that looks or acts
different. **Unexpected is the point:** a different result, a missing option, a confusing name, a
crash, or a slow filter are all worth reporting.

## What to report

For each problem, send:

1. What you did (menu path, the values you typed, the image if you can share it).
2. What GIMP does.
3. What PhotoCraft does instead (a screenshot helps).
4. How bad it is: crash or lost work / wrong result / looks different / just odd.

Names are Photoshop's, not GIMP's, on purpose; the table at the end maps GIMP's names to where the
filters live. A filter that GIMP has with options PhotoCraft's dialog doesn't show is a finding too.

## File formats

| Open | Save | Notes |
|---|---|---|
| `.xcf` (your own files; GIMP 2.x and 3.x) | File › Save As… GIMP | Compare with GIMP: layers, groups, masks, blend modes, opacity, hidden layers, guides. GIMP 2.10+ blends in linear light by default, so overlapping layers can differ slightly; PhotoCraft says so on open. |
| `.pat`, `.gbr`, `.gih` | | Drop on the window or use the Patterns / Brushes import button. Check names, sizes, spacing, and that image-hose files give one preset per cell. |

## Per-filter checklist

Run each on a photo and on a flat test image; try small and large values; try on a selection and on
a layer with transparency; undo and redo.

- Does the preview match what OK applies?
- Are the defaults sensible, and the slider ranges wide enough?
- Do 16-bit and 32-bit documents give the same look as 8-bit?
- Does it stay responsive on a large image (12 MP+)?

## Where GIMP's filters went

| GIMP | PhotoCraft |
|---|---|
| Colors › Value Invert / Linear Invert | Filter › Other › Invert Brightness / Invert (Linear Light) |
| Colors › Color Exchange | Filter › Other › Replace Color Range… |
| Colors › Alien Map | Filter › Other › Channel Waves… |
| Colors › Components › Extract Component | Filter › Other › Extract Channel… |
| Colors › Auto › Stretch Contrast (and HSV) | Filter › Other › Stretch Levels… |
| Colors › Auto › Color Enhance | Filter › Other › Enhance Colors |
| Colors › Dither / Bayer Matrix | Filter › Other › Dither… |
| Colors › Value Propagate | Filter › Other › Propagate Values… |
| Enhance › Stress | Filter › Other › Local Contrast Stretch… |
| Enhance › Wavelet Decompose | Filter › Other › Wavelet Scale… (one scale at a time) |
| Enhance › Antialias | Filter › Other › Smooth Edges… |
| Blur › Selective Gaussian / SNN Mean | Filter › Blur › Surface Blur… / Edge-Preserving Mean… |
| Blur › Variable Blur | Filter › Blur › Lens Blur… with a depth map, or Field Blur |
| Edge-Detect › Sobel / Laplace / Edge | Filter › Stylize › Edge Detection… |
| Edge-Detect › Difference of Gaussians | Filter › Other › Gaussian Difference… |
| Edge-Detect › Neon | Filter Gallery › Glowing Edges |
| Distorts › Mirrors | Filter › Distort › Kaleidoscope… |
| Distorts › Shift | Filter › Distort › Shift Rows… |
| Distorts › Curve Bend | Filter › Distort › Bend… |
| Distorts › Tile Glass / Tile Paper | Filter › Distort › Glass Tiles… / Paper Tiles… |
| Distorts › Illusion | Filter › Distort › Illusion… |
| Distorts › Apply Lens | Filter › Distort › Lens Refraction… |
| Distorts › Little Planet / Panorama Projection | Filter › Distort › Little Planet… / Panorama View… |
| Distorts › Fractal Trace | Filter › Distort › Fractal Trace… |
| Distorts › Recursive Transform | Filter › Distort › Droste… |
| Distorts › Engrave | Filter › Stylize › Engrave… |
| Distorts › Video | Filter › Video › Pixel Display… |
| Distorts › Bump Map / Normal Map | Filter › Stylize › Bump Map… / Normal Map… |
| Distorts › Tile Seamless | Filter › Other › Make Seamless… |
| Artistic › Cubism | Filter › Pixelate › Cubism… |
| Artistic › Cartoon / Photocopy | Filter Gallery › Poster Edges / Photocopy |
| Artistic › Softglow / Bloom | Filter Gallery › Diffuse Glow |
| Light and Shadow › Supernova / Sparkle / Long Shadow / Vignette | Filter › Render › Supernova… / Sparkle…, Filter › Stylize › Long Shadow…, Filter › Render › Vignette… |
| Noise › Hurl / Pick / Slur / Spread | Filter › Noise › Scatter Pixels… |
| Render › Noise (Perlin, Simplex, Cell) | Filter › Render › Noise Pattern… |
| Render › Checkerboard / Grid / Maze / Spiral | Filter › Render › Checkerboard… / Grid… / Maze… / Spiral… |
| Render › Plasma / Sinus / Linear Sinusoid | Filter › Render › Plasma… / Sine Waves… |
| Render › Diffraction Patterns / Fractal Explorer | Filter › Render › Diffraction Pattern… / Fractal Explorer… |
| Colors › Distance Transform | Filter › Other › Distance Map… |

Not done yet: the HDR tone mappers (Fattal, Mantiuk, Reinhard), Mean Curvature Blur, and the
segmentation filters (Waterpixels, SLIC).

## Scripts

Plugins › Script-Fu › Console… accepts GIMP's Scheme (`(gimp-image-new 64 64 RGB)` and friends);
Plugins › Python-Fu works the same with `pdb.gimp_image_new(...)`. Only the procedures listed in
[scripting.md](scripting.md) exist so far: try a script you use and report which procedure is missing.
