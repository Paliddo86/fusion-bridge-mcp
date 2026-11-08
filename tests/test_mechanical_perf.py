"""Hot paths of the revision digest, offline: what each native read costs is the point, so these count reads."""
import importlib
import time
import unittest.mock
from types import SimpleNamespace as NS
import unittest

from test_mechanical import adsk

core = importlib.import_module('mechanical_test_commands.mechanical.core')
readback = importlib.import_module('mechanical_test_commands.mechanical.readback')
regular_joints = importlib.import_module('mechanical_test_commands.mechanical.regular_joints')
bom = importlib.import_module('mechanical_test_commands.mechanical.bill_of_materials')


class Counted:
    """An occurrence whose entityToken read is counted (it costs about 2 ms in Fusion)."""
    objectType = "adsk::fusion::Occurrence"

    def __init__(self, path):
        self.fullPathName, self.reads, self.isValid = path, 0, True

    @property
    def entityToken(self):
        self.reads += 1
        return "token-" + self.fullPathName


class Identity(unittest.TestCase):
    def test_a_known_entity_is_found_without_reading_its_token(self):
        occ = Counted("Frame:1+Rail:3")
        first = core.identity(occ)
        reads = occ.reads
        for _ in range(5):
            self.assertEqual(core.identity(occ), first)
        self.assertEqual(occ.reads, reads)

    def test_a_renamed_entity_is_still_found_by_its_token(self):
        occ = Counted("Frame:1+Rail:4")
        key = core.identity(occ)
        occ.fullPathName = "Frame:1+Rail renamed:4"   # new bucket; same token string? no: token follows the object
        occ.__class__ = type("Renamed", (Counted,), {"entityToken": property(lambda self: "token-Frame:1+Rail:4")})
        self.assertEqual(core.identity(occ), key)

    def test_a_reference_reuses_the_token_read_at_registration(self):
        occ = Counted("Frame:1+Rail:5")
        core.identity(occ)
        reads = occ.reads
        for _ in range(3):
            self.assertEqual(core.entity_token(occ), "token-Frame:1+Rail:5")
        self.assertEqual(occ.reads, reads)

    def test_an_unregistered_occurrence_reads_its_token_once(self):
        occ = Counted("Frame:1+Rail:6")
        self.assertEqual(core.entity_token(occ), "token-Frame:1+Rail:6")
        self.assertEqual(core.entity_token(occ), "token-Frame:1+Rail:6")
        self.assertEqual(occ.reads, 1)

    def test_other_entities_read_their_own_token(self):
        body = NS(objectType="adsk::fusion::BRepBody", entityToken="body-token")
        self.assertEqual(core.entity_token(body), "body-token")


class Body:
    """A body whose witness reads are counted (physical properties cost milliseconds in Fusion)."""
    objectType = "adsk::fusion::BRepBody"

    def __init__(self, name, component):
        self.name, self.parentComponent, self.isValid, self.revisionId = name, component, True, "b1"
        self.material, self.reads = NS(id="steel"), 0
        empty = NS(count=0, item=lambda i: None)
        self.faces, self.edges, self.isSolid = empty, empty, True
        self.boundingBox = NS(minPoint=NS(x=0, y=0, z=0), maxPoint=NS(x=1, y=1, z=1))

    @property
    def physicalProperties(self):
        self.reads += 1
        return NS(volume=1, area=6, mass=7.8, centerOfMass=NS(x=0.5, y=0.5, z=0.5))


class Witnesses(unittest.TestCase):
    def test_an_unchanged_body_is_not_read_again(self):
        body = Body("Plate", NS(name="Bracket"))
        first = readback.topology_witness(body)
        self.assertEqual(readback.topology_witness(body), first)
        self.assertEqual(body.reads, 1)

    def test_a_changed_body_material_or_name_is_read_again(self):
        body = Body("Plate", NS(name="Bracket 2"))
        readback.topology_witness(body)
        body.revisionId = "b2"
        readback.topology_witness(body)
        body.material = NS(id="aluminum")
        readback.topology_witness(body)
        self.assertEqual(body.reads, 3)

    def test_a_cached_witness_is_a_copy(self):
        body = Body("Plate", NS(name="Bracket 3"))
        readback.topology_witness(body)["name"] = "changed by a caller"
        self.assertEqual(readback.topology_witness(body)["name"], "Plate")


class Feature:
    objectType = "adsk::fusion::BaseFeature"

    def __init__(self, name, component):
        self.name, self.parentComponent, self.isValid, self.index_reads = name, component, True, 0
        feature = self

        class Timeline:
            isSuppressed, isValid = False, True

            @property
            def index(self):
                feature.index_reads += 1
                raise RuntimeError("2 : InternalValidationError : res >= 0")
        self.timelineObject = Timeline()


class UnindexedFeatures(unittest.TestCase):
    def test_a_refused_timeline_index_is_asked_once_per_component_revision(self):
        comp = NS(name="Vendor model", revisionId="r1")
        feature = Feature("Base Feature1", comp)
        for _ in range(4):
            data = readback.feature_data(feature, order=0)
            self.assertEqual((data["index"], data["indexUnavailable"]), (None, "native_timeline_position_unavailable"))
        self.assertEqual(feature.index_reads, 1)
        comp.revisionId = "r2"   # the component changed: ask again
        readback.feature_data(feature, order=0)
        self.assertEqual(feature.index_reads, 2)



class Components(unittest.TestCase):
    def comp(self, name):
        return NS(objectType="adsk::fusion::Component", name=name, isValid=True)

    def design(self, comps, occurrences):
        reads = []
        collection = NS(count=len(comps), item=lambda i: reads.append(i) or comps[i])
        return NS(rootComponent=comps[0], allComponents=collection), reads

    def test_components_come_from_occurrences_without_reading_the_slow_collection(self):
        root, a, b = self.comp("root"), self.comp("A"), self.comp("B")
        occurrences = [NS(component=a), NS(component=b), NS(component=a)]
        design, reads = self.design([root, a, b], occurrences)
        found = core.components_by_occurrence(design, occurrences, 3)
        self.assertEqual([c.name for c in found], ["root", "A", "B"])
        self.assertEqual(reads, [])

    def test_a_count_mismatch_falls_back_to_the_full_list(self):
        root, a, orphan = self.comp("root"), self.comp("A"), self.comp("Orphan")
        design, reads = self.design([root, a, orphan], [NS(component=a)])
        found = core.components_by_occurrence(design, [NS(component=a)], 3)
        self.assertEqual([c.name for c in found], ["root", "A", "Orphan"])
        self.assertEqual(reads, [0, 1, 2])


class Level:
    """Occurrences under one parent, found by name the way Fusion's itemByName does."""
    def __init__(self, occurrences=()):
        self.values = list(occurrences)

    @property
    def count(self):
        return len(self.values)

    def item(self, index):
        return self.values[index]

    def itemByName(self, name):
        return next((o for o in self.values if o.name == name), None)


class Occ:
    objectType = "adsk::fusion::Occurrence"
    compared = 0

    def __init__(self, name, children=(), parent=None):
        self.name, self.isValid, self.parent = name, True, parent
        self.childOccurrences = Level(children)
        for child in children:
            child.parent = self

    @property
    def fullPathName(self):
        return (self.parent.fullPathName + "+" if self.parent else "") + self.name

    def __eq__(self, other):
        Occ.compared += 1
        return self is other

    __hash__ = object.__hash__


class Paths(unittest.TestCase):
    def tree(self):
        c = Occ("Nut:1")
        b = Occ("Bolt:1", [c])
        a = Occ("Frame:1", [b])
        plus = Occ("M3+washer:1", [Occ("Ring:2")])
        every = [a, b, c, plus, plus.childOccurrences.item(0)]

        class Everything(Level):
            reads = 0

            def item(self, index):
                Everything.reads += 1
                return self.values[index]
        all_occurrences = Everything(every)
        design = NS(rootComponent=NS(occurrences=Level([a, plus]), allOccurrences=all_occurrences))
        return design, a, b, c, plus, all_occurrences

    def test_an_occurrence_is_found_by_walking_its_path(self):
        design, a, b, c, plus, _ = self.tree()
        self.assertIs(core.occurrence_at(design, "Frame:1+Bolt:1+Nut:1"), c)
        self.assertIs(core.occurrence_at(design, "Frame:1"), a)
        self.assertIsNone(core.occurrence_at(design, "Frame:1+Washer:1"))
        self.assertIsNone(core.occurrence_at(design, ""))

    def test_a_name_holding_a_plus_is_still_found(self):
        design, a, b, c, plus, _ = self.tree()
        self.assertIs(core.occurrence_at(design, "M3+washer:1+Ring:2"), plus.childOccurrences.item(0))

    def test_resolving_a_context_does_not_read_every_occurrence(self):
        design, a, b, c, plus, every = self.tree()
        every.__class__.reads = 0
        self.assertIs(core.occurrence_context(design, c, "Frame:1+Bolt:1+Nut:1"), c)
        self.assertTrue(core.in_root_context(design, b))
        self.assertEqual(every.__class__.reads, 0)

    def test_an_occurrence_outside_the_root_context_still_reads_them_all(self):
        design, a, b, c, plus, every = self.tree()
        local = Occ("Nut:1")   # a definition-local occurrence: its path is relative to its parent
        every.__class__.reads = 0
        self.assertFalse(core.in_root_context(design, local))
        self.assertEqual(every.__class__.reads, every.count)


class JointPoses(unittest.TestCase):
    def test_pose_check_compares_each_occurrence_once(self):
        occurrences = [Occ("Part:%d" % i) for i in range(200)]
        for o in occurrences:
            o.transform2 = NS(asArray=lambda: [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1])
            o.isGrounded, o.isGroundToParent = False, False
        design = NS(rootComponent=NS(allOccurrences=Level(occurrences)))
        before = regular_joints.poses(design)
        Occ.compared = 0
        result = regular_joints.pose_result(design, before)
        self.assertEqual(result["changedOccurrences"], [])
        self.assertLessEqual(Occ.compared, len(occurrences))


class Part:
    """A component whose part number is a counted cloud read (about 0.5 s each in Fusion)."""
    objectType = "adsk::fusion::Component"

    def __init__(self, name):
        self.name, self.isValid, self.revisionId, self.reads = name, True, "r1", 0
        self.bRepBodies, self.occurrences, self.attributes, self.material = Level(), Level(), Level(), None

    @property
    def partNumber(self):
        self.reads += 1
        return "PN-" + self.name

    description = "a part"


class BomBudget(unittest.TestCase):
    def design(self, tag):
        parts = [Part("%s %d" % (tag, i)) for i in range(3)]
        occurrences = [NS(objectType="adsk::fusion::Occurrence", component=p, fullPathName=p.name + ":1", isLightBulbOn=True,
                          timelineObject=NS(isSuppressed=False), childOccurrences=Level(), bRepBodies=Level(), isConfiguration=False)
                       for p in parts]
        root = Part(tag + " root")
        root.occurrences, root.allOccurrences = Level(occurrences), Level(occurrences)
        design = NS(isConfiguredDesign=False, isConfiguration=False, configurationRowId="", rootComponent=root)
        return design, [root] + parts

    def generate(self, design, started):
        with unittest.mock.patch.object(bom, "ref", return_value={}):
            return bom.generate(design, {"documentId": "D", "revision": "R"}, {}, started, 40)

    def test_past_the_budget_part_numbers_are_left_for_the_next_run(self):
        design, comps = self.design("Late")
        result = self.generate(design, time.perf_counter() - 100)
        self.assertEqual(result["nativePending"], 4)
        self.assertEqual(sum(c.reads for c in comps), 0)
        self.assertIn("run bom again", result["nativeHint"])

    def test_inside_the_budget_every_part_number_is_read(self):
        design, comps = self.design("Early")
        result = self.generate(design, time.perf_counter())
        self.assertNotIn("nativePending", result)
        self.assertEqual([r["metadata"]["partNumber"] for r in result["instances"]], ["PN-" + c.name for c in comps])

    def test_what_an_earlier_run_read_is_used_past_the_budget(self):
        design, comps = self.design("Again")
        self.generate(design, time.perf_counter())
        result = self.generate(design, time.perf_counter() - 100)
        self.assertNotIn("nativePending", result)
        self.assertEqual(sum(c.reads for c in comps), 4)


if __name__ == "__main__":
    unittest.main()
