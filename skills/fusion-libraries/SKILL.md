---
name: fusion-libraries
description: How an Adom component library (symbol + footprint + 3D chip) is built and gets into Fusion 360 Electronics as a placeable .lbr. Covers the full pipeline (chip-fetcher -> adom-symbol / adom-footprint -> adom-lbr -> fusion_open_lbr), the Windows file-staging gotcha (there is NO container-to-Windows push verb), and attaching the real 3D chip with fusion_attach_3d_package (the Package3D-generator FINISH that binds a package3d wip_urn onto the deviceset). Read before building or opening any library, or when a part shows only a placeholder 3D package. Trigger words - lbr, eagle library, fusion library, open lbr, chip-fetcher, adom-lbr, adom-symbol, adom-footprint, kicad_sym, kicad_mod, deviceset, package3d, 3d package, 3d chip, fusion_attach_3d_package, wip_urn, attach 3d model, placeholder package, content manager, library into fusion, get library into fusion.
---

# Fusion libraries - symbol + footprint + 3D into a placeable .lbr

> This skill is the concise entry point for **ONE part, end to end**. The **exhaustive guides are
> bundled right here in this skill folder** - read them directly: **[MAKING_LIBRARIES.md](MAKING_LIBRARIES.md)**
> (the full step-by-step guide + screenshots) and **[LIBRARY_FINDINGS.md](LIBRARY_FINDINGS.md)** (the
> findings log: the exact `package3d` markup, the owned-popup error-capture technique, and every
> pitfall). (Their screenshots load from the wiki.)

> **Companion skills (read these too):**
> - For a library with **MANY parts**, each needing its own 3D, use
>   **[fusion-multipart-libraries](../fusion-multipart-libraries/SKILL.md)** (the reorder-file trick,
>   the per-part attach loop, merging + connect/pad validation). This single-part skill is its
>   building block.
> - The operating discipline this all rides on:
>   **[fusion-driving](../fusion-driving/SKILL.md)** (read the dialog array after every step, never
>   blind-dismiss, never fullscreen-capture) and **[fusion-cloud-save](../fusion-cloud-save/SKILL.md)**
>   (saving is async/networked: give the library a cloud home early, let Hub uploads drain before you
>   close/save). Skipping these is how a finished 10-part library got lost.

## 1. The pipeline (where each tool fits)

```
chip-fetcher  ->  adom-symbol   -> .kicad_sym  ┐
(vendor bundle)   adom-footprint -> .kicad_mod ┼-> adom-lbr generate -> NAME.lbr  -> fusion_open_lbr
                  (3D STEP/glb provided)       ┘   (2D: symbol+package+connects)     (Content Manager)
```

> ⛔ **USE THE CURATED `.fusion` VARIANT, NOT A RAW VENDOR DOWNLOAD.** chip-fetcher's library
> (`chip-fetcher/library/<MPN>/`) carries per-EDA variants. For Fusion, feed adom-lbr the
> **`<MPN>.fusion.kicad_sym` + `<MPN>.fusion.kicad_mod`** (Adom's `adom-sfconvert`-tuned files) and
> pair them with **`<MPN>-named.step`** (the curated 3D). Building from a raw `/tmp` vendor
> `EAGLE/*.lbr` or the thin `manufacturer.step` gives a footprint/3D mismatch AND the vendor `.lbr`
> fails `Create3DPackage`'s XML parser. (Convention confirmed in `chip-fetcher/src/thumbnails.rs`.)

- **chip-fetcher** pulls a multi-CAD vendor bundle for an MPN. Its curated library is at
  `chip-fetcher/library/<MPN>/` with variant files: `.fusion.*` (use these for Fusion), `.manufacturer.*`,
  `.altium.*`, `.ds2sf.*`, plus `<MPN>-named.step` (3D with the part number embossed) and render SVGs.
- **adom-symbol** / **adom-footprint** make the `.kicad_sym` / `.kicad_mod` (symbol / footprint, 2D only).
- **adom-lbr generate** `--sym <MPN>.fusion.kicad_sym --fp <MPN>.fusion.kicad_mod --name <MPN>` assembles
  the EAGLE `.lbr` (symbol + footprint + deviceset). **2D-only**, no 3D/URN; the 3D is attached in Fusion
  (sections 4-6). Validate with `adom-lbr validate`. **Inspect it visually**: `adom-lbr serve --port 8785
  --file <MPN>.lbr` run FROM the library dir (so it finds the render SVGs) gives a symbol | footprint | 3D
  + pin-map viewer - open it in pup to confirm the right footprint before touching Fusion.
- The **`-named.step` part-number emboss** is made by **`service-step2glb/api/occt/occt_name_on_chip.py`**
  (OpenCASCADE relief on the top face), driven by `chip-fetcher/scripts/occt-name-on-chip.py`.
- **fusion_open_lbr** imports the `.lbr` into Fusion's Electronics Library editor.

See **[LIBRARY_FINDINGS.md](LIBRARY_FINDINGS.md)** for the full live findings log.

## 2. Opening a .lbr in Fusion (verified working)

`fusion_open_lbr {"filePath": "C:/Users/<user>/.../NAME.lbr", "verify": true}`

- Under the hood it runs `Document.newDesignFromLocal <path>`, which opens the library and switches to the **Electronics Library** workspace.
- `verify:true` re-exports the loaded library to an EAGLE script and returns a preview - proof Fusion actually parsed the deviceset (layers/symbol/package), not just that a window opened.
- After it returns, confirm with `fusion_get_app_state`: expect `activeDocument: NAME`, `activeWorkspace: "Electronics Library"`, `isElectronics: true`. Then screenshot the Fusion window (`desktop_find_window {titleContains:"Fusion"}` -> hwnd -> `desktop_screenshot_window {hwnd}`) to show it.

## 3. THE WINDOWS-STAGING GOTCHA (read this first)

`fusion_open_lbr` (and `fusion_import_step`) take a **filePath on the WINDOWS machine** where Fusion runs (`C:/...`). The whole chip-fetcher/adom-lbr pipeline runs in the **cloud container**, so the `.lbr` / `.step` is on the Linux box, which Fusion cannot read. **There is NO container-to-Windows push verb** (`desktop_pull_*` only goes Windows -> container).

Stage the file onto Windows first. Proven method (same proxy that serves shotlog):

1. Serve it from the container: `cd <dir> && python3 -m http.server 8931` (any proxied port).
2. Download on Windows via `shell_execute`:
   `powershell -NoProfile -Command "New-Item -ItemType Directory -Force -Path C:\Users\<user>\adom-lib | Out-Null; Invoke-WebRequest -Uri 'https://<workspace>.adom.cloud/proxy/8931/NAME.lbr' -OutFile 'C:\Users\<user>\adom-lib\NAME.lbr'"`
3. Open the Windows path with `fusion_open_lbr`.

Confirm the byte count matches after download. Build the `shell_execute` JSON in Python (`json.dumps`), not by hand - nested cmd/PowerShell quoting mangles easily, and Windows paths need `\\` in JSON.

## 4. Attaching the real 3D chip (SOLVED via fusion_attach_3d_package)

A fresh `.lbr` shows the package as a **placeholder** 3D (the Content Manager 3D tab is empty / a generated box). The real chip STEP exists (`chip-fetcher/library/<MPN>/<MPN>.step`) but is **not linked**. This is the part that "has always been complex."

**Why it is hard:** Fusion does NOT embed the STEP in the `.lbr`. A library package references its 3D model by a **Fusion cloud URN**, not a local file. The model must first exist in the Fusion cloud as a URN-addressed asset, and only then can a `package3d` reference point at it. Format of the URN (from the bridge's cloud code):

```
urn:adsk.wipprod:fs.file:vf.<id>?version=<N>
```

**The verified flow (proven on nRF54L15; full walkthrough + screenshots in [MAKING_LIBRARIES.md](MAKING_LIBRARIES.md)):**
1. **Model -> cloud -> URN.** Stage the STEP to Windows, then `fusion_aps_upload {projectId, folderId, localPath:"C:/.../<MPN>.step"}` returns a real cloud URN (`urn:adsk.wipprod:dm.lineage:...`). `Electron.getCloudPathFromUrn` confirms the 3D package references a Fusion **file** URN, which this produces. (`fusion_import_file` is NOT a routed verb; do not rely on it.)
2. **Open the generator.** `fusion_execute_text_command {"command":"Electron.Create3DPackage C:/.../<MPN>.lbr"}` opens the **3D PACKAGE Generator**: a Design-workspace doc with the footprint loaded and a PACKAGE / SELECT / FINISH ribbon.
3. **Insert model + FINISH** -> Fusion writes the `package3d` URN back to the library.

**SOLVED - use `fusion_attach_3d_package` (shipped in v1.5.7):**
One verb does steps 2-3 end to end: open the `.lbr` (library active) -> `Create3DPackage` -> import the
STEP + auto-orient -> `Package3DStop` (FINISH). It returns a `_hint` with the one desktop step left: click
the Fusion **Save** dialog via `desktop_ui_click {automationId:'QTApplication.QTFrameWindow.standardActions.SaveButton'}`.
The 3D then binds onto the deviceset (Content Manager tree shows it; the `Package` column flips
Placeholder -> part name; the preview LAGS a beat). After FINISH+Save, the package **uploads to the
Hub asynchronously** - do NOT close or saveAs until it drains, or you lose it
([fusion-cloud-save](../fusion-cloud-save/SKILL.md)); and READ that Save dialog before clicking, never
blind-dismiss ([fusion-driving](../fusion-driving/SKILL.md)). Fusion writes this exact markup (for a self-uploaded
model): `<package3d urn="" wip_urn="urn:adsk.wipprod:fs.file:vf.<id>?version=1" locally_modified="yes" type="model">`
+ a device `<package3dinstance package3d_urn="...fs.file:vf...?version=1"/>`. See [LIBRARY_FINDINGS.md](LIBRARY_FINDINGS.md) §11-12.

Uploading models into the user's Fusion cloud is an outward, persistent action - confirm first, and put uploads in a contained folder (we used `Standard Components / Electronics 3D`).

## 5. Pitfalls (each one bit us)

- **Container path passed to `fusion_open_lbr`** -> "file not found" / silent fail. The path must be Windows-local; stage it first (section 3).
- **Opening a child instead of a library** - a `.lbr` is a library, not an electronics design; it opens into the **Electronics Library** workspace, not the PCB editor. (Different from the project/schematic/board hierarchy - see the `fusion-electronics` skill.)
- **Assuming the 3D is there** - adom-lbr output is 2D; the package is a placeholder. A part "looking amazing" in symbol+footprint still has NO 3D chip until the URN is linked (section 4).
- **Guessing the `package3d` XML** - no confirmed example exists; reverse-engineer it from a real Fusion-made attachment before writing any.
- **Hand-built `shell_execute` quoting** - use `json.dumps`; escape Windows `\` as `\\` in the JSON.
- **`desktop_screenshot_window` needs `hwnd`** (not `titleContains`); resolve it via `desktop_find_window` first. It returns the PNG as inline base64 - decode to a file.
- **`adom-lbr validate` takes a positional `<PATH>`**, not `--lbr`.

## Importing a library end to end (web control, and the same verbs for you)

The full lane - pre-flight with measured ETAs, the four-artifact evidence bundle, per-part
3D binding, progress polling, and every failure contract that refuses rather than reporting
a false pass - is in **`fusion-web-control`**. Read it before importing a library or
rendering a progress bar for one. Note that the wiki's web control and you share ONE verb
surface: there is no verb only one side can call.

## Part numbers and variants (an EE cannot order without these)

Geometry is not the part. MPN, MANUFACTURER, MOUSER_PN, DIGIKEY_PN and LCSC_PN, and the
variant model that lets an automotive-grade part and a baseline one share one symbol, one
footprint and one 3D body, are covered in **`fusion-part-metadata`**. Check
`fusion_library_parts {libraryPath}` before calling any library complete - `ok: true` from
preflight only means the file OPENS.

## Before you import: pre-flight, and the add-in restart trap

**Pre-flight first.** `fusion_plan_library_import {parts}` does no work and changes nothing.
It tells you whether Fusion is installed, running, or needs its add-in loaded, and returns a
costed plan whose bind estimate scales with the part count. Call it before committing a user
to a long import, so they see what is about to happen instead of a silent spinner.

**The trap that catches new users:** if ab installed or updated the add-in while Fusion was
already open, Fusion cannot see it — add-ins load only at launch. Verbs then return
`addin_restart_required`, carrying `userMessage`, `openDocuments` and `unsavedDocuments`.
**Ask the user**, then `fusion_restart_for_addin {save:true}`, which saves first and refuses
with `unsaved_named_document` rather than discarding a never-saved document. Do not
silently restart a Fusion someone is working in.
