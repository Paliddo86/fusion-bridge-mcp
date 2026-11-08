"""Export commands — STEP, STL, 3MF, F3D, FBX.

All 3D export commands require an active Fusion Design (3D PCB view or a CAD
design document). Switch to 3D PCB view with fusion_show_3d_board before
exporting from an electronics project.
"""

import adsk.core
import adsk.fusion
import os
import time
from pathlib import Path


# Shared hint constants — reused across every export handler.
_HINT_NO_OUTPUT_PATH = (
    "Provide an outputPath arg like {\"outputPath\": \"C:\\\\path\\\\out.step\"}. "
    "The path must include the file extension matching the export format."
)
_HINT_FORMAT_UNAVAILABLE = (
    "This format isn't available in this Fusion build. "
    "Use fusion_export_step or fusion_export_stl as a fallback."
)
_HINT_EXPORT_FAILED = (
    "If this persists, call fusion_screenshot_fusion to check for a blocking dialog, "
    "then fusion_dismiss_blocking_dialogs to answer it. "
    "Wrong-workspace errors: call fusion_show_3d_board first."
)
_HINT_FILE_LOCKED = (
    "Close the file in any viewer (Blender, Windows Explorer preview, etc.), "
    "or call fusion_close_document first if it's open in Fusion, then retry."
)


def _get_active_design(app: adsk.core.Application):
    """Get the active Fusion design, or None."""
    product = app.activeProduct
    if product and product.productType == "DesignProductType":
        return adsk.fusion.Design.cast(product)
    return None


def _no_design_error():
    """Standard error when no active design is available."""
    return {
        "success": False,
        "error": "No active design open in Fusion 360.",
        "_hint": "If you're in PCB Editor (2D board), call fusion_show_3d_board to switch to 3D view. Otherwise call fusion_open_cloud_file to open a Design document, then retry.",
        "data": {
            "hint": "3D exports require a Design product. If you're in PCB Editor (2D board), "
                    "switch to 3D view first with fusion_show_3d_board.",
            "recoverySteps": [
                "Run fusion_show_3d_board to switch to 3D PCB view",
                "Or open a .f3d design document",
                "Then retry the export command",
            ],
        },
    }


def _export_success(fmt: str, output_path: str, extra_msg: str = "") -> dict:
    """Standard success response for 3D exports."""
    size = os.path.getsize(output_path) if os.path.exists(output_path) else 0
    return {
        "success": True,
        "output": f"Exported {fmt} to {output_path}",
        "message": f"{fmt} file exported ({size // 1024} KB). "
                   f"{extra_msg}"
                   "Use pull_file to transfer to Docker.",
        "data": {
            "outputPath": output_path,
            "format": fmt.lower(),
            "fileSizeKB": size // 1024,
            "nextSteps": [
                "Use pull_file to transfer the file back to Docker",
                "Export other formats: fusion_export_step, fusion_export_stl, fusion_export_3mf, fusion_export_f3d, fusion_export_fbx",
            ],
        },
    }


def handle_export_step(app: adsk.core.Application, args: dict) -> dict:
    """Export the active design as a STEP file (.step).

    Industry-standard CAD interchange format. Best for mechanical CAD
    interoperability (SolidWorks, CATIA, Creo, etc.).

    Args:
        outputPath: Full path for the output .step file (required).
    """
    output_path = args.get("outputPath", "")
    if not output_path:
        return {"success": False, "error": "No outputPath specified", "_hint": _HINT_NO_OUTPUT_PATH}

    design = _get_active_design(app)
    if not design:
        return _no_design_error()

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    export_mgr = design.exportManager
    options = export_mgr.createSTEPExportOptions(output_path)
    result = export_mgr.execute(options)

    if result:
        return _export_success("STEP", output_path,
                               "Industry-standard CAD interchange — opens in any CAD tool. ")
    return {"success": False, "error": "STEP export failed", "_hint": _HINT_EXPORT_FAILED}


def handle_export_stl(app: adsk.core.Application, args: dict) -> dict:
    """Export the active design as an STL file (.stl).

    Mesh-based format for 3D printing and visualization.

    Args:
        outputPath: Full path for the output .stl file (required).
        refinement: "low", "medium", or "high" (default: "medium").
    """
    output_path = args.get("outputPath", "")
    if not output_path:
        return {"success": False, "error": "No outputPath specified", "_hint": _HINT_NO_OUTPUT_PATH}

    design = _get_active_design(app)
    if not design:
        return _no_design_error()

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    root = design.rootComponent
    export_mgr = design.exportManager
    options = export_mgr.createSTLExportOptions(root, output_path)

    refinement = args.get("refinement", "medium").lower()
    refinement_map = {
        "low": adsk.fusion.MeshRefinementSettings.MeshRefinementLow,
        "medium": adsk.fusion.MeshRefinementSettings.MeshRefinementMedium,
        "high": adsk.fusion.MeshRefinementSettings.MeshRefinementHigh,
    }
    options.meshRefinement = refinement_map.get(
        refinement, adsk.fusion.MeshRefinementSettings.MeshRefinementMedium
    )

    result = export_mgr.execute(options)

    if result:
        return _export_success("STL", output_path,
                               f"Mesh refinement: {refinement}. Good for 3D printing. ")
    return {"success": False, "error": "STL export failed", "_hint": _HINT_EXPORT_FAILED}


def handle_export_3mf(app: adsk.core.Application, args: dict) -> dict:
    """Export the active design as a 3MF file (.3mf).

    Modern 3D printing format — supports color, materials, and multi-body.

    Args:
        outputPath: Full path for the output .3mf file (required).
    """
    output_path = args.get("outputPath", "")
    if not output_path:
        return {"success": False, "error": "No outputPath specified", "_hint": _HINT_NO_OUTPUT_PATH}

    design = _get_active_design(app)
    if not design:
        return _no_design_error()

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    root = design.rootComponent
    export_mgr = design.exportManager
    options = export_mgr.createC3MFExportOptions(root, output_path)
    result = export_mgr.execute(options)

    if result:
        return _export_success("3MF", output_path,
                               "Modern 3D printing format with color/material support. ")
    return {"success": False, "error": "3MF export failed", "_hint": _HINT_EXPORT_FAILED}


def handle_export_f3d(app: adsk.core.Application, args: dict) -> dict:
    """Export the active design as a Fusion 360 archive (.f3d).

    Native Fusion format — preserves all parametric features, sketches,
    timeline history, and component references. Best for archiving or
    sharing with other Fusion 360 users.

    Args:
        outputPath: Full path for the output .f3d file (required).
    """
    output_path = args.get("outputPath", "")
    if not output_path:
        return {"success": False, "error": "No outputPath specified", "_hint": _HINT_NO_OUTPUT_PATH}

    design = _get_active_design(app)
    if not design:
        return _no_design_error()

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    export_mgr = design.exportManager
    options = export_mgr.createFusionArchiveExportOptions(output_path)
    result = export_mgr.execute(options)

    if result:
        return _export_success("F3D", output_path,
                               "Fusion 360 native archive — preserves all features and history. ")
    return {"success": False, "error": "F3D export failed", "_hint": _HINT_EXPORT_FAILED}


def handle_export_usdz(app: adsk.core.Application, args: dict) -> dict:
    """Export the active design as a USDZ file (.usdz).

    Universal Scene Description (USD) — Pixar's scene-graph format adopted
    by Apple, NVIDIA, and Autodesk. Preserves:
    - Full component hierarchy (board, copper layers, soldermask, packages)
    - PBR materials and colors
    - Named nodes for toggling parts on/off in viewers

    Best format for digital twins, AR (Apple Quick Look), and GLB conversion
    via Blender. The component hierarchy maps directly to toggleable nodes
    in GLB viewers.

    Pipeline: Fusion → USDZ → Blender → GLB (or view USDZ directly on iOS)

    Args:
        outputPath: Full path for the output .usdz file (required).
    """
    output_path = args.get("outputPath", "")
    if not output_path:
        return {"success": False, "error": "No outputPath specified", "_hint": _HINT_NO_OUTPUT_PATH}

    design = _get_active_design(app)
    if not design:
        return _no_design_error()

    # Ensure .usdz extension
    if not output_path.lower().endswith(('.usdz', '.usd', '.usda', '.usdc')):
        output_path = str(Path(output_path).with_suffix('.usdz'))

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    # Pre-delete to avoid overwrite dialogs
    if os.path.exists(output_path):
        try:
            os.remove(output_path)
        except PermissionError:
            return {
                "success": False,
                "error": f"Cannot overwrite {output_path} — file is locked.",
                "_hint": _HINT_FILE_LOCKED,
                "data": {
                    "hint": "Close the file in any viewer and retry.",
                    "outputPath": output_path,
                },
            }

    export_mgr = design.exportManager
    try:
        options = export_mgr.createUSDExportOptions(output_path)
    except AttributeError:
        return {
            "success": False,
            "error": "USDZ export not available in this Fusion 360 version.",
            "_hint": _HINT_FORMAT_UNAVAILABLE,
            "data": {
                "hint": "createUSDExportOptions requires a recent Fusion 360 build. "
                        "Export as STEP instead and convert via Blender.",
                "recoverySteps": [
                    "Export STEP: fusion_export_step (then convert in Blender)",
                    "Export OBJ: fusion_export_fbx (falls back to OBJ)",
                ],
            },
        }

    result = export_mgr.execute(options)

    if result and os.path.exists(output_path):
        size = os.path.getsize(output_path)

        # Get component hierarchy for the response
        components = []
        try:
            root = design.rootComponent
            for i in range(root.occurrences.count):
                occ = root.occurrences.item(i)
                components.append(occ.name)
        except Exception:
            pass

        return {
            "success": True,
            "output": f"Exported USDZ to {output_path}",
            "message": f"USDZ exported ({size // 1024} KB) with full component hierarchy. "
                       f"Contains {len(components)} top-level components that map to "
                       f"toggleable nodes in GLB viewers. "
                       f"Use pull_file to transfer to Docker.",
            "data": {
                "outputPath": output_path,
                "format": "usdz",
                "fileSizeKB": size // 1024,
                "components": components,
                "hint": "USDZ preserves PBR materials and scene hierarchy. "
                        "Each component (Board, copper layers, soldermask, Packages) "
                        "becomes a separate toggleable node in the GLB.",
                "nextSteps": [
                    "Use pull_file to transfer the USDZ to Docker",
                    "Convert to GLB: blender --background --python-expr "
                    "\"import bpy; bpy.ops.wm.read_factory_settings(use_empty=True); "
                    "bpy.ops.wm.usd_import(filepath='input.usdz'); "
                    "bpy.ops.export_scene.gltf(filepath='output.glb')\"",
                    "Or view directly on iOS/macOS — USDZ is Apple's native AR format",
                    "Or import into Unity/Unreal Engine for interactive 3D",
                ],
            },
        }

    return {"success": False, "error": "USDZ export failed", "_hint": _HINT_EXPORT_FAILED}


def handle_export_fbx(app: adsk.core.Application, args: dict) -> dict:
    """Export the active design as an FBX file (.fbx).

    Autodesk FBX format — widely used for 3D interchange with Blender,
    Unity, Unreal Engine, and other DCC tools.

    NOTE: FBX export is not available in current Fusion 360 builds via the API.
    This command will fail clearly — use fusion_export_usdz or fusion_export_step
    instead. No silent fallbacks.

    Args:
        outputPath: Full path for the output .fbx file (required).
    """
    output_path = args.get("outputPath", "")
    if not output_path:
        return {"success": False, "error": "No outputPath specified", "_hint": _HINT_NO_OUTPUT_PATH}

    design = _get_active_design(app)
    if not design:
        return _no_design_error()

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    # Pre-delete to avoid overwrite dialogs
    if os.path.exists(output_path):
        try:
            os.remove(output_path)
        except PermissionError:
            return {
                "success": False,
                "error": f"Cannot overwrite {output_path} — file is locked.",
                "_hint": _HINT_FILE_LOCKED,
                "data": {
                    "hint": "The file may be open in another application. Close it and retry.",
                    "outputPath": output_path,
                },
            }

    # Try FBX via exportManager API
    export_mgr = design.exportManager
    try:
        options = export_mgr.createFBXExportOptions(output_path)
        result = export_mgr.execute(options)
        if result and os.path.exists(output_path):
            return _export_success("FBX", output_path,
                                   "Convert to GLB via Blender: "
                                   "blender --background --python-expr "
                                   "\"import bpy; bpy.ops.import_scene.fbx(filepath='<fbx>'); "
                                   "bpy.ops.export_scene.gltf(filepath='<glb>')\" ")
    except (AttributeError, Exception):
        pass  # createFBXExportOptions doesn't exist in this Fusion build

    # FBX not available — fail clearly, no fallbacks
    return {
        "success": False,
        "error": "FBX export not available in this Fusion 360 build. "
                 "Use fusion_export_usdz or fusion_export_step instead.",
        "_hint": _HINT_FORMAT_UNAVAILABLE,
        "data": {
            "hint": "FBX is not supported via the Fusion API in current builds. "
                    "Use fusion_export_usdz (best for Blender/GLB pipeline, preserves "
                    "PBR materials and scene hierarchy) or fusion_export_step "
                    "(highest geometric fidelity).",
            "alternatives": ["fusion_export_usdz", "fusion_export_step", "fusion_export_stl"],
        },
    }


def handle_export_dxf(app: adsk.core.Application, args: dict) -> dict:
    """Export the active design as a DXF file (.dxf).

    AutoCAD DXF (Drawing Exchange Format) — universal 2D/3D CAD interchange.
    Widely supported by all CAD tools, laser cutters, CNC routers, and
    PCB fabrication houses.

    Args:
        outputPath: Full path for the output .dxf file (required).
    """
    output_path = args.get("outputPath", "")
    if not output_path:
        return {"success": False, "error": "No outputPath specified", "_hint": _HINT_NO_OUTPUT_PATH}

    design = _get_active_design(app)
    if not design:
        return _no_design_error()

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    if os.path.exists(output_path):
        try:
            os.remove(output_path)
        except PermissionError:
            return {
                "success": False,
                "error": f"Cannot overwrite {output_path} — file is locked.",
                "_hint": _HINT_FILE_LOCKED,
                "data": {"outputPath": output_path},
            }

    export_mgr = design.exportManager
    try:
        options = export_mgr.createDXFExportOptions(output_path)
        result = export_mgr.execute(options)
        if result and os.path.exists(output_path):
            return _export_success("DXF", output_path,
                                   "Universal CAD interchange — opens in AutoCAD, LibreCAD, etc. ")
    except (AttributeError, Exception):
        pass

    return {
        "success": False,
        "error": "DXF export failed or not available in this Fusion 360 build.",
        "_hint": _HINT_FORMAT_UNAVAILABLE,
        "data": {
            "hint": "Try fusion_export_step for CAD interchange instead.",
            "alternatives": ["fusion_export_step", "fusion_export_dwg"],
        },
    }


def handle_export_dwg(app: adsk.core.Application, args: dict) -> dict:
    """Export the active design as a DWG file (.dwg).

    AutoCAD native format — the industry standard for 2D/3D technical
    drawings. Best for sharing with AutoCAD, BricsCAD, DraftSight users.

    Args:
        outputPath: Full path for the output .dwg file (required).
    """
    output_path = args.get("outputPath", "")
    if not output_path:
        return {"success": False, "error": "No outputPath specified", "_hint": _HINT_NO_OUTPUT_PATH}

    design = _get_active_design(app)
    if not design:
        return _no_design_error()

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    if os.path.exists(output_path):
        try:
            os.remove(output_path)
        except PermissionError:
            return {
                "success": False,
                "error": f"Cannot overwrite {output_path} — file is locked.",
                "_hint": _HINT_FILE_LOCKED,
                "data": {"outputPath": output_path},
            }

    export_mgr = design.exportManager
    try:
        options = export_mgr.createDWGExportOptions(output_path)
        result = export_mgr.execute(options)
        if result and os.path.exists(output_path):
            return _export_success("DWG", output_path,
                                   "AutoCAD native format — opens in AutoCAD, BricsCAD, etc. ")
    except (AttributeError, Exception):
        pass

    return {
        "success": False,
        "error": "DWG export failed or not available in this Fusion 360 build.",
        "_hint": _HINT_FORMAT_UNAVAILABLE,
        "data": {
            "hint": "Try fusion_export_dxf or fusion_export_step instead.",
            "alternatives": ["fusion_export_dxf", "fusion_export_step"],
        },
    }


def handle_export_iges(app: adsk.core.Application, args: dict) -> dict:
    """Export the active design as an IGES file (.igs/.iges).

    Legacy CAD interchange format. Still used by some older CAD systems
    and manufacturing tools. STEP is preferred for modern workflows.

    Args:
        outputPath: Full path for the output .igs file (required).
    """
    output_path = args.get("outputPath", "")
    if not output_path:
        return {"success": False, "error": "No outputPath specified", "_hint": _HINT_NO_OUTPUT_PATH}

    design = _get_active_design(app)
    if not design:
        return _no_design_error()

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    if os.path.exists(output_path):
        try:
            os.remove(output_path)
        except PermissionError:
            return {
                "success": False,
                "error": f"Cannot overwrite {output_path} — file is locked.",
                "_hint": _HINT_FILE_LOCKED,
                "data": {"outputPath": output_path},
            }

    export_mgr = design.exportManager
    try:
        options = export_mgr.createIGESExportOptions(output_path)
        result = export_mgr.execute(options)
        if result and os.path.exists(output_path):
            return _export_success("IGES", output_path,
                                   "Legacy CAD interchange — STEP is preferred for modern use. ")
    except (AttributeError, Exception):
        pass

    return {
        "success": False,
        "error": "IGES export failed or not available in this Fusion 360 build.",
        "_hint": _HINT_FORMAT_UNAVAILABLE,
        "data": {
            "hint": "Try fusion_export_step instead (modern replacement for IGES).",
            "alternatives": ["fusion_export_step"],
        },
    }


def handle_export_obj(app: adsk.core.Application, args: dict) -> dict:
    """Export the active design as an OBJ file (.obj).

    Wavefront OBJ — simple mesh format widely supported by 3D viewers,
    game engines, and DCC tools. No materials or hierarchy.

    Args:
        outputPath: Full path for the output .obj file (required).
    """
    output_path = args.get("outputPath", "")
    if not output_path:
        return {"success": False, "error": "No outputPath specified", "_hint": _HINT_NO_OUTPUT_PATH}

    design = _get_active_design(app)
    if not design:
        return _no_design_error()

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    if os.path.exists(output_path):
        try:
            os.remove(output_path)
        except PermissionError:
            return {
                "success": False,
                "error": f"Cannot overwrite {output_path} — file is locked.",
                "_hint": _HINT_FILE_LOCKED,
                "data": {"outputPath": output_path},
            }

    export_mgr = design.exportManager
    try:
        options = export_mgr.createOBJExportOptions(output_path)
        result = export_mgr.execute(options)
        if result and os.path.exists(output_path):
            return _export_success("OBJ", output_path,
                                   "Simple mesh format — opens in Blender, Unity, etc. ")
    except (AttributeError, Exception):
        pass

    return {
        "success": False,
        "error": "OBJ export failed or not available in this Fusion 360 build.",
        "_hint": _HINT_FORMAT_UNAVAILABLE,
        "data": {
            "hint": "Try fusion_export_stl or fusion_export_usdz instead.",
            "alternatives": ["fusion_export_stl", "fusion_export_usdz"],
        },
    }


def handle_export_sat(app: adsk.core.Application, args: dict) -> dict:
    """Export the active design as a SAT file (.sat).

    ACIS SAT — solid modeling kernel format. Used by SolidWorks,
    SpaceClaim, and other ACIS-based CAD tools.

    Args:
        outputPath: Full path for the output .sat file (required).
    """
    output_path = args.get("outputPath", "")
    if not output_path:
        return {"success": False, "error": "No outputPath specified", "_hint": _HINT_NO_OUTPUT_PATH}

    design = _get_active_design(app)
    if not design:
        return _no_design_error()

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    if os.path.exists(output_path):
        try:
            os.remove(output_path)
        except PermissionError:
            return {
                "success": False,
                "error": f"Cannot overwrite {output_path} — file is locked.",
                "_hint": _HINT_FILE_LOCKED,
                "data": {"outputPath": output_path},
            }

    export_mgr = design.exportManager
    try:
        options = export_mgr.createSATExportOptions(output_path)
        result = export_mgr.execute(options)
        if result and os.path.exists(output_path):
            return _export_success("SAT", output_path,
                                   "ACIS solid modeling format — opens in SolidWorks, SpaceClaim. ")
    except (AttributeError, Exception):
        pass

    return {
        "success": False,
        "error": "SAT export failed or not available in this Fusion 360 build.",
        "_hint": _HINT_FORMAT_UNAVAILABLE,
        "data": {
            "hint": "Try fusion_export_step instead.",
            "alternatives": ["fusion_export_step"],
        },
    }


def handle_export_skp(app: adsk.core.Application, args: dict) -> dict:
    """Export the active design as a SketchUp file (.skp).

    SketchUp format — popular for architectural visualization and
    simple 3D modeling.

    Args:
        outputPath: Full path for the output .skp file (required).
    """
    output_path = args.get("outputPath", "")
    if not output_path:
        return {"success": False, "error": "No outputPath specified", "_hint": _HINT_NO_OUTPUT_PATH}

    design = _get_active_design(app)
    if not design:
        return _no_design_error()

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    if os.path.exists(output_path):
        try:
            os.remove(output_path)
        except PermissionError:
            return {
                "success": False,
                "error": f"Cannot overwrite {output_path} — file is locked.",
                "_hint": _HINT_FILE_LOCKED,
                "data": {"outputPath": output_path},
            }

    export_mgr = design.exportManager
    try:
        options = export_mgr.createSKPExportOptions(output_path)
        result = export_mgr.execute(options)
        if result and os.path.exists(output_path):
            return _export_success("SKP", output_path,
                                   "SketchUp format — opens in SketchUp, Trimble viewers. ")
    except (AttributeError, Exception):
        pass

    return {
        "success": False,
        "error": "SketchUp export failed or not available in this Fusion 360 build.",
        "_hint": _HINT_FORMAT_UNAVAILABLE,
        "data": {
            "hint": "Try fusion_export_step or fusion_export_stl instead.",
            "alternatives": ["fusion_export_step", "fusion_export_stl"],
        },
    }
