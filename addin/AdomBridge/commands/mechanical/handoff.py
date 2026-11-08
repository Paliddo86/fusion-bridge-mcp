"""Files the bridge exe and this add-in share for mechanical writes (rust/bridge/src/mech.rs).

The exe validates, admits and journals every write; this add-in only executes it. Three small
files carry what the exe cannot observe from outside Fusion:
- session.json: this add-in session's ID and transaction-probe state, for capabilities and
  the exe's session check. Trusted by the exe only while the Fusion process that wrote it runs.
- dispatch/<id>.claim: created exclusively before a write executes. The exe creates it only to
  expire a dispatch it gave up on, so exactly one side wins and "never ran" stays provable.
- dispatch/<id>.result.json: the exact result, kept for the exe when its reply is lost.
"""
import json
import os
import re
import time

from .journal import storage_root

PROTOCOL = "exe-journal-1"
_DISPATCH_ID = re.compile(r"[0-9a-f]{32}")


def _write_atomically(path, data):
    staged = path.with_name(path.name + ".tmp")
    with open(staged, "wb") as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(staged, path)


def publish_session(session_id, source_version, transactions):
    root = storage_root()
    root.mkdir(parents=True, exist_ok=True)
    _write_atomically(root / "session.json", json.dumps({
        "protocol": PROTOCOL, "sessionId": session_id, "pid": os.getpid(), "startedAt": time.time(),
        "sourceVersion": source_version, "transactions": transactions}).encode("utf-8"))


def dispatch_id(dispatch):
    """The exe's dispatch ID, or None. Never a path: 32 lowercase hex characters only."""
    value = dispatch.get("dispatchId") if isinstance(dispatch, dict) else None
    return value if isinstance(value, str) and _DISPATCH_ID.fullmatch(value) else None


def claim(dispatch_id, document, args):
    """Win the dispatch before executing. False means the exe already expired it."""
    folder = storage_root() / "dispatch"
    folder.mkdir(parents=True, exist_ok=True)
    try:
        descriptor = os.open(folder / (dispatch_id + ".claim"), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        return False
    # The claim is owned from here on; its contents only help the exe reconcile later.
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            stream.write(json.dumps({"state": "started", "sessionId": args.get("sessionId"),
                                     "operationId": args.get("operationId"), "observedDocument": document, "at": time.time()}))
            stream.flush()
            os.fsync(stream.fileno())
    except (OSError, TypeError, ValueError):
        pass
    return True


def publish_result(dispatch_id, result):
    folder = storage_root() / "dispatch"
    _write_atomically(folder / (dispatch_id + ".result.json"),
                      json.dumps(result, sort_keys=True, separators=(",", ":"), allow_nan=False).encode("ascii"))
