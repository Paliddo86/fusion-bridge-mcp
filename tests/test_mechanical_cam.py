"""CAM offline: tool choice from libraries, holes (not bosses), 2D geometry, stock and WCS, the Manufacture
workspace round trip, every operation's toolpath status, and posting (the program number, the output folder,
no post while a toolpath is missing). The fakes follow what Fusion 2705 did live (fusion-cert/cam_probe.py)."""
import importlib
from pathlib import Path
import tempfile
from types import SimpleNamespace as NS
import unittest
import unittest.mock

from test_mechanical import adsk

adsk.core.Cylinder = NS(classType=lambda: "Cylinder")
adsk.core.Plane = getattr(adsk.core, "Plane", NS(classType=lambda: "Plane"))
adsk.doEvents = lambda: None
cam = importlib.import_module('mechanical_test_commands.mechanical.cam')
core = importlib.import_module('mechanical_test_commands.mechanical.core')

CAM_API = NS(CAM=NS(cast=lambda value: value), OperationTypes=NS(MillingOperation=0),
             SetupStockModes=NS(FixedBoxStock=0, RelativeBoxStock=1, SolidStock=6),
             LoopTypes=NS(AllLoops=0, OnlyOutsideLoops=1, OnlyInsideLoops=2),
             NCProgramPostProcessOptions=NS(create=lambda: "options"))


def with_cam_api(test):
    """adsk.cam's enums and casts for one test; the shared stub module stays as other suites expect it."""
    def run(self, *args):
        with unittest.mock.patch.multiple(adsk.cam, create=True, **vars(CAM_API)):
            return test(self, *args)
    return run


class Param:
    def __init__(self, name, value=None, expression=None):
        self.name, self.value, self.expression = name, NS(value=value), expression


class Params:
    def __init__(self, **values):
        self.byname = {k: Param(k, v) for k, v in values.items()}

    def itemByName(self, name):
        return self.byname.get(name)

    @property
    def count(self):
        return len(self.byname)

    def item(self, i):
        return list(self.byname.values())[i]


def tool(kind, diameter_cm, description="", flutes=3):
    values = {"tool_type": kind, "tool_diameter": diameter_cm, "tool_description": description, "tool_numberOfFlutes": flutes,
              "tool_cornerRadius": 0.0, "tool_fluteLength": 2.0, "tool_number": 1}
    return NS(parameters=NS(itemByName=lambda n: NS(value=NS(value=values[n])) if n in values else None))


MILLING = [tool("flat end mill", 1.2, "12mm Flat Endmill"), tool("flat end mill", 0.6, "6mm Flat Endmill", 2),
           tool("flat end mill", 0.6, "6mm Flat Endmill Carbide", 3), tool("flat end mill", 0.4, "4mm"), tool("ball end mill", 0.6, "6mm Ball")]
HOLE = [tool("drill", 0.5, "5mm drill"), tool("drill", 0.42, "4.2mm drill"), tool("spot drill", 1.0, "10mm spot")]


def libraries(name, cam_=None, kind=None, cache_=None):
    label, tools = {"milling": ("Milling Tools (Metric)", MILLING), "hole": ("Hole Making Tools (Metric)", HOLE)}[name]
    return label, [t for t in tools if kind is None or cam.tool_data(t)["type"] == kind]


class Tools(unittest.TestCase):
    def setUp(self):
        patcher = unittest.mock.patch.object(cam, "library_tools", libraries)
        patcher.start()
        self.addCleanup(patcher.stop)

    def pick(self, spec, strategy="adaptive"):
        return cam.pick_tool(spec, strategy)[1]

    def test_nearest_diameter_of_the_requested_type(self):
        self.assertEqual(self.pick({"type": "flat end mill", "diameterMm": 5})["description"], "4mm")
        self.assertEqual(self.pick({"diameterMm": 7}, "face")["diameterMm"], 6.0)
        self.assertEqual(self.pick({"type": "ball end mill"})["description"], "6mm Ball")
        with self.assertRaises(core.MechanicalError) as error:
            cam.pick_tool({"type": "chamfer mill"}, "contour")
        self.assertEqual(error.exception.code, "tool_not_found")

    def test_a_tie_goes_to_the_smaller_tool(self):
        self.assertEqual(self.pick({"type": "flat end mill", "diameterMm": 5.0})["diameterMm"], 4.0)
        self.assertEqual(self.pick({"type": "flat end mill", "diameterMm": 9.0})["diameterMm"], 6.0)

    def test_description_and_flutes_narrow_the_choice_and_the_library_is_named(self):
        picked = self.pick({"diameterMm": 6, "description": "carbide"})
        self.assertEqual((picked["description"], picked["flutes"], picked["library"]), ("6mm Flat Endmill Carbide", 3, "Milling Tools (Metric)"))
        self.assertEqual(self.pick({"diameterMm": 6, "flutes": 2})["description"], "6mm Flat Endmill")
        with self.assertRaises(core.MechanicalError) as error:
            cam.pick_tool({"diameterMm": 6, "description": "diamond"}, "face")
        self.assertIn("like 'diamond'", str(error.exception))

    def test_drills_come_from_the_hole_library(self):
        picked = cam.pick_tool({"diameterMm": 4}, "drill")[1]
        self.assertEqual((picked["type"], picked["diameterMm"], picked["library"]), ("drill", 4.2, "Hole Making Tools (Metric)"))
        self.assertEqual(cam.pick_tool({"type": "spot drill"}, "drill")[1]["library"], "Hole Making Tools (Metric)")

    def test_a_job_reads_each_library_once(self):
        calls = []

        def counting(name, cam_=None, kind=None, cache_=None):
            calls.append((name, kind))
            return libraries(name, cam_, kind)
        cache = {}
        with unittest.mock.patch.object(cam, "library_tools", counting):
            for spec in ({"diameterMm": 12}, {"diameterMm": 6}, {"type": "ball end mill"}, {"type": "ball end mill", "diameterMm": 6}):
                cam.pick_tool(spec, "face", None, cache)
        self.assertEqual(calls, [("milling", "flat end mill"), ("milling", "ball end mill")])


class Libraries(unittest.TestCase):
    def test_a_job_loads_each_library_once_and_asks_fusion_for_one_type(self):
        loads, queries = [], []

        class Query:
            def __init__(self):
                self.criteria = NS(add=lambda key, value: queries.append((key, value)))

            def execute(self):
                kind = queries[-1][1]
                return [NS(tool=t) for t in MILLING if cam.tool_data(t)["type"] == kind]

        library = NS(createQuery=Query, count=len(MILLING), item=lambda i: MILLING[i])
        manager = NS(get=lambda: NS(libraryManager=NS(toolLibraries=NS(toolLibraryAtURL=lambda url: loads.append(url) or library))))
        with unittest.mock.patch.multiple(adsk.cam, create=True, CAMManager=manager), \
                unittest.mock.patch.object(adsk.core, "ValueInput", NS(createByString=lambda text: text), create=True), \
                unittest.mock.patch.object(cam, "tool_libraries", lambda: {"Milling Tools (Metric)": ("url-milling", "sample")}):
            cache = {}
            name, flats = cam.library_tools("milling", None, "flat end mill", cache)
            _, balls = cam.library_tools("milling", None, "ball end mill", cache)
            with self.assertRaises(core.MechanicalError) as error:
                cam.library_tools("Milling Tools (Furlong)", None, None, cache)
        self.assertEqual((name, len(flats), len(balls), loads), ("Milling Tools (Metric)", 4, 1, ["url-milling"]))
        self.assertEqual(queries, [("tool_type", "flat end mill"), ("tool_type", "ball end mill")])
        self.assertEqual(error.exception.code, "tool_library_missing")


class Holes(unittest.TestCase):
    def face(self, radius_cm, inward):
        normal = NS(dotProduct=lambda v: -1.0 if inward else 1.0)
        geometry = NS(objectType="Cylinder", radius=radius_cm, origin=NS(vectorTo=lambda p: "to-axis"))
        return NS(geometry=geometry, pointOnFace=NS(x=1, y=0, z=0), evaluator=NS(getNormalAtPoint=lambda p: (True, normal)))

    def test_only_concave_faces_of_the_drill_diameter_are_holes(self):
        body = NS(faces=[self.face(0.25, True), self.face(0.25, False), self.face(0.4, True)])
        self.assertEqual(len(cam.hole_faces([body], 5.0)), 1)


class Workspace:
    """Fusion's UI: a document gets its CAM product only while Manufacture is active."""
    def __init__(self, has_cam=False, current="FusionSolidEnvironment", activation_fails=False):
        self.current, self.has_cam, self.activation_fails, self.log = current, has_cam, activation_fails, []
        ui = self

        class Spaces:
            def itemById(self, wid):
                return NS(id=wid, activate=lambda: ui.activate(wid))
        self.workspaces = Spaces()
        self.products = NS(itemByProductType=self.product)

    @property
    def activeWorkspace(self):
        return NS(id=self.current)

    def activate(self, wid):
        self.log.append(wid)
        if wid == "CAMEnvironment" and self.activation_fails:
            raise RuntimeError("activation refused")
        self.current = wid
        if wid == "CAMEnvironment":
            self.has_cam = True

    def product(self, kind):
        if not self.has_cam:
            raise RuntimeError("3 : no CAM product")
        return "the CAM product"


class CamProduct(unittest.TestCase):
    @with_cam_api
    def test_a_document_with_cam_never_changes_the_workspace(self):
        ui = Workspace(has_cam=True)
        product, workspace = cam.cam_product(NS(userInterface=ui), ui)
        self.assertEqual((product, workspace, ui.log), ("the CAM product", None, []))

    @with_cam_api
    def test_the_first_job_opens_manufacture_and_puts_the_users_workspace_back(self):
        ui = Workspace(current="FusionRenderEnvironment")
        product, workspace = cam.cam_product(NS(userInterface=ui), ui)
        self.assertEqual(product, "the CAM product")
        self.assertEqual(ui.log, ["CAMEnvironment", "FusionRenderEnvironment"])
        self.assertEqual(workspace, {"switched": True, "from": "FusionRenderEnvironment", "to": "CAMEnvironment", "restored": True,
                                     "now": "FusionRenderEnvironment"})

    @with_cam_api
    def test_the_workspace_comes_back_even_when_manufacture_fails(self):
        ui = Workspace(activation_fails=True)
        with self.assertRaises(RuntimeError):
            cam.cam_product(NS(userInterface=ui), ui)
        self.assertEqual(ui.current, "FusionSolidEnvironment")

    @with_cam_api
    def test_no_product_after_manufacture_is_refused_with_the_workspace_back(self):
        ui = Workspace(current="FusionSolidEnvironment")
        ui.products = NS(itemByProductType=lambda kind: None)   # Fusion's documented answer when there is no product
        with self.assertRaises(core.MechanicalError) as error:
            cam.cam_product(NS(userInterface=ui), ui)
        self.assertEqual((error.exception.code, ui.current), ("cam_unavailable", "FusionSolidEnvironment"))

    @with_cam_api
    def test_reads_never_touch_the_workspace(self):
        for lookup in (lambda kind: None, Workspace().product):
            self.assertIsNone(cam.existing_cam(NS(products=NS(itemByProductType=lookup))))
        self.assertIsNone(cam.existing_cam(None))


class Selections:
    def __init__(self):
        self.made = []

    @property
    def count(self):
        return len(self.made)

    def item(self, i):
        return self.made[i]

    def make(self, kind):
        s = NS(kind=kind, inputGeometry=None, hasError=False, hasWarning=False)
        self.made.append(s)
        return s

    def createNewFaceContourSelection(self):
        return self.make("face contour")

    def createNewSilhouetteSelection(self):
        return self.make("silhouette")

    def createNewPocketSelection(self):
        return self.make("pocket")

    def createNewPocketRecognitionSelection(self):
        return self.make("pocket recognition")


class CurveValue:
    def __init__(self):
        self.selections, self.applied = Selections(), None

    def getCurveSelections(self):
        return self.selections

    def applyCurveSelections(self, selections):
        self.applied = selections


def operation(name, strategy="face", **state):
    base = dict(name=name, strategy=strategy, hasToolpath=True, isToolpathValid=True, hasWarning=False, warning="", hasError=False,
                error="", isSuppressed=False, operationState=0)
    base.update(state)
    return NS(**base)


class Geometry(unittest.TestCase):
    def op(self, param):
        value = CurveValue()
        return NS(parameters=NS(itemByName=lambda n: NS(value=value) if n == param else None)), value

    @with_cam_api
    def test_a_2d_contour_without_faces_follows_the_outside_of_the_setup_bodies(self):
        op, value = self.op("contours")
        chosen = cam.apply_geometry(op, "contour2d", {}, None, None, ["body"])
        self.assertEqual([(s.kind, s.inputGeometry, s.loopType) for s in value.applied.made], [("silhouette", ["body"], 1)])
        self.assertEqual(chosen, {"source": "silhouette of the setup bodies", "loops": "outside"})

    @with_cam_api
    def test_given_faces_contour_their_outside_loops_by_default(self):
        op, value = self.op("contours")
        with unittest.mock.patch.object(cam, "resolve", lambda d, v, ref, allowed: "face-" + ref):
            chosen = cam.apply_geometry(op, "contour2d", {"faces": ["a", "b"]}, None, None, ["body"])
            self.assertEqual([(s.kind, s.inputGeometry, s.loopType) for s in value.applied.made],
                             [("face contour", ["face-a"], 1), ("face contour", ["face-b"], 1)])
            self.assertEqual(chosen, {"source": "faces", "faces": 2, "loops": "outside"})
            op, value = self.op("contours")
            cam.apply_geometry(op, "contour2d", {"faces": ["a"], "loops": "all"}, None, None, ["body"])
            self.assertEqual(value.applied.made[0].loopType, 0)

    @with_cam_api
    def test_2d_pockets_clear_the_bodies_floors_unless_faces_are_given(self):
        with unittest.mock.patch.object(cam, "pocket_floors", lambda bodies: ["floor 1", "floor 2"]):
            for strategy in ("pocket2d", "adaptive2d"):
                op, value = self.op("pockets")
                chosen = cam.apply_geometry(op, strategy, {}, None, None, ["body"])
                self.assertEqual([(s.kind, s.inputGeometry) for s in value.applied.made], [("pocket", ["floor 1"]), ("pocket", ["floor 2"])])
                self.assertEqual(chosen, {"source": "pocket floors of the setup bodies (upward faces below the top)", "faces": 2})
        op, value = self.op("pockets")
        with unittest.mock.patch.object(cam, "resolve", lambda d, v, ref, allowed: "floor"):
            chosen = cam.apply_geometry(op, "pocket2d", {"faces": ["f"]}, None, None, ["body"])
        self.assertEqual([(s.kind, s.inputGeometry) for s in value.applied.made], [("pocket", ["floor"])])
        self.assertEqual(chosen, {"source": "faces", "faces": 1})
        with unittest.mock.patch.object(cam, "pocket_floors", lambda bodies: []):
            with self.assertRaises(core.MechanicalError) as error:
                cam.apply_geometry(self.op("pockets")[0], "pocket2d", {}, None, None, ["body"])
        self.assertEqual(error.exception.code, "no_pockets")

    def test_pocket_floors_are_upward_planar_faces_below_the_top(self):
        def face(z, nz, kind=None):
            return NS(geometry=NS(objectType=kind or adsk.core.Plane.classType()), pointOnFace=NS(z=z),
                      evaluator=NS(getNormalAtPoint=lambda p: (True, NS(z=nz))))
        faces = [face(1.2, 1.0), face(0.7, 1.0), face(0.0, -1.0), face(0.3, 0.0), face(0.5, 1.0, "Cylinder")]
        body = NS(boundingBox=NS(maxPoint=NS(z=1.2)), faces=faces)
        self.assertEqual(cam.pocket_floors([body]), [faces[1]])

    @with_cam_api
    def test_a_selection_fusion_complains_about_is_reported(self):
        op, value = self.op("contours")
        original = value.selections.make

        def complaining(kind):
            s = original(kind)
            s.hasWarning, s.warning = True, "No pockets found.\n"
            return s
        value.selections.make = complaining
        chosen = cam.apply_geometry(op, "contour2d", {}, None, None, ["body"])
        self.assertEqual(chosen["problems"], ["No pockets found."])


class Peck(unittest.TestCase):
    def test_the_peck_depth_goes_where_the_cycle_enables_it(self):
        q = Params(peckingDepth=None, incrementalDepth=None)
        q.byname["incrementalDepth"].isEnabled = False
        self.assertEqual(cam.set_peck(q, 2), ["peckingDepth"])
        self.assertEqual((q.itemByName("peckingDepth").expression, q.itemByName("incrementalDepth").expression), ("2.0 mm", None))
        q = Params(incrementalDepth=None)
        self.assertEqual(cam.set_peck(q, 1.5), ["incrementalDepth"])
        with self.assertRaises(core.MechanicalError):
            cam.set_peck(Params(), 2)


class Status(unittest.TestCase):
    def test_each_operation_says_what_was_generated(self):
        rows = [cam.op_status(o) for o in (
            operation("Face1"),
            operation("2D Contour1", "contour2d", hasToolpath=False, hasWarning=True, warning="No passes to link.\nGenerated toolpath is empty.\n", operationState=3),
            operation("Drill1", "drill", hasToolpath=False, hasError=True, error="No holes selected.", operationState=1),
            operation("Parallel1", "parallel", isToolpathValid=False, operationState=1),
            operation("Scallop1", "scallop", hasToolpath=False, operationState=3),
            operation("Pocket1", "pocket2d", hasToolpath=False, isSuppressed=True, operationState=2))]
        self.assertEqual([r["status"] for r in rows], ["generated", "empty", "error", "outdated", "not_generated", "suppressed"])
        self.assertEqual(rows[1]["warnings"], ["No passes to link.", "Generated toolpath is empty."])
        self.assertEqual((rows[2]["errors"], rows[2]["state"]), (["No holes selected."], "invalid"))
        self.assertEqual([r["state"] for r in rows], ["valid", "no_toolpath", "invalid", "invalid", "no_toolpath", "suppressed"])

    def test_machining_time_asks_fusion_in_its_units(self):
        calls = []
        fake = NS(getMachiningTime=lambda what, scale, rapid, change: calls.append((scale, rapid, change)) or NS(machiningTime=1.0))
        cam.machining_time(fake, "setup")
        self.assertEqual(calls, [(100.0, 5000.0 / 600.0, 15.0)])   # feed scale in %, rapidFeed in cm per second

    def test_the_reply_says_what_fusion_timed_rapids_at(self):
        # live: 2.48 cm of rapids in 0.0496 s whatever rapid rate was passed, so Fusion's own 30 m/min
        timing = NS(machiningTime=62.15, totalFeedTime=62.1, totalRapidTime=0.0496, totalToolChangeTime=0.0, feedDistance=102.5,
                    rapidDistance=2.48, toolChangeCount=0)
        data = cam.machining_data(timing)
        self.assertEqual((data["seconds"], data["feedSeconds"], data["rapidMmPerMin"], data["rapidDistanceMm"]), (62.1, 62.1, 30000, 24.8))

    def test_machining_time_only_for_a_generated_toolpath(self):
        fake = NS(getMachiningTime=lambda op, *a: NS(machiningTime=12.34))
        self.assertEqual(cam.op_status(operation("Face1"), fake)["machiningSeconds"], 12.3)
        self.assertNotIn("machiningSeconds", cam.op_status(operation("Empty", hasToolpath=False), fake))


class SetupInput:
    def __init__(self):
        self.parameters = Params(job_stockOffsetMode="'simple'", job_stockOffsetSides=0.1, job_stockOffsetTop=0.1, job_stockOffsetBottom=0,
                                 job_stockFixedX=0, job_stockFixedY=0, job_stockFixedZ=0, wcs_origin_mode="'stockPoint'",
                                 wcs_origin_boxPoint="'top center'")
        self.stockMode, self.stockSolids = 1, None


class Stock(unittest.TestCase):
    def expressions(self, q):
        return {k: p.expression for k, p in q.byname.items() if p.expression is not None}

    @with_cam_api
    def test_relative_box_offsets_and_a_wcs_at_a_stock_corner(self):
        s, spec = SetupInput(), {"stock": {"mode": "relative_box", "offsetMm": {"sides": 2, "top": 1}}, "wcs": {"origin": "stock_point", "point": "top 1"}}
        cam.check_setup_spec(spec)
        cam.stock_input(s, spec, [])
        cam.configure_setup(s.parameters, spec)
        self.assertEqual(s.stockMode, 1)
        self.assertEqual(self.expressions(s.parameters), {"job_stockOffsetMode": "'simple'", "job_stockOffsetSides": "2.0 mm", "job_stockOffsetTop": "1.0 mm",
                                                          "wcs_origin_mode": "'stockPoint'", "wcs_origin_boxPoint": "'top 1'"})

    @with_cam_api
    def test_fixed_box_size_a_model_box_point_and_the_model_origin(self):
        s, spec = SetupInput(), {"stock": {"mode": "fixed_box", "sizeMm": [110, 70, 15]}, "wcs": {"origin": "model_point", "point": "bottom 1"}}
        cam.stock_input(s, spec, [])
        cam.configure_setup(s.parameters, spec)
        self.assertEqual(s.stockMode, 0)
        self.assertEqual(self.expressions(s.parameters), {"job_stockFixedX": "110.0 mm", "job_stockFixedY": "70.0 mm", "job_stockFixedZ": "15.0 mm",
                                                          "wcs_origin_mode": "'modelPoint'", "wcs_origin_boxPoint": "'bottom 1'"})
        s = SetupInput()
        cam.configure_setup(s.parameters, {"wcs": {"origin": "model_origin"}})
        self.assertEqual(self.expressions(s.parameters), {"wcs_origin_mode": "'modelOrigin'"})

    @with_cam_api
    def test_solid_stock_takes_bodies_and_bad_combinations_are_refused(self):
        s = SetupInput()
        cam.stock_input(s, {"stock": {"mode": "solid", "bodies": ["casting"]}}, ["body-casting"])
        self.assertEqual(s.stockSolids, ["body-casting"])
        for spec in ({"stock": {"mode": "fixed_box", "offsetMm": {"top": 1}, "sizeMm": [1, 1, 1]}}, {"stock": {"mode": "fixed_box"}},
                     {"stock": {"mode": "solid"}}, {"stock": {"bodies": ["b"]}}, {"stock": {"sizeMm": [1, 2, 3]}},
                     {"wcs": {"point": "top 1"}}, {"wcs": {"origin": "model_origin", "point": "top 1"}}):
            with self.assertRaises(core.MechanicalError):
                cam.check_setup_spec(spec)

    @with_cam_api
    def test_a_choice_fusion_does_not_offer_is_refused_by_name(self):
        q = Params(wcs_origin_mode="modelOrigin")
        # as Fusion 2705 lists them: values quoted like expressions
        q.byname["wcs_origin_mode"].value = NS(value="'stockPoint'", getChoices=lambda: (True, ["Model origin", "Stock box point"], ["'modelOrigin'", "'stockPoint'"]))
        cam.set_choice(q, "wcs_origin_mode", "modelOrigin")
        self.assertEqual(q.itemByName("wcs_origin_mode").expression, "'modelOrigin'")
        with self.assertRaises(core.MechanicalError) as error:
            cam.set_choice(q, "wcs_origin_mode", "selectedPoint")
        self.assertEqual(error.exception.code, "unknown_choice")
        self.assertIn("modelOrigin, stockPoint", str(error.exception))

    def test_a_value_fusion_flags_is_refused_not_kept(self):
        q = Params(stepover=None)
        q.byname["stepover"].error = "Value out of range."
        with self.assertRaises(core.MechanicalError) as error:
            cam.set_parameter(q, "stepover", "-1 mm")
        self.assertEqual(error.exception.code, "invalid_parameter_value")
        with self.assertRaises(core.MechanicalError) as error:
            cam.set_parameter(q, "noSuchParameter", "1")
        self.assertEqual(error.exception.code, "unknown_parameter")

    @with_cam_api
    def test_the_reply_puts_the_stock_fusion_built_in_model_coordinates(self):
        # live values: the stock box (cm) is relative to the WCS, the WCS matrix is in mm; 2 mm sides, 1 mm top, WCS at top 1
        cells = {(0, 0): 1, (1, 1): 1, (2, 2): 1, (0, 3): -2.0, (1, 3): -2.0, (2, 3): 13.0, (3, 3): 1}
        setup = NS(name="Setup1", stockMode=1, parameters=Params(stockXLow=0.0, stockXHigh=10.4, stockYLow=0.0, stockYHigh=6.4,
                                                                  stockZLow=-1.3, stockZHigh=0.0),
                   workCoordinateSystem=NS(getCell=lambda r, c: cells.get((r, c), 0)))
        data = cam.setup_data(setup)
        self.assertEqual(data["stockMode"], "relative_box")
        self.assertEqual(data["stockSizeMm"], [104.0, 64.0, 13.0])
        self.assertEqual(data["stockBoxMm"], {"x": [-2.0, 102.0], "y": [-2.0, 62.0], "z": [0.0, 13.0]})
        self.assertEqual(data["wcs"], {"originMm": [-2.0, -2.0, 13.0], "xAxis": [1, 0, 0], "zAxis": [0, 0, 1]})

    def test_a_turned_wcs_turns_the_stock_back_into_model_space(self):
        # WCS z along model -z (a flipped setup): x stays, y and z flip
        cells = {(0, 0): 1, (1, 1): -1, (2, 2): -1, (2, 3): 0.0, (3, 3): 1}
        setup = NS(name="Flipped", stockMode=1, parameters=Params(stockXLow=0.0, stockXHigh=10.0, stockYLow=0.0, stockYHigh=6.0,
                                                                   stockZLow=-1.2, stockZHigh=0.0),
                   workCoordinateSystem=NS(getCell=lambda r, c: cells.get((r, c), 0)))
        self.assertEqual(cam.setup_data(setup)["stockBoxMm"], {"x": [0.0, 100.0], "y": [-60.0, 0.0], "z": [0.0, 12.0]})


class NCInput:
    """NCProgramInput: assigning operations from one setup copies the setup's job_programName and job_programComment into
    nc_program_name and nc_program_comment (Fusion's API documentation, NCProgramInput.operations)."""
    def __init__(self, default_folder):
        self.displayName, self._operations = None, None
        self.parameters = Params(nc_program_filename="1001", nc_program_name="1001", nc_program_comment="",
                                 nc_program_output_folder=str(default_folder), nc_program_openInEditor=True)

    @property
    def operations(self):
        return self._operations

    @operations.setter
    def operations(self, ops):
        self._operations = ops
        if len(ops) == 1 and hasattr(ops[0], "parameters"):
            for job, nc in (("job_programName", "nc_program_name"), ("job_programComment", "nc_program_comment")):
                value = ops[0].parameters.itemByName(job).value.value
                if value:
                    self.parameters.itemByName(nc).value.value = value


class NCProgram:
    """An NC program: postProcess writes <filename>.nc into the output folder, the program number in the header."""
    def __init__(self, ninput):
        self.parameters, self.operations, self.name = ninput.parameters, ninput.operations, ninput.displayName
        self.postConfiguration = None
        self.filteredOperations = [NS(name="Face1"), NS(name="Drill1")]

    def postProcess(self, options):
        q = self.parameters
        folder = Path(q.itemByName("nc_program_output_folder").value.value)
        folder.mkdir(parents=True, exist_ok=True)
        comment = q.itemByName("nc_program_comment").value.value
        header = "O" + q.itemByName("nc_program_name").value.value + ((" (" + comment.upper() + ")") if comment else "")
        (folder / (q.itemByName("nc_program_filename").value.value + ".nc")).write_text("%\n" + header + "\n(T1 D=12.)\nG90\nM30\n%\n")
        return True


def plate_setup():
    return NS(name="Plate job", parameters=Params(job_programName="1001", job_programComment=""))


class Posting(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.default = self.root / "Fusion NC Programs"
        self.programs = []

        def add(ninput):
            nc = NCProgram(ninput)
            self.programs.append(nc)
            return nc
        self.cam = NS(ncPrograms=NS(createInput=lambda: NCInput(self.default), add=add))
        self.machine = "FANUC"
        for change in (unittest.mock.patch.object(cam, "find_post", lambda name: NS(description=self.machine)),
                       unittest.mock.patch.object(cam, "DEFAULT_NC_FOLDER", self.default)):
            change.start()
            self.addCleanup(change.stop)

    @with_cam_api
    def test_the_program_number_and_comment_reach_the_header_and_the_file_its_folder(self):
        folder, setup = self.root / "out", plate_setup()
        out = cam.post_program(self.cam, setup, {"machine": "FANUC", "folder": str(folder), "program": "2001", "comment": "plate"})
        self.assertEqual(out["file"], str(folder / "2001.nc"))
        self.assertEqual(out["head"][:2], ["%", "O2001 (PLATE)"])
        self.assertTrue(out["programInHeader"])
        self.assertEqual(out["ncProgramKept"], {"program": "2001", "folder": folder.as_posix()})
        self.assertEqual(out["operations"], ["Face1", "Drill1"])
        self.assertNotIn("movedFromFusionFolder", out)
        self.assertFalse(self.default.exists())
        self.assertEqual(setup.parameters.itemByName("job_programName").value.value, "2001")
        self.assertIs(self.programs[0].parameters.itemByName("nc_program_openInEditor").value.value, False)

    @with_cam_api
    def test_a_header_with_another_number_is_reported(self):
        folder = self.root / "out"
        original = NCProgram.postProcess

        def renumbering(nc, options):
            nc.parameters.itemByName("nc_program_name").value.value = "1001"
            return original(nc, options)
        with unittest.mock.patch.object(NCProgram, "postProcess", renumbering):
            out = cam.post_program(self.cam, plate_setup(), {"folder": str(folder), "program": "2001"})
        self.assertEqual((out["head"][1], out["programInHeader"]), ("O1001", False))

    @with_cam_api
    def test_a_post_that_ignores_the_folder_is_collected_and_said_so(self):
        folder = self.root / "out"
        original = NCProgram.postProcess

        def stubborn(nc, options):
            nc.parameters.itemByName("nc_program_output_folder").value.value = str(self.default)
            return original(nc, options)
        with unittest.mock.patch.object(NCProgram, "postProcess", stubborn):
            out = cam.post_program(self.cam, plate_setup(), {"folder": str(folder), "program": "2002"})
        self.assertEqual((out["file"], out["movedFromFusionFolder"]), (str(folder / "2002.nc"), True))
        self.assertFalse((self.default / "2002.nc").exists())

    @with_cam_api
    def test_numbered_posts_refuse_a_name_and_others_take_it(self):
        with self.assertRaises(core.MechanicalError) as error:
            cam.post_program(self.cam, plate_setup(), {"folder": str(self.root / "out"), "program": "PLATE"})
        self.assertEqual(error.exception.code, "invalid_program")
        with self.assertRaises(core.MechanicalError):
            cam.post_program(self.cam, plate_setup(), {"folder": str(self.root / "out"), "program": "10000"})
        self.machine = "Grbl"
        out = cam.post_program(self.cam, plate_setup(), {"machine": "Grbl", "folder": str(self.root / "grbl"), "program": "PLATE"})
        self.assertEqual(out["file"], str(self.root / "grbl" / "PLATE.nc"))

    @with_cam_api
    def test_a_relative_folder_is_refused(self):
        with self.assertRaises(core.MechanicalError) as error:
            cam.post_program(self.cam, plate_setup(), {"folder": "relative/out"})
        self.assertEqual(error.exception.code, "invalid_destination")


class DocumentTools:
    """The document's tool library: Fusion appends a tool the first time an operation uses it."""
    def __init__(self, tools):
        self.tools = list(tools)

    @property
    def count(self):
        return len(self.tools)

    def remove(self, index):
        self.tools.pop(index)
        return True


class Job:
    """A setup whose operations come out of generation with the given states."""
    def __init__(self, states=None, fail_on=None, flags=None, retry_states=None):
        self.states, self.fail_on, self.deleted, self.ops, self.inputs = states or {}, fail_on, False, [], []
        self.flags, self.retry_states, self.retried = flags or {}, retry_states or {}, []
        job = self

        class Operations:
            def createInput(self, strategy):
                return NS(strategy=strategy, tool=None, parameters=Params(stepover=None))

            def add(self, oinput):
                if job.fail_on == oinput.strategy:
                    raise RuntimeError("Fusion refused " + oinput.strategy)
                params = Params(holeFaces=None, cycleType="'drilling'", incrementalDepth=None)
                for name, p in oinput.parameters.byname.items():
                    params.byname[name] = p
                    if name in job.flags:
                        p.error = job.flags[name]
                params.byname["contours"] = Param("contours", None)
                params.byname["contours"].value = CurveValue()
                params.byname["pockets"] = Param("pockets", None)
                params.byname["pockets"].value = CurveValue()
                op = operation("%s%d" % (oinput.strategy, len(job.ops) + 1), oinput.strategy, parameters=params)
                job.ops.append(op)
                if oinput.tool not in job.document_tools.tools:
                    job.document_tools.tools.append(oinput.tool)
                return op

            @property
            def count(self):
                return len(job.ops)

            def item(self, i):
                return job.ops[i]
        self.setup = NS(name="Setup1", operations=Operations(), deleteMe=self.delete, stockMode=1,
                        parameters=Params(stockXLow=0.0, stockXHigh=10.0, stockYLow=0.0, stockYHigh=6.0, stockZLow=0.0, stockZHigh=1.2),
                        workCoordinateSystem=NS(getCell=lambda r, c: 1.0 if r == c else 0.0))

        def generate(what):
            if what is self.setup:
                for op in job.ops:
                    for k, v in job.states.get(op.strategy, {}).items():
                        setattr(op, k, v)
                return NS(isGenerationCompleted=True, numberOfOperations=len(job.ops))
            for k, v in job.retry_states.get(what.strategy, {}).items():
                setattr(what, k, v)
            job.retried.append(what.name)
            return NS(isGenerationCompleted=True, numberOfOperations=1)
        self.document_tools = DocumentTools(["T1 from an earlier job"])
        self.cam = NS(documentToolLibrary=self.document_tools,
                      setups=NS(createInput=lambda kind: SetupInput(), add=lambda sinput: self.setup), generateToolpath=generate,
                      getMachiningTime=lambda what, *a: NS(machiningTime=60.0, totalFeedTime=40.0, totalRapidTime=5.0, totalToolChangeTime=15.0,
                                                           feedDistance=100.0, rapidDistance=10.0, toolChangeCount=1))

    def delete(self):
        self.deleted = True


class CamJob(unittest.TestCase):
    def run_job(self, job, operations, post=None, workspace=None, holes=4):
        args = {"bodies": ["plate"], "operations": operations, "target": {"documentId": "d", "revision": "r"}}
        if post:
            args["post"] = post
        patches = [unittest.mock.patch.object(cam, "target", lambda app, a, write, model_write: ("doc", "design", a["target"])),
                   unittest.mock.patch.object(cam, "resolve", lambda d, v, ref, allowed: NS(ref=ref, faces=[])),
                   unittest.mock.patch.object(cam, "cam_product", lambda app, doc: (job.cam, workspace)),
                   unittest.mock.patch.object(cam, "library_tools", libraries),
                   unittest.mock.patch.object(cam, "hole_faces", lambda bodies, dia: ["hole"] * holes),
                   unittest.mock.patch.object(cam, "pocket_floors", lambda bodies: ["floor"]),
                   unittest.mock.patch.object(cam, "post_program", lambda c, setup, spec: {"posted": True, "program": spec["program"]})]
        for p in patches:
            p.start()
            self.addCleanup(p.stop)
        return cam.cam_job(NS(), args)

    @with_cam_api
    def test_a_complete_job_reports_every_operation_and_posts(self):
        job = Job()
        out = self.run_job(job, [{"strategy": "face"}, {"strategy": "pocket2d"}, {"strategy": "drill", "cycle": "peck", "peckDepthMm": 2}],
                           post={"folder": "C:/out", "program": "2001"}, workspace={"switched": True})
        self.assertEqual(out["generated"], {"operations": 3, "withToolpath": 3, "complete": True, "coveredByFusion": 3, "retried": []})
        self.assertEqual([o["status"] for o in out["operations"]], ["generated"] * 3)
        drill = job.ops[2].parameters
        self.assertEqual((drill.itemByName("cycleType").expression, drill.itemByName("incrementalDepth").expression), ("'deep-drilling'", "2.0 mm"))
        self.assertEqual(out["operations"][2]["geometry"], {"source": "holes of the drill's diameter", "holes": 4})
        self.assertEqual(out["operations"][1]["geometry"], {"source": "pocket floors of the setup bodies (upward faces below the top)", "faces": 1})
        self.assertEqual(out["post"], {"posted": True, "program": "2001"})
        self.assertEqual(out["workspace"], {"switched": True})
        self.assertEqual(out["setup"]["stockBoxMm"]["x"], [0.0, 100.0])

    @with_cam_api
    def test_an_empty_toolpath_is_reported_and_blocks_the_post(self):
        job = Job(states={"contour2d": dict(hasToolpath=False, hasWarning=True, warning="Generated toolpath is empty.", operationState=3)})
        out = self.run_job(job, [{"strategy": "face"}, {"strategy": "contour2d"}], post={"folder": "C:/out", "program": "2001"})
        self.assertEqual(out["generated"], {"operations": 2, "withToolpath": 1, "complete": False, "coveredByFusion": 2, "retried": []})
        self.assertEqual(out["operations"][1]["status"], "empty")
        self.assertFalse(out["post"]["posted"])
        self.assertIn("contour2d2", out["post"]["reason"])
        self.assertFalse(job.deleted)

    @with_cam_api
    def test_an_operation_fusion_left_untouched_gets_one_more_try(self):
        untouched = dict(hasToolpath=False, isToolpathValid=False, operationState=3)
        job = Job(states={"contour2d": untouched, "adaptive2d": untouched},
                  retry_states={"contour2d": dict(hasToolpath=True, isToolpathValid=True, operationState=0)})
        out = self.run_job(job, [{"strategy": "face"}, {"strategy": "contour2d"}, {"strategy": "adaptive2d"}])
        self.assertEqual(job.retried, ["contour2d2", "adaptive2d3"])
        self.assertEqual([o["status"] for o in out["operations"]], ["generated", "generated", "not_generated"])
        self.assertEqual(out["generated"], {"operations": 3, "withToolpath": 2, "complete": False, "coveredByFusion": 3, "retried": ["contour2d2", "adaptive2d3"]})

    @with_cam_api
    def test_an_override_fusion_flags_fails_the_job_and_deletes_its_setup(self):
        job = Job(flags={"stepover": "The stepover is larger than the tool."})
        with self.assertRaises(core.MechanicalError) as error:
            self.run_job(job, [{"strategy": "parallel", "tool": {"type": "ball end mill"}, "parameters": {"stepover": "20 mm"}}])
        self.assertEqual(error.exception.code, "invalid_parameter_value")
        self.assertIn("larger than the tool", str(error.exception))
        self.assertTrue(job.deleted)
        self.assertEqual(job.ops[0].parameters.itemByName("stepover").expression, "20 mm")

    @with_cam_api
    def test_a_failed_job_deletes_its_setup_and_the_tools_it_brought(self):
        job = Job()
        with self.assertRaises(core.MechanicalError) as error:
            self.run_job(job, [{"strategy": "face", "tool": {"diameterMm": 12}}, {"strategy": "drill", "tool": {"diameterMm": 20}}], holes=0)
        self.assertEqual(error.exception.code, "no_holes")
        self.assertTrue(job.deleted)
        self.assertEqual(job.document_tools.tools, ["T1 from an earlier job"])

    @with_cam_api
    def test_a_failed_job_deletes_its_setup(self):
        job = Job(fail_on="pocket2d")
        with self.assertRaises(RuntimeError):
            self.run_job(job, [{"strategy": "face"}, {"strategy": "pocket2d"}])
        self.assertTrue(job.deleted)

    @with_cam_api
    def test_inputs_that_do_not_fit_the_strategy_are_refused(self):
        for operations, code in (([{"strategy": "face", "faces": ["f"]}], "invalid_input"),
                                 ([{"strategy": "drill", "peckDepthMm": 2}], "invalid_input"),
                                 ([{"strategy": "spiral"}], "unsupported_strategy")):
            job = Job()
            with self.assertRaises(core.MechanicalError) as error:
                self.run_job(job, operations)
            self.assertEqual(error.exception.code, code)


if __name__ == "__main__":
    unittest.main()
