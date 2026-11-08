"""Auto-detect Fusion 360 installation on Windows and macOS."""

import ctypes
import ctypes.wintypes
import json
import os
import re
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

ADDIN_PORT = 8774

# Fusion 360 uses a web-deploy model — the launcher lives under
#   <base>\Autodesk\webdeploy\production\<hash>\FusionLauncher.exe
# where <base> is %LOCALAPPDATA% for a per-user install OR %ProgramFiles%
# (/%ProgramFiles(x86)%/%ProgramW6432%) for a SYSTEM-WIDE `--globalinstall` -
# which is exactly what the fusion-onboarding skill runs. We MUST check both, or
# an AI-installed (globalinstall) Fusion is invisible and onboarding "succeeds"
# yet the bridge still reports not-installed. (Found on the ADOMBASELINE test.)
def _scan_all_user_webdeploy() -> list:
    r"""Scan ALL user profiles for Fusion webdeploy dirs (issue #326).

    Fusion may be installed under a different Windows user profile than the
    current process. Instead of just checking %LOCALAPPDATA%, enumerate all
    user directories under C:\Users and check each one's AppData\Local\Autodesk\webdeploy.

    macOS: the webdeploy root is per-account under ~/Library/Application Support,
    and holds <hash>/Autodesk Fusion.app bundle dirs (no FusionLauncher.exe).
    """
    bases = []
    if sys.platform == "darwin":
        mac_base = Path.home() / "Library" / "Application Support" / "Autodesk" / "webdeploy" / "production"
        if mac_base.is_dir():
            bases.append(mac_base)
        return bases
    try:
        users_dir = Path("C:\\Users")
        if users_dir.exists():
            for user_path in users_dir.iterdir():
                if user_path.is_dir():
                    webdeploy = user_path / "AppData" / "Local" / "Autodesk" / "webdeploy" / "production"
                    if webdeploy not in bases:
                        bases.append(webdeploy)
    except Exception:
        pass  # If enumeration fails, fall through to the normal paths below
    return bases

_WEBDEPLOY_BASES = []
# First, scan ALL user profiles for Fusion (issue #326: cross-user detection)
_WEBDEPLOY_BASES.extend(_scan_all_user_webdeploy())
# Then add the standard environment-based paths (current user + system-wide)
for _var in ("LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"):
    _root = os.environ.get(_var)
    if _root:
        _base = Path(_root) / "Autodesk" / "webdeploy" / "production"
        if _base not in _WEBDEPLOY_BASES:
            _WEBDEPLOY_BASES.append(_base)
# Back-compat alias (some callers import WEBDEPLOY_BASE directly).
if sys.platform == "darwin":
    WEBDEPLOY_BASE = _WEBDEPLOY_BASES[0] if _WEBDEPLOY_BASES else (
        Path.home() / "Library" / "Application Support" / "Autodesk" / "webdeploy" / "production"
    )
else:
    WEBDEPLOY_BASE = _WEBDEPLOY_BASES[0] if _WEBDEPLOY_BASES else (
        Path(os.environ.get("LOCALAPPDATA", "")) / "Autodesk" / "webdeploy" / "production"
    )

# macOS: the app bundle we launch, in the order we look for it. ~/Applications is
# where the Fusion installer puts a per-user install; /Applications is the
# all-users one.
_MAC_APP_BUNDLE = "Autodesk Fusion.app"
_MAC_APP_NAME = "Autodesk Fusion"
_MAC_APP_BUNDLES = [
    Path.home() / "Applications" / "Autodesk Fusion.app",
    Path("/Applications/Autodesk Fusion.app"),
    Path.home() / "Applications" / "Autodesk Fusion 360.app",
    Path("/Applications/Autodesk Fusion 360.app"),
]

# EVERY known per-user add-in dir, newest convention first. Fusion MOVED this
# across versions and silently ignores the others (issue #63: an add-in in the
# legacy API\AddIns dir never loads on 2025+ Fusion, which scans FusionAddins -
# proven live on a fresh install). Track ALL of them; never assume one path.
if sys.platform == "darwin":
    # ~/Library/Application Support/Autodesk (macOS has no %APPDATA%): reading an unset
    # Windows env var yields Path("") == Path("."), i.e. a path RELATIVE to the CWD.
    _mac_autodesk = Path.home() / "Library" / "Application Support" / "Autodesk"
    ADDINS_DIR_CANDIDATES = [
        _mac_autodesk / "FusionAddins",
        _mac_autodesk / "Autodesk Fusion" / "API" / "AddIns",
        _mac_autodesk / "Autodesk Fusion 360" / "API" / "AddIns",
    ]
else:
    ADDINS_DIR_CANDIDATES = [
        Path(os.environ.get("APPDATA", "")) / "Autodesk" / "FusionAddins",
        Path(os.environ.get("APPDATA", "")) / "Autodesk" / "Autodesk Fusion" / "API" / "AddIns",
        Path(os.environ.get("APPDATA", "")) / "Autodesk" / "Autodesk Fusion 360" / "API" / "AddIns",
    ]

# Keep callback references alive to prevent GC
_callbacks = []

# Guard windll so this module imports on non-Windows hosts (galliaApril smoke
# tests, macOS). The Win32-only functions below are never reached off-Windows —
# detect_fusion() short-circuits to installed:False — but the bridge must still
# boot + serve /status on any platform ab spawns it on.
user32 = ctypes.windll.user32 if hasattr(ctypes, "windll") else None


def detect_fusion() -> dict:
    """Detect Fusion 360 installation. Returns a dict with paths and status."""
    exe_path = _find_fusion_launcher()
    addins_dir = _find_addins_dir()
    addin_installed = _check_addin_installed(addins_dir) if addins_dir else False
    running = _is_fusion_running()

    if exe_path:
        return {
            "installed": True,
            "exe_path": str(exe_path),
            "addins_dir": str(addins_dir) if addins_dir else None,
            "addin_installed": addin_installed,
            "running": running,
        }

    return {
        "installed": False,
        "exe_path": None,
        "addins_dir": str(addins_dir) if addins_dir else None,
        "addin_installed": addin_installed,
        "running": running,
        "error": "Fusion 360 not found",
    }


# ---------------------------------------------------------------------------------------------------
# INSTALL-COMPLETENESS DETECTION  (root-caused live on a fresh Hyper-V VM, John 2026-07-14)
#
# WHAT A WEBDEPLOY INSTALL ACTUALLY LOOKS LIKE (verified on disk, do not assume):
#   %LOCALAPPDATA%\Autodesk\webdeploy\production\ holds one hash dir PER content-build. A COMPLETE
#   Fusion install is ~615 files (Fusion360.exe ~880 KB + hundreds of DLLs + Qt/ + Python/ + ...).
#   BUT Autodesk ALSO drops a tiny 8-file launcher STUB dir next to it: icons + FusionLauncher.exe
#   + a small FusionLauncher.exe.ini (~412 bytes). So at steady state there are TWO hash dirs.
#
# THE TRAP that broke the fresh VM (and the two earlier "fixes" that were WRONG):
#   * Keying "installed" off FusionLauncher.exe EXISTING -> true mid-stream (launcher lands early) ->
#     premature fusion_start -> "Error Launching Streamed Application ... .ini missing/incomplete".
#   * Keying off FusionLauncher.exe.ini existing / its SIZE -> ALSO wrong: the COMPLETE app dir has
#     NO .ini at all; only the throwaway stub carries a 412-byte .ini. A size gate would REJECT a
#     perfectly good install and ACCEPT nothing. (Measured it: complete app dir = 615 files, no .ini.)
#
# THE RELIABLE SIGNAL: the real application binary Fusion360.exe. It only exists (full size) when the
#   payload finished streaming. Gate "installed" on Fusion360.exe, NEVER on the launcher or its .ini.
# ---------------------------------------------------------------------------------------------------
_FUSION_APP_EXE = "Fusion360.exe"     # the ACTUAL app binary (~880 KB); the completeness marker
_MIN_APP_EXE_BYTES = 200_000          # guards against a 0-byte placeholder the streamer writes early


def _app_dir_complete(subdir: Path) -> bool:
    """A production hash dir is a COMPLETE Fusion install ONLY when it contains the real app binary
    Fusion360.exe at full size. This is process-independent and version-stable. Do NOT substitute
    FusionLauncher.exe (lands early, mid-stream) or FusionLauncher.exe.ini (absent from the complete
    app dir; only the launcher stub has one). See the block comment above for the on-disk anatomy."""
    exe = subdir / _FUSION_APP_EXE
    try:
        return exe.exists() and exe.stat().st_size >= _MIN_APP_EXE_BYTES
    except OSError:
        return False


def _any_app_complete() -> bool:
    """True if ANY webdeploy hash dir holds a complete Fusion (Fusion360.exe). This is the single
    source of truth for 'Fusion is installed'."""
    for base in _WEBDEPLOY_BASES:
        try:
            if base.exists():
                for subdir in base.iterdir():
                    if subdir.is_dir() and _app_dir_complete(subdir):
                        return True
        except OSError:
            continue
    return False


def _find_mac_fusion_app() -> Path | None:
    """The Fusion app bundle to launch on macOS, or None when Fusion is not installed.

    Same two questions as the launcher scan below, different shape: macOS ships an
    `Autodesk Fusion.app` BUNDLE (no Fusion360.exe, no FusionLauncher.exe.ini), so
    'complete install' is the bundle existing - its Contents/MacOS binary is what runs.
    Order: the installed bundle first (_MAC_APP_BUNDLES), then the webdeploy builds
    (<base>/<hash>/Autodesk Fusion.app, newest build first)."""
    for app in _MAC_APP_BUNDLES:
        if app.is_dir():
            return app
    builds = []
    for base in _WEBDEPLOY_BASES:
        try:
            if not base.is_dir():
                continue
            candidates = [base / _MAC_APP_BUNDLE]
            candidates.extend(d / _MAC_APP_BUNDLE for d in base.iterdir() if d.is_dir())
        except OSError:
            continue
        for app in candidates:
            try:
                if app.is_dir():
                    builds.append((app.stat().st_mtime, app))
            except OSError:
                continue
    if builds:
        builds.sort(key=lambda c: c[0], reverse=True)
        return builds[0][1]
    return None


def _find_fusion_launcher() -> Path | None:
    """Return the FusionLauncher.exe to RUN, but only when Fusion is actually INSTALLED.

    TWO SEPARATE QUESTIONS (conflating them is what broke the launch, John 2026-07-14):
      1. Is Fusion installed?  ->  a hash dir contains the real app binary Fusion360.exe.
      2. Which launcher do I run?  ->  a FusionLauncher.exe that HAS its FusionLauncher.exe.ini
         BESIDE IT. Running a FusionLauncher.exe WITHOUT its .ini throws Fusion's own
         "Error Launching Streamed Application ... FusionLauncher.exe.ini is missing or incomplete".
         The complete APP dir (615 files, Fusion360.exe) frequently has the launcher but NO .ini; a
         separate tiny STUB dir carries FusionLauncher.exe + its ~412-byte .ini and knows how to load
         the app. So we must return the launcher-WITH-.ini, NOT the app dir's bare launcher.

    Because we gate on Fusion360.exe first, the stub's .ini is guaranteed complete by then (mid-stream
    there is no Fusion360.exe, so we return None and nothing launches into a half-written install)."""
    if sys.platform == "darwin":
        return _find_mac_fusion_app()
    if not _any_app_complete():
        return None                                   # not installed (or still streaming) -> None

    runnable = []      # (mtime, launcher) - FusionLauncher.exe that has its .ini beside it
    bare = []          # (mtime, launcher) - FusionLauncher.exe with NO .ini (fallback only)
    app_exes = []      # (mtime, Fusion360.exe) - last-ditch fallback
    for base in _WEBDEPLOY_BASES:
        if not base.exists():
            continue
        try:
            subdirs = [d for d in base.iterdir() if d.is_dir()]
        except OSError:
            continue
        for subdir in subdirs:
            try:
                mtime = subdir.stat().st_mtime
            except OSError:
                mtime = 0.0
            launcher = subdir / "FusionLauncher.exe"
            if launcher.exists():
                if (subdir / "FusionLauncher.exe.ini").exists():
                    runnable.append((mtime, launcher))
                else:
                    bare.append((mtime, launcher))
            if _app_dir_complete(subdir):
                app_exes.append((mtime, subdir / _FUSION_APP_EXE))

    for pool in (runnable, bare, app_exes):            # prefer launcher+ini, then bare launcher, then app exe
        if pool:
            pool.sort(key=lambda c: c[0], reverse=True)
            return pool[0][1]
    return None


def _incomplete_webdeploy_present() -> bool:
    """Disk-state (process-INDEPENDENT) 'a fresh install is actively streaming' signal: some hash dir
    has FusionLauncher.exe but NO complete Fusion360.exe exists ANYWHERE yet. Why disk-state and not a
    tasklist check: the Autodesk streamer spawns short-lived per-chunk worker processes, so
    _installer_running() reads False most of a live multi-GB stream (flickered True only 3 of 25 polls
    during a real install, John 2026-07-14). Why gate on 'no complete app anywhere': once the app IS
    complete, the leftover launcher stub (FusionLauncher.exe, no Fusion360.exe) is NORMAL and must NOT
    read as 'still installing'. (An auto-UPDATE that streams a new build beside a complete old one is
    handled separately by fusion_update_in_progress(), not here.) Never raises."""
    if _any_app_complete():
        return False                                  # a complete install exists -> not installing
    for base in _WEBDEPLOY_BASES:
        try:
            if not base.exists():
                continue
            for subdir in base.iterdir():
                if subdir.is_dir() and (subdir / "FusionLauncher.exe").exists():
                    return True                       # launcher present, no complete app -> streaming
        except OSError:
            continue
    return False


def _clean_incomplete_webdeploy() -> list:
    """Remove the residue of an INTERRUPTED stream so the next fusion_install_fusion re-streams clean.
    Only ever called AFTER a launch failure (we already know something is corrupt).

    SAFETY GUARD: if a COMPLETE install (Fusion360.exe) exists anywhere, do NOTHING and return [] -
    the launcher-only stub dir beside a complete app is a LEGIT part of the install, not corruption,
    and nuking it would break a working Fusion. Only when NO complete app exists (a genuinely half-
    streamed state) do we remove the partial launcher-bearing dirs. Never raises. (John 2026-07-14:
    the old .ini-size test would have deleted the legit stub of a healthy install.)"""
    import shutil
    if _any_app_complete():
        return []                                     # healthy install present - never touch it
    removed = []
    for base in _WEBDEPLOY_BASES:
        if not base.exists():
            continue
        try:
            subdirs = [d for d in base.iterdir() if d.is_dir()]
        except OSError:
            continue
        for subdir in subdirs:
            if not (subdir / "FusionLauncher.exe").exists():
                continue
            if not _app_dir_complete(subdir):         # partial: launcher but no real app binary
                try:
                    shutil.rmtree(subdir, ignore_errors=True)
                    removed.append(subdir.name)
                except Exception:
                    pass
    return removed


def fusion_update_in_progress() -> bool:
    """Best-effort: is Fusion ACTIVELY applying an auto-update right now? While it is,
    Fusion crash-restarts ITSELF every ~30-60s, so verbs intermittently see 'not
    running' (observed live on a fresh VM 2026-07-05: hashes 441fa88... + 6a0c961...
    being streamed side by side). Callers surface a distinct non-fatal `updating`
    status instead of a fatal error so a driving AI waits/retries. Never raises.

    IMPORTANT (regression fix 2026-07-06, caught on a real laptop): a bare
    "2+ production dirs" count is NOT an update - Fusion NEVER prunes old webdeploy
    builds, so a normal machine accumulates many stale production dirs (the laptop had
    5, from April-June, Fusion not even running). That made the flag fire permanently
    and falsely. So require ACTIVE streaming: 2+ launcher builds AND at least one build
    dir TOUCHED in the last ~20 min (the streamer is writing into it). Stale leftover
    dirs have old mtimes and no longer trigger it."""
    import time as _t
    now = _t.time()
    RECENT_SECS = 20 * 60
    launcher_mtimes = []
    for base in _WEBDEPLOY_BASES:
        try:
            if not base.exists():
                continue
            for d in base.iterdir():
                try:
                    if d.is_dir() and (d / "FusionLauncher.exe").exists():
                        launcher_mtimes.append(d.stat().st_mtime)
                except OSError:
                    continue
        except OSError:
            continue
    if len(launcher_mtimes) < 2:
        return False
    return any((now - mt) < RECENT_SECS for mt in launcher_mtimes)


def _find_addins_dir() -> Path | None:
    """Find the Fusion 360 add-ins directory."""
    for candidate in ADDINS_DIR_CANDIDATES:
        if candidate.exists():
            return candidate
    # Return the first candidate even if it doesn't exist yet (for install_addin)
    return ADDINS_DIR_CANDIDATES[0] if ADDINS_DIR_CANDIDATES else None


def _check_addin_installed(addins_dir: Path | None) -> bool:
    """Check if the AdomBridge add-in is installed in ANY known add-in dir.

    Scans every ADDINS_DIR_CANDIDATES entry (not just the preferred one) - the
    copy that counts is whichever dir THIS Fusion version scans, and we can't
    know that a priori, so presence anywhere counts as installed."""
    for candidate in ADDINS_DIR_CANDIDATES:
        if (candidate / "AdomBridge" / "AdomBridge.py").exists():
            return True
    if addins_dir and (Path(addins_dir) / "AdomBridge" / "AdomBridge.py").exists():
        return True
    return False


def _is_fusion_running() -> bool:
    """Check if Fusion 360 is running (process check is authoritative).

    Uses tasklist instead of window title matching to avoid false positives
    from browser tabs that mention 'Autodesk Fusion' in their title.
    """
    return _has_fusion_process()


_FUSION_RUNNING_MEMO = {"value": None, "at": 0.0}


def _fusion_pids_toolhelp() -> list:
    """PIDs of Fusion360.exe via a Toolhelp32 snapshot: no child process, no console,
    milliseconds even when the CPU is pegged. Empty list on any failure."""
    try:
        import ctypes
        from ctypes import wintypes
        k32 = ctypes.windll.kernel32
        TH32CS_SNAPPROCESS = 0x2

        class PE32(ctypes.Structure):
            _fields_ = [("dwSize", wintypes.DWORD), ("cntUsage", wintypes.DWORD),
                        ("th32ProcessID", wintypes.DWORD),
                        ("th32DefaultHeapID", ctypes.POINTER(ctypes.c_ulong)),
                        ("th32ModuleID", wintypes.DWORD), ("cntThreads", wintypes.DWORD),
                        ("th32ParentProcessID", wintypes.DWORD), ("pcPriClassBase", ctypes.c_long),
                        ("dwFlags", wintypes.DWORD), ("szExeFile", ctypes.c_char * 260)]
        snap = k32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
        if snap == wintypes.HANDLE(-1).value or not snap:
            return []
        try:
            pe = PE32(); pe.dwSize = ctypes.sizeof(PE32)
            out = []
            ok = k32.Process32First(snap, ctypes.byref(pe))
            while ok:
                if pe.szExeFile.decode(errors="ignore").lower() == "fusion360.exe":
                    out.append(int(pe.th32ProcessID))
                ok = k32.Process32Next(snap, ctypes.byref(pe))
            return out
        finally:
            k32.CloseHandle(snap)
    except Exception:
        return []


def _has_fusion_process() -> bool:
    """Check if any Fusion 360 process is running.

    ⛔ A FALSE "NOT RUNNING" LAUNCHES A SECOND FUSION (2026-08-29, John saw Fusion's
    "Multiple instances are not supported" dialog mid-import): the tasklist probe below
    timed out while Fusion pegged the CPU importing a STEP, the except path answered
    False, and the dispatcher's auto-launch started another instance. So: a snapshot
    probe that cannot stall comes first, tasklist is the confirmation, and a probe that
    FAILS answers with the last known state, never False.
    """
    if sys.platform == "darwin":
        try:
            r = _has_fusion_process_pgrep()
            _FUSION_RUNNING_MEMO.update(value=r, at=time.time())
            return r
        except Exception:
            last = _FUSION_RUNNING_MEMO.get("value")
            return bool(last) if last is not None else False
    try:
        _pids = _fusion_pids_toolhelp()
        if _pids and any(_pid_alive(p) for p in _pids):
            _FUSION_RUNNING_MEMO.update(value=True, at=time.time())
            return True
    except Exception:
        pass
    try:
        r = _has_fusion_process_tasklist()
        _FUSION_RUNNING_MEMO.update(value=r, at=time.time())
        return r
    except Exception:
        last = _FUSION_RUNNING_MEMO.get("value")
        return bool(last) if last is not None else False


def _has_fusion_process_pgrep() -> bool:
    """pgrep-based check (macOS). -x matches the exact process name; the -f fallback
    matches a full command line, because the bundle's CFBundleExecutable is
    "Autodesk Fusion" (not Fusion360) and Fusion may be running either binary.
    A missing/nonzero pgrep is a real 'no match' (exit 1) or an error -> raises so the
    caller answers with the last known state."""
    for argv in (["pgrep", "-x", "Fusion360"], ["pgrep", "-f", _MAC_APP_NAME]):
        r = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, text=True, timeout=5)
        if r.returncode == 0:
            return True
    return False


def _has_fusion_process_tasklist() -> bool:
    """tasklist-based check (raises on timeout so the caller can fall back)."""
    try:
        # CREATE_NO_WINDOW (0x08000000) on Windows so the child cmd window doesn't
        # flash on screen for every poll. The Tauri side spawns the Python bridge
        # itself with CREATE_NO_WINDOW, but Python's subprocess.run/Popen creates
        # a fresh console window for each child by default — without this flag,
        # every fusion_running check would briefly show a black cmd window.
        creationflags = 0x08000000 if hasattr(subprocess, "CREATE_NO_WINDOW") else 0
        if hasattr(subprocess, "CREATE_NO_WINDOW"):
            creationflags = subprocess.CREATE_NO_WINDOW
        # stdin=DEVNULL is load-bearing: ab spawns the bridge console-less +
        # detached, so the inherited stdin handle is invalid. Without an explicit
        # DEVNULL, this child can block on a dead handle and deadlock the probe —
        # the exact failure mode that bit the KiCad bridge.
        output = subprocess.check_output(
            ["tasklist", "/FI", "IMAGENAME eq Fusion360.exe", "/FO", "CSV", "/NH"],
            stdin=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=5,
            creationflags=creationflags,
        )
        # tasklist returns "INFO: No tasks..." when no match
        if "Fusion360.exe" not in output:
            return False
        # ⚠️ TASKLIST LISTS THE DEAD (root-caused live on ConfRoomROG, 2026-08-19).
        # A Fusion360.exe whose parent died without reaping it stays in the process
        # table as a ZOMBIE: tasklist AND WMI still show it (1GB working set, 1
        # thread), but HasExited is True and taskkill answers "There is no running
        # instance of the task". Trusting tasklist alone made this bridge report
        # running:true forever, which meant fusion_start returned "already running"
        # and NEVER LAUNCHED, every kill failed, and readiness waited on an add-in
        # that could never come. The box was unusable for hours with no error
        # anywhere. So confirm at least one PID is GENUINELY alive.
        pids = []
        for line in output.splitlines():
            parts = [p.strip('"') for p in line.split('","')]
            if len(parts) > 1:
                try:
                    pids.append(int(parts[1]))
                except Exception:
                    pass
        if not pids:
            return "Fusion360.exe" in output     # cannot parse -> old behaviour
        return any(_pid_alive(p) for p in pids)
    except Exception:
        raise   # the caller answers with the last known state, never a guessed False


_STILL_ACTIVE = 259
_PROCESS_QUERY_LIMITED_INFORMATION = 0x1000


def _pid_alive(pid: int) -> bool:
    """Is this PID a LIVE process, or a zombie the process table has not let go of?

    GetExitCodeProcess is the ground truth: a zombie returns its real exit code,
    a live process returns STILL_ACTIVE (259). If the handle cannot be opened we
    answer True - an access-denied process is running, just not ours to inspect,
    and guessing "dead" there would make the bridge launch a second Fusion.
    """
    try:
        k32 = ctypes.WinDLL("kernel32", use_last_error=True)
        h = k32.OpenProcess(_PROCESS_QUERY_LIMITED_INFORMATION, False, int(pid))
        if not h:
            return True
        try:
            code = ctypes.wintypes.DWORD()
            ok = k32.GetExitCodeProcess(h, ctypes.byref(code))
            if not ok:
                return True
            return code.value == _STILL_ACTIVE
        finally:
            k32.CloseHandle(h)
    except Exception:
        return True


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


def is_fusion_running() -> bool:
    """Public API: check if Fusion 360 is currently running (live check)."""
    return _has_fusion_process()


def wait_for_addin(timeout: int = 90, dismiss_dialogs: bool = False) -> bool:
    """Wait for the AdomBridge add-in HTTP server to become responsive.

    Polls the add-in health endpoint once per second up to `timeout` seconds.
    Returns True if the add-in responded, False on timeout.

    If dismiss_dialogs is True, also checks for and dismisses blocking dialogs
    (startup picker, recovery dialog) every 5 seconds while waiting. This is
    critical at startup: dialogs block the add-in's main thread, so dismissing
    them is required for the add-in to become responsive.
    """
    for i in range(timeout):
        try:
            req = urllib.request.Request(
                f"http://127.0.0.1:{ADDIN_PORT}/health", method="GET"
            )
            with urllib.request.urlopen(req, timeout=1) as resp:
                return True
        except Exception:
            # Every 5 seconds during the wait, try to dismiss blocking dialogs.
            # Dialogs are the #1 reason the add-in can't respond at startup.
            if dismiss_dialogs and i >= 5 and i % 5 == 0:
                _dismiss_startup_dialogs()
            time.sleep(1)
    return False


def _dismiss_startup_dialogs():
    """Dismiss common Fusion startup dialogs (recovery, startup picker).

    Uses Win32 APIs directly — no add-in needed. Called during wait_for_addin
    to unblock the main thread so the add-in can become responsive.

    ⚠️ THE SEAT DIALOG IS NOT DISMISSIBLE, IT MUST BE ANSWERED (found live on
    ConfRoomROG, 2026-08-19): "Active Sessions Exceeded" ignores Escape and
    WM_CLOSE, so a generic dismiss sweep hammered it five times, logged it as a
    cleared "startup-picker", and left Fusion blocked for 378s while the Dashboard
    waited on an add-in that could never come up. Route it to the seat resolver
    (UIA Invoke of Continue) FIRST and return - never send Escape at a modal that
    is asking the user to choose something.
    """
    try:
        if find_licensing_dialog() is not None:
            # The resolver lives in server.py (it needs the UIA + owned-popup helpers there).
            # server is the entry point, so it is already in sys.modules - look it up rather
            # than importing, which would re-execute it.
            import sys as _sys
            _srv = _sys.modules.get("server") or _sys.modules.get("__main__")
            _fn = getattr(_srv, "_resolve_seat_dialog", None) if _srv else None
            if _fn:
                _fn(max_clicks=3)
                return
    except Exception:
        pass
    try:
        from handlers.dismiss_recovery import dismiss_recovery_dialog
        dismiss_recovery_dialog(max_wait=2.0)
    except Exception:
        pass

    # Dismiss startup picker ("What do you want to design?") and other
    # Fusion dialogs by sending Escape to the Fusion main window.
    try:
        from handlers.fusion_ui import send_key_to_fusion
        send_key_to_fusion("escape")
    except Exception:
        pass


# ── Background launch: NEVER steal the user's foreground ──────────────────────
# The bridge launches Fusion for AUTOMATION, on a machine the user is actively
# working on. Fusion normally opens its splash + main window in the FOREGROUND,
# yanking focus off whatever the user is doing. That is unacceptable (John, 2026-
# 07-06: "make sure new windows ALWAYS only open in the background"). So we (1)
# start the process MINIMIZED + NOT ACTIVATED via STARTUPINFO, and (2) run a short
# focus-guard thread that, whenever a Fusion window steals the foreground during
# the noisy startup window, minimizes it WITHOUT activating - returning focus to
# the user's work. The add-in still loads fully while minimized. To SHOW Fusion to
# the user later, a verb explicitly restores+foregrounds it (the ONLY time we ever
# foreground). See the fusion-bridge-dev skill "Background-launch invariant".
_SW_SHOWMINNOACTIVE = 7  # show minimized, do NOT activate/steal focus


# Every process in the Autodesk launch family whose windows must NEVER steal the
# user's foreground: the main app (Fusion360.exe), the streamed-app launcher +
# splash ("Loading additional modules" - FusionLauncher.exe), and the identity/seat
# manager (the "Signing in" + "Active Sessions Exceeded" dialogs - AdskIdentityManager
# .exe). The 1.6.33 guard only knew Fusion360.exe, so the splash + seat dialog sailed
# straight past it and yanked focus (John, 2026-07-06). Match the whole family.
_FUSION_FAMILY_IMAGES = {
    "fusion360.exe", "fusionlauncher.exe", "adskidentitymanager.exe",
    "adsso.exe", "adcefwebbrowser.exe", "fusion360bootstrap.exe",
}


def _fusion_family_pids() -> set:
    """PIDs of every running Autodesk-launch-family process (via one tasklist CSV)."""
    try:
        cnw = subprocess.CREATE_NO_WINDOW if hasattr(subprocess, "CREATE_NO_WINDOW") else 0
        out = subprocess.check_output(
            ["tasklist", "/FO", "CSV", "/NH"],
            stdin=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=cnw,
        ).decode("utf-8", "replace")
    except Exception:
        return set()
    pids = set()
    for m in re.finditer(r'"([^"]+\.exe)","(\d+)"', out):
        if m.group(1).lower() in _FUSION_FAMILY_IMAGES:
            pids.add(int(m.group(2)))
    return pids


def _family_pid_images() -> dict:
    """{pid: image_name} for every running Autodesk-launch-family process."""
    try:
        cnw = subprocess.CREATE_NO_WINDOW if hasattr(subprocess, "CREATE_NO_WINDOW") else 0
        out = subprocess.check_output(
            ["tasklist", "/FO", "CSV", "/NH"],
            stdin=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=cnw,
        ).decode("utf-8", "replace")
    except Exception:
        return {}
    m = {}
    for mo in re.finditer(r'"([^"]+\.exe)","(\d+)"', out):
        img = mo.group(1).lower()
        if img in _FUSION_FAMILY_IMAGES:
            m[int(mo.group(2))] = img
    return m


def family_windows() -> list:
    """Visible top-level windows owned by the Autodesk launch family (Fusion360 /
    FusionLauncher / AdskIdentityManager). Each dict: {hwnd, title, pid, image, width,
    height, left, top}.

    This is how the launch state machine detects the SEAT/licensing dialog
    DETERMINISTICALLY - by OWNING PROCESS + size, NOT the "Fusion360" window TITLE that
    fooled the title-based classifier for a whole day (the "Active Sessions Exceeded"
    dialog's title is literally just "Fusion360"; the real text is CEF body content).
    Never raises."""
    results = []
    try:
        user32 = ctypes.windll.user32
        HWND = ctypes.wintypes.HWND
        user32.GetWindowThreadProcessId.argtypes = [HWND, ctypes.POINTER(ctypes.wintypes.DWORD)]
        user32.IsWindowVisible.argtypes = [HWND]
        # restype/argtypes MUST be set or the returned HWND is truncated to 32-bit on
        # 64-bit Windows and the owner handle comes back wrong (same class of bug that
        # silently broke the focus-guard, 2026-07-06).
        user32.GetWindow.restype = HWND
        user32.GetWindow.argtypes = [HWND, ctypes.c_uint]
        fam = _fusion_family_pids()
        if not fam:
            return results
        pid_img = _family_pid_images()
    except Exception:
        return results
    WNDENUMPROC = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.wintypes.HWND, ctypes.wintypes.LPARAM)

    def cb(hwnd, _l):
        try:
            if not user32.IsWindowVisible(hwnd):
                return True
            wpid = ctypes.wintypes.DWORD(0)
            user32.GetWindowThreadProcessId(hwnd, ctypes.byref(wpid))
            if wpid.value not in fam:
                return True
            n = user32.GetWindowTextLengthW(hwnd)
            title = ""
            if n:
                buf = ctypes.create_unicode_buffer(n + 1)
                user32.GetWindowTextW(hwnd, buf, n + 1)
                title = buf.value
            rect = ctypes.wintypes.RECT()
            user32.GetWindowRect(hwnd, ctypes.byref(rect))
            w = rect.right - rect.left
            h = rect.bottom - rect.top
            if w <= 0 or h <= 0:
                return True
            GW_OWNER = 4
            owner = user32.GetWindow(hwnd, GW_OWNER)
            # TOOL WINDOWS ARE NEVER MODALS (2026-09-03, ConfRoomROG): Fusion's message tray
            # is an OWNED, LANDSCAPE, 640x196 window titled "Fusion360" (class
            # Qt683QWindowToolSaveBits, WS_EX_TOOLWINDOW), so it passed every shape gate and
            # readiness reported licensingDialog:true, ready:false on a healthy Fusion for
            # hours. The seat / sign-in modals are ordinary owned windows; a tool window is a
            # palette, tray or popup by definition. Record the ex-style so the gate can skip it.
            GWL_EXSTYLE, WS_EX_TOOLWINDOW = -20, 0x00000080
            try:
                exstyle = user32.GetWindowLongW(hwnd, GWL_EXSTYLE)
            except Exception:
                exstyle = 0
            cls = ""
            try:
                cbuf = ctypes.create_unicode_buffer(128)
                user32.GetClassNameW(hwnd, cbuf, 128)
                cls = cbuf.value
            except Exception:
                pass
            results.append({
                "hwnd": int(hwnd), "title": title, "pid": int(wpid.value),
                "image": pid_img.get(int(wpid.value), ""),
                "width": w, "height": h, "left": rect.left, "top": rect.top,
                "owner": int(owner) if owner else 0,
                "toolWindow": bool(exstyle & WS_EX_TOOLWINDOW) or cls.endswith("ToolSaveBits"),
                "className": cls,
            })
        except Exception:
            pass
        return True

    cb_c = WNDENUMPROC(cb)
    _callbacks.append(cb_c)
    try:
        user32.EnumWindows(cb_c, 0)
    except Exception:
        pass
    try:
        _callbacks.remove(cb_c)
    except ValueError:
        pass
    return results


def find_licensing_dialog():
    """The seat/licensing or sign-in modal ("Active Sessions Exceeded" / "Suspend Remote
    Session" / a sign-in prompt). Detected DETERMINISTICALLY by the OWNED-POPUP signal,
    NOT a fragile height window:

      A seat/sign-in modal is ALWAYS an owned popup of the main Fusion window (confirmed
      via desktop_screenshot_window's ownedPopupCount). The splash and the maximized main
      app are NEVER owned popups. So "family window that has an owner" is the reliable
      tell - it does not care about the exact dialog size or the "Fusion360" title.

    This replaced a size-only filter whose 320px height floor SILENTLY MISSED the
    823x262 "Suspend Remote Session" confirm variant, so readiness reported the dialog
    gone while it sat there blocking (John caught it live, 2026-07-06). A size-based
    fallback is kept for safety in case owner detection ever returns 0.
    Returns the window dict (with 'owner'), or None.

    SHAPE matters, not just ownership: Fusion's own side panels (Data Panel, browser, the
    comments rail) are ~300px-wide OWNED popups too, so an owned-only match false-fires the
    moment a document is open and reads a drivable Fusion as 'blocked' (caught live on winvm
    2026-07-06, right after this over-broad owned check shipped). The seat/sign-in modal is
    always WIDE (823x262 'Suspend Remote Session', 1235x527 'Active Sessions Exceeded') - far
    wider than any side panel - so we require dialog PROPORTIONS: width >= 480 (excludes the
    narrow panels), height >= 180 (excludes toolbar strips), and smaller than the maximized
    main app."""
    wins = family_windows()
    if not wins:
        return None
    # NEVER match the MAIN app window. A seat/sign-in modal is a smaller popup; the main Fusion
    # window is the LARGEST family window. The old no-owner fallback matched the main window whenever
    # it happened to fall inside the shape gate (e.g. a non-maximized 1300x820 window fits
    # 480<=w<=1500, 180<=h<=900) - so find_licensing_dialog false-fired on the drivable main app,
    # and once ab_client came online the seat resolver churned 8x on it and HUNG readiness (John,
    # live 2026-07-14). Excluding the largest window is size-independent and safe: a dialog is never
    # bigger than the app that owns it.
    main_hwnd = max(wins, key=lambda w: w["width"] * w["height"])["hwnd"]
    # ⚠️ ABSOLUTE PIXEL CEILINGS ARE A TRAP - this is the SECOND time one silently hid the
    # dialog. A 320px height FLOOR missed the 823x262 confirm variant (2026-07-06); then a
    # 1500px width CEILING missed "Active Sessions Exceeded" rendering 1646x702 on
    # ConfRoomROG's 2560-wide display (2026-08-19). Fusion sizes this dialog from the
    # display, so no fixed pixel range can be right on every machine. The DPI-independent
    # test is RELATIVE: a dialog is always meaningfully smaller than the app that owns it.
    _main = max(wins, key=lambda w: w["width"] * w["height"])
    _main_area = max(1, _main["width"] * _main["height"])

    def _dialog_shaped(w):
        ww, hh = w["width"], w["height"]
        if w.get("toolWindow"):
            return False          # a tray / palette / popup (WS_EX_TOOLWINDOW), never a modal
        if ww < 480 or hh < 180:
            return False          # narrow side panels / toolbar strips are not modals
        # ORIENTATION is the tell that finally separates a modal from a DOCKED PANEL.
        # Fusion's docked rails (Content Manager, the layer/properties column) are owned,
        # titled "Fusion360" exactly like the seat dialog, and pass every size gate above:
        # measured 600x1620 on ConfRoomROG, which reported licensingDialog:true and pinned
        # ready:false on a completely healthy Fusion (twice, 2026-09-01). Every real seat /
        # sign-in modal is LANDSCAPE and SHORT - 823x262, 1235x527, 1646x702 - i.e. at least
        # ~2.3x wider than tall and never more than about a third of the app's height. A
        # docked rail is portrait (600x1620 = 0.37 wide-to-tall) and spans ~78% of the app.
        # Both tests are ratios, so they stay right on any DPI or display (the lesson the
        # absolute-pixel comments above were already shouting about).
        if ww < hh * 1.3:
            return False          # portrait = a docked rail, not a modal
        if hh > 0.60 * max(1, _main["height"]):
            return False          # full-height = a docked rail, not a modal
        return (ww * hh) <= 0.85 * _main_area

    # Primary: an OWNED popup that is DIALOG-SHAPED (a modal, not a narrow side panel).
    for w in wins:
        if w["hwnd"] == main_hwnd:
            continue
        if w.get("owner") and _dialog_shaped(w):
            return w
    # Fallback (owner==0 for all - owner detection failed): same shape gate, still NOT the main window.
    for w in wins:
        if w["hwnd"] == main_hwnd:
            continue
        if _dialog_shaped(w):
            return w
    return None


def _fusion_pids() -> set:
    """PIDs of running Fusion360.exe processes (via tasklist CSV). Empty on error."""
    try:
        cnw = subprocess.CREATE_NO_WINDOW if hasattr(subprocess, "CREATE_NO_WINDOW") else 0
        out = subprocess.check_output(
            ["tasklist", "/FI", "IMAGENAME eq Fusion360.exe", "/FO", "CSV", "/NH"],
            stdin=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=cnw,
        ).decode("utf-8", "replace")
    except Exception:
        return set()
    return {int(p) for p in re.findall(r'"Fusion360\.exe","(\d+)"', out)}


_SPI_GETFOREGROUNDLOCKTIMEOUT = 0x2000
_SPI_SETFOREGROUNDLOCKTIMEOUT = 0x2001
_SPI_GETANIMATION = 0x0048
_SPI_SETANIMATION = 0x0049


class _ANIMATIONINFO(ctypes.Structure):
    _fields_ = [("cbSize", ctypes.c_uint), ("iMinAnimate", ctypes.c_int)]


def _get_min_animate():
    """Current 'animate windows when minimizing/maximizing' setting (1=on), or None."""
    try:
        user32 = ctypes.windll.user32
        ai = _ANIMATIONINFO()
        ai.cbSize = ctypes.sizeof(_ANIMATIONINFO)
        user32.SystemParametersInfoW(_SPI_GETANIMATION, ai.cbSize, ctypes.byref(ai), 0)
        return int(ai.iMinAnimate)
    except Exception:
        return None


def _set_min_animate(val) -> bool:
    """Turn the minimize/maximize ANIMATION on(1)/off(0). We turn it OFF during launch
    so the focus-guard's minimize is an INSTANT single-frame vanish rather than the
    visible slide-down that read as a 'flash' (John 2026-07-06). Restored to the user's
    original value when the launch window ends. Best-effort; no persist/broadcast."""
    try:
        user32 = ctypes.windll.user32
        ai = _ANIMATIONINFO()
        ai.cbSize = ctypes.sizeof(_ANIMATIONINFO)
        ai.iMinAnimate = int(val)
        user32.SystemParametersInfoW(_SPI_SETANIMATION, ai.cbSize, ctypes.byref(ai), 0)
        return True
    except Exception:
        return False


def _get_fg_lock_timeout():
    """Current SystemParametersInfo foreground-lock timeout (ms), or None."""
    try:
        user32 = ctypes.windll.user32
        val = ctypes.wintypes.DWORD(0)
        user32.SystemParametersInfoW(_SPI_GETFOREGROUNDLOCKTIMEOUT, 0, ctypes.byref(val), 0)
        return int(val.value)
    except Exception:
        return None


def _set_fg_lock_timeout(ms) -> bool:
    """Set the foreground-lock timeout. A NONZERO value makes Windows DENY foreground
    changes from processes that don't own the foreground / have no recent user input -
    they get a taskbar flash instead of an actual foreground grab. We raise it right
    before launching Fusion so its splash/main/seat windows can't STEAL focus in the
    first place (proactive), rather than relying only on the reactive minimize-guard
    (which leaves a visible flash as the window appears-then-shrinks). Restored to the
    user's original value when the guard's launch window ends. Best-effort."""
    try:
        user32 = ctypes.windll.user32
        user32.SystemParametersInfoW.argtypes = [
            ctypes.c_uint, ctypes.c_uint, ctypes.c_void_p, ctypes.c_uint]
        user32.SystemParametersInfoW(
            _SPI_SETFOREGROUNDLOCKTIMEOUT, 0, ctypes.c_void_p(int(ms)), 0)
        return True
    except Exception:
        return False


def _popen_fusion_background(exe_path: str):
    """Launch the Fusion launcher MINIMIZED + NOT ACTIVATED, console-less, and DETACHED
    from this bridge process's tree.

    ⚠ THE #50 ROOT CAUSE (proven by process parentage on AdomLapper, 2026-08-23):
    Fusion360.exe <- FusionLauncher.exe <- python (this bridge) <- adom-bridge.exe.
    Fusion was a CHILD of the bridge, so every bridge reap - each `bridge_install` of a
    release, ab's supervisor respawn (adom-bridge #61), the idle reaper - killed Fusion
    with it. Seven releases shipped during a 20-part build matrix = seven dead Fusions,
    every one mid-chain, every one a 'clean exit' with no crash artifact.

    Two independent kill paths have to be defeated:
      1. Job-object kill-on-close  -> CREATE_BREAKAWAY_FROM_JOB (falls back if the job
         forbids breakaway: launching at all beats launching detached).
      2. Parent-pid tree walks (taskkill /T) -> REPARENT via `cmd /c start`: cmd exits
         immediately, so the launcher's parent is gone and no tree walk from the bridge
         can reach Fusion.
    DETACHED_PROCESS + CREATE_NEW_PROCESS_GROUP also shield it from console/ctrl events.
    `start /min` keeps the launcher minimized-not-activated, like STARTUPINFO did.

    macOS: `open -a` hands the bundle to launchd, so Fusion is not in our process tree at
    all from the first instant - the same defeat of parent-pid tree walks that `cmd /c start`
    buys on Windows. start_new_session=True detaches it from our session/terminal too."""
    if sys.platform == "darwin":
        app = exe_path if exe_path and Path(exe_path).exists() else _MAC_APP_NAME
        return subprocess.Popen(
            ["open", "-a", str(app)],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
    cnw = subprocess.CREATE_NO_WINDOW if hasattr(subprocess, "CREATE_NO_WINDOW") else 0
    DETACHED_PROCESS = 0x00000008
    CREATE_NEW_PROCESS_GROUP = 0x00000200
    CREATE_BREAKAWAY_FROM_JOB = 0x01000000
    base = cnw | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    # `start "" /min <exe>`: the empty "" is the window title slot start expects first.
    argv = ["cmd", "/c", "start", "", "/min", exe_path]
    common = dict(stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        return subprocess.Popen(argv, creationflags=base | CREATE_BREAKAWAY_FROM_JOB, **common)
    except OSError:
        # Job forbids breakaway (ERROR_ACCESS_DENIED): still reparented via cmd, which
        # defeats tree walks; only a job-object kill could still reach it.
        return subprocess.Popen(argv, creationflags=base, **common)



# ── The background envelope (John, 2026-08-14: "ALWAYS open in the background") ──────────────
# The launch guard above only protected fusion_start. But the focus thefts users actually feel
# happen DURING operations on a running Fusion: a cloud open activates the document window, a
# picker/recovery dialog pops, an import raises a chooser - all while the user is typing in
# another app. The envelope generalizes the guard so ANY window-raising verb runs inside it:
#   - foreground-lock raised for the duration (steals become taskbar flashes)
#   - a watcher demotes any Autodesk-family window that still grabs focus
#   - refcounted, so overlapping verbs nest and only the LAST one restores the user's settings
# Two demotion modes: minimize=True (launch: splash/seat windows vanish) and minimize=False
# (running ops: the main window is pushed to the BOTTOM of the z-order WITHOUT minimizing, so
# PrintWindow captures keep rendering it correctly).
_ENVELOPE_LOCK = threading.Lock()
_ENVELOPE_COUNT = 0
_ENVELOPE_SAVED = {}
# The user-is-watching latch (shared across concurrent envelope watchers): once the USER
# raises a Fusion window, ALL watchers stand down until the user focuses something else.
_USER_WATCHING = [False]


def _fusion_main_hwnd_quick():
    """Largest titled family window's hwnd, or None - cheap, for the taskbar badge."""
    try:
        wins = family_windows() or []
        if not wins:
            return None
        return max(wins, key=lambda w: w.get("width", 0) * w.get("height", 0)).get("hwnd")
    except Exception:
        return None


class _FILETIME(ctypes.Structure):
    _fields_ = [("dwLowDateTime", ctypes.wintypes.DWORD),
                ("dwHighDateTime", ctypes.wintypes.DWORD)]


def fusion_using_cpu(sample_s: float = 0.7) -> bool:
    """True if a Fusion-family process is ACTIVELY consuming CPU right now (#49 part 10, John
    2026-08-23: a heavy first-ever cloud mint can exceed a fixed wall-clock ceiling while still
    WORKING, and the hang gate then kills the very run the long-op registry exists to protect).

    A GIL-starved-but-progressing mint (STEP import, geometry, upload) burns CPU; a genuinely
    wedged process sits at ~0%. Sample the family's summed kernel+user CPU time twice and see if
    it advanced. Best-effort: any failure (non-Windows, no handle) returns False so the caller
    falls back to its wall-clock rule - never a reason to SKIP a kill on error."""
    try:
        k32 = ctypes.windll.kernel32
        k32.OpenProcess.restype = ctypes.wintypes.HANDLE
        PQLI = 0x1000  # PROCESS_QUERY_LIMITED_INFORMATION

        def _ticks():
            total = 0
            for pid in (_fusion_family_pids() or set()):
                h = k32.OpenProcess(PQLI, False, int(pid))
                if not h:
                    continue
                try:
                    ct, et, kt, ut = _FILETIME(), _FILETIME(), _FILETIME(), _FILETIME()
                    if k32.GetProcessTimes(h, ctypes.byref(ct), ctypes.byref(et),
                                           ctypes.byref(kt), ctypes.byref(ut)):
                        total += (kt.dwLowDateTime | (kt.dwHighDateTime << 32))
                        total += (ut.dwLowDateTime | (ut.dwHighDateTime << 32))
                finally:
                    k32.CloseHandle(h)
            return total

        t0 = _ticks()
        time.sleep(sample_s)
        t1 = _ticks()
        # CPU time is in 100ns ticks; >5ms of CPU burned in the window == actively working.
        return (t1 - t0) > 50000
    except Exception:
        return False


def begin_background_envelope(seconds: float, minimize: bool = False):
    """Guard the next `seconds` against Fusion-family focus theft. Fire-and-forget."""
    global _ENVELOPE_COUNT
    try:
        with _ENVELOPE_LOCK:
            _ENVELOPE_COUNT += 1
            if _ENVELOPE_COUNT == 1:
                _ENVELOPE_SAVED["fg"] = _get_fg_lock_timeout()
                _ENVELOPE_SAVED["anim"] = _get_min_animate()
                _set_fg_lock_timeout(300000)
                _set_min_animate(0)
                # DENY grabs outright, don't just undo them: Fusion self-activates on
                # document/workspace switches and each one FLASHED before the watcher
                # demoted it (John counted three, 2026-08-15). LockSetForegroundWindow
                # blocks SetForegroundWindow globally until unlock; best-effort (Windows
                # only honors it from some process states) - watcher stays as backstop.
                try:
                    _ENVELOPE_SAVED["lsfw"] = bool(ctypes.windll.user32.LockSetForegroundWindow(1))
                except Exception:
                    _ENVELOPE_SAVED["lsfw"] = False
                # Taskbar badge (John, 2026-08-17): while Adom drives Fusion, its taskbar
                # button wears the Adom mark - the user can glance down and know WHY the
                # app is busy. Set on first envelope, cleared when the refcount drains.
                try:
                    from handlers.taskbar_overlay import set_driving_badge
                    hw = _fusion_main_hwnd_quick()
                    if hw:
                        set_driving_badge(hw)
                except Exception:
                    pass
        threading.Thread(target=_envelope_watch, args=(seconds, minimize), daemon=True).start()
    except Exception:
        pass


def _envelope_watch(seconds: float, minimize: bool):
    try:
        user32 = ctypes.windll.user32
        HWND = ctypes.wintypes.HWND
        user32.GetForegroundWindow.restype = HWND
        user32.GetForegroundWindow.argtypes = []
        user32.GetWindowThreadProcessId.argtypes = [HWND, ctypes.POINTER(ctypes.wintypes.DWORD)]
        user32.GetWindowThreadProcessId.restype = ctypes.wintypes.DWORD
        user32.ShowWindowAsync.argtypes = [HWND, ctypes.c_int]
        user32.ShowWindowAsync.restype = ctypes.wintypes.BOOL
        user32.SetWindowPos.argtypes = [HWND, HWND, ctypes.c_int, ctypes.c_int,
                                        ctypes.c_int, ctypes.c_int, ctypes.c_uint]
        user32.SetWindowPos.restype = ctypes.wintypes.BOOL
        HWND_BOTTOM = HWND(1)
        SWP_FLAGS = 0x0001 | 0x0002 | 0x0010  # NOSIZE | NOMOVE | NOACTIVATE
        # NEVER fight the user (John, 2026-08-15: the watcher demoted HIS clicks and he
        # could not bring Fusion up at all): if real keyboard/mouse input happened in the
        # last 2s, the foreground change is HIS - stand down for that beat.
        class _LII(ctypes.Structure):
            _fields_ = [("cbSize", ctypes.wintypes.UINT), ("dwTime", ctypes.wintypes.DWORD)]
        def _last_input_tick():
            try:
                lii = _LII(); lii.cbSize = ctypes.sizeof(_LII)
                if ctypes.windll.user32.GetLastInputInfo(ctypes.byref(lii)):
                    return int(lii.dwTime)
            except Exception:
                pass
            return None
        def _user_active_recently(window_ms=5000):
            t = _last_input_tick()
            if t is None:
                return False
            return (ctypes.windll.kernel32.GetTickCount() - t) < window_ms
        end = time.time() + seconds
        pids, last_refresh = set(), 0.0
        # ⭐ THE USER-IS-WATCHING LATCH (John, 2026-08-23: "if I want to watch it do its
        # thing, I should be allowed to" - he raised Fusion and the watcher shoved it back).
        # The 2s input window is too short for WATCHING: you click Fusion up, then just look,
        # and 2s later this loop demotes it. So when the user RAISES a Fusion window (a
        # non-Fusion -> Fusion foreground transition with recent input), LATCH hands-off and
        # NEVER demote until the user themselves moves focus to a non-Fusion window. This only
        # fights Fusion's OWN self-activation (a transition with no user input behind it).
        _prev_was_fusion = False
        _fg_pid = ctypes.wintypes.DWORD(0)
        _away_since = [None]          # when a non-Fusion window took the front
        _away_input_tick = [None]     # GetLastInputInfo tick at that moment
        # ⭐ FUSION ALREADY IN FRONT WHEN THIS ENVELOPE STARTS IS THE USER'S DOING (John,
        # 2026-08-29: "when you're running your verbs and I click the taskbar icon to
        # bring Fusion to the foreground, you end up pushing it to the background on me").
        # The hole: between two verbs no watcher is alive, the user raises Fusion and
        # just looks at it; the NEXT verb starts a fresh watcher whose first tick sees
        # Fusion in front with no input in the last 2s, the latch is still False from
        # whoever reset it last, and it demotes HIS window. An envelope only exists to
        # catch raises that happen DURING the bridge's own work, so whatever is in front
        # at its start was put there before the work began: yield to it.
        try:
            pids = _fusion_family_pids()
            last_refresh = time.time()
            fg0 = user32.GetForegroundWindow()
            if fg0 and pids:
                user32.GetWindowThreadProcessId(fg0, ctypes.byref(_fg_pid))
                if _fg_pid.value in pids:
                    _USER_WATCHING[0] = True
                    _prev_was_fusion = True
        except Exception:
            pass
        while time.time() < end:
            now = time.time()
            if now - last_refresh > 2.0:
                pids = _fusion_family_pids()
                last_refresh = now
            try:
                fg = user32.GetForegroundWindow()
                fg_is_fusion = False
                if fg and pids:
                    user32.GetWindowThreadProcessId(fg, ctypes.byref(_fg_pid))
                    fg_is_fusion = _fg_pid.value in pids
                if fg_is_fusion:
                    _away_since[0] = None
                    # User deliberately brought Fusion up -> latch and stand down. A taskbar
                    # click IS input, so a raise with input in the last 5s is his.
                    if (not _prev_was_fusion) and _user_active_recently():
                        _USER_WATCHING[0] = True
                    if not _USER_WATCHING[0]:
                        # Fusion self-activated (no user behind this raise) -> demote.
                        if minimize:
                            user32.ShowWindowAsync(fg, _SW_SHOWMINNOACTIVE)
                        else:
                            user32.SetWindowPos(fg, HWND_BOTTOM, 0, 0, 0, 0, SWP_FLAGS)
                elif fg:
                    # Foreground is something else. That is the user MOVING ON only when it
                    # is his doing and it lasts: input since the switch AND the other window
                    # has held the front for 3s. A toast or a tool window flashing in front
                    # for a moment with no input behind it (1.9.274 hole, John 2026-08-29:
                    # "I tried to bring Fusion to the fg manually and you kept sending it
                    # to the bg") must NOT drop the latch, or Fusion's return two seconds
                    # later reads as a self-grab and gets demoted.
                    if _away_since[0] is None:
                        _away_since[0] = now
                        _away_input_tick[0] = _last_input_tick()
                    elif _USER_WATCHING[0] and now - _away_since[0] >= 3.0:
                        t = _last_input_tick()
                        if t is not None and _away_input_tick[0] is not None and t != _away_input_tick[0]:
                            _USER_WATCHING[0] = False
                _prev_was_fusion = fg_is_fusion
            except Exception:
                pass
            time.sleep(0.05)
        # Drain sweep (2026-08-15, caught live by focus_invariance + John watching): a modal
        # (Package3D Save dialog) can pop AT the deadline and keep the foreground it grabbed
        # after the watcher dies. If a family window holds the foreground at drain time, keep
        # demoting for a grace period instead of walking away mid-steal.
        grace_end = time.time() + 20
        while time.time() < grace_end:
            try:
                if _user_active_recently() or _USER_WATCHING[0]:
                    break  # the user is driving/watching - the foreground is THEIRS now
                fg = user32.GetForegroundWindow()
                if not (fg and pids):
                    break
                wpid = ctypes.wintypes.DWORD(0)
                user32.GetWindowThreadProcessId(fg, ctypes.byref(wpid))
                if wpid.value not in pids:
                    break
                user32.SetWindowPos(fg, HWND_BOTTOM, 0, 0, 0, 0, SWP_FLAGS)
            except Exception:
                break
            time.sleep(0.2)
    except Exception:
        pass  # non-Windows / ctypes unavailable: envelope is a no-op, refcount still drains
    finally:
        global _ENVELOPE_COUNT
        with _ENVELOPE_LOCK:
            _ENVELOPE_COUNT -= 1
            if _ENVELOPE_COUNT == 0:
                if _ENVELOPE_SAVED.get("fg") is not None:
                    _set_fg_lock_timeout(_ENVELOPE_SAVED["fg"])
                if _ENVELOPE_SAVED.get("anim") is not None:
                    _set_min_animate(_ENVELOPE_SAVED["anim"])
                if _ENVELOPE_SAVED.get("lsfw"):
                    try:
                        ctypes.windll.user32.LockSetForegroundWindow(2)
                    except Exception:
                        pass
                # v1.9.58: badge is NO LONGER cleared on envelope drain - it sheds on verb
                # IDLE instead (server's janitor, John: "after like 5 or 10 minutes of
                # inactivity, the way nb does it"). Per-verb on/off flicker was wrong.


def _envelope_user_active_recently(window_ms: int = 2000) -> bool:
    """True when real keyboard/mouse input happened within window_ms (GetLastInputInfo)."""
    try:
        class _LII(ctypes.Structure):
            _fields_ = [("cbSize", ctypes.wintypes.UINT), ("dwTime", ctypes.wintypes.DWORD)]
        lii = _LII(); lii.cbSize = ctypes.sizeof(_LII)
        if ctypes.windll.user32.GetLastInputInfo(ctypes.byref(lii)):
            return (ctypes.windll.kernel32.GetTickCount() - lii.dwTime) < window_ms
    except Exception:
        pass
    return False


def _focus_guard(seconds: float = 150.0, restore_fg_lock=None, restore_min_anim=None):
    """For `seconds`, demote any Autodesk-FAMILY window that steals the foreground.

    SURGICAL: only acts when the CURRENT foreground window belongs to an Autodesk
    launch-family process (Fusion / launcher-splash / identity-seat manager) - i.e.
    something just grabbed focus off the user. It minimizes that window without
    activating anything, so focus falls back to whatever the user was in. It does
    NOT touch the family when the user is working in another app, so it never
    fights a user who deliberately foregrounds Fusion themselves. Best-effort;
    never raises."""
    try:
        user32 = ctypes.windll.user32
        # ⚠️ CRITICAL: set restype/argtypes or ctypes defaults every HANDLE to c_int
        # (32-bit) and TRUNCATES it on 64-bit Windows - GetForegroundWindow then hands
        # back a garbage HWND and ShowWindowAsync silently no-ops (the guard fires into
        # the void; the window is never minimized - the exact 1.6.34 failure, 2026-07-06).
        HWND = ctypes.wintypes.HWND
        user32.GetForegroundWindow.restype = HWND
        user32.GetForegroundWindow.argtypes = []
        user32.GetWindowThreadProcessId.argtypes = [HWND, ctypes.POINTER(ctypes.wintypes.DWORD)]
        user32.GetWindowThreadProcessId.restype = ctypes.wintypes.DWORD
        user32.ShowWindowAsync.argtypes = [HWND, ctypes.c_int]
        user32.ShowWindowAsync.restype = ctypes.wintypes.BOOL
    except Exception:
        return
    end = time.time() + seconds
    pids = set()
    last_refresh = 0.0
    while time.time() < end:
        now = time.time()
        if now - last_refresh > 2.0:
            pids = _fusion_family_pids()  # whole family, not just Fusion360.exe
            last_refresh = now
        try:
            fg = user32.GetForegroundWindow()
            if fg and pids:
                wpid = ctypes.wintypes.DWORD(0)
                user32.GetWindowThreadProcessId(fg, ctypes.byref(wpid))
                if wpid.value in pids:
                    # YIELD TO THE USER (John, 2026-08-29): a family window in front with the
                    # user's input behind it (a taskbar click, alt-tab) is his choice, and
                    # the launch guard used to minimize it anyway. Only a raise with no input
                    # in the last 2s is Fusion's own grab.
                    if _USER_WATCHING[0] or _envelope_user_active_recently():
                        _USER_WATCHING[0] = True
                    else:
                        # minimize the focus-stealing family window, no activation
                        user32.ShowWindowAsync(fg, _SW_SHOWMINNOACTIVE)
                elif _USER_WATCHING[0] and _envelope_user_active_recently(3000):
                    _USER_WATCHING[0] = False   # the user moved on (input behind it); guard again
        except Exception:
            pass
        time.sleep(0.1)
    # Launch window over: restore the user's original foreground-lock + minimize
    # animation (we only changed them DURING Fusion's noisy startup).
    if restore_fg_lock is not None:
        _set_fg_lock_timeout(restore_fg_lock)
    if restore_min_anim is not None:
        _set_min_animate(restore_min_anim)


def _start_focus_guard(seconds: float = 150.0, restore_fg_lock=None, restore_min_anim=None):
    """Fire-and-forget the focus guard on a daemon thread. Never raises."""
    try:
        threading.Thread(
            target=_focus_guard, args=(seconds, restore_fg_lock, restore_min_anim),
            daemon=True,
        ).start()
    except Exception:
        pass


def ensure_fusion_running(fusion_info: dict, wait_addin: bool = False) -> dict | None:
    """Launch Fusion 360 if not already running.

    Uses a live process check (not the stale fusion_info["running"] flag).
    Returns None on success, or an error dict on failure.
    If wait_addin is True, also waits for the add-in to be responsive.
    """
    # Live check — ignore stale fusion_info["running"]
    if _has_fusion_process():
        if wait_addin:
            # Fusion is running but add-in might not be ready yet
            if not wait_for_addin(timeout=10):
                return {
                    "success": False,
                    "error": "Fusion 360 is running but AdomBridge add-in not responding.",
                    "errorCode": "fusion_addin_not_responding",
                    "_hint": "Fix it YOURSELF - never ask the user: (1) run install_addin() so the "
                             "add-in lands in EVERY Fusion add-in dir (Fusion moved it to "
                             "%APPDATA%\\Autodesk\\FusionAddins on 2025+ builds; an add-in in the "
                             "legacy API\\AddIns dir is silently ignored - issue #63), then "
                             "(2) restart Fusion via fusion_stop + fusion_start so it rescans.",
                }
        return None  # Already running (and add-in ready if requested)

    exe_path = fusion_info.get("exe_path")
    if not exe_path or not Path(exe_path).exists():
        return {"success": False, "error": "Fusion 360 executable not found"}

    try:
        # PROACTIVE: raise the foreground-lock timeout so Windows DENIES Fusion's
        # splash/main/seat windows the ability to steal focus in the first place (they
        # get a taskbar flash, not a real foreground grab). This is what stops the
        # visible FLICKER the reactive guard alone leaves. Restored by the guard at the
        # end of the launch window.
        _orig_fg_lock = _get_fg_lock_timeout()
        _set_fg_lock_timeout(300000)
        # Disable the minimize ANIMATION so the guard's minimize is an INSTANT vanish
        # (no visible slide-down = no perceptible flash). Restored by the guard.
        _orig_min_anim = _get_min_animate()
        _set_min_animate(0)
        # Launch MINIMIZED + NOT ACTIVATED (STARTUPINFO). DEVNULL on all three std
        # streams + CREATE_NO_WINDOW: the bridge is a detached, console-less child of
        # ab, so inherited handles are invalid.
        _popen_fusion_background(exe_path)
        # Reactive backstop via the refcounted envelope (minimize mode: splash/seat
        # windows vanish rather than being demoted). The envelope restores the user's
        # foreground-lock + animation when the LAST active guard drains, so it composes
        # with per-verb envelopes armed by the dispatcher.
        _set_fg_lock_timeout(_orig_fg_lock)   # envelope snapshots + re-raises itself
        _set_min_animate(_orig_min_anim)
        begin_background_envelope(150.0, minimize=True)
    except Exception as e:
        return {"success": False, "error": f"Failed to launch Fusion 360: {e}"}

    # Wait for Fusion process to appear (up to 30s for launcher to spawn main process)
    for _ in range(60):
        time.sleep(0.5)
        if _has_fusion_process():
            break
    else:
        # LAUNCH FAILED - do NOT return a blind "process not found". READ the dialog that is almost
        # certainly on screen. The classic fresh-box cause is an INCOMPLETE stream: "Error Launching
        # Streamed Application ... FusionLauncher.exe.ini is missing or incomplete" (John, live on a
        # fresh Hyper-V VM 2026-07-14). Detect + dismiss it, CLEAN the corrupt webdeploy dir(s), and
        # tell the caller to re-stream - instead of silently sitting on a broken install.
        launch_err = None
        try:
            from handlers.dialog_classify import classify_launch_dialogs, close_dialog_bg
            for d in classify_launch_dialogs():
                if d.get("category") == "launch_error" or "streamed application" in (d.get("title") or "").lower():
                    launch_err = d
                    close_dialog_bg(d.get("hwnd"))
        except Exception:
            pass

        # DISTINGUISH the two causes (John 2026-07-14 - conflating them mis-blamed a healthy install):
        #  (a) app is COMPLETE (Fusion360.exe present) but launch failed -> almost always we launched a
        #      FusionLauncher.exe WITHOUT its .ini, or a stale lock from a just-killed instance. The
        #      install is FINE; do NOT tell the user to re-stream. _find_fusion_launcher now prefers the
        #      launcher-with-.ini, so a retry usually succeeds.
        #  (b) app is NOT complete -> a genuinely half-streamed install; clean the partial dirs so the
        #      next fusion_install_fusion re-streams clean.
        if _any_app_complete():
            return {
                "success": False,
                "error": "Fusion is INSTALLED (Fusion360.exe present) but did not start within 30s"
                         + (" - a launch-error dialog was dismissed" if launch_err else "") + ".",
                "errorCode": "fusion_launch_failed",
                "installed": True,
                "dialog": (launch_err or {}).get("title"),
                "_hint": "The install is COMPLETE - do NOT re-stream. Retry fusion_start (it now picks the "
                         "FusionLauncher.exe that has its .ini beside it; a stale lock from a just-killed "
                         "instance also clears on a retry). If it keeps failing, fusion_kill then wait a "
                         "few seconds before fusion_start.",
            }
        removed = _clean_incomplete_webdeploy()
        return {
            "success": False,
            "error": "Fusion launch failed: the streamed install is INCOMPLETE (no Fusion360.exe yet).",
            "errorCode": "fusion_incomplete_install",
            "cleanedWebdeployDirs": removed,
            "dialog": (launch_err or {}).get("title"),
            "_hint": "AUTO-RECOVERED: dismissed the launch-error dialog and removed the partial webdeploy "
                     "dir(s). Re-run fusion_install_fusion, then WAIT for fusion_readiness to report "
                     "installed:true BEFORE fusion_start - starting mid-stream is what corrupts the install.",
        }

    # Give it time to finish initializing UI
    time.sleep(5.0)

    if wait_addin:
        # dismiss_dialogs=True: while waiting for the add-in, also check for
        # and dismiss blocking dialogs every 5s. This is critical because
        # startup dialogs (picker, recovery) block the add-in's main thread.
        if not wait_for_addin(timeout=60, dismiss_dialogs=True):
            return {
                "success": False,
                "error": "addin_timeout_after_launch",
                "message": "Fusion 360 launched but the AdomBridge add-in didn't respond "
                           "within 60s. A modal dialog (e.g. Document Recovery, update prompt) "
                           "may be blocking startup. Check Fusion and dismiss any dialogs, "
                           "then retry.",
            }

    return None


if __name__ == "__main__":
    info = detect_fusion()
    print(json.dumps(info, indent=2))
