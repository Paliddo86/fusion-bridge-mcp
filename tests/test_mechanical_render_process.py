"""Cancelling a local render, offline: only a render that is processing is cancelled, only through the one
RenderProcess.exe this Fusion started after the render began, and it counts as cancelled only once Fusion
reports the render failed."""
import importlib
from types import SimpleNamespace as NS
import unittest
import unittest.mock

import test_mechanical  # noqa: F401  (installs the fake adsk modules and the package path)

rp = importlib.import_module('mechanical_test_commands.mechanical.render_process')
STATES = NS(QueuedLocalRenderState=0, ProcessingLocalRenderState=1, FinishedLocalRenderState=2, FailedLocalRenderState=3)


GONE = "gone"


class Future:
    """renderState walks through `states`, then stays on the last one. GONE is a job Fusion dropped:
    reading its state raises, as it does once the job's process has ended."""
    def __init__(self, *states):
        self.states = list(states)

    @property
    def renderState(self):
        state = self.states.pop(0) if len(self.states) > 1 else self.states[0]
        if state == GONE:
            raise RuntimeError("3 : Error getting render information, render job doesn't exist.")
        return state


class Clock:
    def __init__(self):
        self.now = 0.0

    def __call__(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds


class Cancel(unittest.TestCase):
    def cancel(self, future, rows=(), end=(True, "terminated"), started=100.0):
        self.ended = []
        clock = Clock()

        def stop(pid, when):
            self.ended.append((pid, when))
            return end

        with unittest.mock.patch.object(rp, "os", NS(name="nt")):
            return rp.cancel(future, started, STATES, wait=5, sleep=clock.sleep, clock=clock, find=lambda: list(rows), end=stop)

    def test_a_processing_render_is_ended_through_its_own_process(self):
        out = self.cancel(Future(1, 1, 3), rows=[{"pid": 42, "created": 100.4, "path": "C:\\F\\RenderProcess.exe"}])
        self.assertEqual(out, {"cancelled": True, "renderState": "failed", "reason": "process_ended", "pid": 42})
        self.assertEqual(self.ended, [(42, 100.0)])

    def test_fusion_dropping_the_job_after_its_process_ended_is_a_cancel(self):
        out = self.cancel(Future(1, 1, GONE), rows=[{"pid": 42, "created": 100.4, "path": "C:\\F\\RenderProcess.exe"}])
        self.assertEqual(out, {"cancelled": True, "renderState": "removed", "reason": "process_ended", "pid": 42})

    def test_a_job_already_dropped_is_left_alone(self):
        out = self.cancel(Future(GONE), rows=[{"pid": 42, "created": 100.4, "path": "x"}])
        self.assertEqual((out["cancelled"], out["renderState"], out["reason"]), (False, "removed", "already_removed"))
        self.assertEqual(self.ended, [])

    def test_a_queued_render_has_no_process_and_nothing_is_ended(self):
        out = self.cancel(Future(0), rows=[{"pid": 7, "created": 50.0, "path": "x"}])
        self.assertEqual((out["cancelled"], out["renderState"], out["reason"]), (False, "queued", "queued_behind_another_render"))
        self.assertEqual(self.ended, [])

    def test_an_older_render_process_belongs_to_another_render(self):
        out = self.cancel(Future(1), rows=[{"pid": 7, "created": 50.0, "path": "x"}])
        self.assertEqual((out["cancelled"], out["reason"], out["candidates"]), (False, "process_not_identified", 0))
        self.assertEqual(self.ended, [])

    def test_two_candidates_are_ambiguous_and_neither_is_ended(self):
        rows = [{"pid": 7, "created": 101.0, "path": "x"}, {"pid": 8, "created": 102.0, "path": "y"}]
        out = self.cancel(Future(1), rows=rows)
        self.assertEqual((out["reason"], out["candidates"]), ("process_not_identified", 2))
        self.assertEqual(self.ended, [])

    def test_a_refused_end_is_reported_as_not_cancelled(self):
        out = self.cancel(Future(1), rows=[{"pid": 9, "created": 101.0, "path": "x"}], end=(False, "not_this_render"))
        self.assertEqual((out["cancelled"], out["reason"], out["pid"]), (False, "not_this_render", 9))

    def test_finished_renders_need_no_cancel(self):
        for state, reason in ((2, "already_finished"), (3, "already_failed")):
            out = self.cancel(Future(state))
            self.assertEqual((out["cancelled"], out["reason"]), (False, reason))
        self.assertEqual(self.ended, [])

    def test_a_render_that_finished_first_is_not_called_cancelled(self):
        out = self.cancel(Future(1, 2), rows=[{"pid": 5, "created": 100.0, "path": "x"}])
        self.assertEqual((out["cancelled"], out["renderState"], out["reason"]), (False, "finished", "finished_first"))

    def test_fusion_not_noticing_within_the_wait_is_reported(self):
        out = self.cancel(Future(1), rows=[{"pid": 5, "created": 100.0, "path": "x"}])
        self.assertEqual((out["cancelled"], out["renderState"], out["reason"]), (False, "processing", "state_not_updated"))

    def test_other_platforms_report_that_they_cannot_cancel(self):
        with unittest.mock.patch.object(rp, "os", NS(name="posix")):
            out = rp.cancel(Future(1), 0.0, STATES, find=lambda: self.fail("no process lookup off Windows"))
        self.assertEqual((out["cancelled"], out["reason"]), (False, "unsupported_platform"))
        self.assertEqual(rp.children() if rp.os.name != "nt" else [], [])


if __name__ == "__main__":
    unittest.main()
