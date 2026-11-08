---
name: fusion-cloud-library-import
description: Take a wiki Cloud Library (a cart of component pages) all the way into Fusion as a real EAGLE library with a rendering 3D chip on every package, and PROVE it with screenshots. Covers adom-wiki library create/add/availability/export, staging the bundle onto the Windows box, fusion_install_library_bundle, polling fusion_prove_result to a verdict, what the four evidence frames mean (symbol, footprint, package3d, ganged), and the traps that make a healthy run look broken. Use when a user says import my cloud library into Fusion, add these parts to Fusion, build a Fusion library from the wiki, prove the 3D came across, or asks why an import is stuck. Trigger words - cloud library, fusion import, install library bundle, library export fusion, prove library, ganged view, package3d, prove_result, partsBound, verdict proven.
---

# Wiki Cloud Library to Fusion, end to end

The lane a normal user actually walks: they have a Cloud Library on the wiki (component pages in
a cart) and they want it in Fusion with real 3D. Measured over three full runs 2026-09-01, 15
parts total.

## The whole flow

```bash
# 1. Build the cart (or the user already has one)
adom-wiki library create "My Parts" --description "..."      # -> lib_xxxxxxxx
adom-wiki library add lib_xxxxxxxx <owner-slug>              # repeat per part
adom-wiki library show lib_xxxxxxxx                          # confirm items resolved

# 2. CHECK BEFORE YOU BUILD - which parts can even produce a Fusion library
adom-wiki library availability lib_xxxxxxxx                  # summary {kicad, fusion, altium}

# 3. Export the Fusion bundle (zip: merged .lbr + parts/*.lbr + 3dmodels/*.step)
adom-wiki library export lib_xxxxxxxx --format fusion -o parts.zip
```

```jsonc
// 4. Stage onto the Windows box (Docker -> desktop). NOTE the arg names.
send_files {"filePaths":["/abs/container/parts.zip"], "reason":"..."}   // lands in Downloads
// 5. One verb does pairing + bind + prove
fusion_install_library_bundle {"zipPath":"C:/Users/<user>/Downloads/parts.zip"}
// -> {runId, stage:"bind", parts:[{deviceset, package, modelPath}], unbound:[]}
// 6. Poll to a verdict (minutes, not seconds)
fusion_prove_result {"runId":"<runId>"}
```

**Read `unbound[]` on the install response.** It names any deviceset that got no STEP. Those
parts ship 2D-only; everything else still binds.

## Timing, so you do not think it hung

Measured on a 4K desktop: **~7 minutes to bind 5 packages**, then **~10 minutes** to capture 20
evidence frames. `partsBound` stays `None` for the whole bind phase and then jumps to N at once;
`evidenceCount` climbs one frame at a time afterwards. Poll every ~30s. A 20-minute total is
normal, not a hang.

## The four evidence frames, per part

| step | view | what it proves |
|---|---|---|
| `symbol` | symbol-editor | the pinout drew |
| `footprint` | footprint-editor | the land pattern drew (one frame PER package for a multi-variant deviceset) |
| `package3d` | 3d-editor | a REAL chip body is bound, not Fusion's Placeholder |
| `ganged` | library-manager | Content Manager: symbol + package table + **Mapped ✓** + the 3D preview together |

**`ganged` here means the Content Manager frame, NOT a multi-chip scene.** If someone asks for
"the ganged-up view" meaning all the chips side by side in one 3D window, no verb produces that -
build it yourself (recipe below).

`verdict: "proven"` plus `evidence` with zero `ok:false` entries is the pass. Anything else,
read the failing frames; they carry the reason.

## partsBound can be LESS than the part count, and that is correct

A library SHARES footprints. Two parts on the same package produce one `package3d`, and the
second part's prove step reports `sharedPackageProof: "<the other part>"`. Measured: 5 parts,
`partsBound: 4`, because UCC28C44DR and AL3353S-13 are both SOIC-8. Judge the run by
`provedParts` (one entry per part, each `proven: true`), not by `partsBound`.

## Pin 1 is checked for you

Every bound part reports `pin1: {status, rotatedDeg, mark_mm, pad1_mm}`. `aligned` means the
chip's pin-1 mark and pad 1 agree. `rotatedDeg` non-zero means the binder rotated the model to
make them agree. `no_mark_on_chip` is honest, not a failure: that STEP has no pin-1 feature to
align against. This is the number one thing EEs check, so quote it in your summary.

## Traps

- **`fusion_create_library` shapes.** `libraryPath` is the **INPUT** `.lbr` and must already
  exist. Naming an output file there fails preflight with `lbr_preflight_failed`
  ("file-readable: cannot find the file"). `parts:[...]` is a supplement that binds STEPs to
  packages inside that library, never an input by itself - passing only `parts` returns
  `no_input`. For a whole cloud library, do not hand-assemble `parts[]` at all; use
  `fusion_install_library_bundle`.
- **`send_files` takes `filePaths` (array) and lands in the user's Downloads** unless you pass
  `dest`. `pull_file` also takes `filePaths`, not `path`.
- **A stuck `bind` with no dialog and no alert is usually a broken document, not a broken
  chain.** Screenshot Fusion and check `fusion_alerts` - Fusion's real complaint is often a CEF
  toast invisible to both Win32 and UIA.
- **`fusion_readiness` can report `ready:false` on a healthy Fusion.** The install does not gate
  on it, so try the verb before believing the flag; screenshot to confirm.
- **The export refuses broken parts rather than shipping them.** `LIB_EXPORT_PARTIAL` names each
  skipped part and why. A real one seen live: `pin "VCC" -> pad "8" (no such pad in package
  'SOT-23-6')`. That refusal is doing you a favour: ONE bad connect/pad pair makes Fusion reject
  the ENTIRE merged library. Swap the part out and re-export.

## Building a real ganged 3D view (all chips in one window)

No verb does this. Import the staged STEPs into ONE fresh design and lay them out:

```python
# fusion_run_modeling_script - MUST create its own document: a Part Design holds ONE component,
# and importing a second body into one fails "InternalValidationError : results".
doc = app.documents.add(adsk.core.DocumentTypes.FusionDesignDocumentType)
d = adsk.fusion.Design.cast(app.activeProduct); root = d.rootComponent; im = app.importManager
x = 0.0
for fn in files:
    im.importToTarget(im.createSTEPImportOptions(base + fn), root); adsk.doEvents()
    oc = root.occurrences.item(root.occurrences.count - 1); bb = oc.boundingBox
    # rotate the THINNEST axis into Z so the chip lies flat, then butt it up against the last one
    # (full recipe: fusion-multipart-libraries)
    x += width + 0.4
vp = app.activeViewport; cam = vp.camera
cam.viewOrientation = adsk.core.ViewOrientations.IsoTopRightViewOrientation
cam.isSmoothTransition = False; vp.camera = cam; vp.fit(); vp.refresh()
```

Then `desktop_screenshot_window {hwnd}` on Fusion's main window. Chips land in a row, flat,
proportional, pin-1 dimples visible. That is the frame people actually want in a demo.

## Do not publish a bridge release while a run is in flight

ab auto-updates the fusion bridge whenever a new version is published, and the respawn KILLS an
in-flight run's thread. The run is marked honestly - `fusion_prove_result` returns
`errorCode: bridge_restarted_mid_run` with the stage it died at - but the work after that stage
simply never happened. Seen live: a publisher's own release orphaned the prove phase of the run
they were watching. Finish or re-prove first, publish after.

**Recovering an orphaned run:** the BIND survives (urns are in the bound .lbr), so do not
re-install. Re-prove part by part with `fusion_prove_library {libraryPath, name}` - one call per
deviceset, ~2 minutes each, and read the result from `fusion_prove_result {}` after each (the
relay times out at 60s while the prove keeps running; never fire the next part until the
previous one's 4 frames landed, or the runs race each other on one Fusion). The bound library
for a bundle install lives at `C:/Users/<user>/Documents/AdomLibraries/<bundle>/<Library>.lbr`.

## Clean up after yourself

The chain leaves scratch `Untitled` documents. `fusion_close_document {name:"Untitled",
discard:true}` in a small loop; the API reports every scratch doc as plain `Untitled` even when
the tab strip shows `Untitled(2)`, so loop rather than matching the visible name.
