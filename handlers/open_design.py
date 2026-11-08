"""Handler for open_design command.

Opens a design file (.f3d, .step, .iges, etc.) in Fusion 360.
Launches Fusion if it's not already running.
Imports files via the AdomBridge add-in API when available.
"""

import json
import urllib.request
import urllib.error
from pathlib import Path

from fusion_detect import ensure_fusion_running, _is_fusion_running

ADDIN_PORT = 8774

SUPPORTED_EXTENSIONS = {
    ".f3d", ".f3z",
    ".step", ".stp",
    ".iges", ".igs",
    ".sat",
    ".stl",
    ".3mf",
    ".obj",
}


def _import_via_addin(file_path: str, timeout: int = 30) -> dict:
    """Import a file via the AdomBridge add-in running inside Fusion."""
    body = json.dumps({"command": "import_file", "args": {"filePath": file_path}}).encode("utf-8")
    req = urllib.request.Request(
        f"http://127.0.0.1:{ADDIN_PORT}/command",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.loads(resp.read())
    except urllib.error.URLError as e:
        return {
            "success": False,
            "error": f"AdomBridge add-in not responding (port {ADDIN_PORT}): {e}. "
                     "The add-in is not responding - fix it YOURSELF (never ask the user): fusion_stop + fusion_start reloads it (installed to all add-in dirs incl. FusionAddins).",
        }
    except Exception as e:
        return {"success": False, "error": f"Import request failed: {e}"}


def handle_open_design(fusion_info: dict, args: dict) -> dict:
    """Open a design file in Fusion 360, or just launch Fusion.

    Args:
        fusion_info: Fusion 360 detection info dict.
        args: Dict with:
            - filePath: Path to the file to open (optional).
              If omitted or empty, just ensures Fusion is running.
    """
    file_path = args.get("filePath", "")

    # Check if Fusion is installed
    if not fusion_info.get("installed"):
        return {
            "success": False,
            "error": "Fusion 360 is not installed on this machine.",
            "errorCode": "fusion_not_installed",
            "_hint": ("Fusion 360 isn't installed. Do NOT tell the user to install it themselves - "
                      "this claim was simply STALE (the bridge silent-installs Fusion via the "
                      "fusion-onboarding skill and drives the Autodesk sign-in). OFFER to install it "
                      "for them and do it on a yes, then retry this open."),
        }

    # Check if Fusion is running (live process check — never auto-launch)
    if not _is_fusion_running():
        return {
            "success": False,
            "error": "Fusion 360 is installed but not running.",
            "errorCode": "fusion_not_running",
            "_hint": "Call fusion_start to start Fusion 360 and wait for the add-in to be ready, then retry this command.",
        }

    # Launch-only mode: no file specified, Fusion is already running
    if not file_path:
        return {"success": True, "output": "Fusion 360 is running."}

    path = Path(file_path)
    if not path.exists():
        return {
            "success": False,
            "error": f"File not found: {file_path}",
            "errorCode": "file_not_found",
            "_hint": "Verify the path with `ls` or check if the file is in the cloud instead of local — use fusion_walk_cloud_tree to find cloud files, then fusion_open_cloud_file to open them.",
        }

    ext = path.suffix.lower()
    if ext not in SUPPORTED_EXTENSIONS:
        return {
            "success": False,
            "error": f"Unsupported file type: {ext}. Supported: {', '.join(sorted(SUPPORTED_EXTENSIONS))}",
            "errorCode": "unsupported_extension",
            "_hint": "Use a supported extension, or for electronics files (.fsch/.fbrd/.flbr) use fusion_import_electronics instead.",
        }

    # Fusion is running — ensure add-in is ready (needed for import)
    err = ensure_fusion_running(fusion_info, wait_addin=True)
    if err:
        return err

    # Import the file via the AdomBridge add-in (runs inside Fusion)
    return _import_via_addin(str(path))
