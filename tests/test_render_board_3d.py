"""render_board_3d's high-quality render (rust/bridge/src/docs_script.py), built the way docs.rs builds it: the
script plus render_process.py embedded as RENDER_PROCESS_SOURCE. A timed-out render used to call
RenderFuture.cancel(), which does not exist, and still answer "it was cancelled" while the render went on and
could write outputPath later (found by Drew Owens, PR #40). Now it renders into a staging folder, moves the image
only on success, and ends a timed-out render through render_process.cancel."""
import os
from pathlib import Path
import re
import sys
import tempfile
from types import ModuleType, SimpleNamespace as NS
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
FINISHED, PROCESSING, FAILED, QUEUED = "finished", "processing", "failed", "queued"


def build_source():
    """docs.rs's SOURCE: the script, then render_process.py as RENDER_PROCESS_SOURCE."""
    rs = (ROOT / "rust/bridge/src/docs.rs").read_text()
    assert 'include_str!("../../../addin/AdomBridge/commands/mechanical/render_process.py")' in rs
    assert re.search(r'RENDER_PROCESS_SOURCE = r\'\'\'"', rs)
    script = (ROOT / "rust/bridge/src/docs_script.py").read_text()
    embedded = (ROOT / "addin/AdomBridge/commands/mechanical/render_process.py").read_text()
    return script + "\nRENDER_PROCESS_SOURCE = r'''" + embedded + "'''\n"


def fake_adsk():
    adsk = ModuleType("adsk")
    adsk.core = ModuleType("adsk.core")
    adsk.fusion = ModuleType("adsk.fusion")
    adsk.doEvents = lambda: None
    adsk.core.ViewOrientations = NS(TopViewOrientation=1)
    adsk.core.CameraTypes = NS(PerspectiveCameraType=1, OrthographicCameraType=2)
    adsk.fusion.LocalRenderStates = NS(FinishedLocalRenderState=FINISHED, ProcessingLocalRenderState=PROCESSING,
                                       FailedLocalRenderState=FAILED, QueuedLocalRenderState=QUEUED)
    adsk.fusion.RenderAspectRatios = NS(CustomRenderAspectRatio=9)
    return adsk


class Future:
    def __init__(self, path, finish):
        self.path, self.finish, self.state = path, finish, PROCESSING
        if finish:
            Path(path).write_bytes(b"png")
            self.state = FINISHED

    @property
    def renderState(self):
        if self.state is None:
            raise RuntimeError("3 : render job doesn't exist")
        return self.state


class RenderBoard3D(unittest.TestCase):
    def setUp(self):
        self.adsk = fake_adsk()
        patcher = mock.patch.dict(sys.modules, {"adsk": self.adsk, "adsk.core": self.adsk.core, "adsk.fusion": self.adsk.fusion})
        patcher.start()
        self.addCleanup(patcher.stop)
        self.ns = {}
        exec(compile(build_source(), "docs_script.py", "exec"), self.ns)
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.dir = Path(folder.name)
        self.temp = self.dir / "temp"
        self.temp.mkdir()
        self.ns["tempfile_dir"] = lambda: str(self.temp)
        self.finish = True
        self.futures = []
        rendering = NS(aspectRatio=1, resolutionWidth=800, resolutionHeight=600, renderQuality=50, isBackgroundTransparent=False)

        def start(path):
            future = Future(path, self.finish)
            self.futures.append(future)
            return future
        rendering.startLocalRender = start
        self.rendering = rendering
        camera = NS(viewOrientation=None, cameraType=None, isFitView=False, isSmoothTransition=True, viewExtents=1.0)
        design_doc = NS(name="Board", isVisible=True)
        self.app = NS(activeDocument=design_doc, activeViewport=NS(camera=camera), activeProduct=NS(renderManager=NS(rendering=rendering)),
                      documents=NS(count=1, item=lambda i: design_doc), executeTextCommand=lambda text: None)
        self.ns["_design_3d"] = lambda app, args: (0, design_doc, None)
        self.adsk.fusion.Design = NS(cast=lambda product: product)

    def render(self, **args):
        return self.ns["render_board_3d"](self.app, {"quality": "high", "outputPath": str(self.dir / "board.png"), **args})

    def test_a_finished_render_is_moved_to_outputpath_and_the_staging_folder_goes(self):
        out = self.render()
        self.assertTrue(out["success"], out)
        self.assertEqual((self.dir / "board.png").read_bytes(), b"png")
        # Fusion was given a staging path, never outputPath itself
        self.assertNotEqual(self.futures[0].path, str(self.dir / "board.png"))
        self.assertTrue(self.futures[0].path.startswith(str(self.temp / "adom-fusion-render")))
        self.assertEqual(list((self.temp / "adom-fusion-render").iterdir()), [])
        # every render setting it touched is put back
        self.assertEqual((self.rendering.resolutionWidth, self.rendering.renderQuality), (800, 50))

    def test_an_existing_outputpath_is_replaced_only_on_success(self):
        (self.dir / "board.png").write_bytes(b"old")
        self.finish = False
        cancel = mock.Mock(return_value={"cancelled": True, "renderState": "removed", "reason": "process_ended"})
        with mock.patch.dict(self.ns, {"_render_process": lambda: NS(cancel=cancel)}):
            out = self.render(renderSeconds=0)
        self.assertEqual(out["errorCode"], "render_timeout")
        # the old image is still there: a timed-out render no longer deletes it first
        self.assertEqual((self.dir / "board.png").read_bytes(), b"old")

    def test_a_timed_out_render_is_cancelled_through_its_render_process(self):
        self.finish = False
        cancel = mock.Mock(return_value={"cancelled": True, "renderState": "removed", "reason": "process_ended"})
        with mock.patch.dict(self.ns, {"_render_process": lambda: NS(cancel=cancel)}):
            out = self.render(renderSeconds=0)
        self.assertFalse(out["success"])
        self.assertEqual(out["errorCode"], "render_timeout")
        self.assertIn("cancelled and wrote nothing", out["error"])
        self.assertEqual(out["cancel"]["reason"], "process_ended")
        future, started, states = cancel.call_args.args
        self.assertIs(future, self.futures[0])
        self.assertIs(states, self.adsk.fusion.LocalRenderStates)
        self.assertFalse((self.dir / "board.png").exists())
        self.assertEqual(list((self.temp / "adom-fusion-render").iterdir()), [])

    def test_a_render_that_cannot_be_cancelled_says_so_and_cannot_reach_outputpath(self):
        self.finish = False
        cancel = mock.Mock(return_value={"cancelled": False, "renderState": "queued", "reason": "queued_behind_another_render"})
        with mock.patch.dict(self.ns, {"_render_process": lambda: NS(cancel=cancel)}):
            out = self.render(renderSeconds=0)
        self.assertEqual(out["errorCode"], "render_timeout")
        self.assertNotIn("was cancelled", out["error"])
        self.assertIn("queued_behind_another_render", out["error"])
        # its late image would land in the staging folder named in the reply, never at outputPath
        late = Path(self.futures[0].path)
        self.assertIn(str(late.parent), out["error"])
        self.assertNotEqual(late, self.dir / "board.png")

    def test_the_embedded_render_process_loads_as_its_own_module(self):
        mod = self.ns["_render_process"]()
        self.assertTrue(callable(mod.cancel) and callable(mod.terminate))
        # its names stay in its own module, not the script's
        self.assertNotIn("terminate", self.ns)
        self.assertEqual(mod.children(), [] if os.name != "nt" else mod.children())


if __name__ == "__main__":
    unittest.main()
