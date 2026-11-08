"""When writes run Fusion's full recompute, offline: Fusion's own compute plus a health read per write,
the full recompute at a checkpoint (enough significant change, a move to another part, an export, a
caller asking), and the health a write verified reused while nothing changed."""
import importlib
from types import SimpleNamespace as NS
import unittest
from unittest.mock import patch

import test_mechanical  # noqa: F401  (installs the fake adsk and the test package)

c = importlib.import_module('mechanical_test_commands.mechanical.core')
compute = importlib.import_module('mechanical_test_commands.mechanical.compute')
tx = importlib.import_module('mechanical_test_commands.mechanical.transactions')
ops = importlib.import_module('mechanical_test_commands.mechanical.operations')


def ref(path):
    return {"documentId": "d", "revision": "r", "token": "t", "objectType": "adsk::fusion::Occurrence", "occurrencePath": path}


class Health:
    """compute.health stand-in: records whether each read asked for the full recompute."""
    def __init__(self, healthy=True):
        self.calls, self.healthy = [], healthy

    def __call__(self, design, recompute=False, natives=None):
        self.calls.append(recompute)
        faults = [] if self.healthy else [{"index": "3", "name": "Fillet1", "state": 2, "message": "lost edge"}]
        return {"healthy": self.healthy, "computeSucceeded": True if recompute else None, "faults": faults, "unknown": []}


HEALTHY = {"healthy": True, "faults": [], "unknown": []}


class Weights(unittest.TestCase):
    def test_deletes_outweigh_relationships_edits_and_appends(self):
        w = lambda op: compute.weight(op, {})
        self.assertLess(w("component_metadata"), w("sketch_create"))
        self.assertLess(w("sketch_create"), w("feature_create"))
        self.assertLess(w("feature_create"), w("feature_edit"))
        self.assertLessEqual(w("feature_edit"), w("joint_create"))
        self.assertLess(w("joint_create"), w("entity_delete"))
        self.assertEqual(w("some_future_write"), 1.0)

    def test_a_parameter_batch_weighs_its_changes(self):
        args = {"changes": [{"action": "create"}, {"action": "set"}, {"action": "set"}, {"action": "delete"}]}
        self.assertEqual(compute.weight("parameter_batch", args), 6.0)
        self.assertEqual(compute.weight("parameter_batch", {"changes": [{"action": "create"}]}), 0.0)


class Focus(unittest.TestCase):
    def test_the_first_occurrence_path_names_the_part(self):
        self.assertEqual(compute.focus_of({"feature": ref("Drawer:1+Handle:2")}), "Drawer:1")
        self.assertEqual(compute.focus_of({"geometryOne": {"entity": ref("Lid:1")}, "geometryTwo": ref("Box:1")}), "Lid:1")

    def test_root_level_work_and_the_target_name_no_part(self):
        self.assertIsNone(compute.focus_of({"target": {"occurrencePath": "Lid:1"}, "name": "w", "expression": "2 mm"}))
        self.assertIsNone(compute.focus_of({"feature": dict(ref(None), occurrencePath=None)}))


class Policy(unittest.TestCase):
    def setUp(self):
        compute.reset()
        self.health = Health()
        self.patch = patch.object(compute, "health", self.health)
        self.patch.start()

    def tearDown(self):
        self.patch.stop()

    def write(self, op, revision, **args):
        state, info = compute.settle(None, "d", {"op": op, **args}, HEALTHY)
        return compute.committed("d", revision, state, info)

    def test_appends_stay_incremental_until_they_add_up_to_a_checkpoint(self):
        out = [self.write("feature_create", "r%d" % i, profiles=[ref("Lid:1")]) for i in range(6)]
        self.assertEqual(self.health.calls, [False, False, False, False, False, True])
        self.assertEqual([o["full"] for o in out], [False] * 5 + [True])
        self.assertEqual((out[4]["significance"], out[5]["significance"], out[5]["reason"]), (5.0, 0.0, "significance"))
        self.assertEqual(out[5]["lastFullCompute"]["reason"], "significance")

    def test_moving_on_to_another_part_checkpoints_the_last_one(self):
        self.write("feature_edit", "r1", feature=ref("Lid:1"))
        out = self.write("feature_create", "r2", profiles=[ref("Box:1")])
        self.assertEqual((self.health.calls, out["reason"], out["focus"]), ([False, True], "focus_change", "Box:1"))

    def test_metadata_only_on_one_part_does_not_checkpoint_the_move(self):
        self.write("component_metadata", "r1", component=ref("Lid:1"))
        self.write("feature_create", "r2", profiles=[ref("Box:1")])
        self.assertEqual(self.health.calls, [False, False])

    def test_one_occurrence_move_is_not_a_cluster_worth_a_checkpoint(self):
        self.write("occurrence_update", "r1", occurrence=ref("Lid:1"))
        self.write("occurrence_update", "r2", occurrence=ref("Box:1"))
        self.assertEqual(self.health.calls, [False, False])

    def test_an_unhealthy_result_is_refused_with_its_health_and_counts_nothing(self):
        self.health.healthy = False
        with self.assertRaises(c.MechanicalError) as error:
            compute.settle(None, "d", {"op": "feature_edit", "feature": ref("Lid:1")}, HEALTHY)
        self.assertEqual(error.exception.code, "unhealthy_model")
        self.assertEqual(error.exception.details["compute"]["full"], False)
        self.assertEqual([f["name"] for f in error.exception.details["newFaults"]], ["Fillet1"])
        self.assertEqual(compute.status("d")["writesSinceFullCompute"], 0)

    def test_health_a_write_verified_is_reused_while_nothing_changed(self):
        self.write("feature_create", "r1")
        state = compute.health_before(None, "d", "r1")
        self.assertEqual((self.health.calls, state["computeSucceeded"]), ([False], None))
        compute.health_before(None, "d", "someone-else-edited")
        self.assertEqual(self.health.calls, [False, False])

    def test_faults_a_model_already_carries_are_the_baseline_not_a_refusal(self):
        self.health.healthy = False
        state = compute.health_before(None, "d", "r9")
        self.assertEqual([f["name"] for f in state["faults"]], ["Fillet1"])
        # the same faults after the write: nothing new, so the write settles
        settled, _ = compute.settle(None, "d", {"op": "feature_create"}, state)
        self.assertFalse(settled["healthy"])

    def test_an_explicit_checkpoint_starts_the_counters_again(self):
        self.write("entity_delete", "r1")
        self.assertTrue(compute.pending("d"))
        compute.checkpoint(None, "d", "requested")
        self.assertEqual((self.health.calls[-1], compute.pending("d"), compute.status("d")["lastFullCompute"]["reason"]),
                         (True, False, "requested"))


class Writes(unittest.TestCase):
    """The transaction adapter and the parameter writes report the ledger; export checkpoints first."""
    def setUp(self):
        compute.reset()
        self.health = Health()
        self.revision = ["r0"]
        self.commands = []
        self.patches = [patch.object(compute, "health", self.health),
                        patch.object(tx, "_verified", True),
                        patch.object(tx, "target", lambda app, args, write=False: ("doc", self.design, {"documentId": "d", "revision": self.revision[0]})),
                        patch.object(tx, "current_target", lambda app: {"documentId": "d", "revision": self.revision[0]}),
                        patch.object(tx, "recovery_snapshot", lambda design, health_state=None, strict_sketches=(): {}),
                        patch.object(tx, "same", lambda a, b: True),
                        patch.object(tx, "command", lambda app, text: self.commands.append(text.split()[0]))]
        self.design = NS(timeline=NS(markerPosition=3, count=3))
        for p in self.patches:
            p.start()

    def tearDown(self):
        for p in self.patches:
            p.stop()

    def edit(self, op, n):
        def operation(design, value, args):
            self.revision[0] = "r%d" % n
            return {"done": n}
        return tx.execute(NS(activeDocument="doc"), {"op": op, "target": {}, "feature": ref("Lid:1")}, operation)

    def test_feature_edits_read_health_without_recomputing_until_the_third_reaches_a_checkpoint(self):
        results = [self.edit("feature_edit", n) for n in range(1, 5)]
        # one walk before the first write; after that each write's verified health is reused
        self.assertEqual(self.health.calls, [False, False, False, True, False])
        self.assertEqual([r["compute"]["full"] for r in results], [False, False, True, False])
        self.assertEqual([r["compute"]["significance"] for r in results], [2.0, 4.0, 0.0, 2.0])
        self.assertEqual(results[2]["compute"]["reason"], "significance")
        self.assertEqual(self.commands.count("PTransaction.Commit"), 4)

    def test_an_export_after_edits_runs_the_full_recompute_first(self):
        self.edit("feature_create", 1)
        self.health.calls.clear()
        with patch.object(ops, "target", lambda app, args, write=False, model_write=True: ("doc", self.design, {"documentId": "d", "revision": "r1"})), \
                patch.object(ops, "current_target", lambda app: {"documentId": "d", "revision": "r1"}), \
                patch.object(ops, "export_design", lambda design, args: {"path": "out.step"}):
            out = ops.export(None, {"target": {}})
            # the edit's verified health is the baseline; the export's full recompute is held to it
            self.assertEqual((self.health.calls, out["compute"]["lastFullCompute"]["reason"]), ([True], "export"))
            self.assertEqual(out["preExistingFaults"], {"count": 0, "faults": []})
            ops.export(None, {"target": {}})
            # nothing changed since: no recompute, one walk for the baseline
            self.assertEqual(self.health.calls, [True, False])


if __name__ == "__main__":
    unittest.main()
