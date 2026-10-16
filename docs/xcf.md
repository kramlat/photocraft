# GIMP documents

PhotoCraft opens and saves GIMP's native format, `.xcf`: File › Open, recent files, drops, Save As,
the CLI (`convert`, `info`, `batch`) and automation all use the reader and writer in
`photocraft-io` (`crates/io/src/xcf.rs`), written from the public XCF description. The
format is recognised by its first bytes, so an XCF with another extension still opens.

## What is kept

| | Open | Save |
|---|---|---|
| Versions | 0–27 (GIMP 1.x to 3.2), 4- and 8-byte file offsets | version 11, readable by GIMP 2.10 and 3.x |
| Layers: pixels, position, name, visibility, opacity | yes | yes (cropped to their pixels) |
| Groups, nested | yes | yes |
| Layer masks | yes | yes (feathering is not saved) |
| Blend modes | legacy and current numbers, mapped to the closest Photoshop mode (approximations are reported) | the modes GIMP has; others are saved as Normal and reported |
| Locks (alpha, pixels, position) | yes | yes |
| Colour models | RGB, grayscale, indexed (opens as RGB) | RGB and grayscale |
| Depth | 8 and 16-bit integer, half and 32-bit float; linear-light files are converted to encoded values | 8-bit (deeper documents are reduced and reported) |
| Guides, resolution, colour profile | yes | yes |
| Text, vector and link layers | open as pixels | saved as pixels |
| Channels, paths, selection, non-destructive filters | skipped, reported | not saved, reported |

Tiles are read uncompressed, run-length coded or zlib-compressed; saved run-length coded.

## Verified against GIMP

Six real files from GIMP 2.x and 3.x (a version 3 group file, version 11 single-layer files) open
to the same pixels as GIMP 3.2.6's own flattened rendering, and GIMP 3.2.6 renders PhotoCraft's
saved files to the same pixels as the originals. Saved files mark every layer as blending on the
encoded values (GIMP's "non-linear" composite and blend space): without that GIMP mixes new-style
modes in linear light and edge pixels come out brighter. A group's marker is written before its
other properties because GIMP replaces the layer when it reads it.

## Known differences

- **Linear-light blending.** GIMP 2.10+ composites new-style layer modes in linear light unless a
  layer says otherwise, and many files do. PhotoCraft blends on encoded values (as Photoshop
  does), so such a file shows slightly different colours where layers overlap; each affected
  layer is reported. Layers carrying the explicit non-linear setting, and legacy files, match exactly.
- **Clip to backdrop.** In GIMP most non-Normal modes only show where the layers beneath have
  content; PhotoCraft's modes also show over empty areas.
- Layer modes GIMP has and Photoshop lacks (Grain Extract/Merge, Behind, Erase, …) open as Normal.
