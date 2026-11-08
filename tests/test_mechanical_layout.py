"""Assembly layout and ray casts, offline: the parts of a scope with their root-space boxes, joints and rigid
groups as part paths, rays cast in each occurrence's own coordinates, budgets that always make progress, the
inspect-like guard (and no revision digest unless one is asked for), and which of the two is journaled."""
import importlib
import json
import math
from types import SimpleNamespace as NS
import unittest
from unittest.mock import patch

from test_mechanical import Collection, adsk, isolated_history
import mech_exe

m = importlib.import_module('mechanical_test_commands.mechanical')
core = importlib.import_module('mechanical_test_commands.mechanical.core')
layout = importlib.import_module('mechanical_test_commands.mechanical.layout')


# ---- geometry fakes: Fusion's cm, rigid transforms, a box ray caster -------------------------------------

class Matrix:
    """A rigid transform in cm: rotation rows and a translation."""
    def __init__(self, rotation=((1, 0, 0), (0, 1, 0), (0, 0, 1)), translation=(0.0, 0.0, 0.0)):
        self.r, self.t = [list(row) for row in rotation], list(translation)

    def copy(self):
        return Matrix(self.r, self.t)

    def invert(self):
        rt = [[self.r[j][i] for j in range(3)] for i in range(3)]
        self.r, self.t = rt, [-sum(rt[i][k] * self.t[k] for k in range(3)) for i in range(3)]
        return True

    def point(self, p):
        return [sum(self.r[i][k] * p[k] for k in range(3)) + self.t[i] for i in range(3)]

    def vector(self, v):
        return [sum(self.r[i][k] * v[k] for k in range(3)) for i in range(3)]


class Point:
    def __init__(self, x, y, z):
        self.x, self.y, self.z = x, y, z

    @staticmethod
    def create(x, y, z):
        return Point(x, y, z)

    def transformBy(self, matrix):
        self.x, self.y, self.z = matrix.point([self.x, self.y, self.z])
        return True

    def distanceTo(self, other):
        return math.dist((self.x, self.y, self.z), (other.x, other.y, other.z))


class Vector(Point):
    @staticmethod
    def create(x, y, z):
        return Vector(x, y, z)

    def transformBy(self, matrix):
        self.x, self.y, self.z = matrix.vector([self.x, self.y, self.z])
        return True


class Points:
    def __init__(self):
        self.values = []

    @staticmethod
    def create():
        return Points()

    def add(self, value):
        self.values.append(value)
        return True

    @property
    def count(self):
        return len(self.values)

    def item(self, index):
        return self.values[index]


class Part:
    """A component of boxes (local cm) that answers findBRepUsingRay like Fusion: one hit per body, the entry
    point, or the exit point for a ray that starts inside."""
    def __init__(self, name, boxes=(), fail=False):
        self.name, self.boxes, self.fail, self.calls = name, [tuple(map(tuple, b)) for b in boxes], fail, []

    def findBRepUsingRay(self, origin, direction, kind, tolerance, visible_only, points):
        self.calls.append({"origin": (origin.x, origin.y, origin.z), "direction": (direction.x, direction.y, direction.z),
                           "visibleOnly": visible_only, "tolerance": tolerance})
        if self.fail:
            raise RuntimeError("3 : ray refused")
        o, d = [origin.x, origin.y, origin.z], [direction.x, direction.y, direction.z]
        n = math.sqrt(sum(v * v for v in d))
        d = [v / n for v in d]
        found = Points()
        for lo, hi in self.boxes:
            t0, t1 = -math.inf, math.inf
            for k in range(3):
                if abs(d[k]) < 1e-12:
                    if not lo[k] <= o[k] <= hi[k]:
                        break
                    continue
                a, b = (lo[k] - o[k]) / d[k], (hi[k] - o[k]) / d[k]
                t0, t1 = max(t0, min(a, b)), min(t1, max(a, b))
            else:
                if t1 >= max(t0, 0.0):
                    t = t0 if t0 >= 0 else t1
                    points.add(Point(*[o[k] + t * d[k] for k in range(3)]))
                    found.add(NS(name="body"))
        return found


def body(lo, hi):
    """A body proxy whose root-space box is lo..hi mm."""
    return NS(boundingBox=NS(minPoint=NS(x=lo[0] / 10, y=lo[1] / 10, z=lo[2] / 10), maxPoint=NS(x=hi[0] / 10, y=hi[1] / 10, z=hi[2] / 10)))


class Occ:
    def __init__(self, name, path, bodies=(), children=(), visible=True, grounded=False, component=None, transform=None):
        self.name, self.fullPathName, self.isLightBulbOn, self.isGrounded = name, path, visible, grounded
        self.bRepBodies, self.childOccurrences = Collection(bodies), Collection(children)
        self.component = component or Part(name.split(":")[0])
        self.transform2 = transform or Matrix()


def comp(name, occurrences=(), joints=(), as_built=(), groups=(), bodies=0):
    return NS(name=name, occurrences=Collection(occurrences), joints=Collection(joints), asBuiltJoints=Collection(as_built),
              rigidGroups=Collection(groups), bRepBodies=Collection([object()] * bodies))


def app_for(root, name="Testbed"):
    doc = NS(name=name, isValid=True, creationId="c-" + name)
    design = NS(rootComponent=root, parentDocument=doc)
    return NS(activeDocument=doc, activeProduct=design)


GEOMETRY = {"Point3D": Point, "Vector3D": Vector, "ObjectCollection": Points}


class Base(unittest.TestCase):
    def setUp(self):
        core._entities.clear()
        core._documents.clear()
        for name, value in GEOMETRY.items():
            change = patch.object(adsk.core, name, value, create=True)
            change.start()
            self.addCleanup(change.stop)
        change = patch.object(adsk.fusion, "BRepEntityTypes", NS(BRepBodyEntityType=0), create=True)
        change.start()
        self.addCleanup(change.stop)
        self.digests = patch.object(core, "revision", return_value="r1")
        self.revision = self.digests.start()
        self.addCleanup(self.digests.stop)


# ---- assembly_layout -------------------------------------------------------------------------------------

class Layout(Base):
    def panel(self):
        """A cell with a grounded base, a panel holding a rail, two blocks (one with a nested clip), a part
        named with a "+", a hidden spare, and a part with no bodies."""
        clip = Occ("Clip:1", "Cell:1+Panel:1+Block:1+Clip:1", [body((22, 0, 18), (24, 4, 26))])
        block1 = Occ("Block:1", "Cell:1+Panel:1+Block:1", [body((20, 0, 10), (30, 10, 20))], [clip])
        block2 = Occ("Block:2", "Cell:1+Panel:1+Block:2", [body((40, 0, 10), (50, 10, 20))])
        rail = Occ("Rail:1", "Cell:1+Panel:1+Rail:1", [body((0, 0, 5), (100, 10, 10))])
        plus = Occ("Top+Bottom:1", "Cell:1+Panel:1+Top+Bottom:1", [body((60, 0, 10), (70, 10, 30))])
        spare = Occ("Spare:1", "Cell:1+Panel:1+Spare:1", [body((0, 0, 0), (1, 1, 1))], visible=False)
        empty = Occ("Label:1", "Cell:1+Panel:1+Label:1")
        joints = [NS(isSuppressed=False, occurrenceOne=NS(fullPathName="Rail:1"), occurrenceTwo=NS(fullPathName="Block:1+Clip:1")),
                  NS(isSuppressed=True, occurrenceOne=NS(fullPathName="Rail:1"), occurrenceTwo=NS(fullPathName="Block:2")),
                  NS(isSuppressed=False, occurrenceOne=NS(fullPathName="Top+Bottom:1"), occurrenceTwo=None)]
        as_built = [NS(isSuppressed=False, occurrenceOne=NS(fullPathName="Block:2"), occurrenceTwo=NS(fullPathName="Rail:1"))]
        group = NS(occurrences=Collection([NS(fullPathName="Block:1"), NS(fullPathName="Block:2+Screw:1")]))
        panel_comp = comp("Panel", joints=joints, as_built=as_built, groups=[group], bodies=1)
        panel = Occ("Panel:1", "Cell:1+Panel:1", [], [rail, block1, block2, plus, spare, empty], component=panel_comp)
        base = Occ("Base:1", "Cell:1+Base:1", [body((-10, -10, -5), (110, 20, 0))], grounded=True)
        cell = Occ("Cell:1", "Cell:1", [], [base, panel], component=comp("Cell"))
        root = comp("root", occurrences=[cell])
        return app_for(root), panel

    def layout(self, app, **args):
        return layout.assembly_layout(app, {"op": "assembly_layout", **args})

    def test_the_visible_parts_under_the_scope_with_root_space_boxes_from_their_bodies_and_children(self):
        app, _ = self.panel()
        data = self.layout(app, scope="Cell:1+Panel:1")
        units = {u["path"]: u for u in data["units"]}
        self.assertEqual(list(units), ["Cell:1+Panel:1+Rail:1", "Cell:1+Panel:1+Block:1", "Cell:1+Panel:1+Block:2",
                                       "Cell:1+Panel:1+Top+Bottom:1", "Cell:1+Panel:1+Label:1"])  # the hidden spare is left out
        self.assertEqual(units["Cell:1+Panel:1+Block:1"], {"path": "Cell:1+Panel:1+Block:1", "name": "Block:1", "component": "Block",
                                                          "grounded": False, "box": [[20.0, 0.0, 10.0], [30.0, 10.0, 26.0]], "children": 1})
        self.assertIsNone(units["Cell:1+Panel:1+Label:1"]["box"])
        self.assertEqual((data["scope"], data["scopeName"], data["document"], data["frameBodies"], data["hiddenParts"]),
                         ("Cell:1+Panel:1", "Panel", "Testbed", 1, 1))
        self.assertEqual(data["target"], {"documentId": core.active(app)[2], "revision": "r1"})

    def test_joints_and_rigid_groups_name_the_parts_they_join(self):
        app, _ = self.panel()
        data = self.layout(app, scope="Cell:1+Panel:1")
        self.assertEqual(data["joints"], [["Cell:1+Panel:1+Rail:1", "Cell:1+Panel:1+Block:1", "joint"],     # the clip belongs to block 1
                                          ["Cell:1+Panel:1+Top+Bottom:1", "", "joint"],                    # a "+" in a name stays whole
                                          ["Cell:1+Panel:1+Block:2", "Cell:1+Panel:1+Rail:1", "asBuilt"]])  # the suppressed joint is left out
        self.assertEqual(data["rigidGroups"], [["Cell:1+Panel:1+Block:1", "Cell:1+Panel:1+Block:2"]])

    def test_the_root_is_the_scope_when_none_is_given(self):
        app, _ = self.panel()
        data = self.layout(app)
        self.assertEqual([u["path"] for u in data["units"]], ["Cell:1"])
        self.assertEqual(data["units"][0]["box"], [[-10.0, -10.0, -5.0], [110.0, 20.0, 30.0]])
        self.assertEqual((data["scope"], data["scopeName"]), ("", "Testbed"))

    def test_a_part_with_more_bodies_than_max_bodies_is_boxed_breadth_first_from_the_first_ones_and_says_so(self):
        app, _ = self.panel()
        data = self.layout(app, scope="Cell:1+Panel:1", maxBodies=1)
        units = {u["path"]: u for u in data["units"]}
        self.assertEqual(units["Cell:1+Panel:1+Block:1"]["box"], [[20.0, 0.0, 10.0], [30.0, 10.0, 20.0]])  # its own body, not the clip's
        self.assertTrue(units["Cell:1+Panel:1+Block:1"]["boxCapped"])
        self.assertNotIn("boxCapped", units["Cell:1+Panel:1+Block:2"])
        self.assertEqual(data["maxBodies"], 1)

    def test_a_missing_scope_is_refused(self):
        app, _ = self.panel()
        with self.assertRaises(core.MechanicalError) as error:
            self.layout(app, scope="Cell:1+Nothing:1")
        self.assertEqual(error.exception.code, "occurrence_missing")

    def test_the_target_is_optional_and_checked_like_inspect(self):
        app, _ = self.panel()
        key = core.active(app)[2]
        self.assertEqual(self.layout(app, target={"documentId": key})["target"], {"documentId": key, "revision": "r1"})
        with self.assertRaises(core.MechanicalError) as error:
            self.layout(app, target={"documentId": key, "revision": "r0"})
        self.assertEqual(error.exception.code, "stale_revision")
        with self.assertRaises(core.MechanicalError) as error:
            self.layout(app, target={"documentId": "another document"})
        self.assertEqual(error.exception.code, "wrong_document")

    def test_a_scope_with_too_many_visible_parts_is_refused_with_a_way_out(self):
        app, _ = self.panel()
        with patch.object(layout, "MAX_UNITS", 2), self.assertRaises(core.MechanicalError) as error:
            self.layout(app, scope="Cell:1+Panel:1")
        self.assertEqual(error.exception.code, "layout_budget")
        self.assertIn("subassembly", str(error.exception))


# ---- ray_cast --------------------------------------------------------------------------------------------

class Rays(Base):
    """A plate at the origin, a block lifted 1 cm and moved (2, 2) cm, a bar turned 90 degrees about z."""
    def setUp(self):
        super().setUp()
        self.plate = Part("Plate", [((0, 0, 0), (10, 10, 1))])
        self.block = Part("Block", [((0, 0, 0), (2, 2, 2))])
        self.bar = Part("Bar", [((0, 0, 0), (4, 1, 1))])
        turn = ((0, -1, 0), (1, 0, 0), (0, 0, 1))
        self.occurrences = [Occ("Block:1", "Block:1", component=self.block, transform=Matrix(translation=(2, 2, 1))),
                            Occ("Plate:1", "Plate:1", component=self.plate),
                            Occ("Bar:1", "Bar:1", component=self.bar, transform=Matrix(turn, (6, 1, 1)))]
        self.app = app_for(comp("root", occurrences=self.occurrences))
        self.key = core.active(self.app)[2]

    def cast(self, rays, paths=("Block:1", "Plate:1", "Bar:1"), **args):
        return layout.ray_cast(self.app, {"op": "ray_cast", "target": {"documentId": self.key}, "unit": "mm",
                                          "occurrences": list(paths), "rays": rays, **args})

    @staticmethod
    def ray(origin, direction, length, against):
        return {"origin": list(origin), "direction": list(direction), "length": length, "against": list(against)}

    def test_each_ray_meets_each_named_occurrence_in_its_own_coordinates_nearest_first(self):
        data = self.cast([self.ray((30, 30, 100), (0, 0, -1), 200, [1, 0]),    # plate named first, block nearer
                          self.ray((55, 30, 100), (0, 0, -1), 200, [2]),       # the turned bar spans x 50..60, y 10..50
                          self.ray((30, 30, 100), (0, 0, -1), 75, [0, 1]),     # the plate is past the ray's length
                          self.ray((30, 30, 20), (0, 0, 1), 200, [0])])        # starts inside the block: its exit face
        self.assertEqual(data["hits"], [[[0, 70.0], [1, 90.0]], [[2, 80.0]], [[0, 70.0]], [[0, 10.0]]])
        self.assertEqual((data["cast"], data["complete"], data["missing"], data["failed"], data["casts"]), (4, True, [], 0, 6))
        first = self.block.calls[0]
        self.assertEqual([round(v, 9) for v in first["origin"]], [1.0, 1.0, 9.0])  # world (3, 3, 10) cm in the block's frame
        self.assertFalse(first["visibleOnly"])  # a component's own bodies report invisible
        self.assertEqual(first["tolerance"], -1.0)

    def test_distances_are_rounded_to_a_hundredth_of_a_millimetre(self):
        data = self.cast([self.ray((30, 30, 100.004), (0, 0, -1), 200, [0]), self.ray((30, 30, 100.0051), (0, 0, -1), 200, [0])])
        self.assertEqual(data["hits"], [[[0, 70.0]], [[0, 70.01]]])

    def test_a_missing_occurrence_is_named_and_a_refused_cast_is_counted(self):
        self.plate.fail = True
        data = self.cast([self.ray((30, 30, 100), (0, 0, -1), 200, [0, 1, 3])], paths=("Block:1", "Plate:1", "Bar:1", "Ghost:1"))
        self.assertEqual((data["hits"], data["missing"], data["failed"]), ([[[0, 70.0]]], [3], 1))

    def test_the_budget_stops_between_rays_and_says_how_far_it_got(self):
        ticks = iter([0.0, 20.0, 40.0, 41.0])
        with patch.object(layout, "_clock", lambda: next(ticks)):
            data = self.cast([self.ray((30, 30, 100), (0, 0, -1), 200, [0])] * 3, budgetSeconds=30)
        self.assertEqual((data["cast"], data["complete"], data["stopped"], len(data["hits"])), (2, False, "budget", 2))
        self.assertIn("first 2 of 3 rays", data["hint"])

    def test_every_call_answers_at_least_one_ray(self):
        ticks = iter([0.0, 99.0, 99.0, 99.0])
        with patch.object(layout, "_clock", lambda: next(ticks)):
            data = self.cast([self.ray((30, 30, 100), (0, 0, -1), 200, [0])] * 2, budgetSeconds=1)
        self.assertEqual(data["cast"], 1)
        with patch.object(layout, "MAX_REPLY_BYTES", 5):
            data = self.cast([self.ray((30, 30, 100), (0, 0, -1), 200, [0, 1])] * 2)
        self.assertEqual((data["cast"], data["stopped"]), (1, "reply_bytes"))

    def test_bad_indexes_and_a_zero_direction_are_refused_before_any_cast(self):
        for rays, text in (([self.ray((0, 0, 0), (0, 0, 1), 10, [3])], "index"), ([self.ray((0, 0, 0), (0, 0, 0), 10, [0])], "nonzero")):
            with self.assertRaises(core.MechanicalError) as error:
                self.cast(rays)
            self.assertEqual(error.exception.code, "invalid_input")
            self.assertIn(text, str(error.exception))
        self.assertEqual(self.block.calls, [])

    def test_no_revision_digest_unless_the_target_asks_for_one(self):
        data = self.cast([self.ray((30, 30, 100), (0, 0, -1), 200, [0])])
        self.assertEqual(data["target"], {"documentId": self.key})
        self.revision.assert_not_called()
        checked = layout.ray_cast(self.app, {"op": "ray_cast", "target": {"documentId": self.key, "revision": "r1"}, "unit": "mm", "rays": []})
        self.assertEqual((checked["target"], checked["hits"], checked["complete"]), ({"documentId": self.key, "revision": "r1"}, [], True))
        self.assertEqual(self.revision.call_count, 1)
        for wanted, code in (({"documentId": self.key, "revision": "r0"}, "stale_revision"), ({"documentId": "elsewhere"}, "wrong_document")):
            with self.assertRaises(core.MechanicalError) as error:
                layout.ray_cast(self.app, {"op": "ray_cast", "target": wanted, "unit": "mm", "rays": []})
            self.assertEqual(error.exception.code, code)


# ---- the exe's side: validation and journaling -----------------------------------------------------------

class ExeSide(Base):
    def setUp(self):
        super().setUp()
        isolated_history(self)
        self.app = Layout.panel(self)[0]
        self.key = core.active(self.app)[2]

    def test_both_operations_are_in_the_schema_and_only_the_layout_is_journaled(self):
        self.assertEqual(len(m.SCHEMA), 73)
        self.assertIn("assembly_layout", m.WRITES)
        self.assertNotIn("ray_cast", m.WRITES)
        # the exe decides by the schema: a write is an operation whose schema requires an operationId
        self.assertIn("operationId", m.SCHEMA["assembly_layout"]["required"])
        self.assertNotIn("operationId", m.SCHEMA["ray_cast"]["required"])

    def test_the_layout_goes_through_the_journal_and_status_reads_it_back(self):
        args = {"op": "assembly_layout", "sessionId": m.SESSION_ID, "operationId": "layout-1", "scope": "Cell:1+Panel:1"}
        reply = self.exe.call(self.app, dict(args))
        self.assertTrue(reply["success"], reply)
        self.assertTrue(reply["durability"]["terminalRecorded"])
        self.assertEqual(len(reply["data"]["units"]), 5)
        status = self.exe.status({"sessionId": m.SESSION_ID, "operationId": "layout-1"})
        self.assertEqual(status["state"], "succeeded")
        self.assertEqual(self.exe.call(self.app, dict(args))["replayed"], True)

    def test_a_ray_batch_is_answered_without_a_journal_record(self):
        app = app_for(comp("root", occurrences=[Occ("Plate:1", "Plate:1", component=Part("Plate", [((0, 0, 0), (10, 10, 1))]))]))
        reply = self.exe.call(app, {"op": "ray_cast", "target": {"documentId": core.active(app)[2]}, "unit": "mm", "occurrences": ["Plate:1"],
                                    "rays": [{"origin": [30, 30, 100], "direction": [0, 0, -1], "length": 500, "against": [0]}]})
        self.assertTrue(reply["success"], reply)
        self.assertNotIn("durability", reply)
        self.assertEqual(reply["data"]["hits"], [[[0, 90.0]]])
        self.assertFalse(self.exe.store().path.exists())  # nothing was journaled

    def test_the_exe_refuses_a_malformed_ray_before_fusion_sees_it(self):
        good = {"origin": [0, 0, 0], "direction": [0, 0, 1], "length": 10, "against": [0]}
        base = {"op": "ray_cast", "target": {"documentId": "d"}, "unit": "mm", "occurrences": ["A:1"]}
        for bad in ({**good, "against": ["0"]}, {**good, "against": [1.5]}, {**good, "origin": [0, 0]}, {k: v for k, v in good.items() if k != "length"},
                    {**good, "length": 0}, {**good, "weight": 1}, {**good, "against": []}):
            with self.assertRaises(mech_exe.MechanicalError) as error:
                mech_exe.prepare_request({**base, "rays": [bad]}, m.SCHEMA, m.WRITES, m.SESSION_ID)
            self.assertEqual(error.exception.code, "invalid_input", bad)
        with self.assertRaises(mech_exe.MechanicalError):
            mech_exe.prepare_request({**base, "rays": [good] * 8193}, m.SCHEMA, m.WRITES, m.SESSION_ID)
        frozen, write = mech_exe.prepare_request({**base, "rays": [good]}, m.SCHEMA, m.WRITES, m.SESSION_ID)
        self.assertFalse(write)
        with self.assertRaises(mech_exe.MechanicalError) as error:  # journaled: needs the live session and an operation ID
            mech_exe.prepare_request({"op": "assembly_layout", "sessionId": m.SESSION_ID}, m.SCHEMA, m.WRITES, m.SESSION_ID)
        self.assertEqual(error.exception.code, "invalid_input")


if __name__ == "__main__":
    unittest.main()
