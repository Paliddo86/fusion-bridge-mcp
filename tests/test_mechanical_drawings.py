"""Drawings offline: option checks, what drawing_create refuses before touching the cloud, the drawing
engine's log, and the export rule found live on 2026-11-03 (a call that opens a drawing or ends Edit
Auto Dimensions never exports; a ready drawing exports every format, staged and published)."""
import importlib
import os
from pathlib import Path
import tempfile
import time
from types import ModuleType, SimpleNamespace as NS
import sys
import unittest
import unittest.mock

from test_mechanical import adsk

ENUMS = {
    "DrawingStandardTypes": ["ISODrawingStandardType", "ASMEDrawingStandardType"],
    "DrawingUnitTypes": ["InchDrawingUnitType", "MillimeterDrawingUnitType"],
    "SheetSizes": ["CustomSizeSheetSize", "A4ISOSheetSize", "A3ISOSheetSize", "A2ISOSheetSize", "A1ISOSheetSize", "A0ISOSheetSize",
                   "AASMESheetSize", "BASMESheetSize", "CASMESheetSize", "DASMESheetSize", "EASMESheetSize"],
    "SheetOrientationTypes": ["LandscapeSheetOrientationType", "PortraitSheetOrientationType"],
    "DrawingContentTypes": ["FullAssemblyDrawingContentType", "VisibleOnlyDrawingContentType"],
    "SheetCreationTypes": ["FirstLevelOnlySheetCreationType", "AllLevelsSheetCreationType"],
    "DimensionStrategyTypes": ["OverallDimensionStrategyType", "AutomaticDimensionStrategyType", "BaselineDimensionStrategyType",
                               "ChainDimensionStrategyType", "OrdinateDimensionStrategyType", "SymmetricDimensionStrategyType",
                               "SymmetricWithBaselineDimensionStrategyType", "SymmetricWithOrdinateDimensionStrategyType"],
    "HolePreferencesTypes": ["HoleAndThreadNoteHolePreferencesType", "HoleNoteOnlyHolePreferencesType",
                             "ThreadNoteOnlyHolePreferencesType", "NoHoleAnnotationsHolePreferencesType"],
    "DefaultOriginTypes": ["ModelOriginDefaultOriginType", "BottomLeftDefaultOriginType", "BottomRightDefaultOriginType",
                           "TopLeftDefaultOriginType", "TopRightDefaultOriginType"],
    "DrawingViewStyleTypes": ["VisibleEdgesDrawingViewStyleType", "VisibleAndHiddenEdgesDrawingViewStyleType",
                              "ShadedAndHiddenEdgesDrawingViewStyleType", "ShadedVisibleEdgesDrawingViewStyleType"],
    "TangentEdgeDisplayTypes": ["OffTangentEdgeDisplayType", "FullLengthTangentEdgeDisplayType", "ShortenedTangentEdgeDisplayType"],
    "DrawingCreationModes": ["AutomaticDrawingCreationMode", "ManualDrawingCreationMode"],
    "DWGFormats": ["SimplifiedDWGFormat", "AutoCADDWGFormat"],
}
adsk.drawing = ModuleType("adsk.drawing")
for cls, members in ENUMS.items():
    setattr(adsk.drawing, cls, NS(**{m: i for i, m in enumerate(members)}))
adsk.drawing.DrawingDocument = NS(cast=lambda doc: doc)
sys.modules["adsk.drawing"] = adsk.drawing
dr = importlib.import_module("mechanical_test_commands.mechanical.drawings")
core = importlib.import_module("mechanical_test_commands.mechanical.core")


def raises(test, code, fn, *args, **kwargs):
    with test.assertRaises(core.MechanicalError) as error:
        fn(*args, **kwargs)
    test.assertEqual(error.exception.code, code)
    return error.exception


class Options(unittest.TestCase):
    def test_defaults_follow_the_standard(self):
        iso = dr.resolve_options({})
        self.assertEqual((iso["standard"], iso["units"], iso["sheetSize"], iso["dimensions"]), ("iso", "mm", "A3", "overall"))
        self.assertTrue(iso["orthographic"] and iso["isometric"])
        asme = dr.resolve_options({"standard": "asme"})
        self.assertEqual((asme["units"], asme["sheetSize"]), ("in", "B"))

    def test_a_sheet_of_the_other_standard_is_refused_not_ignored(self):
        # Fusion silently ignores a sheet size from the other standard; refusing says why
        raises(self, "sheet_size_mismatch", dr.resolve_options, {"standard": "asme", "sheetSize": "A3"})
        raises(self, "sheet_size_mismatch", dr.resolve_options, {"sheetSize": "D"})
        raises(self, "invalid_input", dr.resolve_options, {"sheetSize": "A0", "orientation": "portrait"})

    def test_options_reach_every_sheet_type(self):
        def prefs(component=True):
            view = NS(style=None, tangentEdgesType=None)
            dims = NS(isAutoDimensionEnabled=None, dimensionStrategyType=None, holePreferencesType=None)
            if component:
                dims.defaultOriginType = None
            return NS(autoDimensionPreferences=dims, drawingViewPreferences=view,
                      sheetViewPreferences=NS(isOrthogonalViewAdded=None, isIsometricViewAdded=None))
        asm = lambda: NS(autoDimensionPreferences=prefs(False).autoDimensionPreferences, drawingViewPreferences=NS(style=None, tangentEdgesType=None),
                         isoViewSheetPreferences=NS(isSheetCreated=None, isPartsListIncluded=None),
                         orthogonalViewSheetPreferences=NS(isSheetCreated=None, isPartsListIncluded=None))
        auto = NS(globalPreferences=NS(), componentPreferences=prefs(), foldedModelPreferences=prefs(), flatPatternPreferences=prefs(),
                  mainAssemblyPreferences=asm(), subAssemblyPreferences=asm())
        inp = NS(automationPreferences=auto)
        dr.apply_options(inp, dr.resolve_options({"standard": "asme", "sheetSize": "C", "dimensions": "ordinate", "datum": "bottom_left",
                                                  "style": "visible_and_hidden", "views": {"isometric": False}}))
        D = adsk.drawing
        self.assertEqual((inp.standard, inp.units, inp.sheetSize), (D.DrawingStandardTypes.ASMEDrawingStandardType,
                         D.DrawingUnitTypes.InchDrawingUnitType, D.SheetSizes.CASMESheetSize))
        for p in (auto.componentPreferences, auto.flatPatternPreferences, auto.mainAssemblyPreferences):
            self.assertEqual(p.autoDimensionPreferences.dimensionStrategyType, D.DimensionStrategyTypes.OrdinateDimensionStrategyType)
            self.assertEqual(p.drawingViewPreferences.style, D.DrawingViewStyleTypes.VisibleAndHiddenEdgesDrawingViewStyleType)
        self.assertEqual(auto.componentPreferences.autoDimensionPreferences.defaultOriginType, D.DefaultOriginTypes.BottomLeftDefaultOriginType)
        self.assertFalse(auto.componentPreferences.sheetViewPreferences.isIsometricViewAdded)
        # the assembly's isometric sheet carries the parts list: views.isometric leaves it on
        self.assertTrue(auto.mainAssemblyPreferences.isoViewSheetPreferences.isSheetCreated)
        self.assertTrue(auto.globalPreferences.isAutoDimensionEnabled)
        dr.apply_options(inp, dr.resolve_options({"dimensions": "none"}))
        self.assertFalse(auto.globalPreferences.isAutoDimensionEnabled)
        self.assertFalse(auto.componentPreferences.autoDimensionPreferences.isAutoDimensionEnabled)


class Create(unittest.TestCase):
    def setUp(self):
        self.created = []
        root = NS(isRoot=True, name="Project", id="root")
        self.sub = NS(isRoot=False, name="Adom AI Workspace task", id="sub")
        self.source = NS(id="lineage:part", name="Bracket", versionNumber=3, fileExtension="f3d", isComplete=True,
                         parentFolder=self.sub, parentProject=NS(name="Main"))
        self.doc = NS(dataFile=self.source, isModified=False)
        manager = NS(createDrawingInput=lambda f, mode: NS(automationPreferences=None),
                     createDrawing=lambda inp: self.created.append(inp) or self.drawing)
        self.drawing = NS(id="lineage:drawing", name="Bracket Drawing", versionNumber=1, fileExtension="f2d", parentFolder=self.sub,
                          parentProject=NS(name="Main"), move=lambda folder: True)
        dr.adsk.drawing.DrawingManager = NS(get=lambda: manager)
        self.app = NS(data=NS(findFolderById=lambda i: {"root": root, "sub": self.sub}.get(i)))
        self.patches = [unittest.mock.patch.object(dr, "target", lambda app, args, write=False, model_write=True: (self.doc, None, {"documentId": "d", "revision": "r"})),
                        unittest.mock.patch.object(dr, "apply_options", lambda inp, o: None)]
        for p in self.patches:
            p.start()
            self.addCleanup(p.stop)

    def call(self, **args):
        return dr.drawing_create(self.app, {"target": {"documentId": "d", "revision": "r"}, **args})

    def test_a_never_saved_or_edited_design_is_refused_before_any_drawing(self):
        self.doc.dataFile = None
        raises(self, "design_not_saved", self.call)
        self.doc.dataFile, self.doc.isModified = self.source, True
        e = raises(self, "unsaved_changes", self.call)
        self.assertEqual(e.details["savedVersion"], 3)
        self.assertEqual(self.created, [])
        self.assertEqual(self.call(useSavedVersion=True)["drawing"]["fileId"], "lineage:drawing")

    def test_never_into_a_project_root(self):
        self.source.parentFolder = NS(isRoot=True, name="Project", id="root")
        raises(self, "refused_root_upload", self.call)
        raises(self, "refused_root_upload", self.call, folderId="root")
        raises(self, "folder_not_found", self.call, folderId="nope")
        self.assertEqual(self.call(folderId="sub")["movedToFolder"], True)
        self.assertEqual(len(self.created), 1)

    def test_a_drawing_that_cannot_be_moved_is_deleted_not_left_beside_the_design(self):
        deleted = []
        self.drawing.move = lambda folder: False
        self.drawing.deleteMe = lambda: deleted.append(True) or True
        e = raises(self, "drawing_move_failed", self.call, folderId="sub")
        self.assertEqual((deleted, e.details["drawingDeleted"]), ([True], True))

    def test_upload_still_running_is_retryable(self):
        self.source.isComplete = False
        e = raises(self, "design_upload_pending", self.call)
        self.assertTrue(e.details["retryable"])

    def test_a_drawing_is_reported_with_its_source_and_options(self):
        out = self.call(name="Bracket drawing rev A", sheetSize="A4")
        self.assertEqual(self.drawing.name, "Bracket drawing rev A")
        self.assertEqual((out["source"]["fileId"], out["drawing"]["extension"], out["options"]["sheetSize"]), ("lineage:part", "f2d", "A4"))
        self.assertIn("drawing_export", out["next"])


ENGINE_LOG = """20261103T114011 I .35288 cmd will Start: FD_CMD
20261103T114011 I .35288 cmd Finished: FD_CMD Reason: 1
20261103T114012 I .35288 cmd will Start: FD_SMARTLAYOUT
20261103T114020 I .35288 cmd will Start: AUTODIMENSION_NON_INTERACTIVE
20261103T114022 I .35288 cmd Finished: AUTODIMENSION_NON_INTERACTIVE Reason: 1
20261103T114023 I .35288 cmd Finished: FD_SMARTLAYOUT Reason: 1
20261103T114027 I .35288 cmd will Start: QUIT
20261103T114027 I .35288 closing dwg: C:/Temp/Document1.dwg
20261103T114028 I .35288 unloading AcFusionService.crx
20261103T114042 I .50928 Canvas Open
20261103T114043 I .50928 cmd will Start: FD_CMD
20261103T114043 I .50928 cmd Finished: FD_CMD Reason: 1
20261103T114043 I .50928 cmd will Start: AUTODIMENSIONEDIT
"""


ENGINE_OPEN = """20261103T114042 I .50928 Canvas Open
20261103T114043 I .50928 cmd will Start: FD_CMD
20261103T114043 I .50928 doCommand(), json={"action":"opendoc","data":{"TempDir":"C:/Temp/x/"}}
20261103T114043 I .50928 cmd Finished: FD_CMD Reason: 1
"""


class EngineLog(unittest.TestCase):
    def write(self, text):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        path = Path(folder.name) / "FusionDrawing.log"
        path.write_text(text, encoding="utf-8")
        return path

    def at(self, stamp):
        return time.mktime(time.strptime(stamp, "%Y%m%dT%H%M%S"))

    def test_edit_auto_dimensions_left_open_is_found_and_a_closed_engine_is_ignored(self):
        state = dr.engine_state(self.at("20261103T114000"), self.write(ENGINE_LOG), now=self.at("20261103T114100"))
        self.assertEqual(state["unfinished"], {"50928": ["AUTODIMENSIONEDIT"]})
        self.assertEqual(state["quietSeconds"], 17.0)

    def test_a_loaded_idle_engine_after_the_open(self):
        path = self.write(ENGINE_LOG + "20261103T114500 I .50928 cmd Finished: AUTODIMENSIONEDIT Reason: 1\n")
        before = dr.engine_state(self.at("20261103T114000"), path, now=self.at("20261103T114502"))
        self.assertEqual((before["unfinished"], before["quietSeconds"]), ({}, 2.0))
        # the creation engine (35288) loaded nothing and quit; only an opendoc counts as loaded
        self.assertFalse(dr.engine_state(self.at("20261103T114000"), self.write(ENGINE_LOG.split("20261103T114042")[0]),
                                         now=self.at("20261103T114100"))["loaded"])
        self.assertTrue(dr.engine_state(self.at("20261103T114040"), self.write(ENGINE_OPEN), now=self.at("20261103T114100"))["loaded"])

    def test_lines_before_the_open_do_not_count(self):
        state = dr.engine_state(self.at("20261103T114600"), self.write(ENGINE_LOG), now=self.at("20261103T114700"))
        self.assertEqual((state["loaded"], state["unfinished"], state["quietSeconds"]), (False, {}, None))


def dxf(*dimensions):
    """A minimal ASCII DXF with DIMENSION entities: (type, measured, (x1, y1), (x2, y2), angle)."""
    out = ["0", "SECTION", "2", "ENTITIES"]
    for kind, value, p1, p2, angle in dimensions:
        out += ["0", "DIMENSION", "8", "0", "70", str(kind), "13", str(p1[0]), "23", str(p1[1]), "14", str(p2[0]), "24", str(p2[1]),
                "42", str(value)] + (["50", str(angle)] if angle is not None else [])
    out += ["0", "LINE", "10", "0", "20", "0", "0", "ENDSEC", "0", "EOF"]
    return "\n".join(out) + "\n"


class Scale(unittest.TestCase):
    def write(self, text):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        path = Path(folder.name) / "d.dxf"
        path.write_text(text)
        return path

    def test_scale_from_the_exported_dimensions(self):
        # a horizontal 40 and a vertical 30 drawn at half size, an aligned 50 at half size: 1:2
        path = self.write(dxf((0, 40.0, (10, 10), (30, 10), 0), (0, 30.0, (5, 5), (5, 20), 90), (1, 50.0, (0, 0), (15, 20), None)))
        self.assertEqual(dr.dxf_scale(path), {"ratio": 0.5, "text": "1:2", "dimensions": 3, "source": "dxf dimensions"})
        self.assertEqual(dr.dxf_scale(self.write(dxf((0, 10.0, (0, 0), (20, 0), 0))))["text"], "2:1")

    def test_no_linear_dimensions_no_scale(self):
        self.assertIsNone(dr.dxf_scale(self.write(dxf())))
        # an angular dimension (type 2) says nothing about the scale
        self.assertIsNone(dr.dxf_scale(self.write(dxf((2, 90.0, (0, 0), (10, 0), None)))))


class View:
    def __init__(self, kind=0, loading=False):
        self.kind, self.loading = kind, loading

    @property
    def type(self):
        if self.loading:
            raise RuntimeError("2 : InternalValidationError : pAsset")
        return self.kind

    viewCurves = NS(count=12)


class Items:
    def __init__(self, values):
        self.values = values

    @property
    def count(self):
        return len(self.values)

    def item(self, i):
        return self.values[i]


class Manager:
    def __init__(self, fail=False):
        self.calls, self.fail = [], fail

    def _options(self, kind):
        def make(path):
            return NS(kind=kind, path=path)
        return make

    createPDFExportOptions = property(lambda self: self._options("pdf"))
    createDWGExportOptions = property(lambda self: self._options("dwg"))
    createDXFExportOptions = property(lambda self: self._options("dxf"))

    def execute(self, options):
        self.calls.append(options.kind)
        if not self.fail:
            Path(options.path).write_bytes(b"%s of the drawing" % options.kind.encode())
        return not self.fail


class Export(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.dir = Path(folder.name)
        self.views = [View(0), View(1), View(1), View(1)]
        self.manager = Manager()
        sheet = NS(name="Bracket", width=42.0, height=29.7, sheetSize=adsk.drawing.SheetSizes.A3ISOSheetSize, views=Items(self.views))
        self.closed, self.activated = [], []
        self.previous = NS(isValid=True, activate=lambda: self.activated.append("previous"))
        self.doc = NS(dataFile=NS(id="lineage:drawing"), name="Bracket Drawing", isActive=True, isVisible=True, isModified=False,
                      drawing=NS(sheets=Items([sheet]), exportManager=self.manager,
                                 documentSettings=NS(units=adsk.drawing.DrawingUnitTypes.MillimeterDrawingUnitType)),
                      close=lambda save: self.closed.append(save))
        self.opened = []
        self.ui = NS(activeCommand="SelectCommand", terminated=[])
        self.ui.terminateActiveCommand = lambda: self.ui.terminated.append(self.ui.activeCommand) or True
        self.data_file = NS(id="lineage:drawing", fileExtension="f2d")
        self.app = NS(documents=Items([]), userInterface=self.ui, activeDocument=self.previous,
                      data=NS(findFileById=lambda i: self.data_file if i == "lineage:drawing" else None))
        def open_doc(f, visible=True):
            self.opened.append(visible)
            self.app.documents.values.append(self.doc)
            return self.doc
        self.app.documents.open = open_doc
        self.engine = {"loaded": True, "unfinished": {}, "quietSeconds": 9.0}
        p = unittest.mock.patch.object(dr, "engine_state", lambda since, path=None, now=None: dict(self.engine))
        p.start()
        self.addCleanup(p.stop)
        dr._opened.clear()

    def call(self, **args):
        base = {"drawing": {"fileId": "lineage:drawing"}, "pdf": str(self.dir / "b.pdf"), "dwg": str(self.dir / "b.dwg"), "dxf": str(self.dir / "b.dxf")}
        return dr.drawing_export(self.app, {**base, **args})

    def test_the_call_that_opens_the_drawing_never_exports(self):
        e = raises(self, "drawing_not_ready", self.call)
        self.assertTrue(e.details["opened"] and e.details["retryable"])
        # opened visibly (an invisible open never returned), nothing exported
        self.assertEqual((self.opened, self.manager.calls), ([True], []))
        self.assertIs(dr._opened["lineage:drawing"]["previous"], self.previous)

    def open_here(self):
        self.app.documents.values.append(self.doc)
        dr._opened["lineage:drawing"] = {"at": time.time(), "previous": self.previous}

    def test_a_view_fusion_cannot_read_is_reported_not_taken_for_loading(self):
        # live on 2705: View.type raised pAsset for every view of a fully opened, idle drawing
        self.open_here()
        self.views[1].loading = True
        out = self.call()
        self.assertEqual(self.manager.calls, ["pdf", "dwg", "dxf"])
        self.assertEqual(out["sheets"][0]["views"][1]["type"], None)
        self.assertIn("pAsset", out["sheets"][0]["views"][1]["unreadable"])

    def test_before_the_engine_loaded_the_drawing_nothing_exports(self):
        self.open_here()
        self.engine.update(loaded=False, quietSeconds=None)
        e = raises(self, "drawing_not_ready", self.call)
        self.assertFalse(e.details["detail"]["engine"]["loaded"])
        self.assertEqual(self.manager.calls, [])

    def test_edit_auto_dimensions_is_ended_and_the_export_waits_for_the_next_call(self):
        self.open_here()
        self.ui.activeCommand = "FusionDrawingAutoDimensionEditCommand"
        self.engine["unfinished"] = {"50928": ["AUTODIMENSIONEDIT"]}
        e = raises(self, "drawing_not_ready", self.call)
        self.assertEqual(self.ui.terminated, ["FusionDrawingAutoDimensionEditCommand"])
        self.assertEqual(self.manager.calls, [])
        self.assertTrue(e.details["terminated"])

    def test_someone_elses_command_is_never_ended(self):
        self.open_here()
        self.ui.activeCommand = "SketchCreate"
        raises(self, "drawing_not_ready", self.call)
        self.assertEqual((self.ui.terminated, self.manager.calls), ([], []))

    def test_another_command_is_not_ended_even_while_the_engine_edits_dimensions(self):
        # only Fusion's own Edit Auto Dimensions command is ended, never one a person started on the drawing
        self.open_here()
        self.ui.activeCommand = "FusionDrawingTextCommand"
        self.engine["unfinished"] = {"50928": ["AUTODIMENSIONEDIT"]}
        e = raises(self, "drawing_not_ready", self.call)
        self.assertEqual((self.ui.terminated, self.manager.calls), ([], []))
        self.assertNotIn("terminated", e.details)

    def test_close_still_closes_after_edit_auto_dimensions_marked_the_drawing_modified(self):
        # live on Colby-ROG2: ending Edit Auto Dimensions leaves the drawing isModified, and close:true must still close it
        self.open_here()
        self.doc.isModified = True
        out = self.call(close=True)
        self.assertEqual((out["closed"], self.closed, self.activated), (["lineage:drawing"], [False], ["previous"]))

    def test_an_engine_that_just_logged_or_never_loaded_is_not_ready(self):
        self.open_here()
        self.engine["quietSeconds"] = 1.0
        raises(self, "drawing_not_ready", self.call)
        self.engine.update(quietSeconds=30.0, loaded=False)
        raises(self, "drawing_not_ready", self.call)
        self.assertEqual(self.manager.calls, [])

    def test_a_drawing_opened_elsewhere_is_refused(self):
        self.app.documents.values.append(self.doc)
        raises(self, "drawing_open_elsewhere", self.call)
        self.assertEqual((self.opened, self.manager.calls), ([], []))

    def test_a_ready_drawing_exports_every_format_staged_and_published(self):
        self.open_here()
        out = self.call(close=True)
        self.assertEqual(self.manager.calls, ["pdf", "dwg", "dxf"])
        for kind in ("pdf", "dwg", "dxf"):
            path = self.dir / ("b." + kind)
            self.assertEqual(path.read_bytes(), b"%s of the drawing" % kind.encode())
            self.assertEqual(out["outputs"][kind]["fileSize"], path.stat().st_size)
        self.assertEqual([v["type"] for v in out["sheets"][0]["views"]], ["base", "projected", "projected", "projected"])
        self.assertEqual((out["sheets"][0]["paper"], out["sheets"][0]["units"]), ("A3", "mm"))
        self.assertIsNone(out["scale"])  # this stand-in DXF has no dimensions
        # closed without saving, and the document that was in front before the open is in front again
        self.assertEqual((out["closed"], self.closed, self.activated), (["lineage:drawing"], [False], ["previous"]))
        self.assertNotIn("lineage:drawing", dr._opened)
        self.assertEqual([p.name for p in self.dir.iterdir() if p.name.startswith(".adom-drawing-")], [])

    def test_close_also_closes_the_design_the_open_brought_in_and_nothing_else(self):
        closed = []
        def design(fid, cid, modified=False):
            return NS(dataFile=NS(id=fid), creationId=cid, isModified=modified, close=lambda save: closed.append(fid))
        brought = design("lineage:part", "c-new")
        already = design("lineage:other", "c-old")
        edited = design("lineage:edited", "c-new2", modified=True)
        self.doc.documentReferences = Items([NS(dataFile=NS(id="lineage:part")), NS(dataFile=NS(id="lineage:other")),
                                             NS(dataFile=NS(id="lineage:edited"))])
        self.doc.close = lambda save: self.app.documents.values.remove(self.doc)
        self.app.documents.values.extend([self.doc, brought, already, edited])
        dr._opened["lineage:drawing"] = {"at": time.time(), "previous": self.previous, "before": {"c-old"}}
        out = self.call(close=True)
        # the part the open brought in closes; one open before, or edited since, stays
        self.assertEqual((out["closed"], closed), (["lineage:drawing", "lineage:part"], ["lineage:part"]))

    def test_destinations_are_checked_before_anything_opens(self):
        raises(self, "invalid_input", dr.drawing_export, self.app, {"drawing": {"fileId": "lineage:drawing"}})
        raises(self, "invalid_path", self.call, pdf="relative.pdf")
        raises(self, "extension_mismatch", self.call, dwg=str(self.dir / "b.dxf"))
        (self.dir / "b.pdf").write_bytes(b"keep")
        raises(self, "destination_exists", self.call)
        self.assertEqual((self.opened, (self.dir / "b.pdf").read_bytes()), ([], b"keep"))

    def test_an_engine_still_busy_after_one_format_returns_the_files_so_far(self):
        self.open_here()
        calls = {"n": 0}
        def engine(since, path=None, now=None):
            calls["n"] += 1
            # ready before the first export, busy right after it
            return {"loaded": True, "unfinished": {} if calls["n"] == 1 else {"1": ["FD_CMD"]}, "quietSeconds": 9.0}
        with unittest.mock.patch.object(dr, "engine_state", engine):
            e = raises(self, "drawing_not_ready", self.call)
        self.assertEqual((self.manager.calls, list(e.details["outputs"])), (["pdf"], ["pdf"]))
        self.assertTrue((self.dir / "b.pdf").exists() and not (self.dir / "b.dwg").exists())

    def test_a_failed_export_leaves_the_destination_alone(self):
        self.open_here()
        self.manager.fail = True
        raises(self, "export_failed", self.call, dwg=None, dxf=None)
        self.assertFalse((self.dir / "b.pdf").exists())

    def test_only_drawings_open(self):
        self.data_file.fileExtension = "f3d"
        raises(self, "not_a_drawing", self.call)
        raises(self, "drawing_not_found", dr.drawing_export, self.app, {"drawing": {"fileId": "nope"}, "pdf": str(self.dir / "x.pdf")})


class Status(unittest.TestCase):
    def test_closed_open_elsewhere_and_opened_here(self):
        dr._opened.clear()
        doc = NS(dataFile=NS(id="f"), name="D", isActive=True, isVisible=True,
                 drawing=NS(sheets=Items([NS(name="S", width=42.0, height=29.7, sheetSize=adsk.drawing.SheetSizes.BASMESheetSize,
                                             views=Items([View(0)]))]),
                            documentSettings=NS(units=adsk.drawing.DrawingUnitTypes.InchDrawingUnitType)))
        app = NS(documents=Items([]), userInterface=NS(activeCommand="SelectCommand"))
        self.assertFalse(dr.drawing_status(app, {"drawing": {"fileId": "f"}})["open"])
        app.documents.values.append(doc)
        self.assertFalse(dr.drawing_status(app, {"drawing": {"fileId": "f"}})["openedHere"])
        dr._opened["f"] = {"at": time.time(), "previous": None}
        with unittest.mock.patch.object(dr, "engine_state", lambda since, path=None, now=None: {"loaded": True, "unfinished": {}, "quietSeconds": 8.0}):
            out = dr.drawing_status(app, {"drawing": {"fileId": "f"}})
        self.assertTrue(out["ready"] and out["openedHere"])
        self.assertEqual((out["sheets"][0]["paper"], out["sheets"][0]["units"]), ("B", "in"))
        dr._opened.clear()


if __name__ == "__main__":
    unittest.main()
