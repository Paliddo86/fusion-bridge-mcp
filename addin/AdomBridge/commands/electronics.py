"""Electronics commands — run EAGLE commands via Electron.run text command.

Fusion 360's Electronics workspace (inherited from EAGLE) has no Python API,
but we can use `app.executeTextCommand('Electron.run <cmd>')` to send EAGLE
editor commands into the electronics workspace.

IMPORTANT: The user must be in an Electronics editor (schematic, board, or library)
for Electron.run commands to work. The "Electronics Design" overview/container
workspace does NOT count — the user must have a schematic or board open and active.

Workspace hierarchy:
  "Design"              — Normal 3D CAD (Electron.run fails)
  "Electronics Design"  — Container/overview (Electron.run fails)
  "Schematic Editor"    — Schematic editing (Electron.run works)
  "Board Layout"        — PCB layout editing (Electron.run works)
  "Electronics Library" — Library editing (Electron.run works)

Commands that BLOCK (open dialogs — avoid from automation):
  WRITE               — Opens Save As dialog
  ADD                 — Opens component picker dialog
  SHOW                — bare SHOW waits for a click (blocked); SHOW <name> highlights and returns (allowed)
  GRID (no params)    — Opens grid settings dialog
  EDIT <new>.sym      — Opens "Create new?" confirmation dialog

Safe automation patterns:
  SCRIPT '<path>.scr' — Runs batch commands from file (non-interactive)
  EXPORT SCRIPT       — Exports library/schematic to .scr file (for verification)
  DISPLAY             — Shows/hides layers (non-interactive)
  WINDOW FIT          — Zoom to fit
  EDIT <existing>.sym — Opens existing symbol (no dialog)
  SET CONFIRM YES     — In .scr: suppresses create-new confirmations
"""

import adsk.core
import glob
import os
import re
import tempfile


def _get_fusion_log_path() -> str | None:
    """Find the latest Fusion 360 log file."""
    log_dir = os.path.join(
        os.environ.get("LOCALAPPDATA", ""),
        "Autodesk", "Autodesk Fusion 360",
    )
    # Find user-specific log dir (e.g. X3G293QFLPJERKGZ/logs/)
    for entry in os.listdir(log_dir):
        candidate = os.path.join(log_dir, entry, "logs")
        if os.path.isdir(candidate):
            logs = sorted(glob.glob(os.path.join(candidate, "AppLogFile*.log")),
                          key=os.path.getmtime, reverse=True)
            if logs:
                return logs[0]
    return None


def _get_log_line_count(log_path: str) -> int:
    """Get current line count of a log file (fast — seeks to end)."""
    try:
        with open(log_path, "r", encoding="utf-8", errors="replace") as f:
            return sum(1 for _ in f)
    except Exception:
        return 0


def _check_log_for_alerts(log_path: str, after_line: int) -> list[str]:
    """Check Fusion log for 'New Alert message' entries after a given line number.

    Returns list of alert message texts (the error toast content).
    """
    alerts = []
    try:
        with open(log_path, "r", encoding="utf-8", errors="replace") as f:
            for i, line in enumerate(f, 1):
                if i <= after_line:
                    continue
                if "New Alert message" in line:
                    # Extract: "Tooltip: <msg>, Short Message: <msg>"
                    m = re.search(r"Short Message:\s*(.+?)(?:,\s*Long Message:|$)", line)
                    if m:
                        alerts.append(m.group(1).strip())
                    else:
                        # Fallback: grab everything after "Tooltip: "
                        m2 = re.search(r"Tooltip:\s*(.+?)(?:,|$)", line)
                        if m2:
                            alerts.append(m2.group(1).strip())
    except Exception:
        pass
    return alerts


def _extract_export_path(eagle_cmd: str) -> str | None:
    """Extract the output file path from an EAGLE EXPORT command.

    EAGLE EXPORT commands use single-quoted paths:
      EXPORT NETLIST 'C:/tmp/netlist.txt'
      EXPORT IMAGE 'C:/tmp/board.png' 300
      EXPORT PARTLIST 'C:/tmp/parts.txt'

    Returns the unquoted path, or None if not found.
    """
    # Match single-quoted path
    m = re.search(r"'([^']+)'", eagle_cmd)
    if m:
        return m.group(1)
    # Fallback: look for unquoted path-like tokens (with / or \ and an extension)
    parts = eagle_cmd.strip().split()
    for part in parts[2:]:  # Skip "EXPORT" and subcommand
        cleaned = part.strip("'\"")
        if ("/" in cleaned or "\\" in cleaned) and "." in cleaned:
            return cleaned
    return None


def _get_workspace_info(app: adsk.core.Application) -> dict:
    """Get current workspace info and whether Electronics Electron editor is active."""
    try:
        active_ws = app.executeTextCommand("DebugCommands.ActiveWorkspace")
    except Exception:
        active_ws = "unknown"

    is_electron = False
    try:
        app.executeTextCommand("Electron.run DISPLAY")
        is_electron = True
    except Exception:
        pass

    # Get document info
    doc_info = None
    try:
        doc = app.activeDocument
        if doc:
            doc_info = {"name": doc.name, "type": str(doc.documentType)}
    except Exception:
        pass

    # Get product type
    product_type = None
    try:
        product = app.activeProduct
        if product:
            product_type = product.productType
    except Exception:
        pass

    info = {
        "activeWorkspace": active_ws.strip() if active_ws else "unknown",
        "isElectronics": is_electron,
    }
    if doc_info:
        info["document"] = doc_info
    if product_type:
        info["productType"] = product_type
    return info


def _get_recent_operations(app: adsk.core.Application, count: int = 5) -> list:
    """Get recent Fusion operations log via Diagnostics.RecentOperations."""
    try:
        result = app.executeTextCommand(f"Diagnostics.RecentOperations {count}")
        if result:
            lines = [l.strip() for l in result.strip().split("\r\n") if l.strip()]
            # Filter to just command begin/end, skip DLL loads
            return [l for l in lines if l.startswith("Command ")]
        return []
    except Exception:
        return []


def _query_post_command_state(app: adsk.core.Application, eagle_cmd: str, ops_before: list) -> dict:
    """Query Fusion state after an Electron.run command to return meaningful feedback.

    Uses Diagnostics.RecentOperations to diff what actually happened, plus
    workspace/document state for context.
    """
    import time
    time.sleep(0.3)  # Brief pause for command to take effect

    state = {}

    # Always include workspace info
    try:
        active_ws = app.executeTextCommand("DebugCommands.ActiveWorkspace")
        state["activeWorkspace"] = active_ws.strip() if active_ws else "unknown"
    except Exception:
        state["activeWorkspace"] = "unknown"

    # Active document
    try:
        doc = app.activeDocument
        if doc:
            state["activeDocument"] = doc.name
    except Exception:
        pass

    # Diff operations log — what commands fired as a result?
    ops_after = _get_recent_operations(app, 10)
    if ops_before and ops_after:
        # Find new operations that weren't in the before snapshot
        new_ops = []
        for op in ops_after:
            if op not in ops_before:
                new_ops.append(op)
        if new_ops:
            state["fusionOperations"] = new_ops[:5]  # Cap at 5 to keep response size sane

    # Parse command to provide targeted feedback
    cmd_parts = eagle_cmd.strip().split(None, 1)
    cmd_verb = cmd_parts[0].upper() if cmd_parts else ""
    cmd_upper = eagle_cmd.strip().upper()

    # Command-specific hints and extra queries
    if cmd_verb == "DISPLAY":
        state["commandType"] = "layer_visibility"
        state["hint"] = "DISPLAY command executed. Use fusion_take_screenshot to verify layer changes visually."

    elif cmd_verb == "WINDOW":
        state["commandType"] = "view_control"
        state["hint"] = "Zoomed to fit all content in view." if "FIT" in cmd_upper else "View position/zoom changed."

    elif cmd_verb == "EDIT":
        target = cmd_parts[1] if len(cmd_parts) > 1 else ""
        state["commandType"] = "edit"
        ext_map = {".sym": "symbol", ".pac": "package", ".dev": "device"}
        for ext, editor in ext_map.items():
            if target.endswith(ext):
                state["hint"] = f"Opened {editor} editor for: {target}"
                state["editorType"] = editor
                break
        else:
            state["hint"] = f"Opened editor for: {target}"

    elif cmd_verb == "RATSNEST":
        state["commandType"] = "ratsnest"
        state["hint"] = "Ratsnest (airwires) recalculated."

    elif cmd_verb == "BOARD":
        state["commandType"] = "view_switch"
        state["hint"] = "Requested switch to board layout. Verify with a screenshot."

    elif cmd_verb == "SET":
        state["commandType"] = "setting"
        state["hint"] = f"Setting changed: {eagle_cmd.strip()}"

    elif cmd_verb == "SCRIPT":
        state["commandType"] = "script"
        state["hint"] = f"Script executed: {cmd_parts[1] if len(cmd_parts) > 1 else '(unknown)'}"

    elif cmd_verb == "USE":
        state["commandType"] = "library_load"
        state["hint"] = f"Library load requested: {cmd_parts[1] if len(cmd_parts) > 1 else '(unknown)'}"

    elif cmd_verb in ("NAME", "VALUE", "SMASH", "MOVE", "ROTATE", "DELETE", "COPY", "GROUP",
                       "ROUTE", "RIPUP", "WIRE", "CIRCLE", "RECT", "POLYGON", "TEXT", "ARC",
                       "PAD", "SMD", "VIA", "HOLE", "PIN", "LABEL", "NET", "BUS", "JUNCTION"):
        state["commandType"] = "design_edit"
        state["hint"] = f"Design edit command executed: {cmd_verb}"

    elif cmd_verb == "EXPORT":
        state["commandType"] = "export"
        # Try to verify the export actually produced a file
        # Parse output path from command: EXPORT <TYPE> <path> [options]
        export_parts = eagle_cmd.strip().split()
        export_path = None
        if len(export_parts) >= 3:
            # The path is typically the 3rd token (after EXPORT <TYPE>)
            candidate = export_parts[2]
            # Handle quoted paths
            if candidate.startswith(("'", '"')):
                quote = candidate[0]
                rest = eagle_cmd.strip().split(quote)
                if len(rest) >= 3:
                    export_path = rest[1]
            else:
                export_path = candidate

        if export_path:
            import os
            if os.path.exists(export_path):
                fsize = os.path.getsize(export_path)
                state["hint"] = f"Export succeeded: {export_path} ({fsize} bytes)"
                state["exportedFile"] = export_path
                state["exportedFileSize"] = fsize
            else:
                state["hint"] = (
                    f"Export command ran but output file NOT found at: {export_path}. "
                    "The EAGLE command may have failed silently — check Fusion's error toast or operations log."
                )
                state["exportFailed"] = True
                state["expectedPath"] = export_path
        else:
            state["hint"] = f"Export command executed: {eagle_cmd.strip()}"

    else:
        state["commandType"] = "generic"
        state["hint"] = f"Command executed: {eagle_cmd.strip()}. Use fusion_take_screenshot to verify the result visually."

    # If no Fusion operations fired, the command likely failed silently.
    # EAGLE's Electron.run never returns errors — failures only show as
    # toast dialogs on the user's screen that are invisible to the API.
    # We check Fusion's error log for alert toasts (see handle_electron_run),
    # but log writes are async so errors may appear after our check window.
    if "fusionOperations" not in state or not state["fusionOperations"]:
        state["possibleFailure"] = True
        state["hint"] = (
            f"WARNING: No Fusion operations detected after '{cmd_verb}'. "
            f"The command likely failed silently. Check the 'alertErrors' field "
            f"in this response — if populated, it contains the actual Fusion error "
            f"message captured from the log. If 'alertErrors' is empty, the error "
            f"may not have been logged yet; take a screenshot with "
            f"fusion_take_screenshot to see the error toast in Fusion's UI. "
            f"Original hint: {state.get('hint', '')}"
        )

    return state


def handle_electron_run(app: adsk.core.Application, args: dict) -> dict:
    """Execute an EAGLE command in the Electronics workspace via Electron.run.

    Requires an active Electronics editor (schematic, board, or library).
    Returns workspace state + Fusion operations log diff after execution.

    Args:
        args: Dict with:
            - command: The EAGLE command to execute (required).
              Examples: "EDIT resistor.sym", "USE mylib.lbr", "WINDOW FIT"
            - confirm: "yes" | "no" (optional). Pre-answers any confirmation the command
              would raise, via SET CONFIRM, so the modal is never drawn. "yes" = we mean to
              create; "no" = inspect only. EDIT with no `confirm` defaults to "no".
    """
    eagle_cmd = args.get("command", "")
    if not eagle_cmd:
        return {"success": False, "error": "No command specified. Provide an EAGLE command string."}

    # Check we're in Electronics before executing
    ws_info = _get_workspace_info(app)
    if not ws_info["isElectronics"]:
        return {
            "success": False,
            "error": f"Not in Electronics editor (current: {ws_info['activeWorkspace']}). "
                     "Open a schematic or board in Fusion 360's Electronics workspace first.",
            "_hint": "Call fusion_open_schematic {\"filePath\":\"...\"}, fusion_open_board {\"filePath\":\"...\"}, or fusion_open_lbr {\"filePath\":\"...\"} to enter the Electronics editor, then retry.",
            "data": {
                **ws_info,
                "hint": "Use fusion_open_schematic, fusion_open_board, or fusion_open_lbr to open an electronics file first.",
            },
        }

    # Parse the first word as the command verb for validation and routing.
    cmd_verb = eagle_cmd.strip().split()[0].upper() if eagle_cmd.strip() else ""

    # Block commands that don't work in Fusion's Electronics workspace or that
    # pop up error dialogs on the user's screen with no useful result.
    BLOCKED_COMMANDS = {
        "SCHEMATIC": "Use fusion_show_2d_board or fusion_electron_run '{\"command\": \"BOARD\"}' to switch between schematic/board views. "
                     "The SCHEMATIC command doesn't exist in Fusion's EAGLE integration.",
        "ADD": "Opens an interactive component picker dialog that blocks automation. "
               "Place components through the Fusion UI instead.",
        "SHOW": "Opens interactive highlight mode that blocks automation. "
                "Use fusion_board_info to query component/net data instead.",
        "CHANGE": "A bare or open-ended CHANGE waits for clicks. Give the property, its value and a closing "
                  "semicolon, optionally with the points to apply it at: CHANGE WIDTH 0; or CHANGE LAYER 48 (12.5 7.25);",
        "MOVE": "Opens interactive move mode — requires clicking to place. "
                "Not suitable for CLI automation. Use the Fusion API to move components programmatically.",
    }

    # WRITE always opens a blocking "Version Description" dialog on cloud docs,
    # even when a path argument is provided. Tested 2026-04-10 — WRITE <path>
    # still triggers the dialog. Block unconditionally.
    if cmd_verb == "WRITE":
        return {
            "success": False,
            "error": "Command 'WRITE' is blocked — it opens a blocking 'Version Description' save dialog "
                     "on cloud documents, even when a path argument is provided (tested 2026-04-10). "
                     "Use fusion_export_source for .fsch/.fbrd export, or fusion_export_eagle_source for .sch/.brd.",
            "_hint": "For Fusion-format source: fusion_export_source {\"outputPath\":\"C:/out/file.fbrd\"}. "
                     "For EAGLE-format source: fusion_export_eagle_source {\"outputPath\":\"C:/out/file.brd\"}.",
            "data": {
                "command": eagle_cmd,
                "blockedVerb": "WRITE",
                "activeWorkspace": ws_info["activeWorkspace"],
            },
        }

    # CHANGE PACKAGE with EVERYTHING on the line ("CHANGE PACKAGE 'QFN-48_7X7MM-HV' (14 7.7)")
    # is a complete, non-interactive command: it swaps the placed part's package variant
    # without a dialog (John, 2026-08-30: the reason variants live in one deviceset). Only
    # a bare CHANGE (which enters an interactive mode) stays blocked.
    _swap = re.match(r"^\s*CHANGE\s+PACKAGE\s+'[^']+'\s*\(\s*-?[0-9.]+\s+-?[0-9.]+\s*\)\s*;?\s*$",
                     eagle_cmd or "", re.I)
    # SHOW with an explicit object name ("SHOW NET_6", "SHOW GND", "SHOW R1") is a complete,
    # non-interactive command: it highlights the named objects and returns; only a bare SHOW
    # (which waits for a click) blocks automation. Issue #81: the acceptance thread's explicit
    # net highlight was refused while the published videos used exactly this form.
    _show_named = re.match(r"^\s*SHOW\s+[^;\s]+.*$", eagle_cmd or "", re.I)
    # CHANGE <property> <value> ending in ';' (optionally with points) is complete: the ';' ends
    # the command, so it sets the value (and applies it at the given points) without waiting for a
    # click. The bridge's own routing sends "CHANGE DRILL 0.3mm; VIA ..." exactly this way.
    # Page issue #123: "CHANGE WIDTH 0;" and "CHANGE LAYER 48 (x y);" were refused.
    _change_complete = re.match(r"^\s*CHANGE\s+[A-Za-z_]+\s+[^;\s(][^;]*;\s*$", eagle_cmd or "", re.I)
    if cmd_verb in BLOCKED_COMMANDS and not _swap and not _show_named and not _change_complete:
        return {
            "success": False,
            "error": f"Command '{cmd_verb}' is blocked for automation: {BLOCKED_COMMANDS[cmd_verb]}",
            "_hint": f"'{cmd_verb}' opens an interactive dialog. Use fusion_board_info / fusion_save_to_cloud / the Fusion API alternatives instead of this EAGLE command.",
            "data": {
                "command": eagle_cmd,
                "blockedVerb": cmd_verb,
                "hint": BLOCKED_COMMANDS[cmd_verb],
                "activeWorkspace": ws_info["activeWorkspace"],
            },
        }

    # ⛔ DECLARE CONFIRM INTENT BEFORE ANY COMMAND THAT CAN PROMPT.
    # John, 2026-08-26 (3rd report of the same modal): "why do you still allow these dialogs
    # to pop up ... dialogs are dangerous cuz they're usually modals and kill programmatic
    # code from running ... and it dings the computer's speaker."
    # EAGLE/Electron has a deterministic answer for this and we were only using it for
    # EXPORT: SET CONFIRM pre-answers the confirmation so the modal is NEVER DRAWN. There is
    # no third state - a command that can prompt must say which answer it wants:
    #   confirm="yes" -> we INTEND to create (EDIT <new>.pac to author a footprint)
    #   confirm="no"  -> we are only INSPECTING; a prompt means a bad name, decline it
    # The caller declaring intent is what makes this deterministic instead of a race with a
    # dismissal sweep, which loses whenever Fusion is slow.
    _confirm = str(args.get("confirm") or "").strip().lower()
    if _confirm in ("yes", "no"):
        try:
            app.executeTextCommand("Electron.run SET CONFIRM %s" % _confirm.upper())
        except Exception:
            pass  # best effort; the server-side name pre-check is the primary guard
    elif cmd_verb == "EDIT":
        # An EDIT with no declared intent is the exact call that raised John's dialog. We
        # cannot know whether a create is wanted, so refuse to gamble the main thread: the
        # safe answer is NO (decline the create), which also leaves no stray artifact.
        try:
            app.executeTextCommand("Electron.run SET CONFIRM NO")
        except Exception:
            pass

    # Auto-suppress overwrite/confirmation dialogs for EXPORT commands.
    # Without this, EAGLE shows a Qt "Overwrite?" dialog that blocks the
    # add-in thread and is invisible to the CLI/AI — freezing everything.
    export_target_path = None
    if cmd_verb == "EXPORT":
        try:
            app.executeTextCommand("Electron.run SET CONFIRM YES")
        except Exception:
            pass  # Best effort — don't block if SET fails

        # Extract the output file path from the EXPORT command.
        # EAGLE EXPORT syntax: EXPORT <type> '<path>' [options]
        # Paths are typically single-quoted: EXPORT NETLIST '/path/to/file.txt'
        export_target_path = _extract_export_path(eagle_cmd)
        if export_target_path and os.path.exists(export_target_path):
            try:
                os.remove(export_target_path)
            except PermissionError:
                return {
                    "success": False,
                    "error": f"Cannot overwrite '{export_target_path}' — file is locked by another process.",
                    "_hint": "Choose a different outputPath (e.g., append a timestamp) and retry the EXPORT command.",
                    "data": {
                        "command": eagle_cmd,
                        "filePath": export_target_path,
                        "hint": "The target file exists and is locked. Close any program using it, "
                                "or choose a different output path.",
                        "nextSteps": [
                            f"Delete or rename '{export_target_path}' manually",
                            "Or use a different output path in the EXPORT command",
                        ],
                    },
                }
            except Exception as e:
                return {
                    "success": False,
                    "error": f"Cannot remove existing file '{export_target_path}': {e}",
                    "data": {
                        "command": eagle_cmd,
                        "filePath": export_target_path,
                        "hint": "Pre-delete failed. Use a different output path to avoid the overwrite dialog.",
                    },
                }

    # Snapshot operations log and Fusion error log BEFORE execution
    ops_before = _get_recent_operations(app, 10)
    log_path = _get_fusion_log_path()
    log_line_before = _get_log_line_count(log_path) if log_path else 0

    try:
        raw_result = app.executeTextCommand(f"Electron.run {eagle_cmd}")
        if cmd_verb == "WINDOW" and eagle_cmd.strip().rstrip(";").strip().upper() != "WINDOW":
            # a view command is not presented until the next command arrives: send the bare
            # WINDOW (EAGLE's redraw) so a capture or a recording right after this call shows
            # the new view, and forget any tracked pan box (2026-09-05)
            try:
                from . import electron_view as _ev
                _ev._repaint(app)
                _ev.forget_view()
            except Exception:
                pass
    except Exception as e:
        return {
            "success": False,
            "error": f"Electron.run failed: {e}",
            "_hint": "Check the active workspace with fusion_execute_text_command {\"command\": \"DebugCommands.ActiveWorkspace\"} — EAGLE commands need the right editor (schematic vs board vs library). Call fusion_open_schematic / fusion_open_board / fusion_open_lbr to switch.",
            "data": {
                "command": eagle_cmd,
                "hint": "The command may be invalid or require a specific editor context (schematic vs board vs library).",
            },
        }

    # Query state after execution — includes ops log diff
    post_state = _query_post_command_state(app, eagle_cmd, ops_before)

    # Check Fusion's error log for alert messages that appeared after the command.
    # These are the toast errors shown in the lower-right of Fusion's UI that
    # Electron.run silently swallows — the only way to detect them is via the log.
    # Fusion writes to the log asynchronously, so we poll with increasing delays.
    alert_errors = []
    if log_path:
        import time
        for _attempt in range(4):
            time.sleep(0.3 if _attempt == 0 else 0.5)
            alert_errors = _check_log_for_alerts(log_path, log_line_before)
            if alert_errors:
                break

    # Detect export failures (command ran but output file not created)
    succeeded = True
    if post_state.get("exportFailed"):
        succeeded = False

    # If Fusion logged alert errors, the command failed
    if alert_errors:
        succeeded = False
        post_state["alertErrors"] = alert_errors
        post_state["hint"] = (
            f"EAGLE error: {alert_errors[0]}. "
            f"Original hint: {post_state.get('hint', '')}"
        )

    # For EXPORT commands, verify the output file exists and report size
    export_data = {}
    if cmd_verb == "EXPORT" and export_target_path:
        import time
        time.sleep(0.5)  # Give EAGLE a moment to flush to disk
        if os.path.exists(export_target_path):
            fsize = os.path.getsize(export_target_path)
            export_data = {
                "exportedFile": export_target_path,
                "fileSizeBytes": fsize,
                "fileSizeKB": fsize // 1024,
            }
            succeeded = True  # Override exportFailed if file actually exists
        else:
            succeeded = False
            export_data = {
                "exportedFile": export_target_path,
                "exportFailed": True,
                "hint": f"EXPORT command ran but file was not created at '{export_target_path}'. "
                        "This EXPORT subcommand may not be supported in the current editor context.",
            }

    result = {
        "success": succeeded,
        "error": alert_errors[0] if alert_errors else "",
        "output": post_state.get("hint", f"Executed: {eagle_cmd}"),
        "data": {
            "command": eagle_cmd,
            "rawResult": raw_result if raw_result else "",
            "alertErrors": alert_errors,
            **post_state,
            **export_data,
        },
    }

    # If we suspect failure but didn't catch a log error, hint the AI
    # that errors from Electron.run are async — the real error may appear
    # in the next command's alertErrors or in a screenshot.
    if post_state.get("possibleFailure") and not alert_errors:
        result["data"]["asyncErrorHint"] = (
            "Electron.run is fire-and-forget — errors only appear as toast "
            "dialogs in Fusion's UI. The error log was checked but nothing "
            "appeared yet. Run fusion_take_screenshot to see the current "
            "Fusion state and check for error toasts."
        )

    return result


def handle_open_electronics(app: adsk.core.Application, args: dict) -> dict:
    """Check Electronics workspace status and optionally open a file.

    PCB/ELECTRONICS NAV RULE: a board is NOT a standalone file - the schematic, 2D board and 3D board are VIEWS inside ONE electronics DESIGN. Open the DESIGN document first (fusion_open_cloud_file or fusion_open_by_urn), THEN switch to its schematic and its 2D board (fusion_show_2d_board) and 3D board (fusion_show_3d_board). A Select-Electronics-Design-File picker IS that design list of its schematic + board. An EMPTY 2D board usually means you opened a derivative or the wrong file, or it is out of sync - reopen the parent electronics DESIGN and switch to its board view; do NOT open a .brd or .sch as a standalone file.

    Args:
        args: Dict with:
            - type: "status" to just check (default). Other values reserved
                   for future use (workspace switching handled by bridge server).
            - filePath: Optional path to a .sch, .brd, or .lbr file to open
                       (only works if already in Electronics editor).
    """
    file_path = args.get("filePath", "")
    ws_info = _get_workspace_info(app)

    # If already in electronics, handle file opening or return status
    if ws_info["isElectronics"]:
        if file_path:
            try:
                result = app.executeTextCommand(f"Electron.run EDIT '{file_path}'")
                return {
                    "success": True,
                    "output": f"Opened {file_path} in Electronics workspace.",
                    "data": {"result": result if result else "", **ws_info},
                }
            except Exception as e:
                return {
                    "success": False,
                    "error": f"Failed to open {file_path}: {e}",
                    "_hint": "Verify the file exists on the Windows host, then try fusion_open_schematic or fusion_open_board with the correct filePath.",
                }
        return {
            "success": True,
            "output": "Electronics editor is active. Electron.run commands will work.",
            "data": ws_info,
        }

    # Not in Electronics — report status
    return {
        "success": True,
        "output": f"Not in Electronics editor (current workspace: {ws_info['activeWorkspace']}).",
        "data": ws_info,
    }


def handle_export_source(app: adsk.core.Application, args: dict) -> dict:
    """Export the active electronics document as .fsch, .fbrd, or .flbr source file.

    Uses Fusion's Document.CopyToDesktop text command, which exports the
    currently active document in its native format based on the file extension.

    PREREQUISITES — the caller MUST set up the correct view before calling:

    To export .fbrd (board):
      1. Open the .fprj:  fusion_open_cloud_file {"projectName":"Main", "fileName":"MyDesign", "fileExtension":"fprj", ...}
      2. Enter board view: fusion_show_2d_board
      3. Export:           fusion_export_source {"outputPath": "C:/out/MyDesign.fbrd"}

    To export .fsch (schematic):
      4. Switch to schematic: fusion_electron_run {"command": "EDIT .s1"}
      5. Export:              fusion_export_source {"outputPath": "C:/out/MyDesign.fsch"}

    To export .flbr (library):
      1. Open the .lbr:   fusion_open_lbr {"filePath": "C:/path/to/lib.lbr"}
      2. Export:           fusion_export_source {"outputPath": "C:/out/MyLib.flbr"}

    IMPORTANT GOTCHAS:
    - You MUST open the .fprj file first, NOT .fbrd/.fsch directly
    - You MUST call show_2d_board before exporting .fbrd (the 3D board view won't work)
    - To switch between board and schematic: use EDIT .s1 (schematic) or EDIT .brd (board)
    - The active workspace MUST be "Board Layout" for .fbrd or "Schematic Editor" for .fsch
    - If the wrong workspace is active, the file will not be created
    - After opening cloud file, wait for the document to fully load before exporting
    - Always screenshot after open to check for blocking dialogs before exporting

    Args:
        args: Dict with:
            - outputPath: Required. Full path including extension (.fsch, .fbrd, or .flbr).
              The extension determines the export format.
    """
    import os

    output_path = args.get("outputPath", "").strip()
    if not output_path:
        return {
            "success": False,
            "error": "Missing required arg: outputPath",
            "_hint": "Provide outputPath with extension .fsch, .fbrd, or .flbr. "
                     "For plain EAGLE format (.sch/.brd), use fusion_export_eagle_source instead.",
        }

    # Validate extension
    ext = os.path.splitext(output_path)[1].lower()
    valid_exts = {".fsch", ".fbrd", ".flbr"}
    if ext not in valid_exts:
        return {
            "success": False,
            "error": f"Invalid extension '{ext}'. Must be one of: {', '.join(sorted(valid_exts))}",
            "_hint": "Use .fsch for schematic source, .fbrd for board source, .flbr for library source. "
                     "For plain EAGLE format (.sch/.brd), use fusion_export_eagle_source instead.",
        }

    # Ensure output directory exists
    out_dir = os.path.dirname(output_path)
    if out_dir:
        os.makedirs(out_dir, exist_ok=True)

    # Normalize path for Fusion (use forward slashes)
    fusion_path = output_path.replace("\\", "/")

    try:
        result = app.executeTextCommand(f"Document.CopyToDesktop {fusion_path}")
    except Exception as e:
        return {
            "success": False,
            "error": f"Document.CopyToDesktop failed: {e}",
            "_hint": "Ensure the correct workspace is active: Board Layout for .fbrd, Schematic Editor for .fsch, Electronics Library for .flbr. Call fusion_show_2d_board or fusion_electron_run {\"command\":\"EDIT .s1\"} to switch.",
        }

    # Verify file was created
    # Fusion may use the exact path or normalize it
    check_path = output_path.replace("/", "\\")
    if not os.path.exists(check_path) and not os.path.exists(output_path):
        return {
            "success": False,
            "error": f"Command ran but file not found at {output_path}. "
                     "Make sure the correct editor is active (Board Layout for .fbrd, "
                     "Schematic Editor for .fsch).",
            "_hint": "Call fusion_show_2d_board (for .fbrd) or fusion_electron_run {\"command\":\"EDIT .s1\"} (for .fsch) to activate the right workspace, then retry.",
            "data": {"commandResult": result.strip() if result else ""},
        }

    actual_path = check_path if os.path.exists(check_path) else output_path
    size_bytes = os.path.getsize(actual_path)

    # Get active document context
    doc_name = ""
    workspace = ""
    try:
        doc = app.activeDocument
        if doc:
            doc_name = doc.name
        workspace = app.executeTextCommand("DebugCommands.ActiveWorkspace").strip()
    except Exception:
        pass

    return {
        "success": True,
        "output": f"Exported {ext} source ({size_bytes:,} bytes) to {output_path}",
        "data": {
            "savedTo": output_path.replace("\\", "/"),
            "sizeBytes": size_bytes,
            "format": ext.lstrip("."),
            "activeDocument": doc_name,
            "activeWorkspace": workspace,
        },
    }


def handle_export_eagle_source(app: adsk.core.Application, args: dict) -> dict:
    """Export the active electronics document as plain EAGLE .sch or .brd XML.

    Fusion's .fsch/.fbrd files are ZIP containers with a real EAGLE .sch/.brd
    XML file inside at `electron.BlobParts/ExtFile.*.{sch,brd}`. This command:
      1. Exports the .fsch/.fbrd via Document.CopyToDesktop (same as fusion_export_source)
      2. Extracts the inner EAGLE XML from the ZIP
      3. Writes it to outputPath as a plain EAGLE file
      4. Cleans up the intermediate .fsch/.fbrd

    The resulting file is valid EAGLE XML (`<?xml ...><eagle version="9.7.0">...`)
    that can be opened by standalone EAGLE, KiCad (via import), or parsed by any
    XML tool.

    PREREQUISITES — same as fusion_export_source:
      Board (.brd):  must be in "Board Layout" / "PCB Editor" workspace
      Schematic (.sch): must be in "Schematic Editor" workspace

    Args:
        args: Dict with:
            - outputPath: Required. Full path ending in .sch or .brd.
    """
    import tempfile
    import zipfile

    output_path = args.get("outputPath", "").strip()
    if not output_path:
        return {"success": False, "error": "Missing required arg: outputPath"}

    ext = os.path.splitext(output_path)[1].lower()
    valid_exts = {".sch": ".fsch", ".brd": ".fbrd"}
    if ext not in valid_exts:
        return {
            "success": False,
            "error": f"Invalid extension '{ext}'. Must be .sch or .brd.",
            "_hint": "For Fusion-format source (.fsch/.fbrd/.flbr), use fusion_export_source instead.",
        }

    fusion_ext = valid_exts[ext]  # .sch -> .fsch, .brd -> .fbrd

    # Export to a temp .fsch/.fbrd first
    tmp_dir = tempfile.mkdtemp(prefix="adom-eagle-")
    tmp_fusion_path = os.path.join(tmp_dir, f"temp{fusion_ext}")
    fusion_path = tmp_fusion_path.replace("\\", "/")

    try:
        result = app.executeTextCommand(f"Document.CopyToDesktop {fusion_path}")
    except Exception as e:
        return {
            "success": False,
            "error": f"Document.CopyToDesktop failed: {e}",
            "_hint": "Ensure the correct workspace is active: PCB Editor / Board Layout for .brd, "
                     "Schematic Editor for .sch. Use fusion_show_2d_board or "
                     "fusion_electron_run {\"command\":\"EDIT .s1\"} to switch.",
        }

    # Check if the file was created
    check_path = tmp_fusion_path.replace("/", "\\")
    actual_tmp = None
    for p in [check_path, tmp_fusion_path]:
        if os.path.exists(p):
            actual_tmp = p
            break

    if not actual_tmp:
        return {
            "success": False,
            "error": f"Document.CopyToDesktop ran but intermediate {fusion_ext} file not found.",
            "_hint": "Ensure the correct workspace is active: PCB Editor / Board Layout for .brd, "
                     "Schematic Editor for .sch.",
        }

    # Extract the EAGLE XML from the ZIP container
    eagle_data = None
    inner_name = None
    try:
        with zipfile.ZipFile(actual_tmp, "r") as zf:
            for name in zf.namelist():
                if name.endswith(ext):  # Find the .sch or .brd inside
                    eagle_data = zf.read(name)
                    inner_name = name
                    break
    except Exception as e:
        # Clean up temp
        try:
            os.remove(actual_tmp)
            os.rmdir(tmp_dir)
        except Exception:
            pass
        return {
            "success": False,
            "error": f"Failed to read {fusion_ext} ZIP container: {e}",
        }

    # Clean up temp file
    try:
        os.remove(actual_tmp)
        os.rmdir(tmp_dir)
    except Exception:
        pass

    if not eagle_data:
        return {
            "success": False,
            "error": f"No {ext} file found inside {fusion_ext} ZIP container.",
            "_hint": "The Fusion container format may have changed. Use fusion_export_source "
                     "to get the raw .fsch/.fbrd and inspect with a ZIP tool.",
        }

    # Verify it looks like EAGLE XML
    header = eagle_data[:100].decode("utf-8", errors="replace")
    if "<eagle" not in header and "<?xml" not in header:
        return {
            "success": False,
            "error": f"Extracted file does not appear to be EAGLE XML. "
                     f"First 100 bytes: {header!r}",
        }

    # Write to output path
    out_dir = os.path.dirname(output_path)
    if out_dir:
        os.makedirs(out_dir, exist_ok=True)

    with open(output_path, "wb") as f:
        f.write(eagle_data)

    size_bytes = len(eagle_data)

    # Get context
    doc_name = ""
    workspace = ""
    try:
        doc = app.activeDocument
        if doc:
            doc_name = doc.name
        workspace = app.executeTextCommand("DebugCommands.ActiveWorkspace").strip()
    except Exception:
        pass

    return {
        "success": True,
        "output": f"Exported EAGLE {ext} ({size_bytes:,} bytes) to {output_path}",
        "data": {
            "savedTo": output_path.replace("\\", "/"),
            "sizeBytes": size_bytes,
            "format": ext.lstrip("."),
            "innerZipEntry": inner_name,
            "activeDocument": doc_name,
            "activeWorkspace": workspace,
        },
        "_hint": "This is a plain EAGLE XML file — parseable by standalone EAGLE, "
                 "KiCad import, or any XML tool. For Fusion-format source, use "
                 "fusion_export_source instead.",
    }


def handle_execute_text_command(app: adsk.core.Application, args: dict) -> dict:
    """Execute any Fusion 360 text command directly.

    This is a lower-level command that runs `app.executeTextCommand()` without
    any wrapper. Useful for workspace info, debugging, or non-Electron commands.

    Unlike Electron.run, many text commands DO return meaningful results
    (e.g., DebugCommands.ActiveWorkspace returns the workspace name).

    Args:
        args: Dict with:
            - command: The full text command to execute (required).
    """
    text_cmd = args.get("command", "")
    if not text_cmd:
        return {"success": False, "error": "No command specified."}

    try:
        result = app.executeTextCommand(text_cmd)

        data = {"command": text_cmd, "result": result.strip() if result else ""}

        # Add workspace context for common debug queries
        if not text_cmd.startswith("DebugCommands.ActiveWorkspace"):
            try:
                ws = app.executeTextCommand("DebugCommands.ActiveWorkspace")
                data["activeWorkspace"] = ws.strip() if ws else "unknown"
            except Exception:
                pass

        try:
            doc = app.activeDocument
            if doc:
                data["activeDocument"] = doc.name
        except Exception:
            pass

        return {
            "success": True,
            "output": result.strip() if result else f"Executed: {text_cmd} (no output)",
            "data": data,
        }
    except Exception as e:
        return {
            "success": False,
            "error": f"Text command failed: {e}",
            "_hint": "Call fusion_list_text_commands to discover valid commands, or verify the active workspace supports this command.",
        }


def handle_list_text_commands(app: adsk.core.Application, args: dict) -> dict:
    """List available Fusion 360 text commands matching a filter.

    Args:
        args: Dict with:
            - filter: Optional filter string (default: "Electron").
    """
    filter_str = args.get("filter", "Electron")

    try:
        result = app.executeTextCommand("TextCommands.List")
        if result:
            lines = result.split("\n")
            matching = [l for l in lines if filter_str.lower() in l.lower()]
            return {
                "success": True,
                "output": f"Found {len(matching)} commands matching '{filter_str}'",
                "data": {
                    "commands": matching[:50],
                    "total": len(matching),
                },
            }
        return {
            "success": True,
            "output": "TextCommands.List returned no output",
            "data": {"commands": []},
        }
    except Exception as e:
        return {
            "success": False,
            "error": f"TextCommands.List failed: {e}",
            "_hint": "Ensure an Electronics workspace is active — call fusion_open_schematic, fusion_open_board, or fusion_open_lbr first, then retry.",
        }


def handle_board_info(app: adsk.core.Application, args: dict) -> dict:
    """Get structured board data from the current PCB layout.

    Returns XML with copper traces, pads, component placements (name, x, y,
    rotation), net names, layer setup, board thickness, DRC violations, and
    library packages used. Only works when a .brd board is open in PCB Editor.

    This is much richer than a screenshot — the AI can read exact coordinates,
    net connectivity, and component positions.
    """
    # Electron.boardInfo requires an output file path
    board_info_path = os.path.join(tempfile.gettempdir(), "adom_board_info.xml")

    try:
        app.executeTextCommand(f"Electron.boardInfo {board_info_path}")
    except Exception as e:
        err_msg = str(e)
        if "not PCB2d document" in err_msg.lower() or "not pcb" in err_msg.lower():
            ws = "unknown"
            try:
                ws = app.executeTextCommand("DebugCommands.ActiveWorkspace").strip()
            except Exception:
                pass
            return {
                "success": False,
                "error": f"No board open (current workspace: {ws}). Open a .brd file first with fusion_open_board.",
                "_hint": "Call fusion_open_board {\"filePath\":\"...\"} or fusion_show_2d_board to enter Board Layout, then retry.",
                "data": {"activeWorkspace": ws},
            }
        return {
            "success": False,
            "error": f"boardInfo failed: {e}",
            "_hint": "Ensure a .brd file is open in Board Layout — call fusion_show_2d_board first, then retry.",
        }

    # Read the output file (Fusion writes UTF-16 LE BOM)
    result = None
    for enc in ("utf-16", "utf-8-sig", "utf-8"):
        try:
            with open(board_info_path, "r", encoding=enc) as f:
                result = f.read()
            break
        except (UnicodeDecodeError, UnicodeError):
            continue
    if result is None:
        try:
            with open(board_info_path, "rb") as f:
                raw = f.read()
            return {"success": False, "error": f"Failed to decode boardInfo (first bytes: {raw[:20].hex()})"}
        except Exception as e:
            return {"success": False, "error": f"Failed to read boardInfo output: {e}"}

    if not result:
        return {"success": False, "error": "boardInfo returned no data. Is a .brd board open?"}

    # Parse the XML to extract a summary for the AI
    import xml.etree.ElementTree as ET
    summary = {}
    try:
        root = ET.fromstring(result)
        board = root.find("g")
        if board is not None:
            summary["name"] = board.get("name", "")
            summary["unit"] = board.get("unit", "mm")
            summary["boardThickness"] = board.get("boardThickness", "")
            summary["layerSetup"] = board.get("layersetup", "")

        # Extract component placements from <libraries>
        components = []
        for lib_group in root.iter("g"):
            lib = lib_group.get("library")
            pkg = lib_group.get("package")
            if lib and pkg:
                for comp in lib_group.findall("g"):
                    name = comp.get("name")
                    if name:
                        components.append({
                            "name": name,
                            "library": lib,
                            "package": pkg,
                            "x": comp.get("x"),
                            "y": comp.get("y"),
                            "rotation": comp.get("rot", "R0"),
                            "locked": comp.get("LOCKED", "false") == "true",
                        })
        summary["components"] = components
        summary["componentCount"] = len(components)

        # Extract net names from copper traces
        nets = set()
        for copper in root.iter("copper"):
            net_name = copper.get("name", "")
            if net_name:
                nets.add(net_name)
        summary["nets"] = sorted(nets)
        summary["netCount"] = len(nets)

        # Extract DRC issues
        drc = root.find(".//drc")
        if drc is not None:
            overlaps = []
            for overlap in drc.findall(".//overlap"):
                for signal in overlap.findall("signal"):
                    overlaps.append(signal.get("name", ""))
            if overlaps:
                summary["drcOverlaps"] = overlaps

    except ET.ParseError:
        # If XML parsing fails, just return the raw result
        pass

    return {
        "success": True,
        "output": f"Board info: {summary.get('name', 'unknown')}, "
                  f"{summary.get('componentCount', 0)} components, "
                  f"{summary.get('netCount', 0)} nets",
        "data": {
            "summary": summary,
            "rawXml": result,  # Full XML for detailed analysis if needed
        },
    }
