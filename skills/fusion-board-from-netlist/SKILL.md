---
name: fusion-board-from-netlist
description: "Build a whole circuit board in Autodesk Fusion from a netlist, end to end, with the AI driving: the parts library with real 3D and part numbers from the wiki, the schematic drawn live in labelled groups, the board started from the fab's design rules and the fixed parts, then placement, routing, pours, current and heat analysis with refinement, and the manufacturing files, through adom-aiflow's Fusion lane and adom-fields. Proven on the 12 V to 5 V buck molecule (2026-10-05). Read this before building any board in Fusion. Trigger words: build a board in Fusion, design a PCB in Fusion, Fusion electronics from scratch, netlist to Fusion, schematic in Fusion, Fusion schematic live, Fusion board from schematic, aiflow in Fusion, Fusion lane, buck in Fusion, molecule in Fusion, Fusion gerbers, Fusion BOM with part numbers, fusion_schematic_plan, fusion_schematic_run, fusion_board_seed."
---

# A whole board in Fusion, from a netlist

The order matters, and each step has one verb or command that does it with no dialog. Every step reads
back what it did; trust the readback, not the reply.

## 0. Start clean

Fusion must not have half-finished imports open: a design imported next to them can come up with
"Schematic/PCB sync deactivated", and then parts added to the schematic never reach the board. Save
anything worth keeping (`fusion_save_to_cloud`), close the rest (`fusion_close_document {save:false}`),
and restart Fusion (`fusion_stop`, `fusion_start`) when in doubt.

## 1. The parts library (container side, then Fusion)

For each part, clone its wiki component page (`adom-wiki repo clone <mpn>`). The page's `page.json`
names the files: `model_3d_step` is the model to bind (the laser-etched or coloured one; `*_plain` is
the bare one).

- `adom-lbr generate --sym <sym> --fp <fp> --part-metadata page.json --name <MPN> --package-name <MPN>`
  gives each part its own package, so each value keeps its own etched 3D model. Use the page's
  `-outline.kicad_sym` (the chip outline drawn in the symbol) for the parts a reader should recognise
  at a glance (the regulator, the inductor, the bulk capacitor), and standard R, C, L and LED symbols
  for the passives.
- Put Mouser, DigiKey and LCSC numbers in `component.distributor_links` before generating; they become
  MOUSER_PN, DIGIKEY_PN and LCSC_PN on the part and come out in the BOM at the end. Verify each number
  against the exact MPN; never guess one.
- Mark each deviceset `uservalue="yes"` so a VALUE never asks "change it anyway?".
- `adom-lbr assemble *.lbr -o <Library>.lbr`, stage it and the per-part `.lbr` and `.step` files on the box
  (`send_files`), then `fusion_build_library_3d` binds a rendering 3D package to every part (about a
  minute a part; poll `fusion_prove_result`).

## 2. The design starts from the fab's rules and the fixed parts

`fusion_board_seed {libraryPath, outDir, name, outlineMm:[w,h], fixed:[{ref, deviceset, value, x, y, nets:{pin:net}}], rules:{clearance, edge, minWidth, minDrill, holeToHole, padRestringRatio, padRestring, viaRestring, copperUm}}`
writes the starting schematic and board. Fusion's default rules (0.35 mm minimum drill, 1 mm from
copper to the edge, a 25 % pad restring) refuse most fabs' vias and turn a 1.2 mm drill's 1.6 mm pad
into 1.8 mm, so always pass the fab's rules. Fixed parts are the ones that cannot move (a molecule's
machine pins and contacts).

Then, with an electronics document in front, `fusion_new_electronics_from_eagle {schPath, brdPath}` and
poll `fusion_eagle_import_status` until done. Name the design at once with `fusion_save_to_cloud`; every
later step targets it by name.

## 3. The schematic, live

`fusion_schematic {action:"show"}`, then
`fusion_schematic_plan {libraryPath, parts:[{ref, deviceset, value, block}], nets}` returns one command
group per block. Run each group with `fusion_schematic_run {libraryPath, commands, pauseMs}`: the sheet
grows block by block, ADD is proven against the library first (no picker), and the reply carries the
sheet's nets. A long list is fine: the bridge checks all of it first, then runs it in batches of 300. If the reply says
`stillRunning`, poll `fusion_schematic_run {runId}` until it's false; that answer is the whole reply. Compare them with the netlist pin by pin before going on. The board follows the schematic:
`fusion_board_info` should count every part.

## 4. The board: adom-aiflow's Fusion lane

Pull the board (`fusion_board_source`), write the spec (loads, hot parts, wide nets, pours, `planeUnder`
regions, Kelvin pairs; coordinates in the board's frame), then
`adom-aiflow start --board <board>.brd --spec spec.json --remote-board fusion:<design> --target <box>` and
the usual steps: `place check`, `place land`, `route`, `gate`, `land route`, `pour`, `land vias`,
`land pours`, `measure`. Read the adom-aiflow skill and its `docs/fusion-lane.md`.

## 5. Current and heat, and the refinement

`adom-aiflow analyze current` pulls the live board to `board-live.brd`; `adom-fields analyze --board
board-live.brd --spec spec.json --out fields` solves it; analyze again with the solve. When a net necks,
find out why before widening anything. On the buck molecule:

- SW at 133 A/mm2 at the regulator's pin: a GND track crossed the SW patch, so the pour was cut off from
  the pin. A `planeUnder` entry keeping other nets out of the patch brought it to 46.
- VIN at 62 A/mm2: the narrowest copper was the ring around a 0.8 mm via's hole. 1.0 mm power vias
  (`viaPower`) fixed it; a wider track could not enter the regulator's 0.6 mm pin.

Refine by changing the spec and running the board steps again; `adom-aiflow unroute --net <net>` takes a
single net up when only it must change.

## 6. Finish and the manufacturing files

`adom-aiflow finish`, then `fusion_export_gerbers` (the bundled JLCPCB CAM job), `fusion_export_cpl`
(pick and place) and `fusion_board_bom {outputDir}` (one line per part number with the Mouser, DigiKey
and LCSC numbers, plus the JLCPCB form; it names any part missing a number).
