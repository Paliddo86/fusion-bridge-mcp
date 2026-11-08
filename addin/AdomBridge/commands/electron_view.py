"""Video-friendly view control for the Fusion Electronics (EAGLE) editor.

One bridge call = a whole smooth motion, because the loop runs HERE inside the add-in
(tight timing + a repaint per frame), not as N round-trips from the bridge server.

  fusion_electron_zoom   - smooth zoom in/out (or fit)
  fusion_electron_pan    - smooth pan/zoom to a board-coordinate box
  fusion_electron_select - select a part by name (or x/y) and surface its properties

All EAGLE motion is `app.executeTextCommand("Electron.run WINDOW ...")`; each frame is
forced to render with `Electron.ImmediateRepaint /full` (the 2D analogue of vp.refresh()).
"""

import time
import adsk.core

# Last view box we set, in mm (x1, y1, x2, y2), and the editor it belongs to. Lets pan
# interpolate from where the view actually is. None until the first pan establishes it;
# forgotten when the active document or product changes (a box from the symbol editor
# must never seed a glide in the footprint editor - 2026-09-05, the tour panned to nothing).
_view_box = None
_view_owner = None


def _owner(app):
    try:
        return (app.activeDocument.name, app.activeProduct.productType)
    except Exception:
        return None


def _mm(v):
    # EAGLE reads bare coordinates in the editor's CURRENT grid unit (0.1 inch in a symbol
    # editor, 1 mm in a package editor); an explicit unit makes the box mean the same thing
    # in every editor.
    return f"{float(v):.3f}mm"


def forget_view():
    global _view_box, _view_owner
    _view_box = None
    _view_owner = None


def _run(app, eagle_cmd):
    return app.executeTextCommand(f"Electron.run {eagle_cmd}")


def _repaint(app):
    # The electronics editor does not PRESENT a new view until the next command arrives
    # (2026-09-05: a WINDOW box looked dropped for seconds, then appeared the moment any other
    # command came in). A bare WINDOW is EAGLE's redraw and is that next command; the
    # ImmediateRepaint text command does not do it.
    try:
        app.executeTextCommand("Electron.run WINDOW")
    except Exception:
        pass


def handle_electron_zoom(app: adsk.core.Application, args: dict) -> dict:
    """Smoothly zoom the schematic / 2D board view.

    Args:
        fit:          bool - zoom to fit the whole sheet/board (ignores factor).
        factor:       float - total zoom multiple (>1 = in, <1 = out). Default 2.0.
        steps:        int   - frames over which to ramp (smoothness). Default 16.
        frameDelayMs: int   - ms between frames. Default 35.
    """
    if args.get("fit"):
        _run(app, "WINDOW FIT")
        forget_view()
        _repaint(app)
        return {"success": True, "output": "Zoomed to fit."}

    factor = float(args.get("factor", 2.0))
    steps = max(1, int(args.get("steps", 16)))
    delay = float(args.get("frameDelayMs", 35)) / 1000.0
    per = factor ** (1.0 / steps)  # geometric per-frame step -> perceptually even
    for _ in range(steps):
        _run(app, f"WINDOW {per:.5f}")
        _repaint(app)
        _repaint(app)
        adsk.doEvents()
        time.sleep(delay)
    return {
        "success": True,
        "output": f"Zoomed {'in' if factor > 1 else 'out'} {factor}x over {steps} frames.",
    }


def handle_electron_pan(app: adsk.core.Application, args: dict) -> dict:
    """Smoothly pan + zoom the view to frame a box in mm.

    Coords are millimetres in the active editor (board, schematic, symbol or footprint),
    sent to EAGLE with an explicit mm unit. The view animates from the last box this
    add-in set in the same editor, or from `from` when given (the caller knows the fitted
    extent after WINDOW FIT); with neither it jumps.

    Args:
        x1, y1, x2, y2: required floats - the target rectangle (mm).
        from:           [x1, y1, x2, y2] - where the view is now (optional).
        steps:          int   - frames. Default 16.
        frameDelayMs:   int   - ms between frames. Default 35.
    """
    global _view_box, _view_owner
    try:
        tx1, ty1 = float(args["x1"]), float(args["y1"])
        tx2, ty2 = float(args["x2"]), float(args["y2"])
    except (KeyError, TypeError, ValueError):
        return {"success": False, "error": "Need x1,y1,x2,y2 (mm). Get part positions from fusion_board_info."}

    steps = max(1, int(args.get("steps", 16)))
    delay = max(0.03, float(args.get("frameDelayMs", 35)) / 1000.0)
    owner = _owner(app)
    if _view_owner != owner:
        _view_box = None
    start = None
    frm = args.get("from")
    if isinstance(frm, (list, tuple)) and len(frm) == 4:
        try:
            start = tuple(float(v) for v in frm)
        except (TypeError, ValueError):
            start = None
    if start is None:
        start = _view_box

    def frame(x1, y1, x2, y2):
        _run(app, f"WINDOW ({_mm(x1)} {_mm(y1)}) ({_mm(x2)} {_mm(y2)})")
        _repaint(app)

    if start is None:
        frame(tx1, ty1, tx2, ty2)
        _view_box = (tx1, ty1, tx2, ty2)
        _view_owner = owner
        return {"success": True, "output": "Framed region (no animation - first view set; later pans animate, or pass from:[x1,y1,x2,y2]).",
                "data": {"box": _view_box}}

    sx1, sy1, sx2, sy2 = start
    for i in range(1, steps + 1):
        t = i / steps
        # ease in and out so the glide reads as a hand on the wheel, not a slide show
        t = t * t * (3 - 2 * t)
        frame(sx1 + (tx1 - sx1) * t, sy1 + (ty1 - sy1) * t, sx2 + (tx2 - sx2) * t, sy2 + (ty2 - sy2) * t)
        adsk.doEvents()
        time.sleep(delay)
    # the settle frame: the package editor drops WINDOW commands that arrive while it is still
    # laying out the previous one (2026-09-05: a glide could end one gesture behind), so the
    # target is issued once more after the editor has had a moment
    time.sleep(0.3)
    adsk.doEvents()
    frame(tx1, ty1, tx2, ty2)
    adsk.doEvents()
    _view_box = (tx1, ty1, tx2, ty2)
    _view_owner = owner
    return {"success": True,
            "output": f"Panned to ({tx1:.1f},{ty1:.1f})-({tx2:.1f},{ty2:.1f}) over {steps} frames.",
            "data": {"box": _view_box, "from": list(start)}}

def handle_electron_select(app: adsk.core.Application, args: dict) -> dict:
    """Select a part (by reference designator, or by x/y board coords) and read its
    properties - the way clicking a part in the editor reveals the properties panel.

    Args:
        name (or component): str - reference designator (e.g. "R1"). Cleanest.
        x, y:                floats - board-mm point to click/select (alternative).
        properties:          bool - also return the selected device's info. Default true.
    """
    name = args.get("name") or args.get("component")
    try:
        if name:
            sel = app.executeTextCommand(f"Electron.SelectbyName {name}")
        elif "x" in args and "y" in args:
            app.executeTextCommand(f"Electron.LeftButtonDown {args['x']} {args['y']}")
            sel = app.executeTextCommand(f"Electron.Select {args['x']} {args['y']}")
        else:
            return {"success": False, "error": "Provide {name} (reference designator) or {x,y} (board mm)."}
    except Exception as e:
        return {"success": False, "error": f"Select failed: {e}"}

    _repaint(app)
    info = ""
    if args.get("properties", True):
        try:
            info = app.executeTextCommand("Electron.sch_get_deviceinfo")
        except Exception:
            pass
    return {
        "success": True,
        "output": f"Selected {name or (str(args.get('x')) + ',' + str(args.get('y')))}.",
        "data": {"selection": str(sel)[:300], "deviceInfo": str(info)[:600]},
    }
