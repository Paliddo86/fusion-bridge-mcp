---
name: fusion-build-parts-library
description: Reproducible recipe for building a WHOLE parts catalog (e.g. the 138-part Adom basic parts library) into ONE Fusion 360 .lbr where every value is placeable with its real footprint + a rendering 3D model - the SHARED-FOOTPRINT method, end to end. Collapse the catalog to its unique (type,package) footprints, build a 3D package per footprint with fusion_build_library_3d, then clone the base deviceset per catalog value so all values share the footprint + symbol + 3D binding. Covers sourcing the footprints/3D (wiki per-MPN pages, the static footprints library, generic KiCad models), the deviceset-cloning assembly, validation, verification, and publishing. Read this to reproduce or extend the Adom basic parts Fusion library, or to turn any parts list into a Fusion library. Trigger words - build a parts library, adom basic parts library, basic parts fusion lbr, whole catalog library, shared footprint library, clone devicesets, 138 part library, reproduce the basic parts library, parts table to lbr, resistor capacitor library fusion.
---

# Build a whole parts catalog into one Fusion library (the shared-footprint method)

How the **138-part Adom basic parts** Fusion library was built, reproducibly. The key idea (see also
[fusion-multipart-libraries](../fusion-multipart-libraries/SKILL.md)): a real library has FAR fewer
footprints than values - one `R-0402` footprint serves all 41 of the 0402 resistor values. So you
build a 3D package per *footprint* (~10), then clone a deviceset per *value* (138) that all share it.

## Step 1 - get the catalog + collapse it to unique footprints

The Adom basic parts catalog lives in the **[basic-parts-library](https://wiki.adom.inc/basic-parts-library)**
wiki page (the parts array is embedded in its viewer html). Each part has `type`, `value`, `package`,
`mfrPN`, `manufacturer`. Collapse to the unique **`(type, package)`** combos - the 138 parts are just
**10 footprints**: R-0402, R-0603, C-0402, C-0603, C-0805, L, LED-0402, LED-0603, LED-2020, FB-0402.

## Step 2 - source ONE footprint + STEP per combo

Passive land patterns are standard per package (IPC-7351), so one representative part per combo is
enough; a 0603 cap shares the 0603 resistor land, etc. Sources, in order of preference:
- **Per-MPN wiki pages** (chip-fetcher output): `/api/v1/pages/<mpn-lowercased>/files/<MPN>.kicad_mod`
  + `.step` (+ `.kicad_sym`). Pick a representative `mfrPN` from the catalog for the combo.
- **The wiki static footprints library**: `static/footprints/<pkg>/<Name>.kicad_mod` (e.g. `c-0603`,
  `r-0805`).
- **Generic KiCad models**: `C_0603_1608Metric.step`, `R_0402_1005Metric.step`, etc.

## Step 3 - `adom-lbr generate` a base .lbr per combo

```bash
adom-lbr generate --sym <combo>.kicad_sym --fp <combo>.kicad_mod --name <combo> \
                  --model <combo>.step -o <combo>.lbr
```
Gives, per combo, a `.lbr` with one `<package>` + `<symbol>` + `<deviceset>` (named `<combo>`).

## Step 4 - build the 3D once per footprint (~10 calls, not 138)

Merge the ~10 base `.lbr` into a combined `base10.lbr` (concatenate `<packages>`/`<symbols>`/
`<devicesets>`), stage to Windows, then:
```jsonc
fusion_build_library_3d { "lbrPath":"C:/.../base10.lbr",
  "parts":[{package, lbrPath (per-combo .lbr), modelPath (.step)}, ...],
  "outLbrPath":"C:/.../base10_bound.lbr", "task":"AdomBasicParts-3D" }
```
This makes a footprint+chip `.f3d` per combo, uploads it **into `Adom AI Workspace/<task>`** (never the
project root - see [fusion-cloud-hygiene](../fusion-cloud-hygiene/SKILL.md)), and injects the
`<package3d>` bindings. Result: 10 packages, each Mapped to a rendering 3D model.

## Step 5 - clone a deviceset per catalog value (138 from 10)

For each catalog part, copy its combo's bound `<deviceset>` (which already carries the shared
`<gate symbol=…>`, `<device package=…>`, and `<package3dinstances>` 3D binding) and change only:
- `<deviceset name="…">` to an EAGLE-style value name (`R-0402-4K7`, `C-0603-100N`, `LED-0603-BLUE`,
  `FB-0402-600R`), and
- the `<technology>` attributes: `VALUE` (the human value), `MPN`, `MF`, `MOUSER`.

Replace the ~10 base devicesets with the 138 clones. Final `.lbr` = 10 packages + 10 symbols + 10
`<package3d>` + **138 devicesets**, every one Mapped + rendering. `adom-lbr validate` (the only errors
should be the harmless `duplicate-pin-name` warnings).

## Step 6 - open, verify, capture, publish

- `fusion_open_lbr` the full `.lbr`; `fusion_check_dialogs` == 0 (no broken refs); each device shows
  Mapped ✓ + a rendering 3D preview.
- `fusion_capture_library_views {packages:[...]}` for symbol/footprint/component screenshots; pull the
  `before/after` 3D shots; make a montage.
- Publish the `.lbr` + README (a table of all values) + screenshots + video to the wiki.

The whole thing is ~10 3D builds + a deterministic clone-and-assemble - not 138 separate builds.

## Creating chips from parameters (fusion_generate_package) + marking them readably

For GENERATED parts (no vendor STEP needed - royalty-free, license-clean), use
**`fusion_generate_package`**: it drives Fusion's built-in ElectronicsPackageGenerator (~50 IPC
families - chip/soic/qfn/qfp/bga/sot23/dfn/melf/ecap/crystal/headers...), marks the chip top, and
exports STEP in ONE call. Params are datasheet **mm** (auto-converted).

**How the marking finds the top surface (also baked into the verb - you get it for free):**
1. Take the bounding box of the WHOLE chip (all bodies, terminals included).
2. Only upward planar faces whose centroid sits in the **top 5% height band** of that bbox are
   candidates (terminals often TIE the molded body on height - a naive max-Z pick etches a terminal).
3. The **largest-area** face in the band is the top surface.
4. Lay the text out with a **10% margin** inside that face, width-aware auto-fit
   (glyph ~0.75h; override with `etchHeightMm`).

**Marking style - contrast matters.** Default `etchStyle:"raised"` puts the text on as a thin
**positive extrude painted WHITE** (silkscreen-style) - humans can't read a dark-on-dark emboss.
`etchStyle:"engraved"` gives the sunken laser cut when that's what a datasheet calls for. Raised
markings are separate `Marking` bodies (white in Fusion and in the colored STEP).

```bash
adom-bridge fusion_generate_package '{"type":"soic","params":{"DPins":8,"D":4.9,"E":6.0,"E1":3.9,"e":1.27,"A":1.6,"A1":0.1,"b":0.41,"L":0.84},"etch":"LM358","outputStep":"%TEMP%/ADOM-LM358-SOIC8.step"}'
# macOS: swap `%TEMP%` for `$TMPDIR` in `outputStep` (it is expanded on the Fusion host)
```

Read the verb's `_hint` on every call - it carries the band/margin/contrast rules and the per-family
param names (chip: D/E/A/L/L1; soic: A/A1/b/D/E/E1/e/L/DPins).

**Long-axis + manifest (v1.6.19).** The marking always runs along the LONGEST axis of the top face
(auto-rotated 90 deg when the long axis is sketch-y) - maximum room for text. Multi-line works: join
with a newline ("LM358\nADOM-A" = MPN + variant, per-line auto-fit). With `outputStep`, a sidecar
`<step>.manifest.json` records every setting + why + the calculated bboxes; to REFRESH a marking
(e.g. add a variant line) re-call the verb with the manifest's `type` + `paramsCm` (`unitsCm:true`)
+ the new etch text - generation is deterministic, so placement reproduces exactly.

## Part numbers and variants (an EE cannot order without these)

Geometry is not the part. MPN, MANUFACTURER, MOUSER_PN, DIGIKEY_PN and LCSC_PN, and the
variant model that lets an automotive-grade part and a baseline one share one symbol, one
footprint and one 3D body, are covered in **`fusion-part-metadata`**. Check
`fusion_library_parts {libraryPath}` before calling any library complete - `ok: true` from
preflight only means the file OPENS.
