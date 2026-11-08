"""Open electronics files in Fusion 360's Electronics workspace.

Each file type gets its own command for clarity:
  fusion_open_schematic  — .sch files → Schematic Editor
  fusion_open_board      — .brd files → Board Layout editor
  fusion_show_3d_board   — switches Board Layout → 3D PCB view

All use Document.newDesignFromLocal which reliably opens these file types
and switches to the correct Electronics sub-workspace automatically.

fusion_open_lbr (in open_lbr.py) handles .lbr library files.

Fusion 360 Electronics projects are cloud-based (not .epr files like standalone EAGLE).
The project is the Fusion document itself. Open a schematic or board to work with a design.
"""

import adsk.core
import os
import time



_SIBLING_EXTS = ("sch", "brd", "lbr", "step", "stp", "f3d", "edru", "dru", "flbr", "fsch", "fbrd", "ulp")


def _stage_root():
    """The first space-free, writable stage root (a stage under a path with a space is useless)."""
    import os
    cands = [os.path.join(os.path.expanduser("~"), ".adom", "fusion-bridge", "stage")]
    for env in ("LOCALAPPDATA", "TEMP"):
        v = os.environ.get(env, "")
        if v:
            cands.append(os.path.join(v, "adom-fusion-stage"))
    cands.append("C:/adom-fusion-stage")
    for c in cands:
        c = c.replace("\\", "/")
        if " " in c:
            continue
        try:
            os.makedirs(c, exist_ok=True)
            return c
        except Exception:
            continue
    return None


def _stage_for_fusion(file_path):
    """Copy the file (and its same-stem siblings) under a space-free folder; return the copy's path."""
    import os, shutil, hashlib
    src = file_path.replace("\\", "/")
    if not os.path.isfile(src):
        return None
    root = _stage_root()
    if not root:
        return None
    parent = os.path.dirname(src)
    bucket = hashlib.sha1(parent.lower().encode("utf-8")).hexdigest()[:8]
    d = os.path.join(root, bucket).replace("\\", "/")
    os.makedirs(d, exist_ok=True)
    stem = os.path.splitext(os.path.basename(src))[0].lower()
    for name in os.listdir(parent):
        q = os.path.join(parent, name)
        if not os.path.isfile(q):
            continue
        s, ext = os.path.splitext(name)
        if q.replace("\\", "/") == src or (s.lower() == stem and ext.lstrip(".").lower() in _SIBLING_EXTS):
            dst = os.path.join(d, name)
            try:
                if not (os.path.exists(dst) and os.path.getsize(dst) == os.path.getsize(q) and os.path.getmtime(dst) >= os.path.getmtime(q)):
                    shutil.copy2(q, dst)
            except Exception:
                if q.replace("\\", "/") == src:
                    raise
    out = os.path.join(d, os.path.basename(src)).replace("\\", "/")
    return out if os.path.isfile(out) else None


def _open_local_document(app, file_path: str):
    """Run Document.newDesignFromLocal for a path that MAY CONTAIN SPACES.

    BUG (Drew's agent, issue #196, 2026-07-16): executeTextCommand is a SPACE-DELIMITED
    command string, so an unquoted path like
      C:/Users/drew/Downloads/e2e-pmcoil/PM COIL Molecule.brd
    was split at the first space - Fusion got ".../e2e-pmcoil/PM" and the open silently
    no-op'd, surfacing only as the generic "Fusion may not support this file" hint. A
    space-free copy of the same file opened instantly, which is what pinned it to path
    handling rather than file support.

    Two-step, so it works whichever way this Fusion build's parser behaves:
      1. QUOTE the path.
      2. If that fails and the path has spaces, retry with a STAGED copy of the file (plus its
         same-stem siblings) under a space-free folder. Never the 8.3 short path: Fusion titles
         the document after the path it is given, and "0603WA~1" broke every name check
         (bridge 1.10.11, issue #83).

    Returns (result, how) and raises the LAST exception if every attempt fails.
    """
    attempts = []
    quoted = '"%s"' % file_path.replace('"', '')
    attempts.append((quoted, "quoted"))
    if " " in file_path:
        try:
            staged = _stage_for_fusion(file_path)
            if staged and " " not in staged:
                attempts.append((staged, "staged"))
        except Exception:
            pass
    def _doc_name():
        try:
            d = app.activeDocument
            return d.name if d else None
        except Exception:
            return None

    before = _doc_name()
    last = None
    for arg, how in attempts:
        try:
            r = app.executeTextCommand("Document.newDesignFromLocal %s" % arg)
        except Exception as e:
            last = e
            continue
        # executeTextCommand does not always RAISE on failure - it can return an error
        # string and leave the document untouched. So confirm a document actually opened
        # before declaring victory, otherwise fall through to the next attempt.
        time.sleep(0.6)
        if _doc_name() != before:
            return r, how
        last = RuntimeError("open did not change the active document (result=%r)" % (r,))
    raise last if last else RuntimeError("no open attempt ran")


def _open_electronics(app: adsk.core.Application, file_path: str, file_type: str, expected_ext: str) -> dict:
    """Common logic for opening an electronics file in Fusion."""
    if not file_path:
        return {
            "success": False,
            "error": f"No filePath specified. Provide a {expected_ext} file path.",
            "data": {
                "hint": f"Usage: fusion_open_{file_type.replace(' ', '_')} {{\"filePath\": \"C:/path/to/file{expected_ext}\"}}"
            },
        }

    file_path = file_path.replace("\\", "/")

    if not os.path.exists(file_path):
        return {
            "success": False,
            "error": f"File not found: {file_path}",
            "_hint": "The path may exist on Docker but not on the Windows host. Verify with desktop_list_directory on the host, or use push_file to copy from Docker first.",
            "data": {"filePath": file_path, "hint": "Check that the file path exists on the Windows desktop machine, not on Docker."},
        }

    ext = os.path.splitext(file_path)[1].lower()
    if ext != expected_ext:
        return {
            "success": False,
            "error": f"Expected a {expected_ext} file, got {ext}. Use the correct command for this file type.",
            "data": {
                "hint": {
                    ".sch": "Use fusion_open_schematic for .sch files",
                    ".brd": "Use fusion_open_board for .brd files",
                    ".lbr": "Use fusion_open_lbr for .lbr files",
                    ".step": "Use fusion_import_step for .step files",
                    ".stp": "Use fusion_import_step for .stp files",
                }.get(ext, f"This command only handles {expected_ext} files"),
            },
        }

    try:
        r, _open_how = _open_local_document(app, file_path)
    except Exception as e:
        return {
            "success": False,
            "error": f"Failed to open {file_type}: {e}",
            "_hint": "Call fusion_screenshot to check for a modal dialog blocking Fusion, then fusion_dismiss_blocking_dialogs to answer it, then retry.",
            "data": {
                "filePath": file_path,
                "hint": ("PATH WITH SPACES? This verb now quotes the path and falls back to the "
                         "Windows 8.3 short path, so spaces should work (issue #196). If it still "
                         "fails, verify the file EXISTS at exactly this path and is valid, then try "
                         "a space-free copy to isolate it. Otherwise Fusion may not support this "
                         "file, or it is not fully loaded/responsive."),
            },
        }

    time.sleep(1.0)

    # Get workspace info to confirm we landed in the right editor
    ws_info = {}
    try:
        active_ws = app.executeTextCommand("DebugCommands.ActiveWorkspace")
        ws_info["activeWorkspace"] = active_ws.strip() if active_ws else "unknown"
    except Exception:
        pass
    try:
        doc = app.activeDocument
        if doc:
            ws_info["activeDocument"] = doc.name
    except Exception:
        pass

    # Build helpful next-steps based on file type
    next_steps = {
        "schematic": [
            "Use fusion_electron_run to send EAGLE commands (e.g. WINDOW FIT, DISPLAY ALL)",
            "Use fusion_take_screenshot to capture the schematic view",
            "Use fusion_close_document to close without saving when done",
        ],
        "board layout": [
            "Use fusion_show_3d_board to switch to 3D PCB visualization",
            "Use fusion_electron_run to send EAGLE commands (e.g. WINDOW FIT, RATSNEST)",
            "Use fusion_take_screenshot to capture the board view",
            "Use fusion_close_document to close without saving when done",
        ],
    }

    return {
        "success": True,
        "output": f"Opened {file_type}: {os.path.basename(file_path)}",
        "message": f"Opened {file_type}: {os.path.basename(file_path)}. "
                   f"IMPORTANT: Fusion may show a blocking dialog that is NOT visible to the API. "
                   f"You MUST screenshot the Fusion window now to verify no dialog is blocking.",
        "data": {
            "filePath": file_path,
            "fileType": file_type,
            "nextSteps": next_steps.get(file_type, []),
            **ws_info,
        },
        "postOpenCheck": {
            "required": True,
            "steps": [
                "Run desktop_list_windows to find the Fusion HWND",
                "Run desktop_screenshot_window with that HWND",
                "If a dialog is visible, answer it with fusion_dismiss_blocking_dialogs",
                "Screenshot again to confirm the dialog is gone",
            ],
        },
    }


def handle_open_schematic(app: adsk.core.Application, args: dict) -> dict:
    """Open a schematic (.sch) file in Fusion 360's Schematic Editor.

    PCB/ELECTRONICS NAV RULE: a board is NOT a standalone file - the schematic, 2D board and 3D board are VIEWS inside ONE electronics DESIGN. Open the DESIGN document first (fusion_open_cloud_file or fusion_open_by_urn), THEN switch to its schematic and its 2D board (fusion_show_2d_board) and 3D board (fusion_show_3d_board). A Select-Electronics-Design-File picker IS that design list of its schematic + board. An EMPTY 2D board usually means you opened a derivative or the wrong file, or it is out of sync - reopen the parent electronics DESIGN and switch to its board view; do NOT open a .brd or .sch as a standalone file.

    The schematic opens in the Electronics workspace where you can use
    EAGLE commands via fusion_electron_run (e.g. WINDOW FIT, DISPLAY ALL).

    Args:
        args: Dict with:
            - filePath: Path to the .sch file on the Windows desktop (required).
    """
    return _open_electronics(app, args.get("filePath", ""), "schematic", ".sch")


def handle_open_board(app: adsk.core.Application, args: dict) -> dict:
    """Open a board layout (.brd) file in Fusion 360's Board Layout editor.

    PCB/ELECTRONICS NAV RULE: a board is NOT a standalone file - the schematic, 2D board and 3D board are VIEWS inside ONE electronics DESIGN. Open the DESIGN document first (fusion_open_cloud_file or fusion_open_by_urn), THEN switch to its schematic and its 2D board (fusion_show_2d_board) and 3D board (fusion_show_3d_board). A Select-Electronics-Design-File picker IS that design list of its schematic + board. An EMPTY 2D board usually means you opened a derivative or the wrong file, or it is out of sync - reopen the parent electronics DESIGN and switch to its board view; do NOT open a .brd or .sch as a standalone file.

    Opens the 2D board layout where you can see traces, pads, and components.
    Use fusion_show_3d_board to switch to the 3D PCB visualization.
    Use fusion_electron_run {"command": "BOARD"} to return to 2D.

    Args:
        args: Dict with:
            - filePath: Path to the .brd file on the Windows desktop (required).
    """
    return _open_electronics(app, args.get("filePath", ""), "board layout", ".brd")


def handle_show_3d_board(app: adsk.core.Application, args: dict) -> dict:
    """Switch to the 3D PCB board view in Fusion 360.

    Must have a board (.brd) open in the Board Layout editor first.
    The board MUST have an outline on layer 20 (Dimension) — without it,
    Fusion cannot generate the 3D model.

    Uses the Fusion command Switch3dPcbDocCmd to switch workspaces.

    To return to 2D board layout, use:
        fusion_execute_text_command {"command": "Commands.Start Electron::Insert2DPCB"}
    """
    # Check current workspace
    ws = "unknown"
    try:
        ws = app.executeTextCommand("DebugCommands.ActiveWorkspace").strip()
    except Exception:
        pass

    # Already in 3D PCB?
    if ws == "3D PCB":
        return {
            "success": True,
            "output": "Already in 3D PCB view.",
            "data": {
                "activeWorkspace": ws,
                "nextSteps": [
                    "Use fusion_take_screenshot to capture the 3D board view",
                    "Use fusion_show_2d_board to return to 2D layout",
                    "Use fusion_close_document to close without saving when done",
                ],
            },
        }

    # Must be in PCB Editor (board layout) to switch to 3D
    is_pcb_editor = False
    try:
        app.executeTextCommand("Electron.run DISPLAY")
        is_pcb_editor = True
    except Exception:
        pass

    if not is_pcb_editor:
        return {
            "success": False,
            "error": f"Not in Electronics editor (current workspace: {ws}). "
                     "Open a .brd board file first with fusion_open_board.",
            "_hint": "Call fusion_open_board {\"filePath\":\"...\"} to load a board, then retry fusion_show_3d_board.",
            "data": {
                "activeWorkspace": ws,
                "hint": "You need to open a .brd file first: fusion_open_board {\"filePath\": \"C:/path/to/board.brd\"}",
            },
        }

    # Switch to 3D PCB using Fusion's workspace switch command
    try:
        result = app.executeTextCommand("Commands.Start Switch3dPcbDocCmd")
    except Exception as e:
        err_msg = str(e)
        if "outline" in err_msg.lower() or "missing" in err_msg.lower():
            return {
                "success": False,
                "error": f"3D PCB failed: {e}. The board may be missing an outline on layer 20 (Dimension).",
                "_hint": "Call fusion_electron_run with a WIRE command on layer 20 to add a board outline, then retry fusion_show_3d_board.",
                "data": {
                    "hint": "Add a board outline on layer 20 (Dimension) using WIRE command, then try again.",
                },
            }
        return {
            "success": False,
            "error": f"Switch to 3D PCB failed: {e}",
            "data": {"activeWorkspace": ws},
        }

    time.sleep(2.0)

    # Zoom to fit in 3D view
    try:
        app.executeTextCommand("Commands.Start FitCommand")
    except Exception:
        pass  # FitCommand may report missing 3D models as an "error" but still works

    # Verify we're now in 3D PCB
    new_ws = "unknown"
    try:
        new_ws = app.executeTextCommand("DebugCommands.ActiveWorkspace").strip()
    except Exception:
        pass

    if new_ws != "3D PCB":
        return {
            "success": False,
            "error": f"Switch to 3D PCB may have failed (workspace: {new_ws}). "
                     "Ensure the board has an outline on layer 20.",
            "_hint": "Call fusion_screenshot to check state, then verify the board has a layer-20 outline before retrying.",
            "data": {"activeWorkspace": new_ws},
        }

    return {
        "success": True,
        "output": "Switched to 3D PCB board view. Board outline, copper, solder mask, and component placeholders are shown in 3D.",
        "message": "Switched to 3D PCB view. IMPORTANT: Fusion may show a 'What to design?' or "
                   "'PCB out of date' dialog. You MUST screenshot the Fusion window to verify.",
        "data": {
            "activeWorkspace": new_ws,
            "nextSteps": [
                "FIRST: Screenshot Fusion window to check for blocking dialogs — answer with fusion_dismiss_blocking_dialogs",
                "Use fusion_take_screenshot to capture the 3D board view",
                "Use fusion_show_2d_board to return to 2D layout",
                "Use fusion_close_document to close without saving when done",
            ],
        },
        "postOpenCheck": {
            "required": True,
            "steps": [
                "Run desktop_list_windows to find the Fusion HWND",
                "Run desktop_screenshot_window with that HWND",
                "If a dialog is visible, answer it with fusion_dismiss_blocking_dialogs",
                "Screenshot again to confirm the dialog is gone",
            ],
        },
    }


def handle_import_electronics(app: adsk.core.Application, args: dict) -> dict:
    """Import a Fusion-native electronics file (.fsch/.fbrd/.flbr) as a new local project.

    These are Fusion 360's own electronics formats (not legacy EAGLE .sch/.brd).
    Uses Document.newDesignFromLocal to create a new document from the local file.

    After importing, use fusion_save_to_cloud to persist to the Fusion cloud.

    Args:
        args: Dict with:
            - filePath: Path to the .fsch, .fbrd, or .flbr file on Windows (required).
    """
    file_path = args.get("filePath", "")
    if not file_path:
        return {
            "success": False,
            "error": "No filePath specified. Provide a .fsch, .fbrd, or .flbr file path.",
            "data": {
                "hint": "Usage: fusion_import_electronics {\"filePath\": \"C:/path/to/file.fsch\"}",
                "supportedExtensions": [".fsch", ".fbrd", ".flbr"],
            },
        }

    file_path = file_path.replace("\\", "/")

    if not os.path.exists(file_path):
        return {
            "success": False,
            "error": f"File not found: {file_path}",
            "_hint": "The path may exist on Docker but not on the Windows host. Use push_file to copy it from Docker first, or verify the host-side path.",
            "data": {"filePath": file_path, "hint": "Check that the file path exists on the Windows desktop machine."},
        }

    ext = os.path.splitext(file_path)[1].lower()
    valid_extensions = {".fsch", ".fbrd", ".flbr"}
    if ext not in valid_extensions:
        ext_hints = {
            ".sch": "Use fusion_open_schematic for legacy EAGLE .sch files",
            ".brd": "Use fusion_open_board for legacy EAGLE .brd files",
            ".lbr": "Use fusion_open_lbr for legacy EAGLE .lbr files",
            ".step": "Use fusion_import_step for .step files",
            ".stp": "Use fusion_import_step for .stp files",
        }
        return {
            "success": False,
            "error": f"Unsupported extension '{ext}'. This command handles Fusion-native electronics formats: .fsch, .fbrd, .flbr",
            "_hint": ext_hints.get(ext, f"Supported: {', '.join(sorted(valid_extensions))}"),
            "data": {
                "hint": ext_hints.get(ext, f"Supported: {', '.join(sorted(valid_extensions))}"),
                "supportedExtensions": sorted(valid_extensions),
            },
        }

    file_type_map = {
        ".fsch": "Fusion schematic",
        ".fbrd": "Fusion board",
        ".flbr": "Fusion library",
    }
    file_type = file_type_map[ext]

    try:
        r, _open_how = _open_local_document(app, file_path)
    except Exception as e:
        return {
            "success": False,
            "error": f"Failed to import {file_type}: {e}",
            "data": {
                "filePath": file_path,
                "hint": "Fusion 360 may not support this file version. Ensure Fusion is fully loaded and responsive.",
            },
        }

    time.sleep(1.5)

    # Get workspace info to confirm we landed in the right editor
    ws_info = {}
    try:
        active_ws = app.executeTextCommand("DebugCommands.ActiveWorkspace")
        ws_info["activeWorkspace"] = active_ws.strip() if active_ws else "unknown"
    except Exception:
        pass
    try:
        doc = app.activeDocument
        if doc:
            ws_info["activeDocument"] = doc.name
    except Exception:
        pass

    next_steps = []
    if ext == ".fsch":
        next_steps = [
            "Use fusion_electron_run to send EAGLE commands (e.g. WINDOW FIT)",
            "Use fusion_save_to_cloud to persist this document to the Fusion cloud",
            "Use fusion_close_document to close without saving when done",
        ]
    elif ext == ".fbrd":
        next_steps = [
            "Use fusion_show_3d_board to switch to 3D PCB visualization",
            "Use fusion_electron_run to send EAGLE commands (e.g. WINDOW FIT, RATSNEST)",
            "Use fusion_save_to_cloud to persist this document to the Fusion cloud",
            "Use fusion_close_document to close without saving when done",
        ]
    elif ext == ".flbr":
        next_steps = [
            "Use fusion_export_lbr to verify library contents (.scr text format)",
            "Use fusion_save_to_cloud to persist this library to the Fusion cloud",
            "Use fusion_close_document to close without saving when done",
        ]

    return {
        "success": True,
        "output": f"Imported {file_type}: {os.path.basename(file_path)}",
        "message": f"Imported {file_type}: {os.path.basename(file_path)}. "
                   f"This is a NEW local document — use fusion_save_to_cloud to persist it. "
                   f"IMPORTANT: Fusion may show a blocking dialog. Screenshot to verify.",
        "data": {
            "filePath": file_path,
            "fileType": file_type,
            "extension": ext,
            "nextSteps": next_steps,
            **ws_info,
        },
        "postOpenCheck": {
            "required": True,
            "steps": [
                "Run desktop_list_windows to find the Fusion HWND",
                "Run desktop_screenshot_window with that HWND",
                "If a dialog is visible, answer it with fusion_dismiss_blocking_dialogs",
                "Screenshot again to confirm the dialog is gone",
            ],
        },
    }


def handle_show_2d_board(app: adsk.core.Application, args: dict) -> dict:
    """Switch back to the 2D board layout from 3D PCB view.

    Uses the Fusion command Electron::Insert2DPCB to return to PCB Editor.
    """
    ws = "unknown"
    try:
        ws = app.executeTextCommand("DebugCommands.ActiveWorkspace").strip()
    except Exception:
        pass

    if ws == "PCB Editor":
        return {
            "success": True,
            "output": "Already in 2D board layout.",
            "data": {"activeWorkspace": ws},
        }

    try:
        app.executeTextCommand("Commands.Start SwitchPcbDocCmd")
    except Exception as e:
        return {
            "success": False,
            "error": f"Switch to 2D board failed: {e}",
            "_hint": "Call fusion_open_board {\"filePath\":\"...\"} to ensure a board is loaded, then retry.",
            "data": {"activeWorkspace": ws},
        }

    time.sleep(1.0)

    new_ws = "unknown"
    try:
        new_ws = app.executeTextCommand("DebugCommands.ActiveWorkspace").strip()
    except Exception:
        pass

    return {
        "success": True,
        "output": "Switched to 2D board layout. EAGLE commands via fusion_electron_run will work.",
        "data": {
            "activeWorkspace": new_ws,
            "nextSteps": [
                "Use fusion_electron_run to send EAGLE commands",
                "Use fusion_show_3d_board to switch back to 3D view",
            ],
        },
    }


def handle_show_schematic(app: adsk.core.Application, args: dict) -> dict:
    """Switch the OPEN electronics design to its SCHEMATIC view.

    The schematic / 2D board / 3D board are VIEWS of ONE electronics design - open the
    PROJECT first (fusion_open_cloud_file / fusion_open_by_urn), then switch views with
    fusion_show_schematic / fusion_show_2d_board / fusion_show_3d_board. Uses Fusion's
    SwitchSchDocCmd (the symmetric partner of SwitchPcbDocCmd / Switch3dPcbDocCmd).
    Drive the schematic for demos with fusion_electron_zoom / _pan / _select.
    """
    ws = "unknown"
    try:
        ws = app.executeTextCommand("DebugCommands.ActiveWorkspace").strip()
    except Exception:
        pass

    # Already viewing the schematic?
    cur_pt = ""
    try:
        cur_pt = app.activeProduct.productType
    except Exception:
        pass
    if cur_pt == "SchematicProductType":
        return {
            "success": True,
            "output": "Already in the schematic view.",
            "data": {"activeWorkspace": ws, "productType": cur_pt},
        }

    try:
        app.executeTextCommand("Commands.Start SwitchSchDocCmd")
    except Exception as e:
        return {
            "success": False,
            "error": f"Switch to schematic failed: {e}",
            "_hint": "Open the electronics DESIGN first (fusion_open_cloud_file / fusion_open_by_urn), "
                     "then retry. The schematic is a VIEW of the design, not a standalone file.",
            "data": {"activeWorkspace": ws},
        }

    time.sleep(1.2)
    try:
        app.executeTextCommand("Electron.run WINDOW FIT")
    except Exception:
        pass

    new_ws = "unknown"
    new_pt = ""
    try:
        new_ws = app.executeTextCommand("DebugCommands.ActiveWorkspace").strip()
    except Exception:
        pass
    try:
        new_pt = app.activeProduct.productType
    except Exception:
        pass

    return {
        "success": True,
        "output": "Switched to the schematic view. Drive it with fusion_electron_zoom / _pan / _select "
                  "or fusion_electron_run WINDOW commands.",
        "message": "Switched to the schematic. Fusion may show a blocking dialog that is NOT visible to "
                   "the API - screenshot the Fusion window to verify.",
        "data": {
            "activeWorkspace": new_ws,
            "productType": new_pt,
            "nextSteps": [
                "fusion_electron_zoom / fusion_electron_pan to move the view for a recording",
                "fusion_show_2d_board / fusion_show_3d_board to switch to the board / 3D",
                "fusion_take_screenshot to capture the schematic",
            ],
        },
        "postOpenCheck": {
            "required": True,
            "steps": [
                "Run desktop_screenshot_window on the Fusion HWND",
                "If a dialog is visible, answer it with fusion_dismiss_blocking_dialogs",
            ],
        },
    }
