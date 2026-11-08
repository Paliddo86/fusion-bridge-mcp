#!/usr/bin/env python3
"""Authenticode-sign adom-fusion-bridge.exe as Adom Industries, Inc. through Azure Trusted Signing, the path every
Adom release uses (adom/altium-bridge scripts/release.py sign_pe_files; adapted from adom/kicad-bridge
tools/sign_azure.py).

The signing runs on a Windows host reached through Adom Bridge, under that host's own Azure login. Every
identifier (host, staging folder, endpoint, account, profile, tenant, timestamp URL, expected subject) and the
sign-verify.ps1 script live in a PRIVATE folder outside this public repo: $ADOM_SIGNING_DIR, else
~/.adom/signing/fusion-bridge, else ~/.adom/signing/altium-bridge (the same account signs every Adom bridge).
Nothing from it is ever written to this repo, a reply, a log line or an issue.

  python3 tools/sign_pe.py <version> <exe>     # sign one exe in place; exits non-zero unless verified
                                               # (tools/release.sh runs it; the last stdout line is the report)

On the signing machine itself (this WSL runs on the config's host) it signs locally through powershell.exe,
staging under the host's %LOCALAPPDATA%; elsewhere it goes through Adom Bridge. The flow, each step a gate:
  1. stage the exe and sign-verify.ps1 on the host (send_files; the folder name has no dots);
  2. one run_script signs and verifies there: Get-AuthenticodeSignature Valid, the expected subject,
     a Microsoft timestamp, signtool verify /pa;
  3. pull the signed exe back, check its sha256 matches the host's and differs from the unsigned build;
  4. read its Authenticode signer here (openssl): CN = "Adom Industries, Inc.".
"""
import base64
import hashlib
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

SIGNER_ORG = "Adom Industries, Inc."
THREAD = "fusion-bridge release signing"


def signing_dir():
    env = os.environ.get("ADOM_SIGNING_DIR")
    if env:
        return Path(env)
    for name in ("fusion-bridge", "altium-bridge"):
        d = Path.home() / ".adom" / "signing" / name
        if (d / "config.json").is_file():
            return d
    return Path.home() / ".adom" / "signing" / "fusion-bridge"


def authenticode_subjects(path):
    """Certificate subjects in a PE's Authenticode signature, read without Windows: the security data
    directory points at a WIN_CERTIFICATE holding PKCS#7 DER. [] means unsigned."""
    b = Path(path).read_bytes()
    pe = struct.unpack_from("<I", b, 0x3C)[0]
    if b[pe:pe + 4] != b"PE\0\0":
        raise SystemExit(f"{path}: not a PE file")
    opt = pe + 24
    magic = struct.unpack_from("<H", b, opt)[0]
    dirs = opt + (112 if magic == 0x20B else 96)
    off, size = struct.unpack_from("<II", b, dirs + 4 * 8)
    if not off or not size:
        return []
    length, _rev, ctype = struct.unpack_from("<IHH", b, off)
    if ctype != 2:
        return []
    with tempfile.NamedTemporaryFile(suffix=".p7b", delete=False) as f:
        f.write(b[off + 8:off + length])
    r = subprocess.run(["openssl", "pkcs7", "-inform", "DER", "-in", f.name, "-print_certs", "-noout"], capture_output=True, text=True)
    os.unlink(f.name)
    return [l[len("subject="):].strip() for l in r.stdout.splitlines() if l.startswith("subject=")]


def assert_signed(path):
    subs = authenticode_subjects(path)
    if not any(f'CN = "{SIGNER_ORG}"' in s for s in subs):
        raise SystemExit(f"REFUSED: {path} is not Authenticode-signed by {SIGNER_ORG} (found {subs or 'no signature'})")
    return subs


def ab_json(*args, timeout=600):
    out = subprocess.run(["adom-bridge", *args], capture_output=True, text=True, timeout=timeout).stdout
    d = json.loads(out[out.find("{"):]) if "{" in out else {}
    o = d.get("output")
    if isinstance(o, str) and o.strip().startswith("{"):
        try:
            d = json.loads(o)
        except ValueError:
            pass
    return d


POWERSHELL = "/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"


def redact(text, cfg):
    """A message with every private config value blanked: the signing script's hints name the tenant."""
    text = str(text)
    for k in ("endpoint", "account", "profile", "tenant", "timestampUrl", "stageDir"):
        v = str(cfg.get(k) or "")
        if len(v) > 3:
            text = text.replace(v, f"<{k}>")
    return text


def local_windows_host():
    """This machine's Windows name when we run in WSL on it (the signing machine itself), else None."""
    if not os.path.exists(POWERSHELL):
        return None
    r = subprocess.run(["/mnt/c/Windows/System32/cmd.exe", "/c", "hostname"], capture_output=True, text=True, cwd="/mnt/c")
    return r.stdout.strip() or None


def checks_of(v, cfg):
    return {
        "Status Valid": v.get("status") == "Valid",
        "signer subject": cfg["expectSubject"] in (v.get("subject") or ""),
        "timestamped": "Microsoft" in (v.get("timestamper") or ""),
        "signtool verify /pa": "Successfully verified" in (v.get("signtoolVerify") or ""),
    }


def sign_locally(exe, ver, cfg, ps1):
    """On the signing machine itself (WSL on it): stage under its own %LOCALAPPDATA%, run sign-verify.ps1
    with powershell.exe, no Adom Bridge round trip."""
    win = lambda p: subprocess.run(["wslpath", "-w", str(p)], capture_output=True, text=True).stdout.strip()
    lad = subprocess.run(["/mnt/c/Windows/System32/cmd.exe", "/c", "echo %LOCALAPPDATA%"], capture_output=True, text=True, cwd="/mnt/c").stdout.strip()
    stage_win = cfg["stageDir"].replace("%LOCALAPPDATA%", lad) + "\\fusion-build-" + ver.replace(".", "_")
    stage = Path(subprocess.run(["wslpath", "-u", stage_win], capture_output=True, text=True).stdout.strip())
    shutil.rmtree(stage, ignore_errors=True)
    stage.mkdir(parents=True)
    unsigned_sha = hashlib.sha256(exe.read_bytes()).hexdigest()
    shutil.copy2(exe, stage / exe.name)
    shutil.copy2(ps1, stage / "sign-verify.ps1")
    print(f"== sign {exe.name} {ver} here (the signing machine) ==", file=sys.stderr)
    r = subprocess.run([POWERSHELL, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", win(stage / "sign-verify.ps1"),
                        "-Exe", win(stage / exe.name), "-Endpoint", cfg["endpoint"], "-Account", cfg["account"],
                        "-Profile", cfg["profile"], "-Tenant", cfg["tenant"], "-Timestamp", cfg["timestampUrl"]],
                       capture_output=True, text=True, cwd="/mnt/c", timeout=540)
    out = r.stdout
    try:
        v = json.loads(out[out.find("{"):out.rfind("}") + 1])
    except ValueError:
        raise SystemExit(f"REFUSED: the signing script gave no verdict (exit {r.returncode}): {redact((out + r.stderr)[-600:], cfg)}")
    if v.get("error"):
        raise SystemExit(f"REFUSED: cannot sign ({v['error']}): {redact(v.get('detail', ''), cfg)}\n  fix: {redact(v.get('fix', ''), cfg)}")
    checks = checks_of(v, cfg)
    if not all(checks.values()):
        raise SystemExit(f"REFUSED: signature checks failed: {checks}")
    signed = stage / exe.name
    signed_sha = hashlib.sha256(signed.read_bytes()).hexdigest()
    if signed_sha == unsigned_sha:
        raise SystemExit("REFUSED: the exe is byte-identical after signing")
    subs = assert_signed(signed)
    shutil.copy2(signed, exe)
    shutil.rmtree(stage, ignore_errors=True)
    print("   " + ", ".join(f"{k} {'ok' if ok else 'FAIL'}" for k, ok in checks.items()) + f"; verified again here, sha256 {signed_sha[:12]}", file=sys.stderr)
    return {"file": exe.name, "version": ver, "status": v["status"], "subject": SIGNER_ORG, "timestamper": v["timestamper"],
            "signtoolVerify": "Successfully verified", "sha256Unsigned": unsigned_sha, "sha256": signed_sha,
            "localSubjects": [x for x in subs if SIGNER_ORG in x], "where": "signing machine (local)"}


def sign(exe, ver):
    """Sign `exe` in place: here when this is the signing machine (WSL on it), else on the signing host
    through Adom Bridge. Returns the signing report (no private values in it)."""
    exe = Path(exe)
    sd = signing_dir()
    cfg_p, ps1 = sd / "config.json", sd / "sign-verify.ps1"
    if not cfg_p.is_file() or not ps1.is_file():
        raise SystemExit(f"REFUSED: private signing config missing at {sd} (config.json + sign-verify.ps1). It is "
                         f"deliberately not in this public repo; ask Colby to copy it over privately.")
    cfg = json.loads(cfg_p.read_text())
    host = cfg["host"]
    here = local_windows_host()
    if here and here.lower() == host.lower():
        return sign_locally(exe, ver, cfg, ps1)
    lad = cfg.get("localAppData") or (ab_json("--target", host, "--ai-thread", THREAD, "run_script", json.dumps({
        "interpreter": "powershell", "scriptB64": base64.b64encode(b"$env:LOCALAPPDATA").decode(), "timeout": 30,
        "reason": "read the signing host's LOCALAPPDATA for the staging folder"})).get("stdout") or "").strip()
    if not lad:
        raise SystemExit("REFUSED: could not read the signing host's LOCALAPPDATA for the staging folder")
    stage = cfg["stageDir"].replace("%LOCALAPPDATA%", lad) \
        + "\\fusion-build-" + ver.replace(".", "_")  # a dotted folder name reads as a file path to send_files
    unsigned_sha = hashlib.sha256(exe.read_bytes()).hexdigest()
    work = Path(tempfile.mkdtemp(prefix="fusion-sign-"))
    name = exe.name
    shutil.copy2(exe, work / name)
    shutil.copy2(ps1, work / "sign-verify.ps1")
    print(f"== sign {name} {ver} on the signing host ==", file=sys.stderr)
    sent = ab_json("--target", host, "--ai-thread", THREAD, "send_files", json.dumps({
        "filePaths": [str(work / name), str(work / "sign-verify.ps1")], "dest": stage, "destIsFolder": True,
        "reason": f"stage fusion-bridge {ver} for Authenticode signing"}))
    if int(sent.get("filesReceived") or 0) != 2:
        shutil.rmtree(work, ignore_errors=True)
        raise SystemExit(f"REFUSED: the signing host did not receive both files: {sent.get('error') or sent.get('_hint') or 'no reply'}")
    call = (f"(& '{stage}\\sign-verify.ps1' -Exe '{stage}\\{name}' -Endpoint '{cfg['endpoint']}' -Account '{cfg['account']}' "
            f"-Profile '{cfg['profile']}' -Tenant '{cfg['tenant']}' -Timestamp '{cfg['timestampUrl']}' | Out-String)")
    cmd = f"$r = @({call}); '[' + ($r -join ',') + ']'"
    r = ab_json("--target", host, "--ai-thread", THREAD, "run_script", json.dumps({
        "interpreter": "powershell", "scriptB64": base64.b64encode(cmd.encode()).decode(), "timeout": 540,
        "reason": f"Authenticode-sign fusion-bridge {ver} as {SIGNER_ORG}, then Get-AuthenticodeSignature + signtool verify"}), timeout=900)
    so = r.get("stdout") or ""
    try:
        v = json.loads(so[so.find("["):])[0]
    except (ValueError, IndexError):
        shutil.rmtree(work, ignore_errors=True)
        raise SystemExit(f"REFUSED: the signing script gave no verdict (stdout {len(so)} chars, stderr {len(r.get('stderr') or '')} chars, error {r.get('errorCode') or r.get('error') or '-'})")
    if v.get("error"):
        shutil.rmtree(work, ignore_errors=True)
        raise SystemExit(f"REFUSED: the signing host cannot sign ({v['error']}): {redact(v.get('detail', ''), cfg)}\n  fix: {redact(v.get('fix', ''), cfg)}")
    checks = checks_of(v, cfg)
    if not all(checks.values()):
        shutil.rmtree(work, ignore_errors=True)
        raise SystemExit(f"REFUSED: signature checks failed on the host: {checks}")
    pulled = ab_json("--target", host, "--ai-thread", THREAD, "pull_file", json.dumps({
        "filePaths": [f"{stage}\\{name}"], "saveTo": str(work / "back"), "reason": f"pull the signed fusion-bridge {ver} back for packaging"}))
    files = pulled.get("files") or []
    if not files:
        shutil.rmtree(work, ignore_errors=True)
        raise SystemExit(f"REFUSED: could not pull the signed exe back: {pulled.get('error') or 'no files'}")
    back = Path(files[0]["path"])
    signed_sha = hashlib.sha256(back.read_bytes()).hexdigest()
    if v.get("sha256") and signed_sha != v["sha256"].lower():
        raise SystemExit("REFUSED: the signed exe changed in transit (host and local sha256 differ)")
    if signed_sha == unsigned_sha:
        raise SystemExit("REFUSED: the pulled-back exe is byte-identical to the unsigned build")
    subs = assert_signed(back)
    shutil.copy2(back, exe)
    shutil.rmtree(work, ignore_errors=True)
    report = {"file": name, "version": ver, "status": v["status"], "subject": SIGNER_ORG, "timestamper": v["timestamper"],
              "signtoolVerify": "Successfully verified", "sha256Unsigned": unsigned_sha, "sha256": signed_sha,
              "localSubjects": [s for s in subs if SIGNER_ORG in s]}
    print("   " + ", ".join(f"{k} {'ok' if ok else 'FAIL'}" for k, ok in checks.items()) + f"; verified again here, sha256 {signed_sha[:12]}", file=sys.stderr)
    return report


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    print(json.dumps(sign(sys.argv[2], sys.argv[1])))
