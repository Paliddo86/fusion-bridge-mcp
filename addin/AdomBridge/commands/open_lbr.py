"""Open an EAGLE .lbr library file in Fusion 360's Electronics workspace.

Uses the Document.newDesignFromLocal text command to open the file, which
switches Fusion to the Electronics Library editor automatically.
This avoids the fragile workspace-switching dance.

Note: Document.Open fails with error 3 for .lbr files, but
Document.newDesignFromLocal works reliably.

Flow:
  1. Write .lbr XML file to disk (caller provides path or XML content)
  2. Document.newDesignFromLocal <path> opens it in Electronics Library editor
  3. Optionally navigate to a specific symbol with Electron.run EDIT <name>.sym
  4. Optionally export via EXPORT SCRIPT for round-trip verification
"""

import adsk.core
import os
import tempfile
import time


def handle_open_lbr(app: adsk.core.Application, args: dict) -> dict:
    """Open an EAGLE .lbr file in the Electronics Library editor.

    Args:
        args: Dict with:
            - filePath: Path to the .lbr file on disk (required).
            - symbolName: Optional symbol name to navigate to after opening.
            - verify: If True, exports the library back via EXPORT SCRIPT.
    """
    file_path = args.get("filePath", "")
    symbol_name = args.get("symbolName", "")
    verify = args.get("verify", False)

    if not file_path:
        return {"success": False, "error": "No filePath specified"}

    # Normalize path for Fusion (forward slashes)
    file_path = file_path.replace("\\", "/")

    if not os.path.exists(file_path):
        return {
            "success": False,
            "error": f"File not found: {file_path}",
            "_hint": "The path may exist on Docker but not on the Windows host. Use push_file to copy from Docker, or verify the host-side path.",
        }

    if not file_path.lower().endswith(".lbr"):
        return {
            "success": False,
            "error": f"Not a .lbr file: {file_path}",
            "_hint": "Use fusion_open_schematic for .sch files, fusion_open_board for .brd files, or fusion_import_electronics for .fsch/.fbrd/.flbr files.",
        }

    results = []

    # Step 1: Open the .lbr file via Document.newDesignFromLocal
    # (Document.Open fails with error 3 for .lbr files)
    try:
        # issue #196: quote + 8.3 fallback so paths WITH SPACES open (executeTextCommand
        # is space-delimited, so an unquoted path was silently truncated at the space).
        from .open_electronics_file import _open_local_document
        r, _open_how = _open_local_document(app, file_path)
        results.append(f"Document.newDesignFromLocal: {r if r else 'ok'}")
    except Exception as e:
        return {
            "success": False,
            "error": f"Document.newDesignFromLocal failed: {e}",
            "_hint": "Call fusion_screenshot to check for a modal dialog blocking Fusion, answer with fusion_dismiss_blocking_dialogs, then retry.",
            "data": {"filePath": file_path},
        }

    # Step 2: If a symbol name was given, navigate to it
    if symbol_name:
        # Give Fusion a moment to finish opening the library
        time.sleep(1)

        sym_ref = symbol_name if symbol_name.endswith(".sym") else f"{symbol_name}.sym"
        try:
            # inspect-only navigation: pre-answer NO so a stale/renamed sym_ref can never
            # raise a blocking "Create new ...?" modal on the add-in main thread.
            app.executeTextCommand("Electron.run SET CONFIRM NO")
            app.executeTextCommand(f"Electron.run EDIT {sym_ref}")
            results.append(f"EDIT {sym_ref}: ok")
        except Exception as e:
            results.append(f"EDIT {sym_ref} failed: {e}")

        # Zoom to fit
        try:
            app.executeTextCommand("Electron.run WINDOW FIT")
            results.append("WINDOW FIT: ok")
        except Exception:
            pass

    # Step 3: Verification via EXPORT SCRIPT
    verification = None
    if verify:
        verification = _export_and_verify(app, symbol_name)
        results.append(f"Verification: exported={verification.get('exported', False)}")

    response = {
        "success": True,
        "output": f"Opened library: {os.path.basename(file_path)}",
        "data": {"results": results, "filePath": file_path},
    }
    if symbol_name:
        response["data"]["symbolName"] = symbol_name
    if verification:
        response["data"]["verification"] = verification
    return response


def handle_export_lbr(app: adsk.core.Application, args: dict) -> dict:
    """Export the current Electronics library via EXPORT SCRIPT.

    This exports the library contents to an EAGLE .scr script file,
    which is a text representation of the entire library. Useful for
    verifying that a library was loaded correctly (round-trip test).

    Args:
        args: Dict with:
            - outputPath: Path to write the .scr export (optional,
                         defaults to a temp file).
    """
    output_path = args.get("outputPath", "")
    if not output_path:
        import uuid
        output_path = os.path.join(tempfile.gettempdir(), f"_adom_export_{uuid.uuid4().hex[:8]}.scr")

    output_path = output_path.replace("\\", "/")

    try:
        app.executeTextCommand(f"Electron.run EXPORT SCRIPT '{output_path}'")
    except Exception as e:
        return {
            "success": False,
            "error": f"EXPORT SCRIPT failed: {e}. "
                     "Ensure you are in an Electronics editor (library, schematic, or board).",
            "_hint": "Call fusion_open_lbr {\"filePath\":\"...\"} to enter the Electronics Library editor, then retry.",
        }

    if not os.path.exists(output_path):
        return {
            "success": False,
            "error": f"Export file was not created at {output_path}",
        }

    size = os.path.getsize(output_path)
    # Read a preview of the exported content
    with open(output_path, "r", errors="replace") as f:
        preview = f.read(2000)

    return {
        "success": True,
        "output": f"Exported library to {output_path} ({size} bytes)",
        "data": {
            "outputPath": output_path,
            "fileSize": size,
            "preview": preview,
        },
    }


def _export_and_verify(app: adsk.core.Application, symbol_name: str = "") -> dict:
    """Export library and check if the symbol exists in the output."""
    import uuid
    verify_path = os.path.join(tempfile.gettempdir(), f"_adom_verify_{uuid.uuid4().hex[:8]}.scr")
    try:
        app.executeTextCommand(f"Electron.run EXPORT SCRIPT '{verify_path}'")
        if os.path.exists(verify_path):
            with open(verify_path, "r", errors="replace") as f:
                content = f.read()
            result = {
                "exported": True,
                "fileSize": os.path.getsize(verify_path),
                "preview": content[:1000],
            }
            if symbol_name:
                # EAGLE uppercases symbol names in EXPORT SCRIPT output
                has_symbol = (
                    f"Edit '{symbol_name.upper()}.sym'" in content
                    or f"Edit '{symbol_name}.sym'" in content
                )
                result["hasSymbol"] = has_symbol
            return result
        return {"exported": False, "error": "Export file not created"}
    except Exception as e:
        return {"exported": False, "error": str(e)}
