"""Verified Fusion PTransaction adapter; excludes documents and external files.

Autodesk's FusionMCPSample uses these text commands. Availability alone is not
proof: this adapter is enabled only after a session probe demonstrates both abort
and commit effects, followed by semantic restoration. Commands fail closed.
"""
import json
import math
import uuid

import adsk.core

from . import compute
from .core import (MechanicalError, active, current_target, health, recovery_snapshot,
                   require, resolve, same, target, write_context)

_verified = False
_probe_evidence = None
# Called after every probe so the exe's capabilities see the new state (set by __init__).
on_probe = None


def capability():
    return {"verified": _verified, "backend": "PTransaction", "evidence": _probe_evidence,
            "scope": "one active Fusion Design; no document lifecycle, files or cloud effects"}


def command(app, text):
    response = str(app.executeTextCommand(text)).strip()
    require(response == "1", "transaction_unavailable", "Fusion did not acknowledge transaction command.",
            command=text.split()[0], response=response)


def equivalent(left, right):
    if isinstance(left, float) and isinstance(right, (float, int)):
        return math.isclose(left, right, rel_tol=1e-8, abs_tol=1e-8)
    if isinstance(left, dict) and isinstance(right, dict):
        return left.keys() == right.keys() and all(equivalent(left[k], right[k]) for k in left)
    if isinstance(left, list) and isinstance(right, list):
        return len(left) == len(right) and all(equivalent(a, b) for a, b in zip(left, right))
    return left == right


def probe(app, args):
    try:
        return _probe(app, args)
    finally:
        if on_probe is not None:
            try:
                on_probe()
            except Exception:
                pass


def _probe(app, args):
    global _verified, _probe_evidence
    doc, design, _ = target(app, args, True)
    # faults the model already carries are its baseline: the probe must leave exactly those
    baseline = health(design, False)
    before = recovery_snapshot(design, baseline)
    name = "AdomProbe" + uuid.uuid4().hex
    started = False
    try:
        command(app, 'PTransaction.Start "Adom recovery capability probe"')
        started = True
        parameter = design.userParameters.add(name, adsk.core.ValueInput.createByString("1 mm"), "mm", "temporary recovery probe")
        require(parameter is not None, "transaction_probe_failed", "Probe parameter creation failed.")
        command(app, "PTransaction.Abort")
        started = False
        require(design.userParameters.itemByName(name) is None, "transaction_probe_failed", "Abort did not remove the probe parameter.")
        command(app, 'PTransaction.Start "Adom commit capability probe"')
        started = True
        design.userParameters.add(name, adsk.core.ValueInput.createByString("2 mm"), "mm", "temporary recovery probe")
        command(app, "PTransaction.Commit")
        started = False
        parameter = design.userParameters.itemByName(name)
        require(parameter is not None and abs(parameter.value - 0.2) < 1e-9, "transaction_probe_failed", "Commit did not preserve the probe parameter.")
        require(bool(parameter.deleteMe()), "transaction_probe_failed", "Could not remove committed probe parameter.")
        compute.restored(design, baseline)
        require(equivalent(before, recovery_snapshot(design)), "transaction_probe_failed", "Probe did not restore the semantic witness.")
        _verified = True
        _probe_evidence = {"abortRemovedParameter": True, "commitPreservedParameter": True,
                           "semanticRestoration": True, "applicationVersion": str(getattr(app, "version", "unknown"))}
        return {"target": current_target(app), "transaction": capability(), "preExistingFaults": compute.summary(baseline)}
    except Exception as exc:
        _verified = False
        recovery = {"verified": False, "strategy": "abort_and_remove_probe_parameter"}
        try:
            require(same(app.activeDocument, doc), "wrong_document", "Active document changed during probe.")
            if started:
                command(app, "PTransaction.Abort")
            parameter = design.userParameters.itemByName(name)
            if parameter:
                require(parameter.deleteMe(), "recovery_failed", "Probe parameter deletion failed.")
            compute.restored(design, baseline)
            recovery["verified"] = equivalent(before, recovery_snapshot(design))
        except Exception as restore_error:
            recovery["error"] = str(restore_error)
        raise MechanicalError("transaction_probe_failed", str(exc), recovery=recovery)


def _retarget(value, fresh):
    if isinstance(value, dict):
        if "documentId" in value and "revision" in value:
            value.update(fresh)
        for nested in value.values():
            _retarget(nested, fresh)
    elif isinstance(value, list):
        for nested in value:
            _retarget(nested, fresh)


def _native(entity):
    try:
        return getattr(entity, "nativeObject", None) or entity
    except Exception:
        return entity


def sketches_named(design, value, args):
    """The sketches a write's references name: a sketch, or the sketch that owns a referenced sketch curve,
    point, constraint or dimension. The write edits or reads these, so their constraint graph must read
    completely before it starts. A profile's sketch is read only for the profile. A reference that does
    not resolve is left for the operation to refuse."""
    found = []
    pending = [item for key, item in args.items() if key != "target"]
    while pending:
        item = pending.pop()
        if isinstance(item, list):
            pending.extend(item)
            continue
        if not isinstance(item, dict):
            continue
        kind = item.get("objectType")
        if not (isinstance(kind, str) and "documentId" in item):
            pending.extend(item.values())
            continue
        name = kind.split("::")[-1]
        if name == "Profile" or not (name.startswith("Sketch") or name.endswith("Constraint")):
            continue
        try:
            entity = resolve(design, value, item)
        except (MechanicalError, RuntimeError, AttributeError):
            continue
        sketch = _native(entity if name == "Sketch" else getattr(entity, "parentSketch", None))
        if sketch is not None and not any(same(sketch, other) for other in found):
            found.append(sketch)
    return found


def execute(app, args, operation):
    require(_verified, "transaction_not_verified", "Run transaction_probe in this add-in session before typed feature mutations.")
    doc, design, value = target(app, args, True)
    require(design.timeline.markerPosition == design.timeline.count, "timeline_not_at_end", "Finish editing history before a typed model mutation.")
    # the faults the model already carries: the write may not add one or change one (compute.judge)
    baseline = compute.health_before(design, value["documentId"], value["revision"])
    before = recovery_snapshot(design, baseline, sketches_named(design, value, args))
    edits = compute.subjects(design, value, args)
    started, committed = False, False
    try:
        command(app, 'PTransaction.Start "Adom mechanical operation"')
        started = True
        with write_context(baseline):
            serialize = operation(design, value, args)
            require(same(app.activeDocument, doc), "wrong_document", "Active document changed during the operation.")
            state, settled = compute.settle(design, value["documentId"], args, baseline, edits)
        provisional = current_target(app)
        result = serialize(provisional) if callable(serialize) else serialize
        result.update(target=provisional, health=state, transactional=True, preExistingFaults=compute.summary(baseline, state))
        json.dumps(result, allow_nan=False)  # Serialization is validated before commit.
        command(app, "PTransaction.Commit")
        committed = True
        fresh = current_target(app)
        _retarget(result, fresh)
        result["transaction"] = {"committed": True, "backend": "PTransaction"}
        result["compute"] = compute.committed(value["documentId"], fresh["revision"], state, settled)
        return result
    except Exception as exc:
        if committed:
            # Readback after commit can fail, but the model has already changed.
            return {"transaction": {"committed": True, "backend": "PTransaction"},
                    "inspectionError": str(exc), "refreshRequired": True}
        recovery = {"strategy": "PTransaction.Abort", "verified": False,
                    "witness": "complete supported configuration graph including inactive cells, parameters, feature state, sketch geometry/constraint graph and offset links, construction axes, body geometry metrics, component metadata/materials, occurrence transforms/grounding, joint attachments/origins/motion/limits, motion links and timeline marker"}
        try:
            require(same(app.activeDocument, doc), "wrong_document", "Refusing to abort another document's transaction.")
            if started:
                command(app, "PTransaction.Abort")
            compute.restored(design, baseline)
            recovery["verified"] = equivalent(before, recovery_snapshot(design))
        except Exception as restore_error:
            recovery["error"] = str(restore_error)
        details = {"recovery": recovery, "causeCode": exc.code if isinstance(exc, MechanicalError) else
                   ("unsupported_capability" if isinstance(exc, AttributeError) else "kernel_error")}
        if isinstance(exc, MechanicalError) and exc.details:
            details["causeDetails"] = exc.details
        try:
            details["target"] = current_target(app)
        except Exception:
            details["refreshRequired"] = True
        raise MechanicalError("mutation_failed", str(exc), **details)
