"""geometry_query's `near` filter, offline: faces and edges are measured on a bounded temporary copy, moved
into the root frame for root-space queries, never on an occurrence-context proxy. Fusion measures a point to a
proxy face's underlying surface (a proxy edge's underlying curve), so a coplanar face or a collinear edge beside
the pick read 0 mm away and the filter kept it (live, 1.13.61: a box whose top face a groove splits in two)."""
import importlib
import math
from types import SimpleNamespace as NS
import unittest
from unittest.mock import patch

import test_mechanical  # noqa: F401  (installs the fake adsk and the test package)
import adsk

g = importlib.import_module('mechanical_test_commands.mechanical.geometry')


def pt(x, y, z):
    return NS(x=x, y=y, z=z)


class Rect:
    """An axis-aligned face on the plane z = z0 (cm), or, with y0 == y1 and z fixed, an edge along x."""
    def __init__(self, x, y, z):
        self.x, self.y, self.z = x, y, z   # (min, max) ranges; a plane or line when a range is (-inf, inf)

    def nearest(self, p, offset=(0, 0, 0)):
        clamp = lambda v, lo, hi: min(max(v, lo), hi)
        return pt(clamp(p.x - offset[0], *self.x) + offset[0], clamp(p.y - offset[1], *self.y) + offset[1],
                  clamp(p.z - offset[2], *self.z) + offset[2])


INF = (-math.inf, math.inf)


class Native:
    """A native face or edge. Its proxy measures like Fusion's: to the unbounded surface or curve."""
    def __init__(self, bounded, unbounded):
        self.bounded, self.unbounded, self.proxied = bounded, unbounded, []

    def createForAssemblyContext(self, occurrence):
        self.proxied.append(occurrence)
        return NS(kind="proxy", shape=self.unbounded, offset=occurrence.offset)


class Temporary:
    def __init__(self):
        self.copies, self.moves = [], []

    def copy(self, entity):
        self.copies.append(entity)
        return NS(kind="copy", shape=entity.bounded, offset=(0, 0, 0))

    def transform(self, body, matrix):
        self.moves.append(matrix)
        body.offset = tuple(matrix.asArray()[i] for i in (3, 7, 11))
        return True


class Manager:
    def __init__(self):
        self.measured = []

    def measureMinimumDistance(self, p, candidate):
        self.measured.append(candidate.kind)
        w = candidate.shape.nearest(p, candidate.offset)
        return NS(isValid=True, value=math.dist((p.x, p.y, p.z), (w.x, w.y, w.z)), positionOne=p, positionTwo=w)


def occurrence(dx=0.0):
    """A root occurrence translated dx cm along x."""
    m = [1, 0, 0, dx, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]
    return NS(offset=(dx, 0, 0), transform2=NS(asArray=lambda: m))


class NearFilter(unittest.TestCase):
    def setUp(self):
        self.temporary = Temporary()
        for change in (patch.object(adsk.core, 'Point3D', NS(create=lambda x, y, z: pt(x, y, z)), create=True),
                       patch.object(adsk.fusion, 'TemporaryBRepManager', NS(get=lambda: self.temporary), create=True),
                       patch.object(g, 'owner', lambda entity: 'child')):
            change.start()
            self.addCleanup(change.stop)
        self.design = NS(rootComponent='root')
        # the probe's box top, split by a groove: y 0..1 cm and y 1.5..3 cm, both on z = 2 cm
        self.near_face = Native(Rect((0, 3), (0, 1), (2, 2)), Rect(INF, INF, (2, 2)))
        self.far_face = Native(Rect((0, 3), (1.5, 3), (2, 2)), Rect(INF, INF, (2, 2)))
        # the top edge along x = 0 split the same way: y 0..1 cm and y 1.5..3 cm
        self.near_edge = Native(Rect((0, 0), (0, 1), (2, 2)), Rect((0, 0), INF, (2, 2)))
        self.far_edge = Native(Rect((0, 0), (1.5, 3), (2, 2)), Rect((0, 0), INF, (2, 2)))

    def near(self, entity, kind, pick, frame):
        manager = Manager()
        distance, nearest = g.near_geometry(manager, entity, kind, pick, frame, self.design)
        return distance, nearest, manager.measured

    def test_root_space_keeps_only_the_face_under_the_pick(self):
        frame = occurrence()
        on, _, measured = self.near(self.near_face, "faces", [5, 5, 20], frame)
        off, nearest, _ = self.near(self.far_face, "faces", [5, 5, 20], frame)
        self.assertEqual(on, 0)
        self.assertAlmostEqual(off, 10)          # the coplanar face is 10 mm away, not 0
        self.assertEqual(nearest, [5, 15, 20])
        self.assertEqual(measured, ["copy"])     # never the proxy
        self.assertEqual(self.near_face.proxied + self.far_face.proxied, [])

    def test_root_space_keeps_only_the_edge_under_the_pick(self):
        frame = occurrence()
        on, _, _ = self.near(self.near_edge, "edges", [0, 5, 20], frame)
        off, nearest, _ = self.near(self.far_edge, "edges", [0, 5, 20], frame)
        self.assertEqual(on, 0)
        self.assertAlmostEqual(off, 10)
        self.assertEqual(nearest, [0, 15, 20])

    def test_root_space_moves_the_copy_by_the_occurrence_transform(self):
        frame = occurrence(dx=10.0)              # the instance sits 100 mm along x
        here, nearest, _ = self.near(self.near_face, "faces", [105, 5, 20], frame)
        there, _, _ = self.near(self.near_face, "faces", [5, 5, 20], frame)
        self.assertEqual(here, 0)
        self.assertEqual(nearest, [105, 5, 20])
        self.assertAlmostEqual(there, 95)        # 5 mm is 95 mm short of the moved face's x 100..130
        self.assertEqual(len(self.temporary.moves), 2)

    def test_component_space_measures_an_unmoved_copy(self):
        distance, _, measured = self.near(self.far_face, "faces", [5, 5, 20], None)
        self.assertAlmostEqual(distance, 10)
        self.assertEqual(measured, ["copy"])
        self.assertEqual(self.temporary.moves, [])

    def test_vertices_still_use_the_instance_or_its_point(self):
        vertex = NS(createForAssemblyContext=lambda occ: NS(kind="proxy", shape=Rect((0, 0), (0, 0), (2, 2)), offset=occ.offset),
                    geometry=NS(kind="point", shape=Rect((0, 0), (0, 0), (2, 2)), offset=(0, 0, 0)))
        _, _, measured = self.near(vertex, "vertices", [0, 5, 20], occurrence())
        self.assertEqual(measured, ["proxy"])
        _, _, measured = self.near(vertex, "vertices", [0, 5, 20], None)
        self.assertEqual(measured, ["point"])
        self.assertEqual(self.temporary.copies, [])


if __name__ == "__main__":
    unittest.main()
