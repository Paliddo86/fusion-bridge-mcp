#!/usr/bin/env python3
"""Promote helper: rewrite the public manifest from the insiders manifest, gated on version.

Called by tools/promote.sh. Reads the raw insiders fetch from the file in argv[1], requires
its version to equal argv[2] (promote what insiders actually soak-tested, nothing else),
strips the tier field, and writes the public manifest file argv[3]."""
import json, sys

raw = open(sys.argv[1], encoding="utf-8").read()
want, mf = sys.argv[2], sys.argv[3]
m, _ = json.JSONDecoder().raw_decode(raw[raw.index("{"):])
m["released_at"] = __import__("datetime").datetime.now(__import__("datetime").timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")  # the PUBLIC release moment, not the template's
if m.get("version") != want:
    sys.exit("REFUSED: insiders tier serves %r, not %r. Promote what insiders actually ran."
             % (m.get("version"), want))
m.pop("tier", None)
json.dump(m, open(mf, "w"), indent=2)
print("public manifest staged at", mf, "->", want)
