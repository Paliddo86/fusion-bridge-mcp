"""Screenshot command — capture the Fusion 360 viewport."""

import adsk.core
import os
import time
from pathlib import Path


def _build_view_orientations():
    """Build orientation map at runtime to avoid crashing if enum values differ."""
    orientations = {"home": None}
    try:
        vo = adsk.core.ViewOrientations
        mapping = {
            "front": "FrontViewOrientation",
            "back": "BackViewOrientation",
            "top": "TopViewOrientation",
            "bottom": "BottomViewOrientation",
            "left": "LeftViewOrientation",
            "right": "RightViewOrientation",
            "iso_top_front_right": "IsoTopFrontRightViewOrientation",
            "iso_top_front_left": "IsoTopFrontLeftViewOrientation",
            "iso_top_back_right": "IsoTopBackRightViewOrientation",
            "iso_top_back_left": "IsoTopBackLeftViewOrientation",
            "iso_bottom_front_right": "IsoBottomFrontRightViewOrientation",
            "iso_bottom_front_left": "IsoBottomFrontLeftViewOrientation",
            "iso_bottom_back_right": "IsoBottomBackRightViewOrientation",
            "iso_bottom_back_left": "IsoBottomBackLeftViewOrientation",
        }
        for name, attr in mapping.items():
            if hasattr(vo, attr):
                orientations[name] = getattr(vo, attr)
    except Exception:
        pass
    return orientations


def handle_take_screenshot(app: adsk.core.Application, args: dict) -> dict:
    """Capture the active viewport to a PNG file.

    Uses Fusion's viewport render API (same as Capture Image from the menu)
    for high-quality renders at any resolution, without opening a dialog.

    Args:
        args: Dict with:
            - outputPath: Full path for the output .png file (required).
            - width: Image width in pixels (default: 1920).
            - height: Image height in pixels (default: 1080).
            - orientation: Camera orientation preset (optional).
              One of: "home", "front", "back", "top", "bottom", "left", "right",
              "iso_top_front_right", "iso_top_front_left", etc.
            - fitToView: If true, zoom to fit all geometry (default: true).
            - listOrientations: If true, return available orientations.
    """
    VIEW_ORIENTATIONS = _build_view_orientations()

    # List orientations mode
    if args.get("listOrientations", False):
        return {
            "success": True,
            "output": f"{len(VIEW_ORIENTATIONS)} view orientations available",
            "data": {"orientations": list(VIEW_ORIENTATIONS.keys())},
        }

    output_path = args.get("outputPath", "")
    if not output_path:
        return {"success": False, "error": "No outputPath specified"}

    width = int(args.get("width", 1920))
    height = int(args.get("height", 1080))
    orientation = args.get("orientation", "").lower().strip()
    fit_to_view = args.get("fitToView", True)

    viewport = app.activeViewport
    if not viewport:
        return {
            "success": False,
            "error": "No active viewport",
            "_hint": "Call fusion_open_cloud_file or fusion_open_board to open a document first — there is nothing to screenshot.",
        }

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    # Set camera orientation if specified
    orientation_applied = None
    if orientation:
        if orientation not in VIEW_ORIENTATIONS:
            return {
                "success": False,
                "error": f"Unknown orientation '{orientation}'. "
                         f"Available: {', '.join(VIEW_ORIENTATIONS.keys())}",
            }

        camera = viewport.camera
        view_enum = VIEW_ORIENTATIONS[orientation]

        if view_enum is not None:
            camera.viewOrientation = view_enum
        camera.isFitView = True if fit_to_view else False
        viewport.camera = camera

        # Let Fusion render the new view
        adsk.doEvents()
        time.sleep(0.3)
        adsk.doEvents()

        orientation_applied = orientation
    elif fit_to_view:
        camera = viewport.camera
        camera.isFitView = True
        viewport.camera = camera
        adsk.doEvents()

    result = viewport.saveAsImageFile(output_path, width, height)

    if result:
        size = os.path.getsize(output_path) if os.path.exists(output_path) else 0
        response = {
            "success": True,
            "output": f"Screenshot saved to {output_path} ({width}x{height})",
            "data": {
                "outputPath": output_path,
                "width": width,
                "height": height,
                "fileSizeKB": size // 1024,
            },
        }
        if orientation_applied:
            response["data"]["orientation"] = orientation_applied
        return response

    return {
        "success": False,
        "error": "Screenshot capture failed",
        "_hint": "Try desktop_screenshot_window with the Fusion HWND as a fallback (captures the full window including UI chrome).",
    }
