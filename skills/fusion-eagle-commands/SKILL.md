---
name: fusion-eagle-commands
description: EAGLE command reference for Fusion 360 Electronics via fusion_electron_run. Use when driving the Fusion Electronics / PCB workspace through the Adom Bridge Fusion 360 bridge — the EAGLE-syntax commands (DRC, ROUTE, RATSNEST, DISPLAY, GRID, etc.) you pass to fusion_electron_run, and which ones BLOCK (ADD/SHOW/MOVE/WRITE/EDIT) vs are safe. Trigger words - eagle command, fusion_electron_run, electron run, DRC, ERC, RATSNEST, RIPUP, DISPLAY layers, WINDOW FIT, eagle layers, blocking command, WRITE blocks, export eagle source, eagle script.
---

# EAGLE Commands for `fusion_electron_run`

> **Owned by the `fusion-bridge` skill package.** This reference is Fusion-Electronics
> specific, so it ships here (you get it via `adom-wiki pkg install adom/fusion-bridge`),
> not the adom-bridge core package. Companion skill: `fusion-electronics` (driving the editor).

Fusion's Electronics workspace is built on EAGLE. `fusion_electron_run` sends a text command through the same interface EAGLE's command line uses.

## ⚠️ Fire-and-forget — no output returned

`fusion_electron_run` never returns EAGLE's stdout. It returns `activeWorkspace`, a diff of Fusion's internal operation log (`fusionOperations[]`), and an `_hint` field. **For data, use `fusion_board_info` / `fusion_schematic_info` instead.**

## 🚫 Never use these (blocking / modal)

These commands open modal prompts EAGLE waits on forever. Your call will hang until it times out.

| Command | Why it blocks | Use instead |
|---|---|---|
| `ADD` | Prompts for component placement | Edit source .sch/.brd via import commands |
| `SHOW` (bare) | Waits for a click | `SHOW <name>` (for example `SHOW GND`, `SHOW R1`) is allowed since 1.10.3: it highlights the named objects and returns. For data use `fusion_board_info` (boards). There is no `fusion_schematic_info`: for a schematic use `fusion_export_source` / `fusion_export_eagle_source` and read the XML, or `fusion_get_app_state` |
| `MOVE` | Interactive picker | Edit source files offline |
| `CHANGE` (interactive form) | Picker for target object | Use with explicit object: `CHANGE WIDTH 0.3 (1 2)` is OK |
| `WRITE` | Cloud: opens "Version Description" dialog even with path arg (tested 2026-04-10). Local: opens Save As. | `fusion_save_to_cloud`, `fusion_export_source`, `fusion_export_eagle_source` |
| `EDIT` | Opens library/sheet editor | Open target directly |

## ✅ Popular commands (verified working)

### View control

| Command | Effect |
|---|---|
| `WINDOW FIT` | Zoom to fit entire board/schematic |
| `WINDOW SELECT` | Zoom to current selection |
| `DISPLAY ALL` | Turn on all layers |
| `DISPLAY NONE` | Turn off all layers (chain with specific layers) |
| `DISPLAY NONE; DISPLAY 1 17 18` | Top copper + pads + vias only |
| `GRID MM 1` | Set grid to 1 mm |
| `GRID INCH 0.05` | Set grid to 50 mil |
| `GRID ON` / `GRID OFF` | Toggle grid visibility |
| `LAYER 1` | Switch active layer to top copper |
| `LAYER 16` | Switch active layer to bottom copper |

### Layers reference (EAGLE standard)

| Layer | Name | Purpose |
|---|---|---|
| 1 | Top | Top copper |
| 16 | Bottom | Bottom copper |
| 17 | Pads | Through-hole pads |
| 18 | Vias | Vias |
| 19 | Unrouted | Airwires / ratsnest |
| 20 | Dimension | Board outline ⚠️ required for 3D view |
| 21 | tPlace | Top silkscreen |
| 22 | bPlace | Bottom silkscreen |
| 25 | tNames | Top reference designators |
| 27 | tValues | Top component values |
| 29 | tStop | Top soldermask |
| 31 | tCream | Top paste mask |
| 44 | Drills | Plated drills |
| 45 | Holes | Non-plated holes |
| 51 | tDocu | Top documentation |

### Routing

| Command | Effect |
|---|---|
| `RATSNEST` | Recalculate airwires (safe, always non-blocking) |
| `RIPUP ;` | Rip up **all** routed traces (destructive) |
| `RIPUP GND` | Rip up only nets named GND |
| `AUTO ;` | Run autorouter with current DRU settings |

### Design checks

| Command | Effect | Notes |
|---|---|---|
| `DRC` | Run design rule check | Non-blocking; read results via `fusion_board_info` |
| `ERC` | Run electrical rule check (schematic) | |
| `ERRORS` | Show current error list | Reads into fusionOperations log |

### Edit (non-interactive forms only)

| Command | Effect |
|---|---|
| `GRID MM 1; LAYER 21; CHANGE SIZE 1; CHANGE RATIO 12; CHANGE ALIGN CENTER; TEXT VL53L5CX (7.44 9.6); GRID INCH 0.1` | Silkscreen text at a coordinate: non-interactive (inline string + coordinates, no dialog). ALWAYS prefix `GRID MM`: a generated board's grid is inches, so a bare `CHANGE SIZE 1` makes a 1 inch label. `fusion_make_showcase_project` and `fusion_make_test_project` write the part name under each footprint themselves (`silkLabels`). |
| `NAME U1 MCU` | Rename part U1 to MCU |
| `VALUE R1 10k` | Set R1's value to 10k |
| `ROTATE =R90 U1` | Rotate U1 by 90° absolute |
| `SMASH U1` | Detach name/value labels from U1 so they can be moved |
| `GROUP ALL` | Select everything |

### File / script / export

| Command | Effect | Notes |
|---|---|---|
| `SCRIPT /path/to/file.scr` | Run an EAGLE script file | Batches multiple commands; still fire-and-forget |
| `EXPORT IMAGE /path/to/out.png 300` | Export bitmap at 300 DPI | Prefer `fusion_export_*` commands |

**For source file export, do NOT use EAGLE's WRITE command.** Use the Adom commands instead:

| Adom command | Output format | Notes |
|---|---|---|
| `fusion_export_eagle_source {"outputPath":"C:/out/X.brd"}` | Plain EAGLE XML (.sch / .brd) | Extracts from Fusion's ZIP container. Parseable by standalone EAGLE, KiCad import, or any XML tool. |
| `fusion_export_source {"outputPath":"C:/out/X.fbrd"}` | Fusion-wrapped ZIP (.fsch / .fbrd / .flbr) | Contains the EAGLE XML inside a ZIP with metadata + preview PNGs. |

Workflow for both:
1. Open the .fprj: `fusion_open_cloud_file {projectName, fileName, fileExtension:"fprj", folderPath}`
2. Board export: `fusion_show_2d_board` → export with .brd/.fbrd extension
3. Schematic export: `fusion_electron_run {"command":"EDIT .s1"}` → export with .sch/.fsch extension

### Info (read-only, but prefer structured equivalents)

| Command | Prefer instead |
|---|---|
| `INFO U1` | `fusion_board_info` → components[] |
| `SHOW GND` (to read data) | `fusion_board_info` → nets[]; `SHOW GND` itself is fine when you want the visible highlight |

## Chaining

EAGLE supports semicolon-separated command chains in a single call:

```
fusion_electron_run {"command": "DISPLAY NONE; DISPLAY 1 17 18; WINDOW FIT"}
```

Each step runs in order. If any step would block, the whole chain hangs — keep chains to view/ratsnest/display commands only.

## Verified working (with dates)

- **2026-04-10**: `WINDOW FIT`, `DISPLAY ALL`, `RATSNEST`, `DRC` — confirmed non-blocking in Fusion 2026.
- **2026-04-10**: `WRITE <path>` — **BLOCKS** on cloud docs (opens "Version Description" dialog). Use `fusion_export_eagle_source` instead.
- **2026-04-10**: `fusion_export_eagle_source` — confirmed working: extracts plain EAGLE XML (.sch/.brd) from .fsch/.fbrd ZIP containers. Tested on THRU Mid-Signal VNA DUT: .brd=111KB, .sch=73KB.

When you verify a new command works, add it here with the date so future sessions know it's safe.

## Adding new commands to this doc

This file is the source of truth — edit it directly and commit. The CLI's `fusion_electron_run` help points here via its `note` field. No rebuild required.

## Traps that cost a board (2026-10-08, the TPS54202 buck)

- **DELETE at a point can delete a whole part.** In the board editor a point that lands on a part's own tPlace artwork picks the PART, and DELETE removes the element with its connections (J4 and MP4 went this way during a silkscreen cleanup). Delete only at a plain text's or a smashed name's exact origin (that removes just the text), never on a line, and compare the element count before and after.
- **CHANGE at a point does not land** in Fusion's board editor (CHANGE LAYER 51 (x y), CHANGE SIZE (x y)), and neither does CHANGE by name ('R1>NAME'). Move and rotate by name (MOVE 'R1>NAME' (x y), ROTATE =R90 'R1>NAME') inside a SCRIPT; remove a text with DELETE at its origin.
- **`fusion_board_source` saves a new cloud version** each time it reads the board, so an edit is never left unsaved and closing without saving does not undo it. To undo, promote the earlier version: `fusion_aps_versions {query}`, then in `fusion_run_modeling_script`: `app.data.findFileById(<lineage urn>).versions` and `.promote()` on the clean one; close the open copy with `fusion_close_document {name, save:false}` and reopen.
- **`fusion_close_document` with no name closes the ACTIVE document.** Always pass `{name, save:false}` (or `save:true`).
- **Never close the bridge's "ADOM" panel window with a window-close call** (`desktop_close_window`): it closes Fusion with every open document. Click the panel's own Close button, or leave it; it is parked small at the lower right.
- **A SCRIPT stops at a dialog.** When a script's command raises a Fusion dialog, the rest is refused with "dialog is up"; check the reply and the board after every script.
