"""Silkscreen capture — render isolated top/bottom silkscreen layer PNGs.

Hides all 3D bodies except the target silkscreen layer, aims the camera
at the PCB bounding box, captures a 2048x2048 transparent-background PNG,
then restores the original visibility and camera state.

This is a low-level primitive. Orchestration (which documents to open,
which workspace to activate, where to save) belongs in the skill layer.
"""

import adsk.core
import adsk.fusion
import os
import time
from pathlib import Path


def _pump_events(iterations=5):
    """Flush Fusion's event queue so visibility/camera changes render."""
    try:
        for _ in range(iterations):
            adsk.doEvents()
    except Exception:
        pass


def _compute_pcb_bbox(design):
    """Union bounding boxes of all bodies to frame the board."""
    combined = None

    def merge(bbox):
        nonlocal combined
        if not bbox:
            return
        if combined is None:
            combined = adsk.core.BoundingBox3D.create(
                bbox.minPoint.copy(), bbox.maxPoint.copy()
            )
        else:
            combined.expand(bbox.minPoint)
            combined.expand(bbox.maxPoint)

    for ci in range(design.allComponents.count):
        comp = design.allComponents.item(ci)
        for bi in range(comp.bRepBodies.count):
            try:
                merge(comp.bRepBodies.item(bi).boundingBox)
            except Exception:
                pass
        for mi in range(comp.meshBodies.count):
            try:
                merge(comp.meshBodies.item(mi).boundingBox)
            except Exception:
                pass
    return combined


def _bbox_center_and_extent(bbox):
    if bbox is None:
        return adsk.core.Point3D.create(0, 0, 0), 20.0, 20.0
    mn, mx = bbox.minPoint, bbox.maxPoint
    cx = (mn.x + mx.x) / 2
    cy = (mn.y + mx.y) / 2
    cz = (mn.z + mx.z) / 2
    xy_ext = max(abs(mx.x - mn.x), abs(mx.y - mn.y), 1.0)
    z_ext = max(abs(mx.z - mn.z), 1.0)
    return adsk.core.Point3D.create(cx, cy, cz), xy_ext, z_ext


def _is_silk_match(name, target):
    """True if body/mesh/canvas name matches the target silkscreen layer."""
    n = name.lower()
    return "silk" in n and target in n


def _store_visibility(design):
    """Snapshot visibility of all bodies, meshes, and canvases."""
    state = {"root_b": {}, "root_m": {}, "comps": {}}
    root = design.rootComponent
    for i in range(root.bRepBodies.count):
        state["root_b"][i] = root.bRepBodies.item(i).isVisible
    for i in range(root.meshBodies.count):
        state["root_m"][i] = root.meshBodies.item(i).isVisible
    for ci in range(design.allComponents.count):
        comp = design.allComponents.item(ci)
        ck = ci
        state["comps"][ck] = {"b": {}, "m": {}, "cv": {}}
        for bi in range(comp.bRepBodies.count):
            state["comps"][ck]["b"][bi] = comp.bRepBodies.item(bi).isVisible
        for mi in range(comp.meshBodies.count):
            state["comps"][ck]["m"][mi] = comp.meshBodies.item(mi).isVisible
        for cvi in range(comp.canvases.count):
            try:
                c = comp.canvases.item(cvi)
                state["comps"][ck]["cv"][cvi] = getattr(c, "isLightBulbOn", True)
            except Exception:
                pass
    return state


def _restore_visibility(design, state):
    """Restore a previous visibility snapshot."""
    if not state:
        return
    root = design.rootComponent
    for i, vis in state.get("root_b", {}).items():
        try:
            root.bRepBodies.item(i).isVisible = vis
        except Exception:
            pass
    for i, vis in state.get("root_m", {}).items():
        try:
            root.meshBodies.item(i).isVisible = vis
        except Exception:
            pass
    for ci in range(design.allComponents.count):
        cs = state.get("comps", {}).get(ci, {})
        comp = design.allComponents.item(ci)
        for bi, vis in cs.get("b", {}).items():
            try:
                comp.bRepBodies.item(bi).isVisible = vis
            except Exception:
                pass
        for mi, vis in cs.get("m", {}).items():
            try:
                comp.meshBodies.item(mi).isVisible = vis
            except Exception:
                pass
        for cvi, vis in cs.get("cv", {}).items():
            try:
                c = comp.canvases.item(cvi)
                if hasattr(c, "isLightBulbOn"):
                    c.isLightBulbOn = vis
            except Exception:
                pass


def _hide_all_except_silk(design, target):
    """Hide everything except the target silkscreen layer ('top'/'bottom').

    Silkscreen in Fusion 3D PCB is stored as Canvases (not bodies).
    Canvases are overlaid on the Board body, so the Board body must stay
    visible as a backdrop — otherwise the canvas renders over nothing and
    the viewport capture comes out blank.
    """
    root = design.rootComponent

    for i in range(root.bRepBodies.count):
        b = root.bRepBodies.item(i)
        b.isVisible = False
    for i in range(root.meshBodies.count):
        m = root.meshBodies.item(i)
        m.isVisible = False

    for ci in range(design.allComponents.count):
        comp = design.allComponents.item(ci)
        comp_name = comp.name.lower()

        for bi in range(comp.bRepBodies.count):
            b = comp.bRepBodies.item(bi)
            # Keep the Board body visible — canvases need it as a backdrop
            if comp_name == "board" and b.name.lower() == "board":
                b.isVisible = True
            else:
                b.isVisible = False

        for mi in range(comp.meshBodies.count):
            m = comp.meshBodies.item(mi)
            m.isVisible = False

        for cvi in range(comp.canvases.count):
            c = comp.canvases.item(cvi)
            try:
                if hasattr(c, "isLightBulbOn"):
                    c.isLightBulbOn = _is_silk_match(c.name, target)
            except Exception:
                pass


def _capture_png(viewport, output_path, width=2048, height=2048):
    """Save viewport as PNG, preferring transparent background."""
    try:
        opts = adsk.core.SaveImageFileOptions.create(output_path)
        opts.width = width
        opts.height = height
        opts.isBackgroundTransparent = True
        opts.isAntiAliased = True
        if viewport.saveAsImageFileWithOptions(opts):
            return True
    except Exception:
        pass
    try:
        return viewport.saveAsImageFile(output_path, width, height)
    except Exception:
        return False


# ── Public handler ──────────────────────────────────────────────────────

def handle_take_silkscreen_screenshot(app: adsk.core.Application, args: dict) -> dict:
    """Capture an isolated silkscreen layer as a transparent-background PNG.

    Requires the active document to be a 3D Design (not PCB Editor).
    Call fusion_show_3d_board first if you're in the 2D board view.

    Args:
        outputPath (str): Full path for the output .png file (required).
        layer (str): "top" or "bottom" (required).
        width (int): Image width in pixels (default: 2048).
        height (int): Image height in pixels (default: 2048).

    Returns success + outputPath + fileSizeKB.
    """
    output_path = args.get("outputPath", "")
    if not output_path:
        return {
            "success": False,
            "error": "outputPath is required",
            "_hint": 'Provide {"outputPath": "C:/tmp/silk_top.png", "layer": "top"}',
        }

    layer = args.get("layer", "").lower().strip()
    if layer not in ("top", "bottom"):
        return {
            "success": False,
            "error": f"layer must be 'top' or 'bottom', got '{layer}'",
            "_hint": '{"outputPath": "...", "layer": "top"}',
        }

    width = int(args.get("width", 2048))
    height = int(args.get("height", 2048))

    # Need an active Design product (3D view)
    product = app.activeProduct
    if not product or product.productType != "DesignProductType":
        return {
            "success": False,
            "error": "No active Design document. Silkscreen capture requires the 3D PCB view.",
            "_hint": "Call fusion_show_3d_board to switch to the 3D view, then retry.",
        }
    design = adsk.fusion.Design.cast(product)

    viewport = app.activeViewport
    if not viewport:
        return {"success": False, "error": "No active viewport"}

    Path(output_path).parent.mkdir(parents=True, exist_ok=True)

    # Save state
    camera = viewport.camera
    orig_eye = camera.eye.copy()
    orig_target = camera.target.copy()
    orig_up = camera.upVector.copy()
    bbox = _compute_pcb_bbox(design)
    vis_state = _store_visibility(design)

    try:
        center, xy_ext, z_ext = _bbox_center_and_extent(bbox)

        # Hide everything except the target silkscreen layer
        _hide_all_except_silk(design, layer)

        # Aim camera straight down (top) or straight up (bottom)
        if layer == "top":
            eye_z = center.z + z_ext + xy_ext * 2.0
        else:
            eye_z = center.z - z_ext - xy_ext * 2.0

        camera.eye = adsk.core.Point3D.create(center.x, center.y, eye_z)
        camera.target = adsk.core.Point3D.create(center.x, center.y, center.z)
        camera.upVector = adsk.core.Vector3D.create(0, 1, 0)
        camera.viewExtents = xy_ext * 1.2
        camera.isSmoothTransition = False
        viewport.camera = camera
        viewport.refresh()

        # Let GPU finish rendering the new visibility state
        _pump_events()
        time.sleep(1.0)

        ok = _capture_png(viewport, output_path, width, height)
        if not ok:
            return {
                "success": False,
                "error": "Viewport capture failed",
                "_hint": "Try desktop_screenshot_window as a fallback.",
            }

        size = os.path.getsize(output_path) if os.path.exists(output_path) else 0
        return {
            "success": True,
            "output": f"Silkscreen {layer} captured to {output_path}",
            "data": {
                "outputPath": output_path.replace("\\", "/"),
                "layer": layer,
                "width": width,
                "height": height,
                "fileSizeKB": round(size / 1024, 1),
            },
        }
    finally:
        # Always restore
        camera.eye = orig_eye
        camera.target = orig_target
        camera.upVector = orig_up
        viewport.camera = camera
        viewport.refresh()
        _restore_visibility(design, vis_state)
