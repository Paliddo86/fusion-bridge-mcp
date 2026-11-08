"""Side-effect-free reads of what Fusion has in front, for the reads the bridge polls (get_app_state,
document_info). Text commands are not used here: Electron.run DISPLAY starts an EAGLE command, and
starting any command in an electronics editor ends the one the user has in progress (a MOVE snaps
back; issue #131). These read the API only."""

import adsk.core


def active_workspace(app: adsk.core.Application) -> str:
    """The workspace in front by its name (PCB Editor, Schematic Editor, Electronics Library ...: what
    DebugCommands.ActiveWorkspace answered and what the bridge's checks compare), or 'unknown'.
    1.0.29 answered the id (BoardLayoutEnvironement), which the EAGLE import's wait did not
    recognise (issue #133)."""
    try:
        ws = app.userInterface.activeWorkspace
        if ws:
            return ws.name or ws.id or "unknown"
    except Exception:
        pass
    return "unknown"


def active_workspace_id(app: adsk.core.Application) -> str:
    try:
        ws = app.userInterface.activeWorkspace
        return (ws.id if ws else "") or ""
    except Exception:
        return ""


def is_electronics(app: adsk.core.Application) -> bool:
    """True when an electronics editor (schematic, board or library) is in front: its product is an
    adsk::electron object."""
    try:
        p = app.activeProduct
        return bool(p) and str(p.objectType).startswith("adsk::electron::")
    except Exception:
        return False


def user_command_active(app: adsk.core.Application) -> bool:
    """True while the user is in the middle of a command (anything but the plain select)."""
    try:
        cmd = app.userInterface.activeCommand or ""
        return cmd not in ("", "SelectCommand")
    except Exception:
        return False
