"""Import file command — open STEP, IGES, SAT, STL, etc. into the active design."""

import adsk.core
import adsk.fusion
from pathlib import Path

SUPPORTED_EXTENSIONS = {
    ".step", ".stp",
    ".iges", ".igs",
    ".sat",
    ".sab",
    ".smb",
    ".smt",
    ".f3d", ".f3z",
    ".stl",
    ".3mf",
    ".obj",
}


def handle_import_file(app: adsk.core.Application, args: dict) -> dict:
    """Import a file into Fusion 360 using the native import API.

    Args:
        args: Dict with:
            - filePath: Full path to the file to import (required).
    """
    file_path = args.get("filePath", "")
    if not file_path:
        return {"success": False, "error": "No filePath specified"}

    path = Path(file_path)
    if not path.exists():
        return {
            "success": False,
            "error": f"File not found: {file_path}",
            "_hint": "The path may exist on Docker but not on the Windows host. Use push_file to copy from Docker first, or verify the host-side path.",
        }

    ext = path.suffix.lower()
    if ext not in SUPPORTED_EXTENSIONS:
        return {
            "success": False,
            "error": f"Unsupported file type: {ext}. Supported: {', '.join(sorted(SUPPORTED_EXTENSIONS))}",
            "_hint": "Use fusion_import_electronics for .fsch/.fbrd/.flbr electronics files, or verify the extension is one of the 3D formats Fusion supports.",
        }

    import_mgr = app.importManager

    try:
        # Select the right import options based on file type
        if ext in (".step", ".stp"):
            options = import_mgr.createSTEPImportOptions(str(path))
        elif ext in (".iges", ".igs"):
            options = import_mgr.createIGESImportOptions(str(path))
        elif ext in (".sat", ".sab"):
            options = import_mgr.createSATImportOptions(str(path))
        elif ext in (".smb", ".smt"):
            options = import_mgr.createSMTImportOptions(str(path))
        elif ext in (".f3d", ".f3z"):
            options = import_mgr.createFusionArchiveImportOptions(str(path))
        elif ext == ".stl":
            options = import_mgr.createSTLImportOptions(str(path))
        elif ext == ".3mf":
            options = import_mgr.createC3MFImportOptions(str(path))
        elif ext == ".obj":
            options = import_mgr.createOBJImportOptions(str(path))
        else:
            return {
                "success": False,
                "error": f"No import handler for {ext}",
                "_hint": "Use fusion_import_electronics for .fsch/.fbrd/.flbr electronics files, or verify the extension is one of the 3D formats Fusion supports.",
            }

        # Always import to a new document — importToTarget fails on Part Designs
        # because "Part Design documents can only contain one component".
        # importToNewDocument opens the file in its own document tab, which is
        # the expected behavior (like File > Open).
        doc = import_mgr.importToNewDocument(options)
        if not doc:
            return {
                "success": False,
                "error": f"Import returned null for {path.name}",
                "_hint": "Call fusion_screenshot_fusion to check for a blocking dialog (e.g. unit prompt), then fusion_dismiss_blocking_dialogs, then retry.",
            }

        # Activate the newly imported document so subsequent commands work on it
        doc.activate()

        # Verify it's now the active document
        active_doc = app.activeDocument
        doc_name = active_doc.name if active_doc else "Unknown"

        return {
            "success": True,
            "output": f"Imported {path.name} into Fusion 360.",
            "data": {
                "document": doc_name,
            },
        }

    except Exception as e:
        return {
            "success": False,
            "error": f"Import failed: {e}",
            "_hint": "Call fusion_screenshot to check for a modal dialog, answer with fusion_dismiss_blocking_dialogs, then retry.",
        }
