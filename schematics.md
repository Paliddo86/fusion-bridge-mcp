# Schematics

Opening, viewing and authoring Fusion Electronics schematics, including bringing legacy EAGLE
designs in so their parts instantiate.

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
derivatives (`DesignProductType`), not the project. The open verbs reprimand you through their
`_hint` when the file you opened is not an `EcadDesignProductType`.

```bash
adom-desktop fusion_aps_open '{"query":"BQ25792"}'    # resolves + opens the project
adom-desktop fusion_show_schematic '{}'               # switch to the schematic editor
```

## `fusion_show_schematic`

Switches the active electronics design into the schematic editor. Pairs with
[`fusion_show_2d_board`](board-layout-2d.md) and `fusion_show_3d_board`, which are the same idea for
the other two views.

Use `fusion_get_app_state` to confirm where you landed: it reports `activeDocument`,
`activeWorkspace` and `isElectronics`.

## Importing a legacy EAGLE schematic

`fusion_new_electronics_from_eagle` is the only reliable way to author a Fusion board from EAGLE
source, and the reason is worth understanding:

- `Document.newDesignFromLocal` opens the schematic editor but does **not instantiate the parts**.
  You get an empty-looking design.
- The Fusion-native `.fsch` / `.fbrd` container is opaque binary you cannot author offline.

So this verb fires Fusion's own `ImportSCHAndBRDCmd` and drives both native Open dialogs (the `.sch`
then the `.brd`) **in the background** with UIA by accessible name, no foreground steal.

```bash
adom-desktop fusion_new_electronics_from_eagle '{
  "schPath":"C:/Users/me/adom-lib/MyBoard.sch",
  "brdPath":"C:/Users/me/adom-lib/MyBoard.brd"
}'
```

Stage both files onto Windows first with `send_files`. `brdPath` defaults to the sibling `.brd`.

**Paths with spaces work.** They used to fail silently; the open verbs now quote the path, fall back
to the Windows 8.3 short name, and verify `activeDocument` changed rather than reporting a
no-op as success.

## Driving the schematic editor

Fusion Electronics is EAGLE underneath, so it takes EAGLE commands:

```bash
adom-desktop fusion_electron_run '{"command":"GRID MM 1;"}'
adom-desktop fusion_execute_text_command '{"command":"Commands.Start ..."}'
```

`fusion_execute_text_command` is **space-delimited**, which trips people up.

For clean captures there are view controls that do not disturb the user:
`fusion_electron_zoom`, `fusion_electron_pan`, `fusion_electron_select`.

## Getting a picture out

`fusion_take_screenshot` grabs the Fusion window. For a document-level image of the schematic,
switch to it with `fusion_show_schematic` first, then capture.

## Gotchas

- `isElectronics: false` in `fusion_get_app_state` means you are in the mechanical Design workspace.
  Electronics verbs will refuse; open the project first.
- If an open appears to do nothing, check `fusion_dismiss_blocking_dialogs`. A recovery prompt or an
  update nag can sit in front of the document and swallow the action.
- The schematic and board are **separate documents in one project**. Saving one does not save the
  other.
