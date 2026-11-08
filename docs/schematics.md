# Schematics

Opening, viewing and authoring Fusion Electronics schematics, including bringing legacy EAGLE
designs in so their parts instantiate.

## Start here: `fusion_schematic`

One verb, three actions, and it is the one to reach for first. A schematic is a view of an
electronics design, not a file of its own, so the verb takes whatever names the design and lands
you in the schematic editor.

```bash
adom-bridge fusion_schematic '{"action":"open","query":"BQ25792"}'                  # cloud design, by search
adom-bridge fusion_schematic '{"action":"open","fileName":"BME690","projectName":"Molecules"}'
adom-bridge fusion_schematic '{"action":"open","schPath":"C:/Users/me/adom-lib/MyBoard.sch"}'
adom-bridge fusion_schematic '{"action":"create","name":"scratch"}'                 # a new blank schematic
adom-bridge fusion_schematic '{"action":"show"}'                                    # the design already open
```

- `open` accepts a cloud `fileName` (with `projectName`), a `urn` or a `query`. It accepts a local
  EAGLE `schPath`: when a sibling `.brd` exists it is imported with the schematic so the parts
  instantiate, and `withBoard:false` opens the `.sch` alone, dialog-free. It accepts `filePath`
  for a Fusion-native `.fsch`.
- `create` writes a blank EAGLE schematic on the box and opens it as a new local electronics
  design with the schematic editor active. No dialog is drawn anywhere. The reply carries
  `documentName` and `schPath`.
- `show` switches the open design to its schematic view.
- Every reply says which verb did the work in `via`.

`open` lands on the electronics design. When a name search opens the design's 3D PCB or a
mechanical file instead (a `DesignProductType` with no schematic), the verb retries the exact
cloud name once. If that also misses, it answers `not_an_electronics_design` with the candidate
documents, instead of Fusion's bare "Please open or create an electronics design".

## Open the PROJECT, never the schematic

This is the mistake that costs the most time, so it leads:

```
PROJECT        EcadDesignProductType   <- open THIS
  |- schematic   SchematicProductType
  |- board/.brd  BoardProductType
  |- 3D PCB      DesignProductType
```

Opening the schematic child directly gives you an isolated view with no board relationship, and you
cannot pull the board back out of it. Same-named copies in *other* projects are usually 3D-model
derivatives (`DesignProductType`), not the project. The open verbs warn you through their
`_hint` when the file you opened looks like the 3D leaf of an electronics chain, or when you passed
`intent:"electronics"`. Opening a mechanical design on purpose is fine: `intent:"mechanical"`
confirms it.

```bash
adom-bridge fusion_aps_open '{"query":"BQ25792"}'    # resolves + opens the project
adom-bridge fusion_show_schematic '{}'               # switch to the schematic editor
```

`fusion_schematic {action:"open", query:"BQ25792"}` does both steps in one call.

Below is a real Adom board's schematic, the BME690 environmental-sensor Molecule, open in the Fusion
Electronics schematic editor after exactly those two calls. The parts are live and net-connected: the
BME690 sensor, its decoupling caps, the I2C pull-ups, and the JP1 breakout header, with the layer set
(Nets, Busses, Pins) on the right. This whole capture was taken with Fusion in the **background**, it
never came to the foreground.

![BME690 Molecule schematic in the Fusion Electronics schematic editor](fz-schematic.png)

## `fusion_show_schematic`

Switches the active electronics design into the schematic editor. Pairs with
[`fusion_show_2d_board`](board-layout-2d.md) and `fusion_show_3d_board`, which are the same idea for
the other two views.

Use `fusion_get_app_state` to confirm where you landed: it reports `activeDocument`,
`activeWorkspace` and `isElectronics`.

## A library's parts on a new schematic and board: `fusion_make_showcase_project`

[![One prompt: parts library to schematic, 2D board and 3D board, watch on YouTube](docs/img/forum/demo-library-to-3d-board.jpg)](https://youtu.be/Th_HwxCgleE)

*Demo: one prompt in Hydrogen makes a schematic and a board from a library, then shows the schematic, the 2D board and the 3D board.*

"Make a schematic and board to show off these three sensors" is one call:

```bash
adom-bridge fusion_make_showcase_project '{
  "libraryPath":"C:/Users/me/adom-lib/Adom_ToF.lbr",
  "devicesets":["TMF8829","VL53L8"],
  "name":"tof-showcase"
}'
adom-bridge fusion_eagle_import_status '{"runId":"..."}'
```

Every deviceset of the library (or the ones in `devicesets`) is placed in a row on a fresh sheet
and a fresh board, with no nets, and the library's bound 3D urns are carried across so the 3D PCB
shows the real bodies. The design is generated as EAGLE source on the box and opened dialog-free.
The verb is async: it returns a `runId` at once and you poll `fusion_eagle_import_status`. The two
documents are named `<name>-sch` and `<name>-brd`, the board opens last so it is active for the
2D and 3D views, and each view is fitted. Symbols are pitched by their own measured width, so wide
parts do not overlap. `fusion_make_test_project` is the one-part circuit variant.

### The verbatim-library finding

Fusion refuses a schematic whose library block was re-assembled piecewise from a Fusion-exported
`.lbr`. It says the file "has errors and cannot be opened" and instantiates nothing. The same
library block embedded verbatim, whole, opens with every part. So `fusion_make_showcase_project`
embeds the library verbatim in both the generated schematic and the board.
`fusion_make_test_project` still rebuilds the block from pieces and fails the same way on
Fusion-exported libraries. That is a known follow-up.

## Importing a legacy EAGLE schematic

`fusion_new_electronics_from_eagle` is the one dialog-driven verb in the bridge, because Autodesk
gives no API for a paired `.sch` plus `.brd` import. It is the only reliable way to author a Fusion
board from EAGLE source, and the reason is worth understanding:

- `Document.newDesignFromLocal` opens the schematic editor but does **not instantiate the parts**.
  You get an empty-looking design.
- The Fusion-native `.fsch` / `.fbrd` container is opaque binary you cannot author offline.

So this verb fires Fusion's own `ImportSCHAndBRDCmd` and drives both native Open dialogs (the `.sch`
then the `.brd`) **in the background** with UIA by accessible name, no foreground steal.

```bash
adom-bridge fusion_new_electronics_from_eagle '{
  "schPath":"C:/Users/me/adom-lib/MyBoard.sch",
  "brdPath":"C:/Users/me/adom-lib/MyBoard.brd"
}'
```

Stage both files onto Windows first with `send_files`. `brdPath` defaults to the sibling `.brd`.
Most callers should use `fusion_schematic {action:"open", schPath}`, which picks this route only
when a `.brd` exists next to the `.sch`.

**It is async by contract.** The call returns `{started:true, runId, statusVerb}` at once, and
`fusion_get_app_state` keeps answering while the import runs. Poll the run record:

```bash
adom-bridge fusion_eagle_import_status '{"runId":"..."}'
```

The record carries `stage` (started, done or failed), `documentName`, `partsExpected` (elements in
the `.brd`) and `partsPlaced` (read back from the board). Success needs parts on the board. A run
that placed nothing answers `nothing_instantiated` with the likely causes.

**It preflights, and it never leaves a dialog behind.** The verb refuses with
`eagle_files_missing` before it touches any dialog when either file is not on the Fusion machine.
It cancels a stale Open dialog left by an earlier attempt. When the Open dialog does not accept
the file, the bridge cancels the dialog itself and answers `open_dialog_stuck`. The reply says the
dialogs were driven.

**Paths with spaces work, and are never shortened.** Fusion's text command line is space-delimited
and rejects a quoted path, so a file under a path with a space is staged: it is copied with its
same-stem siblings (`.sch` and `.brd`) into a space-free folder, and Fusion is handed the copy. The
document keeps its real name. The bridge does not use Windows 8.3 short names, which made Fusion
title documents things like "0603WA~1". Paths without a space are untouched.

## Driving the schematic editor

Fusion Electronics is EAGLE underneath, so it takes EAGLE commands:

```bash
adom-bridge fusion_electron_run '{"command":"GRID MM 1;"}'
adom-bridge fusion_execute_text_command '{"command":"Commands.Start ..."}'
```

`fusion_execute_text_command` is **space-delimited**, which trips people up.

An explicit `SHOW <name>` is allowed through `fusion_electron_run`: it highlights and returns.
Only a bare `SHOW`, which waits for a click, stays blocked.

For clean captures there are view controls that do not disturb the user:
`fusion_electron_zoom`, `fusion_electron_pan`, `fusion_electron_select`.

## Getting a picture out

`fusion_take_screenshot` grabs the Fusion window. For a document-level image of the schematic,
switch to it with `fusion_show_schematic` first, then capture.

## Gotchas

- `isElectronics: false` in `fusion_get_app_state` means you are in the mechanical Design workspace.
  Electronics verbs will refuse; open the project first.
- If an open appears to do nothing, read the reply. The bridge reads any dialog in front of Fusion,
  expires it safely and runs the verb. A dialog that survives comes back as `blocked_by_dialog`
  with its text and button names, so you answer it by name (`fusion_dismiss_blocking_dialogs`).
- Every add-in verb accepts `expectDocument`. When the named document is not the active one the
  verb refuses with `wrong_document` instead of running against whatever is in front.
- The schematic and board are **separate documents in one project**. Saving one does not save the
  other.
