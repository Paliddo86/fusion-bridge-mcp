#!/usr/bin/env python3
"""Regenerate contracts/describe.json from describe.py: the verb catalog both the Python
bridge and the Rust exe must serve identically. Run after any describe.py change; the
release refuses to ship a stale contract (tools/release.sh)."""
import json, sys, os, datetime
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
import describe
verbs = []
for t in describe._T:
    name, summary, inp, out, timeout, status_verb, long_running, example = (list(t) + [None] * 8)[:8]
    verbs.append({"name": name, "summary": summary, "input": inp or {}, "output": out or {},
                  "timeoutSeconds": timeout, "statusVerb": status_verb, "longRunning": bool(long_running), "example": example or {}})
verbs.sort(key=lambda v: v["name"])
doc = {"contract": "fusion-bridge verb catalog", "frozen": datetime.date.today().isoformat(), "source": "describe.py",
       "note": "The Python bridge and the Rust bridge must both serve this catalog byte-for-byte in meaning: same names, same args, same result shapes, same timeouts. Regenerate ONLY from describe.py and diff in CI.",
       "verbs": verbs}
p = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "contracts", "describe.json")
old = json.load(open(p)) if os.path.exists(p) else {}
if old.get("verbs") == verbs:
    print("contract unchanged (%d verbs)" % len(verbs))
else:
    json.dump(doc, open(p, "w"), indent=2, sort_keys=True); open(p, "a").write("\n")
    print("contract refrozen: %d verbs (was %d)" % (len(verbs), len(old.get("verbs") or [])))

# bridge.json "verbs" is what ab's `commands` listing and verbCount read. It was hand-kept and
# drifted to 65 names against 160 described (adom/adom-bridge#211, 2026-09-22), so it is now
# written from the same catalog, every freeze. Order and every other key are left alone.
root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
bj = os.path.join(root, "bridge.json")
names = [v["name"] for v in verbs]
raw = open(bj).read()
man = json.loads(raw)
if man.get("verbs") != names:
    before = len(man.get("verbs") or [])
    man["verbs"] = names
    json.dump(man, open(bj, "w"), indent=2); open(bj, "a").write("\n")
    print("bridge.json verbs regenerated from the contract: %d (was %d)" % (len(names), before))
else:
    print("bridge.json verbs match the contract (%d)" % len(names))
