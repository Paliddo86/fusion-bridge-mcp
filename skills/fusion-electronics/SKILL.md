---
name: fusion-electronics
description: How to drive Fusion 360 Electronics (the EAGLE-based schematic + PCB editor) through the Adom Fusion bridge - the file hierarchy (open the PROJECT, never the schematic/.brd/3D), switching between schematic / 2D board / 3D PCB views, and moving the editor for demos and screen recordings (zoom, pan, frame a region, orbit the 3D board, select parts, surface the properties panel). Read before opening or recording any PCB. Trigger words - fusion electronics, fusion pcb, open the board, open the schematic, fusion_show_2d_board, fusion_show_3d_board, eagle editor, zoom the board, pan the board, orbit the 3d pcb, select a part, properties panel, record the schematic, record the board, board out of date, empty 2d board, EcadDesignProductType, parent child electronics design.
---

# Fusion Electronics - drive the schematic, board, and 3D PCB

> **Opening or creating a schematic: `fusion_schematic`.** `{action:"open", fileName|urn|query}` opens a
> cloud design and switches to its schematic; `{action:"open", schPath}` opens a local EAGLE .sch (with
> its .brd when one sits beside it, so parts instantiate); `{action:"create", name}` writes a blank
> schematic and opens it as a new local design with no dialog; `{action:"show"}` switches the open
> design to the schematic view. The reply's `via` names the verb it used. To put a LIBRARY'S PARTS on
> a new schematic and board (a demo board, "show these sensors"), `fusion_make_showcase_project
> {libraryPath, devicesets}` places them all, with their bound 3D bodies, dialog-free. Everything
> below is the detail behind that front door. `fusion_new_electronics_from_eagle` and the showcase are
> async: they return a `runId`; poll `fusion_eagle_import_status {runId}` for stage, document names,
> `partsExpected` and `partsPlaced`. `fusion_show_attributes {libraryPath, deviceset}` returns a
> component's metadata and shows its device editor.

## 1. ALWAYS open the PROJECT, never a child file

A Fusion electronics design is NOT one file. It is a parent/child chain of separate files, each a
distinct `productType`:

```
PROJECT        EcadDesignProductType    <- OPEN THIS ONE
  |- schematic   SchematicProductType
  |- board/.brd  BoardProductType
  |- 3D PCB      DesignProductType        (generated FROM the .brd - the LEAF; opening it gives NO editable board)
```

- Open the PROJECT with `fusion_open_cloud_file` / `fusion_open_by_urn`. Then the schematic and board
  are children you switch between - they are VIEWS of the one design, not separate things to open.
- **Opening a schematic / .brd / 3D directly** gives an isolated or empty view, and you cannot derive
  the board from the 3D (it is the leaf). The open verbs **reprimand you in the response `_hint`** when
  the opened file is not `EcadDesignProductType` - read it and reopen the project.
- Same-named copies in OTHER projects (e.g. a drone assembly that imported the board) are usually
  3D-model derivatives (`DesignProductType`), not the project. Do not open those.
- An EMPTY 2D board (`fusion_board_info` returns 0 elements) almost always means you opened the wrong
  file. Reopen the parent PROJECT and switch to its board.

## 2. Switch between the views

The design is open; switch the LIVE editor between its three views with the symmetric trio:
- `fusion_show_schematic` - the schematic (circuit diagram). Uses Fusion's `SwitchSchDocCmd`.
- `fusion_show_2d_board` - the 2D PCB Editor (board layout / copper). `SwitchPcbDocCmd`.
- All three show verbs take `documentName`: with more than one tab open (a board plus its library is the
  usual pair) pass it and the bridge activates that document first. Without it they act on the ACTIVE
  document and answer `not_an_electronics_design` plus `openDocuments` when a library or 3D design is in front.
- An open electronics design is several tabs with ONE name (3D design, board, schematic), all typed
  unknown, so `fusion_activate_document {name}` lands on one of them. Reach a view with the show verbs.
  The export verbs switch for you: a `.sch`/`.fsch` export moves to the Schematic Editor first and a
  `.brd`/`.fbrd` export to the PCB Editor (reply `switchedView`), since 1.10.35.
- `fusion_show_3d_board` - the populated 3D PCB. `Switch3dPcbDocCmd`.

All three move the VISIBLE editor (not just the API's active doc), so they're what you use to set up a
recording of a given view. Schematic queries also run via the `Electron.sch_*` text commands. These are
VIEWS of the open project - if a view is empty, you opened a child/derivative (see section 1).

## 3. Move the editor for demos / screen recordings

Give a recording "life" with these FIRST-CLASS verbs. Each runs its WHOLE motion inside the add-in -
one call = one smooth move, a repaint per frame (the 2D analogue of `vp.refresh()`):

### Zoom (schematic / 2D board)
`fusion_electron_zoom {factor, steps?, frameDelayMs?}` - `factor>1` zooms in, `<1` out; or `{fit:true}`.
  e.g. `adom-bridge fusion_electron_zoom '{"factor":3,"steps":20}'` (smooth zoom in over 20 frames).

### Pan + frame a region (or a part)
`fusion_electron_pan {x1,y1,x2,y2, steps?, frameDelayMs?}` - glide+zoom to a board-coordinate box (mm).
Animates from where the view last was. Get part coordinates from `fusion_board_info` or the CPL export
(`fusion_export_cpl` gives every part's Mid X / Mid Y) - frame a small box around a part to "go to" it.
  e.g. `adom-bridge fusion_electron_pan '{"x1":10,"y1":15,"x2":22,"y2":38,"steps":16}'`.

### Select a part + surface its properties
`fusion_electron_select {name}` (reference designator) or `{x,y}` (board mm) - selects the part the way
clicking it reveals the properties panel, and returns its device info. (Select highlights; pair with
`fusion_electron_pan` to also center on it - the "select chip -> zoom to it" tour.)
  e.g. `adom-bridge fusion_electron_select '{"name":"U1"}'`.

### Orbit / zoom the 3D PCB (Fusion camera)
`fusion_show_3d_board`, then `fusion_run_modeling_script` with a camera loop: `app.activeViewport.camera`,
step the eye around the target in azimuth (vary radius to zoom), `cam.isSmoothTransition=False`,
`vp.camera=cam`, **`vp.refresh()`** + `adsk.doEvents()` + a short `time.sleep()` each frame. The
`vp.refresh()` is what makes the in-between frames render.

### Lower-level building blocks (what the verbs above wrap)
`fusion_electron_run` runs raw EAGLE: `WINDOW FIT`, `WINDOW <factor>`, `WINDOW (x1 y1)(x2 y2)`, `DISPLAY`,
`RATSNEST`, `GRID`. `fusion_execute_text_command` runs `Electron.*`: `SelectbyName`, `LeftButtonDown` /
`Select` x y (+ `ContinueSelect` / `EndSelect` for a marquee), `ImmediateRepaint /full`,
`sch_get_deviceinfo`. Reach for these only when a first-class verb doesn't cover what you need.

### Capture without disrupting the user
Record with WGC (`desktop_record_window_start`) - background, no focus steal, and it grabs the LIVE EAGLE
GL canvas. PrintWindow (`desktop_screenshot_window`) shows the chrome but does NOT reflect the GL view
changing, so verify motion by the verb's `ok` + a WGC frame, not a PrintWindow pixel-diff. A black canvas
means an empty/wrong file - see section 1.

## Gotcha: file paths WITH SPACES (fixed in v1.6.90 - issue #196)

`executeTextCommand` is a **space-delimited** command string, so `Document.newDesignFromLocal`
used to receive an unquoted path and split it at the first space:

```
C:/Users/drew/Downloads/e2e-pmcoil/PM COIL Molecule.brd
                                   ^ Fusion only ever saw ".../PM"
```

(A Windows example - the same split happens on macOS with `/Users/<user>/Downloads/PM COIL Molecule.brd`;
`_open_local_document()` handles both the same way.)

The open then **silently no-op'd** - no dialog, no document, just the generic "Fusion 360 may not
support this file" hint - which sent the reporting agent hunting for a file-format problem when it
was pure path handling. (Caught by Drew's agent: the identical file opened instantly once copied to
a space-free name.)

Fixed in `_open_local_document()` (`commands/open_electronics_file.py`), now used by
`fusion_open_board`, `fusion_open_schematic`, `fusion_import_electronics` and `fusion_open_lbr`:

1. **Quote the path**, then
2. if that fails and the path has spaces, open a **staged copy**: the file and its same-stem
   siblings (.sch with .brd, .lbr with .step) are copied into a space-free folder and Fusion is
   handed the copy. Never the 8.3 short path: Fusion titles the document after the path it is
   given, and a name like `0603WA~1` broke every document-name check (bridge 1.10.11).

**If you are writing a new verb that hands a path to `executeTextCommand`, use that helper.** Never
interpolate a bare path into a text command; assume every user path has spaces in it, because
Downloads and OneDrive folders routinely do.

## Design rules by fab (1.9.352+): `fusion_set_design_rules {fab, process?, layers}`

The rule packs live on the wiki page **adom/pcb-design-rules** (13 vendors, one `.edru` per
vendor/process/layer count, every number citing the vendor's own capabilities page and the
date it was read). The bridge fetches the pack, stages it under `%TEMP%\adom-fusion\design-rules`
(`$TMPDIR/adom-fusion/design-rules` on macOS),
and the add-in runs `drc load <path>`; a rule change is a wiki publish, never a bridge release.

```jsonc
fusion_set_design_rules {"action": "list"}                       // every (fab, process, layers) row
fusion_set_design_rules {"fab": "jlcpcb", "layers": "2"}         // standard tier; layers auto-detects when omitted
fusion_set_design_rules {"fab": "pcbway", "process": "advanced", "layers": "4"}
fusion_set_design_rules {"druPath": "C:/Users/me/my-fab.edru"}   // a local file, as-is; macOS: "/Users/<user>/my-fab.edru"
fusion_set_design_rules {}                                       // the bundled 3rd party fab rules (unchanged)
```

Read `ruleWarnings` in the apply reply (1.10.3+): it names any rule in the downloaded profile
that native DRC will report at every ordinary trace joint (a Copper Clearance with
samesignal="yes" over an all/all or trace scope). Such warnings are rule noise, not copper
defects; a different-signal overlap stays an ERROR. Also read the loaded values back with
`{"action":"export"}` rather than trusting the command reply; the profile was generated on
adom/pcb-design-rules and may lag a vendor change.

- A board must be open in the PCB Editor (`fusion_show_2d_board` first); otherwise the add-in
  answers "Not in Board Layout" and nothing changes.
- **Verify, do not assume.** `fusion_alerts {sinceSec:60}` must be empty afterwards: a refused
  `.edru` is an in-canvas toast, never a dialog. For a hard proof, `{action:"export",
  outputPath}` writes what Fusion now holds; the exported `mdWireWire` / `msWidth` /
  `msDrill` equal the pack's (0.1 mm = 3.94 mil, 0.15 mm = 5.91 mil for JLCPCB 2L). Proven
  live on ConfRoomROG 2026-09-03.
- `errorCode design_rules_not_found` carries `available` (the vendor's processes and layer
  counts); `design_rules_not_generated` means the page has the vendor's record but no usable
  `.edru` for that tier (the vendor publishes no limits for it).
- Missing vendor or wrong limit? Fix it on the wiki page (`rules/records/<vendor>.json`, then
  `python3 tools/generate.py`), not in the bridge.


## The typed read model: `fusion_electronics_query` (1.9.359+)

One verb, one row shape, over the ACTIVE electronics document:

```jsonc
fusion_electronics_query {"entity":"Element","fields":["name","x","y","angle"]}
fusion_electronics_query {"entity":"Signal","filters":[{"property":"name","op":"eq","value":"GND"}]}
fusion_electronics_query {"entity":"Layer","filters":[{"property":"number","op":"lt","value":20}],"limit":50}
```

- `entity` is any of Autodesk's 49 electronics classes (Element, Signal, Net, Layer, Via, Smd, Pad,
  Wire, Part, Instance, Pin, Sheet, Package, Package3D, Library, Error, Variant, NetClass...), with
  or without the `electronics.` prefix; an unknown name answers the full list.
- `source: "mcp"` is Autodesk's own `electronics_read` (all classes, `coordinateUnit` per document,
  `hasMore` paging). It needs the Fusion MCP server on: `fusion_mcp_enable` (about 10 s, Fusion
  running). Board classes want the board in front, schematic classes the schematic.
- `source: "board"` is the fallback when the server is off: Board, Element, Library, Package, Layer,
  Signal, Wire and Error from the `board_info` geometry export, coordinates in mm.
- `source: "auto"` (default) tries the server first and falls back for the eight board classes.

Filters are `{property, op: eq|lt|gt, value}` (strings: `eq` only); `fields` projects; `limit`/`offset`
page. This replaces reading `fusion_board_info` XML by hand.
