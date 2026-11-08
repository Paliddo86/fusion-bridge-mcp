# adom-eda-checks: the shared checks engine and board-file writers

The checks tier of the EDA Bridge Standard, computed from the board model and its companions as plain JSON. It makes
no bridge calls, runs no tool and does no I/O. A bridge links the crate (the Altium, KiCad and Fusion bridges are all
Rust) or runs the `adom-eda-checks` command, then answers the check under its own prefix (`kicad_return_path`).

The code moved over from the Altium bridge's `bridge-rs/src/`, which was the reference implementation. On Altium's own
boards the engine gives the same answers the Altium bridge 0.2.76 gives.

## Checks

| Check | Reads | What it answers |
|---|---|---|
| `assembly_check` | board, stack? | the board against JLCPCB's two PCBA tiers as published (with the page and its read date): size (or `panelMm`), thickness and layers from the stack, one or two sides, the smallest two-pad part and the finest IC pitch, fiducials, and parts without an LCSC number (`lcscMissing` from the bridge's BOM). Pads sharing one centre are one land, and a component with no pads is no part. `service` is the cheaper tier every rule allows. Matches the Altium bridge's answers on its test boards. From its assembly.rs |
| `auto_classes` | board | net classes from the names the nets already have (GND, Power, Clock, HighSpeed) and differential pairs from their suffixes (`_P`/`_N`, `DP`/`DN`, `+`/`-`), with `applyArgs`: the net_classes and diff_pairs calls that write them (Power with a `powerWidthMm` width rule). From the Altium bridge's autoclass.rs |
| `board_report` | board, and every companion it can use | every check the engine has, run on this bundle, as one self-contained HTML page (`text`), dark by default: fab limits (with `fab`), JLCPCB assembly, schematic review and symbols against footprints (with `sheetParts` and `netlist`), impedance, return paths and copper balance (with `stack`), 3D heights (with `bodies`), the design review, and the tool's own DRC (`drc`) and a base64 PNG of the board (`picture`) when the bridge passes them; `args` carries `lcscMissing`, `panelMm`, `gridMm` and `alwaysChecked` to those checks. A section whose input is missing says which to pass. From the Altium bridge's report.rs |
| `bom` | sheetParts, board? | the bill of materials: fitted parts grouped one line per manufacturer part number (natural designator order), a variant applied (`args.variant`: notFitted, alternates), checks for what an order needs (missing MPN, maker or Mouser/DigiKey/LCSC numbers, lines mixing values, JLCPCB lines without LCSC, DNP, excluded and not-purchased parts, sheet and board mismatches), and the order files as text (`files[]`: bom.csv, bom-jlcpcb.csv, cpl-jlcpcb.csv, bom-mouser.csv, bom-digikey.csv); from the Altium bridge's bom.rs |
| `check_clearance` | board, bodies? | pairs of parts closer than `thresholdMm`, closest first: `mode: courtyard` (default 0.25) measures part boxes (pads, and 3D bodies when given) on the same side; `mode: pads` (default 0.2) measures pad copper of different parts that shares a layer, by its real outline, skipping same-net pairs unless `includeSameNet`; `refs` keeps pairs involving those parts. A part under a raised one (pads apart, bodies apart in 3D) is listed in `stacked`, not `pairs`, as placement_check lists it. From the KiCad bridge's place_tools |
| `check_orientation` | board | advisory: each two-pad part's quarter turns scored by airline length (MST of a routable net with the pad there, distance to the nearest pad of a plane-like net such as GND); turns saving `minImprovementMm` (0.6) or more are suggested, nothing moves. From the Altium bridge's design.rs |
| `copper_balance` | board, stack, pours | copper per layer and how evenly mirrored layers match (bow and twist) |
| `return_path` | board, stack, pours | signal tracks running over a gap in the neighbouring layer's reference copper, with a pour plan that closes each gap (`apply_pours`) |
| `estimate_cost` | board, stack?, rules?, prices? | the numbers a fab quote needs (size, copper layers, thickness from the stack, smallest track, gap from the clearance rule, smallest drill, holes, SMD and through-hole parts and joints, unique parts, assembled sides) and, with a `prices` record, a rough table at `quantities` (default 5, 10, 30, 100), with assembly when asked, every assumption stated and the record named in `source`. The engine carries no rates: without `prices` the reply is the numbers only. From the KiCad bridge's fab.rs |
| `find_space` | board, bodies? | free spots for a part (`part`, or `sizeMm`) near another (`near`) or a point (`at`): clear of other part boxes on that side by `clearanceMm`, of that side's routed tracks and the vias, `edgeMm` inside the outline (arcs sampled), nearest first, as where the part's origin goes; `check: true` lists parts that overlap (and, with bodies, those `stacked` under a raised one) or have pads off the outline, as placement_check judges them. Pours do not block (they re-pour). From the Altium bridge's design.rs |
| `fab_check` | board, stack?, rules?, silk?, fab? | the board's smallest features, compared with a fab's limits from adom/pcb-design-rules; each finding names the fab's page and the date it was read |
| `impedance` | stack | the track width (and pair gap) for a target impedance on each copper layer; `apply` answers `add_rule` arguments |
| `cross_check` | sheetParts, board | the sheets' designators against the board's: matched, on the sheets but not the board, on the board but on no sheet |
| `current_capacity` | board, stack? | per net and layer, the narrowest copper against the width the current needs (IPC-2221), and the vias a layer change needs |
| `footprint_from_land_pattern` | args only | a footprint laid out from a datasheet's recommended land pattern (`family` chip, dual, sot, quad or bga, with pitch, span, pad size, `exposedPad`, `body`): pads numbered the standard way (pin 1 top-left, counterclockwise; BGA rows without I, O, Q, S, X, Z), a shorter column spread over the longer one's span (SOT-23-5's pins 4 and 5 level with 3 and 1), silkscreen (`overlay`) kept `silkClearance` from copper with a pin-1 dot, a `fab` outline with a pin-1 chamfer and the `courtyard` round the pads and body grown by its margin, in mm, Y up, origin at the centre. A missing number is refused by name. Each bridge writes it in its own library format. From the Altium bridge's footprint.rs |
| `height_check` | bodies, board? | each part's height on its side, parts over `maxTopMm`/`maxBottomMm` or under a lid region (`regions`), body collisions and near misses (`minGapMm`); `noBody` and `complete: false` when a part has no 3D body |
| `panelize` | board, stack? | a panel plan in panel mm from its lower-left corner: `rows` x `cols` copies (each with its `copyShift`), rails (`railMm`) with three tooling holes and fiducials, and `separation` none, `vscore` (V-CUT lines edge to edge with their labels) or `tabs` (mouse-bite holes and the rout path), with JLCPCB's V-cut or mouse-bite rules checked and copper near the edge or outside the outline named. Refuses tabs when the board draws its outline on a mechanical layer. From the Altium bridge's panel.rs |
| `pin_pad_check` | board, sheetParts | each symbol's pins against its footprint's pads: pins with no pad (a net that can never reach copper), pads with no pin (mounting and exposed pads excepted), parts only on the sheets or only on the board. From the Altium bridge's schreview.rs |
| `place_decoupling` | board, bodies? | positions for an IC's decoupling caps (`ic`, `caps?`, `gapMm` 0.3): each just outside the IC's box on the side of the power pin it serves, turned so its power pad faces the pin and level with it, sliding along when the spot is taken (by a part on the same side, or a through-hole pad from the other). Without `caps` it takes caps across ground and a supply of the IC, leaving out (`leftOut`) a signal net with one cap (a crystal's load) and a cap nearer another IC's pin on that supply. Each cap's pads keep clear of the stubs and vias plan_fanout would give the IC's other pins (`leaveFanout`, default true), so the fanout still fits after. A cap it cannot place stays where it is and keeps its spot. `icPowerNets` lists the IC's supplies. `moveArgs` is move_parts' arguments; `validation` is placement_check on the moved board. From the KiCad bridge's place_tools |
| `length_tune` | board, rules? | a serpentine on the net's longest straight track with room beside it, its bumps sized to add exactly what `targetMm` (or `matchTo`, another net's length) asks, every bump `clearance` (the board's strictest rule) from other nets' pads, routed tracks and vias and inside the board; answered as the track to `remove` and the `routeNet` that replaces it. From the Altium bridge's lengthtune.rs |
| `lint_library` | library | is a symbol or footprint library fit to ship (standard #28): the shared rules with a fixed severity each (`no_pads`, `zero_size_pad`, `duplicate_pad_number`, `no_courtyard`, `origin_off_part`, `pin_without_pad`, `pad_without_pin`, `no_part_number`, ...; `rules` in the reply lists them), checked from the footprints and symbols the bridge sends; a key left out is not checked, a key sent empty is a finding; the bridge's own findings (`tool_refuses_file`) pass through |
| `net_lengths` | board | the routed length of each net (tracks and arcs, by layer, with its vias), the skew of each differential pair (`pairs`, else found by net name: `_P`/`_N`, `+`/`-`) and the spread of each matched-length group against its tolerance (`groups`). From the Altium bridge's lengths.rs |
| `placement_check` | board, bodies? | courtyard overlaps (pads and bodies, grown by `marginMm`), parts off or over the edge, pads within `edgeMm` of the edge, parts on mounting holes; `stacked` lists plan-view overlaps where one part sits wholly above the other (a part under a raised molecule: pads apart, no bodies meeting in 3D); `bodiesUsed: false` when no bodies were given; plus a 0-100 score and grade (from kicad-bridge's score_placement): connectors farther than `connectorEdgeMm` from the edge, decoupling capacitors farther than `decouplingMm` from their IC's pin on the same rail, and the ratsnest's length against the board diagonal; a body-less SMD test point counts as flat, so it can sit stacked under a raised module |
| `plan_fanout` | board, stack?, rules? | the fanout of a BGA or QFN/QFP (`ref`): pitch and package from its pads; a BGA's outer ring escapes on the surface and each inner ball takes a dogbone via, or a via in the pad when the dogbone does not fit (`style` auto); a perimeter pad takes an outward stub, with a via at its end for ground or `viaPerimeter`, and an exposed pad a via grid. `calls` are route_net arguments, one per net; `conflicts` lists each via and stub closer than the clearance to another net's pad, track, arc or via anywhere on the board. A pin its net's copper already reaches is skipped (`includeRouted` plans it anyway), the exposed-pad grid keeps clear of holes already there, and `crossesPours` names other nets' pours the plan runs into (repour before DRC). From the KiCad bridge's place_tools |
| `plan_route` | board, rules? | a path between two pads of a net (`from`, `to` as Ref.Pad) on any listed copper `layers`, with through vias between them: a bounded orthogonal A* on `gridMm`, width and via from the args, else the board's width and via rules (`planner.widthFrom`, `viaFrom`), keeping `widthMm/2` + the board's strictest board-wide copper clearance rule from other nets' copper (pads by their real outline), inside the outline, no via in a pad; a through via is checked on every copper layer of the board (`planner.viasCheckedOn`), not only the planned ones. `routeNet` is route_net's arguments, unchanged. Pours are not obstacles (repour after), but `crossesPours` names each other net's pour the route runs through, since that can split it; `avoidPours: true` keeps the route out of them (or a list of nets: out of theirs only), and `pourCost` makes a mm inside one cost 1 + pourCost mm, a preference where a hard block finds no path; a pour that holds the route's own end pad is exempt and named (`planner.avoidExempt`); with no path, `ends` names the avoided pours at each end pad and whether they are exempt, and `withoutAvoid` says whether a path exists through them. The grid path is smoothed to straight and 45-degree segments where they keep the same clearance (`smooth: false` keeps it) |
| `review_board` | board, sheetParts?, rules? | small design-review checks, errors first: placeholder values, single-pin nets, duplicate designators, parts with pads off the outline polygon or near the edge (`edgeMm`), parts with no footprint name, off-grid parts (with `gridMm`); from the sheet parts, missing Mouser/DigiKey/LCSC numbers and datasheets and placeholder comments; from the rules, disabled ones, missing clearance/width/hole-size/short-circuit rules and same-priority ties (rules classed from a standard kind, a descriptor, Altium's ordinal or the rule's own name; `alwaysChecked` names classes a tool always checks, like EAGLE's shorts). `skipped` and `notImplemented` say what was not judged. From the Altium bridge's review.rs |
| `route_pair` | board, stack?, rules? | a differential pair planned as one centreline (pad midpoints through `via` waypoints) with each net offset half a pitch to its own side, mitred corners and a `fanMm` fan-out; width and gap given, or from `diffOhms` and the stack. `problems` names a crossing or other copper (pads by outline, routed tracks, vias) closer than the clearance; `routeNet` holds both nets' route_net arguments and `widthRule` the pair's add_rule. From the Altium bridge's pairroute.rs |
| `schematic_review` | sheetParts, netlist | every placed pin, with its electrical type, against the compiled netlist: power pins on no net and outputs driving each other (errors); unconnected pins, one-pin nets, inputs nothing drives and IC supplies with no capacitor to ground (warnings; a regulator's switch node on an inductor is no missing capacitor). From the Altium bridge's schreview.rs; plus the design checks from kicad-bridge's design_review and sch_check: I2C lines with no pull-up, reset and enable pins floating or with nothing setting their level, LEDs with no series resistor, regulators missing input or output capacitors, rails with no bulk capacitor, unannotated or doubled designators, no-connect flags on wired pins, and two names on one net (`netlist.names`); each with `message` and `fix`; with the `sheetDrawing` companion (one entry per sheet: wires, buses, junctions, noConnects, labels, pins, symbol boxes, mm, y down) also dangling wire ends (with the nearest pin or label), labels on no wire or pin, and symbols drawn over each other, which are listed in `unread` without it |
| `silk_designators` | board, silk | a place for each reference designator: one size (`size`, stepping down to `minSize`), beside its own part, clear of every pad by `padClearance`, of other parts' bodies and of other labels, inside the board. `applyArgs` is apply_silkscreen's arguments; parts with no clear spot are listed, not forced. From the Altium bridge's silkplace.rs |
| `teardrops` | board, rules? | a tapered fill where a track meets a via or pad of its own net (`targets` via, tht, smd), as wide as the pad at its centre (`widthRatio`) and narrowing to the track `lengthRatio` x the pad's size past its edge; each a polygon on the track's layer kept clear of other nets' copper by the board's clearance, the joints with no room skipped and named |
| `thermal_vias` | board, rules? | a grid of vias in a part's exposed pad (`ref`, `pad?`), clear of vias already there; `apply` is `place_vias` arguments |
| `via_fence` | board, rules? | vias of `viaNet` both sides of the named nets' tracks and/or round the board edge (`edge`), at `pitchMm` or lambda/20 at `freqGHz`, clear of other copper; `apply` is `place_vias` arguments |

The via plans size their vias from `args.size`/`drill`, else the board's via rule (`rules`: `{kind: "via", sizeMm, drillMm}`,
or Altium's Routing Via descriptor), else 0.6/0.3, and say which in `planned.via.from`.

The other checks-tier verbs (assembly, pair routing, length tuning,
designators, panels, the board report, schematic review, and the planners estimate_cost, check_clearance, place_decoupling and plan_fanout)
come over a module at a time.

## Writers

For a bridge whose tool has no native exporter (`writers`, from altium-lane, #16). They stay native-tier verbs in the
standard: a bridge whose tool can write the file keeps its own exporter. No I/O: each returns the file text or the
regions, with `writtenFrom`, and the bridge writes the file or places the regions.

| Verb | Reads | What it answers |
|---|---|---|
| `export_gencad` | board | GenCAD 1.4 text, in inches: outline, pads and padstacks, shapes, components (a bottom part `MIRRORX FLIP`, as KiCad writes it), devices, signals, routes |
| `export_dxf` | board, pours? | one DXF (R12) per layer in `args.layers`: Edge.Cuts as lines and arcs or one closed polyline (`contours`), copper as tracks, arcs, pads, vias and pours. With `pours` a copper layer shows the copper actually poured; without, `poursAsDrawn` names the layers that show the pour as drawn |
| `export_ipc2581` | board, stack?, pours? | IPC-2581 revision C (mm) for assembly and test, valid against the IPC-2581C schema: layers and stack-up, outline (arcs as curves), one package per footprint with its pins in the footprint's frame, components (a bottom part mirrored) and BOM, nets with their pins, and on each copper layer the pads, traces, arcs, vias and, with `pours`, the poured copper island by island with its holes, plus the drills (a hole with no ring is non-plated). `args.date` sets the history date (default now). No mask, paste or silkscreen. From the Fusion bridge's routing_script.py |
| `generate_body` | args only | a part's 3D body as STEP AP214 text from its package family and JEDEC letters (`family`, `dimensionsMm` with `pins` the total pin count and `D2`/`E2` an exposed pad, `marking?`), plus its box in the bodies companion's form; families chip, soic, qfn, qfp, sot23, dfn, bga, melf, ecap, crystal, header (#20) |
| `import_svg` | svg (text) | `regions` in board mm (y up) for a layer: each shape with its holes as one contour reaching the holes through zero-width slits |

## Input

Every check takes one JSON object:

```json
{"board": {...}, "stack": {...}, "pours": {...}, "rules": [...], "silk": {...}, "fab": {...}, "args": {...}}
```

| Key | What it is |
|---|---|
| `board` | the board model, `<prefix>_routing_state` (`schema/board.schema.json`) |
| `stack` | the stack-up companion, `<prefix>_layer_stack` |
| `pours` | poured copper, `<prefix>_pour_state` (its `zones`) |
| `bodies` | the 3D bodies companion, `<prefix>_bodies`: `bodies[{ref, side, x0, y0, x1, y1, z0, z1}]` (one box per body or per part) and `noBody`. A turned body may give its footprint as `{cx, cy, w, h, rotation}` or four `corners`; bodies are then compared by their rectangles (separating axes), not their boxes |
| `rules` | the board's rules, `<prefix>_read_rules`: a clearance rule as `{kind: "clearance", clearanceMm}` (a `value` with its unit, `"0.1mm"`, works too). Only copper-to-copper clearance counts: a rule whose kind or name names the board edge, mask, silk, paste, holes, a keepout or restrict area, the courtyard or the same net is skipped |
| `silk` | `<prefix>_silk_state` |
| `fab` | one record from adom/pcb-design-rules `rules/records/<vendor>.json`, with `vendor` |
| `prices` | a price record read from the fab's own page: `{vendor, url, read, currency, pcb: [{layers, setup, perM2, promo?: {maxMm: [w, h], prices: [[qty, price]]}}], thickness?: {standardMm, factor}, assembly?: {setupPerSide, stencilPerSide, perSmdJoint, perThtJoint, perExtendedPart}, excludes?}`. The pcb row used is the one for the fewest copper layers that covers the board |
| `sheetParts` | every sheet's parts as `<prefix>_read_sheet_file` gives them: `[{designator, comment?, params?: [{name, value}], pins: [{number, name, electrical}]}]` (or a reply with `components`). `electrical` is a type name (input, output, io, passive, power, supply, open_collector, hiz, not_connected) or Altium's code; a part whose pins have no numbers (a package-less supply symbol) is no part |
| `netlist` | the compiled netlist: `{nets: {net: ["U1.8", ...]}}` |
| `drc` | board_report only: the tool's own DRC reply (`run_drc`: errors, unconnected, violations) |
| `picture` | board_report only: a base64 PNG of the board, embedded in the page |
| `args` | the check's own arguments, as the standard verb documents them |

Each part may be the bare model or the bridge's whole reply: the model's keys at the top level, or under `data`.
A missing part is never guessed. A limit with nothing to measure it against is listed in `notChecked`. Copper weight
assumed for lack of a stack is named in `assumed`.

## Use

As a crate, vendored at a tagged standard version (the wiki has no git protocol, so Cargo can't fetch it):

```toml
[dependencies]
adom-eda-checks = { path = "vendor/adom-eda-checks" }
```

```rust
let reply = adom_eda_checks::run("return_path", &serde_json::json!({"board": board, "stack": stack, "pours": pours, "args": {}}));
```

As a command:

```bash
adom-eda-checks list
adom-eda-checks return_path --board board.json --stack stack.json --pours pours.json --args '{"resolutionMm": 0.1}'
adom-eda-checks fab_check bundle.json        # or - for stdin
adom-eda-checks export_dxf --board board.json --pours pours.json --args '{"layers": ["Edge.Cuts", "F.Cu"]}'
adom-eda-checks import_svg --svg logo.svg --args '{"at": [10, 10], "widthMm": 20, "layer": "F.SilkS"}'
```

Replies keep the standard envelope (`success`, `errorCode`, `error`, `_hint`) and name verbs without a prefix
(`apply_pours`, `add_rule`, `place_vias`): a bridge adds its own. Every reply also carries `engine`
(`"adom-eda-checks 0.20.0"`), so a bridge reports the engine it runs without writing the version by hand.

## Tests

`cargo test` runs the unit tests. To compare with a bridge's own answers, point `ADOM_EDA_FIXTURES` at a folder of
saved replies laid out as `<bridge>/<board>.<model>.json` and `<bridge>/expected/<board>.<check>.json`:

```bash
ADOM_EDA_FIXTURES=/path/to/fixtures cargo test --test fixtures
```

Fixtures live on each bridge's own page, not here.
