# Fusion bridge - verb reference

Every `fusion_*` verb this bridge exposes, generated from the bridge's own self-describe
(`describe.py` / `fusion_describe`) so it never drifts. Call as:

```
adom-bridge fusion_<verb> '<input json>'
```

`input` / `output` below are the SCHEMA (field -> type/notes); `example` is a real input.
`timeoutSeconds` is ab's wait; `longRunning` verbs keep working after a timeout - poll `statusVerb`.

## Verbs by group


**Lifecycle:** `fusion_start` `fusion_stop` `fusion_kill` `fusion_close_window` `fusion_get_app_state` 
**Cloud search & files (APS):** `fusion_aps_status` `fusion_aps_set_client_id` `fusion_aps_signin` `fusion_aps_search` `fusion_aps_open` `fusion_open_by_urn` `fusion_aps_browse` `fusion_aps_recent` `fusion_aps_file_info` `fusion_aps_versions` `fusion_aps_download` `fusion_aps_upload` `fusion_aps_create_folder` `fusion_aps_get` `fusion_walk_cloud_tree` `fusion_search_cloud_files` 
**Electronics:** `fusion_show_schematic` `fusion_show_2d_board` `fusion_show_3d_board` `fusion_board_info` `fusion_open_lbr` `fusion_run_modeling_script` 
**CAD & 3D export:** `fusion_export_step` `fusion_export_iges` `fusion_export_stl` `fusion_export_3mf` `fusion_export_usdz` `fusion_export_obj` `fusion_export_dxf` `fusion_export_dwg` 
**Manufacturing:** `fusion_export_gerbers` `fusion_export_bom` `fusion_export_cpl` `fusion_set_design_rules` `fusion_load_design_rules` `fusion_detect_layers` `fusion_dismiss_blocking_dialogs` `fusion_electron_run` `fusion_electron_zoom` `fusion_electron_pan` `fusion_electron_select` 
**Self-describe:** `fusion_describe` 

---

## Lifecycle

### `fusion_start`

Launch Fusion 360 (if needed) and wait for the AdomBridge add-in to be ready.

**Input** `{}`

**Example call:**

```
adom-bridge fusion_start '{}'
```

**Output** (shape):

```json
{
  "success": "bool",
  "addinReady": "bool",
  "status": "str"
}
```

_timeout 150s · long-running (poll `fusion_get_app_state`)_

### `fusion_stop`

Gracefully STOP Fusion 360: close docs cleanly + WM_CLOSE, then wait. NO force-kill - the clean opposite of fusion_start. If a window will not close it returns failure; use fusion_kill then.

**Input** (schema):

```json
{
  "skipCleanClose": "optional bool"
}
```

**Example call:**

```
adom-bridge fusion_stop '{}'
```

**Output** (shape):

```json
{
  "success": "bool",
  "output": "str"
}
```

_timeout 60s_

### `fusion_kill`

Force-KILL Fusion 360 (taskkill /F) - the desperate path for when fusion_stop cannot close it (stuck modal / wedged process). Closes docs cleanly first when reachable.

**Input** (schema):

```json
{
  "skipCleanClose": "optional bool"
}
```

**Example call:**

```
adom-bridge fusion_kill '{}'
```

**Output** (shape):

```json
{
  "success": "bool",
  "output": "str"
}
```

_timeout 30s_

### `fusion_close_window`

Close the active Fusion document/window.

**Input** (schema):

```json
{
  "name": "optional str (default: active)"
}
```

**Example call:**

```
adom-bridge fusion_close_window '{}'
```

**Output** (shape):

```json
{
  "success": "bool"
}
```

_timeout 30s_

### `fusion_get_app_state`

Current app state: running, active document, workspace, isElectronics. The poll/status verb for long-running calls.

**Input** `{}`

**Example call:**

```
adom-bridge fusion_get_app_state '{}'
```

**Output** (shape):

```json
{
  "activeDocument": "str",
  "activeWorkspace": "str",
  "isElectronics": "bool"
}
```

_timeout 20s_

## Cloud search & files (APS)

### `fusion_aps_status`

APS state: configured, signed in, token live, never-charge meter.

**Input** `{}`

**Example call:**

```
adom-bridge fusion_aps_status '{}'
```

**Output** (shape):

```json
{
  "configured": "bool",
  "signedIn": "bool",
  "neverCharge": "bool"
}
```

_timeout 20s_

### `fusion_aps_set_client_id`

Set the org's APS (PKCE) client id used for cloud search.

**Input** (schema):

```json
{
  "clientId": "required str"
}
```

**Example call:**

```
adom-bridge fusion_aps_set_client_id '{"clientId": "D5nn...EPAq"}'
```

**Output** (shape):

```json
{
  "success": "bool"
}
```

_timeout 20s_

### `fusion_aps_signin`

Open the Autodesk sign-in in the browser; a background listener captures the token.

**Input** `{}`

**Example call:**

```
adom-bridge fusion_aps_signin '{}'
```

**Output** (shape):

```json
{
  "success": "bool",
  "authUrl": "str"
}
```

_timeout 30s_

### `fusion_aps_search`

Server-indexed search across the whole team hub (seconds, not a 30-min folder walk).

**Input** (schema):

```json
{
  "query": "required str",
  "limit": "optional int (default 50)",
  "hubId": "optional str",
  "projectId": "optional str"
}
```

**Example call:**

```
adom-bridge fusion_aps_search '{"query": "Molecule", "limit": 10}'
```

**Output** (shape):

```json
{
  "count": "int",
  "results": "[{name,id,projectName,lastModified}]"
}
```

_timeout 60s_

### `fusion_aps_open`

Find a cloud design by name and open it in Fusion (any folder depth). Fire-and-poll.

**Input** (schema):

```json
{
  "query": "required str",
  "wait": "optional bool"
}
```

**Example call:**

```
adom-bridge fusion_aps_open '{"query": "BME690 Molecule"}'
```

**Output** (shape):

```json
{
  "opening": "bool",
  "match": "{name}",
  "statusVerb": "str"
}
```

_timeout 60s · long-running (poll `fusion_get_app_state`)_

### `fusion_open_by_urn`

Open a cloud ELECTRONICS DESIGN by URN. PCB/ELECTRONICS NAV RULE: a board is NOT a standalone file - the schematic, 2D board and 3D board are VIEWS inside ONE electronics DESIGN. Open the DESIGN document first (fusion_open_cloud_file or fusion_open_by_urn), THEN switch to its schematic and its 2D board (fusion_show_2d_board) and 3D board (fusion_show_3d_board). A Select-Electronics-Design-File picker IS that design list of its schematic + board. An EMPTY 2D board usually means you opened a derivative or the wrong file, or it is out of sync - reopen the parent electronics DESIGN and switch to its board view; do NOT open a .brd or .sch as a standalone file.

**Input** (schema):

```json
{
  "urn": "required str"
}
```

**Example call:**

```
adom-bridge fusion_open_by_urn '{"urn": "urn:adsk.wipprod:fs.file:vf.xxxx?version=1"}'
```

**Output** (shape):

```json
{
  "success": "bool",
  "output": "str"
}
```

_timeout 240s · long-running (poll `fusion_get_app_state`)_

### `fusion_aps_browse`

Browse hubs/projects/folders without launching Fusion.

**Input** (schema):

```json
{
  "hubId": "optional str",
  "projectId": "optional str",
  "folderId": "optional str"
}
```

**Example call:**

```
adom-bridge fusion_aps_browse '{}'
```

**Output** (shape):

```json
{
  "items": "[{name,type,id}]"
}
```

_timeout 60s_

### `fusion_aps_recent`

Most-recently-modified designs across the team hub, newest first.

**Input** (schema):

```json
{
  "limit": "optional int (default 25)"
}
```

**Example call:**

```
adom-bridge fusion_aps_recent '{"limit": 25}'
```

**Output** (shape):

```json
{
  "count": "int",
  "results": "[...]"
}
```

_timeout 60s_

### `fusion_aps_file_info`

Version history + who/when/size for a file (tip + recent versions).

**Input** (schema):

```json
{
  "query": "str  OR  projectId+urn"
}
```

**Example call:**

```
adom-bridge fusion_aps_file_info '{"query": "BME690 Molecule"}'
```

**Output** (shape):

```json
{
  "versionCount": "int",
  "tip": "{...}",
  "versions": "[...]"
}
```

_timeout 40s_

### `fusion_aps_versions`

Full (uncapped) version history of a file.

**Input** (schema):

```json
{
  "query": "str  OR  projectId+urn"
}
```

**Example call:**

```
adom-bridge fusion_aps_versions '{"query": "BME690 Molecule"}'
```

**Output** (shape):

```json
{
  "versionCount": "int",
  "versions": "[...]"
}
```

_timeout 40s_

### `fusion_aps_download`

Download a cloud file to the local machine (no Fusion needed).

**Input** (schema):

```json
{
  "query": "str OR projectId+urn",
  "saveDir": "optional str",
  "fileName": "optional str"
}
```

**Example call:**

```
adom-bridge fusion_aps_download '{"query": "BME690 Molecule"}'
```

**Output** (shape):

```json
{
  "path": "str",
  "bytes": "int"
}
```

_timeout 90s_

### `fusion_aps_upload`

Upload a local file as a NEW cloud file (free Data Management; needs data:write scope).

**Input** (schema):

```json
{
  "projectId": "required str",
  "folderId": "required str",
  "localPath": "required str",
  "fileName": "optional str"
}
```

**Example call:**

```
adom-bridge fusion_aps_upload '{"projectId": "a.YnVz...", "folderId": "urn:...", "localPath": "C:/x.f3d"}'
```

**Output** (shape):

```json
{
  "itemUrn": "str",
  "bytes": "int"
}
```

_timeout 90s_

### `fusion_aps_create_folder`

Create a subfolder in a project (free; needs data:create scope).

**Input** (schema):

```json
{
  "projectId": "required str",
  "parentFolderId": "required str",
  "name": "required str"
}
```

**Example call:**

```
adom-bridge fusion_aps_create_folder '{"projectId": "a.YnVz...", "parentFolderId": "urn:...", "name": "new-folder"}'
```

**Output** (shape):

```json
{
  "folderId": "str"
}
```

_timeout 50s_

### `fusion_aps_get`

Raw authenticated GET against a free APS path (dev/verify helper).

**Input** (schema):

```json
{
  "path": "required str"
}
```

**Example call:**

```
adom-bridge fusion_aps_get '{"path": "/project/v1/hubs"}'
```

**Output** (shape):

```json
{
  "status": "int",
  "body": "obj"
}
```

_timeout 40s_

### `fusion_walk_cloud_tree`

DO NOT USE: slow in-app add-in walk (30+ min) that CAN CRASH Fusion (WinError 10054). Use fusion_aps_search instead; see the 'fusion-aps-search' skill.

**Input** `{}`

**Example call:**

```
adom-bridge fusion_walk_cloud_tree '{}'
```

**Output** (shape):

```json
{
  "tree": "obj"
}
```

_timeout 600s · long-running (poll `fusion_get_app_state`)_

### `fusion_search_cloud_files`

DO NOT USE: slow in-app add-in search (30+ min) that CAN CRASH Fusion (WinError 10054). Use fusion_aps_search instead; see the 'fusion-aps-search' skill.

**Input** (schema):

```json
{
  "query": "required str"
}
```

**Example call:**

```
adom-bridge fusion_search_cloud_files '{"query": "Molecule"}'
```

**Output** (shape):

```json
{
  "results": "[...]"
}
```

_timeout 180s · long-running (poll `fusion_get_app_state`)_

## Electronics

### `fusion_show_schematic`

Switch the OPEN electronics design to its SCHEMATIC view (the symmetric partner of fusion_show_2d_board / fusion_show_3d_board, via Fusion's SwitchSchDocCmd). A VIEW of the project, not a standalone file. Drive it for demos with fusion_electron_zoom / _pan / _select.

**Input** `{}`

**Example call:**

```
adom-bridge fusion_show_schematic '{}'
```

**Output** (shape):

```json
{
  "success": "bool",
  "activeWorkspace": "str",
  "productType": "str"
}
```

_timeout 70s · long-running (poll `fusion_get_app_state`)_

### `fusion_show_2d_board`

Switch the OPEN electronics design to its 2D PCB Editor (board layout) view. This is a VIEW of the project, not a file to open standalone - if the board is empty, you opened a child/derivative instead of the EcadDesignProductType PROJECT; reopen the project. Drive the view with fusion_electron_run WINDOW commands.

**Input** `{}`

**Example call:**

```
adom-bridge fusion_show_2d_board '{}'
```

**Output** (shape):

```json
{
  "activeWorkspace": "str"
}
```

_timeout 70s · long-running (poll `fusion_get_app_state`)_

### `fusion_show_3d_board`

Switch the OPEN electronics design to its 3D PCB view (populated board). A VIEW of the project, not a standalone file. Orbit it for demos via fusion_run_modeling_script (app.activeViewport.camera + vp.refresh()).

**Input** `{}`

**Example call:**

```
adom-bridge fusion_show_3d_board '{}'
```

**Output** (shape):

```json
{
  "activeWorkspace": "str",
  "screenshots": "[...]"
}
```

_timeout 70s · long-running (poll `fusion_get_app_state`)_

### `fusion_board_info`

Raw board geometry XML (copper/arcs/lines) for the open board.

**Input** `{}`

**Example call:**

```
adom-bridge fusion_board_info '{}'
```

**Output** (shape):

```json
{
  "rawXml": "str"
}
```

_timeout 60s_

### `fusion_open_lbr`

Import an EAGLE .lbr library into Fusion's Electronics library editor.

**Input** (schema):

```json
{
  "filePath": "required str (.lbr on the machine)",
  "symbolName": "optional str",
  "verify": "optional bool"
}
```

**Example call:**

```
adom-bridge fusion_open_lbr '{"filePath": "C:/libs/nRF52840-CKAA.lbr"}'
```

**Output** (shape):

```json
{
  "success": "bool",
  "postOpenScreenshot": "{savedTo}"
}
```

_timeout 50s_

### `fusion_run_modeling_script`

Run an adsk.fusion modeling script in the live session (sketches/extrudes/params). Free programmatic CAD.

**Input** (schema):

```json
{
  "script": "required str (Python using adsk; globals: adsk, app, ui, design)"
}
```

**Example call:**

```
adom-bridge fusion_run_modeling_script '{"script": "import adsk.fusion\n# ...build geometry..."}'
```

**Output** (shape):

```json
{
  "result": "obj",
  "documentName": "str"
}
```

_timeout 180s · long-running (poll `fusion_get_app_state`)_

## CAD & 3D export

### `fusion_export_step`

Export the active design to STEP (needs the 3D view).

**Input** (schema):

```json
{
  "outputPath": "required str"
}
```

**Example call:**

```
adom-bridge fusion_export_step '{"outputPath": "C:/out/board.step"}'
```

**Output** (shape):

```json
{
  "format": "str",
  "fileSizeKB": "int"
}
```

_timeout 300s · long-running (poll `fusion_get_app_state`)_

### `fusion_export_iges`

Export to IGES.

**Input** (schema):

```json
{
  "outputPath": "required str"
}
```

**Example call:**

```
adom-bridge fusion_export_iges '{"outputPath": "C:/out/board.iges"}'
```

**Output** (shape):

```json
{
  "format": "str"
}
```

_timeout 300s · long-running (poll `fusion_get_app_state`)_

### `fusion_export_stl`

Export to STL mesh.

**Input** (schema):

```json
{
  "outputPath": "required str"
}
```

**Example call:**

```
adom-bridge fusion_export_stl '{"outputPath": "C:/out/part.stl"}'
```

**Output** (shape):

```json
{
  "format": "str"
}
```

_timeout 300s · long-running (poll `fusion_get_app_state`)_

### `fusion_export_3mf`

Export to 3MF mesh.

**Input** (schema):

```json
{
  "outputPath": "required str"
}
```

**Example call:**

```
adom-bridge fusion_export_3mf '{"outputPath": "C:/out/part.3mf"}'
```

**Output** (shape):

```json
{
  "format": "str"
}
```

_timeout 300s · long-running (poll `fusion_get_app_state`)_

### `fusion_export_usdz`

Export to USDZ (AR / Hydrogen 3D).

**Input** (schema):

```json
{
  "outputPath": "required str"
}
```

**Example call:**

```
adom-bridge fusion_export_usdz '{"outputPath": "C:/out/part.usdz"}'
```

**Output** (shape):

```json
{
  "format": "str"
}
```

_timeout 300s · long-running (poll `fusion_get_app_state`)_

### `fusion_export_obj`

Export to OBJ mesh.

**Input** (schema):

```json
{
  "outputPath": "required str"
}
```

**Example call:**

```
adom-bridge fusion_export_obj '{"outputPath": "C:/out/part.obj"}'
```

**Output** (shape):

```json
{
  "format": "str"
}
```

_timeout 300s · long-running (poll `fusion_get_app_state`)_

### `fusion_export_dxf`

Export 2D DXF.

**Input** (schema):

```json
{
  "outputPath": "required str"
}
```

**Example call:**

```
adom-bridge fusion_export_dxf '{"outputPath": "C:/out/sketch.dxf"}'
```

**Output** (shape):

```json
{
  "format": "str"
}
```

_timeout 120s · long-running (poll `fusion_get_app_state`)_

### `fusion_export_dwg`

Export 2D DWG.

**Input** (schema):

```json
{
  "outputPath": "required str"
}
```

**Example call:**

```
adom-bridge fusion_export_dwg '{"outputPath": "C:/out/sketch.dwg"}'
```

**Output** (shape):

```json
{
  "format": "str"
}
```

_timeout 120s · long-running (poll `fusion_get_app_state`)_

## Manufacturing

### `fusion_export_gerbers`

Run the bundled JLCPCB CAM job (auto 2/4-layer) to produce the gerber + drill ZIP.

**Input** (schema):

```json
{
  "outputDir": "optional str (default C:/tmp/adom-gerbers)"
}
```

**Example call:**

```
adom-bridge fusion_export_gerbers '{"outputDir": "C:/tmp/jlc"}'
```

**Output** (shape):

```json
{
  "fileCount": "int",
  "files": "[{name,size}]",
  "zipPath": "str"
}
```

_timeout 180s · long-running (poll `fusion_get_app_state`)_

### `fusion_export_bom`

Export a Bill of Materials CSV (grouped by value+package) from the open board.

**Input** (schema):

```json
{
  "outputPath": "optional str",
  "grouped": "optional bool (default true)"
}
```

**Example call:**

```
adom-bridge fusion_export_bom '{"outputPath": "C:/tmp/bom.csv"}'
```

**Output** (shape):

```json
{
  "componentCount": "int",
  "grouped": "bool"
}
```

_timeout 60s_

### `fusion_export_cpl`

Export the Component Placement List (pick-and-place) CSV.

**Input** (schema):

```json
{
  "outputPath": "optional str",
  "side": "optional str (all|top|bottom)"
}
```

**Example call:**

```
adom-bridge fusion_export_cpl '{"outputPath": "C:/tmp/cpl.csv"}'
```

**Output** (shape):

```json
{
  "topCount": "int",
  "bottomCount": "int"
}
```

_timeout 60s_

### `fusion_set_design_rules`

Load manufacturing-grade DRC into the open board (JLCPCB 2/4-layer profiles).

**Input** (schema):

```json
{
  "action": "optional str (apply|export|show)",
  "layers": "optional str (auto|2|4)"
}
```

**Example call:**

```
adom-bridge fusion_set_design_rules '{"action": "apply", "layers": "2"}'
```

**Output** (shape):

```json
{
  "activeWorkspace": "str",
  "description": "str"
}
```

_timeout 90s_

### `fusion_load_design_rules`

Load a bundled .edru design-rule set into the board.

**Input** (schema):

```json
{
  "layers": "optional str (auto|2|4)"
}
```

**Example call:**

```
adom-bridge fusion_load_design_rules '{}'
```

**Output** (shape):

```json
{
  "success": "bool"
}
```

_timeout 90s_

### `fusion_detect_layers`

Detect 2-layer vs 4-layer board (ULP + CAM comparison).

**Input** `{}`

**Example call:**

```
adom-bridge fusion_detect_layers '{}'
```

**Output** (shape):

```json
{
  "layerCount": "int",
  "copperLayers": "[int]"
}
```

_timeout 90s_

### `fusion_dismiss_blocking_dialogs`

Detect + classify (and optionally dismiss) blocking Fusion dialogs.

**Input** `{}`

**Example call:**

```
adom-bridge fusion_dismiss_blocking_dialogs '{}'
```

**Output** (shape):

```json
{
  "dialogs": "[...]"
}
```

_timeout 30s_

### `fusion_electron_run`

Run an EAGLE/Electron command in the open board (the generic extension point, e.g. RUN <ulp>).

**Input** (schema):

```json
{
  "command": "required str"
}
```

**Example call:**

```
adom-bridge fusion_electron_run '{"command": "set confirm yes;RUN 'C:/tmp/jlcpcb_smta_exporter.ulp' 'C:/tmp/jlc'"}'
```

**Output** (shape):

```json
{
  "success": "bool",
  "output": "str"
}
```

_timeout 90s_

### `fusion_electron_zoom`

Smoothly ZOOM the schematic/2D board for a recording (the whole motion is one call). factor>1 zooms in, <1 out; or fit:true. The loop runs in the add-in with a repaint per frame.

**Input** (schema):

```json
{
  "factor": "float (default 2.0)",
  "fit": "optional bool",
  "steps": "optional int (default 16)",
  "frameDelayMs": "optional int (default 35)"
}
```

**Example call:**

```
adom-bridge fusion_electron_zoom '{"factor": 3, "steps": 20}'
```

**Output** (shape):

```json
{
  "success": "bool",
  "output": "str"
}
```

_timeout 60s · long-running (poll `fusion_get_app_state`)_

### `fusion_electron_pan`

Smoothly PAN + zoom the view to frame a board-coordinate box (mm; get part positions from fusion_board_info). Animates from where the view last was.

**Input** (schema):

```json
{
  "x1": "float",
  "y1": "float",
  "x2": "float",
  "y2": "float",
  "steps": "optional int (default 16)",
  "frameDelayMs": "optional int (default 35)"
}
```

**Example call:**

```
adom-bridge fusion_electron_pan '{"x1": 0, "y1": 0, "x2": 10, "y2": 8}'
```

**Output** (shape):

```json
{
  "success": "bool",
  "output": "str",
  "box": "[...]"
}
```

_timeout 60s · long-running (poll `fusion_get_app_state`)_

### `fusion_electron_select`

SELECT a part (by reference designator, or x/y board mm) and surface its properties - the way clicking a part reveals the properties panel. Great for showing life in a recording.

**Input** (schema):

```json
{
  "name": "str (refdes, e.g. R1)",
  "x": "optional float",
  "y": "optional float",
  "properties": "optional bool (default true)"
}
```

**Example call:**

```
adom-bridge fusion_electron_select '{"name": "U1"}'
```

**Output** (shape):

```json
{
  "success": "bool",
  "output": "str",
  "deviceInfo": "str"
}
```

_timeout 30s · long-running (poll `fusion_get_app_state`)_

## Self-describe

### `fusion_describe`

Self-describe every verb the bridge exposes (this list) for ab's Verbs tab + runner.

**Input** `{}`

**Example call:**

```
adom-bridge fusion_describe '{}'
```

**Output** (shape):

```json
{
  "success": "bool",
  "verbs": "[{name,summary,input,output,timeoutSeconds,statusVerb,longRunning,example}]"
}
```

_timeout 20s_
