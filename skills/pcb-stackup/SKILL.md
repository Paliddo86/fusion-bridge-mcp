---
name: pcb-stackup
description: >
  Think about a PCB as its STACKUP - the physical layer sandwich - before doing ANY 3D work with a
  board: placing overlays (silkscreen/copper textures), computing the board bounding box, picking
  z-heights, making traces visible, or building a wiki 3D model. Records John's teaching (2026-07-15)
  after an overlay was placed at component height instead of the copper layer. Trigger words: pcb
  stackup, board layers, fr4 bounding box, board bbox wrong, silkscreen z height, copper layer z,
  traces not visible, overlay floating above board, layer heights, soldermask z, board thickness.
---

# PCB stackup - how to think about a board's layers (John, 2026-07-15)

A PCB is not "a green slab with parts". It is a **stack**, and every 3D/overlay/bbox decision must
be made per-layer, at that layer's z-height. From bottom to top (typical 1.6mm board):

```
components (top side)            <- pick-and-place, then solder reflow welds them on
top silkscreen (ink)             <- printed ON the soldermask
top soldermask        ~65 um     <- the green; OPENS over pads
top copper (traces)   ~35 um     <- layer 1; the routing
FR4 core + inner Cu   ~1.5 mm    <- inner layers (e.g. 2, 15) live inside
bottom copper         ~35 um     <- layer 16
bottom soldermask     ~65 um
bottom silkscreen
components (bottom side)
```

Measured live on Adonis's BQ25792 board (from the RAW GLB bodies): FR4 core z 0.000-1.516, top
copper 1.516-1.551, top mask 1.516-1.582, bottom copper -0.035-0.000, bottom mask -0.065-0.000.

## The rules this implies (each one was violated once and looked wrong)

1. **The board bbox = the FR4 body, never the whole assembly.** Components overhang edges (a switch
   hung off the BQ board and inflated Y 26.4 -> 28.2mm) and tower above (a quad placed at
   whole-assembly z-max floated ON TOP of the components). Measure the substrate body itself.
2. **Each overlay goes at ITS layer's z.** Copper texture -> copper layer z (FR4 top + eps).
   Silkscreen -> above the mask top. Never "board bbox top + eps".
3. **Copper traces ARE 3D geometry in a Fusion export** - real bodies, 35um thick, so they are NOT
   heavy. They are invisible for two reasons only: Fusion colors them IDENTICAL to the FR4
   appearance, and the soldermask exports opaque. Fix = recolor the copper bodies to a real copper
   material + make the mask translucent (alpha ~0.6 BLEND). No texture overlay needed.
4. **Measure bodies in the RAW (pre-optimize) GLB.** After join/flatten (gltf-transform optimize or
   the service molecule mode), meshes merge across bodies and per-layer semantics are DESTROYED
   (the "Board" node becomes a 10mm-thick mega-mesh). Identify FR4/copper/mask bodies FIRST (raw GLB
   node names: `Board`, `1-copper`, `16-copper`, `1-soldermask`, `16-soldermask`), retarget their
   materials, THEN optimize - join preserves distinct materials.
5. **The stackup is queryable, don't guess.** Two sources:
   - EAGLE `.brd` design rules (`fusion_export_eagle_source`): `layerSetup` (e.g. `(1+2*15+16)` =
     4-layer), `mtCopper` (copper thicknesses), `mtIsolate` (dielectric thicknesses).
   - The raw exported 3D bodies' z-ranges (as measured above). Fusion also shows a stackup dialog;
     the .brd rules are its programmatic equivalent.
6. **Put the stackup table on the wiki page** of every exported board (John wants this standard):
   a layer/Z/thickness table like the BQ25792 page's "Board stackup" section.

## Recipe: visible REAL traces in an optimized GLB (proven on the BQ board)

1. `fusion_export_step` -> raw STEP -> raw GLB (adom-step2glb convert). Bodies stay distinct.
2. In the raw GLB, retarget the `*-copper` nodes' primitives to a new metallic copper material
   (baseColor ~[0.85,0.42,0.12], metallic 1, roughness 0.35) and the `*-soldermask` nodes to a
   translucent green (alpha ~0.62, BLEND, doubleSided).
3. `gltf-transform optimize --compress draco` -> ~800KB with the copper as its OWN mesh.
   Result: real 3D routing visible under the mask, stackup-correct. 19MB -> 822KB on the BQ board.


## Z-fighting: coplanar faces are the enemy (learned live, 2026-07-15)

At true exported heights the stack has PERFECTLY coplanar faces: FR4 top = copper bottom = mask
bottom (one plane, e.g. z=1.516). Renderers z-fight (shimmer) on them. Fix: micro z-offsets that
break coplanarity but are invisible at view scale - mask +/-5um away from the board, copper +/-2um.
Apply to the RAW GLB nodes before optimizing (catch nested sub-assembly copies of copper/mask too).

## Inner layers do NOT exist as 3D bodies

A 4-layer board (layerSetup (1+2*15+16)) exports only OUTER copper as bodies (1-copper, 16-copper).
Inner layers (2, 15) live only in the GERBERS (Fusion CAM names them copper_l2.G1, copper_l3.G2).
To show them in 3D: render those gerbers to copper-colored transparent PNGs framed EXACTLY to the
Profile outline, and add them as textured quads at their stackup z (inside the FR4 core).

## Exploded stackup view (a standard option users may ask for)

Shift each stack group by slot*gap (gap 2mm default, 5mm if crowded), FR4 = slot 0:
  components +5 | top silk +4 | top mask +3 | top copper +2 | inner L2 +1 quad
  [FR4 core 0]
  inner L15 -1 quad | bottom copper -2 | bottom mask -3 | bottom silk -4
Real bodies move by node translation (top-level groups: Board, 1-copper, 16-copper, 1-soldermask,
16-soldermask, Packages=all components); silk + inner layers are gerber quads. Keep the WHOLE
stackup present and in correct order. Do this on the raw GLB, then optimize (join keeps materials).

## Wiki page gotcha: the Overview README is the PACKAGE layer

`adom-wiki repo push README.md` updates the Files tab + assets the viewers fetch (GLB changes show
instantly) but does NOT refresh the page Overview README - that renders from the last `pkg publish`.
Bump the version in package.json (publish reads package.json, NOT page.json) and republish.


## THE PHYSICAL FABRICATION STACK (golden, John 2026-07-15) - do not model FR4 as one slab

A multilayer board is NOT one FR4 slab with copper on it. It is built up in the fab as alternating
copper and dielectric, and the 3D view must show EACH dielectric sublayer at its REAL thickness:

```
4-layer:  Cu(L1) | prepreg | Cu(L2) | CORE | Cu(L3) | prepreg | Cu(L4)
6-layer:  Cu | pp | Cu | core | Cu | pp | Cu | core | Cu | pp | Cu     (extend the pattern)
8-layer:  ... keep extending: copper, dielectric, copper, dielectric ...
```

Fab reality: a thin CORE (rigid FR4) is etched with the two inner copper layers on its faces; then
PREPREG (bonding sheet) is epoxied on top and bottom, and the outer copper foil is pressed onto that.
So the dielectric between copper layers alternates prepreg (outer, thinner) / core (center, thicker).

The stack is in the EAGLE/Fusion design rules (this IS Fusion's stackup dialog, programmatically):
- `layerSetup` e.g. `(1+2*15+16)` - copper layer numbers, with `+` = prepreg bond, `*` = core bond.
  `(1+2*15+16)` = L1 [prepreg] L2 [CORE] L15 [prepreg] L16 (4-layer, symmetric).
- `mtCopper` - copper thickness per layer (e.g. 0.035 mm = 1 oz).
- `mtIsolate` - dielectric thicknesses (prepreg + core). NOTE: these can be left at 2-layer defaults
  (a leading 1.6mm is the default board thickness, not a real prepreg); if they don't reconcile with
  the measured board, fit a symmetric stack to the MEASURED FR4 total instead, keeping copper 0.035.

BQ25792 (measured FR4 dielectric = 1.516mm, fitted symmetric): top/bottom prepreg 0.200 each,
core 1.046, four copper 0.035. Real z (mm): L16 -0.035..0.000 | bot prepreg 0.000..0.200 |
L15 0.200..0.235 | core 0.235..1.281 | L2 1.281..1.316 | top prepreg 1.316..1.516 | L1 1.516..1.551.

## Building the exploded view with REAL thicknesses (the breakthrough view - almost no EDA tool does this)

Every element is its OWN slab at its EXACT thickness, separated by a uniform slot gap (5-6 mm). The
FR4 must be split into its sublayers (prepreg/core/prepreg) - clone the exported Board body 3x, each
z-scaled to that sublayer's thickness (scale_z = thickness / measured_FR4_thickness) and translated
to its slot center; make the sublabs slightly translucent so inner copper reads through. Order:
components / top silk / top mask / L1 copper / top prepreg / L2 copper / CORE / L15 copper /
bottom prepreg / L16 copper / bottom mask / bottom silk. Outer copper (L1/L16) are the real recolored
bodies; inner copper (L2/L15) are gerber-rendered quads (they aren't exported as bodies). Keep the
ORIGINAL Board body but recolor it alpha=0 (hidden) since the 3 slabs replace it. This makes Adom a
breakthrough: you can SEE the inner layers, which almost no EDA tool shows in 3D.

## Related
- `fusion-molecule-export` - the export/optimize pipeline these rules feed into.
- Issue adom/step2glb#158 - the service's `_board_bbox` still uses the whole-assembly XY
  ("nothing overhangs a PCB edge" is false) and a stack z that merged meshes pollute; fix is to key
  on the FR4/substrate body per rules 1-2.
