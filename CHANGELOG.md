# Changelog

All notable changes to the Adom Bridge Fusion 360 bridge, newest first.
Canonical: https://wiki.adom.inc/adom/fusion-bridge · GitHub backup: `adom-inc/fusion-bridge`

## 1.13.84 (2026-11-06)

`render_board_3d` with `quality:"high"` really stops a render that runs past `renderSeconds` (found by Drew Owens on PR #40). It called `RenderFuture.cancel()`, which does not exist; the error was swallowed and the reply said "it was cancelled" while the render kept going and could write `outputPath` later. Now it ends that render's own RenderProcess.exe through the same `render_process.cancel` as `render_image` (embedded in the electronics script), renders into a staging folder and moves the image to `outputPath` only on success, so an existing image is no longer deleted first and a late image can never land there. The reply carries `cancel: {cancelled, renderState, reason}` and says plainly when a render could not be stopped. Exe only: no Fusion restart.

## 1.13.83 (2026-11-05)

`fusion_save_to_cloud` makes the missing folders on `folderPath` for a mechanical design too, as its description says: a save to `fusion-lane-tests/drawings` answered "Folder 'drawings' not found" when `drawings` did not exist yet. Add-in 1.0.41: Fusion needs one restart.

## 1.13.82 (2026-11-05)

Add-in 1.0.40, so a running Fusion is restarted onto 1.13.81's drawing fix (1.13.81 left the add-in at 1.0.39, and the bridge restarts Fusion for an add-in change by its version). Fusion needs one restart.

## 1.13.81 (2026-11-05)

`drawing_export` with `close:true` closes the drawing again: 1.13.80's review change left it open, because ending Edit Auto Dimensions marks the drawing modified. Only the command-name check from that review stays.

## 1.13.80 (2026-11-05)

2D drawings (PR #43 by Drew Owens). `drawing_create` makes Fusion's automatic drawing of a saved design (sheets, base, projected and isometric views, title block, automatic dimensions), with standard, units, sheet size, views, style and folder options, and refuses an unsaved or still-uploading design before any cloud write. `drawing_export` writes PDF, DWG and DXF (with the scale the layout chose, read from the DXF) over several calls: it opens the drawing visibly, ends Fusion's Edit Auto Dimensions command, and exports only when the drawing engine is idle, because an export sent too early blocks Fusion for 10 minutes. `drawing_status` reads readiness, sheets and views. It uses Fusion's preview drawing API and refuses with `unsupported_capability` on a Fusion without it. In review: only Fusion's own Edit Auto Dimensions command is ever ended. Add-in 1.0.39: Fusion needs one restart.

## 1.13.79 (2026-11-05)

Typed reads for assembly planning (PR #42 by Drew Owens). `assembly_layout` lists one scope's visible parts (path, name, component, grounded, children, a root-space box in mm) with the scope's joints, as-built joints and rigid groups as part paths; it is journaled. `ray_cast` casts batches of rays at named occurrences in their own coordinates and answers `[index, distance]` nearest first; it stays out of the journal, stops at `budgetSeconds` or 1 MiB, and says how many it `cast` so the caller sends the rest. A planner no longer needs raw modeling scripts for either. Add-in 1.0.38: Fusion needs one restart.

## 1.13.78 (2026-11-05)

CAM jobs come out as asked (PR #41 by Drew Owens). The FANUC program number and comment are the ones you give (every program was O1001), and the file lands in `post.folder` (a backslash path was ignored and the file went to Fusion's own NC folder; one that still does is moved and flagged `movedFromFusionFolder`). New strategies: 2D adaptive, 2D pocket, 2D contour, drill cycles (drill, peck, chip break with `peckDepthMm`) and pocket clearing. Setups take `stock` (relative box, fixed box or stock bodies) and `wcs`, read back in model mm. Tools come from any sample or local library or the document by type, description, flutes and nearest diameter; `cam_tools` filters by description and diameter. Every operation reports its real status (generated, empty, error, outdated, suppressed, not_generated) with Fusion's warnings and machining time, and the job does not post while an operation lacks a toolpath. A document without CAM opens Manufacture once and goes back to your workspace. Add-in 1.0.37: Fusion needs one restart.

## 1.13.77 (2026-11-05)

`render_image` chooses the lighting and backdrop, and a timed-out render really stops (PR #40 by Drew Owens). `environment` takes a Fusion environment by name or id, or a `.hdr`/`.exr` file; `background` is `"environment"` or a `#rrggbb` colour. At `timeoutSeconds` the bridge ends that render's own RenderProcess.exe (a child of this Fusion, started after the render, the only one), so it writes nothing late; the reply says `cancel: {cancelled, renderState, reason}`. The image is staged under %TEMP% and moved only on success, the scene and camera come back (`sceneRestored`), and size, quality and transparency now reach the render (they never did). Quality starts at 25. Add-in 1.0.36: Fusion needs one restart.

## 1.13.76 (2026-11-05)

`exploded_view_plan` plans big designs (PR #37 by Drew Owens): past 256 occurrences it plans the parts the steps move and their parents, where it used to refuse, up to 4,096 tracked occurrences. `exploded_view_render` follows, and the preview refuses a moved-scope plan explicitly.

`geometry_query`'s `near` measures bounded faces and edges in root space (PR #38 by Drew Owens): a coplanar face or collinear edge beside the pick read 0 mm and matched too. Add-in 1.0.35: Fusion needs one restart.

Clean captures (PR #39 by Drew Owens): `exploded_view_render` and `exploded_view_stills` hide joint, sketch and construction glyphs while they capture, and put back exactly what they turned off, without marking the document modified. `glyphs: true` keeps them. `render_image` hides them only while fitting a view.

## 1.13.75 (2026-11-05)

Mechanical writes work on designs that already carry faults (PR #36 by Drew Owens). A write is held to the faults present before it: it may not add one, change one, or leave one on what it edits in place, or it refuses with `unhealthy_model` and rolls back. A healthy design behaves as before. Replies carry `preExistingFaults`. An unsupported sketch relationship only refuses a write that names that sketch. Add-in 1.0.34: Fusion needs one restart.

## 1.13.74 (2026-11-05)

`fusion_remove_pours` could delete the wrong pour and answer `verified: true` when two pours on a layer share an edge (found by AI Flow): the pick landed on the shared edge, and the read back only counted pours per net and layer. It now picks a point on no other pour's outline, and reads back by id: if the wrong one went, it's undone and answered (`readback_mismatch`, `removedByMistake`). If every edge is shared, it refuses before deleting anything (`unsafe_pick`). `fusion_edit_pours` picks the same way, and undoes a change that touched another pour.

## 1.13.73 (2026-11-05)

`fusion_move_parts` keeps a part's side (adom-aiflow#142, Andy Engevold). A rotation given as a number took the mirror off, so a bottom part moved to the top. A number now keeps the side. To change it, give `side` (top or bottom) or `flip: true`, the standard's arguments, or a string that names it (`R90`, `MR90`); parts that change side are listed in `sideChanged`. If any part doesn't land, every move is undone (`undone`), where the parts already moved used to stay moved.

## 1.13.72 (2026-11-04)

`fusion_apply_silkscreen` moves and hides a part's name or value when the part is marked smashed but has no text of its own (#144, Glenn Jones). It trusted the flag, skipped SMASH, and MOVE and CHANGE then picked nothing.

## 1.13.71 (2026-11-04)

From Glenn Jones's reports on a 4-layer board:

`fusion_export_gerbers` takes the layer count from the board's own stack (#142). Inner planes that were only pours read as a 2-layer board, so the inner layers were left out while the reply said success. A count its CAM jobs don't cover is refused (`unsupported_layer_count`).

New `fusion_change_vias` (#145): drill and diameter of existing vias, by position or by a filter, with only the Vias layer displayed while picking, read back, undone on a mismatch.

A failed `fusion_apply_silkscreen` batch is really undone, free texts included, or says it isn't (#144). The undo stopped when the parts' names matched, which left the new texts on the board.

The "driving Fusion" panel comes down when the verb answers (#143). A second message while it was up stopped it ever closing.

A document with no visible tab no longer shifts the tab a board read switches to (#141). The read-back after an EAGLE import waits for the board to load (#139). The Select Library picker names a cloud copy as well as a file (#140).

## 1.13.70 (2026-11-04)

`fusion_build_library_3d` re-exports faster: a part already minted on this machine and account, with the same model, footprint, placement and destination, reuses its 3D package when its cloud file still exists, so only new or changed parts mint. Parts say `reused: true`; the reply has `partsReused` and `partsMinted`, and `freshMint: true` makes them again. A deleted cloud file falls back to a fresh mint. The last `_3d` scratch document is closed once the library is open (`scratchClosedAfterOpen`).

A library Fusion refuses is reported, not called opened (PR #35 by Colby Knox). `fusion_preflight_lbr` has a new check, `package3d-refs-resolve`, for a 3D binding naming a package the library doesn't have, and `fusion_open_lbr` refuses such a file before Fusion sees it. Any other refusal answers `success: false` with the dialog closed, and `fusion_build_library_3d` reports failure when the finished library won't open, even if every part minted.

## 1.13.69 (2026-11-04)

Fusion is no longer relaunched into a loop of popups. A call that finds Fusion closed used to start it every time, so a poller relaunched a broken install once a minute, a round of Windows error popups each time (Colby's PC, 2026-11-04). Now:
- A stripped install is seen from the disk and never launched: `fusion_install_broken`, naming the folder. `fusion_install_fusion` repairs it, where it used to say "already installed" because another copy was on disk.
- While Fusion's installer runs, calls answer `fusion_updating` rather than launching it.
- After two starts within 5 minutes that didn't stay up, calls stop starting it for 15 minutes (`fusion_autostart_paused`). `fusion_start` always tries.
- The ledger's `fusion_auto_started` says `ok` only if Fusion is really running afterwards.


Pours have a stable `id` (#138, Andy Engevold): `fusion_pour_state` and the board model give each one, made from its net, layer and outline, as Fusion stores none. It stays the same across saves and when other pours come and go. `fusion_remove_pours` and `fusion_edit_pours` take it, and an unknown one is refused before anything runs.

## 1.13.68 (2026-11-03)

`fusion_project_create` makes a design with its sheet and board linked (`linked: true`), through Fusion's own EAGLE import, so a part placed on the sheet goes on the board. It used to open two unlinked documents, which is now only the fallback when the import can't run (`linked: false`, `whyNotLinked`). If Adom Bridge is waiting for an OK, it answers `stillRunning` with a `runId`.

A script that opens a design and meets Fusion's "will be opened with warnings" notice no longer hangs until the call times out: the bridge answers the notice with OK and says so (`noticesAnswered`). From AI Flow.

Add-in 1.0.33: its text no longer names the retired `fusion_send_key` and `fusion_click_fusion`. Text only; Fusion needs one restart to load it.

## 1.13.67 (2026-11-03)

The `fusion_open_board` and `fusion_open_schematic` refusal handling from 1.13.66 now runs on the path those verbs really take: in 1.13.66 the modal still hung the call.

## 1.13.66 (2026-11-03)

`fusion_open_board` no longer reports success on a board Fusion refuses. A file with a part whose library or package isn't in it, or a net naming a part that isn't, is refused before Fusion sees it (`board_file_has_errors`, naming each problem), so no dialog is drawn. If Fusion refuses a board or schematic for another reason, the bridge closes its dialog and the empty design and answers `file_refused_by_fusion`.

The Select Library picker no longer blocks Fusion: when it comes up during `fusion_schematic_run`, `fusion_place_parts`, `fusion_sch_place_part` or `fusion_sch_swap_part`, the bridge closes it and answers `library_name_in_use`, naming both files (`librariesInUse`). Answering it wouldn't help: the part still doesn't land. To clear a clash, rename this file, or take the other one away and restart Fusion.

`fusion_board_summary` with no document named reads the board in front.

## 1.13.65 (2026-11-03)

From AI Flow's run on a board an older bridge converted:

`fusion_set_layer_count` at the same count now also fixes vias Fusion marks `extent: invalid`, drawn while the stack had no through via. They stayed invalid after the stack was repaired, and only RIPUP by net name reaches them, so it lands those nets again (`netsLandedAgain`), reads the copper back and undoes it if it differs. `fusion_routing_state` names such nets (`invalidViaNets`).

The notice "The board file has errors and will be opened with warnings" has its own category, `board_opened_with_warnings`. It was read as an unsaved-changes prompt.

`fusion_dismiss_blocking_dialogs`: `closed` now means the window is gone from the screen, and the verb reports `success: false` and `stillUp` if one isn't. It used to report a window closed when the close was only accepted, and that notice ignores WM_CLOSE: it now clicks OK.

Dialog resolutions and the exe's hints no longer name the retired `fusion_send_key`. The add-in's own text still does, until its next release.

## 1.13.64 (2026-11-03)

`fusion_schematic_run` with a long list (#137): the call waits about 100 s, then answers `stillRunning` with a `runId`, and `fusion_schematic_run {runId}` polls it. Adom Bridge gives a call to another machine 120 s, so a long list used to read as `timed_out` with Fusion still working. A list that finishes in time still answers as one call.

`fusion_sch_reset_fields` lists a field already in place under `untouched`, rather than answering with nothing.

## 1.13.63 (2026-11-03)

`fusion_schematic_run` has a 20 minute budget in Adom Bridge. It had the 60 second default, so a long list reported `timed_out` while Fusion carried on working (#137).

`fusion_set_layer_count` repairs a board an earlier version converted: Fusion drops the bad via definition on load, so what such a board holds is a stack with no through via. Called again at the same count, it writes the stack again.

## 1.13.62 (2026-11-03)

`fusion_schematic_run` takes a list of any length (#137, Andy Engevold): it was refused past 400 commands, about 57 parts at 7 commands each. The bridge
checks the whole list first, so a bad command still means nothing ran, then runs it in batches of 300 and answers as one call (`batches`, and on a failure
`failedAt` in your list, `batchesDone`). Fusion can answer other calls between batches.

## 1.13.61 (2026-11-03)

Vias on converted boards: `fusion_set_layer_count` wrote a via definition with no start or stop layer, so a VIA with a layer range landed
nothing afterwards. It now writes Fusion's own shape, and calling it again at the same count repairs a board it already converted.

`fusion_route_net`: a route that does not land as planned comes off again, one undo step at a time until the copper is back (`undone`,
`viasNotLanded`, `tracksAsPlanned` in the reply). It used to leave the copper on the board.

Pours: `fusion_apply_pours` takes any copper layer of the board, numbered or named (`In1.Cu`); it took top and bottom only. It also takes `cutouts`
(top and bottom, on the restrict layers; Fusion has none for inner layers), `connect` and `order`. `fusion_edit_pours` moves a pour to another net
(`newNet`) by drawing it again with the same outline and settings.

`fusion_edit_tracks` re-aims a track end at an angle: the new track goes down, then the old one comes off. `library_name_in_use` no longer says a
restart clears it: Fusion's in-use list survives a restart.

Efficiency playbook (`fusion-design-efficiency`, PR #34 by Drew Owens): position parts and add joints only when the human asks. Added on
merge: positioned parts don't follow when the part they sit on changes size, so on a parameter-driven assembly the AI says so and offers joints.

## 1.13.60 (2026-11-02)

Library names: a library name already in use from another path is refused (`library_name_in_use`, naming both paths), because loading
it too would make the next ADD open Fusion's Select Library picker, a modal that blocks Fusion. Found when a drill with real libraries hung on it.

## 1.13.59 (2026-11-02)

`fusion_edit_tracks` moves a track's end only along its own line. At an angle Fusion's MOVE bends the track into two segments, whatever
WIRE_BEND says: that is undone and refused (`end_move_bends`), pointing at ripup and route_net. Deleting and redrawing is no way round it:
DELETE of a net's last wire removes the whole signal. `fusion_sch_wire` and the wire `fusion_sch_swap_part` adds draw straight.

## 1.13.56 (2026-11-02)

The rulings on standard #29:
- `fusion_preferences` takes `key` (one setting). `fusion_select_items` takes `where` (board or sheet), refused when the other
  editor is in front. `fusion_cancel_run` with nothing running answers `cancelRequested: false` and a reason, not an error.
- New, the standard's 0.31 verbs: `fusion_sch_label`, `fusion_sch_wire`, `fusion_place_parts` (a board with no schematic), `fusion_edit_tracks`
  (routing_state's tracks now carry an id), `fusion_model_check` (does each part's cloud 3D model open), `fusion_extensions` (list).
  `sch_no_connect` answers not_supported with the reason: Fusion has no such marker.
- `license`: `use_license` takes the seat here, `release` gives it up, anything else reads the account. `state` takes `screenshots`.

## 1.13.55 (2026-11-02)

`fusion_sch_swap_part` keeps the designator when the new device's prefix differs (see 1.13.54), with a regression test on the command form.

## 1.13.54 (2026-11-02)

- `fusion_sch_swap_part` works: REPLACE takes the part first, then the device, and the old order answered "Device not found",
  so the verb has done nothing since it shipped (only its dry run was tested). When the new device's prefix differs Fusion renames
  the part, so the name is put back, then `keepComment` runs (1.13.55: Fusion ignores NAME right after REPLACE until its events
  are pumped, so the rename pumps them first). Where pins end elsewhere Fusion keeps the nets itself:
  `pinsJoined` reports them, and a wire joins any it lost.
- `fusion_bom` honours `dnp` and `excludeFromBom` from the board. `fusion_preferences` routes correctly. `fusion_select_items`
  checks each part exists (Fusion answers "Selected X" for any name). `fusion_update_pcb` does not report `executed` while
  parts on the sheet are missing from the board.

## 1.13.53 (2026-11-02)

Partials on the EDA Bridge Standard, schematic edits first, and three candidates from standard #29:
- `fusion_sch_place_part` takes `partId` (a gate of a multi-gate device), and works from a local .lbr (it did since #135).
- `fusion_sch_swap_part`: where a new pin ends elsewhere, a wire joins it to where the old pin ended (`pinsJoined`,
  by pad number). `lostConnections` is what's left over.
- `fusion_edit_parts`: `dnp`, `excludeFromBom`, `excludeFromPos` and `mount` are attributes on the part, read back,
  and `fusion_board_bom` (`excluded`), `fusion_bom` and `fusion_export_pos` honour them. Fusion's own variants and CAM
  outputs do not read them.
- `fusion_update_pcb` (new): the standard's preview shape. Execute has nothing to run; removals are refused.
- `fusion_preferences` (new): read, or change with `set`. `fusion_select_items` (new): select by designator, or clear.
- `fusion_schematic_run` and `fusion_sch_place_part` refuse a position past 1950 mm (`off_the_sheet`): Fusion dropped such
  a part silently, said "no such device", and left its name unusable.

## 1.13.52 (2026-11-02)

Issue #136, the actual cause, reproduced with the reporter's library and bisected live on Fusion 2706.1.14: Fusion raises
its `assetAdded` error on every ADD, whichever part, from a .lbr where a `<package3d>` names a footprint the file does
not have. `adom-lbr assemble` merges parts with identical footprints, and a 3D entry written per original part was left
pointing at the merged-away name, so any library with two parts sharing a footprint hit this. Removing only those
entries makes the same library add; adding one to a clean library breaks it.
- `fusion_schematic_run`, `fusion_sch_place_part` and `fusion_sch_swap_part` remove those entries before USE, when no
  device uses them, and `fusion_schematic_run` names them in `library3dEntriesRemoved`. An entry a device still points at is
  left alone.

Also for #136: the seat dialog. The bridge answers Autodesk's Active Sessions Exceeded dialog with a click, which Adom Bridge
gates behind its approval, and with approval off the refusal was silent, so a run sat at the dialog. It now raises the
approval prompt (at most every 5 minutes) and clicks through on the next pass once approved; `fusion_claim_seat` and
`fusion_resolve_seat_dialog` answer `needs_approval` with `tellUser`.

## 1.13.51 (2026-11-02)

Drew Owens, PR #33 (add-in 1.0.32): typed writes recompute at checkpoints, as Fusion does. Every write still reads the
timeline's health after Fusion's own compute, and an unhealthy result is refused and rolled back as before. The full
`computeAll` now runs when the writes since the last one add up to 6 (deletes count most, metadata nothing), when the
writes move to another top-level occurrence with edits worth at least 2 behind them, before `export`, and on
`health {recompute:true}`. Failure recovery and the transaction probe still recompute in full. Every write reply and
`health` carry `compute`. On a 1,100-occurrence design a write without a recompute takes 5.8 to 9.9 s, from 13.1 s.

## 1.13.50 (2026-11-02)

Drew Owens, PR #32 (add-in 1.0.31):
- `exploded_view_stills` (new mechanical operation): step images for assembly instructions. Steps in assembly order;
  per step, one fixed orthographic camera saves `step_NNN_before.png` (the step's parts hovering where they go, earlier
  steps seated, later steps out of frame) and `step_NNN_after.png` (seated), plus `step_000_after.png` with nothing
  assembled. The reply gives every moved part's centre and box in both images, normalized, and the camera. `camera`
  auto, fit or current; `context` auto or keep. Limits 64 steps, 2048 moves, 4,096 occurrences.
- Exploded renders move only what moves (`Mover`, `moves` in the reply): a 91-frame animation on a 4,579-occurrence
  assembly now takes 187 s.
- A clean saved design no longer fails with `restore_failed` after a render: moving a part and reverting it leaves
  Fusion's modified flag set, and the API cannot clear it. Home poses and the saved flag are still verified, and the
  reply says `documentModified` with a note. Replies carry `target` as it is after the render and `sourceTarget`.

## 1.13.49 (2026-11-01)

`fusion_new_electronics_from_eagle`: the test-only `_openDialogWaitSec` can be 0, so the late-dialog watch from 1.13.48 can be
proved live.

## 1.13.48 (2026-11-01)

Issue #136 (Andy Engevold, AI Flow on 1.13.47):
- Fusion 2706's ADD also raises its `assetAdded` error on a .lbr with a UTF-8 byte-order mark or single-quoted
  attributes (bisected live). `fusion_schematic_run`, `fusion_sch_place_part` and `fusion_sch_swap_part` now rewrite
  those in place too, keeping the element tree (`libraryReformatted`). Found harmless, and left alone: CRLF, indentation,
  no XML declaration, a DOCTYPE, comments, non-ASCII text, apostrophes in text, an unresolvable 3D package URN.
- `fusion_new_electronics_from_eagle`: the Open dialog that "never appeared" was on screen. The command is queued, and a
  busy Fusion opened the dialog after the 90 s wait, with nothing left to fill or close it. The import now waits up to
  5 minutes (stage `waiting_for_open_dialog`); if it still gives up (`open_dialog_not_seen`), it closes a late dialog and
  the .brd one it chains to for the next 10 minutes, until another import starts. It finds the dialog without depending
  on Windows' language (the File name control, not the title).
- The import no longer takes an unreadable Adom Bridge status as approval: it refuses `ab_unreachable` before any
  dialog opens. If the approval wait itself breaks (a 504), it keeps reading the grant for the rest of the 5 minutes.
- `fusion_board_seed` with no fixed parts writes no library, rather than an empty one under the project library's name.
- `fusion_schematic_run`: the replies name `libraryLoaded`, the .lbr the bridge loads with USE itself (USE is not in the
  command list). A part name in lower case no longer reads as a failed ADD: EAGLE keeps names in upper case.

## 1.13.47 (2026-10-31)

Issue #135, the actual cause, bisected live on Fusion 2706.1.14 with the AI Flow lane (adom-aiflow#114): Fusion
raises its PlacePartWorker `assetAdded` error on every ADD from a .lbr whose elements are not separated by line
breaks (`<packages><package ...>` on one line), whatever the parts are. The same library with a line break between
its elements adds cleanly, with or without 3D packages. One compact boundary inside `<devicesets>` was enough.
1.13.46's diagnosis (parts without a 3D package) was wrong: those parts add fine from a library broken into lines.
- `fusion_schematic_run`, `fusion_sch_place_part` and `fusion_sch_swap_part` rewrite a compact .lbr in place before
  USE: a line break between elements, nothing else (the element tree is compared before and after and nothing is
  written if it differs). In place, because a copy at another path under the same library name makes Fusion ask
  which one to take once both are in use. The reply says `libraryReformatted`. A file the bridge cannot write is
  refused (`library_not_writable`) with the reason.
- The `no_3d_package` refusal from 1.13.46 is removed.
- `fusion_schematic_run` still checks each ADD and stops at the first that did not land (`add_failed`), and an
  assertion during the run still fails it (`fusion_assert`).

## 1.13.46 (2026-10-31)

Issue #135, the real cause (checked live on Fusion 2706.1.14): ADD raises Fusion's PlacePartWorker `assetAdded`
error when the part's footprint carries no 3D package Fusion can add to the design. The same ADD lands for parts whose
footprints carry a cloud 3D package, in a new design or an imported one, from USE of the .lbr; the seed's library and
its name were not the cause.
- `fusion_schematic_run` refuses, before any ADD, a part whose footprint has no 3D package (`no_3d_package`, naming
  each part), so Fusion never raises the error. It still checks each ADD and stops at the first that did not land,
  and an assertion during the run still fails it (`fusion_assert`).
- 1.13.45's whole-library seed is reverted to the fixed parts' devices (`wholeLibrary:true` embeds them all), and
  schematic_run uses the .lbr as before; neither changed the outcome.
- A 3D package that exists but cannot be added (a WIP cloud file in another project, locally modified) still makes
  Fusion raise the error; the run then stops at that part and says so.

## 1.13.45 (2026-10-31)

Issue #135 (Andy Engevold, AI Flow schematic_run: every ADD failed with "No part available", then Fusion's
PlacePartWorker assetAdded assert, clicked away, and the run went on past the caller's timeout):
- `fusion_board_seed` embeds the WHOLE project library in Seed.sch (every deviceset, symbol, package and 3D package),
  not only the fixed parts' devices. Every later ADD is then from a library the design holds; on Fusion 2706 USE of a
  .lbr on disk is accepted but ADD finds nothing, which is what failed. `wholeLibrary:false` keeps the old seed.
- `fusion_schematic_run` ADDs from the design-held library (USE of the .lbr on disk only when the design does not hold
  it), checks after each ADD that the part is on the sheet, and stops at the first that did not land: `add_failed`
  with `failedAt`, `part` and why. The reply names `libraryFrom` (design or use).
- Every Fusion assertion the bridge clicks OK on is recorded; a verb during which a non-harmless assertion was raised
  (anything but the routing pusher.cpp one) fails with `fusion_assert` and `fusionAsserts`, instead of reporting
  success.

## 1.13.44 (2026-10-30)

- The import's needs_approval reply carries ab's own answer (`approvalAnswer`) and no longer claims a five-minute wait
  (ab can answer sooner, as it did live). Checked live on 1.13.43 with approval off on the test laptop: the start reply
  said needsApproval with the sentence for the user, the status read waiting_for_approval, no dialog opened, and the
  run ended needs_approval with Fusion untouched.

## 1.13.43 (2026-10-30)

- Issue #133, the cause on Andy's PC: ab gates UI Automation and keystrokes behind its shell approval even for a
  bridge, so with no grant every way of filling the Open dialog was refused (needs_approval, reproduced on the test
  laptop with approval off). The import now checks approval before any dialog opens: without it, it asks ab (the
  prompt shows in Adom Bridge or Hydrogen), the start reply carries `needsApproval` and `tellUser`, the run's status
  reads `waiting_for_approval`, and it waits up to five minutes for the click. With no click it stops with
  needs_approval and no dialog opened.

## 1.13.42 (2026-10-30)

Issue #133, after Andy's run on 1.13.41 failed on his PC ("the File name box could not be set"):
- When UI Automation cannot set the Open dialog's File name box, the path is typed (a fresh Open dialog has the cursor
  in that box) and Enter pressed; ab gives the window back afterwards. The reply carries `typedVia` and, per attempt,
  `typeErrors` (what ab answered), so a failure says why.
- A stuck import closes the whole chain: cancelling the .sch dialog makes the importer open its .brd dialog at once,
  which was the dialog left on Andy's screen. The Untitled documents are closed only after the last dialog is gone,
  and `dialogClosed` is true only when no Open dialog is left.
- `fusion_eagle_import_status` with no runId answers the latest import on the box, for a caller that lost the runId.

## 1.13.41 (2026-10-30)

- `fusion_dismiss_blocking_dialogs` scans again after closing a native Open dialog: a cancelled EAGLE import opens its
  next Open dialog (the .brd after the .sch) at once, which one pass left behind (seen live).
- Checked live on 1.13.40 with add-in 1.0.30 (#133): fusion_new_electronics_from_eagle imported a .sch and .brd pair
  as AI Flow calls it, 109 of 109 parts placed, success true, no dialog left.

## 1.13.40 (2026-10-30)

- Issue #133, second half: the EAGLE import's wait for the new design compared the workspace's NAME (PCB Editor), but
  add-in 1.0.29 (1.13.38's #131 fix) answered the workspace's ID (BoardLayoutEnvironement), so a successful import
  (live: both dialogs taken, 109 of 109 parts placed) was reported as nothing_instantiated. Add-in 1.0.30 answers the
  name again (activeWorkspace, as DebugCommands.ActiveWorkspace did) with the id beside it (activeWorkspaceId), and the
  import wait and library_views accept either, so a Fusion still on add-in 1.0.29 works too.

## 1.13.39 (2026-10-30)

- Issue #133 (Andy Engevold, AI Flow): `fusion_new_electronics_from_eagle` left Fusion's native Open dialog up, empty,
  and replied that it had cancelled it, while Fusion stayed blocked.
  - The File name box is set by three selectors in turn (its name, the common dialog's control id 1148, the first
    Edit), each submitted, until the dialog takes the path.
  - A dialog that will not take it is closed by its Cancel button, then WM_CLOSE, and the reply says it was closed only
    when it is gone (`dialogClosed`, `closedBy`); otherwise it says the dialog is still open, with its hwnd.
  - The empty Untitled documents the failed import opened are closed (`untitledClosed`).
- `fusion_dismiss_blocking_dialogs` closes a leftover native Open/Save dialog (class #32770) too, with Cancel, and
  proves it gone: fusion_check_dialogs listed it while dismiss said no dialog was open.

## 1.13.38 (2026-10-30)

- Issue #131 (from hydrogen#152): the bridge's polled reads cancelled the user's Fusion command in progress (a MOVE
  snapped back). The add-in's get_app_state ran `Electron.run DISPLAY` as its "is Electronics" test, and starting a
  command in an electronics editor ends the user's; get_app_state is the readiness round trip, the statusVerb, and the
  waits. get_app_state and document_info now read the API only (the active product's objectType, ui.activeWorkspace),
  get_app_state reports userCommandActive, and tests/test_polled_reads_run_no_commands.py keeps text commands out of
  them. Add-in 1.0.29: Fusion loads it on its next start (fusion_restart_for_addin saves open work first).

- `fusion_export_manufacturing` (the standard's export_manufacturing): silkscreen prep, Gerbers and drill (JLCPCB CAM
  job), pick and place with JLCPCB's CPL, the BOM order files, and ODB++ / IPC-2581 with `formats`, in one folder.
- `fusion_pcb_report` (the standard's pcb_report): a .lbr's footprints (pads, pad bounding box in mil, primitive
  census, 3D packages) offline, or the open board's outline, parts, nets, census and tracks.

- `fusion_panel`: a panel the editor in front does not have is refused with that said (Fusion lists panels per editor;
  the 1.13.37 battery met the 3D design in front, which has no Inspector), and the battery opens Browser.

- sheetParts carry each part's `description` (its DESCRIPTION or DESC attribute), so fusion_bom's lines and the
  engine's other schematic checks read it (1.13.37's bom lines had an empty description).

## 1.13.37 (2026-10-30)

- Engine 0.24.0 (standard 0.30.0). `fusion_bom` runs the engine's bom (ported from the Altium bridge, so every bridge
  answers alike): the schematic's parts with their attributes, the live board and the current or named variant, one
  line per MPN, the order checks, and bom.csv, bom-jlcpcb.csv, cpl-jlcpcb.csv, bom-mouser.csv and bom-digikey.csv written
  to outputDir. cross_check stays Fusion's own live sheet-to-board compare.
- `fusion_render_board_3d {refs}`: the body check used the occurrence's component (an Occurrence has no
  allOccurrences, seen live on 1.13.36); a body-less part is listed in framed.missing.

## 1.13.36 (2026-10-30)

- `fusion_board_bom` (and fusion_bom, jlcpcb_order, assembly_check, which read it): parts without an MPN were one line per
  footprint, so a 47 nF and a 10 uF cap of the same footprint shared a line, valued "test point" and given the first
  part's LCSC number (seen live on lane-BQ25792-copy). Without an MPN a line is now footprint, value and LCSC together.
- `fusion_render_board_3d {refs}`: a part with no 3D body (an empty box at the origin) is not framed; with only such
  parts it answers unknown_parts and points at fusion_bodies' noBody.

## 1.13.35 (2026-10-30)

- `fusion_render_board_3d` (the standard's render_board and render_board_3d): `refs` frames the named parts (the union
  of their 3D boxes, `marginMm` round them), `rotate` turns the view ('x,y,z' degrees about its centre), and
  `quality:'high'` is Fusion's local ray-tracer from the same camera, with every render setting put back and the render
  cancelled after `renderSeconds`. Checked live on lane-BQ25792-copy: IC1 and C11 framed and turned '-45,0,45'; an iso
  ray-traced render at 800x450.
- `fusion_render_board_layers`: the standard's presets (outline, drill, assembly_top/bottom, fabrication added) and
  copper layer names over Fusion's EXPORT IMAGE, with widthPx/heightPx; svg refused by name (Fusion writes none).
- `fusion_bom`: the standard's bom from board_bom, schematic_data and the variants (DNP left out, checks for
  missing and inconsistent part numbers, schematic-board differences).

## 1.13.34 (2026-10-30)

Partial standard verbs answered exactly, in the standard's own shapes (stdshapes.rs):
- `fusion_board_summary`: the standard's summary shaped from one read_board (it was fusion_board_info's reply).
- `fusion_read_schematic`: components with designator, libReference, comment and parameters, from schematic_data.
- `fusion_list_library`: a .lbr's component (sch) or footprint (pcb) names, offline.
- `fusion_export_pos`: pick-and-place per side, csv or ascii, mm or in, smdOnly, DNP left out, JLCPCB's CPL with
  fab:'jlcpcb'; format gerber refused by name (Fusion writes none).
- `fusion_window_info`: Fusion's windows and open dialogs without screenshots.

- `fusion_import_step` names the placed component after the import (Fusion's import renamed it, seen live as
  Component376). Checked live on 1.13.33: a SOT-23-5 STEP placed in lane-BQ25792-copy's 3D PCB at (10, 10, 5) mm, turned
  90 degrees, 6 bodies, no new document; then deleted again.
- `fusion_tool_update` check: 25 s budget (it took 42 s on the laptop with the 40 s budget). Live: Fusion 2706.1.14,
  no update offered, auto on.

## 1.13.33 (2026-10-30)

- `fusion_import_step` reaches the exe's board form: 1.13.32 still routed it to the add-in's import, which opened the
  STEP as a new design (seen live). `into:'new'` still does that on purpose.
- `fusion_tool_update {action:'check'}` stops its update-button search at 40 s (`complete:false` then) so it answers
  within the relay's budget, and reads Fusion's version from app.version.
- Checked live on 1.13.32: fusion_panel lists Fusion's 14 panels and opens one, verified EXPANDED;
  fusion_export_3d wrote the board as STL (18.8 MB) and refused vrml by name; fusion_edit_pours set a pour's thermals
  and rank, read them back, and set them back to the board's starting revision.

## 1.13.32 (2026-10-30)

More of the standard answered exactly (eda-bridge-standard#29, "we need the verbs"):
- `fusion_panel` (the standard's panel, no longer not_supported): Fusion's docked panels (Browser, Inspector, Display
  Layers, Selection Filter, Design Manager, Place Components, Errors, Bill of Materials, Text Commands ...) answer its
  Panels.* text commands: Panels.List names them, Panels.Expose opens one, Panels.ValidateState proves it is expanded.
- `fusion_export_3d {format}`: one verb for the board's 3D model (stl, obj, glb, 3mf, fbx, iges, sat, usdz, f3d, skp,
  step); the 3D PCB is brought forward first, and a format Fusion cannot write is refused by name.
- `fusion_import_step` places the STEP with the board: imported into the design's 3D PCB as a new component at `at`
  and `rotation`, so bodies and height_check see it. `into:'new'` keeps the old new-design form.
- `fusion_change_pours` (and `fusion_edit_pours`, the standard's name) also sets thermals, rank and orphans, takes the
  standard's names (minWidthMm, clearanceMm, priority, keepIslands), and names a pour by net and layer when it is the
  only one there. Moving a pour to another net is refused: a polygon belongs to its signal.
- `fusion_tool_update {action: check | apply | auto}`: check reads Fusion's update offer without clicking anything.
- `selection` stays not_supported, now with three live checks: a ULP's ingroup(), ui.activeSelections after a SELECT,
  and the electron Board's selectionSets (InternalValidationError) all read nothing.

## 1.13.31 (2026-10-30)

- Engine 0.23.0 (standard 0.28.0). `fusion_schematic_review` (and `board_report`) send the sheetDrawing companion, so
  the drawing checks run: dangling wire ends, floating labels, overlapping symbols. Read from the schematic's EAGLE
  XML, mm with y down. EAGLE joins by segment, not geometry, so the drawing says what EAGLE means: a label whose text
  sits off its wire is put on the nearest point of its own segment, a wire ending on the middle of another wire of
  its segment gets a junction there, and a supply symbol's pin is a power label. `fusion_schematic_data
  {drawing:true}` returns it too. A body-less SMD test point now counts as flat in placement_check (engine).

## 1.13.30 (2026-10-30)

- Engine 0.22.0 (standard 0.27.0): `fusion_schematic_review` runs KiCad's design checks too (I2C pull-ups, LED series
  resistors, regulator input and bulk capacitors, dangling wires, floating labels, shorted net names, overlapping
  parts), and `fusion_placement_check` gives a placement score. sheetParts now carry each part's `value` and `libRef`
  (its deviceset, EAGLE's device name), which those checks read.

## 1.13.29 (2026-10-30)

- `fusion_inspect_bodies` brings the design's 3D board forward and reads again when the 2D board or schematic is in
  front (it answered "'NoneType' object has no attribute 'rootComponent'" right after review_board); `shown3d` says so.

- `fusion_sch_batch` (the standard's sch_batch, agreed on eda-bridge-standard#29): schematic edits all or nothing. Each op
  is a standard sch_* verb with its arguments, run as that verb runs, so it sees the sheet as the earlier ops left it.
  A failing op undoes the ops before it and the schematic is checked back at its starting revision (`failedOp`,
  nothing changed); if UNDO cannot return, the steps are redone and the reply says `partial` with `ranOps`. Fusion's
  ERC runs before and after (`ercBefore`, `ercAfter`).

- `fusion_log_tail` (the standard's log_tail, agreed on eda-bridge-standard#29): Fusion's own log since a byte offset,
  `entries` [{level, text}] (alert, error, warning, info; `errorsOnly` keeps all but info), at most `maxBytes`
  (default 200000), `next` for the next read, `truncated`, and `rotated` when Fusion started a new log. `fusion_alerts` takes the same `sinceOffset` cursor and now
  always answers `nextOffset`.

## 1.13.28 (2026-10-30)

- `fusion_verb_times` and `fusion_prewarm` answer the standard's shapes (standard 0.26.0, #29). verb_times: `verbs` is
  the list `[{verb, count, p50Sec, p90Sec, maxSec, lastSec, failures}]`, slowest p90 first, plus `since` and `limit`;
  the older map stays as `byVerb`. prewarm: `ready`, and each step's `done` (its `ok` kept).

- Three standard verbs that were not_supported now work (eda-bridge-standard#29, every verb):
  - `fusion_plate_vias`: each unplated `<hole>` in a .lbr's footprints becomes a plated pad. One inside an SMD land (a
    heatsink via in an exposed pad) is named N@k after the land and joined to its connects, so it is on the land's
    net; any other is H<k>. Offline, with a .bak and the preflight.
  - `fusion_cleanup_libs`: a .lbr's empty leftovers removed (components with no device, and footprints and symbols
    with nothing drawn that nothing uses).
  - `fusion_sch_update_from_library`, the plan: each placed part's component as the schematic holds it against the
    same-named deviceset in the .lbr files named (gates, pins added/removed/changed, symbol drawing, attributes),
    and the wired pins a new symbol would drop. apply is refused with the plan (Fusion's UPDATE asks per part).

- `fusion_restart_for_addin` no longer meets Fusion's Unsaved changes box when the add-in cannot save (issue #129).
  A save whose add-in connection dropped is retried once after 5 s (the add-in can be unreachable for a moment
  right after `bridge_install`). If the add-in still answers its status probe but the save failed, the restart
  stops with `restart_needs_decision` before anything is closed. Fusion is closed without the add-in's save only
  when the add-in is not loaded at all.

From Aditya Angajala (PR #17): `fusion_export_optimized_glb` produces a publishable molecule, not just a GLB.
- It exports the EAGLE `.brd` (new `board` arg, default true) and sends it to service-step2glb as the `board` part,
  the same contract as `step2glb convert --board`. With the board the service generates the footprint and symbol
  JSON with per-pad wire-attach Z; without it `footprint_applied` was null and the GLB could not be published.
- The export and `fusion_fetch_optimized_glb` download the job's side artifacts beside the GLB:
  `<stem>_footprint.json`, `<stem>_symbol.json`, and `<stem>.web.glb` when the service made a web-grade twin.
  Replies carry `boardPath`, `footprintPath`, `symbolPath`, `webGlbPath` and the molecule-publish `gates`
  (pinSize, pinSizeSource, footprintApplied, footprintPins, footprintRebase, contactZ, warnings); the `_hint`
  names any gate that is not met.
- Every export also writes `<stem>-fab.zip`, the Gerber + drill output of the bridge's own JLCPCB CAM job (new
  `gerbers` arg, default true; Fusion's one Excellon file is marked mixed plating), and `export.json`: tool and
  version from the .gbrjob, exporter version, every source file with sha256 and role (STEP, .brd, silk PNGs, the
  zip and each file inside it) and the stackup the Gerber job knows. Nothing in the STEP or Gerbers is renamed.
  Replies carry `fabZipPath` and `exportRecordPath`. Merged onto master's silk cleanup (#113) and `compress` check.

## 1.13.27 (2026-10-30)

From Drew Owens (PR #28): the mechanical layer on big assemblies, profiled inside Fusion on a 4,579-occurrence
production assembly built from 23 referenced designs.

- The revision digest every guarded call pays went from 32.6 s to 5.2 s: entity tokens are read only on a miss,
  refused timeline-index reads are remembered per feature revision, and components come from the occurrences read.
- References: an occurrence registered this session reuses its token, and a reference resolves by walking its path's
  names (248 ms to 5.7 ms). Joint solving compares each occurrence with the one at its path instead of every pair.
- Write guards keep each body's witness by revision and reuse the pre-check's health walk.
- `bom` reads metadata and physical properties once per definition; `budgetSeconds` (default 40) stops the cloud
  part-number reads and marks the rest `nativePending` (cached for the next run). `inspect` leaves out body metrics
  above 200 components (`bodyMetrics:true`) and the health walk above 200 timeline items (`health:true`).
- `efficiency_report` returns the full target. The add-in is 1.0.28; the exe embeds the new `schema.json`.

From Drew Owens (PR #30): the hang gate never kills a Fusion that is using CPU. On 2026-10-30 it killed a Fusion 37
minutes into a long build script whose request had timed out, closing 24 (saved) documents. `confirmed_hung` now also
needs the Fusion family under 15% of one core over 1.5 s, and its 180 s clock restarts whenever Fusion works. Busy
replies carry `keptBy` (`under-ceiling`, `fusion-cpu-active`) so a caller sees what keeps Fusion alive.

From Drew Owens (PR #31): `mech_validation.json` regenerated from master after #28, with the 12 validation cases for
`inspect`'s `bodyMetrics`/`health` and `bom`/`bom_export`'s `budgetSeconds` (2,805 cases).

## 1.13.26 (2026-10-30)

- Engine 0.21.1 (standard 0.24.2): lint_library's duplicate_pad_number counts same-numbered pads that touch or overlap
  as one land, so a split exposed pad is no longer a false warning. Otherwise as 1.13.25, which this replaces before
  it was smoke-tested.

## 1.13.25 (2026-10-30)

- Engine 0.21.0 (standard 0.24.0). `fusion_lint_library` (the standard's lint_library): a .lbr read offline (pads,
  courtyard on tKeepout, silk on tPlace, 3D packages, descriptions; each component's pins, footprint and MPN) and judged
  by the shared engine, every problem a finding with a rule name and a severity.
- `fusion_update_footprints_from_library` (the standard's update_footprints_from_library, #27), the plan: each placed
  footprint against the same-named package in the .lbr files named, pad by pad (up_to_date, differs with the diff, or
  library_not_found). apply is refused with the plan: Fusion's UPDATE asks about each part in a dialog.
- `fusion_edit_footprint` writes a library that has footprints but no components yet (it checked it with the full
  preflight, which demands symbols, and refused). 349 verbs.

## 1.13.24 (2026-10-30)

- `fusion_list_installed_libraries` (the standard's list_installed_libraries): the libraries Fusion offers to place
  from are the hub's electronics libraries (.flbr, Library Manager's Hub Libraries tab). Fusion keeps no local list and
  draws that tab without UI Automation, so the verb walks the hub's projects and folders through the signed-in session
  (breadth first, to a depth, within a time budget).
- `fusion_add_symbol_art` (the standard's add_symbol_art): the chip outline adom-symbol baked into a KiCad symbol, drawn
  into the component's symbol fitted to its body (the TPS54202's 162 polylines live). Offline on a .lbr.
- uninstall_library, normalize_bodies and sch_sheet_symbol answer not_supported: taking a hub library out would delete
  the file for everyone; a Fusion footprint's 3D model is a cloud package placed when made, with no stored offsets; and
  a Fusion module cannot be made from a text command (checked live). 347 verbs.

## 1.13.23 (2026-10-30)

- The standard's app tier in Fusion's terms:
  - `fusion_diagnostics`: facts for a bug report without driving Fusion (bridge version and folders, bundled add-in,
    Fusion launcher, processes and windows, whether the add-in answers, the Adom/Fusion/Autodesk environment with the
    values of credential-named variables never shown);
  - `fusion_instances`: every running Fusion process;
  - `fusion_progress`: the long operation in flight, measured phase and verb timings, recent runs;
  - `fusion_foreground {reason}`: Fusion brought in front, the sanctioned exception: the user is notified why and given
    three seconds first.
- panel and selection answer not_supported: Fusion's editors show their own panels (the bridge opens surfaces with
  fusion_show_*), and Fusion does not show the user's board selection to the bridge (a ULP's ingroup() reads no part
  even right after a GROUP, checked live). 345 verbs.

## 1.13.22 (2026-10-30)

- `fusion_variants` (the standard's variants): a design's assembly variants, each with its not-fitted parts and
  alternate values, read from the schematic. Making, changing or removing one is refused with not_supported: Fusion
  2706 takes EAGLE's VARIANT from a text command (any form, schematic or board) and changes nothing.
- `fusion_silkscreen_prep` (the standard's silkscreen_prep, in Fusion's terms): designators on copper moved off it where
  they fit (the engine's silk_designators), counted before and after; loose silk outside the board listed. A footprint's
  own silk lines are its library package's and cannot be clipped on one board: the fab's CAM clips silk at mask openings.

From Drew Owens (PR #27): CAM from the typed mechanical layer, and exploded renders that work on big assemblies.

- `cam_job` makes a milling job in one journaled call:
  - a setup on the given bodies, and operations in order (`face`, `adaptive`, `contour`, `parallel`, `scallop`, `pocket_clearing`, `drill`);
  - tools from Fusion's sample library by type and nearest diameter, with optional parameter overrides by name;
  - drilling finds the bodies' holes of the drill's diameter unless `holeFaces` are given;
  - it generates the toolpaths, reports each operation's warnings and errors and the machining time, and with `post` writes G-code
    through a post from Fusion's library (FANUC, Grbl, HAAS, ...);
  - a failed job deletes its setup.

  `cam_tools` lists the library's tools, and `cam_inspect` reads a document's setups and NC programs. Live: a pocketed, drilled plate gave
  valid face, adaptive, contour and drill toolpaths and a 1,901-line FANUC program. Known gap: the FANUC O-number stays O1001.
- `exploded_view_render` captures only the moved occurrences and their parents (children follow), so a 4,579-occurrence assembly
  renders instead of hitting the 256-instance budget. It checks home poses within 0.01 mm (`restoredWithinMm`) instead of exact float
  equality, which flagged a clean revert on a big assembly as `restore_failed`.
- 67 typed operations; the add-in is 1.0.27, and the exe embeds the new `schema.json`. 341 verbs.

## 1.13.21 (2026-10-29)

- `fusion_replicate_layout` (the standard's replicate_layout): one channel's placement copied to matching channels round
  each target's anchor, turned by the anchors' rotation difference; bottom parts stay mirrored; one guarded move.
- `fusion_symbol_prims` and `fusion_set_visibility` (the standard's): a component's drawing counted (pins, lines,
  rectangles, polygons, body rectangle in mil, footprints), and a parameter shown on the sheet or hidden by adding or
  removing its >NAME text in the component's symbols. Offline on a .lbr.
- component_classes answers not_supported: an EAGLE board has net classes but no component classes or rooms. 339 verbs.

From Drew Owens (PR #26): exploded-view animations for operators, and photoreal renders.

- `exploded_view_render` plays the same steps as `exploded_view_plan` on the real occurrences and saves one viewport image per
  frame from a camera fitted to the most exploded state. Then it reverts every move and verifies each instance is home and the
  document's flags are unchanged. It refuses a design that already has an uncaptured move (the revert would discard it), and
  names any part that did not reach its planned pose (`heldBack`). The reply carries timed captions from the steps' titles and
  instructions, and the ffmpeg line that joins the frames into MP4 (Fusion's API has no video export). Journaled.
- `render_image` renders with Fusion's local renderer from a fitted standard view or the current one, at a size and quality,
  optionally transparent (PNG only) or with the ground. It waits for the render, then restores the camera and every render
  setting. Journaled. Fusion's API cannot choose the render environment or cancel a render; after `render_timeout` the render may
  still write the file.
- Edge cases checked live on Fusion 2705:
  - nested children in their parent's frame, a rotated part along its own axis, a rigidly jointed part and component-pattern copies;
  - someone's uncaptured move refused and left in place, overwrite, the frame budget, bad folders and sessions;
  - render settings and camera restored, JPEG output, and a real timeout.
- 64 typed operations; the add-in is 1.0.26, and the exe embeds the new `schema.json`.

## 1.13.20 (2026-10-29)

- Engine 0.20.0 (standard 0.23.0). `fusion_bodies` gives a part turned off the axes its rectangle along its own axes
  (`cx, cy, w, h, rotation`, measured by Fusion's oriented bounding box along the part's own x axis) beside the world box,
  so height_check, placement_check and the other body checks stop counting the box's empty corners. A SOT-23-5 at 45
  degrees: world box 4.07 x 4.07 mm, its own rectangle 2.8 x 2.9.
- `fusion_simulate`: `analysis {type: noise, source, output, reference?, sweep, points, start, stop}` answers the output
  and input noise spectra (`onoise`, `inoise`, V/rtHz) and their integrated `totals` (an RC: sqrt(4kTR) at 10 Hz,
  2.03e-7 V integrated against kT/C's 2.04e-7). `monteCarlo {runs, distribution, seed, tolerances}` draws every R, C and L
  with a plain value within its tolerance (uniform, or gaussian with sigma a third of it), a signal per run with the drawn
  values as its step; the same seed draws the same values.

## 1.13.19 (2026-10-29)

- review_board, board_report, pin_pad_check and schematic_review give the engine each part's attributes as
  `sheetParts[].params` ({name, value}, empty values left out), so vendor numbers and datasheets the parts carry count.
  On BQ25792 the 48 parts with an LCSC number now miss only Mouser and DigiKey, and the 5 with a DATASHEET have one.

## 1.13.18 (2026-10-29)

From Drew Owens (PR #25). Design efficiency: measure what makes a design slow, and stop the bridge from making it slow.

- `efficiency_report` reads what makes a design slow (timeline make-up and health, Position snapshots, Move features,
  size, repeated definitions made by Paste New, instance-weighted faces, imported history, wires in a parametric design,
  big sketches, pattern compute options, modeled threads, meshes, empty components) and returns findings with the fix.
  It never changes the model and is journaled like `bom_export`; a 4,579-occurrence cell took 76 s.
- `occurrence_update` keeps a move by editing the occurrence's initial position (Fusion's Edit Initial Position) when
  Fusion allows it, so nothing joins the timeline; an earlier occurrence is edited with the marker rolled back to it and
  restored. `method` chooses, and the reply's `placement` says which was used. Nine moves: 0 snapshots, against 9.
- Rectangular and circular patterns take features, faces or components (occurrences, patterned in their parent) as well
  as bodies, with `computeOption` (identical by default, optimized, adjust). The revision guard accepts a component
  pattern's copies. On a 100-hole plate a feature pattern recomputed in 0.24 s, the sketch of circles in 0.87 s, adjust
  in 2.42 s.
- The `fusion-design-efficiency` skill: the playbook, with those benchmarks and its sources.
- The add-in is 1.0.25 (new op `efficiency_report`: 62 typed operations); the exe embeds the new `schema.json`.

## 1.13.17 (2026-10-29)

From Drew Owens (PR #24). The mechanical BOM reads a large real assembly right, and the revision guard accepts assemblies built from referenced designs.

- BOM views beside the purchasing list: `indented` (item numbers, quantity per parent, kits and phantoms), `cutList` with a
  first-fit-decreasing `stockPlan` for extrusion, `wireList`, `orderList` (bought lines merged by supplier part number),
  `fasteners`, `totals` and `warnings`. `bom_export` gains the modes `indented`, `cutlist`, `fasteners`, `wires` and `order`.
- Every line says why it is made or bought (`makeBuyReason`). Metadata wins; otherwise the name is read: made here
  (fabricated, 3D printed, cut from), cut from stock (wire, extrusion, DIN rail, duct), a McMaster number (also filled in as
  the supplier part number, marked `supplierInferred`), a fastener, electronics, hardware or a cable, a vendor note such as
  "(vendor STEP)", or a maker's name. `totals.inferredLines` counts what was read rather than stated.
- An assembly that reads as a bought product (an imported .stp or STEP-translator model, a vendor note, a maker, a McMaster
  number) or whose children are a circuit board's footprints counts as one bought line (`inferKits`). A body-less group
  named as a library is excluded (`includeLibraries`). Stand-in geometry and items supplied with something else are
  ordered by nobody.
- Wires are measured as body volume over end-cap area, exact for a section swept along any path (the box undercounts a
  routed wire).
- `component_create` and `component_metadata` take `category`, `makeBuy`, `process`, `finish`, `unitCost`, `currency`,
  `leadTimeDays`, `stock`, `notes` and `revision`.
- Scale, measured on a 4,579-occurrence cell: the revision guard accepts components inserted by reference (they enter by
  source document version), identity lookups are indexed instead of compared pairwise, and Fusion's cloud part number and
  description reads (about 0.6 s per component) are cached per component revision or skipped with `nativeProperties:false`.
  `inspect` gains `detail:"target"` and `componentProperties`. A guarded operation on that cell went from about 20 minutes
  to about 40 s.
- The add-in is 1.0.24, so Fusion reloads the changed mechanical package; the exe embeds the new `schema.json`.

- `fusion_simulate` sweeps (standard 0.21.2): `sweep {parameter, start, stop, step, scale?}` steps a part's value (R4) or
  a model parameter (Q1.BF), and `temperatures {start, stop, step}` the temperature; each run is its own signal with
  `step` ("R4 = 300, T = 85"), run in one ngspice batch with alter, altermod and option temp. A partly modelled sheet now
  runs and names the parts it left out in `notSimulated` (BQ25792: 89); a sheet with no modelled part refuses with no_model.

## 1.13.16 (2026-10-29)

- Engine 0.19.0 (standard 0.22.1): review_board's rule classes from standard-lane's fix.
- `fusion_teardrops` (the standard's teardrops): the engine plans each teardrop to hold the board's clearance; Fusion
  draws them as named solid polygons (rank 1, no thermals, so Fusion never asks for a signal) and reads them back
  (Fusion writes a drawn polygon as <polygonpour>), undoing all on a mismatch.
- `fusion_panelize` (the standard's panelize): the engine plans the panel (V-score or mouse-bite tabs, rails, tooling
  holes, fiducials, JLCPCB's rules); Fusion builds it as a new EAGLE board from the open board's source, copy k's
  parts and nets renamed _k, the copies' outline left to the panel's rout path and the board shape recomputed, and
  opens it as a local design. 336 verbs.

## 1.13.15 (2026-10-29)

- `fusion_not_supported`: the standard verbs that belong to Altium's own machinery (Altium 365, its script agent and
  .NET extension, integrated libraries, OutJobs, symbol display modes, library defaults) answer not_supported with what
  Fusion does instead, under their own names (fusion_a365_status ...) or through this verb, which lists them.
- `fusion_swap_footprint` (the standard's swap_footprint): a part on a board kept as EAGLE source takes another
  footprint at the same place and turn, its pads rewired from assigns by pad number; designator picks the part.
- `fusion_set_models` and `fusion_add_models` (the standard's): a component keeps one footprint, or gains more as
  further devices, pins joined to pads by number (repeated numbers' N@2 pads too); a footprint already attached is
  skipped. set_params joins a footprint the same way. 334 verbs.

## 1.13.14 (2026-10-29)

- Engine 0.18.0 (standard 0.21.0). plan_route and the other planners take the track width from the board's minimum
  width rule (msWidth) when no net class sets one, instead of the engine's 0.25 mm default.
- `fusion_footprint_from_land_pattern` (the standard's footprint_from_land_pattern): the engine lays out a datasheet's
  land pattern (chip, dual, SOT, quad, BGA, exposed pad) and the bridge writes it into a .lbr offline: pads with EAGLE
  roundness from the engine's cornerRatio, silkscreen on tPlace, the fab outline on tDocu, the courtyard on tKeepout.
  New libraries now carry the tDocu and bDocu layers.
- `fusion_build_board` (the standard's build_board): a new board from a parts list and a netlist, written as EAGLE
  source (outline with rounded corners or points, footprints by origin, pad nets), opened in Fusion as a local design
  dialog-free and read back pin by pin (wrongPins, missingParts, unconnected).
- `fusion_test_board` (the standard's test_board): footprints from a .lbr on a new board, or appended to an existing
  .brd with designators continuing; a plain rectangular outline grows to hold appended parts, and parts outside any
  other outline are named. assigns and stubs apply to the last part.
- `fusion_import_part` (the standard's import_part): the footprint and symbol spec files Altium's bridge takes go into
  one .lbr offline (pins joined to pads by number, a repeated pad number as N@2 on the same pin, params and comment as
  attributes), preflighted before writing; a STEP is minted and bound to the component dialog-free. Importing again
  replaces the footprint, the symbol and the 3D body. 330 verbs.

## 1.13.13 (2026-10-29)

- Engine 0.17.2 (standard 0.20.2). The checks get the board's minimum track width and drill (EAGLE's msWidth,
  msDrill) as the standard's width and hole-size rules, and review_board and board_report are told EAGLE's DRC always
  checks shorts: BQ25792's three missing_critical_rules errors were rules Fusion has.
- `fusion_board_report`: the JLCPCB assembly section gets the BOM lines with no LCSC number (as fusion_assembly_check
  does), and the DRC section reads Fusion's violation and airwire counts.

## 1.13.12 (2026-10-29)

The standard's pair and length planners, the schematic checks and the design review and report from the shared engine 0.17.0 (standard 0.20.0). 326 verbs.

- `fusion_route_pair`: a differential pair planned as a pair, one centreline with each net offset at width + gap
  (or the gap for diffOhms from the stack-up): routeNet for each net, widthRule for fusion_add_rule, skewMm, and the
  problems that stop it. On BQ25792's D pair it finds the pads on swapped sides at the two ends.
- `fusion_length_tune`: a serpentine in a net's longest straight track to reach targetMm or match another net,
  answered as remove (fusion_remove_items) and the routeNet that replaces it. On BQ25792 it took SCL from 53.613 to
  exactly 55.6 mm, placed with no split and no airwire (and put back).
- `fusion_schematic_review`: every placed pin's electrical type against the netlist (power pins on no net, outputs
  driving each other, open inputs, single-pin nets, missing decoupling). BQ25792: no errors; SW1 pin 3 open and no
  decoupling cap on VBAT near U$1/U$3.
- `fusion_pin_pad_check`: each symbol's pins against its footprint's pads, and parts only on the sheets or only on
  the board. BQ25792: 103 parts match; SW1's mounting lugs (pads 4, 5) have no pin; the logos are board-only.
- Package-less symbols (EAGLE's supply symbols) are not parts to these checks.
- `fusion_review_board`: thirteen design-review checks over one read of the board, errors first, with what was not
  judged and why.
- `fusion_board_report`: one self-contained HTML page with every check the shared engine has, Fusion's own DRC and a
  picture of the board (BQ25792: 10 sections, 523 KB). Fusion stores its DRC result in the board: drc:false skips it.
- The engine verbs' replies no longer carry empty `moveArgs`, `remove` and `routeNet` fields (1.13.11 added them as
  null to every reply).

## 1.13.11 (2026-10-29)

Six more of the standard's verbs, engine 0.14.1 (standard 0.17.1). 320 verbs.

- From the shared engine:
  - `fusion_auto_classes`: GND, Power, Clock, HighSpeed classes and pairs from the net names; a proposal, and
    apply:true writes the classes that carry a width as Fusion net classes (fusion_add_rule);
  - `fusion_silk_designators`: every designator one size, upright, beside its part and clear of pads, bodies and
    each other; landed with fusion_apply_silkscreen unless dryRun (Fusion's designator boxes are computed from the
    text for the engine);
  - `fusion_find_space`: free spots for a part near another, nearest first, or check:true for overlaps;
  - `fusion_check_orientation`: 2-pad parts whose turn would shorten their airlines.
- Native:
  - `fusion_jlcpcb_order`: the JLCPCB fab check, Gerber zip, JLCPCB BOM and CPL, assembly-tier check and the order
    form values, in one folder with every issue in one list;
  - `fusion_sch_zoom`: parts highlighted and framed on their sheet, with a picture of the view (no foreground).
- `fusion_apply_silkscreen` resizes a text where it stands, before moving it (picked at its new place, the size
  did not land: BQ25792's SW1), and undoes every command when any text does not read back as asked (before, only a
  copper change was undone).
- The standard's app verbs Fusion answers under its own names are mapped in the standard's equivalents:
  animate_view (fusion_view), window_info (fusion_check_dialogs), license (fusion_account), eval
  (fusion_run_modeling_script, Python with Fusion's API), open_project (fusion_open_cloud_file).

## 1.13.10 (2026-10-29)

Two more of the standard's checks from the shared engine 0.12.2 (standard 0.15.2). 314 verbs.

- `fusion_net_lengths`: each routed net's length by layer with its vias, the skew of each differential pair (by the
  nets' names) and the spread of matched-length groups.
- `fusion_assembly_check`: the board against JLCPCB's Economic and Standard assembly tiers; the BOM lines with no
  LCSC number come from the board's own part attributes.
- Engine 0.12.1-0.12.2: a route with no path says why at each end (`ends`, `avoidExempt`) and, when only an avoided pour blocks it, gives the path through it (`withoutAvoid`); assembly_check counts pads sharing one centre as one land and leaves pad-less parts out.

## 1.13.9 (2026-10-29)

Circuit simulation with the ngspice Fusion bundles (the standard's #23). 312 verbs.

- `fusion_export_spice`: the design's SPICE netlist from EAGLE's own EXPORT SPICENETLIST (no dialog), ngspice dialect,
  with its elements, the parts that have no SPICE mapping, analyses and includes.
- `fusion_simulate`: op, dc, ac (dec, oct, lin), tran or raw SPICE text, probes by net or source current, signals as
  x/y data (AC in dB with the unwrapped phase in degrees). The netlist gets the analysis and a .control block and
  Fusion's bundled ngspice 41 runs it in batch mode in a child process, so Fusion's simulation dialog never opens. A
  part with no SPICE mapping refuses with `no_model`. Checked on an RC low-pass (1k, 100n): -2.99 dB and -44.9
  degrees at 1.585 kHz.

## 1.13.8 (2026-10-29)

Schematic placing and wiring, and engine 0.11.0 (standard 0.14.0). 310 verbs.

- New, each read back and undone on a mismatch, no dialog:
  - `fusion_sch_place_part`: a deviceset from a library the design holds (by name) at x, y (mil), with its
    designator (the next free one for R? or none) and value; Fusion adds it to the board too;
  - `fusion_sch_connect`: a stub from each pin's outer end named for the net, with a label (port: an XREF label;
    power: a label, as Fusion's power symbols are library parts); a pin on another net is refused, never merged;
  - `fusion_sch_swap_part`: a part's device replaced in place with REPLACE; the reply lists the pins that lost
    their net.
  - Two Fusion limits found live (2706) and refused clearly instead of failing silently: a variant named with
    parentheses (Fusion's passives: CHIP-0402(1005-METRIC)) cannot be added from a text command, and a local .lbr
    is accepted by USE but not added from (add the library to the design first). `fusion_schematic_run`'s ADD has
    the same limits.
- `fusion_sch_delete_wire` deletes a labelled stub: its label goes with it (only labels of the deleted wires' nets),
  and the reply lists `labelsRemoved`. Before, the read back undid it.
- `fusion_export_ipc2581` is written by the shared engine (IPC-2581C, valid against the schema): the same copper as
  Fusion's own writer on BQ25792 (920 pads, 577 lines, 176 holes, 43 pour contours, 50 nets); parts with no pads
  (logos) are left out.
- Engine 0.10.4: `fusion_plan_route` smooths grid staircases into straight and 45-degree segments (BQ25792's N$15: 8
  corners, 13.9 mm, its via where the designer put it, routed with no split and fewer DRC entries than the
  original), echoes `avoidPours` and `pourCost`, and exempts a pour holding the route's own end pad.

## 1.13.7 (2026-10-28)

Engine 0.10.3 (standard 0.13.3) and `fusion_diff_pairs`. 307 verbs.

- New: `fusion_diff_pairs`, the board's differential pairs as Fusion keeps them, by the nets' names (NAME_P/NAME_N,
  P/N, +/-, DP/DN). Fusion has no pair object, so defining or removing a pair is refused with how to rename the nets.
- Engine 0.10.2/0.10.3: `fusion_plan_fanout` skips routed pins, checks its stubs and vias against other nets' tracks,
  arcs and vias, keeps an exposed pad's grid off existing holes and names the pours it crosses;
  `fusion_plan_route` takes `avoidPours` as a list of nets and a soft `pourCost`.
- Checked live on BQ25792's N$15: with the original via size (0.55/0.35) on a 0.1 mm grid, the plan puts its via
  where the designer did, (35.7, 1.8), and `fusion_route_net` places it with no split net and no airwire.

## 1.13.6 (2026-10-28)

Engine 0.10.1 (standard 0.13.1) and three more of the standard's verbs. 306 verbs.

- New:
  - `fusion_read_board`: the live board in one round trip, as sections (components, nets and classes, layers, copper,
    rules, occupancy, outline), each list capped at `limit`;
  - `fusion_add_rule`: a scoped width, clearance or via-drill rule as an EAGLE net class. A net scope gets its own
    class (`Adom_<name>`), each net joined with one pick on its wire; read back, undone on a mismatch; `remove` puts
    the nets back in the default class;
  - `fusion_estimate_cost`: the board's quote numbers and a rough price table, from the shared engine.
- Engine 0.10.1: `fusion_plan_route` checks a through via on every copper layer, names the pours a route crosses
  (`crossesPours`) and can keep out of them (`avoidPours`); `fusion_check_clearance` lists stacked parts apart;
  `fusion_place_decoupling` takes supply nets only and leaves room for the IC's fanout.

## 1.13.5 (2026-10-28)

Routing and placement from the standard's shared engine (adom-eda-checks 0.8.0, standard 0.11.0), and `fusion_route_net` on
any board. 303 verbs.

- `fusion_plan_route` is the shared engine's (#21): A* on any copper layers listed, pads by their real outline, planned to
  the board's strictest copper clearance and its copper-to-edge rule, so the plan is one `fusion_route_net` accepts.
  `routeNet` carries `boardId` and `expectedRevision`. Fine-pitch pads need `gridMm` 0.1.
- New, read only, from the shared engine (#22):
  - `fusion_check_clearance`: part-to-part clearance by courtyard (with 3D bodies) or by pad copper;
  - `fusion_place_decoupling`: a plan placing an IC's decoupling caps beside its power pins, as `fusion_move_parts`
    arguments, checked with placement_check;
  - `fusion_plan_fanout`: a part's fanout as `fusion_route_net` calls, with the conflicts against other nets' pads.
- `fusion_route_net` works on any board, not only two-layer ones without pours:
  - any copper layer, by name (In1.Cu) or number; a through pad or via is on every layer of the stack;
  - pours are repoured round the new copper, and a route that cuts another net's pour in two is undone (`splits_net`;
    `allowSplit` keeps it). On BQ25792 a planned N$15 cut VBAT's pour and was undone;
  - curved tracks, footprint and board copper, keepouts (tRestrict, bRestrict) and footprint mounting holes are
    obstacles; pads are measured by their real outline;
  - `viaSize` is accepted for `viaDiameter`; a clearance refusal names the copper it would hit and where.

## 1.13.4 (2026-10-28, public)

The standard's shared engine 0.5.1 (standard 0.8.1) makes 3D bodies, and placement checks know about molecules.

- `fusion_generate_body` uses the shared writer for chip, soic, qfn, qfp, sot23, dfn, bga, melf, ecap, crystal and
  header: the STEP is written on the Fusion machine and Fusion is not needed. It matches the four bodies Fusion's
  generator made for standard #20 (0603, SOIC-8, QFN-32 with its exposed pad, QFP-48), and its STEP opens in Fusion
  and in OCCT. Any other family, or `generator: "fusion"`, still goes to Fusion's package generator.

- `fusion_generate_body` gives each package generator its pins the way that generator counts them. SOIC-8 came out
  with 4 leads and QFP-48 with 24: SOIC, SOJ, SON, DIP and the oscillators take the total, QFP and PLCC the two
  opposite sides together, QFN each side (read from Fusion's generator scripts). `D2`/`E2` now draw the exposed pad
  (the generator's thermal pad; QFN calls it `D1`/`E1`).
- `fusion_generate_package {outputStep}` makes the STEP's folder first. Fusion's STEP export writes nothing into a
  missing folder, and the verb then failed writing its manifest. It now also refuses if no STEP was written.

- `fusion_placement_check` lists a part under a raised molecule as `stacked` (pads
  compared one by one, bodies in 3D) instead of a courtyard overlap. On BQ25792: 39 overlaps and 78 stacked, where
  1.13.3 reported 117 overlaps.

## 1.13.3 (2026-10-28, public)

The standard's shared engine (adom-eda-checks 0.4.0, standard 0.7.0) plans the vias, and two placement checks are new.
300 verbs.

- New, read only, from the shared engine and `fusion_bodies`:
  - `fusion_height_check`: each part's height, parts over a limit or under a lid region, bodies that collide or
    come too close; parts without a 3D body are listed, never passed;
  - `fusion_placement_check`: courtyard overlaps, parts off or over the edge, pads near the edge, parts on a
    mounting hole.
- `fusion_thermal_vias` and `fusion_via_fence` are planned by the shared engine and placed by Fusion, read back and
  undone on a mismatch. The plan keeps clear of vias already in the pad, and `dryRun` answers `apply` as
  `fusion_place_vias` arguments (which now also take `size` for the diameter).
- `fusion_bodies` gives a box per sub-assembly for a part whose 3D model is an assembly (an Adom molecule: its own
  small board on pins, its connector on top), so a part under a molecule's raised board is no longer a collision
  (92 collisions on BQ25792 became 9). Heights come from the bodies themselves: Fusion's box for the whole
  occurrence was 3 mm short on the barrel-jack molecule's housing (16.7 mm, not 13.7). `parts` counts the parts.
- The checks' via size comes from the board's via rule, now passed to the engine in the standard's form.
- Every engine reply reports the engine's own `engine` string.
- The busy gate waits out the bridge's own quick reads (`document_info`, `get_app_state`) for up to 15 s instead
  of refusing the next verb. With about 90 documents open, the post-verb document snapshot can outlive the 3 s
  the window etiquette waits for it, and the caller's next verb then found it still running and got
  `main_thread_busy` (a mechanical `inspect` in the assembly campaign, behind a 4.9 s `document_info`). Real work
  in front of a verb is refused as before.

## 1.13.2 (2026-10-28, insiders)

The standard's shared writers, for files every bridge writes the same way.

- `fusion_export_gencad` is written by the EDA Bridge Standard's shared engine (adom-eda-checks 0.2.1,
  standard 0.5.1), in inches. Shapes are keyed by footprint and pad geometry, so two same-named footprints with
  different pads stay separate shapes.
- `fusion_export_dxf {layers}`: copper layers and Edge.Cuts come from the shared engine; silkscreen, fab and
  documentation layers stay on this bridge's writer, which also draws their texts.
- The engine's checks are unchanged.

## 1.13.1 (2026-10-28, insiders)

More of the standard's schematic, document and board verbs. 298 verbs.

- New, read only:
  - `fusion_schematic_data`: parts with pins (pad numbers, electrical type), gates, values and attributes;
  - `fusion_part_parameters`: attribute names across the placed parts, or one attribute's value on each;
  - `fusion_cross_check`: the schematic's designators against the board's;
  - `fusion_footprint_primitives`: one placed part's pads, wires, arcs and texts on the board;
  - `fusion_export_pdf`: every sheet as one PDF, plus the board with `board:true`.
- New edits, each read back and undone on a mismatch:
  - `fusion_doc_params`: the schematic's global attributes;
  - `fusion_sheet_params`: attributes on placed parts (the BOM write-back);
  - `fusion_sch_edit`: move, rotate, rename, re-value or delete placed parts;
  - `fusion_annotate`: numbers `R?`-style designators.
- New board furniture, each spot clear of other copper, pads, holes and the edge, read back and undone on a
  mismatch:
  - `fusion_thermal_vias`: a grid in an exposed pad;
  - `fusion_via_fence`: vias along RF or fast tracks and/or the board edge, at lambda/20 or a pitch;
  - `fusion_board_features`: mounting holes, fiducials, keepouts and a stitching grid.
- Fixed: GenCAD writes `MIRRORX FLIP` for bottom parts, matching KiCad's native GenCAD (standard #16).
- Live on fusion-lane's copy of BQ25792, with the test edits removed afterwards.

## 1.13.0 (2026-10-28, insiders)

Drew's typed mechanical layer, Fusion 2706 support, his safe-restart work, and library authoring.

### Mechanical layer (merged #22, Drew Owens)

The typed mechanical layer (`fusion_mechanical`, 61 operations; `fusion_mech_capabilities`;
`fusion_mech_operation_status`), with the exe owning validation, the durable journal and replay.
Its development history is in [docs/MECHANICAL-HISTORY.md](docs/MECHANICAL-HISTORY.md).

- The exe owns the layer's validation, durable journal and replies, as it does for the rest of the bridge; the
  add-in only runs the Fusion API work. Capabilities and operation status answer from files, so they never wait
  on Fusion's main thread.
- Readbacks the 2026-10-27 certification run needed (CSWA parts and the feature matrix, built through typed
  operations only):
  - `inspect` lists `constructionPlanes` per component, the three origin planes included, with refs. A plane
    reference can be read again after a write moves the revision, and an origin plane's ref resolves through its
    component (`originPlane` xy/xz/yz), so base planes work wherever a construction plane is accepted
    (`construction_axis` two_planes, `sketch_create`, feature tools).
  - `inspect_entity` on a sketch returns its `frame` (origin, x, y and normal in component space) and each
    profile's `centroidComponent` next to its sketch-space centroid.
  - Parameter rows name their `kind` (model or user) and the `owner` that made a model parameter (feature,
    dimension or joint, with its role), so `feature_edit` no longer needs a parameter-list diff.
  - `material_list` without `libraryId` searches the document's materials and then every loaded library; each
    row names the `libraryId` that `material_assign` needs.
- The verb suite triages the three verbs: `fusion_mech_capabilities` and `fusion_mech_operation_status` as daily
  read-only checks, `fusion_mechanical` as a negative check (an unknown operation is refused by the exe's
  validation). The typed writes are covered by the mechanical drill. On macOS the journal path is returned, not a
  `todo!()`.
- Run live since: `joint_origin_*` and `motion_link_*`. All 15 assembly campaign groups pass on Drew's laptop,
  133 more checks with no failures (PR #23). Not run live: `mcmaster_open` (interactive by design).

### Fusion 2706, safe restarts (merged #21), library authoring

- Fixed for Fusion 2706, which removed the `Electron.boardInfo` text command. On released 1.12.8 that broke
  `fusion_board_info`, `fusion_board_summary`, `fusion_export_cpl` and electronics_read's board fallback. The exe
  now builds the same answer from the board's own export: the summary, plus an XML in boardInfo's layout, so
  every consumer reads it unchanged. It also writes the pick-and-place CSV itself. On older Fusion the add-in
  path is used as before.
- Merged #21 (Drew Owens), in his words:
  The bridge no longer restarts Fusion over a busy add-in, and a restart it does need no longer kills Fusion over
  unsaved work.
  - A busy add-in is never mistaken for a missing one. While a long native Fusion call holds the add-in's main
    thread, not even `/status` or `/health` answers, and the gate read that silence as "the add-in was installed
    while Fusion was running" and restarted Fusion. On a person's own PC that killed Fusion with their documents
    open six times between 2026-10-14 and 2026-10-28 (a 93 s script was in flight on 10-28). Now silence counts as
    work whenever there is evidence the add-in is loaded: one of the exe's own requests to it is still in flight,
    or it answered earlier inside the same Fusion process and its port still takes connections. The verb gate, the
    timeout diagnosis, the readiness launch observer (which restarted Fusion after about 3 minutes of silence) and
    a warm `fusion_start` all ask for that evidence first and answer `fusion_addin_busy`. `fusion_readiness` and
    `fusion_state` report it as `addinBusy`, and the dashboard no longer restarts over it. A request in flight also
    counts as a long operation for the hang verdict.
  - `restartPolicy` `auto` (the default) stops short of losing work. When the add-in cannot save, Fusion is closed
    the way a person closes it instead of killed: it closes at once when nothing is unsaved, and a save prompt for
    a document the bridge did not make (or a never-saved document) ends the restart with `restart_needs_decision`,
    with nothing killed or discarded. For 30 minutes after that the restart is asked for instead of retried. A stop
    that did not finish is no longer escalated to a kill. The old behaviour is `restartPolicy: force`, for a machine
    only an AI drives. The ledger's restart summary now says what happened: it said "Documents were saved first"
    when the save step had failed and Fusion was killed.
  - `fusion_stop` waits for heavy designs: up to `closeTimeoutSec` (default 120; it was 15 s). A save prompt for a
    document the bridge made is answered Don't Save; one for anyone else's work is cancelled and the stop fails
    with `unsaved_work`.
  - Documents a caller opens are no longer registered as bridge scratch. Any document that appeared during a
    window-raising verb was registered, so a user's assembly reopened with `fusion_open_by_urn` became "the
    bridge's": its save prompt would be answered Don't Save and a restart could discard it. Now only documents a
    verb made (not saved cloud documents, which were opened) and the bridge's own Save As are registered, and
    nothing Fusion opens at launch. The registry evicts its oldest entry when full; it used to evict the
    alphabetically first.
- New, offline on .lbr files, each change preflighted before it is written (the old file kept as .bak):
  - `fusion_create_footprint`, `fusion_create_symbol`, `fusion_read_symbol`, `fusion_find_symbol`;
  - `fusion_copy_footprint`, `fusion_rename_footprint`, `fusion_remove_footprint`, `fusion_xcopy_footprint`;
  - `fusion_clone_symbol`, `fusion_rename_symbol`, `fusion_remove_symbol`;
  - `fusion_set_params`: attributes, and joining a symbol's pins to a footprint's pads by number.
- The daily verb suite runs on fusion-lane's copy of BQ25792 (Default Project/fusion-lane-tests), never on the
  owner's design.

## 1.12.8 (2026-10-28, insiders)

KiCad's remaining core and app proposals, and a close that never saves unasked. 271 verbs.

- Fixed: `fusion_close_document {save:false}` on a saved cloud document with unsaved changes used to save a
  new version first, silently, because Fusion asks to save even when told not to. It made BQ25792 v228 for a
  caller who meant to discard. Such a close is now refused (`unsaved_cloud_document`) and nothing is
  written, unless `save:true` or `saveToClose:true`. The bridge's own internal closes pass `saveToClose`.
- New:
  - `fusion_cross_probe`: a schematic part to its board part and back, by designator; `select` highlights both.
  - `fusion_import_svg`: SVG artwork onto silkscreen, mask, paste or documentation layers. Fusion's POLYGON
    asks for a signal name in a modal, so filled shapes are drawn row by row with RECT. Holes follow
    even-odd, bottom layers are mirrored, and copper is refused before any command runs.
  - `fusion_copy_routing`: tracks and through vias by an offset, nets remapped through paired parts or a
    netMap. A copy that would land on another net is refused before drawing; Fusion's DRC runs before and
    after, and any new error undoes it.
  - `fusion_set_layer_count`: 2, 4, 6 or 8 copper layers through the board's design rules, with the
    thickness kept; layers still carrying copper are refused.
  - `fusion_project_info`, `fusion_project_snapshot` (EAGLE XML, PNGs and PDFs in a dated folder) and
    `fusion_project_create` (an empty schematic and board opened as a new design).
  - `fusion_export_library_part` and `fusion_edit_footprint`: offline .lbr work, preflighted before writing.
  - `fusion_project_rename`: a design kept as EAGLE files (folder and files renamed together); refused while open
    in Fusion, and for a cloud design, which Fusion's data panel renames.
  - `create_part_project`: the standard's name now runs `fusion_make_test_project`.
- The checks engine is `adom-eda-checks` 0.1.4 (standard 0.4.5): copper-only clearance (#13) and each pour
  layer listed once in `current_capacity` (#15).
- Daily suite: 131 pass on DESKTOP-BAR6S0F; the 5 failures are the APS verbs without the APS sign-in.

## 1.12.7 (2026-10-27, insiders)

The standard's fab exports, and read_footprint. 261 verbs.

- New:
  - `fusion_export_odb`: Fusion's own ODB++ (Electron.mfgodb with the JLCPCB CAM job), packed as zip, tgz or a folder;
  - `fusion_export_ipc356`: the IPC-D-356A netlist for bare-board electrical test (317 and 327 records, metric,
    long net names as NNAME aliases);
  - `fusion_export_gencad`: GenCAD 1.4 (outline, pads and padstacks, shapes, components with bottom parts mirrored,
    devices, signals, routes and vias);
  - `fusion_export_ipc2581`: IPC-2581 revision C, valid against the IPC-2581C schema (layers and stack-up,
    packages, components and BOM, nets, pads, traces, vias, the poured copper island by island, drills). It is
    mode ASSEMBLY: mask, paste and silkscreen are not in it, so a fab order still goes through ODB++ or Gerbers;
  - `fusion_read_footprint`: one footprint's pads, layers, courtyard box, graphics per layer and 3D packages, from a
    .lbr or (by `libId` "Lib:Name") from the libraries the open board embeds.
- Fab text is ASCII: unit signs in values are named (10kΩ becomes 10kohm in GenCAD and IPC-356, `\U+03A9` in DXF).
  Before, a board DXF with an Ω on the silkscreen failed with a codec error.
- The checks engine is now `adom-eda-checks` 0.1.2 (standard 0.4.3). Fusion passes only copper clearances to it,
  since 0.1.2's `min_clearance` also takes mask, dimension and restrict clearances (standard issue #13).
- Daily suite: 121 pass on DESKTOP-BAR6S0F; the 5 failures are the APS verbs without the APS sign-in.

## 1.12.6 (2026-10-27, insiders)

- The board verbs bring the named design's board forward when its schematic, 3D PCB or another document is in front,
  instead of refusing with "Activate a PCB board" (an error the bridge can fix itself). Found by running the checks
  right after the daily suite, which leaves the 3D PCB in front.

## 1.12.5 (2026-10-27, insiders)

The checks tier through the EDA Bridge Standard's shared engine. 256 verbs.

- `adom-eda-checks` 0.1.1 is vendored from the standard (0.4.2, commit 680ab69; `rust/vendor/adom-eda-checks/VENDOR.md`
  says how to update it, never by editing the copy) and linked into the exe.
- New: `fusion_copper_balance`, `fusion_return_path`, `fusion_impedance`, `fusion_current_capacity`, `fusion_fab_check`.
  Their input comes from one export of the live board (the board model, stack-up, Fusion's pour fills, the rules
  with copper clearances in the standard's `{kind: clearance, clearanceMm}`, the silkscreen), and the engine's
  follow-up verbs come back as this bridge's (`fusion_apply_pours`, `fusion_add_rule`).
  `fab_check {vendor}` fetches the record for the board's copper layer count from adom/pcb-design-rules.
- On BQ25792: JLCPCB's 4-layer record finds R3.1's pad 0.405 mm past the board edge (minimum 0.2 mm, routed edge),
  and 5 A on VBAT needs 2.8 mm of outer copper where the narrowest is 0.81 mm.
- Daily suite: 117 pass on DESKTOP-BAR6S0F; the 5 failures are the APS verbs without the APS sign-in.

## 1.12.4 (2026-10-27, insiders)

Standard parity, round three: the verbs credited by name now answer the standard's shape, and eight more. 251 verbs.

- Now in the standard's shape:
  - `fusion_export_dxf` with `layers` writes board layers to DXF, one file per layer (outline, copper with track
    widths, pads, vias, pour fills, silkscreen and fab with readable texts and resolved placeholders); without
    `layers` it is still the mechanical DXF;
  - `fusion_plan_route` takes `from`/`to`/`widthMm` and answers `found` and `routeNet` (two-layer boards);
  - `fusion_document_info` adds kind, saved, modified, path, version and project;
  - `fusion_close_all_documents` keeps unsaved work by default: a changed document stays open (keptOpen) unless
    `save` or `discardUnsaved`; scratch documents close only with `includeScratch`. Before, a changed saved document
    was saved (a new cloud version) and closed.
- New:
  - `fusion_render_board_3d` (Fusion's 3D view of the design, its 3D PCB shown when needed and the user's tab put
    back; transparent background);
  - `fusion_generate_body` and `fusion_library_views` (the standard's names over the package generator and the
    library captures, which open the library themselves);
  - `fusion_edit_parts` (value, designator, fields, lock; DNP and BOM flags refused, Fusion keeps them in variants);
  - `fusion_align_parts` (footprint box or anchor; refuses only the overlaps a move makes);
  - `fusion_remove_graphics` (board graphics by id, layer or kind; a dry run lists the ids);
  - `fusion_remove_parts` (deleted in the schematic: Fusion ignores a part deleted on the board).
- Learned live: `LOCK -` is not a command (a shift-click LOCK unlocks); a field the library part defines cannot be
  removed from the board; Fusion's `Electron.mfgodb` needs more than a CAM job (ODB++ is not offered yet).
- Daily suite: 111 pass on DESKTOP-BAR6S0F; the 5 failures are the APS verbs without the APS sign-in, and one test
  of mine corrected (plan_route on the four-layer BQ25792 is refused as two-layer only).

## 1.12.3 (2026-10-27, insiders)

- New `fusion_update_board_from_eagle {brdPath, expectDocument, save?}` (page issue #125, item 4): makes the open
  design's board the board in an EAGLE .brd (or .fbrd) in place, so a re-run updates the same design instead of
  importing another. Parts move and turn by name; tracks and vias are ripped up and drawn from the file; pours, the
  outline, holes and net-less copper are replaced; then the whole board (parts, copper, pours, outline, holes, nets)
  is read back against the file. With `save:true` it is saved in place only after an exact read back; otherwise
  nothing is saved and the reply says to close without saving. A file that adds or removes parts or changes a net's
  pads is a schematic change and is refused with `schematic_differs`. Not synced, and named in `notSynced` when the
  file differs: silkscreen and documentation, design rules, name and value positions. Fusion's export writes a pour
  as `<polygonpour>`, a file as `<polygon>`: both are compared by net, layer, pen, isolation, rank and outline.
  Verified live on a design saved to Default Project / fusion-lane-tests: J2 moved, NET_1 routed on both layers
  through two vias, a pour and a hole, then a second file applied with `save:true`: board v1 to v2 of the same
  lineage, the folder still holds one .fprj, one .fsch and one .fbrd, and the design reopened from the cloud shows the
  second file's board.
- `fusion_open_cloud_file` by name opens the electronics design (.fprj) when one has that name: opening the
  board or schematic file of a saved design raised Fusion's "links to multiple Electronics designs" chooser, which
  cannot be answered headlessly. It falls back to the plain name match when there is no design of that name;
  `fileExtension` still picks a file outright.
- Checked for #125: closing a never-saved imported design without saving writes nothing to the hub (no
  Untitled file appears in the project).
- Schematic edits honour `save: true`: after a read-back edit the design is saved in place (as
  `fusion_save_document`). `save` defaults to false, as the standard's conventions now allow, because each Fusion
  save is a new cloud version; every edit verb's describe says so. A never-saved design answers `saved: false` with
  the save's reason (with several open documents named Untitled, that reason can name the wrong one).

## 1.12.2 (2026-10-27, insiders)

The EDA Bridge Standard's schematic verbs (`sch_*`, agreed in standard 0.2.0): all 21, each read back on a live
design and undone on a mismatch. 243 verbs in the catalog.

- Reads, from Fusion's own export of the design's schematic whichever tab is in front (the tab is put back), or
  from a .sch/.fsch on the Fusion machine: `fusion_sch_items` (wires, buses, entries, labels, power ports,
  junctions, texts, module instances, each with an id built from its kind and position, since Fusion stores none),
  `fusion_sch_pins` (connection point, pad numbers, type, side, net, wires and pins on its segment),
  `fusion_sch_trace_point`, `fusion_sch_page`, `fusion_sch_hierarchy` (modules), `fusion_sch_sheet_pins`
  (module ports). Pin positions: every connected pin of BQ25792 and the routing fixture (274) sits exactly on a
  wire end of its own net segment. Fusion's ULP pin coordinates count the part's offset twice, so they are not
  the reference.
- Edits: `fusion_sch_text`, `fusion_sch_junction`, `fusion_sch_delete_wire` (junctions left joining fewer than
  three items, pins counted, go too), `fusion_sch_connect_pins`, `fusion_sch_split_wire`, `fusion_sch_bus`,
  `fusion_sch_bus_entry`, `fusion_sch_connect_to_bus`, `fusion_sch_edit_label`, `fusion_sch_move_region`,
  `fusion_sch_fix_connectivity`, `fusion_sch_reset_fields`, `fusion_sch_create`, and `fusion_render_schematic`
  (EXPORT IMAGE, with pixel baselines: drift, changed region in mm, a diff image).
- Refused with the reason: setting the page (a frame part and design attributes Fusion's commands do not reach),
  sheet-pin edits (module ports), `fusion_sch_update_from_library` (Fusion's Library Manager asks per part),
  renumbering sheets, `junction:false` on a split.
- What Fusion taught (in sch_script.py): SET CONFIRM only answers a question when it is on the same command line;
  a moved pin landing on another net makes Fusion ask to merge them, so moves answer No and refuse with
  `would_join_nets`; a group's outline snaps to the sheet grid (`box_edge_off_grid`); a scripted group MOVE leaves
  parts and global labels behind, so they are moved by name; a junction click on its centre takes a wire end;
  TEXT uses the current layer; edits run at a 0.001 mm grid and put the user's sheet and tab back.
- Undo never takes the user's own work: nothing is undone when Fusion made no change; steps are undone one short
  of the count and then one at a time, and if the starting revision never returns every step is redone and the
  edit reported as left in place.
- ab budgets: the routing and schematic edit verbs get 120 to 300 s instead of the 60 s default.
- Known: once, moving a wire end onto another net raised a Fusion "Error" box (closed by the dialog backstop; the
  verb timed out); not reproduced.

## 1.12.1 (2026-10-27, insiders)

The EDA Bridge Standard 0.1.1 settlements (standard issue #2).

- `isolatedCopper` (`{net, layer, at:{x,y}, areaMm2}`) in `fusion_routing_state`: a group of only poured copper
  (an island, or a pour that reaches no pad) is listed there and not counted in `unconnected`; a stray track or
  via still counts. `floatingPourIslands` stays beside it.
- A custom pad's `copperPoints` is one ring (the largest drawn polygon), as the schema now says; a pad that
  gathers several also lists them under `copperOutlines`. `polygons[].kind` is `zone` (a pour) or `graphic` (drawn
  copper, `source:"footprint"` inside a part).
- New `fusion_apply_silkscreen`: designators and values moved, sized, turned or hidden, free texts and lines added,
  earlier ones removed by `fusion_silk_state`'s new `raw` handles. Each pick is made with only the target's layer
  displayed; the layer display is restored one layer at a time (a DISPLAY that names the origin layers drops every
  layer after them); if copper, parts or the outline move, every command is undone. Verified on BQ25792: IC1 and
  R2 names moved, sized and turned exactly, C1's value hidden and shown, revision unchanged, display as before.

- Daily verb suite: 94 pass on DESKTOP-BAR6S0F (the new reads, select_net, layer visibility, board_outline, run_erc
  and the aliases run daily on BQ25792); the 5 failures are the APS verbs on a box without the APS sign-in.

Standard parity: 20 more verbs (222 in the catalog; Fusion covers 77 of the standard's 182, was 56).

- `fusion_set_track_width {nets | netClass, layer?, widthMm}`: each track is picked with only its layer displayed,
  at the point along it farthest from other copper (a midpoint pick took a neighbour), in up to three read-back
  passes; only the asked tracks may change, and only in width, or every change is undone. A stub lying wholly
  under a wider track of its own net, or inside its own pad, is reported as `covered`.
- `fusion_remove_items {tracks:[{start,end,layer}], vias:[{position}]}`, given as `fusion_routing_state` returns
  them; read back as the board minus exactly those, or undone. `results[]` per item.
- `fusion_set_outline {outline:{rect:[x0,y0,w,h], cornerRadius?} | {points}}` replaces the Dimension-layer outline
  (rounded corners are arcs: Fusion's WIRE takes the angle bare, `+90`; `$90` and `@r` are ignored without an
  error). The outline is read back closed with the asked edges, or every command is undone. `fusion_board_shape`
  is its rectangle form; `fusion_board_outline` reports the edge (Fusion's Gerbers export layer 20 as the outline).
- `fusion_set_layer_visibility {layerId, displayed}`, read back from the board's layer flags.
- `fusion_select_net {net, zoom?}` highlights a whole net (Fusion clears a finished SHOW's highlight at once, so
  SHOW is left waiting for a click: a command-line prompt, not a dialog) and frames it.
- `fusion_run_erc`: Fusion's ERC on the board's schematic (its tab brought forward, the board's put back), every
  message read from `UL_SCHEMATIC.errors` with class, code, sheet and position.
- `fusion_autoroute {nets?}`: Fusion's autorouter on the given nets, or every net with unconnected copper; existing
  copper is kept (read back, or undone); DRC before and after. Refused before anything runs when a copper layer in
  use is off in the board's autorouter setup: Fusion would ask "used but not enabled, run anyway?", which neither
  SET CONFIRM nor AUTO LOAD answers. Verified on the routing fixture: 6 unconnected to 0, DRC 0 errors.
- Standard names answer through the bridge's own verbs (marked `aliasOf`): status, save, run_script, run_command,
  list_commands, dialog_log, signin_status, license_status, open_document, show_3d_chip. `fusion_check_dialogs`
  is now in the catalog.
- `fusion_export_optimized_glb {compress: draco | quantize | none}` passes the STEP service's compression override
  (page issue #113): `none` gives a plain GLB that wiki README and widget frames can load, since they cannot reach a
  Draco decoder. Omitted, a molecule is Draco-compressed as before.

## 1.12.0 (2026-10-27, insiders)

The board model, to the [EDA Bridge Standard](https://wiki.adom.inc/adom/eda-bridge-standard) (fusion-lane thread).
Needs AI Flow 0.2.22 or later for its Fusion lane (a pad's `name` is now its own; `key` is `U1.3`).

- The daily verb suite now runs 20 of the new verbs on the canonical BQ25792 board (the board model, stack-up,
  netlist, pours, the reads, 3D bodies, the BOM's part numbers, the board_summary alias): 83 pass on
  DESKTOP-BAR6S0F; the 5 failures are the APS verbs on a box without the APS sign-in.
- Release exes are signed through Azure Trusted Signing (`tools/sign_pe.py`, from the KiCad bridge's signer).

- `fusion_routing_state` returns the standard's board model (`model: "eda-bridge-standard/board 0.1"`),
  computed from Fusion's own board export in the background:
  - millimetres, Fusion's board frame, y up; copper layers named `F.Cu`, `In<n>.Cu` (by stack order) and
    `B.Cu`, `*.Cu` for a through pad; `copperLayers` lists the stack;
  - `tracks`, `arcs` (center, radius, startAngle, endAngle counterclockwise), `vias` (position, size from
    the board's restring rules when the via leaves it to them, drill, layers), `pads` (ref, name, x, y,
    sizeX, sizeY, rotation, layer, drill, net, shape), `components` (ref, x, y, rotation, side, pattern,
    has3d), `polygons`;
  - a custom-shape pad is its bounding rectangle: Fusion's footprint `polygonshape` copper joining pads
    (the CSD17581Q3A drain joins pads 5 to 9, each 0.127 mm alone) folds into every pad it touches, with
    the real outline kept as `copperPoints`;
  - net-less copper (board copper, free footprint copper) is kept with an empty net;
  - the outline as `outlineEdges` with arcs, cutouts in `outlineCutouts`, and `outline` sampled every 2
    degrees; `outlineClosed` and `outlineProblem` say when it is not closed;
  - `unconnected` and `splitNets` from the copper itself (Fusion's fill included; slivers a pour leaves
    are `floatingPourIslands`, not missing connections); Fusion's ratsnest is `fusionAirwires`, and the
    hint explains a difference;
  - `revision` hashes copper, parts and outline only (silkscreen, rules and the ratsnest leave it alone).
  - The AI flow's keys stay: `segments`, `elements`, `airwireCount`, `remainingNets`, each pad's `at`,
    `radius`, `layers` and its `key` (`U1.3`), each via's `at` and `diameter`. **Changed:** a pad's `name`
    is now its own name (`3`) as the standard says, with `REF.PAD` under `key`; the old per-net rows moved
    from `nets` (now the list of net names) to `netRows`.
  - Verified on the BQ25792 charger (109 parts, 304 pads, 4 layers, 38 pours): every pad's position, size,
    rotation, layer and drill equals Fusion's own (a ULP dump of every contact), live and offline.
- New `fusion_layer_stack`: the stack-up from Fusion's Layer Stack Manager data (copper um, dielectric um
  and Dk x 1000 below each layer, prepreg or core, total), in the background. Falls back to the design
  rules and says the dielectric constant is unknown.
- `fusion_pour_state` adds `zones`: each pour's fill island by island with its holes.
- New `fusion_netlist`: every net with its pads (`U1.3`), live from the board or from a `.sch`/`.fsch` file
  without Fusion (`schPath`); `net` for one net, `expect` to compare pin by pin.
- New `fusion_read_sheet_file`: an EAGLE `.sch` (or the `.sch` in a `.fsch`) read in the exe, no Fusion: parts
  with value, part number, library, sheet and position, and pins with pad and electrical type.
- New `fusion_bodies`: each part's 3D body as a box from the 3D PCB (footprint rectangle, height above the
  surface, leads past the far side for through-hole parts), `noBody`, and `suspect` for a model off its
  footprint, floating or sunk.
- Standard names as aliases: `fusion_launch`, `fusion_board_summary`, `fusion_export_gerber`,
  `fusion_dismiss_dialogs`, `fusion_install` (the reply carries `aliasOf`).
- Board reads in the standard's names, all from the same background export, in mm: `fusion_component_data`,
  `fusion_component_pins`, `fusion_list_nets`, `fusion_net_connections` (with each net's airline length, the
  minimum spanning tree over its pads), `fusion_list_designators`, `fusion_layers`, `fusion_copper_layers`,
  `fusion_layers_in_use`, `fusion_read_rules` and `fusion_rules_by_kind` (Fusion's rule manager plus the classic
  values), `fusion_silk_state`, `fusion_net_classes`.
- `fusion_run_drc`: Fusion's DRC on the live board with every violation (rule, severity, layer, position,
  approved), no revision guard. `fusion_repour`: refill every pour and read the copper back. `fusion_zoom_to`:
  frame a rectangle, parts or a net.
- **Fixed: DRC errors and warnings were swapped.** Fusion's ULP gives a violation's type as 1 for a warning and
  2 for an error (and its state as 2 when approved); the bridge read type 2 as a warning, so
  `fusion_routing_drc` (and the AI flow's DRC gate) reported the BQ25792 board's 1611 errors as warnings and its
  14 Wire Stub warnings as errors. Each violation now carries `severity` and `approved` from Fusion's own
  constants (read live from the ULP, 2026-10-26).
- **`fusion_export_bom` carries part numbers** (page issue #105): after the old columns come MPN, Manufacturer, LCSC,
  Mouser and DigiKey from each part's attributes, read under every name libraries use (Adom's `MPN`/`LCSC_PN`,
  LCSC/JLCPCB's `LCSC`, SnapEDA's `MANUFACTURER_PART_NUMBER`, `MF`/`MP`...), blank when a part has none. The reply
  counts the parts with an MPN and an LCSC number and names the rest. The export now runs in the exe from the board
  export, in the background, and switches to the board itself when the schematic or 3D view is in front. The same
  names now feed `fusion_board_bom` (which read only `LCSC_PN`) and `fusion_component_data`. Live on BQ25792: 109
  parts, 45 lines, LCSC on 63 parts, MPN on 28.
- **Fixed: a routing read or edit could act on another board than the one named.** `doc.activate()` moves
  Fusion's API to a document without always moving its UI, and the board export and every Electron command act
  on the UI's board. With the two apart, `fusion_routing_state {expectDocument:"A"}` passed its name check and
  returned board B's copper, and `fusion_route_net` drew on B (caught live 2026-10-26, scratch boards only).
  Every routing verb now puts the UI on the named document's tab first (`UI.SwitchToTab`; a tab's index is the
  document's index), and so do the view verbs (`fusion_show_2d_board` and friends reported success while the
  UI stayed put).
- **Saving an electronics design** (page issue #125):
  - `fusion_save_to_cloud` on an electronics design saves the DESIGN (.fprj with its .fsch and .fbrd, linked;
    the 3D PCB beside them when open), whichever of its tabs is in front. A Save As by name with the board in
    front used to write a standalone .fbrd the design never linked.
  - It refuses a design that is already saved (`already_saved_use_save_document`) unless `asCopy:true`, refuses
    a name already in the folder unless `overwrite:true`, creates missing folders on `folderPath`, and needs
    `projectName` when no project is open in Fusion's data panel (Fusion cannot read the active project then).
  - New `fusion_save_document {documentName?, comment?}`: saves the open design in place as a new version and
    reads back each changed document's version before and after and the board the design links.
  - Verified live: first save with the board in front (fprj, fsch, fbrd v1, linked), route a net, save in place
    (board v1 to v2), a second Save As refused; closed and reopened from the cloud, the board shows the routed
    net and the folder holds exactly one .fprj, .fsch and .fbrd.
- **Library 3D models go where the run says** (page issue #126): `fusion_make_3d_package` and
  `fusion_build_library_3d` take `projectName` and `folderPath`; each .f3d is saved into
  `<projectName>/<folderPath or "Adom AI Workspace">/<task>` (folders created), found by name in the hub, never
  the active project. A missing project is refused (`project_not_found`), and a failed save with a named
  destination is reported, not uploaded to the fixed APS folder of another project. The reply's `savedTo` names
  the project and folder. Without `projectName` the mint is unchanged. Verified live: with no project open,
  ADS8588SIPM minted into Default Project/fusion-lane-tests/lib3d/issue-126, nothing new in Main.
- **DisplayLink warning** (page issue #121): when Fusion has detected an indirect display adapter
  (`IndirectDisplayAdapterDetected=1` in its `NMachineSpecificOptions.xml`), Fusion Electronics can die in its
  D3D12 canvas on any monitor. `fusion_readiness` now carries `displayWarning`, and the guided tour and
  `fusion_new_electronics_from_eagle` say so (reply and hint) before they start.
- **The EAGLE import stops on a crash dialog** (page issue #121): `fusion_new_electronics_from_eagle` checks for
  Fusion's D3D12 device-loss and "unexpected condition" dialogs while it waits for each Open dialog, before it
  types a path, while the path is submitted and while the design comes up. It stops at once, cancels its Open
  dialog and answers `fusion_graphics_error` (or `blocked_by_dialog`) with the dialog, instead of sitting at
  `stage: started` for minutes.
- Layer arguments take the standard's names: `F.Cu` / `B.Cu` as well as 1 / 16 (`fusion_route_net`, pours).
- Net names with `!` (active-low, `!CE`) are accepted by the routing verbs; they were refused as invalid.
- Tests: `tests/test_board_model.py` (16 cases over every demo board, against the standard's schemas, copied
  to `contracts/eda-bridge-standard/`), Rust tests for the sheet reader, bodies and aliases.

## 1.11.42 (2026-10-17)

- The "ADOM is driving Fusion" panel no longer sits at 0% (page issue #121). Most steps do not know their own
  percent, and the panel drew that as 0% for the whole step. It now fills by elapsed time against the step's
  usual duration on this machine, counts down the time left, and says "taking longer than usual" past it; a step
  with no estimate shows a moving "Working" bar and a seconds counter. Verified live in Fusion.
- The Dashboard comes back after a container restart (page issue #121). Where systemd runs and sudo needs no
  password, the installer adds `adom-fusion-dashboard.service` (Restart=always, port 8930). The dock button
  reuses that server. Elsewhere it starts from the dock button as before, and the installer says so.
- Release notes for 1.11.10 to 1.11.41 are filled in below.

## 1.11.41 (2026-10-17)

- Guided tour fixes (page issue #121):
  - A beat that fails stops narrating. A beat counts as failed when its action fails, its view cannot be
    confirmed on screen, or its motion never moves the view (after one retry). Before, a beat whose view never
    moved could still be marked `ok`. The caption says the step failed and why, and the tour moves on.
    `viewChanged` now counts any frame after a gesture, not only the last one.
  - Back and Exit are debounced: a repeat within 3 s, or while the same press is queued, is dropped, and so is
    any press for a stopped run.
  - Exit stops the narration, captions and remote at once. The step in progress unwinds at its next Fusion
    call, then cleanup closes the tour's documents.
  - Going back over the chip beat reuses the chip already minted instead of minting a second binding.
  - Once the remote is stopped or hidden, nothing shows it again for that run.
- The floating panel in the electronics editors is parked in the bottom-right corner of Fusion's window, over
  the layer list. Fusion ignores size requests for a floating panel, and its position call fails on a monitor
  left of the primary one, so the bridge moves the window itself. In the 3D view the panel stays docked right.

## 1.11.40 (2026-10-17)

- The optimized GLB's silkscreen is cropped to the board and keyed to clean white on transparent, so it no
  longer lands offset or carries the editor background (page issue #113).
- The bundled fab design rules use neutral names.
- The panel does not appear for a plain view switch (schematic, 2D board, 3D board).
- Board-editor traps added to the fusion-eagle-commands skill.
- Native Rust mechanical checkpoint from a community pull request (#19).
- The exe is signed as "Adom Industries, Inc.".

## 1.11.33 to 1.11.39 (2026-10-06)

- `fusion_remove_pours`: delete pours for a refine, picking far from every pad. Shift-delete takes the whole
  polygon (a plain DELETE took one vertex), verified by count, and the change is undone if a part or track moves.
- EAGLE import: waits up to 90 s for the queued import's Open dialog, never mistakes the schematic's lingering
  dialog for the board's, and restores a minimized Fusion in the background first (minimized, the dialog has no
  controls).
- `fusion_schematic_run`: one `USE` per library (Fusion cannot take a library out of use).

## 1.11.15 to 1.11.32 (2026-10-05)

- Board building from a netlist, end to end (the fusion-board-from-netlist skill): `fusion_board_seed` starts a
  board with the fab's rules and fixed parts, `fusion_schematic_plan` plans a grouped schematic, and
  `fusion_schematic_run` draws it live with complete, non-interactive commands (`ADD` proven against the .lbr so
  the picker never opens, `SET CONFIRM YES` so no prompt is drawn, sizes in mm).
- `fusion_board_source` (the live board as EAGLE XML, read-only), `fusion_board_bom` (MPN, manufacturer, Mouser,
  DigiKey and LCSC from the board's parts, plus the JLCPCB form), `fusion_ripup_nets` (pours stay, and their fill
  is restored and proven), `fusion_change_pours` (pen width and isolation in place, verified).
- `route_net`: the board's clearance rules decide, SMD and through-hole pads and vias are checked as their real
  copper, the via drill is checked only when the path has one, and angled segments land straight.
- EAGLE import types the full path into the Open dialog and is queued so its modal cannot block the call.
- The seat-takeover toast shows at most once per 24 hours.

## 1.11.10 to 1.11.14 (2026-10-04 to 10-05)

- `fusion_capture_library_views` photographs the bound 3D package (footprint and chip, opened by its URN) instead
  of Fusion's Create3DPackage generator, which showed only the footprint and left an unsaved tab (page issue #109).
- A library import interrupted by a restart no longer pins a stale 0% panel over Fusion.
- New verbs for the AI flow's Fusion lane: `fusion_move_parts`, `fusion_place_vias`, `fusion_apply_pours`,
  `fusion_pour_state`; the routing state now lists every part with its position, 3D binding and MPN.
- Contributing guide, demo videos and forum videos in the docs.

## 1.11.9 (2026-09-29)

- `fusion_aps_signin` always opens a browser. With no remembered browser profile it passed an EMPTY profile name, ab
  refused it ("no local browser profile matches ''"), and nothing opened, so arav-rog's cloud search could not be
  signed back in. The empty name is dropped, and if no profile opens, the consent opens in the machine's default
  browser through `desktop_open_url`; the localhost listener catches the callback either way.

## 1.11.8 (2026-09-29)

- Cloud search stays signed in. The APS access token was refreshed from up to 12 parallel search workers at once,
  plus the janitor and status calls from other processes, all replaying the same single-use refresh token.
  Autodesk rotates refresh tokens and can revoke the whole family when one is replayed, so ConfRoomROG fell
  signed out on 2026-09-24 and every daily run through 09-28 was red (page issues #100-#104). The refresh is now
  serialized across threads and processes, re-reads the tokens once it holds the lock (another refresher may have
  finished), writes tokens.json atomically, and records Autodesk's refusal in last_refresh_error.json.

## 1.11.7 (2026-09-29)

- The bridge exe is code-signed as "Adom Industries, Inc." with a trusted timestamp. Windows 11 Smart App Control
  blocks unsigned executables ("Part of this app has been blocked"); earlier releases were unsigned. The release
  now refuses to publish an exe whose signature does not verify.

## 1.11.6 (2026-09-23)

- `fusion_aps_search` and `fusion_aps_recent` declare a 120 s budget. A hub-wide search takes 20-30 s on Adom's
  hub (each folder's results are paged and filtered by name on the bridge, because Autodesk's own name filter is
  unreliable), and under the inherited 60 s default a slow moment at Autodesk turned into a timeout. The hints no
  longer promise "~2 s" cloud lookups; they say hub-wide takes 20-30 s and projectId makes it a few seconds.

## 1.11.5 (2026-09-23)

- Faster cloud search. `fusion_aps_search` fetched every project's top folders one at a time before searching (16
  round trips on Adom's hub), so a search took about 28 s and a slow moment at Autodesk pushed it past the 60 s
  budget (the only failure in the 1.11.4 suite run). Top folders are now fetched in parallel and cached for six
  hours, and a successful search no longer waits for the shortened-query searches that only a zero-result search
  reads.

## 1.11.4 (2026-09-23)

- `fusion_claim_seat` waits for Fusion to finish resuming after the seat modal before reading the account. It
  had reported a failure on ConfRoomROG while the claim had worked.
- `fusion-license-seat` explains the one machine-level requirement for hands-free sign-in: the default browser
  must carry the Adom extension (AdomLapper's default is Edge without it, so Fusion's request is invisible there).

## 1.11.3 (2026-09-23)

- `fusion_signin {method}` gets a FRESH request by itself when the pending one expired (Fusion expires them after
  about two minutes): at the sign-in screen Fusion holds no documents, so the bridge restarts it once and retries,
  instead of answering `stale` with a recipe.

## 1.11.2 (2026-09-23)

- `fusion_signin {method, account}` fixes from the first live run on AdomLapper: the account picks the browser
  profile SIGNED INTO that identity (its Google session is live, so no password step), clicks land on the real
  button rather than a wrapper element, a Google chooser row is picked by its `data-identifier`, and a provider
  click that does not move the page stops after two tries instead of ten.

## 1.11.1 (2026-09-23)

- `fusion_signin` takes `method` (google, apple, microsoft, facebook, password) and `account`, and remembers them.
  With a method, the bridge opens the Autodesk sign-in in a window it can drive and does the page itself: clicks
  the provider, picks the account in the provider's chooser, clicks Open Product. Without one, the reply is
  `method_required` with the methods on the page, so the AI asks the user once instead of guessing.
- Fewer toasts. `fusion_signin` toasts only when a human step is actually reached (a password), never for a choice
  the AI can ask about. The passive seat sweep no longer toasts a takeover the AI caused on purpose
  (`fusion_claim_seat` / `fusion_release_seat` within 10 minutes), and at most once an hour otherwise: two boxes
  trading the seat toasted the user twice in a minute.
- `fusion_claim_seat` warns that the box it took the seat from can grab it back through its own sweep, and says
  how to stop that (`fusion_release_seat` there, or `fusion_set_seat_takeover {enabled:false}`). `fusion_account`
  says plainly that "holds the seat" is a local reading: Autodesk lets a just-moved box stay authorized until its
  next reauthorize.

## 1.11.0 (2026-09-23)

- The Autodesk SEAT is first class: `fusion_account` (who is signed in here, the licence blob from Fusion's own
  Application.LicenseInformation, and whether this box holds the seat), `fusion_claim_seat` (take it: Fusion starts
  if needed and the "Active Sessions Exceeded" modal is answered in the background, the other machine suspending
  resumably) and `fusion_release_seat` (close cleanly, stay signed in, so the next box claims with no modal).
  Testing across ConfRoomROG, arav-rog and AdomLapper moves the licence constantly; it is no longer a recipe.
  New skill `fusion-license-seat`. The bridge still never types a credential: a box at the sign-in screen answers
  `needs_signin` with the exact `fusion_signin` call. 164 verbs.

## 1.10.43 (2026-09-22)

- `fusion_prewarm`, declared as `bridge.json` `prewarm.verb` with `requiresHostApp: true` (page issue #99). Adom
  Bridge 2.1.153+ calls it once per installed version, in the background. It confirms the add-in is synced into
  every Fusion add-in folder, readies the staging folder, and caches the PCB design-rules catalog. It never starts
  Fusion. 161 verbs.

## 1.10.42 (2026-09-22)

- The show verbs' tab picker only changes tabs when the design's container tab, or a different document, is in
  front. 1.10.40 also hopped from the 3D tab to the board tab and back, so `fusion_show_3d_board` while already
  in 3D took about 20 s instead of returning at once.

## 1.10.41 (2026-09-22)

- No slow verbs while a Fusion panel floats. With the design BROWSER undocked (a 600x800 tool window titled
  "Fusion360"), every verb that runs the window-etiquette envelope read that panel's contents through UI
  Automation several times, and a plain `fusion_get_preferences` took 15 s and then timed out in the daily run.
  Panel-shaped tool windows (taller than wide) are skipped before any read, control reads are cached per window
  for 20 s, and the bridge's own internal scripts (the preferences read, the show verbs' tab picker) go straight
  to the add-in instead of through the envelope.

## 1.10.40 (2026-09-22)

- The show verbs pick the right TAB of an electronics design. A design is four tabs with one name, told apart only
  by product type (the EcadDesign container, Schematic, Board, and the 3D Design). Activating by name could land on
  the container, where no editor switch works ("Not in Electronics editor (current: Electronics Design)").
  `fusion_show_schematic` now activates the design's Schematic tab and `fusion_show_2d_board` /
  `fusion_show_3d_board` its Board tab, with or without `documentName`, and also when the container tab is the
  one in front. Works with every add-in version, so no Fusion restart is needed.

## 1.10.39 (2026-09-22)

- Captures are delivered to container callers (page issue #98, finished). ab pulls a reply's `fullPath` only from
  the user-profile and temp roots, and the bridge wrote its own captures to `C:/tmp/conduit-screenshots`, outside
  them. They now go to `%TEMP%/adom-fusion-shots`, so ab pulls them and adds `localFullPath` / `localSafePath`.
- `fusion_take_screenshot` works with no arguments, as its contract always said: the default output path is in
  that folder. The add-in used to answer "No outputPath specified".

## 1.10.38 (2026-09-22)

- The window the dialog gate closed on 2026-09-22 was Fusion's own design BROWSER panel, floating and titled just
  "Fusion360" (seen live on ConfRoomROG, its UIA text is "BROWSER ToolbarFramework::QTView"). Fusion's dockable
  panels carry a `QTView` marker in the title or the UIA text; they are no longer listed as dialogs at all, so they
  are never closed and never reported as blocking (page issue #97).

## 1.10.37 (2026-09-22)

- The dialog gate can no longer close Fusion. `fusion_dismiss_blocking_dialogs` and the pre-verb dialog gate sent
  WM_CLOSE to every Fusion Qt window no rule recognised. On ConfRoomROG that hit a window titled "Fusion360": Fusion
  closed every document and ran `ExitApplicationCommand`, and the daily run went red (page issue #97; the same shape
  on 2026-09-03). An unknown window now gets WM_CLOSE only when it is plainly a dialog: smaller than the main window,
  not an EAGLE editor window, and carrying a Cancel/Close/OK/No-style button. Anything else is left open and
  reported (`leftOpen[]`, `notClosableWhy`), with a ledger entry `dialog_left_open`.
- Image replies use ab's delivery contract (page issue #98). Replies that carry a Windows image path under `path`,
  `savedTo`, `screenshot` or `postOpenScreenshot` also carry `fullPath`/`safePath` and `screenshots[]`, so ab pulls
  the picture to a container caller automatically. Old keys stay.
- `fusion_set_design_rules` takes `druText` or `druBase64`: a rules file that exists only on the caller's side is
  staged on the box and loaded. That replaces ab's removed CLI helper `fusion_load_design_rules`; its other removed
  helper, `fusion_apply_3rdpartyfab_rules`, is `fusion_set_design_rules` with no fab (page issue #96).

## 1.10.36 (2026-09-22)

- `bridge.json` lists all 160 verbs. It was hand-kept and had drifted to 65 names, including one that no code
  handles (`fusion_load_design_rules`), against 160 in `fusion_describe`. ab's `commands` listing and its
  `verbCount` read that list (adom/adom-bridge#211). `tools/freeze-contract.py` now writes it from the contract on
  every freeze, and the release refuses to ship when the two differ.

## 1.10.35 (2026-09-21)

- Export verbs switch to the right editor (page issue #95). An electronics design's schematic, board and 3D view
  are separate tabs with one name, and the export verbs write whichever editor is active. From the board editor a
  `.sch` export answered "No .sch file found inside .fsch ZIP container" and a `.fsch` export silently held only the
  board. `fusion_export_eagle_source` and `fusion_export_source` now switch to the Schematic Editor for `.sch`/`.fsch`
  and the PCB Editor for `.brd`/`.fbrd` first, and say so in `switchedView`. If the switch fails they answer
  `wrong_editor_active` with the fix.
- `fusion_activate_document` reports `sameNameDocuments` when several open tabs share the name and points at the
  show verbs, which reach a specific view.
- Cloud search setup uses `adom-wiki settings get|set <org> autodesk-aps` (adom-wiki 1.7.55+, from
  adom/adom-wiki-cli issue 59) in every hint and skill, instead of the raw API route and manual page pushes. With
  several orgs the hint says to ask the user which one.

## 1.10.34 (2026-09-20)

- Part numbers in Fusion-authored libraries are read. Fusion stores them as layered `<att_values>` (deviceset, then
  device) plus named `<att_set>`s, not EAGLE's `<technology><attribute>`. The parser only knew the EAGLE form, so
  `fusion_library_parts` and `fusion_show_attributes` reported such libraries as having no MPN or distributor
  numbers. Both dialects are read now, later non-empty values winning.
- `fusion_show_attributes` opens Fusion's own Variant Attributes table. EAGLE's `ATTRIBUTE` command in the device
  editor raises it directly, so the positioned-click recipe is gone: the bridge reads the table through UI
  Automation (full values), captures the dialog window, and Cancels it. `dialog:false` skips it (issue #93).

## 1.10.33 (2026-09-20)

- The sign-in window fix that works. ab parks the browser window at -32000 and returns no handle on either open
  path, so 1.10.32's "open it visible" did nothing. The bridge now finds the Autodesk window itself and moves it
  onto the primary monitor, in front, whenever a human is needed: always without the browser extension, and for a
  "Sign in" (credentials) page with it. A warm session under the extension stays in the background. The reply
  says `surfaced:true`.

## 1.10.32 (2026-09-20)

- APS for a whole org. The org's APS app ID (a public PKCE identifier, no secret) now has a home: the members-only
  `<org>/org-settings` wiki page, `integrations/autodesk-aps.json`. `fusion_aps_status`, `fusion_aps_signin` and
  `fusion_readiness` send an unconfigured box there first; `fusion_aps_set_client_id` returns `orgSettingsFile`, the
  exact JSON to publish when the org has none. New skill **fusion-aps-org-setup**. The skills no longer claim a
  Client ID is baked into the bridge (none ever was) or tell an AI to read a peer machine's config file.
- A consent window that needs a HUMAN opens visible on the primary monitor. It was opened "background" and parked
  at -32000,-32000 where no monitor could show it (arav-rog).

## 1.10.31 (2026-09-20)

- `fusion_walk_cloud_tree` and `fusion_search_cloud_files` are hard-blocked in the exe, as the contract has always
  said. The block lived in the Python sidecar and was lost in 1.10.0, so the add-in walk could run and hold
  Fusion's main thread for minutes (caught by the first verb-suite run on arav-rog, a box without APS). They now
  answer `deprecated_cloud_search_disabled` with the next step for that box's APS state. Nothing reaches Fusion.
- Hints that recommended the disabled walk now point at `fusion_aps_search`.
- Verb suite: user-neutral scratch folder, and `fusion_prove_result` accepts the honest "no run yet" answer.

## 1.10.30 (2026-09-20)

- Legacy window verbs answer again. `fusion_check_dialogs` (every dialog as text, buttons and hwnd),
  `fusion_dismiss_recovery`, `fusion_relocate_recovery` and `fusion_close` (alias of `fusion_stop`) are handled by
  the exe. Names that have no exe equivalent (`fusion_send_key`, `fusion_click_fusion`, `fusion_screenshot_all`,
  `fusion_window_info`, `fusion_detect`, `fusion_open_symbol`, `fusion_schematic_info`, `fusion_dark_mode` and a few
  more) answer `verb_retired` with `useInstead` and the exact replacement call, instead of `sidecar_absent`.
- Skills no longer teach the retired names (SKILL.md, fusion-driving, fusion-onboarding, fusion-libraries).

## 1.10.26 to 1.10.29 (2026-09-20)

- Captures always target the "Autodesk Fusion" window; the bridge's own activity palette can never be picked
  as the main window (it was, whenever Fusion was minimized).
- `fusion_show_attributes` reads each variant's attributes straight from the library file.
- `partsPlaced` (showcase and EAGLE import) reads the board's component count from the right key.
- `fusion_show_schematic` / `fusion_show_2d_board` / `fusion_show_3d_board` take `documentName` and activate it first;
  with a library or 3D design in front they answer `not_an_electronics_design` plus `openDocuments`.
- Add-in 1.0.20: `fusion_activate_document` tries every same-name match (a board and its 3D view share a name),
  exact names first.

## 1.10.25 (2026-09-20)

- Boards the tools make carry the part name on the top silkscreen under each footprint
  (`fusion_make_showcase_project`, `fusion_make_test_project`; `silkLabels:false` turns it off)
  (page issue #91). The EAGLE command skill lists the non-interactive `TEXT` form with its
  `GRID MM` warning.
- New `fusion_show_attributes {libraryPath, deviceset}`: the component's metadata with full
  values from the library file, plus the DEVICE editor opened and captured (symbol, Packages
  table, footprint and 3D previews). Fusion's own Attribute Sets table is an opaque Qt panel;
  the reply carries the positioned-click recipe and the recording note (page issue #93).
- Daily verb test: one retry when another session holds the add-in for a moment, and an honest
  "No active Fusion Design" counts as the BOM guard's refusal (page issue #92).
- Skills brought in line with 1.10.13 to 1.10.24: `fusion_state` replaces the verb that was
  never in the contract, staging replaces the 8.3 short-path text, the default restart policy
  and the dialog ladder are stated where the old toast was described, native viewport frames,
  the async import status verb, caller-aware prove, the heatsink-via lint.
- README and the sub-readmes regenerated and updated (verb catalog from the contract).

## 1.10.24 (2026-09-20)

- Showcase schematic: symbols are pitched by their own measured width (they overlapped at a fixed 45 mm on the ToF sensors).

## 1.10.23 (2026-09-20)

- `fusion_make_showcase_project` embeds the library VERBATIM (whole block) in the generated schematic and board and is async (runId, `fusion_eagle_import_status`); the two documents are named `<name>-sch` and `<name>-brd`, the board opens last so it is active for the 2D and 3D views, and each view is fitted. Found on the Adom_ToF library: a Fusion-exported .lbr rebuilt from extracted pieces made Fusion refuse the schematic ("has errors and cannot be opened") and instantiate nothing; the same block verbatim opens with every part. `fusion_make_test_project` still uses the piecewise rebuild and fails the same way on Fusion-exported libraries (follow-up).

## 1.10.22 (2026-09-20)

- New `fusion_make_showcase_project {libraryPath, devicesets?, name?}`: every deviceset of a
  library (or the ones named) placed on a fresh schematic and board, no nets, the library's
  bound 3D urns carried across; generated as EAGLE source and opened dialog-free. The answer to
  "make a schematic and board to show off these three sensors" (John, 2026-09-20).
- `fusion_new_electronics_from_eagle` is async by contract (page issue #90): a runId at once,
  `fusion_eagle_import_status {runId}` for the record, `fusion_get_app_state` keeps answering.
  The record and the final reply carry documentName, partsExpected (elements in the .brd) and
  partsPlaced (read back from the board); success needs parts on the board
  (`nothing_instantiated` otherwise, with the likely causes).
- `fusion_library_parts` no longer calls same-net pads a short (page issue #89): pads that share
  a base name (B4 and B4@2, heatsink vias on the exposed pad's own net) are reported as an info
  line, `heatsink_vias`; `pad_overlap` stays for genuinely different names. Unit test.
- Skills: fusion-bridge and fusion-electronics open with the schematic front door, the showcase
  verb, and the one dialog-driven verb named as such.

## 1.10.21 (2026-09-20)

- `fusion_schematic {action:"open"}` lands on the electronics DESIGN: when a name search opens the design's 3D PCB or a mechanical file (DesignProductType, no schematic) it retries the exact cloud name once and otherwise answers `not_an_electronics_design` with the candidate documents, instead of Fusion's bare "Please open or create an electronics design".

## 1.10.20 (2026-09-20)

The obvious path to a schematic, and no more dialogs left on screen (John, 2026-09-20):

- New `fusion_schematic {action: open | create | show}`: the one verb an AI reaches for.
  `open` lands in the schematic editor from a cloud fileName, urn or query, a local EAGLE .sch
  (with its sibling .brd when present so parts instantiate, or alone and dialog-free with
  withBoard:false), or a Fusion-native .fsch. `create` writes a blank EAGLE schematic on the box
  and opens it as a new local electronics design with no dialog. `show` switches the open design
  to its schematic view. Every reply says which verb did the work (`via`).
- `fusion_new_electronics_from_eagle` is the one dialog-driven verb (Autodesk gives no API for a
  sch+brd import). It now refuses before touching a dialog when either file is missing on the
  box (`eagle_files_missing`), cancels a stale Open dialog from an earlier attempt, and cancels
  the dialog itself when it does not accept the file (`open_dialog_stuck`), so nothing is ever
  left on the user's screen. The reply says the dialogs were driven.
- Dialog inventory: native Windows Open/Save dialogs (class #32770) are read and expired like
  any other; Fusion's own tool windows no longer count as dialogs. 1.10.17 to 1.10.19 refused
  every verb on a box whose Fusion had its Data panel open (two owned popups, no dialog).

## 1.10.19 (2026-09-19)

Contributing back is part of the product (John: "I really want to encourage the community to
help contribute to this bridge"):

- New user skill `fusion-contributing`: where the source lives (the wiki page is the repo,
  GitHub mirrors it), how to file with evidence, clone, the layout, the common contributions
  (a dialog rule, a hint, a verb, a skill), building the exe, trying it on your own box behind
  a dev pin, and sending the PR with adom-wiki.
- Every failing reply from the exe carries `_contribute` with the same pointer, and
  `fusion_describe` carries a `contribute` block, so an AI that just worked around the bridge
  is told to file or fix it without having to read a skill.
- The fusion-bridge skill opens with the same invitation.

## 1.10.18 (2026-09-19)

- After the gate expires a dialog it re-probes the add-in (up to 20 s) and either runs the verb or answers `dialog_expired_retry`; it no longer falls through to the add-in restart path, which the 1.10.17 live test still hit (the box was gone, the add-in had not resumed yet, and the gate read that as "not loaded").

## 1.10.17 (2026-09-19)

Answers to the Adom_ToF library run (page issues #87 and #88):

- A dialog in front of Fusion is never a reason to restart it. The gate and the timeout
  diagnosis now read every dialog through UIA (title, text, button names), expire it safely
  without a person (the known ones through their rule, the unknown ones with WM_CLOSE, which
  is Cancel or the X and never saves or discards), and run the verb when that clears it
  (`dialog_expired_retry` after a timeout). Only a dialog that survives comes back, as
  `blocked_by_dialog` with `dialogs[] {hwnd, title, category, text, buttons, resolution}` first
  and ab's parent/child screenshots second, so the AI answers it by button name. The 90 s
  auto-restart over a modal from 1.10.6 is gone.
- The relaunch itself: the add-in gate treated a long operation in flight (a STEP bind that
  starves the add-in for 20 to 60 s) as "not loaded" and restarted Fusion. That phase is now
  `fusion_addin_busy` like the other busy phases. Chains (`fusion_create_library`,
  `fusion_install_library_bundle`) never restart Fusion on their own: `allowRestart:true` with
  `restartReason` is the only way, recorded in `restarts[]` on `fusion_prove_result`.
- The save-to-cloud dialog is pre-empted: `close_document` and `close_all_documents` drain
  the cloud upload for up to 180 s (was 45 s, close_document only) before closing, and defer
  instead of closing over a live upload.
- Prove is decided by the caller: an AI thread binds and places only (`verdict:
  bound_not_proven`, `partsBound`, the library path); web control proves. `prove:true/false`
  always wins; the reply says what prove costs before it starts, and the banner's final line
  says "bound; proof skipped" instead of a proof count.

## 1.10.16 (2026-09-19)

- The sibling-folder fallback of `fusion_aps_search` resolves index hits (which are VERSIONS)
  to their item before reading the parent folder, and runs both shortened queries in parallel
  with the main scan, so the whole answer fits one relay call.

## 1.10.15 (2026-09-19)

- `fusion_aps_search` zero-result fallback, third cut and the one that finds the file: the
  issue's own suggestion, automated. A shortened query ("Pincer Moving" for "Pincer Moving
  Large") runs speculatively in parallel with the real one; when the full query comes back
  empty, the parent folders of those related hits are listed through the Data API (not the
  index) and names matched directly. A two-level walk runs only when projectId scopes the
  search. Results carry projectId, folderId and lineage.

## 1.10.14 (2026-09-19)

- `fusion_aps_search` zero-result fallback now walks folder CONTENTS through the Data API (the
  index-backed "recent items" scan in 1.10.13 missed the same file the index missed): the
  scanned top folders, two levels deep, at most 80 listings, names matched directly, results
  with `folderId` and lineage. Verified on the Depth Lab file from issue #85.
- The "Fusion restarted by the bridge" toast is gone (John: "fairly pointless"); the restart
  stays in the action ledger with its summary.

## 1.10.13 (2026-09-19)

Answers to the three Depth Lab reports (page issues #84, #85, #86):

- `fusion_state` reports geometry separately from window pixels. It now exports the frame
  Fusion itself renders (`nativeViewport {path, ok, bodies, productType}` through
  activeViewport.saveAsImageFile, no foreground), counts bodies through the API (`geometry`),
  and measures the window capture (`windowPixels.dominantColorFraction`, `blank` above 0.97).
  A blank capture of a Design viewport under WGC now says so and points at the native frame;
  `viewport:"native"` skips the window capture entirely.
- `fusion_aps_search` never lets an index zero stand alone: the reply carries `data.fallback`
  (a name scan of the newest 400 items across the scanned projects), `data.completeness`
  (the index lags and tokenises), and a hint to browse a known sibling's folder, scoped, with
  no cloud walk.
- Opening a mechanical design is no longer "STOP - WRONG FILE". The electronics-parent
  warning fires only for a DesignProductType that looks like the 3D leaf of an electronics
  chain (name evidence) or when the caller passes `intent:"electronics"`; `intent:"mechanical"`
  confirms the open. Stale hints that pointed at `fusion_window_info` (not in the contract)
  now point at `fusion_state {screenshot:false}`.
- Daily test cron: picks the wiki CLI that actually has `issue create` by probing (two
  lineages live on the dev box), so a red run files its issue again.

## 1.10.12 (2026-09-11)

- The `expectDocument` guard is back on every add-in verb. The Python bridge refused a verb with
  `wrong_document` when the named document was not the active one (issue #289); the port had
  dropped it, so `fusion_run_modeling_script {expectDocument:"X"}` ran against whatever was
  active. Restored in the exe pass-through (fails open when the active document cannot be read).
  The daily verb test had flagged it every day since September 7.
- Daily verb test: runs against ConfRoomROG (the test box) instead of AdomLapper, uses the
  wiki CLI that knows `issue create` when a run is red, the four negative tests that expected
  "errorCode None" now expect the refusal they get, and the routing, lifecycle and narration
  verbs are triaged (routing guards as negative tests, live routing as the weekly demo).

## 1.10.11 (2026-09-10)

No more DOS 8.3 paths, anywhere (John: "we should never run into this"):

- Fusion's `executeTextCommand` is EAGLE's space-delimited command line and rejects a quoted
  path, so a file under a path with a space (the bridge cache under "Adom Bridge", a profile
  such as "John Lauer", "Program Files") used to be handed over as its 8.3 alias. Fusion then
  titled the document after that alias ("0603WA~1"), which is what broke the mint checks in
  1.10.9 and 1.10.10. Now such a file is STAGED: copied with its same-stem siblings (.sch/.brd,
  .lbr/.step, rule files) into a space-free folder (the bridge state dir, else LOCALAPPDATA,
  TEMP or the drive root) and Fusion is handed the copy, so the document keeps its real name.
  Outputs that Fusion writes (`fusion_save_lbr`) are written to a staged target and moved to the
  requested path afterwards. Paths without a space are untouched.
- Applies to the exe (`fusion_open_lbr`, the test project, `fusion_save_lbr`) and to the add-in
  (`fusion_open_board`, `fusion_open_schematic`), which had its own GetShortPathNameW fallback.
  `Ctx::host_paths_short` is deprecated and unused. Unit test covers staging, siblings, the
  space-free pass-through and the output move.
- The 8.3 title matching from 1.10.9/1.10.10 stays as a harmless safety net for a document that
  was opened by hand through a short path.

## 1.10.10 (2026-09-10)

- The 8.3 title match also accepts the window-title form ("0603WA~1* (Adom) - Autodesk Fusion"), which the front-window check reads; 1.10.9 fixed the document name but not that check, so the mint still reported library_not_active.

## 1.10.9 (2026-09-10)

- Library mints work again on a bridge cache under a path with spaces: Fusion titles a library
  opened through the DOS short path by its 8.3 stem ("0603WA~1" for 0603WAF3300T5E.lbr), and
  the active-library check, the duplicate-close in `fusion_open_lbr` and the footprint check in
  `fusion_make_3d_package` only knew the long name, so every mint of a sample under
  `Adom Bridge/bridges-cache` failed with library_not_active and left duplicate library
  documents open. Both title forms count now (unit test).

## 1.10.8 (2026-09-10)

Explicit model placement for component 3D packages (page issue #83, TMF8829 Depth Lab):

- `fusion_make_3d_package` (and each `parts[]` entry of `fusion_build_library_3d`) takes
  `placement: {translation:[x,y,z] mm, rotation:[rx,ry,rz] deg, scale:1}`, the manufacturer's
  authored transform. Rotation is applied about X, then Y, then Z (extrinsic, about the origin),
  then the translation. It bypasses every heuristic: no bounding-box reorientation, no
  centering, no pin-1 rotation. The reply returns `placement.appliedMatrix` (row-major 4x4 read
  back from the occurrence, translation column in mm), `placement.verification`
  (verified | mismatch), `placement.heuristics`, and `source {path, sha256, bytes, preserved}`
  for the STEP Fusion read. Non-finite numbers, wrong shapes, units other than mm/deg and any
  scale other than 1 are refused before Fusion is touched (`invalid_placement`,
  `unsupported_scale`).
- `orient:false` now means what it says: the model is imported exactly as authored, and the
  pin-1 mark routine no longer rotates it (it used to survive orient:false). The pin-1 reply
  says `skipped_orient_false` / `skipped_explicit_placement`; a default mint still reports
  `rotated` / `aligned` plus a warning that heuristics were used.
- Unit tests cover the refusals and the three script shapes; the mint script golden is
  regenerated for the two new template lines.

## 1.10.7 (2026-09-08)

- `fusion_set_design_rules {action:"list"}` answers on a machine without Fusion: bridge.json now
  lists set_design_rules in `detect.hostAppOptionalVerbs`, so ab's host-app gate lets the
  wiki-backed list through (found by the promote gate on winvm; apply and export still need a
  board open and say so themselves).
- Gate record for 1.10.6 in docs/preprod/fusion-bridge-1.10.6-2026-09-08.md.

## 1.10.6 (2026-09-07)

Fusion restarts are the bridge's job, not a question for the user (John: "the AI is the only
thing controlling Fusion ... all the open docs are just the AI's docs ... let it restart it"):

- New pref `restartPolicy` (prefs.json, or env ADOM_FUSION_RESTART_POLICY), default `auto`.
  Under auto the "Fusion needs a restart / Save and restart" toast is never raised; the
  restart-info walk answers `autoRestart: true` even when the add-in cannot list documents or a
  never-saved document looks like a person's, and the gate restarts Fusion in the background
  (`addin_restarting`). `ask` keeps the old toast and the 30-minute wait for shared machines.
- `fusion_restart_for_addin` under auto never refuses: documents Fusion can save are saved,
  never-saved scratch is discarded (named in `discarded` and in the ledger), and when the add-in
  cannot save at all the restart is a kill plus start. It ends with a plain informational toast
  on that box, "Fusion restarted by the bridge", with no buttons.
- A modal the bridge has no rule for no longer parks the AI forever. When a verb times out
  because the add-in's main thread is blocked by a dialog, the diagnosis now counts how long
  it has been blocked; after 90 s (seat and sign-in dialogs excluded, they heal on their own)
  the bridge restarts Fusion itself and answers `fusion_auto_restarting` with the dialog list
  and screenshot, so the AI polls readiness and re-runs instead of waiting for a person who has
  gone swimming. Before 90 s the reply says how long it has been blocked and that a restart is
  coming, instead of "do not restart Fusion".

## 1.10.5 (2026-09-07)

Skills catch up with 1.10.1 to 1.10.4 (no runtime change):

- fusion-eagle-commands and fusion-no-modal-dialogs: `SHOW <name>` is allowed and returns; only
  a bare `SHOW` is blocked.
- fusion-autodesk-signin and fusion-web-control: a board verb sent while Fusion is signing in
  answers `needs_signin` at once; it is the same signal as readiness, never a restart.
- fusion-electronics: read `ruleWarnings` from `fusion_set_design_rules` and export the loaded
  profile to read values back.
- fusion-bridge-dev: `_timing.scope` and the sign-in gate.
- Operational note: an in_place_merge install never deletes files the new package stopped
  shipping. A cache that still holds `server.py` from a 1.9.x dev pin makes Adom Bridge spawn
  Python instead of the exe (ConfRoomROG today; adom/adom-bridge#162). Symptom: exe-owned verbs
  such as `fusion_routing_state` answer "Unknown command" and no reply carries `shell: rust`.
  Remedy until ab fixes spawn selection: delete the stale `server.py`, kill the Python process,
  let ab respawn.

## 1.10.4 (2026-09-07)

- `fusion_describe` now lists fourteen verbs the add-in already served but the frozen contract
  never declared, so callers could only find them in prose: `fusion_open_board`,
  `fusion_open_schematic`, `fusion_open_electronics`, `fusion_import_electronics`,
  `fusion_export_source`, `fusion_export_eagle_source`, `fusion_export_lbr`,
  `fusion_export_board_image`, `fusion_set_parameter`, `fusion_list_text_commands`,
  `fusion_create_cloud_folder`, `fusion_delete_cloud_file`, `fusion_export_cloud_file` and
  `fusion_check_recovery`. Argument names come from the handlers, not guesses; the contract is
  156 verbs (was 142). Found while reproducing the 1.10.2 acceptance, which used
  `fusion_open_board` from the README while `fusion_describe` denied it existed.
- Not added on purpose: `take_silkscreen_screenshot` (not in either shell's allowlist, would
  404) and `import_file` (the internal name behind `fusion_import_step`).

## 1.10.3 (2026-09-07)

Answers to the 1.10.2 acceptance on AdomLapper (page issues #81 and #82):

- `fusion_electron_run` lets an explicit `SHOW <name>` through (it highlights and returns);
  only a bare `SHOW`, which waits for a click, stays blocked. The published videos used the
  explicit form through `fusion_execute_text_command`, which was never guarded; the two
  paths now agree.
- `fusion_routing_drc` reports `layer`, `x`, `y` (mm) per error, a `summary` by description,
  and a note when Overlap warnings are same-signal joints.
- `fusion_set_design_rules` scans the downloaded profile and returns `ruleWarnings` for a
  same-signal Copper Clearance over an all/all or trace scope. The 3rd party fab 2L profile (and
  every other generated profile on adom/pcb-design-rules) carries "Same-net track spacing"
  (0.25 mm, all/all, samesignal=yes); on the six-net fixture native DRC reports it as 31
  Overlap warnings at the 13 pad entries and 18 trace/via joints. With that one rule disabled
  the identical board is clean (0/0/0). A fix for the generator is proposed on that page.

## 1.10.2 (2026-09-07)

- The `fusion-routing` skill is the general board-engineering skill from wiki PR 9
  (docs/general-routing-engineering, merged 2026-09-07 18:17), with its two references
  (electrical priorities, copper and thermal design). The 1.10.1 release push had put the
  older demo-oriented copy back over it for thirteen minutes; this restores PR 9's text and
  ships it in the package.

## 1.10.1 (2026-09-07)

Sign-in state answers fast instead of masquerading as an add-in fault (page issue #79):

- Every add-in verb now short-circuits with `errorCode: needs_signin` (well under a second)
  while Fusion's main window says "Signing in" / "Welcome to Fusion". Before, the gate probed
  the unloaded add-in for 5 s, ran the 20 s hang probe, then the restart-info walk, and
  returned `addin_restart_required` after 60 s+ for a state `fusion_readiness` already knew.
  The reply carries `needsSignin`, `mainTitle` and a hint that points at the sign-in skill,
  not at a restart.
- `sidecar_absent` names the verb it could not route and says it is a verb-name problem
  (the exe ships alone since 1.10.0), not a Fusion or add-in problem.
- `_timing` carries a `scope` line: one exe pass-through of the verb, excluding the relay hop,
  ab's queue and any earlier pass-through a composite verb made, so a 24 s `_timing` under a
  68 s caller wall clock is explained rather than contradictory.
- The add-in's lock watchdog logs to `~/.adom/fusion-bridge/addin-watchdog.log` instead of
  calling Fusion's API from its own thread (issue #76 audit, commit 71d0b3e).
- The routing skill's wording on checkpoints and the thermal-reroute sources is current.
- Stale `bridge_list` version rows after an install are ab's registry, tracked as
  adom/adom-bridge#160 (page issue #80).

## 1.10.0 (2026-09-07)

Live per-net trace routing from the Astra work lands in the released bridge, merged from
`feature/per-net-trace-routing` and `fix/document-save-hints` (PR 7 on adom/fusion-bridge):

- `fusion_routing_state`, `fusion_route_net`, `fusion_routing_drc` and `fusion_plan_route`:
  read the live board, create explicit copper trace by trace with board and revision guards
  and exact copper read-back, run native Autodesk DRC through UL_BOARD.errors, and plan a
  bounded two-pad orthogonal path. Two-layer geometric preflight; native DRC is the final word.
- The routing skill (`skills/fusion-routing`) now asks which manufacturer the board is for,
  loads and reads back that house's rules through `fusion_set_design_rules`, and keeps the
  stricter of the rules and the user's electrical constraints.
- Every add-in reply carries read-only `documentHygiene` and a save/cleanup hint when a
  document needs saving; nothing is saved, closed or discarded automatically.
- The activity palette gains the routing panel (selected connection, calculated route,
  measured checks, recent operations) fed by the demo's event stream.
- `demo/routing/` ships the fixture, plans, recorders, audits and evidence behind the Fusion
  routing videos on adom/codex (the videos themselves stay on the wiki page).
- Also since 1.9.366: the electronics tour rebuilt around gestures Fusion renders, the
  palette parked off the drawing, slow verbs answering `_timing`, the small taskbar badge in
  every path, and the dashboard's Send to AI through Hydrogen's AI picker.

Not included: the thermal-bottleneck reroute scripts (video 12) remain unpublished on the
Codex side, and the shared EDA skillpack is not yet an installable package.

## 1.9.76 - 2026-08-18 (ralph, dashboard)

- **"Fusion is opening right now" LED (John's ask).** A launch takes 30-90s and the board used to
  show a bare red dot the whole time, so the user could not tell whether anything was happening.
  New pulsing-amber `starting` state, marked SYNCHRONOUSLY when the launch is requested (so the
  very next status already says it, no red flash), with the sentence "Opening Fusion right now -
  launching in the background and waiting for its bridge add-in."
- **A failed launch explains itself** instead of silently reverting to red: the LED carries why
  (add-in never came up / the attempt did not take - seat busy elsewhere or a licensing dialog).
- **The tour finally SHOWS its screenshots.** The dashboard only counted them in a toast; it now
  renders a live gallery, streaming each step's capture as it completes via the bridge's
  stepShot + sinceShot contract (the same one built for ab), click to open full size.
- **Close Fusion auto-escalates and verifies.** When the graceful close is blocked ONLY by the
  bridge's own generated test docs (bridgeDocsOnly), it force-closes for the user, then confirms
  by readiness rather than trusting fusion_kill's flag (which reports failure on closes that
  actually worked).
- **Import part survives a busy add-in**: the open is retried for ~2.5 minutes when another
  session's long verb holds the main thread, instead of returning a rejection.
- **`force:true` now really forces** an auto-green run (the debounce was still swallowing it).

## 1.9.74 - 2026-08-18 (ralph, dashboard)

- **The board no longer lies while the bridge is busy.** Driving the guided tour from the UI
  exposed two honesty bugs in one screenshot: the header regressed to "bridge ?" and EVERY LED
  flipped red while the tour was visibly running (a status probe that times out under load was
  being read as "everything off"). Now: the server keeps a last-verified payload per target and
  serves it with `stale:true` + `busyReason` when a probe cannot answer, the header never
  regresses a known version, and a running-but-not-answering add-in renders AMBER "busy or still
  starting" with the timestamp it was last verified - never red "not serving".

## 1.9.72 - 2026-08-18 (ralph)

- **Design surfaces regenerate their project on demand.** Staging cached itself as complete even
  when `make_test_project` failed (it can fail if Fusion is mid-restart), so every later design
  click paid a full re-heal and still failed. The staging cache now records `designReady`, and a
  design click regenerates just the project - after calling ensure_ready first, which the cached
  path was skipping (a doctrine gap in my own new code, caught in the same loop).
  Verified: schematic went from failing to opening in 221s including a Fusion relaunch.
- Dashboard/bridge back in version lockstep (both 1.9.72).

## 1.9.71 - 2026-08-18 (ralph)

- **The DESIGN surfaces survive a bridge respawn.** show_project / show_schematic / show_2d_board /
  show_3d_board resolve through the in-memory `_TEST_PROJECT` handle, which every bridge_install
  wiped - so all four failed with "query or urn required" until something re-staged (found by
  driving all eight surfaces: 3/7 passed, the 4 design ones failed). The handle now persists to
  bridge prefs and is restored at import when both generated files still exist on the box.
- Dashboard: the surface remediation also re-stages on "make_test_project / urn required / no
  design" errors, not just library-shaped ones.

## 1.9.70 - 2026-08-18 (ralph)

- **fusion_state imageAs:base64 now DOWNSCALES instead of refusing.** On a PIL-less bridge the
  capture is a ~24MB BMP, which blew the 4MB inline cap, so every Dashboard surface card came back
  with no image at all (found by driving the Library surface end to end). It now falls back to the
  stdlib downscaler (<=1280px) exactly like the tour gallery, and says so in screenshotNote.

## 1.9.69 - 2026-08-18 (ralph loop findings)

- **Driving badge: diagnostics + transport fallback.** The badge silently never appeared on one box
  while working on another with identical code. fusion_state.drivingBadge now reports `enabled` and
  `lastError` (unavailable / timeout / error / missing asset), the desktop_taskbar timeout went
  6s -> 15s, and when the in-process direct API is unavailable the bridge now falls back to
  shelling ab's CLI instead of failing silently.
- **Dashboard, from the ralph pass:** status verbs now fan out in PARALLEL (a cold remote box took
  36.7s serially; warm latency is now ~3.0s), skeleton LED cards render instantly so the Status
  section is never blank while the first status is in flight, every button carries a tooltip
  (16 had none), a `window.__dash` handle exposes live page state for AI drivability, and
  auto-green's single-flight/debounce is now PER TARGET - a run for one box was silently
  blocking the other while the endpoint still answered "started".
- **auto-green heals a stuck add-in**: Fusion running but the add-in not serving used to sit red
  forever (auto-green only tried sign-in); it now shares ensure_ready's healing path. Proven live:
  a stuck ConfRoomROG went green in 59s.

## 1.9.67 - 2026-08-18

- **Window sizing now CONVERGES** (found by measuring the previous release live): the bridge sized
  Fusion to 92% at t+75s, and Fusion then restored its OWN saved geometry at t+90s, overriding it.
  Here that landed larger, but on a box whose saved geometry is small Fusion would have won and
  the window would still be tiny. The sweep now retries the size check for ~2 minutes and stops
  permanently as soon as the window reports "already large enough" or user-maximized, so whoever
  writes the geometry last, the window ends up big.

## 1.9.66 - 2026-08-18

- **Fusion's window is now a usable size** (John: "why is your fusion window always kind of small
  and not like 90% of the screen size?"). Fusion reopens at whatever geometry it last had, which
  after an automated launch is a small default - bad for the human looking at it and wasteful of
  screenshot resolution. The bridge now grows it to ~92% of the monitor WORK AREA, centered,
  after launch and on the background sweep (once per window). Done with SetWindowPos +
  SWP_NOACTIVATE|SWP_NOZORDER - NOT ShowWindow(SW_MAXIMIZE), which would activate the window and
  break the background etiquette. A window the user maximized, or already sized to >=60% of the
  work area, is never touched. Sticky opt-out: fusion_set_window_sizing {enabled:false}.

## 1.9.65 - 2026-08-18

- **THE AUTOMATION DOCTRINE, enforced in code (John: "never make the adom user do anything
  manually").** Every Dashboard action that needs Fusion now calls `ensure_ready()` first: it
  joins an in-flight auto-green instead of duplicating it, waits with narration, and SELF-HEALS
  while waiting - clears blocking dialogs, and restarts Fusion once if it is running but the
  add-in never came up (waiting longer never fixes a missing add-in; it only loads at launch).
  Surface clicks that still fail get a remediation pass (dismiss dialogs, re-ready, re-stage the
  part) and one retry before any error is shown. The old "Fusion is not ready - press Setup"
  refusal is gone; it was a failure of this bridge, not an instruction for the user.
  PROVEN live: with Fusion KILLED, a single Symbol click launched it, staged IN-S63BTG from the
  wiki, bound the 3D, generated the test project, and rendered the symbol - 5m30s, no human step.
- Doctrine recorded in CLAUDE.md and dev-skills/fusion-dashboard so it survives this session.
- Settings: the white halo around the modal was Chrome's default focus ring, drawn in currentColor
  because the dialog is focused on open to keep it scrolled to the top; suppressed on the
  container only (children keep their teal focus-visible rings).

## 1.9.63 - 2026-08-18 (dashboard only; bridge zip stays 1.9.62)

- **The house scrollbar, everywhere** (John: "that scrollbar is hideous"). Thin, transparent
  track, translucent pill that only turns teal on hover - applied globally, not just in the
  modal. The Settings dialog no longer scrolls ITSELF: the radius element is overflow:hidden and
  an inner body scrolls with an 8px track inset, so the bar can never slice through the rounded
  corners. New adom-ui-design `scrollbars` sub-skill carries the full recipe (the existing one
  was buried in shotlog's dev skill, unavailable to other apps).
- **Settings modal mechanics completed** (John: click-outside was missing, X unknown, dim
  unverified): X button in the header, click-outside closes via a backdrop hit-test (a naive
  handler also fires on the dialog's own padding), Escape still native, backdrop dim verified at
  rgba(0,0,0,.55). All four drive-tested live. The four behaviors are now mandatory in the
  settings-layout skill, along with the Settings-never-Preferences naming rule (our own copy had
  one "preferences" left, now fixed).

## 1.9.62 - 2026-08-18

- **AUTO-GREEN: opening the Dashboard now MAKES everything ready (John's ruling).** "The meaning
  of opening the dashboard is that you open fusion and all the other services." On open (and on
  Setup) the Dashboard drives every facet to green in the background: launches Fusion if closed,
  self-drives the Autodesk sign-in to the password wall, enables the MCP server, signs in to APS
  when a client id is configured - narrating each step as a toast and stopping honestly at any
  wall that needs a human. Sticky opt-out toggle returns the old status-only behavior.
  Live-verified: closed Fusion to all seven LEDs green in 86s.
- **SURFACES demo on a DIFFERENT part**: eight content-type buttons (symbol, footprint, 3D
  package, library, schematic, 2D board, 3D board, project) driving aravk/in-s63btg (green 0603)
  instead of web control's IN-S42ATR (red 0402), so the click-through proves the flow generalizes.
  First click stages the part (clone -> send_files -> bind_library_3d -> make_test_project),
  later clicks just switch surfaces; each card renders its own fusion_state capture.
- **Driving badge: race fixed + user toggle.** `fusion_start` is dispatched BEFORE Fusion's window
  exists, so the single badge attempt found no hwnd and the badge never appeared after an
  auto-launch (John caught it). The ensure path now retries for ~90s and the janitor self-heals a
  missing badge whenever driving is recent and Fusion is up. New sticky `fusion_set_driving_badge`
  verb + Settings toggle (off clears it immediately).
- **Settings restyled as ah settings cards** (new adom-ui-design `settings-layout` skill): elevated
  cards with a teal left accent, inline live status per card, and descriptive prose that explains
  the MECHANISM of each group (what auto-green actually does step by step, how the seat take-over
  answers Autodesk, how the badge is painted and when it sheds), plus the storage-location footer.

## 1.9.61 - 2026-08-18

- **windowEtiquette annotation now covers ORCHESTRATED verbs** (show_*, make_test_project, tour,
  bind_library_3d, ...): a thin dispatch wrapper guarantees every backgrounded window-raising
  result carries the marker + re-call shape - the inner closure only covered direct/add-in paths.
- **Zombie-window gate on the save-prompt sentinel**: an UNATTRIBUTED "Save" dialog is only
  escalated (Escape) when the add-in main thread is actually blocked; responsive-thread ghosts
  (ConfRoomROG's black no-UIA windows) are left alone instead of escape-spammed every sweep.
- **`fusion-dashboard open [--pup|--webview]` shipped** - the founding contract's third CLI verb:
  --pup opens the Dashboard in a BACKGROUND pup window on the user's desktop (taskbar flash, never
  foregrounded); --webview prints the URL + the webview-skill instruction (panel placement is the
  AI's call, never a CLI guess). Auto-discovers the serving port and the proxy URL.
- **Graceful stop hardening**: the clean-close document pass retries once (3s + 10s budget)
  instead of silently skipping on a transiently-busy add-in - skipping was what let WM_CLOSE hit
  the unsaved-doc Save modal. On failure, `bridgeDocsOnly:true` + a kill-is-safe hint when every
  blocking doc is a bridge-created throwaway, so callers can escalate to fusion_kill confidently.

## 1.9.60 - 2026-08-18 (dashboard only; bridge zip stays 1.9.59)

- **Every interactive element now acknowledges the human (John: the UI must say "yes, you just
  hovered me / yes, you just pressed me").** Added across the Dashboard: :hover border/background
  shifts, :active pressed states (translateY + scale compress, darker fill), iconbtn press-shrink,
  pill hover/press, toggle hover ring + iOS-style knob stretch on press, prompt-row hover,
  input focus ring, and :focus-visible outlines for keyboard users. Transitions tuned (150ms
  hover, 60ms press - a press must feel instant).
- Companion: new `input-feedback` sub-skill in adom/adom-ui-design (the acknowledgment doctrine).

## 1.9.59 - 2026-08-17

- **Janitor tick section-scoping (found by the badge-shed watcher within the hour).** The seat
  section's early returns exited the whole 20s tick on the NO-dialog path - the normal case - so
  the badge idle-shed never ran and the save-prompt sweep only ran when a seat dialog happened to
  be up (also the true reason yesterday's standing Save prompt survived the sweep). The tick's
  three jobs (seat takeover, badge shed, save sweep) are now self-contained sections; none may
  return.

## 1.9.58 - 2026-08-17

- **Driving badge sheds on idle, nb-style (John's call).** Every non-passive verb stamps a
  driving-activity clock (passive reads - readiness/state/demo_progress polls etc. - are
  excluded, or the Dashboard's 15s polls would pin the badge forever); the 20s janitor clears
  the badge after 5 minutes of quiet. No more per-verb on/off flicker (the envelope-drain clear
  is gone). Badge set is throttled (60s/same-hwnd) and async so it never slows a verb.
  fusion_state now reports drivingBadge {active, lastDrivingVerbAt, idleShedSeconds}.
- **dev-skills/fusion-taskbar-badge**: the complete verbose recipe (SetOverlayIcon trap,
  desktop_taskbar contract, brand asset provenance + sizing, activity/idle lifecycle, red-box
  proof method, kicad porting checklist).

## 1.9.57 - 2026-08-17

- **Driving badge shrunk ~30%** (John: the user should see MORE of the Fusion icon and only
  "slightly" know Adom is driving): the brand mark now renders at 22px inside the transparent
  32px overlay slot, hugging the corner. Re-proven live with a red-box capture.

## 1.9.56 - 2026-08-17

- **The driving badge is now the REAL Adom brand mark** (John: "that overlay is not the adom
  favicon"): pup's canonical teal-leaves overlay asset (pup-overlay-32.png) replaces the
  improvised white-"a" glyph, so a driven Fusion button matches every other Adom tile on the
  taskbar. Re-proven live with a red-box capture.

## 1.9.55 - 2026-08-17

- **The driving badge actually renders now (pup's lesson, stolen with credit).** In-process
  ITaskbarList3::SetOverlayIcon returns S_OK cross-process and the shell silently ignores it -
  pup root-caused this long ago and routes through AD-core's `desktop_taskbar` verb, which paints
  natively. taskbar_overlay.py now does the same (overlay {icon: b64 png, tooltip} / {badge:
  "none"}). PROVEN live on AdomLapper: red-box capture of Fusion's taskbar button wearing the
  teal Adom mark is in fusion-proofs.

## 1.9.54 - 2026-08-17

- **Ralph-test findings on 1.9.53, same day.** (1) An UNATTRIBUTED save prompt (its spawning doc
  already closed, so no title names it) stood forever - the sentinel now posts ESCAPE (Cancel,
  non-destructive for anyone's doc) after 2 sweep sightings; Don't Save still requires
  attribution. (2) UIA-less prompt hwnds are probed for a Don't-Save button ONCE, not 2x10s every
  sweep. (3) Dashboard SSE state pushes for a DIFFERENT target no longer overwrite the user's
  selected box view (the header/pill mismatch in the first 1.9.53 capture).

## 1.9.53 - 2026-08-17

- **Honest state detection (John: "you aren't detecting my state all that well").** Sign-in,
  add-in, and MCP are UNKNOWABLE while Fusion is closed - the bridge now persists last-verified
  observations (lastReadyAt / lastSignedInAt on readiness, lastEnabledAt on mcp_status) and the
  Dashboard paints those facets AMBER-dormant ("configured, resumes when Fusion starts") instead
  of broken-red. Red now means "actually needs setup".
- **Setup is clean for configured users**: dormant facets are excluded from the pending list, so
  a weeks-old setup shows "Everything is configured - Fusion just is not running" with Start.
- **Multi-box is first-class.** New /api/targets + target pills in the Dashboard header (running
  dot per box, sticky choice); every status read and action carries the chosen box. One-seat copy
  explains the workflow: start Fusion on the conference box, the seat follows automatically, the
  laptop stays undisturbed.
- **Taskbar badge: "Adom is driving Fusion" (John's overlay idea).** New handlers/taskbar_overlay.py
  (ITaskbarList3::SetOverlayIcon via ctypes, no deps) badges Fusion's taskbar button with the Adom
  mark while the background envelope is active - set on first envelope, cleared when the refcount
  drains. resources/adom-overlay.ico ships in the zip.

## 1.9.52 - 2026-08-16

- **The catalog now teaches the full foreground contract.** fusion_describe's foreground arg docs
  were stale after 1.9.51 (still "optional bool") - an AI following the catalog would hit the
  foreground_reason_required refusal and only learn from the error. All foreground-capable verbs
  now document the background default, the windowEtiquette response marker, and the exact
  {foreground:true, foregroundReason:"..."} shape with the 3s-caption semantics; the background
  annotation hint shows the same literal re-call shape.

## 1.9.51 - 2026-08-16

- **Foreground etiquette contract (John, after ab's demo stole his focus).** (1) The tour's
  single est-sized background envelope EXPIRED mid-run on slow tours (485-890s vs ~376s armed),
  so later steps foregrounded Fusion while the user typed - the envelope now RE-ARMS per step.
  (2) foreground:true now REQUIRES foregroundReason and shows it as an ab caption for 3 seconds
  BEFORE any focus change, so the user knows why and can yield (foreground_reason_required
  teaching refusal otherwise). (3) Every backgrounded window-raising verb annotates its response
  with windowEtiquette:"background" + a hint so the calling AI knows no focus was taken and never
  compensates by foregrounding. Contract recorded in dev-skills/fusion-background-etiquette.

## 1.9.50 - 2026-08-16

- **Third tour failure, last gap closed**: the tour's own bundled sample docs (ADS8588SIPM,
  0603WAF resistor) were missing from the bridge-doc conventions, so the sweep correctly refused
  their Save prompts and the launch wait starved behind them. Sample names added to the builtins,
  and the launch wait now sweeps bridge-doc save prompts each iteration (they block get_app_state
  exactly like the seat modal).

## 1.9.49 - 2026-08-16

- **Save-prompt hardening after two live tour failures (ConfRoomROG).** A bare Qt "Save" modal
  from a killed session blocked the add-in main thread invisibly: no UIA tree, PrintWindow black,
  classified "unknown". Now: (1) title "save" classifies save_changes; (2) the sentinel detects
  the UIA-less variant by title and falls back to posting ESCAPE (= Cancel, non-destructive,
  needs no UIA and no ab input grant) when the Don't-Save click is unavailable; (3) the
  bridge-doc registry persists to prefs.json AND carries built-in conventions (adom-test-*,
  AdomDemoSample) so a respawn can't orphan the bridge's own docs; (4) the 20s janitor sweep
  now clears standing bridge-doc save prompts, not just seat dialogs and update nags.

## 1.9.48 - 2026-08-16

- **stepShot frames actually deliver on PIL-less bridges.** AD's provisioned python has no PIL,
  so captures fall back to raw ~24MB BMPs; 1.9.47's encoder silently skipped every one (live
  tour: 9 shots, 0 stepShot frames). New pure-stdlib BMP->PNG nearest-neighbor downscaler
  (zlib+struct, no deps) feeds the ~640px stepShot contract regardless of PIL.
- **Inline gallery hard cap.** imagesAs:base64 now refuses to inline files over 4MB and ships
  the downscaled (<=1280px) frame + imageNote instead - a BMP-era done frame would have been a
  ~280MB response.

## 1.9.47 - 2026-08-16

- **Live per-step tour evidence (ab's #30 contract, John's call).** fusion_demo_progress frames
  now carry `stepShot {step, title, image}` - a ~640px base64 capture the bridge took at each
  step's completion, one per step, riding the first frame after it completes. shots[] also
  streams mid-run (paths always; `imagesAs:base64` + `sinceShot:<n>` windows to only the new
  ones, so 1s polls stay cheap). The done:true gallery is unchanged.
- **Finished tours survive bridge respawns.** ab's poller lost the final frame because
  bridge_install replaced the process between polls (tours were never reaped - the process was).
  Done tours snapshot to ~/.adom/fusion-bridge/tours/ (last 8 kept) and fusion_demo_progress
  falls back to the snapshot for unknown in-memory ids.
- **license_sessions_exceeded is now a first-class classification.** The seat modal often has a
  bare Win32 title, so the substring classifier filed it generic; fusion_state and
  fusion_window_info now also run the ownership+shape detector and append the decidable category
  with the auto-takeover status. The tour cold-start self-heals it (takes the seat, reports it as
  its own progress state) and FAILS FAST with the classification when takeover cannot land or is
  opted out - no more 150s wedge at 0%.

## 1.9.46 - 2026-08-16

- **fusion_set_seat_takeover: the seat-takeover sweep is now a STICKY setting** (default ON,
  mirrors auto-update; no-args = read). Gates only the passive 20s sweep - a verb-driven
  fusion_start still grabs the seat, since asking for Fusion implies wanting the seat.
- **Dashboard Settings panel now carries BOTH sticky defaults**: silent Fusion auto-update and
  the seat auto-takeover, each toggleable with live toasts. One lockstep version everywhere
  again (bridge zip + dashboard binary + pkg all 1.9.46).

## 1.9.45 - 2026-08-16

- **Seat conflicts: ALWAYS take over the session, even mid-session (John's call).** The proven
  background resolver (UIA-clicks Continue through BOTH Autodesk dialogs - 'Active Sessions
  Exceeded' then the 'Suspend Remote Session' confirm - with owned-popup verification) previously
  ran only at launch and on readiness polls, so a seat dialog popping mid-session sat unhandled.
  It now also runs from the janitor loop every ~20s: cheap no-op when clear, automatic takeover
  when Autodesk flags the session limit. Suspend is the pre-selected option, so the other
  machine's Fusion suspends RESUMABLY (nothing lost) and this machine gets the seat. The user
  gets ONE toast after a real takeover (debounced 120s) so they know the other box was suspended.

## 1.9.44 - 2026-08-16 (dashboard only; bridge zip stays 1.9.42)

- **Dashboard house-contract completion (John's callout: no favicon = not done).** Served
  favicon + tab identity, SSE live channel at /events (state/toast/click events), POST
  /ui/toast + /ui/click, CLI driving verbs (`ui toast|click|state|console`), brand-hover
  app-info popover, viewport-clamped tooltips, centered dialogs, 24h idle reaper,
  --ai-thread identity forwarded to every bridge call. All ralph-tested live in pup:
  CLI-driven Settings open + toast render verified by screenshot, favicon verified in the
  actual browser tab via OS window capture.
- **Two streaming lessons now in the dev skill:** tiny_http buffers a streamed body until
  EOF (server is now hand-rolled HTTP/1.1 with per-event flush, binary smaller), and the
  container proxy buffers SSE until you send no-transform + X-Accel-Buffering: no + a 2KB
  comment padding at connect.

## 1.9.42 - 2026-08-16

- **THE DASHBOARD (John's Launch Page spec).** New `dashboard/` Rust CLI (`fusion-dashboard`) served
  INTO THE CONTAINER by this page's pkg tarball - the first bridge with a container-side binary (the
  three-artifact pattern: bridge zip -> ab, pkg -> container skills + `bin/fusion-dashboard`, page
  media -> wiki). Serves a house-style web app (LED status per facet: Fusion running, sign-in, APS
  cloud search, MCP server, Data Hub, pending update; Setup flow for first-time users; Settings with
  the sticky auto-update toggle; copy-paste AI prompts; the demo tour with a live progress bar; a
  wiki basic-part import demo). Status aggregates by shelling `adom-bridge` verbs - the bridge stays
  the single source of truth - and every API response carries `_hint`s for the calling AI. `dock.json`
  (Dock Bar SDK 1.1.0) makes it a dock candidate: pkg install + `serve --print-url`.
- **`fusion_state` shipped (#28, ab's top ask on #30).** kicad_state parity: running + window + rect +
  dialogs + CLASSIFIED blockingDialogs + main-thread busy + best-effort activeDocument + a background
  screenshot (imageAs:base64 inlines up to 4MB). Server-side only ON PURPOSE - it answers even while
  the add-in main thread is busy or wedged, exactly the moments a render verb is needed.
- **Save-prompt sentinel (#30 wedge defect).** Fusion's unsaved-doc "Save / Don't Save / Cancel" modal,
  popped mid doc-switch, orphaned the in-flight add-in call and wedged the main thread past recovery.
  Doc-switching add-in calls now run under a watcher that auto-clicks "Don't Save" IN THE BACKGROUND -
  but ONLY when the dialog text names a document the bridge itself created this session (_BRIDGE_DOCS);
  a user's own unsaved design is never auto-answered. The watcher lingers ~12s past the call return to
  cover the prompt-after-timeout case. Add-in-side command watchdog remains tracked on #30.
- **`fusion_window_info` now returns classified `blockingDialogs[]`** (cause + resolution), the key
  ab's demo runner reads - previously only raw titles.
- **Recovered Documents dialog auto-closed by the 5-min background sweep** (John: "anytime it comes up,
  just ignore it and close it"). Launch-time relocation already prevented it on bridge-driven starts;
  the sweep now covers mid-session appearances and user-launched Fusion. Info-only dialog: File >
  Recover Documents keeps the list, and dismissal archives recovery files to ~/.adom/recovery.

## 1.6.58 - 2026-07-08

- **Long cloud ops now SURVIVE AD's 60s relay cap (John: "get all 4 pins perfect" + "update your code
  with your learnings").** `fusion_build_library_3d` cloud Package3D + Hub upload takes MINUTES, but the
  relay hard-caps requests at ~60s and `timeoutSeconds` is ignored - so the synchronous build got its
  thread killed at 60s, wrote no bound .lbr, and lost every wip_urn (a demo board's 4th pin came back a
  flat pad). It now runs DETACHED (`async` default true) + is POLLABLE: returns immediately, keeps
  building in a daemon thread, writes the bound .lbr when done; caller polls it for one `<package3d
  wip_urn=urn:...>` per part. `async:false` for fast machines.
- **Background, no-dialog EAGLE board import documented + steered to.** For a `.brd` with placed
  `<elements>`, `fusion_open_board` (newDesignFromLocal) instantiates them with ZERO modal dialogs / ZERO
  foreground - vs `fusion_new_electronics_from_eagle`'s ImportSCHAndBRDCmd, which pops two Open dialogs
  that STEAL focus. The import verb no longer dead-ends when the AD direct API is absent; it points to
  `fusion_open_board`. NEW: embed `<packages3d>` + a per-`<element>` `package3d_urn` in the `.brd` to get
  REAL 3D component bodies on the board (Fusion resolves the Hub urns) - documented in the verb docstring.
- **Fixed the HWND-truncation window-finder bug (cost hours).** `handlers/fusion_ui.py` declared NO
  ctypes argtypes, so 64-bit HWND handles truncated to 32-bit `c_int` - `_find_fusion_hwnd`'s
  GetWindowTextW/IsWindowVisible ran on garbage handles and matched nothing, so `get_fusion_window_info`
  returned `mainHwnd:None` even though Fusion's window plainly existed, blinding all background driving.
  Declared HWND argtypes/restypes for the window funcs so the full handle marshals.

## 1.6.57 - 2026-07-08

- **APS sign-in now REMEMBERS the browser+profile that authed to Autodesk and reuses it, instead of
  firing `webbrowser.open` at the OS default (John: "power users have tons of browsers with tons of
  profiles ... cache what browser you used when you successfully logged in ... don't be lazy").** The
  old code blindly opened the OS default browser, which on a multi-browser box is usually the wrong
  one (no warm Autodesk/SSO session / wrong profile) and forces a needless fresh login. Now
  `start_signin`: (1) launches a browser+profile passed in the call, else (2) the REMEMBERED one
  (`~/.adom/fusion-aps/signin-browser.json`) via a real `--profile-directory`/`--new-window` launch,
  else (3) opens NOTHING and returns `needsExtensionOpen:true` so the caller drives `authUrl` in the
  user's real browser via `nbrowser_open_window` (new window) — the OS default is only used when the
  caller explicitly passes `allowDefaultBrowser:true`. New verbs `fusion_aps_set_browser` /
  `fusion_aps_get_browser` / `fusion_aps_forget_browser` persist the choice across sessions.
- **Notify-before-foreground, baked into code (John: "if you have no other choice than to
  foreground, let me know with an ad notify ... do this via code").** SendInput key/click paths
  need Fusion in the FOREGROUND, which steals the user's focus mid-work. `_notify_before_foreground()`
  now fires an AD notify (reach_user, debounced to one per 45s burst) BEFORE `fusion_send_key` /
  `fusion_click_fusion` foreground Fusion, telling the user WHY, reminding that Adom ALWAYS drives in
  the background and only foregrounds as a last resort, and ribbing Fusion for not being AI-native
  enough to allow background driving (hoping Autodesk makes it fully AI-drivable soon). Prefer the
  background path first (e.g. `fusion_open_board`'s dialog-free `newDesignFromLocal`, `desktop_ui_*`,
  `WM_CLOSE`).

## 1.6.55 - 2026-07-07

- **Universal error-dialog guard, baked into every mutating/open verb (John: "you have to
  screenshot to get the parent/child array to constantly look for errors ... bake this into every
  command").** A malformed `.lbr` makes Fusion pop an owned "Error" dialog ("<file>.lbr has errors
  and cannot be opened") while `Document.newDesignFromLocal` still returns ok (it opened an EMPTY
  design behind the error), so `open_lbr` reported success on a library that never loaded. New
  `operation_error` dialog classification (title "Error" / "cannot be opened" / "has errors") + a
  shared `_apply_failure_dialogs()` now runs after every open/mutating verb: it screenshots the
  owned popup (hwnd-targeted, background), flips the result to `success:false` +
  `errorCode:fusion_operation_error` with the exact resolution, attaches the shot to
  `data.errorDialog`, and dismisses the pure-ack popup so it can't wedge the next command. A failed
  op can no longer be returned as success.

## 1.6.54 - 2026-07-07

- **Fixed 3D auto-orient laying TALL through-hole parts on their side** (John caught it: a machine
  pin's 3D came in horizontal). `make_3d_package` / `attach_3d_package` / `build_library_3d` always
  rotated the SMALLEST bbox dim to Z (correct for a flat SMD chip, wrong for a tall pin whose long
  axis is already Z). Now a tall-part guard keeps a part vertical when its largest dimension is
  already Z, and only flattens a clearly-thin part whose thin axis isn't Z yet. Verified: machine
  pins stay vertical, a flat 0603 stays flat, a chip imported on its edge still gets flattened.
- `attach_3d_package` and `build_library_3d` parts[] now honor an `orient` flag (default true) so a
  caller can skip auto-orient for an already-correctly-oriented STEP.

## 1.6.53 - 2026-07-07

- **Fixed slow-machine opens being mislabeled "add-in not running" (caught live on a GPU-less Azure
  VM).** `_orchestrate_open_lbr`'s `Document.newDesignFromLocal` call had a fixed 30s read timeout;
  on software-rendered machines the open takes 60s+, so the proxy timed out and the canned error
  claimed the ADD-IN was down while the document was actually opening fine -
  `fusion_build_library_3d` then aborted every part on a lie. Now: 120s timeout AND on any failure
  the open is VERIFIED BY STATE (poll `get_app_state` for the expected document) before failing -
  the doc actually being open outranks the synchronous return.
- **NEW verb `fusion_notify_owner` - deterministic last-resort human escalation (John's rule).**
  Toasts the user's MAIN computer by fanning out cross-AD to every relay peer, so a bridge on an
  unattended VM still reaches them. For true human walls only (password/2FA, UAC); hints tell the
  driving AI to exhaust programmatic options first and not to re-toast.
- **Hints now teach the parent/child screenshot discipline**: never poll blind, never fullscreen -
  `desktop_screenshot_window` returns a `screenshots[]` array where every OWNED POPUP rides back as
  its own child shot; launch/sign-in hints now say to READ those.

## 1.6.48-1.6.52 - 2026-07-06 (pkg hygiene, no runtime change)

- Declared the `adom/adom-desktop` pkg dependency (SDK two-artifact audit, #83).
- Trimmed the install tarball from ~1.18 MB to ~90 KB: dropped the 1 MB `hero.png` (a wiki
  billboard served from `/files`, not an install asset) and the publish-channel `PUBLISHING.md`.
- De-duplicated the library docs: `MAKING_LIBRARIES.md` / `LIBRARY_FINDINGS.md` had drifted root
  copies (broken relative image links) alongside the canonical `fusion-libraries` skill copies;
  removed the root copies. Moved `CREATING_BASIC_PARTS_LIBRARIES.md` into the `fusion-multipart-libraries`
  skill and fixed the cross-skill references.
- Split this changelog: recent versions ship in the tarball, older entries moved to
  `CHANGELOG_ARCHIVE.md` (kept on the wiki, not shipped).

## 1.6.47 - 2026-07-06

- **Tightened seat-dialog detection to DIALOG-SHAPED owned popups (fixes a false positive from
  1.6.46).** 1.6.46 matched any owned popup wider than 300px, but Fusion's own side panels (Data
  Panel / browser rail, ~300x785) are owned popups too - so the moment a document was open,
  `find_licensing_dialog()` flagged a drivable Fusion as "blocked by a seat dialog" and readiness
  reported ready:false forever (caught live on winvm: get_app_state succeeded while readiness lied).
  The seat modal is always WIDE (823x262 / 1235x527); side panels are narrow. Detection now requires
  width >= 480, so the panels are excluded and the real dialog still matches.

## 1.6.46 - 2026-07-06

- **Fixed the seat/licensing dialog going UNDETECTED (John caught it live on winvm).** The
  "Suspend Remote Session" seat-confirm variant is only **823x262**, but `find_licensing_dialog()`
  required height **320-820**, so it silently slipped through - readiness reported `licensingDialog`
  gone while the dialog sat there blocking the whole launch. Root cause: detecting a modal by a
  fragile SIZE window. Now detected DETERMINISTICALLY by the **owned-popup** signal: a seat/sign-in
  modal is always an OWNED POPUP of the main Fusion window (the splash/main app never are), found via
  `GetWindow(GW_OWNER)` in `family_windows()`. Size is only a fallback.
- **Resolution is now SCREENSHOT-VERIFIED, not heuristic-verified.** `_resolve_seat_dialog()` clicks
  'Continue' (background UIA) then confirms the dialog truly cleared by reading the parent window's
  `ownedPopupCount` via `desktop_screenshot_window` (parent/child screenshotting) - instead of trusting
  the same size filter that had false-'resolved' it. This is the check that would have caught the
  earlier "I solved it" that hadn't.
- **Closed the auto-resolve GAP.** The launch loop only auto-resolved the seat dialog during its ~50s
  window. But the dialog also appears LATER (minutes post-sign-in, when the license server notices too
  many sessions), long after that loop exited. `fusion_readiness` now SELF-HEALS: whenever it detects
  the dialog it auto-resolves it in the background and re-probes - so the seat is handled in CODE no
  matter when it shows up, and the AI never has to notice or act.
- ctypes `GetWindow` given explicit `restype/argtypes` (HWND) to avoid 64-bit handle truncation.

## 1.6.45 - 2026-07-06

- **Add-in changes now AUTO-DEPLOY - the bridge no longer only installs the add-in when it's MISSING.**
  Startup guarded `install_addin()` behind `if not addin_installed`, so a STALE add-in was never
  UPDATED: any add-in fix (e.g. the 1.6.44 get_parameters fix) failed to reach users who already had a
  copy - it silently kept running the old add-in. Now the bridge ALWAYS syncs on startup (idempotent -
  copies only changed files; safe while Fusion is up, lands on the next respawn with Fusion closed).
  This is a real distribution bug - without it, none of our add-in fixes shipped to existing installs.

## 1.6.44 - 2026-07-06 (add-in 1.0.4)

- **Fixed `fusion_get_parameters` crashing on non-numeric parameters** (found during the laptop verb
  battery). `Parameter.value` throws "Parameter is not numeric type" for string/text params, which
  blew up the entire enumeration. Every field is now read defensively so one odd parameter never kills
  the call. Add-in bumped to 1.0.4.

## 1.6.43 - 2026-07-06

- **Docs + hints caught up to the real (UIA) seat-dialog handling.** The `fusion-bridge-dev` skill now
  records the hard facts: it's a QT dialog (not CEF), titled just "Fusion360", its "Continue" button is
  UIA-invokable in the BACKGROUND, the radios are NOT in the a11y tree, kill+relaunch never frees a
  server-held seat (dead code), retry-until-verified-gone, and the seat check outranks a stale add-in
  port. Response hints now NARRATE what the bridge is doing + what happens next: `fusion_start` says it
  launched in the background (don't foreground), whether it auto-resolved a seat dialog, and to poll
  `fusion_readiness`; the readiness `licensingDialog` hint describes the UIA auto-resolve (not the old
  "free it from peer machines" text).

## 1.6.42 - 2026-07-06

- **Seat-resolve now RETRIES until the dialog is verified gone, and takes priority over a stale add-in
  port** (the bug that made 1.6.40 look solved when it wasn't - John caught it live). Two fixes: (1) the
  first UIA `Continue` Invoke on a mid-render dialog silently NO-OPs, so the launch loop now re-invokes
  every tick until `find_licensing_dialog` no longer sees it, instead of clicking once; (2) the loop
  and `fusion_readiness` no longer trust the add-in probe while the seat dialog is up - a STALE add-in
  server from a prior killed instance keeps port 8774 answering, which made the loop break early and
  readiness report `licensingDialog:false`/`ready:*` while the dialog sat right there on screen. Seat
  dialog now blocks `ready` whenever detected. Verified: full verb battery passes on the laptop
  (modeling, EPG+etch->STEP 234KB, export 234KB, dark mode) once the seat dialog is auto-cleared.

## 1.6.41 - 2026-07-06

- **Instant-minimize: disable the minimize ANIMATION during launch** so the focus-guard's minimize is
  a single-frame vanish, not the visible slide-down that read as a 'flash' (John). Combined with the
  1.6.40 seat auto-resolve (no more stuck sign-in retry loop = no repeated foregrounding) and the
  foreground-lock, a background launch should now be imperceptible. `SPI_SETANIMATION` off during the
  guard window, restored to the user's original value when it ends. (Chosen over virtual-desktop
  isolation, which proved un-shippable: Win11 virtual-desktop control is undocumented, per-build, and
  the driver won't even install cleanly - it would make the bridge brittle across Windows updates.)

## 1.6.40 - 2026-07-06

- **THE seat-dialog fix: `fusion_start` now auto-resolves "Active Sessions Exceeded" via UIA in the
  BACKGROUND** (John pushed me back to UIA - it was the answer). The launch handler is now a
  deterministic state loop: poll the add-in round-trip (ground truth for drivable) while, each tick,
  detecting the seat dialog (`find_licensing_dialog`, by process+size) and invoking its native
  'Continue' button with `desktop_ui_click` via the AD direct API - which UIA-Invokes it WITHOUT
  foregrounding (proven: "Clicked in the BACKGROUND - the window did NOT come to the foreground").
  Suspend is pre-selected, so Continue grabs the license. This un-sticks the launch, which is what
  caused the endless foreground-stealing: a blocked seat dialog made Fusion retry sign-in in a loop,
  foregrounding every retry. No more kill+relaunch (the licensing server keeps the seat anyway) and
  no notify/punt to the user.

## 1.6.39 - 2026-07-06

- **Dev skill: added THE CORE GOAL** (John) - the entire reason this bridge exists is for the AI to
  run EVERYTHING so the user never does anything manual. Never ask the user to click/close/sign-in;
  the only acceptable walls are UAC/elevation and a password/2FA we must not enter. Any flow that
  still needs the user is the next bug to fix in the bridge, not the user's job. Reinforces detecting
  + handling state deterministically in code, not via the AI following skills.

## 1.6.38 - 2026-07-06

- **Deterministic, process-based detection of the seat/licensing dialog** (John: track state in CODE,
  not via the AI following skills). The "Active Sessions Exceeded" dialog's window TITLE is literally
  just "Fusion360" (the text is CEF body content), so the title-based classifier never caught it - it
  silently blocked Fusion's main thread for a whole session. New `family_windows()` enumerates visible
  top-level windows by OWNING PROCESS (Fusion360 / FusionLauncher / AdskIdentityManager), and
  `find_licensing_dialog()` finds the licensing modal by process + dialog size. `fusion_start` now
  auto-reclaims on it deterministically, and `fusion_readiness` reports `licensingDialog:true` with a
  clear hint instead of a vague "add-in not responding" hang.

## Older versions

Entries for **1.6.37 and earlier** live in [`CHANGELOG_ARCHIVE.md`](https://wiki.adom.inc/adom/fusion-bridge) (full history,
kept on the wiki page's Files tab, not shipped in the install tarball). The per-release changelog
for every version is also on the page's **Releases** tab.
