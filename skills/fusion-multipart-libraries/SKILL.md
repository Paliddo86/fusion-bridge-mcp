---
name: fusion-multipart-libraries
description: "How to build ONE Fusion 360 .lbr library holding MANY parts where EACH package has its real 3D model attached and RENDERING - the SOLVED, fully programmatic, zero-GUI-dialog recipe. The method: for each part, import its STEP into a Design, orient it flat, saveAs an .f3d, aps_upload the .f3d, construct an fs.file:vf wip_urn from the upload lineage, and hand-write the package3d binding into the combined .lbr. Covers the exact binding markup, the orient, merging per-part .lbr files, the connect/pad pre-merge validation, and the dead ends to avoid (the GUI Package3D generator's unbeatable CEF dialogs; a raw STEP upload renders nothing). Read this when attaching 3D to more than one part in a single library, or building a parts-library sampler. Trigger words - multi part library, multiple 3d packages, attach 3d to many parts, combined lbr, merge lbr, basic parts library, parts sampler, per package 3d, package3d, f3d upload, fs.file vf urn, aps_upload 3d, programmatic 3d binding, render 3d package, thumbnail download failed, Create3DPackage dialogs, attach all 3d."
---

# Building a many-part Fusion library with real, RENDERING 3D on every package

The multi-part extension of [fusion-libraries](../fusion-libraries/SKILL.md) (one part, end to end).
Getting a distinct, RENDERING 3D model onto each of many packages inside one `.lbr` is **solved** -
and the solution is fully programmatic, with **zero GUI dialogs**. Built on
[fusion-cloud-save](../fusion-cloud-save/SKILL.md) and [fusion-driving](../fusion-driving/SKILL.md).
Proven 2026-06-29: a 10-part Adom basic-parts library (R/C/L/LED/MCU), every package rendering its
real chip, flat on its footprint, built entirely this way.

## ⚡ FASTEST PATH - one call: `fusion_build_library_3d` (v1.6.0+)

> **From a Cloud Library bundle (zip of .lbr + STEPs):** `fusion_install_library_bundle
> {zipPath}` derives `parts[]` itself by pairing every deviceset with a STEP by name, then
> runs this exact chain. Unpaired devicesets are named in `unbound[]`; the rest still ship.
> It mints EACH part on its own temporary single-deviceset library (never the merged file:
> the generator loads the merged file's first package, so three chips landed on one footprint,
> John 2026-08-29), trusts the STEP's published origin and orientation (`orient:false`, no
> bounding-box guesses), binds every urn into the merged library, proves it, and then saves
> it into the user's cloud project under `Adom Libraries` so Fusion's Library Manager lists it,
> and leaves that library OPEN in front of the user (only the scratch is closed). The mint
> refuses with `wrong_footprint_loaded` if the generator loaded a different package than the
> one being minted. The full technical account, with the wrong-footprint and corrected frames,
> is `docs/MULTIPART_LIBRARY_IMPORT.md` on the fusion-bridge wiki page.

For a whole many-part library, you no longer loop `make_3d_package` + hand-write bindings. ONE verb
does the entire pipeline - per-part footprint+chip f3d packages, **all** bindings injected in one
pass, the finished library opened once, plus a BEFORE/AFTER screenshot per part:

```jsonc
fusion_build_library_3d {
  "lbrPath": "C:/Users/<user>/lib/Combined.lbr",          // the merged 2D library to bind + open
  "parts": [                                               // one entry per part
    {"package":"R_4k7","lbrPath":"C:/.../R_4k7.lbr","modelPath":"C:/.../R_4k7.step"},
    {"package":"ESP32-S3FN8","lbrPath":"C:/.../ESP32-S3FN8.lbr","modelPath":"C:/.../ESP32-S3FN8-named.step"}
    // ...
  ],
  "outLbrPath": "C:/Users/<user>/lib/Combined_bound.lbr",  // optional (default: overwrite lbrPath)
  "capture": true                                          // before/after PNGs per part (default true)
}
```

- Each part's `lbrPath` is its OWN per-part `.lbr` (whose FIRST package is that footprint - what
  Create3DPackage loads); `modelPath` is its STEP. `package` is the package name in BOTH the per-part
  and the combined `.lbr` (they match - that's how the binding lands).
- `projectId`/`folderId` are **optional** - they default to the MAIN-project upload folder, so you do
  not have to `fusion_aps_browse` for them.
- Returns `{partsBound, partsTotal, parts:[{package,success,wip_urn,dims_mm,error}], screenshots:
  [{package,stage,path}], boundLbr, opened}`. The before/after PNGs land on the box
  (`%TEMP%/adom-fusion-shots`); since 1.10.38 ab pulls them into your container automatically (`localFullPath` in each `screenshots[]` entry) - use those for a demo video.
- **Each part is independent.** If Fusion resets mid-run, the parts already done kept their f3d
  uploads; just re-run the whole `parts[]` (re-making a few f3ds is cheap) - the injection rewrites
  all bindings in one pass, so a single final call always yields a complete library.

### Authored placement beats every heuristic (1.10.8, issue #83)

When the manufacturer's STEP carries a deliberate transform (the TMF8829 ships with translation
[-0.00008128, 1.39191746, 0.18269966] mm and rotation [0, 0, 90] deg), pass it and nothing is guessed:

```jsonc
fusion_make_3d_package {"lbrPath": "...", "modelPath": "...",
  "placement": {"translation": [-0.00008128, 1.39191746, 0.18269966], "rotation": [0, 0, 90], "scale": 1}}
```

Rotation is applied about X, then Y, then Z (extrinsic, about the origin), then the translation, in
mm and degrees. With a placement the bounding-box orient, centering and pin-1 rotation heuristics
are all skipped; the reply carries `placement.appliedMatrix` (row-major 4x4 read back from the
occurrence, translation column in mm), `placement.verification` (verified | mismatch), `source`
{path, sha256, bytes, preserved} for the STEP Fusion read, and `warnings`. Non-finite values,
wrong shapes and any scale other than 1 are refused before Fusion is touched. `orient:false`
without a placement imports the model exactly as authored, no reorientation and no pin-1 rotation
(before 1.10.8 the pin-1 rotation survived orient:false). A default mint keeps both heuristics and
says so in `warnings`, so read `pin1.status` (`rotated` means the mark heuristic moved the chip).
Each `parts[]` entry of `fusion_build_library_3d` accepts the same `placement` and `orient`.

### Prove is for web control (1.10.17)

Called from an AI thread, `fusion_install_library_bundle` and `fusion_create_library` bind and place
only (`verdict: bound_not_proven`, `partsBound`, the library path); web control proves. `prove:true`
asks for the four frames per part; `fusion_capture_library_views {packages}` shows one part on demand.
A chain never restarts Fusion on its own: `allowRestart:true` with `restartReason`, recorded in
`restarts[]`.

### ⚠️ Relay timeout - a many-part run "times out" but KEEPS GOING (don't panic)

The `adom-bridge` relay times out the *request* at ~60s, but `fusion_build_library_3d` for a dozen
parts takes minutes. So you will often get **`status: timeout / Request timed out` while the build is
still running server-side and finishes fine.** Do NOT treat that as failure. Instead:

1. Wait, then **verify by the artifacts, not the response**: read the `boundLbr` and count
   `<package3d name=` (should equal your part count), and list the before/after PNGs in
   `%TEMP%/adom-fusion-shots` (pulled to you as `localFullPath`).
2. If a part is missing (the FIRST part of a run is the usual casualty, as Fusion settles from a prior
   view), **just re-run `fusion_build_library_3d` for the missing parts with `outLbrPath` pointing at
   the SAME file.** The binding injection is **merge-aware + idempotent** (v1.6.1+): it reads the
   bindings already in the file and accumulates, so re-running a subset NO LONGER wipes the parts that
   already succeeded. (Each part also self-retries once on a transient first-part failure.)

This is the fix for the "a re-run dropped my earlier bindings" trap - earlier the injection replaced
the whole `<packages3d>` block, so a subset re-run left you with only that subset.

### 🔍 Show the library views (what makes EEs trust it) - `fusion_capture_library_views`

The 3D before/after shots prove the chip renders, but they do NOT show the **symbol, footprint, or
device** - the views developers actually want. After building, open the library and call:

```jsonc
fusion_capture_library_views { "packages": ["ESP32-S3FN8", "ATSAMD51J20A-AUT", "R_4k7"] }
```

It runs `EDIT <deviceset>.sym / .pac / .dev` + `WINDOW FIT` + a background screenshot per view and
returns the PNG paths (on the box; `desktop_pull_file` them). You get, per part: the **SYMBOL** (full
pinout), the **FOOTPRINT** (pad array + the Copper/Silkscreen/Mask layer stack), and the
**COMPONENT** view (Content Manager: the symbol + the package table with the footprint<->package
**Mapped ✓** check + pin/pad counts). Put these in the demo video alongside the 3D - they are the warm
-and-fuzzy shots. (Manual equivalent: `fusion_open_lbr` then `fusion_electron_run "EDIT X.sym"` etc.)

### 🏭 Scaling to a WHOLE catalog - SHARE footprints (do NOT build 3D per value)

Building a big library (e.g. the entire **138-part Adom basic parts library**) does NOT mean 138 3D
builds. A real EAGLE/Fusion library SHARES footprints: ONE `R-0402` package is used by all 41 of the
0402 resistor *values*; ONE `C-0603` by all 14 of those caps. The 3D model is per *package*, not per
value. So:

1. **Collapse the catalog to its unique `(type, package)` combos.** The whole Adom basic parts catalog
   (138 values) is just **10 footprints**: R-0402, R-0603, C-0402, C-0603, C-0805, L, LED-0402,
   LED-0603, LED-2020, FB-0402.
2. **Build ONE base part per combo** (`adom-lbr generate` → a `.lbr` with that footprint + symbol),
   then `fusion_build_library_3d` over the ~10 bases → ~10 f3d `package3d` bindings. (10 builds, not 138.)
3. **Clone the base deviceset per value.** Take each bound base `<deviceset>` (it already carries the
   shared `<gate symbol=…>`, `<device package=…>`, and `<package3dinstances>` 3D binding) and emit one
   copy per catalog value, changing only the `<deviceset name=…>` (EAGLE-style, e.g. `R-0402-4K7`,
   `C-0603-100N`) and the `<technology>` attributes (`VALUE`/`MPN`/`MF`/`MOUSER`). All clones reference
   the SAME symbol + package + 3D urn.
4. Result: **N packages + N symbols + N package3d + M devicesets** (M ≫ N). The 138-part library =
   10 packages + 138 devicesets, every one Mapped ✓ and rendering, built from 10 uploads.

**Sourcing the ~10 footprints + 3D:** passive land patterns are standard per package (IPC-7351), so a
0603 cap shares the 0603 *resistor* land, an 0805 cap shares the 0805 land, etc. Pull real CAD from the
per-MPN wiki pages (`/api/v1/pages/<mpn-lowercased>/files/<MPN>.kicad_mod`/`.step` - chip-fetcher's
output), the wiki **static footprints library** (`static/footprints/<pkg>/`), or generic KiCad models
(`C_0603_1608Metric.step`, etc.). One representative MPN per combo is enough.

Everything below is the MANUAL equivalent (what the verb does internally) - read it to understand or
debug, but reach for `fusion_build_library_3d` first.

## ✅ THE RECIPE - per part, then merge (no Create3DPackage, no dialogs)

A proper component 3D package contains the **FOOTPRINT (pads + courtyard) AND the chip**, merged and
aligned, so the 3D viewer can verify the chip's pads land on the footprint pads. For each part:

1. **Make the component f3d (footprint + chip) - one verb does it:**
   `fusion_make_3d_package {lbrPath:"C:/.../sp_PART.lbr", modelPath:"C:/.../PART.step", projectId, folderId}`
   -> returns a `wip_urn`. It opens the lbr, runs `Electron.Create3DPackage` to load the package's
   FOOTPRINT into a generator doc, imports the chip onto it, orients it flat, `saveAs` an `.f3d`
   (footprint + chip), uploads it, and constructs the urn. (projectId/folderId from `fusion_aps_browse`.)
   Then **skip to step 4.**

   Manual equivalent (what the verb does - the key is Create3DPackage loads the footprint, and you
   `saveAs` instead of FINISH to skip the CEF dialogs):
   ```python
   # fusion_open_lbr sp_PART.lbr, then Electron.Create3DPackage <sp_PART.lbr>
   #   -> a generator doc with the package's FOOTPRINT (pads/courtyard) loaded. THEN, in that doc:
   import adsk.core, adsk.fusion, math
   app = adsk.core.Application.get(); doc = app.activeDocument        # the generator doc (HAS the footprint)
   d = adsk.fusion.Design.cast(app.activeProduct); root = d.rootComponent
   im = app.importManager
   im.importToTarget(im.createSTEPImportOptions("C:/.../PART.step"), root)   # chip ONTO the footprint
   oc = root.occurrences.item(root.occurrences.count-1); bb = oc.boundingBox
   dx,dy,dz = bb.maxPoint.x-bb.minPoint.x, bb.maxPoint.y-bb.minPoint.y, bb.maxPoint.z-bb.minPoint.z
   axis = (1,0,0) if (dy<=dx and dy<=dz) else ((0,1,0) if (dx<=dy and dx<=dz) else None)  # smallest -> flat in Z
   if axis:
       mat = oc.transform2.copy(); r = adsk.core.Matrix3D.create()
       r.setToRotation(math.pi/2, adsk.core.Vector3D.create(*axis), adsk.core.Point3D.create(0,0,0))
       mat.transformBy(r); oc.transform2 = mat
   doc.saveAs("PART_fp3d", app.data.activeProject.rootFolder, "fp+chip", "")  # saveAs, NOT Package3DStop/FINISH
   result = {"path": doc.dataFile.id}        # local .f3d cache path (footprint + chip)
   ```
   ⛔ Do NOT import into an EMPTY Design - that yields a chip with **NO footprint** (an incomplete
   package: it renders the chip but the 3D viewer can't show the pads aligning).
2. **Upload the f3d** (the verb does this): `fusion_aps_upload {projectId, folderId, localPath:<the .f3d>,
   fileName:"PART_3d.f3d"}` -> `{ "itemUrn": "urn:adsk.wipprod:dm.lineage:<id>", ... }`.
3. **Construct the binding urn:** `urn:adsk.wipprod:fs.file:vf.<id>?version=1` - the lineage `<id>`
   doubles as the version id for these single-version assets.
4. **Hand-write the binding** into the combined `.lbr` (markup below). Repeat per part, then open the
   merged library: it RESOLVES and **RENDERS** every component (footprint + chip), and saves to cloud.

### The binding markup (exactly what Fusion writes)

- Library-level, inserted between `</packages>` and `<symbols>` (one `<package3d>` per part):
  ```xml
  <packages3d>
  <package3d name="PKG" urn="" wip_urn="urn:adsk.wipprod:fs.file:vf.<id>?version=1" locally_modified="yes" type="model">
  <description>PKG</description>
  <packageinstances><packageinstance name="PKG"/></packageinstances>
  </package3d>
  ...one per part...
  </packages3d>
  ```
- Inside each device, right after its `</connects>`:
  ```xml
  <package3dinstances><package3dinstance package3d_urn="urn:adsk.wipprod:fs.file:vf.<id>?version=1"/></package3dinstances>
  ```

Verify success: open the merged `.lbr`, `fusion_check_dialogs` returns 0 (no broken-ref), and each
device's lower-right 3D preview shows the real chip (NOT "Thumbnail download failed").

## Speedups + what to expect (so you go fast)

- **Batch it.** Call `fusion_make_3d_package` once per part, collect the urns, hand-merge ALL bindings
  into the combined `.lbr` in ONE pass, then `fusion_open_lbr` ONCE. Do NOT open/save the library per
  part - it is slow and Fusion can reset the session between calls.
- **What success looks like:** `fusion_check_dialogs` == 0, every device's 3D preview shows footprint +
  chip, and opening an f3d shows the chip's pads landing on the footprint pads.
- **Fusion resets between calls** on long runs (and after a `bridge_install` respawn it can briefly
  report "not running" - `fusion_start` then retry). So: save the finished library to the cloud, and
  **reopen it from the cloud** (`fusion_aps_open`) for a clean Electronics Library view + screenshots -
  the in-session view often gets an empty "Untitled" shoved in front of it.
- **It is entirely dialog-free** - no Save dialog, no CEF modals; capture only hwnd-targeted background
  screenshots (never fullscreen - see fusion-driving).

## ⛔ Why an f3d and not a raw STEP (the key gotcha)

A raw `.step` uploaded via `fusion_aps_upload` gives a urn that **resolves** (no broken-ref) and saves
- BUT the library 3D preview shows **"Thumbnail download failed"** and does NOT render. A raw STEP is
not a renderable Fusion 3D asset. An **f3d** (a real Fusion Design with the STEP imported) IS, and
binds + renders identically to a GUI-minted package. So always: STEP -> import into a Design ->
`saveAs` f3d -> upload the **f3d**. (Proven contrast: a STEP-bound nRF showed "Thumbnail download
failed"; the f3d-bound nRF renders a clean black QFN.)

## ⛔ Do NOT use the GUI Package3D generator for this (it is dialog-trapped)

`Electron.Create3DPackage` + import + `Package3DStop` (FINISH) is the manual/GUI way and it is a trap:
- FINISH pops a native Save dialog AND **two opaque "Fusion360" CEF modals** that are unreadable by
  Win32, by UIA (`desktop_find_control` finds nothing), and by PrintWindow (captures solid black), and
  they ignore programmatic escape/WM_CLOSE.
- `Create3DPackage` also binds to the **first package of the OPEN library**, not the file you pass it,
  so looping reordered files against one open library binds only 1/N.
The f3d recipe above sidesteps ALL of this. Use it; do not fight the generator.

## Merging per-part `.lbr` files into one combined library

An EAGLE `.lbr` is a library of many devicesets, so combine the per-part files: keep ONE file's
`<layers>` skeleton, then concatenate every part's `<package>`, `<symbol>`, `<deviceset>` (MPN-unique
names, no collisions), then add the `<packages3d>` + per-device `<package3dinstances>` from the recipe.
`adom-lbr validate combined.lbr`.

## ⚠️ VALIDATE connect/pad consistency BEFORE merging (one bad part poisons everything)

Fusion rejects the ENTIRE merged `.lbr` ("has errors and cannot be opened") if ANY device has a
`<connect pad="N">` referencing a pad its package does not define. A single-device `.lbr` may still
open, masking it until merged. Before merging, for every device confirm every `<connect pad="N">`
references an existing `<smd name="N">`/`<pad name="N">`. Passives are the usual offenders (symbol pin
count vs footprint pad count). `adom-lbr` lint did NOT catch this - validate it yourself.

## Capture the money shots (for a demo video)

Per part, hwnd-targeted (never fullscreen - see fusion-driving): the footprint alone, then the device
view with the real rendered chip on its footprint (the money shot), reopening the cloud library so the
3D preview refreshes. See [CREATING_BASIC_PARTS_LIBRARIES.md](../../CREATING_BASIC_PARTS_LIBRARIES.md)
for the full tool-chain tutorial + montage recipe.

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

## The one-call path, and the state that stops mid-build evidence

Everything here is the manual recipe. For a normal import prefer
`fusion_create_library {parts:[...]}` — it chains bind and prove behind one `runId`, streams
each artifact as it lands, and treats one part as N=1 rather than a special case.

Two states worth knowing while a build runs:
- `build_in_progress` — a capture during a 3D build photographs the build editor, not the
  view it claims. Wait; the marker survives a bridge respawn so the refusal is not silently lost.
- `addin_restart_required` — the add-in was installed while Fusion was open. Ask, then
  `fusion_restart_for_addin {save:true}`.

Before calling a multi-part library done, run `fusion_library_parts {libraryPath}`: with many
parts it is easy to ship one whose part numbers never made it, and `stage: done` does not
check that.
