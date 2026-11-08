"""The bridge exe's mechanical protocol in Python, for offline add-in tests.

Mirrors rust/bridge/src/mech.rs step for step (validate, admit, dispatch with a claim, settle from
the add-in's answer or its result file) on top of tests/mech_reference, the dev.9 admission code the
Rust port is pinned to. The add-in under test only sees what the exe would send it.
"""
import copy
import json
import math
import os
import uuid

from mech_reference.journal import (Journal, JournalError, MAX_DATABASE_BYTES, MAX_RECORDS, MAX_RECORD_BYTES,
                                    MAX_REQUEST_BYTES, STORAGE_VERSION, canonical)
from mech_reference.operation_history import OperationHistory

UNITS = {"length": "mm", "angle": "deg", "mass": "kg", "area": "mm2", "volume": "mm3"}
DISPATCH_FIELDS = ("success", "dispatchId", "nativeExecutionStarted", "observedDocument", "result")


class MechanicalError(Exception):
    def __init__(self, code, message, **details):
        super().__init__(message)
        self.code, self.details = code, details


def require(condition, code, message, **details):
    if not condition:
        raise MechanicalError(code, message, **details)


def validate(value, schema, path="args"):
    """dev.9 __init__.validate, verbatim."""
    if "oneOf" in schema:
        variants = schema["oneOf"]
        if isinstance(value, dict):
            for discriminator in ("type", "action"):
                if discriminator in value:
                    matching = [variant for variant in variants if value[discriminator] in
                                variant.get("properties", {}).get(discriminator, {}).get("enum", [])]
                    if len(matching) == 1:
                        validate(value, matching[0], path)
                        variants = []
                        break
        if variants:
            successes = 0
            for variant in variants:
                try:
                    validate(value, variant, path)
                    successes += 1
                except MechanicalError:
                    pass
            require(successes == 1, "invalid_input", path + " must match exactly one supported schema variant")
    kinds = {"object": dict, "string": str, "boolean": bool, "number": (int, float), "integer": int, "array": list}
    kind = schema.get("type")
    if kind:
        require(isinstance(value, kinds[kind]) and not (kind in ("number", "integer") and isinstance(value, bool)),
                "invalid_input", path + " must be " + kind)
    if kind in ("number", "integer"):
        require(math.isfinite(value), "invalid_input", path + " must be finite")
        if "exclusiveMinimum" in schema:
            require(value > schema["exclusiveMinimum"], "invalid_input", path + " is too small")
        if "minimum" in schema:
            require(value >= schema["minimum"], "invalid_input", path + " is too small")
        if "maximum" in schema:
            require(value <= schema["maximum"], "invalid_input", path + " is too large")
    if kind == "array":
        require(schema.get("minItems", 0) <= len(value) <= schema.get("maxItems", 10000), "invalid_input", path + " has an invalid item count")
        for index, item in enumerate(value):
            validate(item, schema.get("items", {}), path + "[" + str(index) + "]")
    if "enum" in schema:
        require(value in schema["enum"], "invalid_input", path + " has an unsupported value")
    if kind == "object":
        require(len(value) >= schema.get("minProperties", 0), "invalid_input", path + " requires at least one property")
        for key in schema.get("required", []):
            require(key in value, "invalid_input", path + "." + key + " is required")
        props = schema.get("properties", {})
        if schema.get("additionalProperties") is False:
            require(not (set(value) - set(props)), "invalid_input", path + " has unknown fields")
        for key in value.keys() & props.keys():
            validate(value[key], props[key], path + "." + key)
        if isinstance(schema.get("additionalProperties"), dict):
            for key in value.keys() - props.keys():
                validate(value[key], schema["additionalProperties"], path + "." + key)


def prepare_request(args, schema, writes, session_id):
    """dev.9 __init__.prepare_request, with the schema, write set and current session passed in."""
    require(isinstance(args, dict), "invalid_input", "Arguments must be an object.")
    args = {key: value for key, value in args.items() if key not in ("caller", "_callerBudgetMs")}
    op = args.get("op")
    require(isinstance(op, str) and op in schema, "unsupported_operation", "Discover supported operations with fusion_mech_capabilities.")
    validate(args, schema[op])
    allowed = set(schema[op]["properties"]) | {"op", "sessionId", "operationId", "target"}
    require(not (set(args) - allowed), "invalid_input", "Unknown operation arguments: " + ", ".join(sorted(set(args) - allowed)))
    write = op in writes or (op == "health" and args.get("recompute", False))
    if write:
        require(args.get("sessionId") == session_id, "session_mismatch", "Old-session history is readable, but native execution requires the current add-in session.")
        require(isinstance(args.get("operationId"), str) and 1 <= len(args["operationId"]) <= 128,
                "operation_id_required", "Every write needs a unique operationId; retries must preserve the exact request.")
    return json.loads(canonical(args, MAX_REQUEST_BYTES)), write


def capabilities(schema, session_id, transactions_capability):
    """dev.9 __init__.capabilities, with the schema, session and probe state passed in."""
    return {"success": True, "sessionId": session_id, "schemaVersion": 1, "units": UNITS,
            "operations": schema, "transactions": {"genericAtomicBatch": False, "typedModelOperations": transactions_capability,
            "parameterRecovery": "compensating expression restore with semantic readback",
            "newDesignRecovery": "discard only the new unsaved document",
            "externalEffects": "exports publish atomically; model recovery does not undo files"},
            "idempotency": {"scope": "durable history keyed by add-in session and operation ID", "capacity": MAX_RECORDS,
                            "storageVersion": STORAGE_VERSION, "databaseByteLimit": MAX_DATABASE_BYTES,
                            "recordByteLimit": MAX_RECORD_BYTES, "admission": "FULL SQLite commit before native dispatch",
                            "restartBehavior": "old-session history is readable; native execution never replays across sessions"}}


class Exe:
    """One exe over one journal directory, dispatching to the add-in package `m`."""

    def __init__(self, m, folder, source_version="offline-test"):
        self.m, self.folder = m, folder
        self.history = OperationHistory(m.SESSION_ID, source_version, Journal(folder))

    def store(self):
        return self.history.store()

    def admit(self, request, dispatch_id):
        return self.history.admit(request, dispatch_id)

    def status(self, args):
        return self.history.status(args)

    def start(self, descriptor, request, document):
        return self.history.start(descriptor, request, document)

    def _dispatch_file(self, dispatch_id, suffix):
        return os.path.join(self.folder, "dispatch", dispatch_id + suffix)

    def call(self, app, args):
        m = self.m
        envelope = {k: args.pop(k) for k in ("foreground", "foregroundReason") if isinstance(args, dict) and k in args}
        try:
            frozen, write = prepare_request(copy.deepcopy(args), m.SCHEMA, m.WRITES, m.SESSION_ID)
        except (MechanicalError, JournalError) as error:
            return {"success": False, "sessionId": m.SESSION_ID, "errorCode": error.code, "error": str(error), **getattr(error, "details", {})}
        if not write:
            return m.handle_mechanical(app, {**copy.deepcopy(frozen), **envelope})
        dispatch_id = uuid.uuid4().hex
        descriptor, reply = self.history.admit(frozen, dispatch_id)
        if reply is not None:
            return reply
        return self.dispatch(app, frozen, descriptor)

    def dispatch(self, app, frozen, descriptor):
        answer = self.m.handle_mechanical(app, {**copy.deepcopy(frozen), "_dispatch": {"dispatchId": descriptor["dispatchId"]}})
        return self.settle(descriptor, frozen, answer)

    def settle(self, descriptor, frozen, answer):
        dispatch_id = descriptor["dispatchId"]
        answered = isinstance(answer, dict) and answer.get("dispatchId") == dispatch_id
        if answered and answer.get("nativeExecutionStarted") is True:
            return self._started(descriptor, frozen, answer.get("observedDocument"), answer)
        claim = self._dispatch_file(dispatch_id, ".claim")
        os.makedirs(os.path.dirname(claim), exist_ok=True)
        try:
            os.close(os.open(claim, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600))
        except FileExistsError:
            with open(claim, encoding="utf-8") as stream:
                text = stream.read()
            state = json.loads(text) if text else {}
            if state.get("state") != "expired":
                return self._started(descriptor, frozen, state.get("observedDocument"), answer)
        result = answer["result"] if answered else answer
        return self._envelope(self.history.not_executed(descriptor, result), answer if answered else {})

    def _started(self, descriptor, frozen, document, answer):
        pending = self.history.start(descriptor, frozen, document)
        if pending is not None:
            return self._envelope(pending, answer)
        path = self._dispatch_file(descriptor["dispatchId"], ".result.json")
        result = None
        if os.path.exists(path):
            with open(path, encoding="ascii") as stream:
                result = json.load(stream)
        elif isinstance(answer, dict) and answer.get("dispatchId") == descriptor["dispatchId"]:
            result = answer.get("result")
        if result is None:
            record = self.store().lookup(descriptor["sessionId"], descriptor["operationId"])
            return self._envelope(self.history.pending_reply(record), answer)
        return self._envelope(self.history.finish(descriptor, result), answer)

    @staticmethod
    def _envelope(reply, answer):
        if isinstance(reply, dict) and isinstance(answer, dict):
            for key, value in answer.items():
                if key not in DISPATCH_FIELDS and key not in reply:
                    reply[key] = value
        return reply
