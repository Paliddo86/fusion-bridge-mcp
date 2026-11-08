# Electronics library manager

Building and maintaining Fusion Electronics libraries: symbols, footprints, and real 3D packages,
generated rather than hand-drawn.

## What a library part needs

A usable Adom part is three things that must agree with each other:

1. a **symbol** (schematic)
2. a **footprint** (the pads the part solders to)
3. a **3D package** (what it looks like on the board)

Get the third one misaligned and the board renders wrong; get the second wrong and it does not
solder.

The whole 138-part Adom basic parts library open in Fusion's library manager: the component list on
the left, and each part carrying its symbol, footprint and 3D package.

![The 138-part Adom basic parts library in the Fusion library manager](lib3-138-overview.png)

The three views an EE checks for a single part, here the schematic **symbol** (full pinout) and the
**footprint** with its pad stack:

![A part's schematic symbol in the library editor](lib2-symbol.png)

![A part's footprint with pads and layers](lib2-esp32-footprint.png)

And a **3D package bound to its footprint**, the check that catches a misaligned part before it ever
reaches a board:

![A 3D chip model bound to its footprint](lib-09-3d-bound-moneyshot.png)

## Opening and inspecting

```bash
adom-bridge fusion_open_lbr '{"lbrPath":"%TEMP%/AdomBasicParts.lbr"}'
```

`.lbr` is the EAGLE library format Fusion Electronics uses.

## Generating packages

```bash
adom-bridge fusion_generate_package '{"package":"R_4k7", ...}'
```

`fusion_generate_package` builds a parametric IPC package, including laser-etched markings, rather
than drawing pads by hand. IPC-derived geometry is what keeps footprints consistent across a
library.

## 3D packages

```bash
adom-bridge fusion_make_3d_package   '{...}'      # build one
adom-bridge fusion_attach_3d_package '{...}'      # bind it to a footprint
adom-bridge fusion_build_library_3d  '{"lbrPath":"%TEMP%/newlib/AdomBasicParts2.lbr",
                                        "parts":[{"package":"R_4k7",
                                                  "lbrPath":"%TEMP%/newlib/R_4k7.lbr",
                                                  "modelPath":"%TEMP%/newlib/R_4k7.step"}]}'
adom-bridge fusion_capture_library_views '{...}'  # render views for a wiki page
```

`fusion_build_library_3d` is the batch path: hand it a list of parts with their STEP models and it
binds 3D to each package in one pass, returning per-part success plus screenshots.

### Explicit `placement`: the manufacturer's transform, no heuristics

`fusion_make_3d_package`, and each `parts[]` entry of `fusion_build_library_3d`, takes
`placement`:

```bash
adom-bridge fusion_make_3d_package '{
  "lbrPath":"C:/Users/me/adom-lib/TMF8829.lbr",
  "modelPath":"C:/Users/me/adom-lib/TMF8829.step",
  "placement":{"translation":[0,0,0.05],"rotation":[0,0,90],"scale":1}
}'
```

- `translation` is `[x, y, z]` in mm. `rotation` is `[rx, ry, rz]` in degrees, applied about X,
  then Y, then Z (extrinsic, about the origin), then the translation. `scale` must be 1.
- It bypasses every heuristic: no bounding-box reorientation, no centering, no pin-1 rotation.
- The reply returns `placement.appliedMatrix` (a row-major 4x4 read back from the occurrence, with
  the translation column in mm), `placement.verification` (`verified` or `mismatch`),
  `placement.heuristics`, and `source {path, sha256, bytes, preserved}` for the STEP Fusion read.
- Non-finite numbers, wrong shapes, units other than mm and degrees, and any scale other than 1 are
  refused before Fusion is touched (`invalid_placement`, `unsupported_scale`).

`orient:false` now means what it says. The model is imported exactly as authored, and the pin-1
mark routine no longer rotates it (it used to survive `orient:false`). The pin-1 part of the reply
says `skipped_orient_false` or `skipped_explicit_placement`. A default mint still reports
`rotated` or `aligned`, plus a warning that heuristics were used.

### The auto-orient rule

Auto-orient used to rotate the **smallest** bounding-box dimension to Z. That is correct for a flat
SMD chip and badly wrong for a tall through-hole machine pin, which ended up lying on its side.

Tall parts now stay vertical: if `dz` is already the largest dimension, the part is left alone and
only flat SMD parts get tipped. If you are adding a part type with unusual proportions, check the
3D orientation before committing it to a library.

## Whole libraries: prove, restarts and the pad lint

`fusion_create_library` and `fusion_install_library_bundle` run the bind and prove chain in the
background, and `fusion_prove_result` is the poll surface. Three behaviors are worth knowing.
[Multi-part library import](MULTIPART_LIBRARY_IMPORT.md) has the full account.

- **Prove is decided by the caller.** An AI thread binds and places only: the run ends with
  `verdict: bound_not_proven`, `partsBound` and the library path, and `prove.skipped` says why. Web
  control proves (four evidence frames per part). `prove:true` or `prove:false` always wins. The
  reply says what prove costs before it starts. For one part's frames afterwards, use
  `fusion_capture_library_views`.
- **Chains never restart Fusion on their own.** `allowRestart:true` with a `restartReason` is the
  only way, and every restart a chain performs is listed in `restarts[] {at, why, kind}` on
  `fusion_prove_result`. A long STEP bind that starves the add-in answers `fusion_addin_busy`, not
  a restart.
- **The `heatsink_vias` lint.** `fusion_library_parts` no longer calls same-net pads a short. Pads
  that share a base name (`B4` and `B4@2`, heatsink vias on the exposed pad's own net) are reported
  as an info line, `heatsink_vias`. `pad_overlap` stays for pads with genuinely different names.

## Verifying a part before you commit it

A part that is subtly wrong is worse than a missing one, because it silently produces bad boards.
Check:

- **Pin 1** is where the footprint says it is.
- The **seat plane** is at z=0, so the part sits on the board rather than floating or sinking.
- The 3D body's footprint **matches the pad geometry**.

The `adom-chipfit` tool exists for exactly this and validates a 3D chip GLB against its
`.kicad_mod`: pin-1 alignment, package-family guard, seat-plane delta-z and size match.

## Publish the whole part

When a component gets a wiki page, publish everything: symbol, footprint, 3D model, the source
`.lbr`, and the STEP. Someone else's board depends on being able to use the part, not just look at
it.

## Related

- [3D board view and export](3d-viewer-and-export.md) for the GLB pipeline these models feed.
- [2D board layout](board-layout-2d.md) for how footprints become manufacturing output.
