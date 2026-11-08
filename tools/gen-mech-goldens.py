#!/usr/bin/env python3
"""Freeze the Python mechanical admission layer as goldens for its Rust port.

John's pattern: the Python side generates, the Rust side matches byte for byte. The source is the
dev.9 admission layer, kept verbatim as tests/mech_reference (journal, operation history) and
tests/mech_exe.py (validate, prepare_request, capabilities), with the add-in's schema.json and write
set. The same reference drives the offline add-in tests, so both sides share one contract.

Python's validator walks set intersections, so an input with several faults reports them in
hash order. The script therefore runs its corpus in eight child processes with different
PYTHONHASHSEED values and keeps only the cases all eight runs answer identically.

    python3 tools/gen-mech-goldens.py [--evidence DIR]
"""
import argparse, copy, hashlib, json, os, re, subprocess, sys, tempfile
from pathlib import Path
from types import ModuleType, SimpleNamespace as NS

ROOT = Path(__file__).resolve().parents[1]
GOLDEN = ROOT / "rust" / "core" / "tests" / "golden"
SESSION = "S-GOLDEN-0000"
STALE = "S-STALE-0000"
SOURCE_VERSION = "1.12.0-dev.10"


def load_mechanical():
    """The add-in package (schema, write set) plus the frozen dev.9 admission reference."""
    adsk = ModuleType("adsk"); adsk.core = ModuleType("adsk.core"); adsk.fusion = ModuleType("adsk.fusion")
    adsk.core.Application = object
    adsk.fusion.Design = NS(cast=lambda value: value)
    adsk.fusion.CalculationAccuracy = NS(LowCalculationAccuracy=0, MediumCalculationAccuracy=1, HighCalculationAccuracy=2, VeryHighCalculationAccuracy=3)
    adsk.fusion.FeatureHealthStates = NS(HealthyFeatureHealthState=0)
    adsk.fusion.JointDirections = NS(XAxisJointDirection=0, YAxisJointDirection=1, ZAxisJointDirection=2, CustomJointDirection=3)
    for module in (adsk, adsk.core, adsk.fusion):
        sys.modules[module.__name__] = module
    pkg = ModuleType("golden_commands"); pkg.__path__ = [str(ROOT / "addin/AdomBridge/commands")]
    sys.modules[pkg.__name__] = pkg
    sys.path.insert(0, str(ROOT / "tests"))
    import importlib
    addin = importlib.import_module("golden_commands.mechanical")
    import mech_exe
    from mech_reference import journal as jm, operation_history as hm
    m = NS(SCHEMA=addin.SCHEMA, WRITES=addin.WRITES, transactions=addin.transactions,
           prepare_request=lambda args: mech_exe.prepare_request(args, addin.SCHEMA, addin.WRITES, SESSION),
           capabilities=lambda app, args: mech_exe.capabilities(addin.SCHEMA, SESSION, addin.transactions.capability()))
    return m, jm, hm


# ---- canonical JSON -------------------------------------------------------------------------

CANONICAL_VALUES = [
    None, True, False, 0, 1, -1, 7, 2 ** 53 + 1, -(2 ** 62), 0.0, -0.0, 1.0, -1.5, 100.0, 0.1, 0.30000000000000004,
    1e-05, 1.23e-05, 0.0001, 0.00012, 1e16, 1.5e16, 9999999999999998.0, 1e15, 123456789.12345679, 1e22, 1e-07,
    5e-324, 1.7976931348623157e308, 2.5e-310, 1792500002.123456, 1792500000.125, 42.0, 3.141592653589793,
    "", "plain", "quote\"back\\slash", "ctl\n\r\t\b\f\x00\x01\x1f\x7f", "slash/", "é", "Ω≈ç√", "😀 emoji", "  ",
    "퟿￿", [], {}, [1, [2, [3, {}]]], {"b": 1, "a": 2, "A": 3, "_": 4, "é": 5, "aa": 6, "a ": 7, "10": 8, "9": 9},
    {"nested": {"z": [1.0, 2.5, {"y": None, "x": True}], "k": "v"}}, [0.1, -0.0, 1e100, -1e-100],
]


def canonical_cases(journal):
    out = []
    for value in CANONICAL_VALUES:
        raw = journal.canonical(value)
        out.append({"value": value, "canonical": raw.decode("ascii"), "sha256": hashlib.sha256(raw).hexdigest()})
    return out


# ---- validation corpus ----------------------------------------------------------------------

def enum_strings(schema, acc):
    if isinstance(schema, dict):
        for value in schema.get("enum", []):
            if isinstance(value, str):
                acc.add(value)
        for child in schema.values():
            enum_strings(child, acc)
    elif isinstance(schema, list):
        for child in schema:
            enum_strings(child, acc)
    return acc


def scrub(value, keep, key=None):
    """Keep enum vocabulary and structure; replace every other string with a stable token."""
    if isinstance(value, dict):
        return {k: scrub(v, keep, k) for k, v in value.items()}
    if isinstance(value, list):
        return [scrub(v, keep, key) for v in value]
    if isinstance(value, str):
        if key in ("op", "type", "action") or value in keep:
            return value
        if key == "sessionId":
            return SESSION
        if key == "operationId":
            return "op-" + hashlib.sha256(value.encode()).hexdigest()[:12]
        return "s" + hashlib.sha256(value.encode()).hexdigest()[:10]
    return value


def harvest(evidence, schema, keep, per_op=12):
    found = {}
    def walk(node):
        if isinstance(node, dict):
            if isinstance(node.get("op"), str) and node["op"] in schema:
                args = scrub(node, keep)
                found.setdefault(args["op"], {})[json.dumps(args, sort_keys=True)] = args
            for child in node.values():
                walk(child)
        elif isinstance(node, list):
            for child in node:
                walk(child)
    if evidence and Path(evidence).is_dir():
        for path in sorted(Path(evidence).rglob("*.json")):
            try:
                walk(json.loads(path.read_text(encoding="utf-8")))
            except (ValueError, UnicodeError, OSError):
                continue
    return {op: list(cases.values())[:per_op] for op, cases in found.items()}


def synth(schema, depth=0):
    """A best-effort valid value for one schema node (the golden records Python's verdict either way)."""
    if depth > 12:
        return None
    if "oneOf" in schema:
        variant = schema["oneOf"][0]
        merged = {k: v for k, v in schema.items() if k != "oneOf"}
        merged["properties"] = {**schema.get("properties", {}), **variant.get("properties", {})}
        merged["required"] = list(dict.fromkeys(schema.get("required", []) + variant.get("required", [])))
        merged["type"] = schema.get("type", variant.get("type"))
        return synth(merged, depth + 1)
    if "enum" in schema:
        return schema["enum"][0]
    kind = schema.get("type")
    if kind == "object":
        out = {}
        for key in schema.get("required", []):
            if key in ("op", "sessionId", "operationId", "target"):
                continue
            out[key] = synth(schema.get("properties", {}).get(key, {}), depth + 1)
        return out
    if kind == "array":
        return [synth(schema.get("items", {}), depth + 1) for _ in range(schema.get("minItems", 0))]
    if kind in ("number", "integer"):
        low = schema.get("minimum", schema.get("exclusiveMinimum", 0))
        value = low + (1 if "exclusiveMinimum" in schema else 0)
        if "maximum" in schema:
            value = min(value, schema["maximum"])
        return int(value) if kind == "integer" else float(value)
    if kind == "boolean":
        return False
    if kind == "string":
        return "x"
    return {}


TARGET = {"documentId": SESSION + ":doc", "revision": "r1"}


def base_args(op, op_schema):
    args = {"op": op}
    args.update(synth(op_schema))
    required = op_schema.get("required", [])
    if "sessionId" in required: args["sessionId"] = SESSION
    if "operationId" in required: args["operationId"] = "op-synth-" + op
    if "target" in required: args["target"] = dict(TARGET)
    return args


def mutations(args, op_schema):
    out = []
    props = op_schema.get("properties", {})
    for key in list(args):
        if key == "op":
            continue
        dropped = dict(args); dropped.pop(key); out.append(dropped)
    for key, prop in props.items():
        kind = prop.get("type")
        bad = {"object": "nope", "string": 7, "boolean": "yes", "number": "1", "integer": 1.5, "array": {}}.get(kind)
        if bad is not None:
            out.append({**args, key: bad})
        if kind in ("number", "integer"):
            if "minimum" in prop: out.append({**args, key: prop["minimum"] - 1})
            if "maximum" in prop: out.append({**args, key: prop["maximum"] + 1})
            if "exclusiveMinimum" in prop: out.append({**args, key: prop["exclusiveMinimum"]})
            out.append({**args, key: True})
        if "enum" in prop:
            out.append({**args, key: "__not_in_enum__"})
        if kind == "array" and "maxItems" in prop:
            out.append({**args, key: [synth(prop.get("items", {}))] * (prop["maxItems"] + 1)})
    out.append({**args, "unexpectedField": 1})
    if "sessionId" in args:
        out.append({**args, "sessionId": STALE})
    if "operationId" in args:
        out.append({**args, "operationId": ""})
        out.append({**args, "operationId": "o" * 129})
        out.append({**args, "operationId": 5})
    out.append({**args, "caller": {"agent": "x"}, "_callerBudgetMs": 1000})
    return out


EXTRA = [
    "not an object", [], {}, {"op": "no_such_op"}, {"op": 5}, {"op": "inspect"}, {"op": "health"},
    {"op": "health", "recompute": True}, {"op": "health", "recompute": True, "sessionId": SESSION, "operationId": "h1"},
    {"op": "inspect", "target": dict(TARGET)},
]


def nested_mutations(args, limit=24):
    """One fault at a time below the top level: retype a leaf, drop a key, add an unknown key."""
    out, paths = [], []
    def walk(node, path):
        if isinstance(node, dict):
            if path:
                paths.append((path, "dict"))
            for k, v in node.items():
                if not path and k in ("op", "sessionId", "operationId"):
                    continue
                walk(v, path + [k])
        elif isinstance(node, list):
            if path:
                paths.append((path, "list"))
            for i, v in enumerate(node[:3]):
                walk(v, path + [i])
        elif path:
            paths.append((path, "leaf"))
    walk(args, [])
    def setp(root, path, value, drop=False):
        root = copy.deepcopy(root); node = root
        for k in path[:-1]:
            node = node[k]
        if drop:
            del node[path[-1]]
        else:
            node[path[-1]] = value
        return root
    for path, kind in paths:
        if len(out) >= limit:
            break
        if len(path) < 2:
            continue
        current = args
        for k in path:
            current = current[k]
        if kind == "leaf":
            bad = "str" if isinstance(current, (int, float)) and not isinstance(current, bool) else 3 if isinstance(current, str) else "yes"
            out.append(setp(args, path, bad))
        elif kind == "dict":
            out.append(setp(args, path, "notobject"))
            out.append(setp(args, path + ["__unknown__"], 1))
            for k in list(current)[:2]:
                out.append(setp(args, path + [k], None, drop=True))
        else:
            out.append(setp(args, path, {"not": "a list"}))
    return out[:limit]


def corpus(m, evidence):
    keep = enum_strings(m.SCHEMA, set())
    real = harvest(evidence, m.SCHEMA, keep)
    cases = list(EXTRA)
    for op, op_schema in m.SCHEMA.items():
        args = base_args(op, op_schema)
        cases.append(args)
        cases.extend(mutations(args, op_schema))
        cases.extend(nested_mutations(args))
        for sample in real.get(op, []):
            cases.append(sample)
            cases.extend(mutations(sample, op_schema)[:6])
            cases.extend(nested_mutations(sample))
    return cases


def run_validation(m, cases):
    out = []
    for args in cases:
        try:
            frozen, write = m.prepare_request(copy.deepcopy(args))
            out.append({"args": args, "ok": {"canonical": json.dumps(frozen, sort_keys=True, separators=(",", ":")), "write": write}})
        except Exception as error:  # MechanicalError / JournalError
            out.append({"args": args, "error": {"code": getattr(error, "code", type(error).__name__), "message": str(error),
                                                "details": getattr(error, "details", {})}})
    return out


# ---- history scenarios ----------------------------------------------------------------------

class Clock:
    def __init__(self): self.t = 1792500000.0; self.step = 0
    def time(self):
        self.step += 1
        # mostly exact binary fractions, every fifth one a repr-hard value
        return self.t + self.step * 0.125 + (0.000001 * self.step if self.step % 5 == 0 else 0)


def history_scenarios(m, jm, hm):
    scenarios = []

    def fresh(max_records=jm.MAX_RECORDS, fail_on=None):
        clock = Clock(); jm.time = NS(time=clock.time)
        directory = Path(tempfile.mkdtemp(prefix="mech-golden-"))
        class Injecting(jm.Journal):
            calls = 0
            def transition(self, *a, **k):
                Injecting.calls += 1
                if fail_on is not None and Injecting.calls == fail_on:
                    raise jm.JournalError("operation_storage_failed", "OSError: injected persistence failure")
                return super().transition(*a, **k)
        journal = Injecting(directory, max_records=max_records)
        return hm.OperationHistory(SESSION, SOURCE_VERSION, journal), journal

    def rows(journal):
        import sqlite3
        if not journal.path.exists():
            return []
        con = sqlite3.connect(str(journal.path))
        try:
            return [{"sessionId": s, "operationId": o, "record": bytes(r).decode("ascii"), "digest": d}
                    for s, o, r, d in con.execute("SELECT session_id,operation_id,record_json,record_digest FROM operations ORDER BY session_id,operation_id")]
        finally:
            con.close()

    def write_req(op_id, extra=None):
        req = {"op": "parameter_set", "sessionId": SESSION, "operationId": op_id, "target": dict(TARGET),
               "name": "w", "expression": "42 mm"}
        req.update(extra or {})
        return json.loads(jm.canonical(req, jm.MAX_REQUEST_BYTES))

    def scenario(name, body, **kw):
        history, journal = fresh(**kw)
        steps = []
        def call(kind, *args):
            fn = getattr(history, kind)
            result = fn(*copy.deepcopy(args))
            steps.append({"call": kind, "args": list(args), "out": result})
            return result
        body(call)
        scenarios.append({"name": name, "options": kw, "steps": steps, "rows": rows(journal)})

    doc = {"documentId": SESSION + ":doc", "creationId": "c1", "dataFileId": None}
    ok = {"success": True, "sessionId": SESSION, "operation": "parameter_set", "data": {"value": 42}, "operationId": "w1"}
    bad = {"success": False, "sessionId": SESSION, "errorCode": "expression_invalid", "error": "bad", "operationId": "w2"}

    def s_success(call):
        r = write_req("w1"); d, rep = call("admit", r, "d-1")
        call("start", d, r, doc); call("finish", d, ok)
        call("status", {"sessionId": SESSION, "operationId": "w1"})
        call("admit", r, "d-2")  # replay of a terminal record
    scenario("success_then_replay", s_success)

    def s_failure(call):
        r = write_req("w2"); d, _ = call("admit", r, "d-1"); call("start", d, r, doc); call("finish", d, bad)
        call("status", {"sessionId": SESSION, "operationId": "w2"})
    scenario("handler_failure", s_failure)

    def s_conflict(call):
        r = write_req("w3"); d, _ = call("admit", r, "d-1")
        call("admit", write_req("w3", {"expression": "43 mm"}), "d-2")
        call("admit", r, "d-3")  # same request while dispatched: pending reply
        call("status", {"sessionId": SESSION, "operationId": "w3"})
    scenario("conflict_and_pending", s_conflict)

    def s_not_executed(call):
        r = write_req("w4"); d, _ = call("admit", r, "d-1")
        call("not_executed", d, {"success": False, "errorCode": "request_expired_before_execution", "error": "expired"})
        call("start", d, r, doc)  # a late handler loses its claim
        call("status", {"sessionId": SESSION, "operationId": "w4"})
    scenario("not_executed_wins_claim", s_not_executed)

    def s_session(call):
        call("admit", write_req("w5", {"sessionId": STALE}), "d-1")
        call("status", {"sessionId": STALE, "operationId": "w5"})
        call("status", {"sessionId": SESSION, "operationId": "nope"})
        call("status", {"sessionId": "", "operationId": "x"})
        call("status", {"operationId": "x"})
        call("not_executed", {"sessionId": STALE, "operationId": "w5", "requestDigest": "0" * 64, "dispatchId": "d"}, {"success": False, "error": "x"})
        call("not_executed", None, {"success": False, "error": "passthrough"})
    scenario("session_and_unknown", s_session)

    def s_capacity(call):
        call("admit", write_req("c1"), "d-1"); call("admit", write_req("c2"), "d-2")
    scenario("capacity", s_capacity, max_records=1)

    def s_storage_after_execution(call):
        r = write_req("w6"); d, _ = call("admit", r, "d-1"); call("start", d, r, doc)
        call("finish", d, {**ok, "operationId": "w6"})  # 3rd transition fails: dispatched, executing, [terminal]
        call("status", {"sessionId": SESSION, "operationId": "w6"})
        call("admit", r, "d-2")
    scenario("storage_failure_after_execution", s_storage_after_execution, fail_on=3)

    def s_storage_at_claim(call):
        r = write_req("w7"); call("admit", r, "d-1")  # the dispatch claim (1st transition) fails
        call("status", {"sessionId": SESSION, "operationId": "w7"})
    scenario("storage_failure_at_dispatch_claim", s_storage_at_claim, fail_on=1)

    def s_identity(call):
        r = write_req("w8"); d, _ = call("admit", r, "d-1")
        call("start", {**d, "requestDigest": "f" * 64}, r, doc)
        call("start", d, write_req("w8", {"expression": "1 mm"}), doc)
    scenario("start_identity_mismatch", s_identity)
    return scenarios


def capabilities_golden(m):
    caps = m.capabilities(None, {})
    caps["sessionId"] = "<session>"
    return caps


def child(evidence):
    m, jm, hm = load_mechanical()
    print(json.dumps({"validation": run_validation(m, corpus(m, evidence))}))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--evidence", default=str(ROOT.parent / "research" / "fusion-mechanical-verification-2026-10-02"))
    ap.add_argument("--child", action="store_true")
    a = ap.parse_args()
    if a.child:
        return child(a.evidence)
    runs = []
    for seed in ("1", "2", "3", "4", "5", "6", "7", "8"):
        env = dict(os.environ, PYTHONHASHSEED=seed)
        out = subprocess.run([sys.executable, __file__, "--child", "--evidence", a.evidence], env=env, capture_output=True, text=True, check=True)
        runs.append(json.loads(out.stdout)["validation"])
    stable = [cases[0] for cases in zip(*runs) if all(c == cases[0] for c in cases)]
    # Keep every distinct outcome, at most four of the smallest inputs per (op, outcome).
    groups = {}
    for case in stable:
        op = case["args"].get("op") if isinstance(case["args"], dict) else None
        outcome = ("ok", case["ok"]["write"]) if "ok" in case else (case["error"]["code"], case["error"]["message"])
        groups.setdefault((str(op), json.dumps(outcome)), []).append(case)
    kept = []
    for key in sorted(groups):
        kept.extend(sorted(groups[key], key=lambda c: len(json.dumps(c["args"])))[:4])
    total, stable = len(stable), kept
    m, jm, hm = load_mechanical()
    meta = {"source": "addin/AdomBridge/commands/mechanical (dev.9 merged onto 1.11.44)", "generator": "tools/gen-mech-goldens.py", "session": SESSION}
    GOLDEN.mkdir(parents=True, exist_ok=True)
    def dump(name, body):
        compact = name == "mech_validation.json"
        text = json.dumps({**meta, **body}, indent=None if compact else 1, separators=(",", ":") if compact else None, ensure_ascii=True)
        (GOLDEN / name).write_text(text + "\n", encoding="utf-8")
    dump("mech_canonical.json", {"cases": canonical_cases(jm)})
    dump("mech_validation.json", {"cases": stable, "droppedNondeterministic": len(runs[0]) - total, "sampledFrom": total})
    dump("mech_capabilities.json", {"capabilities": capabilities_golden(m), "writes": sorted(m.WRITES)})
    dump("mech_history.json", {"sourceVersion": SOURCE_VERSION, "scenarios": history_scenarios(m, jm, hm)})
    print("goldens: %d canonical, %d validation kept of %d stable (%d dropped as hash-order dependent), 9 history scenarios"
          % (len(CANONICAL_VALUES), len(stable), total, len(runs[0]) - total))


if __name__ == "__main__":
    main()
