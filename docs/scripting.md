# Scripting: Script-Fu and Python-Fu

Plugins › Script-Fu and Plugins › Python-Fu run scripts written for GIMP against PhotoCraft's engine.
Each has a **Console…** (a window with an input line and a log) and **Run Script…** (a `.scm` / `.py` file).

| | Script-Fu | Python-Fu |
|---|---|---|
| Crate | `photocraft-scheme` (interpreter, no workspace deps) + `photocraft-script` (procedures) | `photocraft-pyfu` (RustPython 0.4, no standard library) |
| Language | Scheme: R5RS core plus the TinyScheme extras Script-Fu uses | Python 3 subset RustPython supports |
| Runaway scripts | stop at a step budget (50 M) and a recursion limit | **not interrupted**: a `while True` hangs the window until the app is closed |
| Host access | none of its own | `open`, `exec`, `eval`, `input` removed and every import except GIMP's own modules refused. This keeps accidents out; it is **not** a security boundary, so run only scripts you trust |

Both call the same procedure database (`crates/script/src/pdb.rs`): GIMP's names (`gimp-image-new`,
`pdb.gimp_image_new`) so existing scripts run, each one a thin mapping onto a PhotoCraft command, so
scripts get the same selections, locks, colour models and history as the menus. Procedures return a list
of results as in Script-Fu; Python-Fu unwraps a one-element list.

## Covered today

Images (`gimp-image-new/list/width/height/scale/resize/flatten/delete/get-active-layer`), layers
(`gimp-layer-new`, `-set-opacity`, `gimp-item-set-name`), fills (`gimp-drawable-fill`, `gimp-edit-fill`),
selections (`gimp-selection-all/none`, `gimp-image-select-rectangle/ellipse`), context colours,
`gimp-message`, and the filters `plug-in-gauss*`, `plug-in-edge`, `plug-in-pixelize`, `plug-in-vinvert`.
`script-fu-register` / `register()` record a script's name, menu path and parameters
(`ScriptFu::registered`, `PythonFu::registered`); a Run dialog for them is not built yet.

## Differences from GIMP

- `gimp-image-new` makes an image with one transparent layer (GIMP's has none).
- `gimp-layer-new` adds the layer immediately; `gimp-image-insert-layer` is accepted and does nothing.
- Procedures outside the list above report `unbound variable` / `no procedure named …`.

Scripts live outside the engine's command layer on purpose: the UI shell receives the runners through
`Services::script_run` / `Services::python_run` (installed by `apps/photocraft`), which keeps `ui-egui`
free of the interpreters.
