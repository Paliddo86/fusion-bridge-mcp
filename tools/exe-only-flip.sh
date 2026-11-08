#!/usr/bin/env bash
# Phase 4, the package flip (run ONLY on John's word): the release becomes the Rust exe alone.
#   - bridge.json: spawn.kind exe, entrypoint adom-fusion-bridge.exe, no python dependency
#   - tools/release-files.txt: the Python bridge and its helpers leave the zip (describe.py stays
#     a BUILD-time input for freeze-contract; the add-in tree and resources ship as before)
#   - tools/release.sh keeps its contract freeze and the exe build; the python import checks go
# Idempotent; prints what changed; commits nothing. Review the diff, then tools/release.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
python3 - <<'PY'
import json, re, pathlib
b = pathlib.Path("bridge.json"); m = json.loads(b.read_text(encoding="utf-8"))
sp = m["spawn"]
sp.update({"kind": "exe", "entrypoint": "adom-fusion-bridge.exe", "killImageName": "adom-fusion-bridge.exe",
           "_hint": "spawn.kind exe: one sha256-verified binary, no interpreter provisioning. The Python bridge left the package in Phase 4 of the Rust port (2026-09); an owner pref of \"sidecar\" in ~/.adom/fusion-bridge/prefs.json is honoured only where server.py still sits beside the exe."})
m.get("dependencies", {}).pop("python", None)
b.write_text(json.dumps(m, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
print("bridge.json: spawn.kind exe, python dependency dropped")
drop = {"server.py", "aps.py", "fusion_detect.py", "fusion_helper.py", "eagle_lbr.py", "timings.py", "library_bundle.py",
        "ab_client.py", "install_addin.py", "handlers/__init__.py", "handlers/close_fusion.py", "handlers/dialog_classify.py",
        "handlers/dismiss_recovery.py", "handlers/fusion_ui.py", "handlers/open_design.py", "handlers/taskbar_overlay.py", "start.bat"}
rf = pathlib.Path("tools/release-files.txt"); lines = [l for l in rf.read_text().splitlines() if l.strip()]
kept = [l for l in lines if l not in drop]
rf.write_text("\n".join(kept) + "\n"); print("release-files.txt: dropped", len(lines) - len(kept), "python files, kept", len(kept))
rs = pathlib.Path("tools/release.sh"); s = rs.read_text()
s2 = re.sub(r'^python3 -c "import ast; ast\.parse\(open\(\'server\.py\'\)\.read\(\)\)".*\n', '', s, flags=re.M)
s2 = re.sub(r'^python3 tools/shadow_check\.py server\.py.*\n', '', s2, flags=re.M)
s2 = re.sub(r'^python3 -c "import server".*\n', '', s2, flags=re.M)
s2 = re.sub(r'^python3 -c "import fusion_detect".*\n', '', s2, flags=re.M)
s2 = s2.replace('*server.py*fusion*', '*adom-fusion-bridge.exe*')
if s2 != s: rs.write_text(s2); print("release.sh: python import checks removed, process probe names the exe")
PY
git status --short | head -20
