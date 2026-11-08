"""Clean captures, offline: the viewport captures hide joint, joint origin, sketch, origin and
construction glyphs through the design's Object Visibility for every image, put back exactly the
switches they turned off on every way out (success, a failed image, a timeout), keep everything as
it is with glyphs:true, and refuse with restore_failed when a switch does not come back. render_image
hides them only while it fits a standard view (Fusion's renderer never draws them)."""
import importlib
from pathlib import Path
import tempfile
from types import SimpleNamespace as NS
import unittest
import unittest.mock

from test_mechanical import adsk  # noqa: F401  (installs the fake adsk and the package path)
import test_mechanical_exploded_render as render_tests
import test_mechanical_exploded_stills as stills_tests
import test_mechanical_render_image as render_image_tests

g = importlib.import_module('mechanical_test_commands.mechanical.glyphs')
core = importlib.import_module('mechanical_test_commands.mechanical.core')
ri = importlib.import_module('mechanical_test_commands.mechanical.render_image')

ALL = [attr for attr, _ in g.SWITCHES]
NAMES = [name for _, name in g.SWITCHES]


class Visibility:
    """Design.objectVisibility: ten switches, all on unless told otherwise. stuck names a switch
    that refuses to turn back on; side_effect(attr, value, self) runs after every set."""
    def __init__(self, off=(), stuck=None, side_effect=None):
        object.__setattr__(self, "values", {attr: attr not in off for attr in ALL})
        object.__setattr__(self, "sets", [])
        object.__setattr__(self, "stuck", stuck)
        object.__setattr__(self, "side_effect", side_effect)

    def __getattr__(self, attr):
        if attr in ALL:
            return self.values[attr]
        raise AttributeError(attr)

    def __setattr__(self, attr, value):
        assert attr in ALL, attr
        if value and attr == self.stuck:
            raise RuntimeError("3 : the switch cannot be set")
        self.sets.append((attr, value))
        self.values[attr] = value
        if self.side_effect:
            self.side_effect(attr, value, self)

    def state(self):
        return dict(self.values)


def watch(vis, function):
    """Wrap a capture call so every image records the switches as they were when it was taken."""
    seen = []

    def wrapped(*args, **kwargs):
        seen.append(vis.state())
        return function(*args, **kwargs)
    return wrapped, seen


class Switches(unittest.TestCase):
    def test_only_the_switches_that_are_on_are_turned_off_then_back_on(self):
        vis = Visibility(off=("isSketchesVisible",))  # someone hid sketches: they stay hidden, untouched
        before = vis.state()
        glyphs = g.Glyphs(NS(objectVisibility=vis)).hide()
        self.assertEqual(vis.state(), {attr: False for attr in ALL})
        self.assertTrue(glyphs.restore())
        self.assertEqual(vis.state(), before)
        self.assertNotIn("isSketchesVisible", [attr for attr, _ in vis.sets])
        self.assertEqual(glyphs.report(), {"shown": False, "hidden": [n for n in NAMES if n != "sketches"], "restored": True})

    def test_a_second_restore_sets_nothing(self):
        vis = Visibility()
        glyphs = g.Glyphs(NS(objectVisibility=vis)).hide()
        self.assertTrue(glyphs.restore())
        sets = len(vis.sets)
        self.assertTrue(glyphs.restore())
        self.assertEqual(len(vis.sets), sets)

    def test_glyphs_true_touches_nothing(self):
        vis = Visibility()
        glyphs = g.Glyphs(NS(objectVisibility=vis), show=True).hide()
        self.assertTrue(glyphs.restore())
        self.assertEqual((vis.sets, glyphs.report()), ([], {"shown": True, "hidden": [], "restored": True}))

    def test_a_fusion_without_object_visibility_still_captures_and_says_so(self):
        glyphs = g.Glyphs(NS()).hide()
        self.assertTrue(glyphs.restore())
        report = glyphs.report()
        self.assertEqual((report["shown"], report["hidden"], report["restored"]), (True, [], True))
        self.assertIn("Object Visibility", report["note"])

    def test_a_switch_that_does_not_come_back_is_named_and_restore_never_raises(self):
        vis = Visibility(stuck="isSketchesVisible")
        glyphs = g.Glyphs(NS(objectVisibility=vis)).hide()
        self.assertFalse(glyphs.restore())
        self.assertEqual((glyphs.failed, glyphs.restored), (["sketches"], False))
        self.assertEqual([attr for attr, value in vis.state().items() if not value], ["isSketchesVisible"])

    def test_every_switch_is_checked_against_its_state_before_not_only_the_ones_turned_off(self):
        def side_effect(attr, value, vis):  # turning sketches back on drops the work points
            if attr == "isSketchesVisible" and value:
                vis.values["isUserWorkPointsVisible"] = False
        vis = Visibility(side_effect=side_effect)
        glyphs = g.Glyphs(NS(objectVisibility=vis)).hide()
        self.assertTrue(glyphs.restore())
        self.assertEqual(vis.state(), {attr: True for attr in ALL})


class ExplodedRender(unittest.TestCase):
    def setUp(self):
        self.folder = Path(tempfile.mkdtemp()) / "frames"

    def scene(self, **vis):
        scene = render_tests.Scene(**{k: v for k, v in vis.items() if k in ("fail_at",)})
        scene.vis = Visibility(stuck=vis.get("stuck"))
        scene.design.objectVisibility = scene.vis
        scene.viewport.saveAsImageFile, scene.seen = watch(scene.vis, scene.viewport.saveAsImageFile)
        return scene

    def test_every_frame_is_clean_and_the_glyphs_come_back(self):
        scene = self.scene()
        out = scene.run(self.folder)
        self.assertEqual(len(scene.seen), out["frames"])
        self.assertTrue(all(not any(state.values()) for state in scene.seen))
        self.assertEqual(scene.vis.state(), {attr: True for attr in ALL})
        self.assertEqual(out["glyphs"], {"shown": False, "hidden": NAMES, "restored": True})

    def test_glyphs_true_keeps_them_in_every_frame(self):
        scene = self.scene()
        out = scene.run(self.folder, glyphs=True)
        self.assertTrue(all(all(state.values()) for state in scene.seen))
        self.assertEqual((scene.vis.sets, out["glyphs"]["shown"]), ([], True))

    def test_a_frame_that_fails_still_puts_the_glyphs_back(self):
        scene = self.scene(fail_at=2)
        with self.assertRaises(core.MechanicalError) as error:
            scene.run(self.folder)
        self.assertEqual(error.exception.code, "render_failed")
        self.assertEqual(scene.vis.state(), {attr: True for attr in ALL})

    def test_a_switch_that_does_not_come_back_refuses_after_the_parts_and_camera_are_home(self):
        scene = self.scene(stuck="isJointsVisible")
        with self.assertRaises(core.MechanicalError) as error:
            scene.run(self.folder)
        self.assertEqual(error.exception.code, "restore_failed")
        self.assertIn("joints", str(error.exception))
        self.assertEqual((scene.snapshots.hasPendingSnapshot, scene.viewport.camera.name, scene.occ._t.values),
                         (False, "original", render_tests.HOME))

    def test_a_refused_render_touches_no_switch(self):
        scene = self.scene()
        scene.snapshots.hasPendingSnapshot = True
        with self.assertRaises(core.MechanicalError) as error:
            scene.run(self.folder)
        self.assertEqual((error.exception.code, scene.vis.sets), ("pending_snapshot", []))


class ExplodedStills(unittest.TestCase):
    def setUp(self):
        self.folder = Path(tempfile.mkdtemp()) / "stills"

    def scene(self, fail_at=None):
        scene = stills_tests.Scene(fail_at=fail_at)
        scene.vis = Visibility()
        scene.design.objectVisibility = scene.vis
        scene.viewport.saveAsImageFile, scene.seen = watch(scene.vis, scene.viewport.saveAsImageFile)
        return scene

    def test_every_step_image_is_clean_and_the_glyphs_come_back(self):
        scene = self.scene()
        out = scene.run(self.folder)
        self.assertEqual(len(scene.seen), out["images"])
        self.assertTrue(all(not any(state.values()) for state in scene.seen))
        self.assertEqual(scene.vis.state(), {attr: True for attr in ALL})
        self.assertEqual(out["glyphs"], {"shown": False, "hidden": NAMES, "restored": True})

    def test_glyphs_true_and_a_failed_image(self):
        scene = self.scene()
        scene.run(self.folder, glyphs=True)
        self.assertTrue(all(all(state.values()) for state in scene.seen))
        self.assertEqual(scene.vis.sets, [])
        scene = self.scene(fail_at=1)
        with self.assertRaises(core.MechanicalError):
            scene.run(Path(tempfile.mkdtemp()) / "stills")
        self.assertEqual(scene.vis.state(), {attr: True for attr in ALL})


class RenderImage(unittest.TestCase):
    """render_image: Fusion's renderer never draws glyphs, but the fit counts them, so a standard view is
    fitted with them hidden and they are back before the render starts, and after every way out."""
    def run_render(self, states, stuck=None, **args):
        f = render_image_tests.Fusion(render_image_tests.Scene(render_image_tests.LIBRARY[0]), states)
        f.vis = Visibility(stuck=stuck)
        design = NS(renderManager=f.manager, objectVisibility=f.vis)
        seen = {"fit": [], "start": []}
        f.viewport.fit = lambda: seen["fit"].append(f.vis.state())
        f.on_start = lambda filename: seen["start"].append(f.vis.state())
        work = Path(tempfile.mkdtemp())
        report = {"cancelled": True, "renderState": "removed", "reason": "process_ended"}
        with unittest.mock.patch.object(ri, "SESSION_ID", "s"), unittest.mock.patch.object(ri.time, "sleep", lambda _: None), \
                unittest.mock.patch.object(ri, "staging_root", lambda: work / "staging"), \
                unittest.mock.patch.object(ri.render_process, "cancel", lambda future, started, states, pump=None: dict(report)), \
                unittest.mock.patch.object(ri, "target", lambda app, a: (f.document, design, {"documentId": "d"})):
            try:
                return f, seen, ri.render_image(f.app, {"sessionId": "s", "filename": str(work / "render.png"), **args}), None
            except core.MechanicalError as error:
                return f, seen, None, error

    def test_a_standard_view_is_fitted_without_glyphs_and_they_are_back_before_the_render(self):
        s, seen, out, error = self.run_render([1, 2])
        self.assertIsNone(error)
        self.assertEqual(seen, {"fit": [{attr: False for attr in ALL}], "start": [{attr: True for attr in ALL}]})
        self.assertEqual(s.vis.state(), {attr: True for attr in ALL})
        self.assertEqual(out["glyphsHiddenForFit"], NAMES)

    def test_the_current_view_touches_no_switch(self):
        s, seen, out, _ = self.run_render([2], view="current")
        self.assertEqual((seen["fit"], s.vis.sets, out["glyphsHiddenForFit"]), ([], [], []))

    def test_every_switch_is_back_after_a_failure_or_a_timeout(self):
        for states, code, extra in (([1, 3], "render_failed", {}), ([1], "render_timeout", {"timeoutSeconds": 0})):
            s, seen, _, error = self.run_render(states, **extra)
            self.assertEqual(error.code, code)
            self.assertEqual(seen["fit"], [{attr: False for attr in ALL}])
            self.assertEqual(s.vis.state(), {attr: True for attr in ALL})

    def test_a_switch_that_does_not_come_back_is_restore_failed(self):
        s, _, _, error = self.run_render([2], stuck="isSketchesVisible")
        self.assertEqual(error.code, "restore_failed")
        self.assertIn("sketches", str(error))


if __name__ == "__main__":
    unittest.main()
