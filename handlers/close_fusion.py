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
import os
import signal
import subprocess
import sys
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
    if sys.platform == "darwin":
        # macOS has no Win32 window enumeration, and the bridge takes no pyobjc/Quartz
        # dependency just to list windows. Answering [] is the CONTRACT the darwin stop
        # path needs (it drives the app through AppleEvents/SIGTERM, never through
        # windows). Without this branch the function died on the next line with
        # "AttributeError: module 'ctypes' has no attribute 'WINFUNCTYPE'", which is why
        # fusion_stop and fusion_kill could not close Fusion on macOS at all.
        return []
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


# ---------------------------------------------------------------------------
# macOS (darwin): no Win32, so stop/kill drive the app itself.
#
# These branches exist for one scenario: the ADD-IN WEDGE. Fusion's main thread is
# parked inside a native call (a cloud fetch that never returns), so the add-in's HTTP
# server still answers /health but can never run another command. The pre-macOS-branch
# code made that unsurvivable: fusion_stop first knocked on the add-in through the
# retry ladder in _close_all_documents_via_addin (~44-94 s) and then crashed in
# _find_windows_by_title, so neither verb could close Fusion and the user had to run
# `kill -9` by hand.
# ---------------------------------------------------------------------------

MAC_APP_NAME = "Autodesk Fusion"
_MAC_BUNDLE_MARK = "/Autodesk Fusion.app/"
# The main executable inside the bundle. `ps` shows it as
# <bundle>/Contents/MacOS/Autodesk Fusion; every helper has a DIFFERENT basename
# (QtWebEngineProcess, cer_dialog, ADPClientService, "Autodesk Identity Manager"),
# so matching on the basename is what separates the app from its children.
_MAC_APP_BINARY = "Autodesk Fusion"
_HEALTH_UNSET = object()


def _addin_health(timeout: float = 2.0):
    """GET the add-in's /health with a SHORT timeout. Returns the parsed dict, or None
    when nothing answers (Fusion closed, add-in not loaded, port taken)."""
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{ADDIN_PORT}/health",
                                    timeout=timeout) as resp:
            doc = json.loads(resp.read())
        return doc if isinstance(doc, dict) else None
    except Exception:
        return None


def _addin_wedged(health=_HEALTH_UNSET) -> bool:
    """True when the add-in answers /health but cannot serve another command.

    "blocked"/"busy" means Fusion's main thread is inside something (the classic
    wedge), and pending_commands > 0 means a command is still queued for it. Either
    way the clean document close would only spend its whole retry ladder knocking on a
    main thread that cannot answer - so skip it and go straight to quitting.
    """
    h = _addin_health() if health is _HEALTH_UNSET else health
    if not isinstance(h, dict):
        return False
    if str(h.get("main_thread") or "") in ("blocked", "busy"):
        return True
    try:
        return int(h.get("pending_commands") or 0) > 0
    except (TypeError, ValueError):
        return False


def _darwin_fusion_pids() -> list:
    """PIDs of the Autodesk Fusion app process itself (macOS).

    Four traps, all measured on a real box:

    1. A shell wrapper whose argv contains "Autodesk Fusion" must never be hit -
       that includes this bridge's own caller. So discovery uses `pgrep -x`, which
       matches the executable NAME exactly, and every candidate is confirmed.
    2. `ps` truncates the command column (100 chars by default); a webdeploy path
       is longer, so matching failed silently. `ps -ww` lifts the limit.
    3. "Application Support" contains a space, so splitting the command on
       whitespace yields "/Users/paliddo/Library/Application". Never split it.
    4. The bundle runs 13 processes (QtWebEngineProcess, cer_dialog,
       ADPClientService, "Autodesk Identity Manager", node helpers) and exactly 1
       is Fusion. Signalling all of them orphans the helpers and makes the
       "is it gone?" poll never succeed.

    The identity test is therefore: the executable (argv[0], recovered without
    splitting) is a path ending in "/Contents/MacOS/Autodesk Fusion" that sits
    inside a Fusion app bundle.
    """
    suffix = "/Contents/MacOS/" + _MAC_APP_BINARY

    def _cmd_for(pid: int) -> str:
        try:
            return subprocess.run(["ps", "-ww", "-p", str(pid), "-o", "command="],
                                  stdin=subprocess.DEVNULL, capture_output=True,
                                  text=True, timeout=10).stdout.strip()
        except Exception:
            return ""

    def _is_fusion(pid: int) -> bool:
        cmd = _cmd_for(pid)
        if not cmd or _MAC_BUNDLE_MARK not in cmd:
            return False
        # argv[0] runs from the start of cmd to the end of the bundle suffix. No
        # whitespace splitting: the path legitimately contains spaces.
        end = cmd.find(suffix)
        if end < 0:
            return False
        argv0 = cmd[: end + len(suffix)]
        # A trailing character means argv[0] was only a PREFIX of this binary
        # (e.g. ".../Autodesk Fusion Service Utility"): reject only if the next
        # char continues the name rather than starting a new argv entry.
        tail = cmd[end + len(suffix):]
        if tail and not tail.startswith(" "):
            return False
        return argv0.endswith(suffix) and _MAC_BUNDLE_MARK in argv0

    try:
        out = subprocess.run(["pgrep", "-x", _MAC_APP_BINARY],
                             stdin=subprocess.DEVNULL, capture_output=True,
                             text=True, timeout=10).stdout
    except Exception:
        out = ""

    pids = []
    for token in (out or "").split():
        try:
            pid = int(token)
        except ValueError:
            continue
        if pid == os.getpid() or pid in pids:
            continue
        if _is_fusion(pid):
            pids.append(pid)
    return pids


def _darwin_has_fusion() -> bool:
    """Is Fusion still running? fusion_detect's pgrep-backed check is the single source
    of truth (it also handles the bundle's real executable name); the bundle scan is
    only the fallback when that import fails."""
    try:
        from fusion_detect import _has_fusion_process as _detect_has_fusion
    except Exception:
        return bool(_darwin_fusion_pids())
    return bool(_detect_has_fusion())


def _darwin_wait_gone(timeout: float = 10.0) -> bool:
    """Poll until Fusion is gone (bounded): after each quit step we need a CONFIRMED
    exit, not a hopeful sleep, before deciding whether to escalate."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not _darwin_has_fusion():
            return True
        time.sleep(0.3)
    return not _darwin_has_fusion()


def _darwin_signal_fusion(sig: int) -> int:
    """Send `sig` to every Fusion process. Returns how many were signalled."""
    sent = 0
    for pid in _darwin_fusion_pids():
        try:
            os.kill(pid, sig)
            sent += 1
        except Exception:
            continue
    return sent


def _darwin_osascript_quit(timeout: float = 15.0) -> str:
    """Ask Fusion to quit through its AppleEvent handler. Returns a human detail string
    and never raises. A wedged main thread answers no AppleEvent, so osascript fails
    with -1712 (AppleEvent timed out) at the timeout ceiling - the caller then falls
    through to signals instead of treating that as the end of the stop."""
    try:
        r = subprocess.run(["osascript", "-e", f'tell application "{MAC_APP_NAME}" to quit'],
                           stdin=subprocess.DEVNULL, capture_output=True, text=True,
                           timeout=timeout)
        if r.returncode == 0:
            return "AppleEvent accepted"
        return f"osascript exit {r.returncode}: {(r.stderr or r.stdout or '').strip()[:200]}"
    except subprocess.TimeoutExpired:
        return f"osascript timed out after {timeout:.0f}s (-1712: main thread not answering)"
    except Exception as e:
        return f"osascript failed: {e}"


def _stop_fusion_darwin(args: dict) -> dict:
    """Graceful STOP on macOS: AppleEvent quit, then SIGTERM, then SIGKILL only when the
    caller passed force=true. Every step is bounded and a failure falls through to the
    next one - a wedged Fusion answers nothing, so aborting on the first timeout would
    leave the wedge exactly where it was."""
    force = bool(args.get("force", False))
    skip_clean = bool(args.get("skipCleanClose", False))

    health = _addin_health()
    wedged = _addin_wedged(health)
    close_report = {"closed": [], "errors": [], "count": 0}
    if not skip_clean and not wedged:
        close_report = _close_all_documents_via_addin()
        if close_report["closed"]:
            time.sleep(0.5)

    def _ok(step: str, note: str = "") -> dict:
        parts = [f"Fusion 360 stopped via {step}"]
        if note:
            parts.append(note)
        if close_report["closed"]:
            parts.append(f"{len(close_report['closed'])} document(s) closed cleanly first")
        return {
            "success": True,
            "output": "; ".join(parts) + ".",
            "data": {"step": step, "documentsClosed": close_report["closed"],
                     "addinWedged": wedged},
        }

    if not _darwin_has_fusion():
        return {"success": True, "output": "Fusion 360 is not running."}

    steps = []

    # (a) AppleEvent quit - the document-aware path: Fusion closes/saves its own state.
    note = _darwin_osascript_quit()
    steps.append({"step": "osascript quit", "detail": note})
    if _darwin_wait_gone(10.0):
        return _ok("osascript quit", note)

    # (b) SIGTERM - polite to the whole app bundle; Fusion still gets to run its exit
    #     handlers, which is what keeps the 'Recovered Documents' dialog away.
    signalled = _darwin_signal_fusion(signal.SIGTERM)
    steps.append({"step": "SIGTERM", "signalled": signalled})
    if _darwin_wait_gone(10.0):
        return _ok("SIGTERM")

    # (c) SIGKILL - ONLY on an explicit force=true. Graceful stop never destroys unsaved
    #     work on its own authority; the caller decides that.
    if force:
        signalled = _darwin_signal_fusion(signal.SIGKILL)
        steps.append({"step": "SIGKILL", "signalled": signalled})
        if _darwin_wait_gone(10.0):
            return _ok("SIGKILL (force=true)")

    return {
        "success": False,
        "error": ("Fusion did not exit after osascript quit and SIGTERM." if not force else
                  "Fusion did not exit after osascript quit, SIGTERM and SIGKILL."),
        "output": f"Steps tried: {steps}",
        "data": {"steps": steps, "addinWedged": wedged,
                 "survivingPids": _darwin_fusion_pids()},
        "_hint": ("A modal dialog or a wedged main thread is holding it. Re-call fusion_kill "
                  "with {\"force\": true} (SIGTERM then SIGKILL) - do not hand the user a "
                  "manual kill." if not force else
                  "All three steps ran and something still holds: check fusion_readiness for "
                  "fusionCrashed (a crash-report host can survive) and re-call after clearing it."),
    }


def _kill_fusion_darwin(args: dict) -> dict:
    """Force-KILL on macOS: SIGTERM every Fusion process, wait, then SIGKILL the
    survivors. Documents are still closed cleanly first when the add-in is healthy -
    killing dirty ones is exactly what produces the 'Recovered Documents' dialog."""
    skip_clean = bool(args.get("skipCleanClose", False))

    health = _addin_health()
    wedged = _addin_wedged(health)
    close_report = {"closed": [], "errors": [], "count": 0}
    if not skip_clean and not wedged:
        close_report = _close_all_documents_via_addin()
        if close_report["closed"]:
            time.sleep(0.5)

    if not _darwin_has_fusion():
        return {"success": True, "output": "Fusion 360 is not running."}

    steps = []
    step_used = None
    killed = _darwin_signal_fusion(signal.SIGTERM)
    steps.append({"step": "SIGTERM", "signalled": killed})
    if _darwin_wait_gone(10.0):
        step_used = "SIGTERM"
    else:
        killed = _darwin_signal_fusion(signal.SIGKILL)
        steps.append({"step": "SIGKILL", "signalled": killed})
        if _darwin_wait_gone(10.0):
            step_used = "SIGKILL"

    if step_used is None:
        return {
            "success": False,
            "error": "Some Fusion processes survived SIGTERM and SIGKILL.",
            "output": f"Steps tried: {steps}",
            "data": {"steps": steps, "addinWedged": wedged,
                     "survivingPids": _darwin_fusion_pids()},
            "_hint": ("Do NOT hand this to the user as a Terminal chore: check whether the "
                      "survivor is a crash-report host (fusion_readiness reports fusionCrashed) "
                      "or is pinned by a modal, then re-call fusion_kill."),
        }

    parts = [f"Fusion 360 force-killed on macOS via {step_used}"]
    if close_report["closed"]:
        parts.append(f"{len(close_report['closed'])} document(s) closed cleanly first")
    return {
        "success": True,
        "output": "; ".join(parts) + ".",
        "data": {"step": step_used, "documentsClosed": close_report["closed"],
                 "addinWedged": wedged, "steps": steps},
    }


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
          force (bool, default false) - macOS only: permits SIGKILL as the last step.
    """
    if sys.platform == "darwin":
        return _stop_fusion_darwin(args)

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
    macOS: SIGTERM, then SIGKILL for survivors (_kill_fusion_darwin).
    """
    if sys.platform == "darwin":
        return _kill_fusion_darwin(args)

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
