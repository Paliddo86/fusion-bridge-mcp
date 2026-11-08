#!/usr/bin/env python3
"""Fusion 360 helper utilities — start, stop, status, force-kill.

Provides reliable OS-level management of Fusion 360 processes on Windows.
Works independently of the bridge server or add-in.

Usage:
    python fusion_helper.py status      # Show Fusion process status
    python fusion_helper.py start       # Launch Fusion 360
    python fusion_helper.py stop        # Graceful stop (WM_CLOSE)
    python fusion_helper.py kill        # Force kill all Fusion processes
    python fusion_helper.py restart     # Kill + start
    python fusion_helper.py addin       # Check if add-in is responding
"""

import ctypes
import ctypes.wintypes
import json
import subprocess
import sys
import time
import urllib.request
import urllib.error
from pathlib import Path

# Import fusion_detect from same directory
sys.path.insert(0, str(Path(__file__).parent))
from fusion_detect import detect_fusion

ADDIN_PORT = 8774
BRIDGE_PORT = 8773

# Win32 constants
WM_CLOSE = 0x0010
PROCESS_TERMINATE = 0x0001
PROCESS_QUERY_INFORMATION = 0x0400

EnumWindows = ctypes.windll.user32.EnumWindows
GetWindowTextW = ctypes.windll.user32.GetWindowTextW
GetWindowTextLengthW = ctypes.windll.user32.GetWindowTextLengthW
PostMessageW = ctypes.windll.user32.PostMessageW
IsWindowVisible = ctypes.windll.user32.IsWindowVisible
GetWindowThreadProcessId = ctypes.windll.user32.GetWindowThreadProcessId

WNDENUMPROC = ctypes.WINFUNCTYPE(ctypes.wintypes.BOOL, ctypes.wintypes.HWND, ctypes.wintypes.LPARAM)



# Console tools (tasklist / taskkill) spawned WITHOUT this flag flash a visible terminal
# window on the user's screen for ~50ms - and some of these run on POLLS, so the user sees
# rhythmic blips while "nothing" is happening (John, 2026-09-01: "really annoying to the
# human"). Every console-tool spawn in this codebase must carry it.
_NO_WINDOW = subprocess.CREATE_NO_WINDOW if hasattr(subprocess, "CREATE_NO_WINDOW") else 0

def _get_fusion_processes() -> list[dict]:
    """Get all running Fusion-related processes using tasklist."""
    result = []
    try:
        output = subprocess.check_output(
            ["tasklist", "/FO", "CSV", "/NH"],
            stderr=subprocess.DEVNULL,
            text=True,
            creationflags=_NO_WINDOW,
        )
        fusion_names = {"Fusion360.exe", "FusionLauncher.exe", "Fusion.exe",
                        "FusionService.exe", "FusionCEF.exe", "Fusion360Helper.exe"}
        for line in output.strip().split("\n"):
            line = line.strip()
            if not line:
                continue
            # CSV format: "name","pid","session","session#","mem"
            parts = line.split('","')
            if len(parts) >= 2:
                name = parts[0].strip('"')
                pid = parts[1].strip('"')
                if name in fusion_names:
                    result.append({"name": name, "pid": int(pid)})
    except Exception as e:
        print(f"  Error listing processes: {e}")
    return result


def _find_fusion_windows() -> list[dict]:
    """Find all visible Fusion 360 windows.

    Matches windows whose title starts with 'Autodesk Fusion' (the actual Fusion app)
    rather than browser tabs that happen to mention it.
    """
    windows = []

    def enum_callback(hwnd, _):
        if IsWindowVisible(hwnd):
            length = GetWindowTextLengthW(hwnd)
            if length > 0:
                buf = ctypes.create_unicode_buffer(length + 1)
                GetWindowTextW(hwnd, buf, length + 1)
                title = buf.value
                # Fusion windows typically start with "Autodesk Fusion" or the design name
                # followed by "- Autodesk Fusion". We also check the process name.
                if "Autodesk Fusion" in title:
                    pid = ctypes.wintypes.DWORD()
                    GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
                    # Verify it's actually a Fusion process, not a browser
                    if _is_fusion_pid(pid.value):
                        windows.append({
                            "hwnd": hwnd,
                            "title": title,
                            "pid": pid.value,
                        })
        return True

    EnumWindows(WNDENUMPROC(enum_callback), 0)
    return windows


def _is_fusion_pid(pid: int) -> bool:
    """Check if a PID belongs to a Fusion 360 process."""
    try:
        output = subprocess.check_output(
            ["tasklist", "/FI", f"PID eq {pid}", "/FO", "CSV", "/NH"],
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=5,
            creationflags=_NO_WINDOW,
        )
        fusion_names = {"Fusion360.exe", "FusionLauncher.exe", "Fusion.exe"}
        for name in fusion_names:
            if name in output:
                return True
    except Exception:
        pass
    return False


def _close_fusion_window(hwnd) -> bool:
    """Send WM_CLOSE to a Fusion window for graceful shutdown."""
    return PostMessageW(hwnd, WM_CLOSE, 0, 0) != 0


def _force_kill_pid(pid: int):
    """Force kill a process by PID."""
    try:
        subprocess.run(["taskkill", "/F", "/PID", str(pid)],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       creationflags=_NO_WINDOW)
    except Exception:
        pass


def _check_port(port: int, timeout: float = 2.0) -> dict | None:
    """Check if an HTTP server is responding on a port."""
    try:
        req = urllib.request.Request(f"http://127.0.0.1:{port}/health", method="GET")
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.loads(resp.read())
    except Exception:
        return None


def cmd_status():
    """Show comprehensive Fusion 360 status."""
    print("=" * 60)
    print("FUSION 360 STATUS")
    print("=" * 60)

    # Detection
    info = detect_fusion()
    print(f"\n[Detection]")
    print(f"  Installed:     {info.get('installed', False)}")
    if info.get("installed"):
        print(f"  Exe path:      {info.get('exe_path')}")
        print(f"  Add-in files:  {'installed' if info.get('addin_installed') else 'NOT installed'}")
    else:
        print(f"  WARNING: Fusion 360 not found!")

    # Processes
    procs = _get_fusion_processes()
    print(f"\n[Processes] ({len(procs)} Fusion processes)")
    for p in procs:
        print(f"  PID {p['pid']:>8}  {p['name']}")

    # Windows
    windows = _find_fusion_windows()
    print(f"\n[Windows] ({len(windows)} Fusion windows)")
    for w in windows:
        title = w['title'].encode('ascii', 'replace').decode('ascii')
        print(f"  HWND {w['hwnd']}  PID {w['pid']}  \"{title}\"")

    # Bridge server
    print(f"\n[Bridge Server] (port {BRIDGE_PORT})")
    bridge = _check_port(BRIDGE_PORT)
    if bridge:
        print(f"  Status: RUNNING")
        addin_info = bridge.get("addin")
        if addin_info:
            print(f"  Add-in via bridge: CONNECTED")
        else:
            print(f"  Add-in via bridge: NOT CONNECTED")
    else:
        print(f"  Status: NOT RUNNING")

    # Direct add-in check
    print(f"\n[Add-In Server] (port {ADDIN_PORT})")
    addin = _check_port(ADDIN_PORT)
    if addin:
        print(f"  Status: RUNNING — {addin}")
    else:
        print(f"  Status: NOT RUNNING")

    # Summary
    running = len(procs) > 0
    has_window = len(windows) > 0
    addin_up = addin is not None

    print(f"\n[Summary]")
    print(f"  Fusion running:  {'YES' if running else 'NO'}")
    print(f"  UI visible:      {'YES' if has_window else 'NO'}")
    print(f"  Add-in active:   {'YES' if addin_up else 'NO'}")
    print(f"  Bridge running:  {'YES' if bridge else 'NO'}")
    print("=" * 60)

    return {"running": running, "window": has_window, "addin": addin_up, "bridge": bridge is not None}


def cmd_start(wait_for_addin: bool = True, timeout: int = 120):
    """Launch Fusion 360 and optionally wait for the add-in."""
    info = detect_fusion()
    if not info.get("installed"):
        print("ERROR: Fusion 360 not found!")
        return False

    # Check if already running
    procs = _get_fusion_processes()
    if procs:
        print(f"Fusion 360 already running ({len(procs)} processes)")
        windows = _find_fusion_windows()
        if windows:
            print(f"  Window: \"{windows[0]['title']}\"")
    else:
        exe_path = info["exe_path"]
        print(f"Launching: {exe_path}")
        try:
            # Fusion itself SHOULD show a window; _NO_WINDOW only suppresses the
            # console that would otherwise ride along with the spawn.
            subprocess.Popen(
                [exe_path],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                creationflags=_NO_WINDOW,
            )
        except Exception as e:
            print(f"ERROR: Failed to launch: {e}")
            return False

        # Wait for window
        print("Waiting for Fusion window...", end="", flush=True)
        for i in range(timeout):
            time.sleep(1)
            windows = _find_fusion_windows()
            if windows:
                print(f" found after {i+1}s")
                print(f"  Window: \"{windows[0]['title']}\"")
                break
            if i % 10 == 9:
                print(".", end="", flush=True)
        else:
            print(f" TIMEOUT after {timeout}s")
            return False

    if wait_for_addin:
        print("Waiting for add-in on port 8774...", end="", flush=True)
        for i in range(timeout):
            time.sleep(1)
            addin = _check_port(ADDIN_PORT, timeout=1.0)
            if addin:
                print(f" ready after {i+1}s")
                return True
            if i % 10 == 9:
                print(".", end="", flush=True)
        print(f" TIMEOUT — add-in not responding after {timeout}s")
        print("  You may need to enable it: UTILITIES > ADD-INS > AdomBridge > Run on Startup + Run")
        return False

    return True


def cmd_stop():
    """Graceful stop — send WM_CLOSE to Fusion windows."""
    windows = _find_fusion_windows()
    if not windows:
        print("No Fusion windows found.")
        procs = _get_fusion_processes()
        if procs:
            print(f"  But {len(procs)} Fusion processes are running. Use 'kill' to force-stop.")
        return

    for w in windows:
        print(f"Closing: \"{w['title']}\" (PID {w['pid']})")
        _close_fusion_window(w["hwnd"])

    # Wait for processes to exit
    print("Waiting for shutdown...", end="", flush=True)
    for i in range(30):
        time.sleep(1)
        procs = _get_fusion_processes()
        if not procs:
            print(f" done after {i+1}s")
            return
        if i % 5 == 4:
            print(".", end="", flush=True)

    remaining = _get_fusion_processes()
    print(f"\n  {len(remaining)} processes still running. Use 'kill' to force-stop.")


def cmd_kill():
    """Force kill ALL Fusion processes."""
    procs = _get_fusion_processes()
    if not procs:
        print("No Fusion processes running.")
        return

    print(f"Force-killing {len(procs)} Fusion processes...")
    pids = set()
    for p in procs:
        pids.add(p["pid"])
        print(f"  Killing PID {p['pid']} ({p['name']})")

    for pid in pids:
        _force_kill_pid(pid)

    time.sleep(1)
    remaining = _get_fusion_processes()
    if remaining:
        print(f"  WARNING: {len(remaining)} processes still running")
    else:
        print("  All Fusion processes stopped.")


def cmd_restart(wait_for_addin: bool = True):
    """Kill Fusion and restart it."""
    print("=== Stopping Fusion 360 ===")
    cmd_kill()
    time.sleep(2)
    print("\n=== Starting Fusion 360 ===")
    return cmd_start(wait_for_addin=wait_for_addin)


def cmd_addin():
    """Check add-in status and attempt to reach it."""
    addin = _check_port(ADDIN_PORT)
    if addin:
        print(f"Add-in is running: {json.dumps(addin)}")
        return True
    else:
        print("Add-in is NOT responding on port 8774.")
        procs = _get_fusion_processes()
        if not procs:
            print("  Fusion is not running. Use 'start' to launch it.")
        else:
            print(f"  Fusion is running ({len(procs)} processes) but add-in is not active.")
            print("  Try: UTILITIES tab > ADD-INS > AdomBridge > check 'Run on Startup' > click 'Run'")
        return False


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(0)

    cmd = sys.argv[1].lower()

    if cmd == "status":
        cmd_status()
    elif cmd == "start":
        success = cmd_start()
        sys.exit(0 if success else 1)
    elif cmd == "stop":
        cmd_stop()
    elif cmd == "kill":
        cmd_kill()
    elif cmd == "restart":
        success = cmd_restart()
        sys.exit(0 if success else 1)
    elif cmd == "addin":
        success = cmd_addin()
        sys.exit(0 if success else 1)
    else:
        print(f"Unknown command: {cmd}")
        print(__doc__)
        sys.exit(1)


if __name__ == "__main__":
    main()
