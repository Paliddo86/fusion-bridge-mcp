"""The efficiency report's rules, offline: findings() is pure, and measure() reads a small fake design."""
import importlib
from types import SimpleNamespace as NS
import unittest
import unittest.mock

from test_mechanical import adsk, Collection, Entity

adsk.fusion.DesignTypes = NS(ParametricDesignType=1, DirectDesignType=0)
adsk.fusion.FeatureHealthStates = NS(HealthyFeatureHealthState=0, WarningFeatureHealthState=1, ErrorFeatureHealthState=2)
adsk.fusion.PatternComputeOptions = NS(OptimizedPatternCompute=0, IdenticalPatternCompute=1, AdjustPatternCompute=2)
e = importlib.import_module('mechanical_test_commands.mechanical.efficiency')


def box(x, y, z):
    return NS(minPoint=NS(x=0, y=0, z=0), maxPoint=NS(x=x, y=y, z=z))


def body(faces, size=(1, 1, 1)):
    return Entity(name="Body1", faces=Collection([0] * faces), boundingBox=box(*size))


def features(moves=0, bases=0, threads=(), patterns=()):
    return NS(moveFeatures=Collection([0] * moves), baseFeatures=Collection([0] * bases), threadFeatures=Collection([NS(isModeled=t) for t in threads]),
              rectangularPatternFeatures=Collection(list(patterns)), circularPatternFeatures=Collection([]))


def component(name, bodies=(), **extra):
    return Entity(name=name, bRepBodies=Collection(list(bodies)), meshBodies=Collection([]), sketches=Collection(extra.pop("sketches", [])),
                  occurrences=Collection(extra.pop("occurrences", [])), features=extra.pop("features", features()), **extra)


class Findings(unittest.TestCase):
    def test_a_clean_design_has_no_findings(self):
        self.assertEqual(e.findings({"snapshots": 0, "occurrences": 40, "components": {"all": 12}, "timeline": {"count": 30}}), [])

    def test_each_rule_reports_what_it_measured_and_the_fix(self):
        found = {f["rule"]: f for f in e.findings({
            "snapshots": 66, "pendingSnapshot": True, "moveFeatures": 2, "designType": "parametric", "baseFeatures": 637,
            "wires": 114, "occurrences": 4579, "components": {"all": 1249}, "timeline": {"count": 689, "errors": 1, "warnings": 3},
            "duplicateDefinitions": {"extraDefinitions": 30}, "heavyDefinitions": [{"component": "Breaker", "weightedFaces": 5514}],
            "bigSketches": [{"curves": 400}], "patterns": [{"computeOption": "adjust"}], "modeledThreads": 4, "meshBodies": 2,
            "emptyComponents": 1})}
        self.assertEqual(set(found), {"placement-snapshots", "pending-snapshot", "move-features", "duplicate-definitions", "design-size",
                                      "timeline-length", "imported-history", "wires-in-assembly", "heavy-geometry", "sketch-size",
                                      "pattern-compute", "timeline-health", "modeled-threads", "mesh-bodies", "empty-components"})
        self.assertEqual((found["placement-snapshots"]["severity"], found["placement-snapshots"]["measured"]), ("high", 66))
        self.assertEqual(found["design-size"]["limit"], 1000)
        self.assertIn("occurrence_update", found["placement-snapshots"]["fix"])

    def test_worst_first(self):
        severities = [f["severity"] for f in e.findings({"snapshots": 3, "meshBodies": 1, "occurrences": 2000, "components": {"all": 10}})]
        self.assertEqual(severities, ["high", "medium", "low"])

    def test_direct_designs_are_not_told_to_drop_history(self):
        rules = {f["rule"] for f in e.findings({"designType": "direct", "baseFeatures": 500, "wires": 100})}
        self.assertFalse(rules & {"imported-history", "wires-in-assembly"})


class Measure(unittest.TestCase):
    def test_counts_copies_snapshots_wires_and_heavy_definitions(self):
        plate = component("Plate", [body(6, (0.4, 0.4, 0.05))])
        copy = component("Plate (1)", [body(6, (0.4, 0.4, 0.05))])
        wire = component("W001 18AWG red", [body(3)])
        breaker = component("Breaker", [body(25000)])
        empty = component("Empty")
        threaded = component("Bolt", [body(40)], features=features(threads=(True, False), moves=1))
        comps = [plate, copy, wire, breaker, empty, threaded]
        tl_items = [NS(isGroup=False, entity=Entity(objectType="adsk::fusion::Snapshot"), healthState=0, isSuppressed=False),
                    NS(isGroup=False, entity=Entity(objectType="adsk::fusion::Occurrence"), healthState=1, isSuppressed=False),
                    NS(isGroup=True)]
        for comp in comps:
            comp.entityToken = comp.name
        occurrences = [NS(component=c) for c in (plate, copy, wire, breaker, breaker, threaded)]
        # Fusion's allComponents holds the root component too
        root = component("Root", allOccurrences=Collection(occurrences), allJoints=Collection([]), allAsBuiltJoints=Collection([]),
                         allRigidGroups=Collection([]), occurrences=occurrences)
        root.entityToken = "Root"
        design = NS(rootComponent=root, designType=1, timeline=Collection(tl_items), snapshots=NS(count=1, hasPendingSnapshot=False),
                    allComponents=Collection([root] + comps))
        for comp in [root] + comps:
            comp.parentDesign = design
        m = e.measure(None, design)
        self.assertEqual((m["timeline"]["count"], m["timeline"]["groups"], m["timeline"]["warnings"], m["timeline"]["byType"]),
                         (3, 1, 1, {"Snapshot": 1, "Occurrence": 1}))
        self.assertEqual((m["snapshots"], m["wires"], m["emptyComponents"], m["copyNames"], m["modeledThreads"], m["moveFeatures"]), (1, 1, 1, 1, 1, 1))
        self.assertEqual(m["duplicateDefinitions"]["extraDefinitions"], 1)
        self.assertEqual(m["heavyDefinitions"][0], {"component": "Breaker", "faces": 25000, "instances": 2, "weightedFaces": 50000})
        self.assertEqual(m["skipped"], [])

    def test_a_spent_budget_skips_the_costly_passes_and_says_so(self):
        a = component("A", [body(6)])
        root = component("Root", allOccurrences=Collection([NS(component=a)]), allJoints=Collection([]), allAsBuiltJoints=Collection([]),
                         allRigidGroups=Collection([]), occurrences=[NS(component=a)])
        design = NS(rootComponent=root, designType=0, snapshots=NS(count=0, hasPendingSnapshot=False), allComponents=Collection([root, a]))
        a.parentDesign = root.parentDesign = design
        m = e.measure(None, design, budget_seconds=0)
        self.assertEqual(m["skipped"], ["heavy geometry and duplicate definitions"])



a = importlib.import_module('mechanical_test_commands.mechanical.assemblies')
core = importlib.import_module('mechanical_test_commands.mechanical.core')


class Matrix:
    def __init__(self, values):
        self.values = list(values)

    def asArray(self):
        return list(self.values)


class Snapshots:
    def __init__(self):
        self.hasPendingSnapshot, self.added = False, 0

    def add(self):
        self.hasPendingSnapshot, self.added = False, self.added + 1
        return object()


class Occurrence:
    """Moving it the old way leaves a pending snapshot, as Fusion does; editing its initial position does not."""
    def __init__(self, snapshots, editable=None, refuse=False, timeline=None, index=0):
        self.snapshots, self.refuse, self.timeline, self.index = snapshots, refuse, timeline, index
        self.isGrounded = self.isGroundToParent = False
        self._transform = Matrix([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1])
        if editable is not None:
            self.isVaildForEditInitialPosition = editable

    @property
    def transform2(self):
        return self._transform

    @transform2.setter
    def transform2(self, matrix):
        self._transform, self.snapshots.hasPendingSnapshot = matrix, True

    @property
    def initialTransform(self):
        return self._transform

    @initialTransform.setter
    def initialTransform(self, matrix):
        if self.refuse or (self.timeline and self.timeline.markerPosition != self.index + 1):
            raise RuntimeError("Cannot be edited before rolling back" if self.timeline else "cannot find the occurrence path")
        self._transform = matrix

    @property
    def timelineObject(self):
        if not self.timeline:
            raise RuntimeError("no timeline object")
        occ = self

        class Item:
            index = occ.index

            def rollTo(self, before):
                occ.timeline.markerPosition = occ.index + (0 if before else 1)
                occ.timeline.rolls += 1
                return True
        return Item()


def moved(x):
    return Matrix([1, 0, 0, x / 10, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1])


class Placement(unittest.TestCase):
    def place(self, occ, snapshots, **args):
        design = NS(snapshots=snapshots, timeline=occ.timeline)
        with unittest.mock.patch.object(a, "resolve", lambda *_: occ), \
                unittest.mock.patch.object(a, "transform", lambda spec: moved(spec["position"][0])), \
                unittest.mock.patch.object(a, "occurrence_data", lambda o, v: {"ref": "occ"}):
            done = a.occurrence_update(design, {}, {"occurrence": "ref", "transform": {"position": [80, 0, 0]}, **args})
            return done({})

    def test_an_editable_occurrence_moves_without_a_snapshot(self):
        snapshots = Snapshots()
        occ = Occurrence(snapshots, editable=True)
        self.assertEqual(self.place(occ, snapshots)["placement"], "initial_position")
        self.assertEqual((snapshots.added, occ.transform2.asArray()[3]), (0, 8.0))

    def test_an_earlier_occurrence_is_edited_rolled_back_and_the_marker_restored(self):
        snapshots, timeline = Snapshots(), NS(markerPosition=7, rolls=0)
        occ = Occurrence(snapshots, editable=True, timeline=timeline, index=3)
        self.assertEqual(self.place(occ, snapshots)["placement"], "initial_position")
        self.assertEqual((snapshots.added, timeline.rolls, timeline.markerPosition, occ.transform2.asArray()[3]), (0, 1, 7, 8.0))

    def test_otherwise_the_move_is_captured(self):
        for occ_args in ({"editable": False}, {}, {"editable": True, "refuse": True}):
            snapshots = Snapshots()
            self.assertEqual(self.place(Occurrence(snapshots, **occ_args), snapshots)["placement"], "capture", occ_args)
            self.assertEqual(snapshots.added, 1, occ_args)

    def test_the_caller_can_insist_either_way(self):
        snapshots = Snapshots()
        self.assertEqual(self.place(Occurrence(snapshots, editable=True), snapshots, method="capture")["placement"], "capture")
        with self.assertRaises(core.MechanicalError) as error:
            self.place(Occurrence(Snapshots(), editable=False), Snapshots(), method="initial_position")
        self.assertEqual(error.exception.code, "initial_position_unavailable")
        with self.assertRaises(core.MechanicalError) as error:
            self.place(Occurrence(Snapshots(), editable=True, refuse=True), Snapshots(), method="initial_position")
        self.assertEqual(error.exception.code, "initial_position_refused")


if __name__ == "__main__":
    unittest.main()
