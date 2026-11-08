# 2D board layout and manufacturing output

The PCB side of Fusion Electronics: the board editor, design rules, layer discovery, and every
artifact a fab or assembler needs.

## Getting to the board

```bash
adom-bridge fusion_aps_open   '{"query":"BQ25792"}'   # open the PROJECT (not the .brd)
adom-bridge fusion_show_2d_board '{}'                 # PCB editor
adom-bridge fusion_board_info '{}'                    # size, layer count, component count
```

Opening the `.brd` child directly gives an isolated view. See [schematics](schematics.md) for why.

## Design rules

```bash
adom-bridge fusion_set_design_rules '{"action":"list"}'                              # every fab, process, layers row
adom-bridge fusion_set_design_rules '{"fab":"jlcpcb","process":"standard","layers":"2"}'
adom-bridge fusion_set_design_rules '{}'                                             # the bundled 3rd party fab rules
adom-bridge fusion_set_design_rules '{"druPath":"C:/tmp/house.edru"}'                # a local rule file, as-is
adom-bridge fusion_alerts '{}'                                                       # confirm the load
```

`fusion_set_design_rules` loads a fab's rules into the open board. The rule packs live on the wiki
page [adom/pcb-design-rules](https://wiki.adom.inc/adom/pcb-design-rules): JLCPCB, PCBWay, OSH
Park, Seeed, Eurocircuits, Aisler, Elecrow, Advanced Circuits, Sierra, MacroFab, NextPCB, ALLPCB
and 3rd party fab, one `.edru` per vendor, process and layer count, every number citing the
vendor's own capabilities page. The bridge fetches the pack, stages it on the box, and the add-in
runs `drc load`. `layers` auto-detects from the open board. `{action:"list"}` answers on a machine
without Fusion, so you can offer the user a real choice of fab before a board is open. `apply` and
`export` need a board open and say so themselves.

Read `ruleWarnings` in the reply. The bridge scans the downloaded profile and warns when it carries
a same-signal Copper Clearance rule over an all/all or trace scope. The 3rd party fab 2L profile, like
the other generated profiles, carries "Same-net track spacing" (0.25 mm, all/all, samesignal=yes).
On the six-net test fixture native DRC reported that one rule as 31 Overlap warnings, at the 13 pad
entries and 18 trace and via joints. With the rule disabled the identical board is clean. Use
`{action:"export"}` to read the loaded values back.

Always confirm with `fusion_alerts` afterwards. A refused rule file is an in-canvas toast, never a
dialog. Applying a fab's ruleset before routing is what keeps a board manufacturable at the fab you
use.

## Layers

`fusion_detect_layers` reports the real layer stack of the open board rather than assuming a
2-layer or 4-layer default. It is backed by a layer-detect ULP shipped in `resources/`.

`fusion_board_stackup` returns the physical stackup: per-layer thickness, copper weight and
material. This matters more than it sounds. When you render a board in 3D, the **board bbox is the
FR4 body**, never the whole assembly, because components overhang the edges and tower above it.
Getting that wrong produces a 3D model that looks correct and measures wrong.

Below is a real Adom board, the DRV8411A motor-driver, open in the Fusion Electronics 2D PCB
editor. The copper pours, routed traces, silkscreen and pad stack are all live and editable here,
which is where `fusion_electron_run` commands act.

![DRV8411A copper layout in the Fusion Electronics 2D editor](fz-2d.png)

## Routing

### Live per-net routing

Four verbs route a live two-layer board one net at a time, with explicit copper and no general
autorouting. All four take `expectDocument` (the exact active board name), and the three that
follow `fusion_routing_state` take the `boardId` and `expectedRevision` it returned, so a call made
against a board that changed underneath it refuses instead of mutating the wrong geometry.

```bash
adom-bridge fusion_routing_state '{"expectDocument":"six-net-brd"}'
adom-bridge fusion_plan_route    '{"expectDocument":"six-net-brd","boardId":"...","expectedRevision":"...",
                                   "net":"SDA","fromPad":"U1.3","toPad":"R2.1"}'
adom-bridge fusion_route_net     '{"expectDocument":"six-net-brd","boardId":"...","expectedRevision":"...",
                                   "net":"SDA","layer":1,"width":0.25,"points":[{"pad":"U1.3"},[12.5,8.0],{"pad":"R2.1"}]}'
adom-bridge fusion_routing_drc   '{"expectDocument":"six-net-brd","boardId":"...","expectedRevision":"..."}'
```

| verb | what it does |
|---|---|
| `fusion_routing_state` | Reads the live board from a fresh Fusion export: `boardId`, geometry `revision`, `pads`, `segments`, `vias`, `nets` and `remainingNets` (per-net airwires). |
| `fusion_plan_route` | Read-only. Calculates a bounded orthogonal A* path between `fromPad` and `toPad`, using existing copper and the shared conservative clearance model. Returns explicit arguments for `fusion_route_net`. Tunable with `grid`, `allowVias`, `viaCost`, `maxNodes` and `timeoutMs`. |
| `fusion_route_net` | Creates the copper. One or several trace segments per call (`points`, or `paths`), on layer 1 or 16, with `width`, `clearance`, `viaDiameter` and `viaDrill`. It runs a conservative clearance preflight, checks the board and revision guards, and reads the exact copper back (`verified`, the new `revision`, `remainingNets`). `dryRun:true` plans without mutating. |
| `fusion_routing_drc` | Runs fresh native Autodesk DRC and reads `UL_BOARD.errors`, including approved errors, warnings and airwires. It does not accept or clear any violation. |

`fusion_routing_drc` in detail: the reply carries `drc {checked, clean, errorCount, warningCount,
airwireCount, errors}`. Each error reports `layer`, `x` and `y` in mm, so you can pan straight to
it with `fusion_electron_pan`. A `summary` groups the errors by description. When Overlap warnings
are same-signal joints (a trace meeting its own pad or via), a note says so, and the usual cause is
the same-signal clearance rule that `ruleWarnings` names above. The geometric preflight in
`fusion_route_net` covers two layers and is deliberately conservative. Native DRC is the final
word.

The `fusion-routing` skill is the board-engineering playbook for these verbs. It asks which
manufacturer the board is for, loads and reads back that house's rules through
`fusion_set_design_rules`, and keeps the stricter of the rules and the user's electrical
constraints.

### EAGLE commands

Fusion Electronics is EAGLE underneath, so the editor also takes EAGLE commands:

```bash
adom-bridge fusion_electron_run '{"command":"RATSNEST;"}'
adom-bridge fusion_electron_run '{"command":"AUTO ;"}'      # autoroute
```

`fusion_execute_text_command` is **space-delimited**. Note the space in `AUTO ;`.

An explicit `SHOW <name>` is allowed through `fusion_electron_run`: it highlights the net or part
and returns. Only a bare `SHOW`, which waits for a click and would park the add-in, stays blocked.

```bash
adom-bridge fusion_electron_run '{"command":"SHOW SDA;"}'
```

## Manufacturing output

| verb | produces |
|---|---|
| `fusion_export_gerbers` | Gerbers + drill files, via a CAM job |
| `fusion_export_bom` | electronics BOM (this is the **electronics-only** one, see below) |
| `fusion_export_cpl` | component placement list for pick-and-place |
| `fusion_export_dxf` / `fusion_export_dwg` | 2D vector output |

JLCPCB CAM jobs ship with the bridge in `resources/`, so gerbers come out in the shape JLC expects
rather than a generic set you have to fix.

```bash
adom-bridge fusion_export_gerbers '{"outputPath":"%TEMP%/fab"}'
adom-bridge fusion_export_bom     '{"outputPath":"%TEMP%/bom.csv"}'
adom-bridge fusion_export_cpl     '{"outputPath":"%TEMP%/cpl.csv"}'
```

### `fusion_export_bom` is electronics-only

On a mechanical assembly it returns `No board open (current workspace: Design)`. That is expected.
For mechanical assemblies use [`fusion_assembly_bom`](mechanical-bom.md), which is kit-aware and
matches Fusion's Manage -> BOM.

## Publish the whole design, not just the 3D

When a board gets a wiki page, that page repo should carry **every** exportable file: `.f3d`/`.step`
/`.usdz`, EAGLE `.brd` + `.sch`, Fusion `.fbrd` + `.fsch`, gerbers, BOM, CPL, GLBs and renders. The
page exists so someone else can build the design, and a 3D model alone does not let them.

## Gotchas

- Wrong workspace is the usual cause of a refused verb. Check `isElectronics` in
  `fusion_get_app_state`.
- A blocking dialog (recovery prompt, update nag) no longer swallows commands silently. The bridge
  reads it, expires it safely and runs the verb. A dialog that survives comes back as
  `blocked_by_dialog` with its text and button names. Answer it by name with
  `fusion_dismiss_blocking_dialogs`. The bridge never restarts Fusion over a dialog.
- The `expectDocument` guard: every add-in verb accepts `expectDocument`, and refuses with
  `wrong_document` when the named document is not the active one. For example
  `fusion_run_modeling_script {expectDocument:"X"}` will not run against whatever happens to be in
  front. The guard fails open when the active document cannot be read. The routing verbs require
  it.
- Gerber export runs a CAM job and is genuinely slow on a dense board. A relay timeout at ~60s does
  not mean it failed; the work continues on the bridge, so poll.
