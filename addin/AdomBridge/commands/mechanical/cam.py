"""CAM: milling jobs from Fusion's Manufacture workspace, driven as data.

cam_tools lists Fusion's tool libraries and their tools. cam_job builds a setup (stock and work coordinate system) and its
operations from a spec, generates the toolpaths, reports every operation's toolpath status, warnings and errors and the
machining time, and posts G-code. cam_inspect reads a document's CAM back.

A document has no CAM product until the Manufacture workspace has been opened in it once. Products has no way to add one
(only item and itemByProductType, which raises or returns null), so the first job in a document opens Manufacture for the
moment it takes Fusion to create the product, puts the user's workspace back even when that fails, and says so in its
reply. Every later job, cam_inspect and cam_tools leave the workspace alone."""
import os
from pathlib import Path
import re
import threading
import time

import adsk.core
import adsk.fusion
try:
    import adsk.cam  # always present inside Fusion; offline tools load this package without it
except ImportError:
    pass

from .core import active, require, resolve, target

# names here -> Fusion's strategy ids (setup.operations.compatibleStrategies)
STRATEGIES = {"face": "face", "adaptive2d": "adaptive2d", "pocket2d": "pocket2d", "contour2d": "contour2d", "drill": "drill",
              "adaptive": "adaptive", "pocket_clearing": "pocket_clearing", "contour": "contour3d", "parallel": "parallel",
              "scallop": "scallop"}
CONTOUR_2D = ("contour2d",)
POCKET_2D = ("adaptive2d", "pocket2d")
# Fusion's cycleType values, as its hole templates and posts name them (FANUC: G81, G83 with Q, G73 with Q)
DRILL_CYCLES = {"drill": "drilling", "peck": "deep-drilling", "chip_break": "chip-breaking"}
PECK_PARAMETERS = ("peckingDepth", "incrementalDepth")
LOOPS = {"outside": "OnlyOutsideLoops", "inside": "OnlyInsideLoops", "all": "AllLoops"}
LIBRARY_ALIASES = {"milling": "Milling Tools (Metric)", "hole": "Hole Making Tools (Metric)"}
HOLE_TOOLS = ("drill", "spot drill", "center drill", "reamer", "tap right hand", "tap left hand", "boring bar", "counter bore", "counter sink")
STOCK_MODES = {"relative_box": "RelativeBoxStock", "fixed_box": "FixedBoxStock", "solid": "SolidStock"}
STOCK_MODE_NAMES = {0: "fixed_box", 1: "relative_box", 2: "fixed_cylinder", 3: "relative_cylinder", 4: "fixed_tube",
                    5: "relative_tube", 6: "solid", 7: "previous_setup"}
# wcs.origin -> Fusion's wcs_origin_mode choice, and the parameter holding the box point for it
WCS_ORIGINS = {"stock_point": ("stockPoint", "wcs_origin_boxPoint"), "model_point": ("modelPoint", "wcs_origin_boxPoint"),
               "model_origin": ("modelOrigin", None)}
OPERATION_STATES = {0: "valid", 1: "invalid", 2: "suppressed", 3: "no_toolpath"}
WCS_MATRIX_TO_MM = 1.0   # Setup.workCoordinateSystem comes in mm, unlike the API's usual cm
RAPID_MM_PER_MIN = 5000.0
TOOL_CHANGE_SECONDS = 15.0
FEED_SCALE_PERCENT = 100.0
DEFAULT_NC_FOLDER = Path(os.path.expanduser("~")) / "Documents" / "Fusion 360" / "NC Programs"


def mm(value):
    """A length expression Fusion reads in millimetres, whatever the document's units."""
    return "%r mm" % float(value)


def text_lines(message):
    return [line.strip() for line in str(message or "").splitlines() if line.strip()]


def set_parameter(parameters, name, expression, check=True):
    """Set a CAM parameter by expression and refuse a value Fusion flags (its error, not a silent default). An input's
    parameters are not evaluated until Fusion builds the operation, so check=False there and parameter_problems after."""
    parameter = parameters.itemByName(name)
    require(parameter is not None, "unknown_parameter", "Fusion has no parameter '%s' here." % name)
    parameter.expression = expression
    if check:
        problem = parameter_problems(parameters, [name])
        require(not problem, "invalid_parameter_value", "Fusion refused %s = %s: %s" % (name, expression, "; ".join(problem)))
    return parameter


def parameter_problems(parameters, names):
    out = []
    for name in names:
        parameter = parameters.itemByName(name)
        problem = str(getattr(parameter, "error", "") or "").strip() if parameter is not None else ""
        if problem:
            out.append("%s: %s" % (name, problem))
    return out


def set_choice(parameters, name, value):
    """Set a choice parameter, checking the value against the choices Fusion offers for it."""
    parameter = parameters.itemByName(name)
    require(parameter is not None, "unknown_parameter", "Fusion has no parameter '%s' here." % name)
    try:
        choices = parameter.value.getChoices()
        # Fusion lists the values quoted, as expressions: "'stockPoint'"
        values = [str(v).strip("'") for v in choices[2]] if isinstance(choices, (list, tuple)) and len(choices) >= 3 else None
    except Exception:
        values = None
    require(values is None or value in values, "unknown_choice", "%s takes %s, not '%s'." % (name, ", ".join(values or []), value))
    return set_parameter(parameters, name, "'%s'" % value)


def machining_time(cam, what):
    """Fusion's estimate, asked in the units its reference gives (feed scale in %, rapid feed in cm/s, so 5000 mm/min
    goes in as 8.33). Fusion 2705 ignores both and times rapids at its own rate; machining_data reports what it used."""
    return cam.getMachiningTime(what, FEED_SCALE_PERCENT, RAPID_MM_PER_MIN / 600.0, TOOL_CHANGE_SECONDS)


def machining_data(timing):
    out = {"seconds": round(timing.machiningTime, 1), "feedSeconds": round(timing.totalFeedTime, 1), "rapidSeconds": round(timing.totalRapidTime, 1),
           "toolChangeSeconds": round(timing.totalToolChangeTime, 1), "feedDistanceMm": round(timing.feedDistance * 10, 1),
           "rapidDistanceMm": round(timing.rapidDistance * 10, 1), "toolChanges": timing.toolChangeCount}
    if timing.totalRapidTime > 0:
        out["rapidMmPerMin"] = round(timing.rapidDistance * 10 / timing.totalRapidTime * 60)
    out["basis"] = "Fusion's own estimate at the toolpaths' feeds; rapids at Fusion's own rate (rapidMmPerMin)"
    return out


# ---- the CAM product -------------------------------------------------------------------------

def existing_cam(doc):
    """The document's CAM product, or None without touching the UI (Fusion raises or returns null when there is none)."""
    if doc is None:
        return None
    try:
        product = doc.products.itemByProductType("CAMProductType")
    except RuntimeError:
        return None
    return adsk.cam.CAM.cast(product) if product is not None else None


def cam_product(app, doc):
    """(cam, workspace) for the document. workspace is None when the document already had CAM; otherwise it records the
    round trip through Manufacture that creating the product takes, and whether the user's workspace came back."""
    cam = existing_cam(doc)
    if cam is not None:
        return cam, None
    ui = app.userInterface
    before = ui.activeWorkspace.id if ui.activeWorkspace else None
    manufacture = ui.workspaces.itemById("CAMEnvironment")
    require(manufacture is not None, "cam_unavailable", "This Fusion has no Manufacture workspace.")
    try:
        opened = manufacture.activate()
        cam = existing_cam(doc)
    finally:
        back = before or "FusionSolidEnvironment"
        if ui.activeWorkspace is None or ui.activeWorkspace.id != back:
            ui.workspaces.itemById(back).activate()
    after = ui.activeWorkspace.id if ui.activeWorkspace else None
    require(cam is not None, "cam_unavailable", "Fusion did not create the document's CAM product (Manufacture %s)." % ("opened" if opened else "refused to open"))
    return cam, {"switched": True, "from": before, "to": "CAMEnvironment", "restored": after == (before or "FusionSolidEnvironment"), "now": after}


# ---- tools ---------------------------------------------------------------------------------

def tool_libraries():
    """Every library a job can take tools from, by name: Fusion's samples and the user's local libraries."""
    libraries = adsk.cam.CAMManager.get().libraryManager.toolLibraries
    found = {}
    for location, kind in ((adsk.cam.LibraryLocations.Fusion360LibraryLocation, "sample"),
                           (adsk.cam.LibraryLocations.LocalLibraryLocation, "local")):
        try:
            urls = libraries.childAssetURLs(libraries.urlByLocation(location))
        except Exception:
            continue
        for url in urls:
            name = url.toString().rstrip("/").split("/")[-1]
            if name.lower().endswith(".json"):
                name = name[:-5]
            found.setdefault(name, (url, kind))
    return found


def library_tools(name, cam=None, kind=None, cache=None):
    """(library name, tools) for a library named by alias (milling, hole), by name, or 'document' for the tools the
    document's CAM already uses; with kind, only the tools of that type, found by Fusion's own query (reading every
    tool from Python took 2.1 s on the 258-tool hole library, the query 1.3 s). Loading a library costs 1 to 3 s, so a
    job loads each once (cache)."""
    if name == "document":
        require(cam is not None, "tool_library_missing", "This document has no CAM yet, so it has no tools of its own.")
        label, library = "document", cam.documentToolLibrary
    elif cache is not None and ("library", name) in cache:
        label, library = cache[("library", name)]   # a job's tool lists are keyed ("tools", library, type)
    else:
        label = LIBRARY_ALIASES.get(name, name)
        found = tool_libraries()
        match = found.get(label) or next((v for k, v in found.items() if k.lower() == label.lower()), None)
        require(match is not None, "tool_library_missing", "No tool library named '%s'; cam_tools lists them: %s." % (name, ", ".join(sorted(found))[:600]))
        library = adsk.cam.CAMManager.get().libraryManager.toolLibraries.toolLibraryAtURL(match[0])
        if cache is not None:
            cache[("library", name)] = (label, library)
    if kind:
        try:
            query = library.createQuery()
            query.criteria.add("tool_type", adsk.core.ValueInput.createByString(kind))
            return label, [result.tool for result in query.execute()]
        except Exception:
            pass   # read every tool instead
    return label, [library.item(i) for i in range(library.count)]


def tool_data(tool):
    def value(name):
        p = tool.parameters.itemByName(name)
        return p.value.value if p is not None else None

    def length(name):
        v = value(name)
        return round(v * 10, 3) if isinstance(v, (int, float)) and not isinstance(v, bool) else None
    return {"type": value("tool_type"), "diameterMm": length("tool_diameter"), "description": value("tool_description") or "",
            "flutes": value("tool_numberOfFlutes"), "cornerRadiusMm": length("tool_cornerRadius"), "fluteLengthMm": length("tool_fluteLength"),
            "number": value("tool_number"), "productId": value("tool_productId") or "", "vendor": value("tool_vendor") or ""}


def pick_tool(spec, strategy, cam=None, cache=None):
    """(tool, data) from the named library: the requested type (and description, flutes), nearest the requested diameter."""
    kind = spec.get("type") or ("drill" if strategy == "drill" else "flat end mill")
    library = spec.get("library") or ("hole" if kind in HOLE_TOOLS else "milling")
    cache = {} if cache is None else cache
    if ("tools", library, kind) not in cache:
        name, tools = library_tools(library, cam, kind, cache)
        cache[("tools", library, kind)] = (name, [(t, tool_data(t)) for t in tools])
    name, rows = cache[("tools", library, kind)]
    rows = [(t, d) for t, d in rows if d["type"] == kind]
    if spec.get("description"):
        needle = spec["description"].lower()
        rows = [(t, d) for t, d in rows if needle in d["description"].lower()]
    if spec.get("flutes"):
        rows = [(t, d) for t, d in rows if d["flutes"] == spec["flutes"]]
    require(rows, "tool_not_found", "No '%s'%s in %s; list them with cam_tools." % (
        kind, (" like '%s'" % spec["description"]) if spec.get("description") else "", name))
    want = spec.get("diameterMm")
    if want is None:
        tool, data = rows[0]
    else:
        # nearest diameter; a tie goes to the smaller tool, which fits wherever the bigger one would
        tool, data = min(rows, key=lambda r: (abs((r[1]["diameterMm"] or 0) - want), r[1]["diameterMm"] or 0))
    return tool, dict(data, library=name)


# ---- geometry ------------------------------------------------------------------------------

def hole_faces(bodies, diameter_mm, tolerance_mm=0.1):
    """Concave cylindrical faces of the setup's bodies with the drill's diameter: the holes to drill."""
    out = []
    for body in bodies:
        for face in body.faces:
            geometry = face.geometry
            if geometry.objectType == adsk.core.Cylinder.classType() and abs(geometry.radius * 20 - diameter_mm) <= tolerance_mm:
                normal = face.evaluator.getNormalAtPoint(face.pointOnFace)[1]
                to_axis = geometry.origin.vectorTo(face.pointOnFace)
                if normal.dotProduct(to_axis) < 0:   # pointing at the axis: a hole, not a boss
                    out.append(face)
    return out


def set_peck(q, depth_mm):
    """The peck depth goes in whichever of Fusion's peck parameters the chosen cycle enables (Fusion 2705's drill has
    both peckingDepth and the older incrementalDepth)."""
    names = [n for n in PECK_PARAMETERS if q.itemByName(n) is not None]
    require(names, "unknown_parameter", "Fusion's drill has none of " + ", ".join(PECK_PARAMETERS) + ".")
    enabled = [n for n in names if getattr(q.itemByName(n), "isEnabled", True)]
    for name in enabled or names[:1]:
        set_parameter(q, name, mm(depth_mm))
    return enabled or names[:1]


def selection_problems(selections):
    out = []
    for i in range(selections.count):
        s = selections.item(i)
        for flag, attr in (("hasError", "error"), ("hasWarning", "warning")):
            try:
                if getattr(s, flag):
                    out.extend(text_lines(getattr(s, attr)))
            except Exception:
                pass
    return out


def pocket_floors(bodies):
    """Upward planar faces below the top of each setup body: the floors a 2D pocket clears when no faces are given.
    (Fusion's pocket recognition selected the pocket on a test plate but generated an empty toolpath.)"""
    out = []
    for body in bodies:
        top = body.boundingBox.maxPoint.z
        for face in body.faces:
            if face.geometry.objectType != adsk.core.Plane.classType():
                continue
            normal = face.evaluator.getNormalAtPoint(face.pointOnFace)[1]
            if normal.z > 0.999 and face.pointOnFace.z < top - 1e-4:
                out.append(face)
    return out


def apply_geometry(op, strategy, spec, design, value, bodies):
    """Select a 2D operation's geometry: the faces given, else the outside of the setup bodies' silhouette (2D contour)
    or the bodies' pocket floors (2D pocket and adaptive). Returns what was chosen, so the reply says where the toolpath
    came from."""
    faces = [resolve(design, value, ref, ("BRepFace",)) for ref in spec.get("faces") or []]
    loops = spec.get("loops", "outside")
    if strategy in CONTOUR_2D:
        parameter = op.parameters.itemByName("contours")
        require(parameter is not None, "unknown_parameter", "Fusion's 2D contour has no 'contours' parameter.")
        selections = parameter.value.getCurveSelections()
        if faces:
            for face in faces:
                s = selections.createNewFaceContourSelection()
                s.inputGeometry = [face]
                s.loopType = getattr(adsk.cam.LoopTypes, LOOPS[loops])
            chosen = {"source": "faces", "faces": len(faces), "loops": loops}
        else:
            s = selections.createNewSilhouetteSelection()
            s.inputGeometry = list(bodies)
            s.loopType = getattr(adsk.cam.LoopTypes, LOOPS[loops])
            chosen = {"source": "silhouette of the setup bodies", "loops": loops}
    else:
        require("loops" not in spec, "invalid_input", "loops applies to contour2d.")
        parameter = op.parameters.itemByName("pockets")
        require(parameter is not None, "unknown_parameter", "Fusion's %s has no 'pockets' parameter." % strategy)
        selections = parameter.value.getCurveSelections()
        source = "faces"
        if not faces:
            faces, source = pocket_floors(bodies), "pocket floors of the setup bodies (upward faces below the top)"
            require(faces, "no_pockets", "The setup's bodies have no pocket floors; pass faces for %s." % strategy)
        for face in faces:
            s = selections.createNewPocketSelection()
            s.inputGeometry = [face]
        chosen = {"source": source, "faces": len(faces)}
    parameter.value.applyCurveSelections(selections)
    problems = selection_problems(parameter.value.getCurveSelections())
    if problems:
        chosen["problems"] = problems
    return chosen


# ---- setup: stock and WCS -----------------------------------------------------------------

def check_setup_spec(spec):
    """Refuse a stock or WCS spec that does not fit together, before anything in the document changes."""
    stock = spec.get("stock") or {}
    mode = stock.get("mode")
    require(mode == "solid" or not stock.get("bodies"), "invalid_input", "stock.bodies applies to solid stock.")
    require(mode != "solid" or stock.get("bodies"), "invalid_input", "Solid stock needs stock.bodies.")
    require(mode == "fixed_box" or "sizeMm" not in stock, "invalid_input", "stock.sizeMm applies to fixed_box stock.")
    require(mode != "fixed_box" or (isinstance(stock.get("sizeMm"), list) and len(stock["sizeMm"]) == 3), "invalid_input",
            "Fixed box stock needs stock.sizeMm [x, y, z].")
    require(not stock.get("offsetMm") or mode in (None, "relative_box"), "invalid_input", "Stock offsets apply to relative_box stock.")
    wcs = spec.get("wcs") or {}
    require(wcs.get("origin") or not wcs.get("point"), "invalid_input", "wcs.point needs wcs.origin stock_point or model_point.")
    require(wcs.get("origin") != "model_origin" or not wcs.get("point"), "invalid_input", "model_origin takes no wcs.point.")


def stock_input(sinput, spec, stock_bodies):
    """The stock mode goes on the setup input through Fusion's typed API; solid stock takes its bodies."""
    mode = (spec.get("stock") or {}).get("mode")
    if mode == "solid":
        sinput.stockSolids = stock_bodies
    elif mode:
        sinput.stockMode = getattr(adsk.cam.SetupStockModes, STOCK_MODES[mode])


def configure_setup(q, spec):
    """Stock offsets or size and the WCS origin, on the new setup's parameters (on the setup input Fusion has not
    evaluated them yet: they read back as unspecified and not editable)."""
    stock = spec.get("stock") or {}
    offsets = stock.get("offsetMm")
    if offsets:
        set_choice(q, "job_stockOffsetMode", "simple")
        for side, name in (("sides", "job_stockOffsetSides"), ("top", "job_stockOffsetTop"), ("bottom", "job_stockOffsetBottom")):
            if side in offsets:
                set_parameter(q, name, mm(offsets[side]))
    if stock.get("mode") == "fixed_box":
        for axis, length in zip("XYZ", stock["sizeMm"]):
            set_parameter(q, "job_stockFixed" + axis, mm(length))
    wcs = spec.get("wcs") or {}
    if wcs.get("origin"):
        choice, point_parameter = WCS_ORIGINS[wcs["origin"]]
        set_choice(q, "wcs_origin_mode", choice)
        if point_parameter:
            set_choice(q, point_parameter, wcs.get("point", "top center"))


def setup_data(setup):
    """The stock and the WCS Fusion built, in mm and model coordinates. Fusion keeps the stock box (stockXLow ... in cm)
    relative to the WCS, and Setup.workCoordinateSystem in mm (seen on Fusion 2705 in mm and inch documents alike)."""
    q = setup.parameters

    def cm(name):
        p = q.itemByName(name)
        v = p.value.value if p is not None else None
        return v * 10 if isinstance(v, (int, float)) and not isinstance(v, bool) else None
    out = {"name": setup.name}
    try:
        out["stockMode"] = STOCK_MODE_NAMES.get(int(setup.stockMode), str(setup.stockMode))
    except Exception:
        pass
    low = [cm("stock%sLow" % a) for a in "XYZ"]
    high = [cm("stock%sHigh" % a) for a in "XYZ"]
    if None in low or None in high:
        return out
    out["stockSizeMm"] = [round(h - l, 3) for l, h in zip(low, high)]
    try:
        cell = setup.workCoordinateSystem.getCell
        origin = [cell(r, 3) * WCS_MATRIX_TO_MM for r in range(3)]
        axes = [[cell(r, c) for r in range(3)] for c in range(3)]   # the WCS x, y and z axes in model space
    except Exception:
        out["stockBoxWcsMm"] = {a: [round(l, 3), round(h, 3)] for a, l, h in zip("xyz", low, high)}
        return out
    corners = [[(high if (i >> k) & 1 else low)[k] for k in range(3)] for i in range(8)]
    model = [[origin[r] + sum(axes[k][r] * c[k] for k in range(3)) for r in range(3)] for c in corners]
    out["stockBoxMm"] = {a: [round(min(p[r] for p in model), 3), round(max(p[r] for p in model), 3)] for r, a in enumerate("xyz")}
    out["wcs"] = {"originMm": [round(v, 3) for v in origin], "xAxis": [round(v, 6) for v in axes[0]], "zAxis": [round(v, 6) for v in axes[2]]}
    return out


# ---- operations --------------------------------------------------------------------------

def op_status(op, cam=None):
    """An operation's toolpath as it stands: status is generated (a valid toolpath), empty (generated to nothing), error,
    outdated (a toolpath the model has since invalidated), suppressed, or not_generated."""
    warnings = text_lines(op.warning) if op.hasWarning else []
    errors = text_lines(op.error) if op.hasError else []
    has, valid = bool(op.hasToolpath), bool(op.isToolpathValid)
    suppressed = bool(getattr(op, "isSuppressed", False))
    if errors:
        status = "error"
    elif suppressed:
        status = "suppressed"
    elif has and valid:
        status = "generated"
    elif has:
        status = "outdated"
    elif any("empty" in w.lower() for w in warnings):
        status = "empty"
    else:
        status = "not_generated"
    out = {"name": op.name, "strategy": op.strategy, "status": status, "hasToolpath": has, "valid": valid,
           "warnings": warnings, "errors": errors}
    try:
        out["state"] = OPERATION_STATES.get(int(op.operationState), str(op.operationState))
    except Exception:
        pass
    if cam is not None and status == "generated":
        try:
            out["machiningSeconds"] = round(machining_time(cam, op).machiningTime, 1)
        except Exception:
            pass
    return out


def cam_tools(app, args):
    library = args.get("library", "milling")
    name, tools = library_tools(library, existing_cam(app.activeDocument) if library == "document" else None, args.get("type"))
    rows = [tool_data(t) for t in tools]
    if args.get("type"):
        rows = [t for t in rows if t["type"] == args["type"]]
    if args.get("description"):
        rows = [t for t in rows if args["description"].lower() in t["description"].lower()]
    if args.get("minDiameterMm") is not None:
        rows = [t for t in rows if (t["diameterMm"] or 0) >= args["minDiameterMm"]]
    if args.get("maxDiameterMm") is not None:
        rows = [t for t in rows if (t["diameterMm"] or 0) <= args["maxDiameterMm"]]
    out = {"library": name, "count": len(rows), "tools": rows[: int(args.get("limit", 100))],
           "libraries": [{"name": k, "location": v[1]} for k, v in sorted(tool_libraries().items())]}
    if not args.get("type"):
        out["types"] = sorted({t["type"] for t in rows if t["type"]})
    return out


def cam_job(app, args):
    require(threading.current_thread() is threading.main_thread(), "wrong_thread", "CAM requires the main thread.")
    doc, design, value = target(app, args, True, model_write=False)
    bodies = [resolve(design, value, ref, ("BRepBody",)) for ref in args["bodies"]]
    require(bodies, "invalid_selection", "A job needs at least one body.")
    check_setup_spec(args)
    stock_bodies = [resolve(design, value, ref, ("BRepBody",)) for ref in (args.get("stock") or {}).get("bodies") or []]
    for spec in args["operations"]:
        strategy = spec["strategy"]
        require(strategy in STRATEGIES, "unsupported_strategy", "Strategies: " + ", ".join(STRATEGIES))
        require(strategy in CONTOUR_2D + POCKET_2D or not spec.get("faces"), "invalid_input", "faces apply to contour2d, pocket2d and adaptive2d.")
        require(strategy in CONTOUR_2D or "loops" not in spec, "invalid_input", "loops applies to contour2d.")
        require(strategy == "drill" or not ({"cycle", "peckDepthMm", "holeFaces"} & set(spec)), "invalid_input", "cycle, peckDepthMm and holeFaces apply to drill.")
        require(spec.get("cycle", "drill") != "drill" or spec.get("peckDepthMm") is None, "invalid_input", "peckDepthMm needs cycle peck or chip_break.")
    cam, workspace = cam_product(app, doc)
    tools_before = cam.documentToolLibrary.count
    sinput = cam.setups.createInput(adsk.cam.OperationTypes.MillingOperation)
    sinput.models = bodies
    stock_input(sinput, args, stock_bodies)
    setup = cam.setups.add(sinput)
    try:
        if args.get("name"):
            setup.name = args["name"]
        configure_setup(setup.parameters, args)
        result = build_job(app, cam, setup, design, value, bodies, args)
    except Exception:
        # never leave a half-made setup behind: a failed job changes nothing
        try:
            setup.deleteMe()
        except Exception:
            pass
        forget_tools(cam, tools_before)
        raise
    if workspace:
        result["workspace"] = workspace
    return result


def generate(cam, what, deadline):
    """Generate the toolpaths of a setup or one operation and wait; returns how many operations Fusion said it would do."""
    future = cam.generateToolpath(what)
    while not future.isGenerationCompleted:
        require(time.perf_counter() < deadline, "generate_timeout", "Toolpaths did not finish in time; simplify the job or raise timeoutSeconds.")
        adsk.doEvents()
        time.sleep(0.25)
    try:
        return future.numberOfOperations
    except Exception:
        return None


def forget_tools(cam, count):
    """The tools a failed job brought into the document's tool library go too (Fusion appends them as operations use
    them, and deleting the setup leaves them behind)."""
    try:
        library = cam.documentToolLibrary
        for index in range(library.count - 1, count - 1, -1):
            library.remove(index)
    except Exception:
        pass


def build_job(app, cam, setup, design, value, bodies, args):
    created, notes, cache = [], [], {}
    for spec in args["operations"]:
        strategy = spec["strategy"]
        tool, tool_info = pick_tool(spec.get("tool") or {}, strategy, cam, cache)
        oinput = setup.operations.createInput(STRATEGIES[strategy])
        oinput.tool = tool
        overrides = spec.get("parameters") or {}
        for name, expression in overrides.items():
            set_parameter(oinput.parameters, name, str(expression), check=False)
        op = setup.operations.add(oinput)
        problems = parameter_problems(op.parameters, overrides)
        require(not problems, "invalid_parameter_value", "Fusion refused the parameters of %s: %s" % (op.name, "; ".join(problems)))
        if spec.get("name"):
            try:
                op.name = spec["name"]
            except Exception as error:
                # names are unique in the document; keep Fusion's and say so rather than fail the job
                notes.append("%s: Fusion kept its own name, not '%s' (%s)" % (op.name, spec["name"], str(error).strip()[:120]))
        row = {"name": op.name, "strategy": strategy, "tool": tool_info}
        if strategy == "drill":
            dia = tool_info["diameterMm"]
            faces = [resolve(design, value, ref, ("BRepFace",)) for ref in spec["holeFaces"]] if spec.get("holeFaces") else hole_faces(bodies, dia)
            require(faces, "no_holes", "No %.2f mm holes on the setup's bodies; pass holeFaces or another drill diameter." % dia)
            op.parameters.itemByName("holeFaces").value.value = faces
            cycle = spec.get("cycle", "drill")
            set_choice(op.parameters, "cycleType", DRILL_CYCLES[cycle])
            row["geometry"] = {"source": "holeFaces" if spec.get("holeFaces") else "holes of the drill's diameter", "holes": len(faces)}
            row["cycle"] = cycle
            if spec.get("peckDepthMm") is not None:
                row["peck"] = {"depthMm": spec["peckDepthMm"], "parameters": set_peck(op.parameters, spec["peckDepthMm"])}
            notes.append("%s: %d holes of %.2f mm, %s cycle" % (op.name, len(faces), dia, cycle))
        elif strategy in CONTOUR_2D or strategy in POCKET_2D:
            row["geometry"] = apply_geometry(op, strategy, spec, design, value, bodies)
        created.append(row)
    started = time.perf_counter()
    deadline = started + float(args.get("timeoutSeconds", 900))
    covered = generate(cam, setup, deadline)
    ops = [setup.operations.item(i) for i in range(setup.operations.count)]
    # an operation Fusion left untouched (no toolpath, no warning, no error) gets one more try on its own
    retried = [op for op in ops if op_status(op)["status"] == "not_generated"]
    for op in retried:
        generate(cam, op, deadline)
    generated = round(time.perf_counter() - started, 1)
    toolpaths = [op_status(op, cam) for op in ops]
    for row, path in zip(created, toolpaths):
        row.update({k: path[k] for k in ("status", "warnings", "errors") if k in path})
        if "machiningSeconds" in path:
            row["machiningSeconds"] = path["machiningSeconds"]
    timing = machining_time(cam, setup)
    done = sum(1 for p in toolpaths if p["status"] == "generated")
    result = {"target": value, "setup": setup_data(setup), "operations": created, "toolpaths": toolpaths,
              "generated": {"operations": len(toolpaths), "withToolpath": done, "complete": done == len(toolpaths), "coveredByFusion": covered,
                            "retried": [op.name for op in retried]},
              "generateSeconds": generated, "notes": notes,
              "machining": machining_data(timing)}
    post = args.get("post")
    if post:
        missing = [p["name"] for p in toolpaths if p["status"] != "generated"]
        if missing:
            # Fusion's post leaves an operation without a valid toolpath out of the program without a word
            result["post"] = {"posted": False, "reason": "operations without a valid toolpath: " + ", ".join(missing),
                              "hint": "Fix or remove them and run the job again; Fusion's post would leave them out of the program silently."}
        else:
            result["post"] = post_program(cam, setup, post)
    return result


# ---- posting -------------------------------------------------------------------------------

def find_post(name):
    library = adsk.cam.CAMManager.get().libraryManager.postLibrary
    configs = list(library.createQuery(adsk.cam.LibraryLocations.Fusion360LibraryLocation).execute())
    config = next((c for c in configs if c.description.lower() == name.lower()), None) or \
        next((c for c in configs if name.lower() in c.description.lower()), None)
    require(config is not None, "post_not_found", "No post named like '%s' in Fusion's library (try FANUC, Grbl, HAAS - Next Generation Control)." % name)
    return config


def set_string(parameters, name, text):
    parameter = parameters.itemByName(name)
    require(parameter is not None, "unknown_parameter", "Fusion has no parameter '%s' here." % name)
    parameter.value.value = text


def read_value(parameters, name):
    parameter = parameters.itemByName(name)
    return parameter.value.value if parameter is not None else None


def post_program(cam, setup, spec):
    """Post the setup through an NC program with a post from Fusion's library, matched by name ("FANUC", "Grbl",
    "HAAS - Next Generation Control"), into spec.folder under the program name or number.

    The program name and comment: when an NC program's operations come from one setup, Fusion copies the setup's
    job_programName and job_programComment into nc_program_name and nc_program_comment (NCProgramInput.operations), so a
    name set before the operations was overwritten with the setup's default, 1001. They are set on the setup, and on the
    NC program after its operations and again after its post. The folder goes in with forward slashes, as Fusion's
    samples write it."""
    folder = Path(spec["folder"])
    require(folder.is_absolute(), "invalid_destination", "post.folder must be an absolute path.")
    config = find_post(spec.get("machine", "FANUC"))
    program = str(spec.get("program", "1001"))
    numeric = any(word in config.description.lower() for word in ("fanuc", "haas"))
    require(not numeric or (program.isdigit() and 0 < int(program) <= 9999), "invalid_program",
            "The %s post takes a program number from 1 to 9999, not '%s'." % (config.description, program))
    comment = spec.get("comment")
    folder.mkdir(parents=True, exist_ok=True)
    set_string(setup.parameters, "job_programName", program)
    if comment is not None:
        set_string(setup.parameters, "job_programComment", comment)

    def apply(q):
        set_string(q, "nc_program_filename", program)
        set_string(q, "nc_program_name", program)
        if comment is not None:
            set_string(q, "nc_program_comment", comment)
        set_string(q, "nc_program_output_folder", folder.as_posix())
        q.itemByName("nc_program_openInEditor").value.value = False
    ninput = cam.ncPrograms.createInput()
    ninput.displayName = setup.name + " NC"
    ninput.operations = [setup]
    apply(ninput.parameters)
    nc = cam.ncPrograms.add(ninput)
    try:
        return post_through(nc, config, apply, folder, program)
    except Exception:
        try:
            nc.deleteMe()   # a failed post leaves no NC program behind either
        except Exception:
            pass
        raise


def post_through(nc, config, apply, folder, program):
    nc.postConfiguration = config
    apply(nc.parameters)
    kept = {"program": read_value(nc.parameters, "nc_program_name"), "folder": read_value(nc.parameters, "nc_program_output_folder")}
    # the program asked for replaces a file of the same name: remove it first, so the file found afterwards is this post's
    existing = folder / ("%s.%s" % (program, getattr(config, "extension", "") or "nc"))
    replaced = existing.exists()
    if replaced:
        existing.unlink()
    started = time.time()
    require(nc.postProcess(adsk.cam.NCProgramPostProcessOptions.create()), "post_failed", "Fusion's post processor reported a failure.")
    found = newest(folder, program, started)
    moved = False
    if found is None:
        found = newest(DEFAULT_NC_FOLDER, program, started)
        require(found is not None, "post_output_missing", "The post ran but its file is neither in post.folder nor in Fusion's NC folder.")
        destination = folder / found.name
        destination.write_bytes(found.read_bytes())
        found.unlink()
        found, moved = destination, True
    text = found.read_text(errors="replace")
    head = text.splitlines()[:6]
    numbers = [m.group(1) for m in (re.match(r"O(\d+)", line) for line in head) if m]
    out = {"posted": True, "machine": config.description, "ncProgram": nc.name, "file": str(found), "bytes": found.stat().st_size,
           "lines": text.count("\n"), "head": head, "program": program, "ncProgramKept": kept,
           "programInHeader": (int(numbers[0]) == int(program)) if numbers and program.isdigit() else any(program in line for line in head[:4])}
    try:
        out["operations"] = [o.name for o in nc.filteredOperations]
    except Exception:
        pass
    if moved:
        out["movedFromFusionFolder"] = True
    if replaced:
        out["replacedExisting"] = True
    return out


def newest(folder, program, since):
    if not folder.exists():
        return None
    files = [p for p in folder.glob(program + ".*") if p.stat().st_mtime >= since - 5]
    return max(files, key=lambda p: p.stat().st_mtime) if files else None


# ---- inspection ----------------------------------------------------------------------------

def cam_inspect(app, args):
    doc, design, value = active(app)
    cam = existing_cam(doc)
    if cam is None:
        return {"target": value, "setups": [], "ncPrograms": [], "note": "This document has no CAM yet; cam_job creates it."}
    setups = []
    for i in range(cam.setups.count):
        setup = cam.setups.item(i)
        row = setup_data(setup)
        row["operations"] = []
        for j in range(setup.operations.count):
            op = setup.operations.item(j)
            status = op_status(op, cam)
            try:
                status["tool"] = tool_data(op.tool) if op.tool is not None else None
            except Exception:
                pass
            row["operations"].append(status)
        setups.append(row)
    programs = []
    for i in range(cam.ncPrograms.count):
        nc = cam.ncPrograms.item(i)
        try:
            post = nc.postConfiguration.description if nc.postConfiguration else None
        except Exception:
            post = None
        programs.append({"name": nc.name, "post": post, "program": read_value(nc.parameters, "nc_program_name"),
                         "filename": read_value(nc.parameters, "nc_program_filename"), "folder": read_value(nc.parameters, "nc_program_output_folder")})
    return {"target": value, "setups": setups, "ncPrograms": programs}


OPERATIONS = {"cam_tools": cam_tools, "cam_job": cam_job, "cam_inspect": cam_inspect}
WRITES = {"cam_job"}
