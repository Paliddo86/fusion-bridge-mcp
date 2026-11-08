"""Assembly step images, offline: the camera projection, the images in assembly order with later parts
parked out of view, only changed poses moved, and every way out of a run leaving the model and the
camera as they were."""
import importlib
from pathlib import Path
import tempfile
from types import SimpleNamespace as NS
import unittest
import unittest.mock

from test_mechanical import adsk

adsk.doEvents = lambda: None
adsk.core.CameraTypes = NS(OrthographicCameraType=0, PerspectiveCameraType=1)
st = importlib.import_module('mechanical_test_commands.mechanical.exploded_stills')
er = importlib.import_module('mechanical_test_commands.mechanical.exploded_render')
ev = importlib.import_module('mechanical_test_commands.mechanical.exploded_view')
core = importlib.import_module('mechanical_test_commands.mechanical.core')


def at(x, y, z):
    return [1., 0., 0., x, 0., 1., 0., y, 0., 0., 1., z, 0., 0., 0., 1.]


def point(x, y, z):
    return NS(x=x, y=y, z=z)


def camera(**extra):
    """Top-down orthographic camera, 20 cm across the short side (the scratch calibration's)."""
    return NS(eye=point(0, 0, 100), target=point(0, 0, 0), upVector=point(0, 1, 0), viewExtents=20.0,
              cameraType=1, isSmoothTransition=True, **extra)


class Projection(unittest.TestCase):
    """Fusion measured: cubes at (0,0), (8,3) and (-6,-4) cm land at these pixels in each image size."""
    def test_points_match_the_measured_renders(self):
        for (w, h), pixels in {(1280, 720): [(640, 360), (928, 252), (424, 504)],
                               (960, 960): [(480, 480), (864, 336), (192, 672)],
                               (720, 1280): [(360, 640), (648, 532), (144, 784)]}.items():
            view = st.View(camera(), w, h)
            for (x, y), (px, py) in zip([(0, 0), (80, 30), (-60, -40)], pixels):
                self.assertEqual(view.point([x, y, 5]), [round(px / w, 5), round(py / h, 5)])

    def test_a_box_covers_its_corners(self):
        view = st.View(camera(), 1280, 720)
        self.assertEqual(view.box([75, 25, 0], [85, 35, 10]), [view.point([75, 35, 0])[0], view.point([75, 35, 0])[1],
                                                               view.point([85, 25, 0])[0], view.point([85, 25, 0])[1]])

    def test_a_camera_looking_along_its_up_vector_is_refused(self):
        with self.assertRaises(core.MechanicalError) as error:
            st.View(NS(eye=point(0, 0, 10), target=point(0, 0, 0), upVector=point(0, 0, 1), viewExtents=20.0), 1280, 720)
        self.assertEqual(error.exception.code, "camera_invalid")


class Camera(unittest.TestCase):
    def test_parts_coming_in_along_y_are_shown_from_their_side_and_across_the_screen(self):
        current = {"forward": [-0.2, -0.9, -0.3], "up": [0.0, 0.1, 0.99]}
        forward, up = st.choose_direction(current, [([0.0, 1.0, 0.0], 92)])
        third = 1 / 3 ** 0.5
        self.assertEqual(up, [0.0, 0.0, 1.0])
        self.assertEqual([round(v, 6) for v in forward], [round(-third, 6)] * 3)  # nearest the current view of the two equal ones

    def test_a_part_dropping_straight_down_is_seen_from_above_at_an_angle(self):
        forward, _ = st.choose_direction({"forward": [0.0, 0.0, -1.0], "up": [0.0, 1.0, 0.0]}, [([0.0, 0.0, 1.0], 1)])
        self.assertLess(forward[2], 0.0)  # z is not up here, so the view is not a top view
        forward, _ = st.choose_direction({"forward": [1.0, 1.0, -1.0], "up": [0.0, 0.0, 1.0]}, [([0.0, 0.0, 1.0], 1)])
        self.assertLess(forward[2], 0.0)

    def test_framing_fits_every_corner_with_a_margin(self):
        corners = [[x, y, 0.0] for x in (-100.0, 100.0) for y in (-25.0, 25.0)]
        center, true_up, extents, diagonal = st.frame([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], corners, 1280, 720)
        self.assertEqual(center, [0.0, 0.0, 0.0])
        self.assertAlmostEqual(extents, st.MARGIN * 200.0 * 720 / 1280)  # the width binds on a wide image
        _, _, extents, _ = st.frame([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], corners, 720, 1280)
        self.assertAlmostEqual(extents, st.MARGIN * 200.0)  # on a tall image the short side is the width


class Context(unittest.TestCase):
    """A panel inside a cell: when every moved part is in the panel, the rest of the cell and the other
    visible root occurrences go out of view; parts at the root hide nothing."""
    @staticmethod
    def occ(name, path, children=(), visible=True):
        kids = list(children)
        return NS(name=name, fullPathName=path, isLightBulbOn=visible, childOccurrences=NS(count=len(kids), item=lambda i: kids[i]))

    def design(self):
        o = self.occ
        panel = o("Panel:1", "Cell:1+Panel:1", [o("Block:1", "Cell:1+Panel:1+Block:1"), o("Block:2", "Cell:1+Panel:1+Block:2")])
        cell = o("Cell:1", "Cell:1", [panel, o("Frame:1", "Cell:1+Frame:1"), o("Table:1", "Cell:1+Table:1")])
        roots = [cell, o("Person:1", "Person:1"), o("Floor:1", "Floor:1", visible=False)]
        return NS(rootComponent=NS(occurrences=NS(count=len(roots), item=lambda i: roots[i])))

    def test_everything_outside_the_panel_and_still_visible_goes(self):
        out = st.context_paths(self.design(), ["Cell:1+Panel:1+Block:1", "Cell:1+Panel:1+Block:2"])
        self.assertEqual(sorted(out), ["Cell:1+Frame:1", "Cell:1+Table:1", "Person:1"])

    def test_parts_at_the_root_or_in_different_subassemblies_hide_nothing(self):
        self.assertEqual(st.context_paths(self.design(), ["Cell:1", "Person:1"]), [])
        self.assertEqual(st.context_paths(self.design(), ["Cell:1+Panel:1+Block:1", "Cell:1+Frame:1"]), ["Person:1"])


class Centering(unittest.TestCase):
    def test_the_camera_centres_on_what_it_sees(self):
        third = 1 / 3 ** 0.5
        forward, up = [-third] * 3, [0.0, 0.0, 1.0]
        corners = [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 0.0, 100.0], [100.0, 0.0, 100.0], [0.0, 20.0, 0.0]]
        center, true_up, extents, _ = st.frame(forward, up, corners, 1280, 720)
        right = st.unit(st.cross(forward, up))
        dot = lambda a, b: sum(x * y for x, y in zip(a, b))
        xs, ys = [dot(c, right) for c in corners], [dot(c, true_up) for c in corners]
        self.assertAlmostEqual(dot(center, right), (max(xs) + min(xs)) / 2)
        self.assertAlmostEqual(dot(center, true_up), (max(ys) + min(ys)) / 2)


class Occurrence:
    def __init__(self, scene, key, home):
        self.scene, self.key, self.home, self._t = scene, key, home, at(*home)

    @property
    def transform2(self):
        return NS(asArray=lambda: list(self._t))

    @transform2.setter
    def transform2(self, matrix):
        self.scene.snapshots.hasPendingSnapshot = True
        self.scene.document.isModified = True  # Fusion marks the document modified on any move
        self.scene.sets.append(self.key)
        self._t = list(matrix)


class Scene:
    """Two assembly steps: a bracket (A) comes down 50 mm onto the base, then a cover (B) slides 30 mm
    in along y. Plans and boxes are in mm; Fusion matrices in cm."""
    def __init__(self, pending=False, fail_at=None):
        self.document = NS(isSaved=True, isModified=False)
        self.snapshots = NS(hasPendingSnapshot=pending)
        self.design = NS(snapshots=self.snapshots)
        self.sets, self.saved, self.states = [], [], []
        self.occ = {"A": Occurrence(self, "A", (8.0, 3.0, 0.0)), "B": Occurrence(self, "B", (-6.0, -4.0, 0.0))}

        def revert():
            self.snapshots.hasPendingSnapshot = False
            for o in self.occ.values():
                o._t = at(*o.home)
        self.snapshots.revertPendingSnapshot = revert
        self.original = camera(name="original")
        self.viewport = NS(camera=self.original, refresh=lambda: None, saveAsImageFile=self._save)
        self.app = NS(activeViewport=self.viewport)
        self.fail_at = fail_at

    def _save(self, path, w, h):
        if self.fail_at is not None and len(self.saved) == self.fail_at:
            return False
        Path(path).write_bytes(b"png")
        self.saved.append(Path(path).name)
        self.states.append({k: (round(o._t[3] * 10, 3), round(o._t[7] * 10, 3), round(o._t[11] * 10, 3)) for k, o in self.occ.items()})
        return True

    def run(self, folder, **args):
        rows = [{"instanceId": k, "path": k + ":1", "componentName": k, "homeMatrix": at(o.home[0] * 10, o.home[1] * 10, o.home[2] * 10),
                 "parentInstanceId": None} for k, o in self.occ.items()]
        snapshot = {"instances": rows, "rootBodyGroup": {}}
        # explode order: the cover (last assembled) comes off first
        compiled = [{"index": 0, "title": "Cover", "instruction": "", "startSeconds": 0.0, "endSeconds": 1.0, "durationSeconds": 1.0,
                     "moves": [{"instanceId": "B", "worldTranslation": [0.0, 30.0, 0.0]}]},
                    {"index": 1, "title": "Bracket", "instruction": "", "startSeconds": 1.0, "endSeconds": 2.0, "durationSeconds": 1.0,
                     "moves": [{"instanceId": "A", "worldTranslation": [0.0, 0.0, 50.0]}]}]
        boxes = {"A": ([75.0, 25.0, 0.0], [85.0, 35.0, 10.0]), "B": ([-65.0, -45.0, 0.0], [-55.0, -35.0, 10.0])}

        def capture(design, value, scope):
            home = all(o._t == at(*o.home) for o in self.occ.values())
            return (snapshot if home else {"instances": [dict(r, homeMatrix=[0.0] * 16) for r in rows]}), {k: o for k, o in self.occ.items()}
        patches = [unittest.mock.patch.object(st, "SESSION_ID", "s"),
                   unittest.mock.patch.object(st, "target", lambda app, a: (self.document, self.design, {"documentId": "d", "revision": "r"})),
                   unittest.mock.patch.object(st, "current_target", lambda app: {"documentId": "d", "revision": "after"}),
                   unittest.mock.patch.object(st, "capture", capture),
                   unittest.mock.patch.object(st, "moved_scope", lambda design, value, steps, limit: {"A:1", "B:1"}),
                   unittest.mock.patch.object(st, "compile_steps", lambda *a: compiled),
                   unittest.mock.patch.object(st, "world_box", lambda occurrence: boxes[occurrence.key]),
                   unittest.mock.patch.object(er, "to_matrix", lambda pose: [v / 10 if i in (3, 7, 11) else v for i, v in enumerate(pose)])]
        for p in patches:
            p.start()
        try:
            move = {"occurrence": {}, "direction": [0, 0, 1], "distance": 50, "frame": "world"}
            return st.exploded_view_stills(self.app, {"sessionId": "s", "unit": "mm", "folder": str(folder), "camera": "current",
                                                      "steps": [{"title": "Bracket", "moves": [move]}, {"title": "Cover", "moves": [dict(move, direction=[0, 1, 0], distance=30)]}],
                                                      **args})
        finally:
            for p in patches:
                p.stop()


class StepImages(unittest.TestCase):
    def setUp(self):
        self.folder = Path(tempfile.mkdtemp()) / "stills"

    def test_images_come_in_assembly_order_with_later_parts_parked(self):
        scene = Scene()
        out = scene.run(self.folder)
        self.assertEqual(scene.saved, ["step_000_after.png", "step_001_before.png", "step_001_after.png",
                                       "step_002_before.png", "step_002_after.png"])
        park = 800.0  # four view extents (200 mm) along the camera's right
        A, B = (80.0, 30.0, 0.0), (-60.0, -40.0, 0.0)
        self.assertEqual(scene.states, [
            {"A": (A[0] + park, A[1], 50.0), "B": (B[0] + park, B[1] + 30, 0.0)},   # nothing assembled
            {"A": (A[0], A[1], 50.0), "B": (B[0] + park, B[1] + 30, 0.0)},          # bracket hovering, cover away
            {"A": A, "B": (B[0] + park, B[1] + 30, 0.0)},                           # bracket seated
            {"A": A, "B": (B[0], B[1] + 30, 0.0)},                                  # cover hovering
            {"A": A, "B": B}])                                                      # cover seated
        self.assertEqual([s["title"] for s in out["steps"]], ["Bracket", "Cover"])
        self.assertEqual(out["steps"][1]["context"], "step_001_after.png")
        cover = out["steps"][1]["instances"][0]
        self.assertEqual((cover["path"], cover["before"]["point"], cover["after"]["point"]),
                         ("B:1", [round(424 / 1280, 5), round(396 / 720, 5)], [round(424 / 1280, 5), round(504 / 720, 5)]))
        # park both, A in, A down, B in, B down
        self.assertEqual((out["moves"], len(scene.sets)), (6, 6))
        self.assertEqual((scene.snapshots.hasPendingSnapshot, scene.viewport.camera.name, out["restored"]), (False, "original", True))
        self.assertEqual(out["documentModified"], True)  # it was saved and unmodified; the moves set the flag
        self.assertEqual((out["target"]["revision"], out["sourceTarget"]["revision"]), ("after", "r"))
        self.assertTrue(all(o._t == at(*o.home) for o in scene.occ.values()))

    def test_a_failed_image_still_reverts_every_move_and_restores_the_camera(self):
        scene = Scene(fail_at=2)
        with self.assertRaises(core.MechanicalError) as error:
            scene.run(self.folder)
        self.assertEqual(error.exception.code, "render_failed")
        self.assertEqual((scene.snapshots.hasPendingSnapshot, scene.viewport.camera.name), (False, "original"))
        self.assertTrue(all(o._t == at(*o.home) for o in scene.occ.values()))

    def test_someone_elses_uncaptured_move_is_refused_before_anything_moves(self):
        scene = Scene(pending=True)
        with self.assertRaises(core.MechanicalError) as error:
            scene.run(self.folder)
        self.assertEqual((error.exception.code, scene.sets), ("pending_snapshot", []))

    def test_images_are_never_mixed_with_an_older_run(self):
        Scene().run(self.folder)
        with self.assertRaises(core.MechanicalError) as error:
            Scene().run(self.folder)
        self.assertEqual(error.exception.code, "destination_exists")
        self.assertEqual(len(Scene().run(self.folder, overwrite=True)["steps"]), 2)

    def test_bigger_plans_than_an_animation_are_accepted(self):
        move = {"occurrence": {}, "direction": [0, 0, 1], "distance": 5, "frame": "world"}
        steps = [{"durationSeconds": 1.0, "moves": [move] * 30} for _ in range(40)]
        self.assertEqual(ev.request_arguments({"unit": "mm", "steps": steps}, st.MAX_STEPS, st.MAX_MOVES)[0], 40.0)
        with self.assertRaises(core.MechanicalError):
            ev.request_arguments({"unit": "mm", "steps": steps})


class Mover(unittest.TestCase):
    """Only changed poses are set, and a moved parent carries its tracked child's applied offset."""
    def setUp(self):
        self.sets = []
        test = self

        class Handle:
            def __init__(self, key):
                self.key = key

            @property
            def transform2(self):
                return None

            @transform2.setter
            def transform2(self, value):
                test.sets.append(self.key)
        rows = [{"instanceId": "P", "homeMatrix": at(0, 0, 0)}, {"instanceId": "C", "parentInstanceId": "P", "homeMatrix": at(10, 0, 0)}]
        self.patch = unittest.mock.patch.object(er, "to_matrix", lambda pose: pose)
        self.patch.start()
        self.mover = er.Mover({"instances": rows}, {"P": Handle("P"), "C": Handle("C")})

    def tearDown(self):
        self.patch.stop()

    def test_unchanged_poses_are_not_set_again(self):
        self.mover.apply({"P": at(0, 0, 5), "C": at(10, 0, 5)})
        self.mover.apply({"P": at(0, 0, 5), "C": at(10, 0, 5)})
        self.assertEqual(self.sets, ["P"])  # the child rode along with its parent

    def test_a_child_that_moves_against_its_parent_is_still_set(self):
        self.mover.apply({"P": at(0, 0, 5), "C": at(10, 0, 0)})
        self.assertEqual(self.sets, ["P", "C"])
        self.assertEqual(self.mover.applied["C"], [0.0, 0.0, 0.0])


if __name__ == "__main__":
    unittest.main()
