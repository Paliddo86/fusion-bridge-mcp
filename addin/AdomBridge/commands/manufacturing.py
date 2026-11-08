"""Manufacturing export commands — Gerbers, BOM, CPL for PCB fabrication and assembly.

These commands extract manufacturing data from the open PCB board in Fusion 360's
Electronics workspace and write industry-standard output files:
  - Gerbers (RS-274X) + Excellon drill files via EAGLE CAM processor
  - BOM (Bill of Materials) CSV for component purchasing
  - CPL (Component Placement List) CSV for pick-and-place assembly
  - Design rules (DRU) injection for the 3rd party fab's manufacturing capabilities

All three export commands require a .brd board to be open in PCB Editor.

IMPORTANT — EAGLE "Overwrite?" dialog:
  EAGLE's EXPORT commands open a blocking confirmation dialog if the output
  file already exists. This freezes Fusion's UI thread and crashes the add-in.
  Always use _safe_eagle_export() which deletes the file first and runs
  SET CONFIRM YES before any EAGLE export command.
"""

import adsk.core
import os
import csv
import html
import io
import json
import tempfile
import time
import xml.etree.ElementTree as ET
import zipfile


def _safe_eagle_export(app, eagle_cmd: str, output_file: str) -> dict:
    """Run an EAGLE export command safely, avoiding the 'Overwrite?' dialog.

    EAGLE's EXPORT commands open a blocking confirmation dialog if the output
    file already exists. This helper:
      1. Deletes the target file if it exists
      2. Runs SET CONFIRM YES to suppress any remaining dialogs
      3. Executes the export command

    Returns a dict with 'deletedExisting' flag so the caller can include it
    in response data for AI debugging context.
    """
    result = {"deletedExisting": False}
    if os.path.exists(output_file):
        os.remove(output_file)
        result["deletedExisting"] = True
    app.executeTextCommand("Electron.run SET CONFIRM YES")
    app.executeTextCommand(f"Electron.run {eagle_cmd}")
    return result


def _get_board_xml(app: adsk.core.Application) -> str:
    """Get board info XML from the current PCB layout via Electron.boardInfo."""
    board_info_path = os.path.join(tempfile.gettempdir(), "adom_mfg_board_info.xml")

    try:
        app.executeTextCommand(f"Electron.boardInfo {board_info_path}")
    except Exception as e:
        err_msg = str(e)
        if "not pcb" in err_msg.lower():
            ws = "unknown"
            try:
                ws = app.executeTextCommand("DebugCommands.ActiveWorkspace").strip()
            except Exception:
                pass
            raise RuntimeError(
                f"No board open (current workspace: {ws}). "
                "Open a .brd file first with fusion_open_board."
            )
        raise RuntimeError(f"boardInfo failed: {e}")

    # Read the output file (Fusion writes UTF-16 LE BOM)
    for enc in ("utf-16", "utf-8-sig", "utf-8"):
        try:
            with open(board_info_path, "r", encoding=enc) as f:
                return f.read()
        except (UnicodeDecodeError, UnicodeError):
            continue

    raise RuntimeError("Failed to decode boardInfo XML output.")


def _parse_components(xml_str: str) -> list:
    """Parse component data from boardInfo XML.

    Returns list of dicts with: name, value, package, library, x, y, rotation, side.
    EAGLE rotation format: R0/R90/R180/R270 = top, MR0/MR90/MR180/MR270 = bottom.
    """
    root = ET.fromstring(xml_str)
    components = []

    for lib_group in root.iter("g"):
        lib = lib_group.get("library")
        pkg = lib_group.get("package")
        if not (lib and pkg):
            continue

        for comp in lib_group.findall("g"):
            name = comp.get("name")
            if not name:
                continue

            rot_str = comp.get("rot", "R0")
            # M prefix = mirrored = bottom side
            side = "Bottom" if rot_str.startswith("M") else "Top"
            # Extract numeric rotation
            rot_num = rot_str.replace("M", "").replace("R", "").replace("S", "")
            try:
                rotation = float(rot_num)
            except ValueError:
                rotation = 0.0

            components.append({
                "name": name,
                "value": comp.get("value", ""),
                "package": pkg,
                "library": lib,
                "x": comp.get("x", "0"),
                "y": comp.get("y", "0"),
                "rotation": rotation,
                "side": side,
            })

    return components


def _parse_attributes(xml_str: str) -> dict:
    """Parse component attributes from boardInfo XML.

    Returns dict of {component_name: {attr_name: attr_value}}.
    Looks for MPN, MANUFACTURER, MOUSER_PN, DIGIKEY_PN, LCSC_PN etc.
    """
    root = ET.fromstring(xml_str)
    attrs = {}

    # Attributes are stored as <attribute> elements within component groups
    for elem in root.iter("attribute"):
        name = elem.get("name", "")
        value = elem.get("value", "")
        # The parent chain should lead us to the component
        # For now, collect all attributes
        if name and value:
            # Try to find parent component name
            parent = elem
            comp_name = None
            for _ in range(5):  # Walk up max 5 levels
                parent = None  # ET doesn't have parent refs, use different approach
                break
            if name not in attrs:
                attrs[name] = value

    return attrs


# ─── BOM Export ───────────────────────────────────────────────────────────

def handle_export_bom(app: adsk.core.Application, args: dict) -> dict:
    """Export Bill of Materials (BOM) from the open PCB board.

    Generates a CSV with component values, quantities, and designators —
    suitable for ordering from Mouser, Digi-Key, or LCSC.

    Two approaches combined:
      1. EAGLE's EXPORT PARTLIST for raw parts data
      2. boardInfo XML parsing for coordinates and grouping

    Args:
        outputPath: File path for the output CSV (default: C:/tmp/adom-bom.csv)
        grouped: If true (default), group identical parts by value+package.
                 If false, list every individual component.
    """
    output_path = args.get("outputPath", "C:/tmp/adom-bom.csv")
    grouped = args.get("grouped", True)

    # Ensure output directory exists
    os.makedirs(os.path.dirname(output_path), exist_ok=True)

    # Also try EXPORT PARTLIST for additional attribute data
    partlist_path = os.path.join(tempfile.gettempdir(), "adom_partlist.txt")
    partlist_data = {}
    try:
        _safe_eagle_export(app, f"EXPORT PARTLIST '{partlist_path}'", partlist_path)
        time.sleep(0.5)
        if os.path.exists(partlist_path):
            for enc in ("utf-16", "utf-8-sig", "utf-8", "latin-1"):
                try:
                    with open(partlist_path, "r", encoding=enc) as f:
                        lines = f.readlines()
                    # Parse tab-separated partlist
                    if len(lines) > 1:
                        headers = lines[0].strip().split("\t")
                        for line in lines[1:]:
                            fields = line.strip().split("\t")
                            if len(fields) >= 2:
                                part_name = fields[0].strip()
                                part_info = {}
                                for i, h in enumerate(headers):
                                    if i < len(fields):
                                        part_info[h.strip()] = fields[i].strip()
                                partlist_data[part_name] = part_info
                    break
                except (UnicodeDecodeError, UnicodeError):
                    continue
    except Exception:
        pass  # EXPORT PARTLIST might fail if not in board view; we still have boardInfo

    # Get components from boardInfo XML
    try:
        xml_str = _get_board_xml(app)
    except RuntimeError as e:
        return {
            "success": False,
            "error": str(e),
            "_hint": "Call fusion_show_2d_board or fusion_open_board {\"filePath\":\"...\"} to enter Board Layout, then retry.",
        }

    components = _parse_components(xml_str)
    if not components:
        return {
            "success": False,
            "error": "No components found in board. Is a populated .brd file open?",
            "data": {
                "hint": "The board XML was parsed but contained no component data. "
                        "This usually means the board has no placed components yet.",
                "recoverySteps": [
                    "Verify the board has components placed (not just an empty board outline)",
                    "Switch to schematic and back: fusion_electron_run with 'BOARD' command",
                    "Try fusion_board_info to get raw board data for debugging",
                ],
            },
        }

    # Merge partlist attributes into component data
    for comp in components:
        pl = partlist_data.get(comp["name"], {})
        if pl.get("Value"):
            comp["value"] = pl["Value"]

    if grouped:
        # Group by value + package (same part = same BOM line)
        groups = {}
        for comp in components:
            key = (comp["value"], comp["package"], comp["library"])
            if key not in groups:
                groups[key] = {
                    "value": comp["value"],
                    "package": comp["package"],
                    "library": comp["library"],
                    "designators": [],
                    "quantity": 0,
                }
            groups[key]["designators"].append(comp["name"])
            groups[key]["quantity"] += 1

        # Sort by designator (natural sort: C1, C2, ..., R1, R2, ...)
        rows = sorted(groups.values(), key=lambda g: g["designators"][0])

        # Write grouped BOM CSV
        with open(output_path, "w", newline="", encoding="utf-8") as f:
            writer = csv.writer(f)
            writer.writerow(["Comment", "Designator", "Footprint", "Quantity", "Library"])
            for row in rows:
                writer.writerow([
                    row["value"],
                    " ".join(sorted(row["designators"])),
                    row["package"],
                    row["quantity"],
                    row["library"],
                ])
    else:
        # Ungrouped — one row per component
        with open(output_path, "w", newline="", encoding="utf-8") as f:
            writer = csv.writer(f)
            writer.writerow(["Designator", "Comment", "Footprint", "Library", "Side"])
            for comp in sorted(components, key=lambda c: c["name"]):
                writer.writerow([
                    comp["name"],
                    comp["value"],
                    comp["package"],
                    comp["library"],
                    comp["side"],
                ])

    unique_count = len(rows) if grouped else len(components)
    return {
        "success": True,
        "output": f"BOM exported: {len(components)} components"
                  + (f" in {unique_count} unique parts" if grouped else "")
                  + f" → {output_path}",
        "message": (
            f"Bill of Materials CSV with {unique_count} unique part"
            f"{'s' if unique_count != 1 else ''} and {len(components)} total placements. "
            "Format: Comment, Designator, Footprint, Quantity, Library — "
            "compatible with JLCPCB, PCBWay, Mouser, and Digi-Key."
        ),
        "data": {
            "outputPath": output_path,
            "componentCount": len(components),
            "uniquePartCount": unique_count,
            "grouped": grouped,
            "nextSteps": [
                "Run fusion_export_cpl to generate the pick-and-place CSV",
                "Run fusion_export_gerbers to generate the Gerber fabrication ZIP",
                "Once you have BOM + CPL + Gerbers, upload all three to your PCB fab service",
            ],
            "hint": (
                "If BOM looks incomplete, check that component values are set in the schematic. "
                "Use fusion_board_info for raw structured component data."
            ),
        },
    }


# ─── CPL / Pick-and-Place Export ──────────────────────────────────────────

def handle_export_cpl(app: adsk.core.Application, args: dict) -> dict:
    """Export Component Placement List (CPL) for pick-and-place assembly.

    Generates a CSV with component centroid positions, rotation, and side —
    the standard format expected by PCBA manufacturers (JLCPCB, PCBWay, etc.).

    Coordinates are in millimeters relative to the board origin.

    Args:
        outputPath: File path for the output CSV (default: C:/tmp/adom-cpl.csv)
        side: Filter by side — "all" (default), "top", or "bottom"
    """
    output_path = args.get("outputPath", "C:/tmp/adom-cpl.csv")
    side_filter = args.get("side", "all").lower()

    os.makedirs(os.path.dirname(output_path), exist_ok=True)

    try:
        xml_str = _get_board_xml(app)
    except RuntimeError as e:
        return {
            "success": False,
            "error": str(e),
            "_hint": "Call fusion_show_2d_board or fusion_open_board {\"filePath\":\"...\"} to enter Board Layout, then retry.",
        }

    components = _parse_components(xml_str)
    if not components:
        return {
            "success": False,
            "error": "No components found in board. Is a populated .brd file open?",
            "data": {
                "hint": "Board has no component placements. CPL requires components to be placed on the board.",
                "recoverySteps": [
                    "Verify the board has components placed (check with fusion_board_info)",
                    "If schematic has components but board doesn't, the board may need updating",
                ],
            },
        }

    # Filter by side if requested
    if side_filter in ("top", "bottom"):
        target = side_filter.capitalize()
        components = [c for c in components if c["side"] == target]

    if not components:
        return {
            "success": False,
            "error": f"No {side_filter}-side components found.",
        }

    # Write CPL CSV (standard JLCPCB/generic format)
    with open(output_path, "w", newline="", encoding="utf-8") as f:
        writer = csv.writer(f)
        writer.writerow(["Designator", "Mid X", "Mid Y", "Layer", "Rotation"])
        for comp in sorted(components, key=lambda c: c["name"]):
            writer.writerow([
                comp["name"],
                comp["x"],
                comp["y"],
                comp["side"],
                comp["rotation"],
            ])

    top_count = sum(1 for c in components if c["side"] == "Top")
    bottom_count = sum(1 for c in components if c["side"] == "Bottom")

    return {
        "success": True,
        "output": f"CPL exported: {len(components)} placements "
                  f"({top_count} top, {bottom_count} bottom) → {output_path}",
        "message": (
            f"Pick-and-place CSV with {len(components)} component placements "
            f"({top_count} top-side, {bottom_count} bottom-side). "
            "Format: Designator, Mid X, Mid Y, Layer, Rotation — "
            "coordinates in mm, compatible with JLCPCB/PCBWay/Adom PCBA."
        ),
        "data": {
            "outputPath": output_path,
            "totalPlacements": len(components),
            "topCount": top_count,
            "bottomCount": bottom_count,
            "sideFilter": side_filter,
            "nextSteps": [
                "Run fusion_export_bom if you haven't already — you need BOM + CPL together",
                "Run fusion_export_gerbers to generate the Gerber fabrication ZIP",
                "Upload BOM + CPL + Gerbers together to your PCBA service for assembly quoting",
            ],
            "hint": (
                "If placement coordinates look wrong, verify the board origin in EAGLE. "
                "Use side='top' or side='bottom' to export only one side for single-sided assembly."
            ),
        },
    }


# ─── Resource Path Helper ────────────────────────────────────────────────

def _get_resources_dir() -> str:
    """Get the path to the add-in's bundled resources directory.

    Resources (CAM jobs, DRU files, ULP scripts) are bundled at:
      AdomBridge/resources/  (synced to %APPDATA%/.../AdomBridge/resources/)
    """
    # manufacturing.py is at: AdomBridge/commands/manufacturing.py
    # resources are at:       AdomBridge/resources/
    addin_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    return os.path.join(addin_root, "resources")


# ─── Layer Detection ─────────────────────────────────────────────────────

def _detect_layer_count(app: adsk.core.Application) -> dict:
    """Detect if the open board is 2-layer or 4-layer.

    Uses two methods in priority order:
      1. ULP script — runs detect_layers.ulp to inspect copper layers and traces
      2. CAM comparison — exports with both 2-layer and 4-layer CAM jobs,
         compares ZIP sizes to detect inner copper content

    Returns dict with: layerCount, is4Layer, method, copperLayers, details
    """
    resources = _get_resources_dir()
    result = {"layerCount": 2, "is4Layer": False, "method": "default",
              "copperLayers": [1, 16], "details": ""}

    # Method A: ULP script detection
    ulp_path = os.path.join(resources, "ulp", "detect_layers.ulp").replace("\\", "/")
    if os.path.exists(ulp_path):
        try:
            temp_fd, temp_json = tempfile.mkstemp(suffix=".json")
            os.close(temp_fd)
            temp_json = os.path.abspath(temp_json).replace("\\", "/")

            app.executeTextCommand(f"Electron.run RUN '{ulp_path}' '{temp_json}'")

            if os.path.exists(temp_json):
                with open(temp_json, "r") as f:
                    data = json.loads(f.read())
                os.remove(temp_json)

                copper_count = data.get("copperLayerCount", 0)
                layers_list = data.get("layers", [])
                copper_layers = [
                    l["number"] for l in layers_list
                    if l.get("isCopper") and (l["number"] in (1, 16) or l.get("hasTraces"))
                ]
                internal = [l for l in copper_layers if l not in (1, 16)]

                if internal or copper_count > 2:
                    result.update({
                        "layerCount": max(copper_count, 4),
                        "is4Layer": True,
                        "method": "ulp",
                        "copperLayers": copper_layers,
                        "details": f"ULP found {len(internal)} internal routing layer(s): {internal}",
                    })
                else:
                    result.update({
                        "method": "ulp",
                        "copperLayers": copper_layers,
                        "details": f"ULP found {copper_count} copper layers, no internal routing",
                    })
                return result
        except Exception as e:
            result["details"] = f"ULP detection failed: {e}; "

    # Method B: CAM comparison (export both, compare sizes)
    cam_2 = os.path.join(resources, "cam", "jlcpcb-2layers.cam").replace("\\", "/")
    cam_4 = os.path.join(resources, "cam", "jlcpcb-4layers.cam").replace("\\", "/")

    if os.path.exists(cam_2) and os.path.exists(cam_4):
        try:
            tmp_dir = tempfile.gettempdir().replace("\\", "/")
            zip_2 = f"{tmp_dir}/adom_test_2layer.zip"
            zip_4 = f"{tmp_dir}/adom_test_4layer.zip"

            # Export with 2-layer CAM
            app.executeTextCommand(
                f"Electron.run set confirm yes;manufacturing export '{zip_2}' '{cam_2}'"
            )
            size_2 = os.path.getsize(zip_2) if os.path.exists(zip_2) else 0

            # Export with 4-layer CAM
            app.executeTextCommand(
                f"Electron.run set confirm yes;manufacturing export '{zip_4}' '{cam_4}'"
            )
            size_4 = os.path.getsize(zip_4) if os.path.exists(zip_4) else 0

            # Clean up
            for f in (zip_2, zip_4):
                try:
                    os.remove(f)
                except Exception:
                    pass

            if size_2 > 0 and size_4 > 0:
                ratio = size_4 / size_2
                # Adaptive threshold based on file size
                if size_2 > 100_000:
                    threshold = 1.02
                elif size_2 > 50_000:
                    threshold = 1.03
                else:
                    threshold = 1.05

                if ratio > threshold:
                    result.update({
                        "layerCount": 4, "is4Layer": True, "method": "cam_comparison",
                        "details": f"4-layer ZIP is {ratio:.2f}x larger (threshold {threshold})",
                    })
                else:
                    result.update({
                        "method": "cam_comparison",
                        "details": f"4-layer ZIP ratio {ratio:.2f} (< {threshold}), treating as 2-layer",
                    })
                return result
        except Exception as e:
            result["details"] += f"CAM comparison failed: {e}"

    return result


def handle_detect_layers(app: adsk.core.Application, args: dict) -> dict:
    """Detect the layer count of the open PCB board.

    Uses ULP script and/or CAM comparison to determine if the board is
    2-layer or 4-layer. Useful before gerber export or design rules application.
    """
    # Verify we're in board layout
    try:
        ws = app.executeTextCommand("DebugCommands.ActiveWorkspace").strip()
    except Exception:
        ws = "unknown"

    if "Board" not in ws and "PCB" not in ws:
        return {
            "success": False,
            "error": f"Not in Board Layout (current: {ws}). Open a .brd file first.",
            "_hint": "Call fusion_show_2d_board or fusion_open_board {\"filePath\":\"...\"} to enter PCB Editor, then retry.",
            "data": {"activeWorkspace": ws},
        }

    detection = _detect_layer_count(app)

    layer_type = f"{detection['layerCount']}-layer"
    return {
        "success": True,
        "output": f"Board detected as {layer_type} ({detection['method']} method). "
                  f"Copper layers: {detection['copperLayers']}",
        "message": (
            f"This is a {layer_type} board. Detection used {detection['method']} method. "
            f"Copper layers found: {detection['copperLayers']}. "
            f"All subsequent manufacturing commands (gerbers, design rules) will auto-select "
            f"the {layer_type} configuration."
        ),
        "data": {
            **detection,
            "activeWorkspace": ws,
            "nextSteps": [
                f"Run fusion_set_design_rules to apply Adom's {layer_type} manufacturing rules",
                f"Run fusion_export_gerbers — auto-selects the {layer_type} CAM job",
                "Run fusion_electron_run with command 'DRC' after applying design rules to check compliance",
            ],
        },
    }


# ─── Gerber Export ────────────────────────────────────────────────────────

def handle_export_gerbers(app: adsk.core.Application, args: dict) -> dict:
    """Export Gerber manufacturing files from the open PCB board.

    Uses Fusion's CAM processor with bundled JLCPCB-compatible CAM job files
    to produce a ZIP containing industry-standard gerber files:
      - Top/Bottom Copper (.GTL/.GBL)
      - Top/Bottom Silkscreen (.GTO/.GBO)
      - Top/Bottom Solder Mask (.GTS/.GBS)
      - Top/Bottom Paste (.GTP/.GBP)
      - Board Outline (.GKO)
      - Drill file (.XLN)
      - (4-layer boards also get inner copper .G1/.G2)

    Auto-detects 2-layer vs 4-layer boards and selects the right CAM job.

    Args:
        outputDir: Directory for the output ZIP (default: C:/tmp/adom-gerbers/)
        boardName: Prefix for the ZIP filename (default: from active document)
        layers: Force layer count — "2" or "4" (default: auto-detect)
    """
    output_dir = args.get("outputDir", "C:/tmp/adom-gerbers")
    board_name = args.get("boardName", "")
    force_layers = args.get("layers", "auto")

    os.makedirs(output_dir, exist_ok=True)

    # Verify board layout
    try:
        ws = app.executeTextCommand("DebugCommands.ActiveWorkspace").strip()
    except Exception:
        ws = "unknown"

    if "Board" not in ws and "PCB" not in ws:
        return {
            "success": False,
            "error": f"Not in Board Layout (current: {ws}). "
                     "Open a .brd file in PCB Editor first.",
            "_hint": "Call fusion_show_2d_board to switch to PCB Editor, then retry.",
            "data": {
                "activeWorkspace": ws,
                "hint": "Use fusion_show_2d_board to switch to PCB Editor.",
            },
        }

    # Get board name
    if not board_name:
        try:
            doc = app.activeDocument
            if doc:
                board_name = doc.name.replace(".brd", "").replace(".fsch", "").replace(" ", "_")
            else:
                board_name = "board"
        except Exception:
            board_name = "board"

    # Detect layer count
    if force_layers in ("2", "4"):
        layer_count = int(force_layers)
        detection_method = "manual"
    else:
        detection = _detect_layer_count(app)
        layer_count = detection["layerCount"]
        detection_method = detection["method"]

    # Select CAM job file
    resources = _get_resources_dir()
    cam_file = os.path.join(
        resources, "cam", f"jlcpcb-{layer_count}layers.cam"
    ).replace("\\", "/")

    if not os.path.exists(cam_file):
        return {
            "success": False,
            "error": f"CAM job file not found: {cam_file}",
            "data": {
                "hint": "Ensure the resources/cam/ directory contains jlcpcb-2layers.cam "
                        "and jlcpcb-4layers.cam. Run install_addin.py to sync.",
            },
        }

    # Build output ZIP path
    zip_path = os.path.join(output_dir, f"{board_name}_gerbers.zip").replace("\\", "/")

    # Delete existing ZIP to avoid overwrite dialog
    if os.path.exists(zip_path):
        try:
            os.remove(zip_path)
        except Exception:
            pass

    # Execute the CAM export — this is the working command from AdomHelperJoe
    try:
        export_cmd = f"Electron.run set confirm yes;manufacturing export '{zip_path}' '{cam_file}'"
        app.executeTextCommand(export_cmd)
    except Exception as e:
        return {
            "success": False,
            "error": f"Manufacturing export failed: {e}",
            "_hint": "Call fusion_show_2d_board to ensure 2D PCB Editor is active (not 3D view), then retry.",
            "data": {
                "command": f"manufacturing export '{zip_path}' '{cam_file}'",
                "hint": "Ensure you are in the PCB Editor (not schematic or 3D view).",
            },
        }

    # Verify output
    if not os.path.exists(zip_path):
        return {
            "success": False,
            "error": "CAM export command ran but ZIP was not created.",
            "_hint": "Call fusion_show_2d_board to ensure 2D PCB Editor is active, then retry.",
            "data": {
                "zipPath": zip_path,
                "camFile": cam_file,
                "layerCount": layer_count,
                "hint": "Try switching to 2D board view with fusion_show_2d_board, "
                        "then retry.",
            },
        }

    zip_size = os.path.getsize(zip_path)

    # List ZIP contents for the AI
    gerber_files = []
    try:
        with zipfile.ZipFile(zip_path, "r") as zf:
            for info in zf.infolist():
                if not info.is_dir():
                    gerber_files.append({
                        "name": info.filename,
                        "size": info.file_size,
                    })
    except Exception:
        pass

    file_names = [f["name"] for f in gerber_files]
    return {
        "success": True,
        "output": (
            f"Gerbers exported: {len(gerber_files)} files in ZIP "
            f"({zip_size // 1024} KB) → {zip_path}"
        ),
        "message": (
            f"Gerber ZIP created for {layer_count}-layer board '{board_name}' with "
            f"{len(gerber_files)} files ({zip_size // 1024} KB). "
            f"Contains: {', '.join(file_names[:5])}{'...' if len(file_names) > 5 else ''}. "
            "This ZIP is ready for upload to any PCB fabrication service."
        ),
        "data": {
            "zipPath": zip_path,
            "zipSizeKB": zip_size // 1024,
            "layerCount": layer_count,
            "detectionMethod": detection_method,
            "boardName": board_name,
            "files": gerber_files,
            "fileCount": len(gerber_files),
            "nextSteps": [
                "Run fusion_export_bom to generate the Bill of Materials CSV",
                "Run fusion_export_cpl to generate the Component Placement List CSV",
                "Once you have Gerbers + BOM + CPL, upload all three to your PCBA service",
                "Use pull_file to transfer the ZIP back to Docker for ordering",
            ],
            "hint": (
                f"Auto-detected {layer_count}-layer board via {detection_method}. "
                "If wrong, force layer count with layers='2' or layers='4'."
            ),
        },
    }


# ─── Design Rules (DRU) ──────────────────────────────────────────────────

def _parse_edru_description(edru_path: str) -> str:
    """Parse an .edru file and extract its human-readable description."""
    try:
        tree = ET.parse(edru_path)
        root = tree.getroot()
        desc_elem = root.find(".//description[@language='en']")
        if desc_elem is not None and desc_elem.text:
            return html.unescape(desc_elem.text).strip()
    except Exception:
        pass
    return ""


def handle_set_design_rules(app: adsk.core.Application, args: dict) -> dict:
    """Set PCB design rules in the open board to match Adom's manufacturing capabilities.

    Loads a bundled .edru design rules file via EAGLE's `drc load` command.
    The .edru files are industry-tested rules from JLCPCB, adapted for Adom's
    3rd party fab manufacturing process.

    Auto-detects 2-layer vs 4-layer boards and loads the appropriate rule set.

    Args:
        action: "apply" (default) to load Adom rules, "export" to save current rules,
                "show" to display capabilities without changing anything
        layers: "auto" (default), "2", or "4" — which rule set to load
        outputPath: For "export" — path to save current .dru file
    """
    action = args.get("action", "apply")
    force_layers = args.get("layers", "auto")
    resources = _get_resources_dir()

    if action == "show":
        # Show capabilities from the 2-layer .edru description
        edru_2 = os.path.join(resources, "dru", "Adom-3rdPartyFab-2-Layer.edru")
        edru_4 = os.path.join(resources, "dru", "Adom-3rdPartyFab-4-Layer.edru")
        desc_2 = _parse_edru_description(edru_2) if os.path.exists(edru_2) else ""
        desc_4 = _parse_edru_description(edru_4) if os.path.exists(edru_4) else ""

        return {
            "success": True,
            "output": "3rd party fab design rules (from bundled .edru files)",
            "data": {
                "2layer_rules": desc_2 or "2-layer .edru file not found",
                "4layer_rules": desc_4 or "4-layer .edru file not found",
                "nextSteps": [
                    "Use fusion_set_design_rules with action='apply' to load rules into the board",
                    "Layer count is auto-detected, or force with layers='2' or layers='4'",
                ],
            },
        }

    # Verify we're in board layout
    try:
        ws = app.executeTextCommand("DebugCommands.ActiveWorkspace").strip()
    except Exception:
        ws = "unknown"

    if "Board" not in ws and "PCB" not in ws:
        return {
            "success": False,
            "error": f"Not in Board Layout (current: {ws}). Open a .brd file first.",
            "_hint": "Call fusion_show_2d_board or fusion_open_board {\"filePath\":\"...\"} to enter PCB Editor, then retry.",
            "data": {"activeWorkspace": ws},
        }

    if action == "export":
        output_path = args.get("outputPath", "C:/tmp/current-design-rules.dru")
        os.makedirs(os.path.dirname(output_path), exist_ok=True)
        try:
            app.executeTextCommand(f"Electron.run DRC SAVE '{output_path}'")
            time.sleep(0.5)
            if os.path.exists(output_path):
                return {
                    "success": True,
                    "output": f"Current design rules exported to {output_path}",
                    "data": {"outputPath": output_path},
                }
            return {
                "success": False,
                "error": "DRC SAVE did not produce output.",
            }
        except Exception as e:
            return {"success": False, "error": f"DRC SAVE failed: {e}"}

    # action == "apply": Load bundled .edru design rules
    if force_layers in ("2", "4"):
        layer_count = int(force_layers)
        detection_method = "manual"
    else:
        detection = _detect_layer_count(app)
        layer_count = detection["layerCount"]
        detection_method = detection["method"]

    # Resolve the .edru file: a fab pack the bridge staged (druPath, fetched from the wiki page
    # adom/pcb-design-rules), else the bundled 3rd party fab rules for this layer count.
    dru_path_arg = (args.get("druPath") or "").replace("\\", "/")
    if dru_path_arg:
        edru_path = dru_path_arg
        edru_filename = os.path.basename(edru_path)
    else:
        edru_filename = f"Adom-3rdPartyFab-{layer_count}-Layer.edru"
        edru_path = os.path.join(resources, "dru", edru_filename)

    if not os.path.exists(edru_path):
        return {
            "success": False,
            "error": f"Design rules file not found: {edru_path}",
            "data": {
                "hint": "Ensure resources/dru/ contains the .edru files. "
                        "Run install_addin.py to sync.",
            },
        }

    edru_path_fwd = edru_path.replace("\\", "/")

    # Load via EAGLE's drc load command (lowercase — this is the correct syntax)
    try:
        app.executeTextCommand(f"Electron.run drc load '{edru_path_fwd}'")
    except Exception as e:
        return {
            "success": False,
            "error": f"Failed to load design rules: {e}",
            "data": {"edruPath": edru_path_fwd},
        }

    # Parse description for the success message
    description = _parse_edru_description(edru_path)

    return {
        "success": True,
        "output": (
            f"{'Fab' if dru_path_arg else 'Adom'} {layer_count}-layer design rules loaded from {edru_filename}. "
            f"Detection method: {detection_method}."
        ),
        "message": (
            (f"Design rules from {edru_filename} are now active. Rule violations will now show as "
             f"DRC markers in the board editor.") if dru_path_arg else
            (f"3rd party fab {layer_count}-Layer design rules are now active. "
             f"Min trace: 0.08mm (3mil), min spacing: 0.08mm (3mil), min drill: 0.2mm (8mil). "
             f"Rule violations will now show as DRC markers in the board editor.")
        ),
        "data": {
            "edruFile": edru_filename,
            "edruPath": edru_path_fwd,
            "layerCount": layer_count,
            "detectionMethod": detection_method,
            "description": description,
            "activeWorkspace": ws,
            "nextSteps": [
                "Run fusion_electron_run with command 'DRC' to check for rule violations",
                "Run fusion_export_gerbers to export Gerber files with the correct CAM job",
                "Run fusion_export_bom and fusion_export_cpl to complete the manufacturing package",
            ],
            "hint": (
                "DRC violations appear as markers in the board. Use fusion_board_info to get "
                "structured DRC data, or fusion_export_board_image with preset 'fabrication' "
                "to visually inspect the result."
            ),
        },
    }


# ─── Board Image Export (PNG / PDF / layer views) ────────────────────────

# Layer presets for common manufacturing views
LAYER_PRESETS = {
    "all": {
        "description": "All layers visible",
        "command": "DISPLAY ALL",
    },
    "top_copper": {
        "description": "Top copper + pads + vias",
        "layers": [1, 17, 18],
    },
    "bottom_copper": {
        "description": "Bottom copper + pads + vias",
        "layers": [16, 17, 18],
    },
    "top_silkscreen": {
        "description": "Top silkscreen + names",
        "layers": [21, 25],
    },
    "bottom_silkscreen": {
        "description": "Bottom silkscreen + names",
        "layers": [22, 26],
    },
    "top_soldermask": {
        "description": "Top solder mask openings",
        "layers": [29],
    },
    "bottom_soldermask": {
        "description": "Bottom solder mask openings",
        "layers": [30],
    },
    "top_paste": {
        "description": "Top paste/stencil openings",
        "layers": [31],
    },
    "bottom_paste": {
        "description": "Bottom paste/stencil openings",
        "layers": [32],
    },
    "board_outline": {
        "description": "Board outline (dimension layer)",
        "layers": [20],
    },
    "drill": {
        "description": "Drill holes + vias",
        "layers": [44, 45, 17, 18],
    },
    "assembly_top": {
        "description": "Top assembly view — copper + silk + outline + names",
        "layers": [1, 17, 18, 20, 21, 25, 51],
    },
    "assembly_bottom": {
        "description": "Bottom assembly view — copper + silk + outline + names",
        "layers": [16, 17, 18, 20, 22, 26, 52],
    },
    "fabrication": {
        "description": "Full fabrication view — all copper + mask + silk + outline",
        "layers": [1, 16, 17, 18, 19, 20, 21, 22, 25, 26, 29, 30, 51],
    },
}


def handle_export_board_image(app: adsk.core.Application, args: dict) -> dict:
    """Export a PNG image of the open PCB board with configurable layer visibility.

    Uses EAGLE's EXPORT IMAGE command to render the board at a specified DPI.
    Layer presets make it easy to export common manufacturing views (silkscreen,
    copper, outline, etc.) without manually toggling layers.

    After export, restores the previous layer visibility.

    Args:
        outputPath: File path for the output image (default: C:/tmp/adom-board.png)
        dpi: Resolution in dots per inch (default: 300, max: 600)
        preset: Layer preset name — one of:
            "all", "top_copper", "bottom_copper", "top_silkscreen",
            "bottom_silkscreen", "top_soldermask", "bottom_soldermask",
            "top_paste", "bottom_paste", "board_outline", "drill",
            "assembly_top", "assembly_bottom", "fabrication"
        layers: Custom layer numbers (list of ints) — overrides preset if provided
        monochrome: If true, export in black & white (default: false)
        mirror: If true, mirror the image horizontally (default: false)
        listPresets: If true, return available presets instead of exporting
    """
    # List presets mode
    if args.get("listPresets", False):
        preset_list = {k: v["description"] for k, v in LAYER_PRESETS.items()}
        return {
            "success": True,
            "output": f"{len(preset_list)} layer presets available",
            "data": {"presets": preset_list},
        }

    output_path = args.get("outputPath", "C:/tmp/adom-board.png")
    dpi = min(int(args.get("dpi", 300)), 600)
    preset = args.get("preset", "all")
    custom_layers = args.get("layers", None)
    monochrome = args.get("monochrome", False)
    mirror_img = args.get("mirror", False)

    os.makedirs(os.path.dirname(output_path), exist_ok=True)

    # Verify we're in an electronics editor
    try:
        ws = app.executeTextCommand("DebugCommands.ActiveWorkspace").strip()
    except Exception:
        ws = "unknown"

    # Works in Board Layout and Schematic Editor
    is_electronics = False
    try:
        app.executeTextCommand("Electron.run DISPLAY")
        is_electronics = True
    except Exception:
        pass

    if not is_electronics:
        return {
            "success": False,
            "error": f"Not in Electronics editor (current: {ws}). "
                     "Open a .brd or .sch file first.",
            "_hint": "Call fusion_open_board {\"filePath\":\"...\"} or fusion_show_2d_board to enter the PCB editor, then retry.",
            "data": {
                "activeWorkspace": ws,
                "hint": "Use fusion_open_board or fusion_show_2d_board to enter the PCB editor.",
                "recoverySteps": [
                    "Open a .brd file with fusion_open_board",
                    "Or switch to 2D board view with fusion_show_2d_board",
                    "Then retry this command",
                ],
            },
        }

    # Save current layer state (we'll restore it after export)
    # There's no direct way to query current DISPLAY state, so we'll
    # just restore ALL after export.

    # Silkscreen registration: when exporting a silkscreen layer, fit the view to
    # the BOARD OUTLINE (layer 20) rather than the silk content. The silk content's
    # bbox is smaller than (and offset within) the board, so a content-fit PNG can't
    # be UV-mapped to the board's XY bbox by a downstream overlay (e.g. OCCT
    # apply_silkscreen). We show the outline ONLY to key WINDOW FIT to the board
    # bbox, then hide it before EXPORT so it's not baked into the silk PNG.
    # (Pixel-exact bbox crop + bottom-side mirror are done container-side, where an
    # image lib is available — Fusion's add-in Python has none.)
    silk_export_display = None  # set for silk presets: the silk-only DISPLAY used for EXPORT

    # Set layer visibility
    if custom_layers:
        layer_nums = " ".join(str(l) for l in custom_layers)
        display_cmd = f"DISPLAY NONE; DISPLAY {layer_nums}"
    elif preset in LAYER_PRESETS:
        p = LAYER_PRESETS[preset]
        if "command" in p:
            display_cmd = p["command"]
        else:
            layer_nums = " ".join(str(l) for l in p["layers"])
            if preset in ("top_silkscreen", "bottom_silkscreen"):
                # Fit with the outline visible; export with silk only.
                display_cmd = f"DISPLAY NONE; DISPLAY {layer_nums} 20"
                silk_export_display = f"DISPLAY NONE; DISPLAY {layer_nums}"
            else:
                display_cmd = f"DISPLAY NONE; DISPLAY {layer_nums}"
    else:
        return {
            "success": False,
            "error": f"Unknown preset '{preset}'. Use listPresets to see available options.",
            "_hint": "Call fusion_export_board_image with {\"listPresets\": true} to see valid preset names.",
            "data": {"availablePresets": list(LAYER_PRESETS.keys())},
        }

    try:
        # Set layers — split semicolon-separated commands into individual calls
        for cmd in display_cmd.split(";"):
            cmd = cmd.strip()
            if cmd:
                app.executeTextCommand(f"Electron.run {cmd}")
                time.sleep(0.2)
        time.sleep(0.3)

        # Zoom to fit. For silk presets the board outline (layer 20) is visible
        # here, so WINDOW FIT keys the view to the board's XY bbox.
        app.executeTextCommand("Electron.run WINDOW FIT")
        time.sleep(0.3)

        # Silk: hide the outline now (keep the same window extent) so the EXPORT
        # captures only silkscreen, board-bbox-keyed.
        if silk_export_display:
            for cmd in silk_export_display.split(";"):
                cmd = cmd.strip()
                if cmd:
                    app.executeTextCommand(f"Electron.run {cmd}")
                    time.sleep(0.15)
            time.sleep(0.2)

        # Delete existing file to avoid overwrite dialog
        output_path_escaped = output_path.replace("\\", "/")
        if os.path.exists(output_path_escaped):
            os.remove(output_path_escaped)

        # Export image
        export_cmd = f"EXPORT IMAGE '{output_path_escaped}' {dpi}"
        if monochrome:
            export_cmd = f"EXPORT IMAGE '{output_path_escaped}' MONOCHROME {dpi}"

        app.executeTextCommand(f"Electron.run SET CONFIRM YES")
        app.executeTextCommand(f"Electron.run {export_cmd}")
        time.sleep(1.0)

    except Exception as e:
        # Restore layers before returning error
        try:
            app.executeTextCommand("Electron.run DISPLAY ALL")
        except Exception:
            pass
        return {
            "success": False,
            "error": f"Export failed: {e}",
            "data": {
                "activeWorkspace": ws,
                "preset": preset,
                "hint": "EXPORT IMAGE may fail if no board/schematic content is visible. "
                        "Try WINDOW FIT first, or use a different layer preset.",
            },
        }

    # Restore all layers visible
    try:
        app.executeTextCommand("Electron.run DISPLAY ALL")
    except Exception:
        pass

    # Verify output file exists
    if not os.path.exists(output_path):
        return {
            "success": False,
            "error": f"EXPORT IMAGE did not create file at {output_path}. "
                     "The command may not be supported in this context.",
            "_hint": "Call fusion_show_2d_board first to ensure 2D PCB Editor is active, then retry.",
        }

    file_size = os.path.getsize(output_path)
    preset_desc = LAYER_PRESETS.get(preset, {}).get("description", preset) if not custom_layers else f"custom layers: {custom_layers}"

    return {
        "success": True,
        "output": f"Board image exported ({preset_desc}) at {dpi} DPI → {output_path}",
        "message": (
            f"PNG image of {preset_desc} at {dpi} DPI ({file_size // 1024} KB). "
            "Use pull_file to transfer to Docker for inspection or documentation."
        ),
        "data": {
            "outputPath": output_path,
            "dpi": dpi,
            "preset": preset if not custom_layers else "custom",
            "presetDescription": preset_desc,
            "fileSize": file_size,
            "fileSizeKB": file_size // 1024,
            "monochrome": monochrome,
            "activeWorkspace": ws,
            "nextSteps": [
                "Use pull_file to transfer the image to Docker for review",
                "Export more views: 'assembly_top', 'assembly_bottom', 'top_copper', 'board_outline'",
                "For manufacturing: also export gerbers, BOM, and CPL",
            ],
            "hint": (
                "Common presets: 'assembly_top' (component placement review), "
                "'top_copper' (routing check), 'fabrication' (full fab view), "
                "'board_outline' (dimensions check)."
            ),
        },
    }
