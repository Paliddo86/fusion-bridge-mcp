"""Typed mechanical operations, executed for the bridge exe.

The exe (rust/bridge/src/mech.rs) validates every request against schema.json, admits and journals
writes, and serves capabilities and status itself. This package runs the Fusion API work on the main
thread. A write carries the exe's dispatch: it runs only if this add-in claims the dispatch first,
and its exact result is also left for the exe in case the reply is lost (handoff.py).
"""
import json
from pathlib import Path
import threading

import adsk.core

from .core import SESSION_ID, MechanicalError, document_identity, require
from .operations import OPERATIONS, WRITES
from .journal import Journal, JournalError
from . import handoff, transactions, sketches, features, assemblies, purchased, bill_of_materials, geometry, sketch_edits, regular_joints, motion_links, configurations, render_scene, exploded_view, exploded_preview, efficiency, exploded_render, exploded_stills, render_image, cam, layout, drawings

for _name, _handler in {**sketches.OPERATIONS, **features.OPERATIONS, **assemblies.OPERATIONS,
                        **geometry.OPERATIONS, **sketch_edits.OPERATIONS, **regular_joints.OPERATIONS, **motion_links.OPERATIONS}.items():
    OPERATIONS[_name] = lambda app, args, fn=_handler: transactions.execute(app, args, fn)
    WRITES.add(_name)
OPERATIONS.update(assemblies.READS)
OPERATIONS.update(geometry.READS)
OPERATIONS.update(sketch_edits.READS)
OPERATIONS.update(configurations.READS)
OPERATIONS.update(render_scene.READS)
OPERATIONS.update(exploded_view.READS)
# Presentation effects have explicit ownership/disposal, not model PTransaction.
for _name, _handler in {**exploded_preview.WRITES, **exploded_preview.READS}.items():
    OPERATIONS[_name] = lambda app, args, fn=_handler: fn(app, args, _history)
WRITES.update(exploded_preview.WRITES)
OPERATIONS.update(purchased.OPERATIONS)
OPERATIONS.update(bill_of_materials.OPERATIONS)
OPERATIONS.update(efficiency.OPERATIONS)
OPERATIONS.update(exploded_render.OPERATIONS)
OPERATIONS.update(exploded_stills.OPERATIONS)
OPERATIONS.update(render_image.OPERATIONS)
OPERATIONS.update(cam.OPERATIONS)
OPERATIONS.update(layout.OPERATIONS)
OPERATIONS.update(drawings.OPERATIONS)
WRITES.update(purchased.WRITES | bill_of_materials.WRITES | efficiency.WRITES | exploded_render.WRITES | exploded_stills.WRITES | render_image.WRITES | cam.WRITES | layout.WRITES | drawings.WRITES)
OPERATIONS["transaction_probe"] = transactions.probe
WRITES.add("transaction_probe")

SOURCE_VERSION = json.loads((Path(__file__).resolve().parents[2] / "AdomBridge.manifest").read_text(encoding="utf-8"))["version"]
SCHEMA = json.loads((Path(__file__).with_name("schema.json")).read_text(encoding="utf-8"))


class _Proofs:
    """Exploded previews prove earlier writes from the exe's journal (read-only here)."""
    def __init__(self):
        self.journal = Journal()

    def store(self):
        return self.journal


_history = _Proofs()


def error_result(error):
    if isinstance(error, MechanicalError):
        return {"success": False, "sessionId": SESSION_ID, "errorCode": error.code,
                "error": str(error), **error.details}
    if isinstance(error, JournalError):
        return {"success": False, "sessionId": SESSION_ID, "errorCode": error.code, "error": str(error)}
    return {"success": False, "sessionId": SESSION_ID, "errorCode": "mechanical_failed",
            "error": str(error), "recovery": {"verified": False, "strategy": "none"}}


def observed_document(app):
    require(threading.current_thread() is threading.main_thread(), "wrong_thread", "Document observation requires the Fusion main thread.")
    doc = app.activeDocument
    if doc is None:
        return None
    data_file = doc.dataFile
    return {"documentId": document_identity(doc), "creationId": doc.creationId,
            "dataFileId": data_file.id if data_file is not None else None}


def publish_session():
    handoff.publish_session(SESSION_ID, SOURCE_VERSION, transactions.capability())


transactions.on_probe = publish_session


def handle_mechanical(app, args):
    dispatch = args.pop("_dispatch", None) if isinstance(args, dict) else None
    op = args.get("op") if isinstance(args, dict) else None
    if op not in OPERATIONS:
        return error_result(MechanicalError("unsupported_operation", "Discover supported operations with fusion_mech_capabilities."))
    if not (op in WRITES or (op == "health" and args.get("recompute", False))):
        try:
            return {"success": True, "sessionId": SESSION_ID, "operation": op, "data": OPERATIONS[op](app, args)}
        except Exception as error:
            return error_result(error)
    dispatch_id = handoff.dispatch_id(dispatch)
    if dispatch_id is None:
        return error_result(MechanicalError("dispatch_required", "Writes are validated and journaled by the bridge exe; send them through fusion_mechanical."))
    try:
        require(args.get("sessionId") == SESSION_ID, "session_mismatch", "Old-session history is readable, but native execution requires the current add-in session.")
        document = observed_document(app)
        require(handoff.claim(dispatch_id, document, args), "dispatch_expired", "The bridge gave up on this dispatch before it started; it was not executed.")
    except Exception as error:
        return {"success": False, "dispatchId": dispatch_id, "nativeExecutionStarted": False, "result": error_result(error)}
    try:
        result = {"success": True, "sessionId": SESSION_ID, "operation": op, "data": OPERATIONS[op](app, args)}
    except Exception as error:
        result = error_result(error)
    result["operationId"] = args["operationId"]
    try:
        handoff.publish_result(dispatch_id, result)
    except (OSError, TypeError, ValueError):
        pass  # the reply below still carries it
    return {"success": result["success"], "dispatchId": dispatch_id, "nativeExecutionStarted": True,
            "observedDocument": document, "result": result}


def _publish_at_load():
    """Inside Fusion the session is published as the add-in loads; offline tests have no app."""
    try:
        if adsk.core.Application.get() is None:
            return
    except AttributeError:
        return
    try:
        publish_session()
    except Exception:
        pass  # the exe then answers writes with session_mismatch; reads are unaffected


_publish_at_load()
