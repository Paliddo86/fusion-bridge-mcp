# Fusion Bridge - Verb Reference

Every verb the Adom Bridge Fusion 360 Bridge exposes, by category, with what it does, its arguments, and an example call. Invoke any verb as `adom-bridge <verb> '<json-args>'` (Adom Bridge proxies the `fusion_*` prefix to the bridge).

Back to the [main README](README.md).


---

## Lifecycle: launch, documents, windows

### `fusion_activate_document`

Switch to an open document tab by name or type

**Args:** `documentType` (optional: Electronics|PCB|FusionDesign|Drawing), `name` (optional substring match)


### `fusion_close`

Close Fusion 360 gracefully


### `fusion_close_all_documents`

Close all open documents

**Args:** `saveChanges` (optional bool (default: false))


### `fusion_close_document`

Close a document without save dialog (avoids modal blocking)

**Args:** `name` (optional (default: active)), `save` (optional bool (default: false))


### `fusion_close_window`

Close a specific Fusion dialog window by sending WM_CLOSE (equivalent to clicking its X button). Works on dialogs that Escape doesn't close - e.g. 'Recovered Documents'.

**Args:** `hwnd` (required - HWND from fusion_window_info or fusion_dismiss_blocking_dialogs remaining[])


### `fusion_dismiss_blocking_dialogs`

First-class blocking-dialog killer. Enumerates all Fusion Qt windows, matches them against a known list of blocking/picker patterns ('What do you want to design?', error toasts, recovery prompts, etc.), and sends Escape to each offending HWND until the add-in main thread reports responsive. Whenever any fusion_* command returns errorCode 'fusion_addin_not_responding', call this FIRST before retrying.

**Args:** `extraPatterns` (optional array of case-insensitive title substrings to also treat as blocking), `maxAttempts` (optional int (default 5) - retry loop cap)


### `fusion_document_info`

List all open document tabs with cloud metadata for active doc


### `fusion_get_app_state`

Query open documents, active workspace, Electronics status


### `fusion_start`

First-class Fusion 360 startup. Discovers Fusion360.exe via glob (so version-indirected webdeploy hashes don't matter), verifies it exists BEFORE spawning (no 'Windows cannot find' GUI dialogs), launches it if not already running, waits for the AdomBridge add-in to come online, and auto-dismisses any blocking dialogs via fusion_dismiss_blocking_dialogs. Returns when Fusion is fully ready.

**Args:** `dismissBlockingDialogs` (optional bool (default true) - auto-clear the 'What do you want to design?' picker, error ), `timeoutSeconds` (optional int (default 120) - total wait for add-in + dialog dismissal)

```
adom-bridge fusion_start '{}'
-> { "addinReady": true, "status": "ok" }
```



---

## Cloud search & files (APS)

### `aps_auth`

Interactive Autodesk sign-in (opens the system browser, 3-legged PKCE OAuth). One-time per device; tokens cached in ~/.adom/aps_tokens.json and auto-refreshed. Requires aps_client_id to be configured.


### `aps_hubs`

List the Autodesk hubs (teams) the signed-in account can see.


### `aps_projects`

List projects in a hub (default: first hub).

**Args:** `hubId` (optional - default first hub (aps_hubs lists ids))


### `aps_search`

FAST indexed cloud-file search via Autodesk Platform Services (APS) Data Management API. ~Instant server-side index - NOT folder-walking. Works even when Fusion is CLOSED. Prefer this over fusion_search_cloud_files whenever APS is configured.

**Args:** `hubId` (optional - exact APS hub id (default: first hub); aps_hubs lists ids), `projectId` (optional - exact APS project id (overrides projectName); aps_projects lists ids), `projectName` (optional - substring match against your project names (default: search all projects, up to), `query` (required case-insensitive substring of the file's displayName)


### `aps_status`

Is native APS cloud search available on this box? Non-prompting discovery check - call this FIRST to decide between aps_search (fast) and fusion_search_cloud_files (slow fallback).


### `fusion_check_recovery`

Check if a cloud file has crash recovery data

**Args:** `fileName` (required), `folderPath` (optional), `projectName` (optional)


### `fusion_create_cloud_folder`

Create folder (idempotent)

**Args:** `folderName` (required), `parentPath` (optional), `projectName` (optional)


### `fusion_delete_cloud_file`

Delete a cloud file (must not be open)

**Args:** `fileName` (required), `folderPath` (optional), `projectName` (optional)


### `fusion_export_cloud_file`

Export active document to local file (step/stl/f3d/iges/sat/smt)

**Args:** `format` (optional (default: step)), `outputPath` (required)


### `fusion_list_cloud_files`

List files and subfolders in a cloud project/folder

**Args:** `folderPath` (optional), `projectName` (optional)


### `fusion_list_cloud_projects`

List all cloud projects in user's hub


### `fusion_open_cloud_file`

Open a cloud file by name. STOPS if recovery exists - you must specify recovery='open' or recovery='discard'

**Args:** `fileName` (required), `folderPath` (optional), `projectName` (optional), `recovery` (optional: open|discard)


### `fusion_save_to_cloud`

Save active document to Fusion cloud

**Args:** `description` (optional), `folderPath` (optional), `name` (required), `projectName` (optional)


### `fusion_search_cloud_files`

Search a cloud project / subfolder by file-name substring. Returns matches + a detailed scope analysis. Use this for targeted lookups; for exhaustive whole-cloud walks prefer fusion_walk_cloud_tree.

**Args:** `folderPath` (optional starting subfolder (default: project root). NARROW THE SCOPE HERE - naming the ri), `folderTimeout` (optional int seconds per-folder cloud-API timeout (default 30)), `maxDepth` (optional int (default 2, no upper cap)), `maxFolders` (optional int (default 10, no upper cap). For exhaustive 100+ folder searches consider walk), `maxResults` (optional int (default 20, no upper cap)), `projectName` (optional (defaults to active project)), `query` (required case-insensitive substring of the file name), `recursive` (optional bool (default false). false = root folder only (fast); true = descend into subfol), `searchTimeout` (optional int seconds overall wall-clock timeout (default 120). If you set this >180, also )


### `fusion_walk_cloud_tree`

Walk a cloud folder tree iteratively (BFS) and return a flat list of folders + files with full paths.

**Args:** `extensions` (optional list of extensions to include, e.g. ["f3d","fprj"]), `folderPath` (optional starting folder (defaults to root)), `includeFiles` (optional bool (default true) - set false for folder-only tree), `maxDepth` (optional int (default 10)), `maxFolders` (optional int (default 500, truncates if exceeded)), `nameContains` (optional case-insensitive substring filter on file names), `projectName` (optional (defaults to active))



---

## Electronics: schematic, board, library

### `fusion_board_info`

Get structured board data: components, nets, traces, DRC violations

_Prerequisite: Board open in PCB Editor_

```
adom-bridge fusion_board_info '{}'
-> { "rawXml": "<pcb>... copper / arcs / lines geometry ...</pcb>" }
```


### `fusion_electron_run`

Execute EAGLE command (WINDOW FIT, DISPLAY ALL, RATSNEST, DRC, etc.)

**Args:** `command` (required EAGLE command string)


### `fusion_execute_text_command`

Low-level Fusion app.executeTextCommand() access

**Args:** `command` (required Fusion text command)


### `fusion_export_eagle_source`

Export active electronics document as plain EAGLE .sch or .brd XML - extracts from Fusion's .fsch/.fbrd ZIP container

**Args:** `outputPath` (required - full path ending in .sch or .brd)

_Prerequisite: MUST have correct editor active: PCB Editor / Board Layout for .brd, Schematic Editor for .sch_


### `fusion_export_lbr`

Export library as .scr script (no 3D refs)

**Args:** `outputPath` (optional)


### `fusion_export_source`

Export active electronics document as .fsch (schematic), .fbrd (board), or .flbr (library) source file

**Args:** `outputPath` (required - full path with extension (.fsch, .fbrd, or .flbr))

_Prerequisite: MUST have correct editor active: Board Layout for .fbrd, Schematic Editor for .fsch_


### `fusion_import_electronics`

Import Fusion-native .fsch/.fbrd/.flbr file as new local electronics project

**Args:** `filePath` (required Windows path to .fsch, .fbrd, or .flbr file)


### `fusion_list_text_commands`

List available EAGLE text commands in current context


### `fusion_open_board`

Open .brd in Board Layout editor

**Args:** `filePath` (required)


### `fusion_open_electronics`

Check if Electronics workspace is active


### `fusion_open_lbr`

Open .lbr library in Electronics Library editor

**Args:** `filePath` (required)

```
adom-bridge fusion_open_lbr '{"filePath":"C:/.../nRF52840-CKAA.lbr"}'
-> { "success": true, "postOpenScreenshot": { "savedTo": "C:/tmp/...png" } } // library opens in the Electronics library editor
```


### `fusion_open_schematic`

Open .sch in Schematic Editor

**Args:** `filePath` (required)


### `fusion_show_2d_board`

Switch back to 2D board layout from 3D view

```
adom-bridge fusion_show_2d_board '{}'
-> { "activeWorkspace": "Electronics Design" }
```


### `fusion_show_3d_board`

Switch to 3D PCB view (board needs outline on layer 20)

_Prerequisite: Board must be open in PCB Editor_

```
adom-bridge fusion_show_3d_board '{}'
-> { "activeWorkspace": "3D PCB", "screenshots": [ { "savedTo": "C:/tmp/...png" } ] }
```



---

## CAD & 3D export

### `fusion_export_3mf`

Export active design as 3MF

**Args:** `outputPath` (required)

_Prerequisite: Active 3D design_


### `fusion_export_dwg`

Export active design as DWG

**Args:** `outputPath` (required)

_Prerequisite: Active 3D design_


### `fusion_export_dxf`

Export active design as DXF

**Args:** `outputPath` (required)

_Prerequisite: Active 3D design_


### `fusion_export_f3d`

Export active design as Fusion 360 archive (.f3d)

**Args:** `outputPath` (required)

_Prerequisite: Active 3D design_


### `fusion_export_fbx`

Export active design as FBX

**Args:** `outputPath` (required)

_Prerequisite: Active 3D design_


### `fusion_export_iges`

Export active design as IGES (.igs)

**Args:** `outputPath` (required)

_Prerequisite: Active 3D design_


### `fusion_export_obj`

Export active design as OBJ

**Args:** `outputPath` (required)

_Prerequisite: Active 3D design_


### `fusion_export_sat`

Export active design as SAT (.sat, ACIS solid model)

**Args:** `outputPath` (required)

_Prerequisite: Active 3D design_


### `fusion_export_skp`

Export active design as SKP (SketchUp)

**Args:** `outputPath` (required)

_Prerequisite: Active 3D design_


### `fusion_export_step`

Export active design as STEP

**Args:** `outputPath` (required)

_Prerequisite: Active 3D design (not Electronics)_

```
adom-bridge fusion_export_step '{"outputPath":"C:/tmp/board.step"}'
-> { "format": "step", "fileSizeKB": 8890 } // requires the 3D view active (fusion_show_3d_board)
```


### `fusion_export_stl`

Export active design as STL

**Args:** `outputPath` (required)

_Prerequisite: Active 3D design_


### `fusion_export_usdz`

Export active design as USDZ (best for digital twins + GLB)

**Args:** `outputPath` (required)

_Prerequisite: Active 3D design_


### `fusion_get_design_info`

Get active design metadata (bodies, components)

_Prerequisite: Active 3D design_


### `fusion_get_parameters`

List user parameters from active design


### `fusion_import_step`

Import STEP/STL/IGES/SAT/OBJ/F3D file into Fusion

**Args:** `filePath` (required Windows path)


### `fusion_set_parameter`

Modify a user parameter

**Args:** `name` (required), `value` (required)


### `fusion_take_screenshot`

Capture Fusion viewport as PNG with camera orientation control

**Args:** `height` (optional (default 1080)), `orientation` (optional: home, front, back, top, bottom, left, right), `outputPath` (required), `width` (optional (default 1920))



---

## Manufacturing: gerbers, BOM, CPL, design rules

### `fusion_detect_layers`

Auto-detect PCB copper layer count (2 or 4) using ULP + CAM comparison


### `fusion_export_board_image`

Export PNG image with layer presets (top_copper, assembly_top, fabrication, etc.)

**Args:** `dpi` (optional int (default: 300, max: 600)), `layers` (optional int[] - custom layer numbers (overrides preset)), `listPresets` (optional bool - returns preset list instead of exporting), `monochrome` (optional bool), `outputPath` (optional (default: C:/tmp/adom-board.png)), `preset` (optional: all|top_copper|bottom_copper|top_silkscreen|bottom_silkscreen|top_soldermask|bot)


### `fusion_export_bom`

Export Bill of Materials as CSV (grouped by value+package)

**Args:** `grouped` (optional bool (default: true) - false for one row per component), `outputPath` (optional (default: C:/tmp/adom-bom.csv))

```
adom-bridge fusion_export_bom '{"outputPath":"C:/tmp/bom.csv"}'
-> { "componentCount": 13, "grouped": true } // CSV: Comment,Designator,Footprint,Quantity,Library
```


### `fusion_export_cpl`

Export Component Placement List (pick-and-place) as CSV

**Args:** `outputPath` (optional (default: C:/tmp/adom-cpl.csv)), `side` (optional: all (default) | top | bottom)

```
adom-bridge fusion_export_cpl '{"outputPath":"C:/tmp/cpl.csv"}'
-> { "topCount": 12, "bottomCount": 1 } // CSV: Designator,Mid X,Mid Y,Layer,Rotation
```


### `fusion_export_gerbers`

Export Gerber RS-274X + Excellon drill files as ZIP (auto-detects layers, selects CAM job)

**Args:** `boardName` (optional (default: from active doc name)), `layers` (optional: auto | 2 | 4), `outputDir` (optional (default: C:/tmp/adom-gerbers/))

```
adom-bridge fusion_export_gerbers '{}'
-> { "boardName": "LEDStrip_WS2812B-BT_v12", "fileCount": 11, "files": [ {"name":"...GTL","size":24922}, {"name":"...GBL"}, ... {"name":"...XLN"}, {"name":"gerber_job.gbrjob"} ] }
```


### `fusion_set_design_rules`

Apply Adom/JLCPCB design rules (.edru files) - auto-detects 2/4 layer

**Args:** `action` (optional: apply (default) | export | show), `layers` (optional: auto (default) | 2 | 4), `outputPath` (optional, for export action)

```
adom-bridge fusion_set_design_rules '{"action":"apply","layers":"2"}'
-> { "activeWorkspace": "PCB Editor", "description": "board thickness 1.6mm, 1oz copper, ... (JLCPCB profile)" }
```



---

## UI interaction

### `fusion_click_fusion`

Click at coordinates in Fusion window or a specific dialog

**Args:** `hwnd` (optional - target a specific dialog HWND instead of main window), `relative` (optional bool (default: true, 0.0-1.0)), `x` (required float), `y` (required float)


### `fusion_screenshot_all`

Screenshot main window + list all dialog windows with HWNDs


### `fusion_screenshot_fusion`

Capture Fusion main window or a specific dialog as PNG

**Args:** `hwnd` (optional - dialog HWND to screenshot instead of main window)


### `fusion_send_key`

Send keyboard input to Fusion or a specific dialog

**Args:** `hwnd` (optional - target a specific dialog HWND), `key` (required string)


### `fusion_window_info`

Get main window HWND, title, rect, and all Qt dialog windows



---

## Recovery

### `fusion_dismiss_recovery`

Dismiss recovery dialogs + relocate recovery files to ~/.adom/recovery/


### `fusion_relocate_recovery`

Move crash recovery files to ~/.adom/recovery/fusion/ (call before launch to prevent dialogs)



---

_68 verbs. Generated from the live Adom Bridge verb catalog; headline verbs show real captured output._
