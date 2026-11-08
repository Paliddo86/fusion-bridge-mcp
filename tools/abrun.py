"""Run a shell command on the box via ab, PowerShell-etiquette compliant.

John's rules (2026-08-15): shell commands driven through the bridge must (1) always
exit with a real exit code, and (2) NEVER blip a console window into the user's
foreground. So:
- PowerShell goes through run_script (base64 script -> temp .ps1, no shell-string
  parsing) with -NoProfile -NonInteractive semantics and an explicit `exit $LASTEXITCODE`
  appended, instead of shell_execute'ing a `powershell -Command` one-liner.
- cmd one-liners keep shell_execute but should be rare; prefer the bridge's own file
  verbs (read_file/write_file/pull_file) which spawn nothing at all.

Usage: python3 tools/abrun.py <command> [reason] [--ps]
  --ps  treat <command> as a PowerShell SCRIPT body (run_script path)
"""
import base64
import json
import subprocess
import sys

args = [a for a in sys.argv[1:] if a != "--ps"]
use_ps = "--ps" in sys.argv[1:]
cmd = args[0]
reason = args[1] if len(args) > 1 else "fusion-bridge maintenance"

if use_ps:
    script = cmd.rstrip() + "\nexit $LASTEXITCODE\n"
    payload = json.dumps({
        "interpreter": "powershell",
        "scriptBase64": base64.b64encode(script.encode()).decode(),
        "reason": reason,
    })
    verb = "run_script"
else:
    payload = json.dumps({"command": cmd, "reason": reason})
    verb = "shell_execute"

p = subprocess.run(["adom-bridge-cli", "--target", "AdomLapper",
                    "--ai-thread", "fusion-bridge maintenance", verb, payload],
                   capture_output=True, text=True, timeout=180)
try:
    d = json.loads(p.stdout)
    data = d.get("data") or d
    rc = data.get("exitCode")
    print((data.get("stdout") or data.get("stderr") or "(empty)").strip()[:6000])
    if rc not in (0, None):
        print(f"[exitCode {rc}]", file=sys.stderr)
        sys.exit(int(rc) if isinstance(rc, int) and 0 < rc < 256 else 1)
except Exception:
    print(p.stdout[:400] or p.stderr[:400])
    sys.exit(1)
