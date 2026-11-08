# Demo videos

Three short demos of Hydrogen driving Autodesk Fusion through the Fusion bridge. Each one is a
real run: a single prompt typed into Hydrogen, then Hydrogen does the work in Fusion with nobody
touching the mouse. The only edits are cuts, speed-ups of the waiting, and narration, timed so
there is no dead air.

## Clearing a modal dialog that blocks a script

[![Clearing a modal dialog that blocks a script, watch on YouTube](docs/img/forum/demo-modal-dialogs.jpg)](https://youtu.be/KPbHgk800wE)

[Watch on YouTube](https://youtu.be/KPbHgk800wE) (51 s)

**The prompt typed into Hydrogen:**

> In Fusion, show me how the Fusion bridge handles a modal dialog that blocks a script. Open the
> ToF showcase board, run ADD from a script so the library picker opens and blocks it, then have
> the bridge read the dialog and cancel it in the background.

Fusion Electronics edits go through `executeTextCommand('Electron.run ...')`, and any command that
opens a modal (an error, the ADD library picker) blocks every later script until a person closes
it. The bridge reads each open Fusion dialog through Windows UI Automation (title, text, buttons),
answers the ones it recognises, cancels unknown ones in the background, and hands the text back to
the AI. In the video the picker pops up, the script hangs, and the bridge presses Cancel without
moving the mouse or taking focus. Details: [Dialog awareness](../README.md#dialog-awareness-parentchild-screenshots-the-automation-breakthrough)
and the `fusion-no-modal-dialogs` skill.

## One prompt: parts library to schematic, 2D board and 3D board

[![One prompt: parts library to schematic, 2D board and 3D board, watch on YouTube](docs/img/forum/demo-library-to-3d-board.jpg)](https://youtu.be/Th_HwxCgleE)

[Watch on YouTube](https://youtu.be/Th_HwxCgleE) (55 s)

**The prompt typed into Hydrogen:**

> In Fusion, make a schematic and a board with every part in my Adom_ToF library. Then show me
> the schematic, the 2D board and the 3D board, and orbit the 3D view so I can see the parts.

Hydrogen reads the bridge's commands, checks the library, and calls `fusion_make_showcase_project`,
which writes the schematic and board as EAGLE source and opens them with
`Document.newDesignFromLocal`, so no dialog appears. Then it switches through the schematic, the
2D board and the 3D board, where every part shows its bound 3D model, and orbits the camera. Every
Fusion frame is a window capture, which works in the schematic and PCB editors even with Fusion in
the background. Details: [Schematics](schematics.md).

## A real 3D model on every part of a library

[![A real 3D model on every part of a library, watch on YouTube](docs/img/forum/demo-3d-library.jpg)](https://youtu.be/poAy__9Iggw)

[Watch on YouTube](https://youtu.be/poAy__9Iggw) (1:45)

**The prompt typed into Hydrogen:**

> Build a Fusion library from my Demo Board cloud library on the Adom wiki, with the real 3D
> model bound to every part, and save it to my Fusion Library Manager.

This took us longest. Three traps:

1. **A STEP in the cloud is not enough.** Bind a raw STEP's urn and the library says "Thumbnail
   download failed". Only a Fusion design (`.f3d`) with the STEP inside renders.
2. **The GUI path is a dialog trap.** The 3D package generator ends with FINISH, which opens a
   Save dialog and two browser modals that no script can read or close.
3. **The generator loads the first package of the open library,** whatever you ask for, so in a
   multi-part library every chip lands on part one's footprint.

The order that works: split the library into one library per part; start the generator on that
part's library so it loads that footprint; import the STEP exactly as the maker published it;
Save As an `.f3d` instead of FINISH; wait for the cloud lineage id and build the urn from it;
write every urn into the merged library in one pass, and open it once. In the video Hydrogen
binds six parts, proves each one (symbol, footprint, 3D package, component view), and saves the
library to the Library Manager. Details: [Multi-part library import](MULTIPART_LIBRARY_IMPORT.md).

## How the videos were made

- **The run.** A Windows box with Hydrogen full screen and Fusion in front of it at 85%. The
  prompt is typed into Hydrogen's AI chat; Hydrogen drives Fusion through this bridge.
- **The capture.** Adom Bridge records the whole monitor (taskbar included) and Hydrogen's window
  separately, in the background, so the chat can be inset in the corner while Fusion works.
- **The edit.** `tools/video/compose.py` takes a list of sections (a time range, a layout and one
  narration line each), voices each line with adom-tts, and speeds up or slows each section so it
  lasts exactly as long as its line, which keeps the narration continuous. The section lists are
  `tools/video/spec-*.json`.
- **The hosting.** Unlisted on the Adom Industries YouTube channel.

## Sharing them in forum replies

These are feature demos first. When someone on a forum asks about something one of them shows,
we link the relevant video in a reply that answers their question on its own merits. The first
one is a reply to Aske Carstensen's
[Electronics API + MCP: modal dialogs block scripts, no undo for Electron.run edits](https://forums.autodesk.com/t5/fusion-electronics-forum/electronics-api-mcp-modal-dialogs-block-scripts-no-undo-for/td-p/14286758)
on Autodesk's Fusion Electronics forum, [posted 2026-10-05](https://forums.autodesk.com/t5/fusion-electronics-forum/electronics-api-mcp-modal-dialogs-block-scripts-no-undo-for/m-p/14289140/highlight/true#M24737).
