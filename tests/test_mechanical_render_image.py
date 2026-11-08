"""render_image, offline: the camera, every render setting and the document's scene come back after any
render; the lighting environment and the backdrop are the ones asked for while it renders; Fusion writes
only into the staging folder, so a failed, refused or timed-out render never touches filename."""
import importlib
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace as NS
import unittest
import unittest.mock

from test_mechanical import adsk
import mech_exe

adsk.doEvents = lambda: None
adsk.core.ViewOrientations = NS(IsoTopRightViewOrientation=7, FrontViewOrientation=3)
adsk.core.Color = NS(create=lambda r, g, b, a: NS(red=r, green=g, blue=b, opacity=a))
adsk.fusion.RenderAspectRatios = NS(CurrentViewportRenderAspectRatio=0, CustomRenderAspectRatio=6)
adsk.fusion.RenderResolutions = NS(Video1920x1080RenderResolution=16, CustomRenderResolution=17)
adsk.fusion.RenderSceneBackgroundTypes = NS(EnvironmentRenderSceneBackgroundType=0, SolidColorRenderSceneBackgroundType=1)
adsk.fusion.LocalRenderStates = NS(QueuedLocalRenderState=0, ProcessingLocalRenderState=1, FinishedLocalRenderState=2, FailedLocalRenderState=3)
ri = importlib.import_module('mechanical_test_commands.mechanical.render_image')
core = importlib.import_module('mechanical_test_commands.mechanical.core')


class Env:
    def __init__(self, id, name, custom=False, file=None):
        self.id, self.name, self.isCustomEnvironment, self.file = id, name, custom, file


LIBRARY = [Env("Studio1", "Sharp Highlights"), Env("Studio7", "Grid Light"), Env("CustomLoadedEnvironment", "Custom...", True),
           Env("mi360_Plaza", "Plaza"), Env("DryLakeBed", "Dry lake bed")]
UNREADABLE = "3 : Getting this property is only valid when the backgroundType is EnvironmentRenderSceneBackgroundType"


class Scene:
    """Fusion's scene settings: an environment set is shown behind the model and lights it; a colour set
    makes a solid backdrop, the environment still lights the scene and can no longer be read."""
    def __init__(self, lighting, backdrop="color", rgb=(178, 178, 178)):
        self.lighting, self.backdrop, self.rgb, self.isGroundDisplayed = lighting, backdrop, rgb, False
        self.refuse = set()

    @property
    def backgroundType(self):
        return 0 if self.backdrop == "environment" else 1

    @property
    def backgroundEnvironment(self):
        if self.backdrop != "environment":
            raise RuntimeError(UNREADABLE)
        return self.lighting

    @backgroundEnvironment.setter
    def backgroundEnvironment(self, environment):
        if environment.id in self.refuse:
            raise RuntimeError("3 : environment could not be loaded")
        self.lighting, self.backdrop = environment, "environment"

    @property
    def backgroundSolidColor(self):
        if self.backdrop != "color":
            raise RuntimeError(UNREADABLE.replace("EnvironmentRender", "SolidColorRender"))
        return NS(red=self.rgb[0], green=self.rgb[1], blue=self.rgb[2], opacity=255)

    @backgroundSolidColor.setter
    def backgroundSolidColor(self, value):
        self.rgb, self.backdrop = (value.red, value.green, value.blue), "color"

    def look(self):
        return (self.lighting.id, self.backdrop, self.rgb if self.backdrop == "color" else None)


class Rendering:
    """Width and height make the resolution custom; a preset sets the size and the aspect ratio."""
    def __init__(self):
        self.aspectRatio, self._resolution, self.resolutionWidth, self.resolutionHeight = 0, 17, 1961, 1320
        self.renderQuality, self.isBackgroundTransparent = 33, False

    def __setattr__(self, name, value):
        object.__setattr__(self, name, value)
        if name in ("resolutionWidth", "resolutionHeight"):
            object.__setattr__(self, "_resolution", 17)

    @property
    def resolution(self):
        return self._resolution

    @resolution.setter
    def resolution(self, value):
        object.__setattr__(self, "_resolution", value)
        if value == 16:
            object.__setattr__(self, "resolutionWidth", 1920)
            object.__setattr__(self, "resolutionHeight", 1080)
            object.__setattr__(self, "aspectRatio", 3)


class Manager:
    """shared=True hands out one Rendering object (the worst case); shared=False is what Fusion does: every
    access is a new object holding the document's settings, so changes on one never reach the document."""
    def __init__(self, fusion, rendering, scene):
        self.fusion, self._rendering, self.sceneSettings, self.shared = fusion, rendering, scene, True
        self.renderEnvironments = NS(count=len(LIBRARY), item=LIBRARY.__getitem__)
        self.handed = []

    @property
    def rendering(self):
        if self.shared:
            return self._rendering
        own = Rendering()
        for name in ("aspectRatio", "resolutionWidth", "resolutionHeight", "renderQuality", "isBackgroundTransparent"):
            object.__setattr__(own, name, getattr(self._rendering, name))
        object.__setattr__(own, "_resolution", self._rendering.resolution)
        own.startLocalRender = lambda filename, own=own: self.fusion.start(filename, own)
        self.handed.append(own)
        return own

    @property
    def isRenderWorkspaceActive(self):
        return self.fusion.render_active

    def activateRenderWorkspace(self):
        self.fusion.ui.activeWorkspace = self.fusion.render_ws
        return True


class Fusion:
    """One design in Fusion: render manager, Render workspace and its text command, viewport, camera."""
    def __init__(self, scene, states=(1, 1, 2)):
        self.scene, self.rendering, self.states = scene, Rendering(), list(states)
        self.started, self.during, self.loaded, self.on_start = [], [], [], None
        design_ws, self.render_ws = NS(id="FusionSolidEnvironment"), NS(id="FusionRenderEnvironment")
        design_ws.activate = lambda: setattr(self.ui, "activeWorkspace", design_ws)
        self.ui = NS(activeWorkspace=design_ws)
        self.manager = Manager(self, self.rendering, scene)
        self.camera = NS(isSmoothTransition=True, name="original", viewOrientation=0)
        self.viewport = NS(camera=self.camera, fit=lambda: None, refresh=lambda: None)
        self.document = NS(isSaved=True, isModified=False)
        self.app = NS(activeViewport=self.viewport, userInterface=self.ui, executeTextCommand=self.text_command)
        self.rendering.startLocalRender = self.start
        adsk.fusion.RenderEnvironment = NS(loadCustomEnvironment=self.load)

    @property
    def render_active(self):
        return self.ui.activeWorkspace is self.render_ws

    def text_command(self, command):
        if not self.render_active:
            raise RuntimeError("3 : Please switch to Render workspace to take effect")
        assert command == "Render.GetEnvironment"
        return self.scene.lighting.name

    def load(self, path):
        self.loaded.append(path)
        return Env("CustomLoadedEnvironment", "Custom...", True, file=path)

    def start(self, filename, rendering=None):
        fusion = self
        rendering = self.rendering if rendering is None else rendering
        self.started.append(filename)
        self.during.append((self.scene.look(), rendering.isBackgroundTransparent, self.render_active))
        if self.on_start:
            self.on_start(filename)

        class Future:
            imageWidth, imageHeight = 800, 600

            @property
            def renderState(self):
                state = fusion.states.pop(0) if len(fusion.states) > 1 else fusion.states[0]
                if state == "gone":
                    raise RuntimeError("3 : Error getting render information, render job doesn't exist.")
                if state == 2 and not Path(filename).exists():
                    Path(filename).write_bytes(b"png")
                return state
        return Future()

    def settings(self):
        r = self.rendering
        return (r.aspectRatio, r.resolution, r.resolutionWidth, r.resolutionHeight, r.renderQuality, r.isBackgroundTransparent,
                self.scene.isGroundDisplayed, self.viewport.camera.name)


class Base(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp())
        self.staging = self.dir / "staging"

    def fusion(self, lighting="Studio1", backdrop="color", rgb=(178, 178, 178), states=(1, 1, 2)):
        return Fusion(Scene(next(e for e in LIBRARY if e.id == lighting), backdrop, rgb), states)

    def render(self, f, path, cancel=None, **args):
        manager = f.manager
        patches = [unittest.mock.patch.object(ri, "SESSION_ID", "s"), unittest.mock.patch.object(ri.time, "sleep", lambda _: None),
                   unittest.mock.patch.object(ri, "staging_root", lambda: self.staging),
                   unittest.mock.patch.object(ri, "target", lambda app, a: (f.document, NS(renderManager=manager), {"documentId": "d"}))]
        if cancel is not None:
            patches.append(unittest.mock.patch.object(ri.render_process, "cancel", cancel))
        for patch in patches:
            patch.start()
        try:
            return ri.render_image(f.app, {"sessionId": "s", "filename": str(path), **args})
        finally:
            for patch in reversed(patches):
                patch.stop()

    def refused(self, code, f, path, **args):
        with self.assertRaises(core.MechanicalError) as error:
            self.render(f, path, **args)
        self.assertEqual(error.exception.code, code, str(error.exception))
        return error.exception


class Settings(Base):
    def test_settings_and_camera_come_back_after_a_render(self):
        f = self.fusion()
        before = f.settings()
        out = self.render(f, self.dir / "a.png", width=1024, height=576, quality=50, ground=True, transparent=True)
        self.assertEqual((out["settingsRestored"], out["sceneRestored"], f.settings()), (True, True, before))
        self.assertEqual(f.during[0][1], True)

    def test_fusion_hands_each_caller_its_own_settings_so_the_documents_never_change(self):
        f = self.fusion()
        f.manager.shared = False
        before = f.settings()
        out = self.render(f, self.dir / "a.png", width=800, height=450, quality=30, transparent=True)
        self.assertEqual((out["settingsRestored"], f.settings(), f.during[0][1]), (True, before, True))
        self.assertGreaterEqual(len(f.manager.handed), 3)

    def test_a_viewport_resized_meanwhile_is_not_a_change(self):
        settings = {"aspectRatio": 0, "resolution": 17, "resolutionWidth": 1931, "resolutionHeight": 1240, "renderQuality": 75,
                    "isBackgroundTransparent": False}
        resized = dict(settings, resolutionHeight=1314)
        self.assertEqual(ri.same_settings(settings, resized), {})
        custom = dict(settings, aspectRatio=6)
        self.assertEqual(ri.same_settings(custom, dict(custom, resolutionHeight=1314)), {"resolutionHeight": [1240, 1314]})

    def test_a_real_change_to_the_documents_settings_is_reported(self):
        f = self.fusion()
        f.rendering.aspectRatio = 6
        original = Fusion.start

        def start_and_change(self_, filename, rendering=None):
            future = original(self_, filename, rendering)
            object.__setattr__(self_.rendering, "renderQuality", 99)
            return future
        with unittest.mock.patch.object(ri, "restore_settings", lambda rendering, saved: None), \
                unittest.mock.patch.object(Fusion, "start", start_and_change):
            f.rendering.startLocalRender = lambda filename: f.start(filename)
            out = self.render(f, self.dir / "b.png")
        self.assertEqual((out["settingsRestored"], out["settingsChanged"]["renderQuality"]), (False, [33, 99]))

    def test_a_preset_resolution_comes_back_as_the_preset(self):
        f = self.fusion()
        f.rendering.resolution = 16
        before = f.settings()
        out = self.render(f, self.dir / "a.png", width=640, height=360)
        self.assertEqual((out["settingsRestored"], f.settings()), (True, before))

    def test_and_after_a_failed_render_or_a_timeout(self):
        report = {"cancelled": True, "renderState": "failed", "reason": "process_ended", "pid": 4}
        for states, code, extra in (([1, 3], "render_failed", {}), ([1], "render_timeout", {"timeoutSeconds": 0})):
            f = self.fusion(states=states)
            before = (f.settings(), f.scene.look())
            self.refused(code, f, self.dir / ("%s.png" % code), quality=90, environment="Plaza", background="#ffffff",
                         cancel=lambda future, started, states, pump=None: dict(report), **extra)
            self.assertEqual((f.settings(), f.scene.look()), before)


class Destinations(Base):
    def test_destinations_are_checked_first(self):
        f = self.fusion()
        (self.dir / "taken.png").write_bytes(b"x")
        for path, args, code in ((self.dir / "a.jpg", {"transparent": True}, "invalid_destination"),
                                 (Path("relative.png"), {}, "invalid_destination"),
                                 (self.dir / "a.bmp", {}, "invalid_destination"),
                                 (self.dir / "taken.png", {}, "destination_exists")):
            self.refused(code, f, path, **args)
        self.assertEqual(f.started, [])
        self.assertEqual(self.render(f, self.dir / "taken.png", overwrite=True)["bytes"], 3)

    def test_fusion_writes_only_into_the_staging_folder(self):
        f = self.fusion()
        out = self.render(f, self.dir / "out" / "a.png")
        self.assertTrue(f.started[0].startswith(str(self.staging)))
        self.assertEqual(((self.dir / "out" / "a.png").read_bytes(), out["filename"]), (b"png", str(self.dir / "out" / "a.png")))
        self.assertEqual(list(self.staging.iterdir()), [])

    def test_a_timed_out_render_leaves_the_old_image_and_no_staging(self):
        f = self.fusion(states=(1,))
        (self.dir / "old.png").write_bytes(b"old")
        reports = {"cancelled": (True, "process_ended"), "queued": (False, "queued_behind_another_render")}
        for label, (cancelled, reason) in reports.items():
            error = self.refused("render_timeout", f, self.dir / "old.png", overwrite=True, timeoutSeconds=0,
                                 cancel=lambda *a, **k: {"cancelled": cancelled, "renderState": "failed" if cancelled else "queued", "reason": reason})
            self.assertEqual(error.details["cancel"]["cancelled"], cancelled)
            self.assertIn("cancelled" if cancelled else "could not be cancelled", str(error))
            self.assertEqual(((self.dir / "old.png").read_bytes(), list(self.staging.iterdir())), (b"old", []))

    def test_a_file_that_appears_during_the_render_is_kept_without_overwrite(self):
        f = self.fusion()
        f.on_start = lambda staged: (self.dir / "b.png").write_bytes(b"someone else")
        self.refused("destination_exists", f, self.dir / "b.png")
        self.assertEqual((self.dir / "b.png").read_bytes(), b"someone else")
        self.assertEqual(sorted(p.name for p in self.dir.iterdir()), ["b.png", "staging"])

    def test_publishing_across_volumes_and_without_hard_links(self):
        real_replace, real_link = ri.os.replace, ri.os.link
        moves = []

        def replace(src, dst):
            if "staging" in src:  # the staging folder sits on another volume
                raise OSError(17, "The system cannot move the file to a different disk drive")
            moves.append(("replace", Path(dst).name))
            return real_replace(src, dst)

        def link(src, dst):
            raise OSError(1, "hard links are not supported on this volume")
        with unittest.mock.patch.object(ri.os, "replace", replace), unittest.mock.patch.object(ri.os, "link", link):
            out = self.render(self.fusion(), self.dir / "c.png")
        self.assertEqual(((self.dir / "c.png").read_bytes(), out["bytes"], moves), (b"png", 3, [("replace", "c.png")]))
        self.assertEqual(sorted(p.name for p in self.dir.iterdir()), ["c.png", "staging"])

    def test_old_staging_folders_are_swept(self):
        old, new = self.staging / "render-old", self.staging / "render-new"
        for folder in (old, new):
            folder.mkdir(parents=True)
        (old / "render.png").write_bytes(b"late")
        ri.sweep(self.staging, now=time.time() + ri.STALE_SECONDS / 2)
        self.assertTrue(old.exists())
        ri.sweep(self.staging, now=time.time() + ri.STALE_SECONDS + 5)
        self.assertEqual(list(self.staging.iterdir()), [])


class Environments(Base):
    def test_a_library_environment_by_name_or_id_in_any_case(self):
        for value in ("plaza", "mi360_Plaza", "PLAZA"):
            f = self.fusion(lighting="Studio1", backdrop="environment")
            out = self.render(f, self.dir / "a.png", environment=value, overwrite=True)
            self.assertEqual(f.during[0][0], ("mi360_Plaza", "environment", None))
            self.assertEqual(out["environment"], {"name": "Plaza", "id": "mi360_Plaza", "custom": False})
            self.assertEqual((f.scene.look(), out["sceneRestored"]), (("Studio1", "environment", None), True))

    def test_an_unknown_name_lists_the_library_without_the_custom_slot(self):
        f = self.fusion()
        error = self.refused("unknown_environment", f, self.dir / "a.png", environment="Moon base")
        self.assertEqual(error.details["available"], ["Sharp Highlights", "Grid Light", "Plaza", "Dry lake bed"])
        self.assertEqual((f.started, f.render_active), ([], False))

    def test_on_a_solid_backdrop_the_backdrop_stays_and_the_lighting_comes_back(self):
        f = self.fusion(lighting="Studio7", rgb=(10, 20, 30))
        out = self.render(f, self.dir / "a.png", environment="Dry lake bed")
        self.assertEqual(f.during[0], (("DryLakeBed", "color", (10, 20, 30)), False, False))
        self.assertEqual((f.scene.lighting.id, f.scene.backdrop, f.scene.rgb), ("Studio7", "color", (10, 20, 30)))
        self.assertEqual((out["sceneRestored"], f.ui.activeWorkspace.id), (True, "FusionSolidEnvironment"))

    def test_a_colour_backdrop_keeps_the_lighting(self):
        f = self.fusion(lighting="mi360_Plaza", backdrop="environment")
        out = self.render(f, self.dir / "a.png", background="#FFfFff")
        self.assertEqual(f.during[0][0], ("mi360_Plaza", "color", (255, 255, 255)))
        self.assertEqual((f.scene.look(), out["background"]), (("mi360_Plaza", "environment", None), "#FFfFff"))

    def test_the_environment_backdrop_on_a_solid_document_shows_its_own_lighting(self):
        f = self.fusion(lighting="Studio7")
        self.render(f, self.dir / "a.png", background="environment")
        self.assertEqual(f.during[0][0], ("Studio7", "environment", None))
        self.assertEqual(f.scene.look(), ("Studio7", "color", (178, 178, 178)))

    def test_environment_and_backdrop_together(self):
        f = self.fusion(lighting="Studio1")
        self.render(f, self.dir / "a.png", environment="Grid Light", background="environment")
        self.assertEqual(f.during[0][0], ("Studio7", "environment", None))
        self.assertEqual(f.scene.look(), ("Studio1", "color", (178, 178, 178)))

    def test_bad_backdrops_and_transparency_with_a_backdrop_are_refused(self):
        f = self.fusion()
        for value in ("white", "#fff", "#gggggg", "#12345678", ""):
            self.refused("invalid_input", f, self.dir / "a.png", background=value)
        self.refused("invalid_input", f, self.dir / "a.png", background="#ffffff", transparent=True)
        self.assertEqual(f.started, [])

    def test_custom_lighting_behind_a_solid_backdrop_comes_back_through_the_custom_slot(self):
        f = self.fusion()
        f.scene.lighting = Env("CustomLoadedEnvironment", "Custom...", True, file="C:/own.hdr")
        out = self.render(f, self.dir / "a.png", environment="Plaza")
        self.assertEqual(f.during[0][0], ("mi360_Plaza", "color", (178, 178, 178)))
        self.assertEqual((f.scene.look(), out["sceneRestored"]), (("CustomLoadedEnvironment", "color", (178, 178, 178)), True))

    def test_a_custom_file_never_replaces_the_documents_own_custom_environment(self):
        hdr = self.dir / "other.hdr"
        hdr.write_bytes(b"#?RADIANCE\n")
        for backdrop in ("color", "environment"):
            f = self.fusion(backdrop=backdrop)
            f.scene.lighting = Env("CustomLoadedEnvironment", "Custom...", True, file="C:/own.hdr")
            before = (f.settings(), f.scene.look())
            self.refused("custom_environment_in_use", f, self.dir / "a.png", environment=str(hdr))
            self.assertEqual((f.loaded, f.started, f.settings(), f.scene.look(), f.render_active), ([], [], before[0], before[1], False))

    def test_lighting_fusion_does_not_name_is_refused_before_anything_changes(self):
        f = self.fusion()
        f.scene.lighting = Env("Mystery", "Not in the library")
        before = (f.settings(), f.scene.look())
        self.refused("scene_unreadable", f, self.dir / "a.png", environment="Plaza")
        self.assertEqual((f.started, f.settings(), f.scene.look(), f.render_active), ([], before[0], before[1], False))


class Failures(Base):
    def test_an_environment_fusion_refuses_is_reported_and_nothing_is_left_changed(self):
        f = self.fusion(lighting="Studio1", backdrop="environment")
        before = (f.settings(), f.scene.look())
        f.scene.refuse.add("DryLakeBed")
        self.refused("environment_unavailable", f, self.dir / "a.png", environment="Dry lake bed", quality=90)
        self.assertEqual((f.started, f.settings(), f.scene.look()), ([], before[0], before[1]))

    def test_a_job_fusion_drops_mid_render_is_a_failure_with_everything_put_back(self):
        f = self.fusion(states=(1, "gone"))
        before = (f.settings(), f.scene.look())
        error = self.refused("render_failed", f, self.dir / "a.png", environment="Plaza")
        self.assertIn("dropped", str(error))
        self.assertEqual((f.settings(), f.scene.look(), list(self.staging.iterdir())), (before[0], before[1], []))

    def test_a_restore_step_that_fails_does_not_keep_the_camera_from_coming_back(self):
        f = self.fusion(lighting="Studio1", backdrop="environment")
        f.on_start = lambda staged: f.scene.refuse.add("Studio1")
        out = self.render(f, self.dir / "a.png", environment="Plaza")
        self.assertEqual((out["sceneRestored"], [x["step"] for x in out["restoreFailed"]]), (False, ["scene"]))
        self.assertEqual((f.viewport.camera.name, out["settingsRestored"]), ("original", True))


class CustomFiles(Base):
    def file(self, name, data):
        path = self.dir / name
        path.write_bytes(data)
        return path

    def test_hdr_and_exr_files_are_loaded_by_their_absolute_path(self):
        for name, data in (("studio.hdr", b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n"), ("hall.EXR", b"v/1\x01\x02\x00\x00\x00")):
            f = self.fusion(lighting="Studio7", backdrop="environment")
            path = self.file(name, data)
            out = self.render(f, self.dir / "a.png", environment=str(path), overwrite=True)
            self.assertEqual((f.loaded, f.during[0][0][0]), ([str(path)], "CustomLoadedEnvironment"))
            self.assertEqual(out["environment"], {"name": name, "file": str(path), "custom": True})
            self.assertEqual(f.scene.look(), ("Studio7", "environment", None))

    def test_a_file_fusion_cannot_load_is_reported_not_raised_raw(self):
        f = self.fusion()
        path = self.file("broken.hdr", b"#?RADIANCE\n")

        def load(path):
            raise RuntimeError("3 : File does not exist")
        f.load = load
        adsk.fusion.RenderEnvironment = NS(loadCustomEnvironment=load)
        error = self.refused("environment_load_failed", f, self.dir / "a.png", environment=str(path))
        self.assertIn("broken.hdr", str(error))
        self.assertEqual(f.started, [])

    def test_files_fusion_should_not_be_handed_are_refused(self):
        f = self.fusion()
        cases = ((str(self.file("fake.hdr", b"<html>")), "invalid_environment"), ("studio.hdr", "invalid_environment"),
                 (str(self.dir / "missing.exr"), "environment_not_found"), (str(self.file("photo.jpg", b"\xff\xd8")), "invalid_environment"))
        for value, code in cases:
            self.refused(code, f, self.dir / "a.png", environment=value)
        self.assertEqual((f.loaded, f.started), ([], []))


class Schema(unittest.TestCase):
    """What the exe admits before anything reaches Fusion (the goldens pin the same verdicts for Rust)."""
    def admit(self, **args):
        mech = importlib.import_module('mechanical_test_commands.mechanical')
        request = {"op": "render_image", "sessionId": mech.SESSION_ID, "operationId": "q", "target": {"documentId": "d", "revision": "r"},
                   "filename": "C:/x.png", **args}
        return mech_exe.prepare_request(request, mech.SCHEMA, mech.WRITES, mech.SESSION_ID)

    def test_quality_below_25_is_refused_because_fusion_refuses_it(self):
        with self.assertRaises(mech_exe.MechanicalError) as error:
            self.admit(quality=24)
        self.assertEqual((error.exception.code, str(error.exception)), ("invalid_input", "args.quality is too small"))
        self.assertTrue(self.admit(quality=25)[1])

    def test_environment_and_background_are_strings(self):
        self.assertTrue(self.admit(environment="Plaza", background="#ffffff")[1])
        for key in ("environment", "background"):
            with self.assertRaises(mech_exe.MechanicalError) as error:
                self.admit(**{key: 7})
            self.assertEqual(str(error.exception), "args.%s must be string" % key)


if __name__ == "__main__":
    unittest.main()
