# Multi-part library import: how a Fusion library gets real, rendering 3D on every part

[![A real 3D model on every part of a library, watch on YouTube](docs/img/forum/demo-3d-library.jpg)](https://youtu.be/poAy__9Iggw)

*Demo: one prompt in Hydrogen builds a six-part library from a wiki Cloud Library with every 3D model bound, explains the three traps, and proves every part.*

This is the technical account of what `fusion_install_library_bundle` does between "a zip of
`.lbr` + `.step` files" and "a library in your Fusion Library Manager where every package
renders its chip on its own footprint". It exists because the middle of that sentence is
where Fusion makes things hard, and because two of the ways to get it subtly wrong were
found on real runs and are pictured below.

If you only want to use it: press **Create Fusion library** on a wiki part page (or several
parts on the Quick Actions page), and the bridge does everything here. If you are debugging
it, extending it, or wondering why your chip sits on the wrong pads, read on.

## The problem Fusion sets

An EAGLE/Fusion library (`.lbr`) describes symbols, footprints (packages) and devicesets in
XML. A footprint can carry a **3D package**, but the 3D package is not in the XML. It is a
reference to a **Fusion cloud design**, addressed by a **urn**:

```xml
<packages3d>
  <package3d name="TPS54427DDAR" urn="" wip_urn="urn:adsk.wipprod:fs.file:vf.AbCd...?version=1"
             locally_modified="yes" type="model">
    <packageinstances><packageinstance name="TPS54427DDAR"/></packageinstances>
  </package3d>
</packages3d>
...
<device name="" package="TPS54427DDAR">
  <connects>...</connects>
  <package3dinstances><package3dinstance package3d_urn="urn:adsk.wipprod:fs.file:vf.AbCd...?version=1"/></package3dinstances>
  <technologies>...</technologies>
</device>
```

Three facts drive the whole design:

1. **Only a Fusion design renders.** Upload a raw STEP to the cloud and bind its urn: the
   reference resolves, the library opens, and the 3D preview says "Thumbnail download
   failed". A `.f3d` (a real Fusion design with the STEP imported into it) binds and renders
   exactly like a package minted by hand in the GUI.
2. **The GUI way is dialog-trapped.** Fusion's Package3D generator ends with FINISH, which
   pops a native Save dialog and two Chromium modals that no automation can read or dismiss
   in the background. So the bridge never presses FINISH.
3. **The generator loads the first package of the open library**, whatever path you hand
   the command. This one is the trap pictured below.

## What one part goes through (the "mint")

```
per-part .lbr  --open-->  library active
                --Electron.Create3DPackage-->  generator document, footprint (pads + courtyard) loaded
                --importToTarget(STEP)-->      chip placed exactly as the STEP was published
                --doc.saveAs(<pkg>_3d)-->      cloud design in  Adom AI Workspace/<library>/   (no FINISH)
                --wait for lineage id-->       urn:adsk.wipprod:fs.file:vf.<id>?version=1
```

- The bridge trusts the STEP's origin and orientation as published on the wiki. It does
  **not** rotate or translate by bounding box: some chips are lopsided and no bounding-box
  rule is ever right for all of them (`orient:false`).
- `orient:false` means exactly that since 1.10.8: no bounding-box reorientation, and the pin-1
  mark routine does not rotate the model either. When a manufacturer publishes an authored
  transform, `fusion_make_3d_package` and each `parts[]` entry of `fusion_build_library_3d` take
  an explicit `placement: {translation:[x,y,z] mm, rotation:[rx,ry,rz] deg, scale:1}`, which
  bypasses every heuristic. The reply carries `placement.appliedMatrix`,
  `placement.verification` (`verified` or `mismatch`) and `source {path, sha256, bytes,
  preserved}`. A bad shape or a scale other than 1 is refused before Fusion is touched
  (`invalid_placement`, `unsupported_scale`).
- The saved design lives in the user's project under `Adom AI Workspace/<library name>/`,
  one per package. That folder is the 3D package store; Fusion's own GUI keeps the same kind
  of design, it just hides it. Do not delete it: the library's urns point at it.
- The lineage id doubles as the version id for these single-version designs, which is why
  the urn can be constructed rather than looked up.

## What a multi-part library goes through

```
bundle.zip (one merged .lbr, N STEPs)
  -> unpack (zip-slip guarded, size capped), place under Documents/AdomLibraries/<name>/
  -> pair every deviceset with a STEP by name (deviceset, package, normalised, prefix)
  -> SPLIT the merged library into one temporary library per deviceset
  -> mint each part on ITS OWN library          (fact 3 above)
  -> bind all N urns into the merged .lbr in one pass (merge-aware, idempotent)
  -> open the merged library, prove every deviceset (4 frames each, measured)
  -> save the library to <project>/Adom Libraries/<name>  (Library Manager lists it)
  -> close the scratch (generators, <pkg>_3d tabs, temp libraries), leave the library open
```

### Why the split matters: the wrong-footprint run

The first multi-part run minted all three parts against the merged library. Because the
generator loads the **first** package of whatever library is open, chips two and three were
imported onto part one's footprint, saved, uploaded, bound and "proven".

Reproduced on purpose for these docs: the six-part Demo Board library, the generator started
for the CH340G. Fusion loaded the library's first package, the LAN7800's 48-pad QFN, and the
CH340G's 16-lead SOIC went on top of it. The pads run off both ends and sit under a body that
does not fit:

![CH340G on the LAN7800's QFN-48 footprint](docs/img/multipart/wrong-footprint-ch340g.png)

After the split, the same CH340G on its own SOIC-16 footprint, a pad under every lead:

![CH340G on its own footprint](docs/img/multipart/own-footprint-ch340g.png)

The mint now reads the generator document's name back (Fusion names it after the package it
loaded) and refuses with `wrong_footprint_loaded` if it is not the package being minted, so
this class of error fails before anything is uploaded.

### The finished library

Three devicesets in one library, each with its symbol, footprint and rendering 3D package:

![adom-multi-3 in the Content Manager](docs/img/multipart/library-three-parts-ganged.png)

### The end state

What the user is left with: one tab, the saved library (no unsaved marker on any deviceset),
active in the Electronics Library workspace, listed in Library Manager under
`<project>/Adom Libraries/`. Every generator document, every `<pkg>_3d` design tab and every
temporary per-part library was closed as soon as its part was done, so the open-document
count never exceeded three during the whole three-part run:

![The saved library left open for the user](docs/img/multipart/end-state-library-open.png)

## What the user sees while it runs

The bridge shows an Adom-branded palette inside Fusion for the whole run, including while
the Package3D generator is active (this is a capture of that palette during the generator
stage):

![The Adom banner inside the Package3D generator](docs/img/multipart/banner-in-package3d-generator.png)

The Quick Actions page (and any web-control caller) gets the same run as a 1 fps live view
of the Fusion window with its owned popups composited at their screen positions, the
bridge's measured percent, the stage text, and the proof frames as they land:

![Quick Actions live view during an import](docs/img/multipart/quick-actions-live-view.png)

Several wiki parts can be picked into one multi-part library from the page:

![Picking three parts for one library](docs/img/multipart/quick-actions-multi-pick.png)

## Proof, not "done"

`fusion_prove_result` is the single poll surface. `stage: done` only means the chain
finished; the score is `verdict`. Every deviceset is proved with four frames (symbol,
footprint, package3d, ganged), each frame is measured from the file (a 237x39 title-bar
sliver once passed as a "symbol" until this check existed), and `provedParts[]` is per part.
`partMetadata` reports missing MPN / manufacturer / distributor numbers; it never blocks the
import, it bubbles up so someone can fix the data upstream.

**Prove is decided by the caller.** Proof frames are for web control, so web control and anonymous
callers prove. An AI thread binds and places only: the run ends with `verdict: bound_not_proven`,
`partsBound` and the library path, `prove.planned` and `prove.skipped` say what happened and why,
and the banner's final line says "bound; proof skipped" instead of a proof count. `prove:true` or
`prove:false` on `fusion_create_library` or `fusion_install_library_bundle` always wins, and the
reply says what prove costs before it starts. For one part's frames afterwards, use
`fusion_capture_library_views`.

The offline lint in `fusion_library_parts` reports pads that share a base name (`B4` and `B4@2`,
heatsink vias on the exposed pad's own net) as an info line, `heatsink_vias`. They are one net, not
a short. `pad_overlap` stays for pads with genuinely different names.

## Contracts worth knowing

| Situation | What you get |
|---|---|
| A deviceset has no STEP in the bundle | a named skip in `unbound[]`; the rest still bind; `partsBound < partsTotal` |
| No STEP at all | `installed:true`, `nothing_to_bind`, nothing opened in Fusion |
| Fusion closed | the chain launches it (about a minute) and carries on |
| Fusion open, add-in silent | the chain never restarts Fusion on its own. A long operation in flight reads as `fusion_addin_busy` and the chain waits. When the add-in really is not answering, the run says so and stops short of a restart unless the caller passed `allowRestart:true` with a `restartReason`; a restart the chain performs is listed in `restarts[] {at, why, kind}` on `fusion_prove_result`. The "Save and restart" toast exists only under `restartPolicy: ask`. |
| A dialog in front of Fusion | read through UIA and expired safely, then the chain carries on. A dialog that survives ends the run as `blocked_by_dialog` with the dialog's text and buttons. Fusion is never restarted over a dialog |
| The bridge restarts mid-run | `bridge_restarted_mid_run`, honest and terminal; scratch documents are swept before the next run |
| Same library name already in `Adom Libraries` | reported in `cloudLibrary.olderSameName`; retired only with `replaceExisting:true` |
| Several devicesets on one package (symbol variants, value clones) | one mint per package; every device on it gets the urn; listed in `shared[]`, not `unbound[]` |
| The user presses Cancel (`fusion_cancel_run {runId}`) | the run stops at its next part boundary: parts already minted keep their 3D and are bound, scratch is closed, the library stays open (not saved to the cloud); `stage: cancelled`, `cancelledAt` says which stage |

## Skills

- `fusion-multipart-libraries`: the manual recipe (per-part f3d, urn construction, binding
  markup, merge rules) and the dead ends.
- `fusion-web-control`: the import lane, pre-flight, progress, evidence, and every failure
  contract, for web callers and AI threads alike.
- `fusion-part-metadata`: MPN, manufacturer and distributor numbers, and variants.
- `fusion-ralph-test-via-wiki`: how the bridge tests this lane the way a user drives it.
