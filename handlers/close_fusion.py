"""Handler for close_fusion command.

Closes all open documents cleanly via the add-in (avoiding recovered documents
dialog on next launch), then sends WM_CLOSE to all Fusion 360 windows, with
optional force-kill.

IMPORTANT: Always close documents via the add-in BEFORE killing Fusion.
taskkill //f causes unsaved document recovery on next launch, which shows a
blocking modal dialog that prevents automation.
"""

import ctypes
import ctypes.wintypes
import json
import subprocess
import time
import urllib.request
import urllib.error

WM_CLOSE = 0x0010

# Guard windll so the module imports on non-Windows hosts (the bridge must boot
# + serve /status anywhere); these Win32 paths are only reached on Windows.
user32 = ctypes.windll.user32 if hasattr(ctypes, "windll") else None

_callbacks = []

# Fusion process names to kill when force-stopping
FUSION_PROCESS_NAMES = [
    "Fusion360.exe", "FusionLauncher.exe", "Fusion.exe",
    "FusionService.exe", "FusionCEF.exe", "Fusion360Helper.exe",
]


def _find_windows_by_title(substring: str) -> list:
    """Find all visible windows whose title contains the given substring."""
    results = []
    WNDENUMPROC = ctypes.WINFUNCTYPE(
        ctypes.c_bool, ctypes.wintypes.HWND, ctypes.wintypes.LPARAM
    )

    def callback(hwnd, _lparam):
        if user32.IsWindowVisible(hwnd):
            length = user32.GetWindowTextLengthW(hwnd)
            if length > 0:
                buf = ctypes.create_unicode_buffer(length + 1)
                user32.GetWindowTextW(hwnd, buf, length + 1)
                if substring in buf.value:
                    results.append((hwnd, buf.value))
        return True

    cb = WNDENUMPROC(callback)
    _callbacks.append(cb)
    user32.EnumWindows(cb, 0)
    _callbacks.remove(cb)
    return results


def _close_and_wait(hwnd: int, title: str, timeout: float = 15.0) -> bool:
    """Send WM_CLOSE and wait for the window to disappear.

    Fusion 360 may take longer to close than typical apps, so default timeout is 15s.
    """
    user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        time.sleep(0.3)
        if not user32.IsWindow(hwnd) or not user32.IsWindowVisible(hwnd):
            return True
    return False


def _ab_process_rows(name_substr):
    """Fusion-family process rows via ab's own process_list verb, or None when ab is
    unreachable. VERB-FIRST DOCTRINE (John, 2026-09-01: "aren't those ab verbs you could
    call instead?"): ab already owns host process ops - one implementation, no child
    process to spawn, and the call lands in ab's activity log with this bridge's caller
    identity, where a raw taskkill is invisible. The tasklist/taskkill spawns below survive
    only as the degraded path for when ab_client is down (unattended VM, ab mid-restart).

    NOTE the deliberate exception: fusion_detect's POLLING uses a local Toolhelp32 snapshot
    instead of this verb, and should keep doing so - a poll every few seconds wants
    microseconds in-process, not an HTTP round trip. Verb for OPERATIONS, snapshot for
    POLLS, hidden spawn only as a fallback."""
    try:
        import ab_client
        if not ab_client.available():
            return None
        r = ab_client.call("process_list", {"name": name_substr}, timeout=10)
        if not isinstance(r, dict):
            return None
        rows = r.get("processes")
        if rows is None and isinstance(r.get("output"), str):
            import json as _j
            try:
                rows = (_j.loads(r["output"]) or {}).get("processes")
            except Exception:
                rows = None
        return rows if isinstance(rows, list) else None
    except Exception:
        return None


def _ab_process_kill(pid, why):
    """Kill one PID through ab's process_kill verb. True on confirmed kill, False when the
    verb was unavailable or refused (caller falls back to a hidden taskkill)."""
    try:
        import ab_client
        if not ab_client.available():
            return False
        r = ab_client.call("process_kill", {"pid": int(pid), "tree": True, "reason": why},
                           timeout=15)
        return bool(isinstance(r, dict) and (r.get("success") or r.get("ok")))
    except Exception:
        return False


def _fusion_pids() -> list:
    """PIDs of every surviving Fusion-family process (tasklist CSV, empty on error)."""
    rows = _ab_process_rows("Fusion")
    if rows is not None:
        return [int(r.get("pid")) for r in rows
                if r.get("pid") and str(r.get("name") or "") in FUSION_PROCESS_NAMES]
    pids = []
    for name in FUSION_PROCESS_NAMES:
        try:
            _nw = subprocess.CREATE_NO_WINDOW if hasattr(subprocess, "CREATE_NO_WINDOW") else 0
            out = subprocess.check_output(
                ["tasklist", "/FI", "IMAGENAME eq %s" % name, "/FO", "CSV", "/NH"],
                stderr=subprocess.DEVNULL, text=True, timeout=5, creationflags=_nw)
        except Exception:
            continue
        for line in out.splitlines():
            parts = [p.strip('"') for p in line.split('","')]
            if len(parts) > 1 and parts[0].lower().endswith(".exe"):
                try:
                    pids.append(int(parts[1]))
                except Exception:
                    pass
    return pids


def _force_kill_fusion() -> int:
    """Force kill all Fusion processes. Returns number of processes killed.

    ESCALATES (doctrine, 2026-08-19): `taskkill /F /IM` is not the end of the road, and
    when it left a Fusion360.exe standing the bridge used to answer "ask the user to kill
    it via Task Manager" - handing the human a chore the code can do itself. So a survivor
    is now killed BY PID with /T (the whole process tree), which clears the case /IM
    misses. Only if a PID survives THAT do we report, and then with the pids and what was
    tried - never with an instruction to open Task Manager.
    """
    killed = 0
    # Verb-first: enumerate + kill through ab, so the kills are attributed in ab's
    # activity log. Fall through to taskkill for anything the verb path did not confirm.
    _verb_killed = set()
    for r in (_ab_process_rows("Fusion") or []):
        pid = r.get("pid")
        if pid and str(r.get("name") or "") in FUSION_PROCESS_NAMES:
            if _ab_process_kill(pid, "close Fusion: force-kill surviving %s" % r.get("name")):
                killed += 1
                _verb_killed.add(int(pid))
    for name in FUSION_PROCESS_NAMES:
        try:
            result = subprocess.run(
                ["taskkill", "/F", "/IM", name],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            )
            if result.returncode == 0:
                killed += 1
        except Exception:
            pass
    # Escalate on survivors: per-PID, whole tree.
    try:
        time.sleep(1.0)
        for pid in _live_fusion_pids():
            try:
                r = subprocess.run(["taskkill", "/F", "/T", "/PID", str(pid)],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                   creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
                if r.returncode == 0:
                    killed += 1
            except Exception:
                pass
    except Exception:
        pass
    return killed


def _live_fusion_pids() -> list:
    """Surviving pids that are GENUINELY alive - a zombie is not something to kill."""
    try:
        from fusion_detect import _pid_alive
    except Exception:
        return _fusion_pids()
    return [p for p in _fusion_pids() if _pid_alive(p)]


def _has_fusion_process() -> bool:
    """Is a GENUINELY LIVE Fusion process still running?

    Deliberately not a raw tasklist check: a zombie Fusion360.exe (exited, parent
    gone, never reaped) still appears there, and counting it made a successful kill
    report "Some Fusion processes survived force-kill" forever - a failure nobody
    could act on, because there was nothing left to kill (ConfRoomROG, 2026-08-19).
    """
    try:
        return bool(_live_fusion_pids())
    except Exception:
        return False


ADDIN_PORT = 8774


def _close_all_documents_via_addin() -> dict:
    """Ask the add-in to close all open documents without saving.

    This prevents the 'Recovered Documents' dialog on next launch.
    Returns {"closed": [...], "errors": [...], "count": N}.
    """
    result = {"closed": [], "errors": [], "count": 0}

    # First, get list of open documents
    try:
        body = json.dumps({"command": "document_info", "args": {}}).encode("utf-8")
        req = urllib.request.Request(
            f"http://127.0.0.1:{ADDIN_PORT}/command",
            data=body,
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(req, timeout=5) as resp:
            resp_data = json.loads(resp.read())
    except Exception:
        # RETRY HARD before giving up (2026-08-24: one short retry was not enough - a
        # busy add-in skipped the clean close, the exit raised an Unsaved-changes modal
        # per dirty doc, and a graceful stop wedged 10 minutes on the user's screen).
        # The add-in is nearly always BUSY here, not dead: keep knocking ~40s.
        resp_data = None
        import time as _t
        for _w in (3, 6, 10, 10, 10):
            _t.sleep(_w)
            try:
                with urllib.request.urlopen(req, timeout=10) as resp:
                    resp_data = json.loads(resp.read())
                break
            except Exception:
                continue
        if resp_data is None:
            return result  # Add-in truly unreachable after ~40s, skip clean close

    if not resp_data.get("success"):
        return result

    docs = resp_data.get("data", {}).get("openDocuments", [])
    result["count"] = len(docs)

    if not docs:
        return result

    # Close each document (close active last to avoid switching issues)
    # Reverse order so we close non-active docs first
    for doc in reversed(docs):
        doc_name = doc.get("name", "")
        if not doc_name:
            continue
        try:
            body = json.dumps({
                "command": "close_document",
                "args": {"name": doc_name, "save": False},
            }).encode("utf-8")
            req = urllib.request.Request(
                f"http://127.0.0.1:{ADDIN_PORT}/command",
                data=body,
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(req, timeout=10) as resp:
                close_resp = json.loads(resp.read())
            if close_resp.get("success"):
                result["closed"].append(doc_name)
            else:
                result["errors"].append(f"{doc_name}: {close_resp.get('error', '?')}")
        except Exception as e:
            result["errors"].append(f"{doc_name}: {e}")

    return result


def handle_close_fusion(fusion_info: dict, args: dict) -> dict:
    """Close all Fusion 360 windows, with optional force-kill.

    Always attempts to close documents cleanly via the add-in first,
    to prevent the 'Recovered Documents' dialog on next launch.

    Args:
        fusion_info: Fusion 360 detection info dict (unused but required by interface).
        args: Dict with:
            - force: If true, force-kill all Fusion processes (default: false).
            - skipCleanClose: If true, skip document close via add-in (default: false).
    """
    force = args.get("force", False)
    skip_clean = args.get("skipCleanClose", False)

    # Always try to close documents cleanly first (unless explicitly skipped)
    close_report = {"closed": [], "errors": [], "count": 0}
    if not skip_clean:
        close_report = _close_all_documents_via_addin()
        if close_report["closed"]:
            time.sleep(0.5)  # Brief pause after closing docs

    if force:
        # Force kill all Fusion processes immediately
        killed = _force_kill_fusion()
        time.sleep(1)
        if _has_fusion_process():
            return {
                "success": False,
                "error": "Some Fusion processes survived force-kill.",
                "_hint": ("The bridge tried taskkill /F /IM for every Fusion process and then "
                          "taskkill /F /T per surviving PID, and something still holds on. Do NOT "
                          "hand this to the user as a Task Manager chore: check whether the survivor "
                          "is a crash-report host (fusion_readiness reports fusionCrashed) or is "
                          "pinned by a modal, and re-call fusion_kill after clearing that."),
                "survivingPids": _live_fusion_pids(),
            }
        return {
            "success": True,
            "output": f"Force-killed Fusion 360 ({killed} process group{'s' if killed != 1 else ''}). "
                      f"Closed {len(close_report['closed'])} document(s) cleanly first.",
            "data": {"documentsClosed": close_report["closed"]},
        }

    # Graceful close: send WM_CLOSE to windows
    windows = _find_windows_by_title("Autodesk Fusion")

    if not windows:
        # No windows but maybe processes are hanging
        if _has_fusion_process():
            return {
                "success": True,
                "output": "No Fusion windows found but processes are running. Use force=true to kill them.",
            }
        return {"success": True, "output": "Fusion 360 is not running."}

    closed = []
    failed = []
    for hwnd, title in windows:
        if _close_and_wait(hwnd, title):
            closed.append(title)
        else:
            failed.append(title)

    if failed:
        # Try force-killing since graceful close failed
        _force_kill_fusion()
        time.sleep(1)
        if not _has_fusion_process():
            return {
                "success": True,
                "output": f"Fusion 360 force-killed after graceful close failed.",
            }
        return {
            "success": False,
            "error": f"Failed to close: {', '.join(failed)}",
            "output": f"Closed: {', '.join(closed)}" if closed else "",
            "_hint": "Call fusion_close with {\"force\": true} to force-kill stuck windows, then retry.",
        }

    # Wait for lingering processes
    for _ in range(10):
        if not _has_fusion_process():
            break
        time.sleep(0.5)

    return {
        "success": True,
        "output": f"Fusion 360 closed ({len(closed)} window{'s' if len(closed) != 1 else ''}).",
    }


def handle_fusion_stop(fusion_info: dict, args: dict) -> dict:
    """Gracefully STOP Fusion 360 - the clean opposite of fusion_start.

    Closes all docs cleanly via the add-in (avoids the 'Recovered Documents' dialog),
    then WM_CLOSE every Fusion window and waits. Does NOT force-kill: if a window will
    not close (usually a modal dialog blocking WM_CLOSE), it returns failure and tells
    you to use fusion_kill. No surprise escalation - that's the whole point.

    Args: skipCleanClose (bool, default false) - skip the add-in document close.
    """
    skip_clean = args.get("skipCleanClose", False)
    close_report = {"closed": [], "errors": [], "count": 0}
    if not skip_clean:
        close_report = _close_all_documents_via_addin()
        if close_report["closed"]:
            time.sleep(0.5)

    windows = _find_windows_by_title("Autodesk Fusion")
    if not windows:
        if _has_fusion_process():
            return {
                "success": False,
                "error": "No Fusion windows found, but Fusion processes are still running.",
                "_hint": "A graceful stop cannot reach them - use fusion_kill to force-kill.",
            }
        return {"success": True, "output": "Fusion 360 is not running."}

    closed, failed = [], []
    for hwnd, title in windows:
        (closed if _close_and_wait(hwnd, title) else failed).append(title)

    if failed:
        # Bridge-created throwaway docs (keep the tuple in sync with server._BRIDGE_DOC_BUILTINS):
        # when EVERY failed window is one of ours, force-kill risks nothing of the user's, and
        # callers (tour cleanup, the test suite) can escalate automatically on bridgeDocsOnly.
        _bridge_marks = ("adom-test-", "adomdemosample", "adom-fusion-wrap", "ads8588sipm", "0603waf")
        bridge_only = all(any(m in t.lower() for m in _bridge_marks) for t in failed)
        return {
            "success": False,
            "error": f"Graceful stop failed for: {', '.join(failed)}",
            "output": f"Closed: {', '.join(closed)}" if closed else "",
            "bridgeDocsOnly": bridge_only,
            "_hint": ("The unsaved document(s) blocking the close are BRIDGE-created throwaways "
                      "(sample/test docs) - fusion_kill is SAFE here, nothing of the user's is at "
                      "risk. Kill, then retry." if bridge_only else
                      "A window would not close gracefully (a modal dialog often blocks WM_CLOSE). "
                      "Use fusion_kill to force-kill, then retry."),
        }

    for _ in range(10):
        if not _has_fusion_process():
            break
        time.sleep(0.5)
    return {
        "success": True,
        "output": f"Fusion 360 stopped gracefully ({len(closed)} window{'s' if len(closed) != 1 else ''}); "
                  f"{len(close_report['closed'])} document(s) closed cleanly first.",
        "data": {"documentsClosed": close_report["closed"]},
    }


def handle_fusion_kill(fusion_info: dict, args: dict) -> dict:
    """Force-KILL Fusion 360 (taskkill /F) - the 'desperate' path for when fusion_stop
    can't close it (a stuck modal, a wedged process).

    Still closes docs cleanly via the add-in FIRST when reachable, to avoid the
    'Recovered Documents' dialog on next launch.

    Args: skipCleanClose (bool, default false) - skip the add-in document close.
    """
    skip_clean = args.get("skipCleanClose", False)
    close_report = {"closed": [], "errors": [], "count": 0}
    if not skip_clean:
        close_report = _close_all_documents_via_addin()
        if close_report["closed"]:
            time.sleep(0.5)
    killed = _force_kill_fusion()
    time.sleep(1)
    if _has_fusion_process():
        return {
            "success": False,
            "error": "Some Fusion processes survived force-kill.",
            "_hint": ("The bridge tried taskkill /F /IM for every Fusion process and then "
                      "taskkill /F /T per surviving PID, and something still holds on. Do NOT hand "
                      "this to the user as a Task Manager chore: check whether the survivor is a "
                      "crash-report host (fusion_readiness reports fusionCrashed) or is pinned by a "
                      "modal, and re-call fusion_kill after clearing that."),
            "survivingPids": _live_fusion_pids(),
        }
    return {
        "success": True,
        "output": f"Fusion 360 force-killed ({killed} process group{'s' if killed != 1 else ''}); "
                  f"{len(close_report['closed'])} document(s) closed cleanly first.",
        "data": {"documentsClosed": close_report["closed"]},
    }
