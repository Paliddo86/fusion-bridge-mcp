"""Exploded-view render, offline: timing, captions, and every way out of a render (a frame that fails,
a design with someone's uncaptured move, a part held back) leaves the model and the camera as they were.
render_image has its own file, test_mechanical_render_image.py."""
import importlib
from pathlib import Path
import tempfile
from types import SimpleNamespace as NS
import unittest
import unittest.mock

from test_mechanical import adsk

adsk.doEvents = lambda: None
adsk.core.ViewOrientations = NS(IsoTopRightViewOrientation=7, FrontViewOrientation=3)
r = importlib.import_module('mechanical_test_commands.mechanical.exploded_render')
core = importlib.import_module('mechanical_test_commands.mechanical.core')
HOME = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.]


class Timing(unittest.TestCase):
    def test_frames_cover_the_plan_then_hold_the_last_pose(self):
        times = r.frame_times(2.0, 4, 0.5)
        self.assertEqual(times[:9], [0.0, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0])
        self.assertEqual(times[9:], [2.0, 2.0])

    def test_a_duration_between_frames_still_ends_on_the_last_pose(self):
        self.assertEqual(r.frame_times(1.1, 2, 0)[-1], 1.1)


class Captions(unittest.TestCase):
    steps = [{"index": 0, "title": "Lift the lid", "instruction": "Two screws", "startSeconds": 0.0, "endSeconds": 2.0},
             {"index": 1, "title": "Slide out the tray", "instruction": "", "startSeconds": 2.0, "endSeconds": 5.0}]

    def test_explode_keeps_the_step_order(self):
        self.assertEqual([(c["title"], c["startSeconds"], c["endSeconds"]) for c in r.captions(self.steps, 5.0, "explode")],
                         [("Lift the lid", 0.0, 2.0), ("Slide out the tray", 2.0, 5.0)])

    def test_assemble_plays_the_steps_backwards(self):
        self.assertEqual([(c["title"], c["startSeconds"], c["endSeconds"]) for c in r.captions(self.steps, 5.0, "assemble")],
                         [("Slide out the tray", 0.0, 3.0), ("Lift the lid", 3.0, 5.0)])


class Pose:
    def __init__(self, values):
        self.values = list(values)

    def asArray(self):
        return list(self.values)


class Occurrence:
    """Setting transform2 moves it and leaves a pending snapshot, unless something holds it back."""
    def __init__(self, design, stuck=False):
        self.design, self.stuck, self._t = design, stuck, Pose(HOME)

    @property
    def transform2(self):
        return self._t

    @transform2.setter
    def transform2(self, pose):
        self.design.snapshots.hasPendingSnapshot = True
        self.design.moves += 1
        self.design.document.isModified = True  # Fusion marks the document modified on any move
        if not self.stuck:
            self._t = pose


class Scene:
    """A design with one instance that the plan moves 50 mm along x, a viewport and its camera."""
    def __init__(self, pending=False, stuck=False, fail_at=None):
        self.document = NS(isSaved=False, isModified=True)
        self.snapshots = NS(hasPendingSnapshot=pending, reverts=0)
        self.design = NS(snapshots=self.snapshots, moves=0, document=self.document)

        def revert():
            self.snapshots.reverts += 1
            self.snapshots.hasPendingSnapshot = False
            self.occ._t = Pose(HOME)
        self.snapshots.revertPendingSnapshot = revert
        self.occ = Occurrence(self.design, stuck)
        self.original_camera = NS(isSmoothTransition=True, name="original")
        self.saved = []

        def save(path, w, h):
            if fail_at is not None and len(self.saved) == fail_at:
                return False
            Path(path).write_bytes(b"png")
            self.saved.append(path)
            return True
        self.viewport = NS(camera=self.original_camera, fit=lambda: None, refresh=lambda: None, saveAsImageFile=save)
        self.app = NS(activeViewport=self.viewport)
        self.snapshot = {"instances": [{"instanceId": "i1", "path": "Lid:1", "homeMatrix": list(HOME)}], "rootBodyGroup": {}}

    def poses(self, seconds):
        """The plan's pose in mm: 50 mm along x at the end of its one-second step."""
        pose = list(HOME)
        pose[3] = 50.0 * min(seconds, 1.0)
        return [{"timeSeconds": seconds, "poses": [{"instanceId": "i1", "matrix": pose}]}]

    def run(self, folder, **args):
        steps = [{"index": 0, "title": "Lift", "instruction": "", "startSeconds": 0.0, "endSeconds": 1.0, "durationSeconds": 1.0, "moves": []}]
        captured = []

        def capture(design, value, scope):
            captured.append(1)
            return ({"instances": [{"instanceId": "i1", "path": "Lid:1", "homeMatrix": list(HOME)}], "rootBodyGroup": {}}
                    if self.occ._t.values == HOME else {"moved": True}), {"i1": self.occ}
        patches = [unittest.mock.patch.object(r, "SESSION_ID", "s"),
                   unittest.mock.patch.object(r, "target", lambda app, a: (self.document, self.design, {"documentId": "d"})),
                   unittest.mock.patch.object(r, "current_target", lambda app: {"documentId": "d", "revision": "after"}),
                   unittest.mock.patch.object(r, "capture", capture),
                   unittest.mock.patch.object(r, "moved_scope", lambda design, value, steps: {"Lid:1"}),
                   unittest.mock.patch.object(r, "compile_steps", lambda *a: steps),
                   unittest.mock.patch.object(r, "sample_plan", lambda snap, st, times, total, playback: self.poses(times[0])),
                   unittest.mock.patch.object(r, "to_matrix", lambda pose: Pose([v / 10 if i in (3, 7, 11) else v for i, v in enumerate(pose)]))]
        for p in patches:
            p.start()
        try:
            return r.exploded_view_render(self.app, {"sessionId": "s", "unit": "mm", "steps": [{"durationSeconds": 1.0, "moves": [
                {"occurrence": {}, "direction": [1, 0, 0], "distance": 50, "frame": "world"}]}], "folder": str(folder), "fps": 4,
                "holdSeconds": 0, **args})
        finally:
            for p in patches:
                p.stop()


class RenderExits(unittest.TestCase):
    def setUp(self):
        self.folder = Path(tempfile.mkdtemp()) / "frames"

    def test_a_clean_render_writes_every_frame_and_puts_everything_back(self):
        scene = Scene()
        out = scene.run(self.folder)
        self.assertEqual((out["frames"], out["heldBack"], out["restored"]), (5, [], True))
        self.assertEqual((scene.snapshots.reverts, scene.viewport.camera.name, scene.occ._t.values), (1, "original", HOME))
        self.assertEqual(len(list(self.folder.glob("frame_*.png"))), 5)

    def test_a_saved_design_the_moves_mark_modified_still_renders_and_says_so(self):
        scene = Scene()
        scene.document.isSaved, scene.document.isModified = True, False
        out = scene.run(self.folder)
        self.assertEqual((out["restored"], out["documentModified"]), (True, True))
        self.assertIn("nothing to save", out["documentModifiedNote"])

    def test_a_frame_that_fails_still_reverts_every_move_and_restores_the_camera(self):
        scene = Scene(fail_at=2)
        with self.assertRaises(core.MechanicalError) as error:
            scene.run(self.folder)
        self.assertEqual(error.exception.code, "render_failed")
        self.assertEqual((scene.snapshots.reverts, scene.snapshots.hasPendingSnapshot, scene.viewport.camera.name, scene.occ._t.values),
                         (1, False, "original", HOME))

    def test_someone_elses_uncaptured_move_is_refused_before_anything_moves(self):
        scene = Scene(pending=True)
        with self.assertRaises(core.MechanicalError) as error:
            scene.run(self.folder)
        self.assertEqual(error.exception.code, "pending_snapshot")
        self.assertEqual((scene.design.moves, scene.snapshots.reverts, scene.snapshots.hasPendingSnapshot), (0, 0, True))

    def test_a_part_held_back_is_named(self):
        out = Scene(stuck=True).run(self.folder)
        self.assertEqual([h["path"] for h in out["heldBack"]], ["Lid:1"])
        self.assertEqual(out["heldBack"][0]["offByMm"], 50.0)

    def test_frames_are_never_mixed_with_an_older_render(self):
        Scene().run(self.folder)
        (self.folder / "frame_00099.png").write_bytes(b"stale")
        with self.assertRaises(core.MechanicalError) as error:
            Scene().run(self.folder)
        self.assertEqual(error.exception.code, "destination_exists")
        Scene().run(self.folder, overwrite=True)
        self.assertEqual(sorted(p.name for p in self.folder.glob("frame_*.png"))[-1], "frame_00004.png")

    def test_too_many_frames_or_a_relative_folder_is_refused_before_touching_the_model(self):
        scene = Scene()
        with self.assertRaises(core.MechanicalError) as error:
            r.exploded_view_render(scene.app, {"sessionId": r.SESSION_ID, "unit": "mm", "folder": "frames", "steps": []})
        self.assertEqual(error.exception.code, "invalid_destination")
        with unittest.mock.patch.object(r, "request_arguments", lambda a: (100.0, [0.0, 100.0], "explode")), \
                self.assertRaises(core.MechanicalError) as error:
            r.exploded_view_render(scene.app, {"sessionId": r.SESSION_ID, "unit": "mm", "folder": str(self.folder), "fps": 60, "steps": []})
        self.assertEqual((error.exception.code, scene.design.moves), ("plan_budget", 0))


class Scope(unittest.TestCase):
    """Only moved occurrences and their parents are tracked, so a 4,579-occurrence assembly fits."""
    def test_a_nested_move_tracks_the_part_and_its_parents_only(self):
        paths = ["Cabinet:1", "Drawer:1", "Drawer:1+Handle:1", "Drawer:1+Box:1", "Shelf:1+Bolt:7"]
        occurrences = {p: NS(fullPathName=p, component=NS(name=p.split("+")[-1].split(":")[0]), transform2=Pose(HOME)) for p in paths}
        design = NS(rootComponent=NS(allOccurrences=NS(count=len(paths), item=lambda i: occurrences[paths[i]])))
        steps = [{"moves": [{"occurrence": "Drawer:1+Handle:1"}]}]
        with unittest.mock.patch.object(r, "resolve", lambda d, v, ref, kinds: occurrences[ref]), \
                unittest.mock.patch.object(r, "concrete", lambda x: x), unittest.mock.patch.object(r, "identity", lambda c: c.name), \
                unittest.mock.patch.object(r, "ref", lambda o, v: o.fullPathName):
            scope = r.moved_scope(design, {"documentId": "d"}, steps)
            self.assertEqual(scope, {"Drawer:1", "Drawer:1+Handle:1"})
            snapshot, handles = r.capture(design, {"documentId": "d"}, scope)
        rows = snapshot["instances"]
        self.assertEqual([row["path"] for row in rows], ["Drawer:1", "Drawer:1+Handle:1"])
        self.assertEqual(rows[1]["parentInstanceId"], rows[0]["instanceId"])
        self.assertIsNone(rows[0]["parentInstanceId"])

    def test_too_many_moved_parts_are_refused(self):
        # the same bound as stills and moved-scope plans: 4,096 occurrences with their parents
        with unittest.mock.patch.object(r, "resolve", lambda d, v, ref, kinds: NS(fullPathName=ref)):
            self.assertEqual(len(r.moved_scope(None, {}, [{"moves": [{"occurrence": "Box:1+P%d:1" % i} for i in range(300)]}])), 301)
            with self.assertRaises(core.MechanicalError) as error:
                r.moved_scope(None, {}, [{"moves": [{"occurrence": "P%d:1" % i} for i in range(4097)]}])
        self.assertEqual((error.exception.code, error.exception.details["tracked"]), ("plan_budget", 4097))
        self.assertIn("parent subassembly", error.exception.details["hint"])


class Carry(unittest.TestCase):
    def test_moving_a_parent_carries_its_tracked_children_and_grandchildren(self):
        sets = []

        class Occ:
            def __init__(self, key):
                self.key = key

            @property
            def transform2(self):
                return Pose(HOME)

            @transform2.setter
            def transform2(self, matrix):
                sets.append(self.key)
        rows = [{"instanceId": "G", "parentInstanceId": None, "homeMatrix": HOME},
                {"instanceId": "P", "parentInstanceId": "G", "homeMatrix": HOME},
                {"instanceId": "C", "parentInstanceId": "P", "homeMatrix": HOME}]
        mover = r.Mover({"instances": rows}, {key: Occ(key) for key in "GPC"})
        self.assertEqual(mover.below, {"G": ["P", "C"], "P": ["C"], "C": []})
        at = lambda x: [1., 0., 0., x, 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.]
        with unittest.mock.patch.object(r, "to_matrix", lambda pose: pose):
            mover.apply({"G": at(10), "P": at(10), "C": at(10)})  # the parent's move carries both below it
            self.assertEqual((sets, mover.moves), (["G"], 1))
            mover.apply({"G": at(10), "P": at(10), "C": at(15)})
            self.assertEqual((sets, mover.moves), (["G", "C"], 2))


class Drift(unittest.TestCase):
    def rows(self, x):
        pose = list(HOME); pose[3] = x
        return {"instances": [{"path": "Lid:1", "homeMatrix": pose}]}

    def test_float_noise_after_a_revert_still_counts_as_home(self):
        self.assertLess(r.home_drift(self.rows(12.0), self.rows(12.0 + 1e-9)), r.POSE_TOLERANCE_MM)

    def test_a_real_move_or_a_missing_instance_does_not(self):
        self.assertEqual(r.home_drift(self.rows(12.0), self.rows(12.5)), 0.5)
        self.assertIsNone(r.home_drift(self.rows(12.0), {"instances": []}))


if __name__ == "__main__":
    unittest.main()
