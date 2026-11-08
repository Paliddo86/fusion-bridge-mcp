"""2D drawings with Fusion's automatic drawing API (adsk.drawing.DrawingManager, a July 2026 preview).

drawing_create makes a drawing of the active, saved cloud design in one journaled call: Fusion lays
out the sheets itself, with a base view and its projected views, an optional isometric view, a
filled title block and automatic dimensions. The API cannot place a view or set a view's scale; the
layout picks the scale for the chosen sheet size. The drawing is a new cloud file (.f2d) in the
design's folder, or in folderId.

drawing_export writes PDF, DWG and DXF of a drawing, and drawing_status says whether one is ready.

Measured on Fusion 2705 (2026-11-03): createDrawing took 24.7 to 33.6 s for a small part (three
runs); it works out of process (a drawing engine, AcCoreConsole.exe, lays out, dimensions and saves,
then quits) and does not open the drawing. Moving the new file to another folder took 0.6 s and
kept its file id.

HARD RULES (both found live, each one blocked Fusion's main thread for every caller):
1. Never export in the call that opens a drawing. A drawing a script opens only finishes loading
   after that script returns, and opening an automatically created drawing starts the interactive
   Edit Auto Dimensions command (AUTODIMENSIONEDIT) in the drawing engine. An export sent while that
   command waits for input is fed to its prompt and rejected ("Invalid selection"), and
   DrawingExportManager.execute holds the main thread until its 600 s timeout ("3 : export timed
   out"); PDF, DWG and DXF each waited the full 600 s.
2. Never open a drawing invisibly. Documents.open(file, False) opened the drawing in 0.8 s by
   Fusion's log, but no drawing engine started for it and the call did not return.
So drawing_export opens visibly and answers drawing_not_ready (retryable); a later call exports only
once the sheets read, Fusion runs no command, the drawing engine that loaded the drawing has no
unfinished command and its log has been quiet for a few seconds. (View.type is no readiness signal:
it raised "pAsset" for every view of a fully opened, idle drawing.)
"""
import math
import os
from pathlib import Path
import re
import tempfile
import threading
import time

import adsk.core
import adsk.fusion
try:
    import adsk.drawing  # always present inside Fusion 2705+; offline tests install a stand-in
except ImportError:
    pass

from .core import MechanicalError, require, target
from .exporting import publish_staged

STANDARDS = {"iso": "ISODrawingStandardType", "asme": "ASMEDrawingStandardType"}
UNITS = {"mm": "MillimeterDrawingUnitType", "in": "InchDrawingUnitType"}
ISO_SIZES = {"A4": "A4ISOSheetSize", "A3": "A3ISOSheetSize", "A2": "A2ISOSheetSize", "A1": "A1ISOSheetSize", "A0": "A0ISOSheetSize"}
ASME_SIZES = {"A": "AASMESheetSize", "B": "BASMESheetSize", "C": "CASMESheetSize", "D": "DASMESheetSize", "E": "EASMESheetSize"}
DEFAULTS = {"iso": {"units": "mm", "sheetSize": "A3"}, "asme": {"units": "in", "sheetSize": "B"}}
ORIENTATIONS = {"landscape": "LandscapeSheetOrientationType", "portrait": "PortraitSheetOrientationType"}
DIMENSIONS = {"overall": "OverallDimensionStrategyType", "automatic": "AutomaticDimensionStrategyType",
              "baseline": "BaselineDimensionStrategyType", "chain": "ChainDimensionStrategyType",
              "ordinate": "OrdinateDimensionStrategyType", "symmetric": "SymmetricDimensionStrategyType",
              "symmetric_baseline": "SymmetricWithBaselineDimensionStrategyType",
              "symmetric_ordinate": "SymmetricWithOrdinateDimensionStrategyType"}
HOLE_NOTES = {"hole_and_thread": "HoleAndThreadNoteHolePreferencesType", "hole": "HoleNoteOnlyHolePreferencesType",
              "thread": "ThreadNoteOnlyHolePreferencesType", "none": "NoHoleAnnotationsHolePreferencesType"}
DATUMS = {"model_origin": "ModelOriginDefaultOriginType", "bottom_left": "BottomLeftDefaultOriginType",
          "bottom_right": "BottomRightDefaultOriginType", "top_left": "TopLeftDefaultOriginType",
          "top_right": "TopRightDefaultOriginType"}
STYLES = {"visible": "VisibleEdgesDrawingViewStyleType", "visible_and_hidden": "VisibleAndHiddenEdgesDrawingViewStyleType",
          "shaded_and_hidden": "ShadedAndHiddenEdgesDrawingViewStyleType", "shaded": "ShadedVisibleEdgesDrawingViewStyleType"}
TANGENT_EDGES = {"off": "OffTangentEdgeDisplayType", "full": "FullLengthTangentEdgeDisplayType",
                 "shortened": "ShortenedTangentEdgeDisplayType"}
CONTENT = {"full": "FullAssemblyDrawingContentType", "visible": "VisibleOnlyDrawingContentType"}
LEVELS = {"first": "FirstLevelOnlySheetCreationType", "all": "AllLevelsSheetCreationType"}
DWG_FORMATS = {"autocad": "AutoCADDWGFormat", "simplified": "SimplifiedDWGFormat"}
VIEW_TYPES = {0: "base", 1: "projected", 2: "section", 3: "detail", 4: "auxiliary", 5: "flat_pattern", 6: "unknown"}
EXTENSIONS = {"pdf": ".pdf", "dwg": ".dwg", "dxf": ".dxf"}
IDLE_COMMANDS = ("", "SelectCommand")
AUTO_DIMENSION_EDIT = "AUTODIMENSIONEDIT"
# the same command as Fusion's UI names it; only this command is ever ended, never one a person started
AUTO_DIMENSION_UI_COMMAND = "FusionDrawingAutoDimensionEditCommand"

# file id -> {"at": time.time() when a call here opened that drawing (its engine log is read from then on),
#             "previous": the document that was active then, active again when the drawing closes,
#             "before": creationIds of the documents open then: opening a drawing also opens the design it
#             references, and close closes that design only if it was not open before}
_opened = {}


def _enum(cls_name, member):
    return getattr(getattr(adsk.drawing, cls_name), member)


def _main_thread():
    require(threading.current_thread() is threading.main_thread(), "wrong_thread", "Drawings require Fusion's main thread.")


def _drawing_api():
    manager = getattr(getattr(adsk, "drawing", None), "DrawingManager", None)
    require(manager is not None, "unsupported_capability",
            "This Fusion has no automatic drawing API (adsk.drawing.DrawingManager arrived in July 2026); update Fusion.")
    dm = manager.get()
    require(dm is not None, "unsupported_capability", "Fusion returned no DrawingManager.")
    return dm


# ---------------------------------------------------------------- drawing_create

def resolve_options(args):
    """The caller's options with defaults filled in; refuses combinations Fusion would silently ignore."""
    standard = args.get("standard", "iso")
    defaults = DEFAULTS[standard]
    size = args.get("sheetSize", defaults["sheetSize"])
    sizes = ISO_SIZES if standard == "iso" else ASME_SIZES
    require(size in sizes, "sheet_size_mismatch",
            "Sheet %s is not a%s size; use %s." % (size, "n ISO" if standard == "iso" else "n ASME", ", ".join(sizes)),
            standard=standard, sheetSize=size)
    orientation = args.get("orientation", "landscape")
    require(not (orientation == "portrait" and size in ("A0", "E")), "invalid_input",
            "Fusion has no portrait %s sheet." % size)
    views = args.get("views") or {}
    sheets = args.get("sheets") or {}
    omit = args.get("omit") or {}
    return {"standard": standard, "units": args.get("units", defaults["units"]), "sheetSize": size, "orientation": orientation,
            "orthographic": views.get("orthographic", True), "isometric": views.get("isometric", True),
            "style": args.get("style", "visible"), "tangentEdges": args.get("tangentEdges", "off"),
            "dimensions": args.get("dimensions", "overall"), "holeNotes": args.get("holeNotes", "hole_and_thread"),
            "datum": args.get("datum", "model_origin"), "content": args.get("content", "full"),
            "levels": sheets.get("levels", "all"), "assembly": sheets.get("assembly", True),
            "subassemblies": sheets.get("subassemblies", True), "components": sheets.get("components", True),
            "sheetMetal": sheets.get("sheetMetal", True), "partsList": sheets.get("partsList", True),
            "omitKeywords": omit.get("keywords", "Bolt,Screw,Nut,Washer"), "omitFasteners": omit.get("fasteners", False),
            "configuration": args.get("configuration", "")}


def apply_options(inp, o):
    """Copy resolved options onto a CreateDrawingInput (automatic mode)."""
    inp.standard = _enum("DrawingStandardTypes", STANDARDS[o["standard"]])
    inp.units = _enum("DrawingUnitTypes", UNITS[o["units"]])
    inp.sheetSize = _enum("SheetSizes", (ISO_SIZES if o["standard"] == "iso" else ASME_SIZES)[o["sheetSize"]])
    inp.orientationType = _enum("SheetOrientationTypes", ORIENTATIONS[o["orientation"]])
    inp.content = _enum("DrawingContentTypes", CONTENT[o["content"]])
    inp.sheetCreationType = _enum("SheetCreationTypes", LEVELS[o["levels"]])
    if o["configuration"]:
        inp.configuration = o["configuration"]
    prefs = inp.automationPreferences
    g = prefs.globalPreferences
    g.isMainAssemblySheetGenerated = bool(o["assembly"])
    g.isSubAssemblySheetGenerated = bool(o["subassemblies"])
    g.isComponentSheetGenerated = bool(o["components"])
    g.isFoldedModelSheetGenerated = bool(o["sheetMetal"])
    g.isFlatPatternSheetGenerated = bool(o["sheetMetal"])
    g.isAnimationSheetGenerated = False
    g.isAutoDimensionEnabled = o["dimensions"] != "none"
    g.omitComponentsWithKeywords = o["omitKeywords"]
    g.isDetectAndOmitFasteners = bool(o["omitFasteners"])
    for p in (prefs.componentPreferences, prefs.foldedModelPreferences):
        p.sheetViewPreferences.isOrthogonalViewAdded = bool(o["orthographic"])
        p.sheetViewPreferences.isIsometricViewAdded = bool(o["isometric"] and o["orthographic"])
    for asm in (prefs.mainAssemblyPreferences, prefs.subAssemblyPreferences):
        # the assembly's isometric sheet carries the parts list, so views.isometric leaves it alone
        asm.isoViewSheetPreferences.isSheetCreated = True
        asm.isoViewSheetPreferences.isPartsListIncluded = bool(o["partsList"])
        asm.orthogonalViewSheetPreferences.isSheetCreated = bool(o["orthographic"])
    for p in (prefs.componentPreferences, prefs.foldedModelPreferences, prefs.flatPatternPreferences,
              prefs.mainAssemblyPreferences, prefs.subAssemblyPreferences):
        dims = p.autoDimensionPreferences
        dims.isAutoDimensionEnabled = o["dimensions"] != "none"
        if o["dimensions"] != "none":
            dims.dimensionStrategyType = _enum("DimensionStrategyTypes", DIMENSIONS[o["dimensions"]])
        dims.holePreferencesType = _enum("HolePreferencesTypes", HOLE_NOTES[o["holeNotes"]])
        if hasattr(dims, "defaultOriginType"):
            dims.defaultOriginType = _enum("DefaultOriginTypes", DATUMS[o["datum"]])
        view = p.drawingViewPreferences
        view.style = _enum("DrawingViewStyleTypes", STYLES[o["style"]])
        view.tangentEdgesType = _enum("TangentEdgeDisplayTypes", TANGENT_EDGES[o["tangentEdges"]])


def drawing_create(app, args):
    _main_thread()
    doc, design, value = target(app, args, True, model_write=False)
    options = resolve_options(args)
    source = doc.dataFile
    require(source is not None, "design_not_saved",
            "Fusion draws a design from its cloud file, and this design was never saved. Save it into an "
            "'Adom AI Workspace' subfolder (fusion_save_to_cloud), then call drawing_create again.")
    require(not doc.isModified or args.get("useSavedVersion", False), "unsaved_changes",
            "The drawing would show the last saved version, not the changes made since. Save a version first, "
            "or pass useSavedVersion:true to draw the saved one.", savedVersion=source.versionNumber)
    require(source.isComplete, "design_upload_pending", "The design's last save is still uploading; call again in a few seconds.",
            retryable=True)
    destination = None
    if args.get("folderId"):
        destination = app.data.findFolderById(args["folderId"])
        require(destination is not None, "folder_not_found", "No cloud folder with that folderId.")
        require(not destination.isRoot, "refused_root_upload", "Put drawings in a subfolder, never a project's root folder.")
    else:
        require(not source.parentFolder.isRoot, "refused_root_upload",
                "The design sits in its project's root folder, where Fusion would also put the drawing. Pass folderId "
                "of a subfolder (an 'Adom AI Workspace' task folder) for the drawing.")
    dm = _drawing_api()
    inp = dm.createDrawingInput(source, _enum("DrawingCreationModes", "AutomaticDrawingCreationMode"))
    require(inp is not None, "drawing_failed", "Fusion did not create a drawing input for this design.")
    apply_options(inp, options)
    started = time.perf_counter()
    drawing = dm.createDrawing(inp)
    seconds = round(time.perf_counter() - started, 1)
    require(drawing is not None, "drawing_failed", "Fusion's automatic drawing failed for this design.", seconds=seconds)
    moved = None
    if destination is not None:
        moved = bool(drawing.move(destination))
        if not moved:
            # a failed call changes nothing: the drawing it made is deleted, not left beside the design
            removed = bool(drawing.deleteMe())
            raise MechanicalError("drawing_move_failed", "Fusion made the drawing but could not move it to folderId.",
                                  drawingDeleted=removed, seconds=seconds)
    if args.get("name"):
        drawing.name = args["name"]
    return {"target": value, "drawing": file_data(drawing), "source": file_data(source), "options": options,
            "seconds": seconds, "movedToFolder": moved,
            "next": "drawing_export with drawing.fileId; it opens the drawing in one call and exports in the next."}


def file_data(f):
    folder = f.parentFolder
    return {"fileId": f.id, "name": f.name, "version": f.versionNumber, "extension": f.fileExtension,
            "folder": folder.name if folder else None, "folderId": folder.id if folder else None,
            "project": f.parentProject.name if f.parentProject else None}


# ---------------------------------------------------------------- readiness

def find_open(app, file_id):
    for i in range(app.documents.count):
        d = app.documents.item(i)
        try:
            if d.dataFile is not None and d.dataFile.id == file_id:
                return d
        except Exception:
            continue
    return None


def engine_log_path():
    """Fusion's drawing-engine log: %LOCALAPPDATA%/Autodesk/Autodesk Fusion 360/<user>/logs/FusionDrawing.log."""
    root = Path(os.environ.get("LOCALAPPDATA", "")) / "Autodesk" / "Autodesk Fusion 360"
    logs = sorted(root.glob("*/logs/FusionDrawing.log"), key=lambda p: p.stat().st_mtime, reverse=True) if root.exists() else []
    return logs[0] if logs else None


LOG_LINE = re.compile(r"^(\d{8}T\d{6}) \w \.(\d+) (.*)$")


QUIET_SECONDS = 5


def engine_state(since, path=None, now=None):
    """What Fusion's drawing engines logged at or after `since` (epoch seconds), from FusionDrawing.log:
    per engine process, its unfinished commands, whether it loaded a document (opendoc) and whether it
    closed; plus how long the log has been quiet. None when the log cannot be found."""
    path = path or engine_log_path()
    if path is None:
        return None
    stamp = time.strftime("%Y%m%dT%H%M%S", time.localtime(since))
    engines, last = {}, None
    with open(path, encoding="utf-8", errors="replace") as f:
        for raw in f:
            m = LOG_LINE.match(raw.rstrip("\n"))
            if not m or m.group(1) < stamp:
                continue
            last = m.group(1)
            e = engines.setdefault(m.group(2), {"open": [], "loaded": False, "closed": False})
            text = m.group(3)
            if text.startswith("cmd will Start: "):
                e["open"].append(text[16:].strip())
            elif text.startswith("cmd Finished: "):
                name = text[14:].split(" Reason")[0].strip()
                if name in e["open"]:
                    e["open"].remove(name)
            elif '"action":"opendoc"' in text:
                e["loaded"] = True
            elif text.startswith("closing dwg") or text.startswith("unloading AcFusionService"):
                e["closed"] = True
    quiet = None
    if last is not None:
        quiet = round((now if now is not None else time.time()) - time.mktime(time.strptime(last, "%Y%m%dT%H%M%S")), 1)
    live = {pid: e for pid, e in engines.items() if not e["closed"]}
    return {"loaded": any(e["loaded"] for e in live.values()),
            "unfinished": {pid: e["open"] for pid, e in live.items() if e["open"]}, "quietSeconds": quiet}


PAPER = {v: k for k, v in {**ISO_SIZES, **ASME_SIZES, "custom": "CustomSizeSheetSize"}.items()}


def paper_name(sheet):
    for member, name in PAPER.items():
        if getattr(adsk.drawing.SheetSizes, member, None) == sheet.sheetSize:
            return name
    return str(sheet.sheetSize)


def view_data(view):
    """A view's type and curve count. Fusion 2705's View.type raises "InternalValidationError: pAsset"
    on drawings it opened (its log: no base model asset in the drawing document), so a view that
    cannot be read is reported as such and is not taken as a sign that the drawing is still loading."""
    try:
        return {"type": VIEW_TYPES.get(view.type, str(view.type)), "curves": view.viewCurves.count}
    except Exception as error:
        return {"type": None, "unreadable": str(error)[:80]}


def sheet_data(drawing):
    """Sheets and their views. Width and height are Fusion's own numbers (mm on an mm drawing: 420 x 297
    for A3 landscape, read live); units is the drawing's unit setting."""
    units = "mm" if drawing.documentSettings.units == adsk.drawing.DrawingUnitTypes.MillimeterDrawingUnitType else "in"
    out = []
    for i in range(drawing.sheets.count):
        sheet = drawing.sheets.item(i)
        views = [view_data(sheet.views.item(j)) for j in range(sheet.views.count)]
        out.append({"index": i + 1, "name": sheet.name, "paper": paper_name(sheet), "width": sheet.width, "height": sheet.height,
                    "units": units, "views": views})
    return out


def readiness(app, doc, file_id):
    """(ready, detail) for a drawing this session opened, without changing anything. Ready means its
    sheets read, Fusion runs no command, a drawing engine loaded the drawing since it was opened
    (opendoc in the engine's log) and has no unfinished command, and the engine log has been quiet for
    QUIET_SECONDS: in the live run Edit Auto Dimensions started in the same second the open finished,
    so a single instant is not enough."""
    detail = {"open": True, "active": doc.isActive, "visible": doc.isVisible,
              "activeCommand": app.userInterface.activeCommand}
    try:
        drawing = adsk.drawing.DrawingDocument.cast(doc).drawing
        detail["sheets"] = sheet_data(drawing)
        detail["loaded"] = True
    except Exception as error:
        detail["loaded"], detail["loadError"] = False, str(error)[:160]
    engine = engine_state(_opened[file_id]["at"])
    detail["engine"] = engine
    detail["ready"] = bool(detail["loaded"] and detail["activeCommand"] in IDLE_COMMANDS and engine is not None
                           and engine["loaded"] and not engine["unfinished"]
                           and engine["quietSeconds"] is not None and engine["quietSeconds"] >= QUIET_SECONDS)
    return detail["ready"], detail


def drawing_status(app, args):
    _main_thread()
    file_id = args["drawing"]["fileId"]
    doc = find_open(app, file_id)
    if doc is None:
        return {"drawing": {"fileId": file_id}, "open": False, "ready": False,
                "next": "drawing_export opens it; the export follows in a later call."}
    if file_id not in _opened:
        return {"drawing": {"fileId": file_id, "name": doc.name}, "open": True, "ready": False, "openedHere": False,
                "next": "Open elsewhere (by a person or before the add-in restarted); drawing_export only exports drawings it opened."}
    ready, detail = readiness(app, doc, file_id)
    return {"drawing": {"fileId": file_id, "name": doc.name}, "openedHere": True, **detail}


# ---------------------------------------------------------------- drawing_export

def not_ready(message, **details):
    raise MechanicalError("drawing_not_ready", message, retryable=True, retryAfterSeconds=details.pop("retryAfterSeconds", 3), **details)


def drawing_export(app, args):
    _main_thread()
    file_id = args["drawing"]["fileId"]
    outputs = {k: args[k] for k in EXTENSIONS if args.get(k)}
    require(outputs, "invalid_input", "Name at least one of pdf, dwg or dxf (absolute paths on the PC).")
    overwrite = bool(args.get("overwrite", False))
    for kind, raw in outputs.items():
        path = Path(raw)
        require(path.is_absolute(), "invalid_path", "%s must be an absolute path on the PC." % kind)
        require(path.suffix.lower() == EXTENSIONS[kind], "extension_mismatch", "%s must end in %s." % (kind, EXTENSIONS[kind]))
        require(overwrite or not path.exists(), "destination_exists", "%s exists; pass overwrite:true to replace it." % path)
    doc = find_open(app, file_id)
    if doc is None:
        data_file = app.data.findFileById(file_id)
        require(data_file is not None, "drawing_not_found", "No cloud file with that fileId.")
        require(data_file.fileExtension == "f2d", "not_a_drawing", "That file is a %s, not a drawing (.f2d)." % data_file.fileExtension)
        _opened[file_id] = {"at": time.time(), "previous": app.activeDocument,
                            "before": {app.documents.item(i).creationId for i in range(app.documents.count)}}
        # visible: an invisible open never returned (hard rule 2)
        opened = app.documents.open(data_file, True)
        require(opened is not None, "drawing_open_failed", "Fusion could not open the drawing.")
        not_ready("Opened the drawing; Fusion finishes loading it after this call returns. Call drawing_export again in a "
                  "few seconds (a new operationId): exporting in the call that opens a drawing blocks Fusion for 10 minutes.",
                  opened=True, retryAfterSeconds=5)
    require(file_id in _opened, "drawing_open_elsewhere",
            "This drawing is open in Fusion but was not opened by drawing_export in this add-in session, so its "
            "drawing engine's state is unknown and an export could block Fusion. Close it in Fusion, then call again.")
    ready, detail = readiness(app, doc, file_id)
    if not ready:
        unfinished = [c for cmds in ((detail.get("engine") or {}).get("unfinished") or {}).values() for c in cmds]
        if detail.get("activeCommand") == AUTO_DIMENSION_UI_COMMAND and doc.isActive and AUTO_DIMENSION_EDIT in unfinished:
            # Fusion starts Edit Auto Dimensions when an automatic drawing opens; end it like Esc would
            ended = app.userInterface.terminateActiveCommand()
            not_ready("Ended Fusion's Edit Auto Dimensions command on the drawing; export in the next call.",
                      endedCommand=detail.get("activeCommand"), terminated=bool(ended), detail=detail)
        not_ready("The drawing is still loading or Fusion is busy with a command; call again shortly.", detail=detail)
    drawing = adsk.drawing.DrawingDocument.cast(doc).drawing
    manager = drawing.exportManager
    results, started = {}, time.perf_counter()
    for kind, raw in outputs.items():
        if results:
            # each export runs a command in the drawing engine; the next one goes only once that finished
            engine = engine_state(_opened[file_id]["at"])
            if engine is None or engine["unfinished"]:
                not_ready("The drawing engine is still busy after the %s export; call again for the rest." % ", ".join(results),
                          outputs=results, engine=engine)
        results[kind] = export_one(manager, kind, Path(raw), overwrite, args)
    out = {"drawing": {"fileId": file_id, "name": doc.name}, "sheets": detail.get("sheets"), "outputs": results,
           "seconds": round(time.perf_counter() - started, 1)}
    if "dxf" in results:
        out["scale"] = dxf_scale(Path(outputs["dxf"]))
    if args.get("close", False):
        out["closed"] = close_drawing(app, doc, file_id)
    return out


def close_drawing(app, doc, file_id):
    """Close the drawing without saving, then the designs its open brought in (only those that were not
    open before and are unmodified), and put the previously active document back in front."""
    record = _opened.pop(file_id, {})
    references = set()
    try:
        refs = doc.documentReferences
        references = {refs.item(i).dataFile.id for i in range(refs.count) if refs.item(i).dataFile is not None}
    except Exception:
        pass
    # the drawing reads as modified once its Edit Auto Dimensions command ends (live, 2026-11-05), so
    # isModified cannot tell a person's edits from that; it is closed unsaved as Drew wrote it
    doc.close(False)
    closed = [file_id]
    before = record.get("before") or set()
    for i in reversed(range(app.documents.count)):
        d = app.documents.item(i)
        try:
            fid = d.dataFile.id if d.dataFile is not None else None
        except Exception:
            continue
        if fid in references and d.creationId not in before and not d.isModified:
            d.close(False)
            closed.append(fid)
    previous = record.get("previous")
    if previous is not None and previous.isValid:
        previous.activate()
    return closed


def dxf_dimensions(path):
    """(measured value, length on the sheet) of every linear DIMENSION in an ASCII DXF: group 42 is the
    measured value, 13/23 and 14/24 the two definition points, 50 the angle of a rotated dimension."""
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    pairs = [(lines[i].strip(), lines[i + 1].strip()) for i in range(0, len(lines) - 1, 2)]
    found, current = [], None
    for code, value in pairs:
        if code == "0":
            if current is not None:
                found.append(current)
            current = {} if value == "DIMENSION" else None
        elif current is not None and code in ("13", "23", "14", "24", "42", "50", "70"):
            try:
                current[code] = float(value)
            except ValueError:
                pass
    if current is not None:
        found.append(current)
    out = []
    for d in found:
        if not {"13", "23", "14", "24", "42"} <= set(d) or int(d.get("70", 0)) & 15 not in (0, 1) or d["42"] <= 0:
            continue  # linear (rotated, horizontal, vertical) and aligned dimensions only
        dx, dy = d["14"] - d["13"], d["24"] - d["23"]
        if int(d.get("70", 0)) & 15 == 0:
            a = math.radians(d.get("50", 0.0))
            length = abs(dx * math.cos(a) + dy * math.sin(a))
        else:
            length = math.hypot(dx, dy)
        out.append((d["42"], length))
    return out


def dxf_scale(path):
    """The scale the layout chose, read from the exported DXF's dimensions (sheet length / measured
    value, median over the dimensions); None when the drawing has no linear dimensions."""
    try:
        ratios = sorted(length / value for value, length in dxf_dimensions(path) if value > 0 and length > 0)
    except OSError:
        return None
    if not ratios:
        return None
    r = ratios[len(ratios) // 2]
    text = ("%g:1" % round(r, 3)) if r >= 1 else ("1:%g" % round(1 / r, 3))
    return {"ratio": round(r, 4), "text": text, "dimensions": len(ratios), "source": "dxf dimensions"}


def export_one(manager, kind, destination, overwrite, args):
    destination.parent.mkdir(parents=True, exist_ok=True)
    started = time.perf_counter()
    with tempfile.TemporaryDirectory(prefix=".adom-drawing-", dir=str(destination.parent)) as folder:
        staged = Path(folder) / ("drawing" + EXTENSIONS[kind])
        if kind == "pdf":
            options = manager.createPDFExportOptions(str(staged))
            options.openPDF = False
            options.useLineWeights = bool(args.get("pdfLineWeights", True))
            if args.get("pdfSheets"):
                options.sheetRange = args["pdfSheets"]
        elif kind == "dwg":
            options = manager.createDWGExportOptions(str(staged))
            options.format = _enum("DWGFormats", DWG_FORMATS[args.get("dwgFormat", "autocad")])
        else:
            options = manager.createDXFExportOptions(str(staged))
            options.exportSplinesAsSplines = bool(args.get("dxfSplines", True))
        require(options is not None, "export_preflight_failed", "Fusion did not create %s export options." % kind)
        require(bool(manager.execute(options)), "export_failed", "Fusion's %s export returned false." % kind)
        result = publish_staged(staged, destination, overwrite)
    result["seconds"] = round(time.perf_counter() - started, 1)
    return result


OPERATIONS = {"drawing_create": drawing_create, "drawing_export": drawing_export, "drawing_status": drawing_status}
WRITES = {"drawing_create", "drawing_export"}
