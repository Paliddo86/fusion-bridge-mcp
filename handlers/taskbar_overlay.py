"""Taskbar overlay badge: "Adom is driving Fusion" (John, 2026-08-17).

pup badges its Chrome taskbar buttons so users can tell an Adom-driven window at a
glance; the same signal belongs on FUSION's button whenever this bridge drives it.

THE LESSON (stolen from pup, who paid for it first - src/server.js): an in-bridge
ITaskbarList3::SetOverlayIcon is IN-PROCESS-ONLY. The COM call returns S_OK
cross-process and the shell silently ignores it (verified live here too: badge never
rendered). The working path is ab-core's `desktop_taskbar` verb (ab >=1.9.106): hand
it the badge PNG as base64 and ab paints the overlay natively, cross-process.

    set:   desktop_taskbar {hwnd, overlay: {icon: <b64 png>, tooltip}}
    clear: desktop_taskbar {hwnd, overlay: {badge: "none"}}

Wired to the background envelope's refcount: first envelope (driving starts) badges the
Fusion main window with resources/adom-overlay-32.png; when the refcount drains to zero
the badge clears. Best-effort everywhere - a missing asset or an ab error must never
break a verb. PROVEN live on AdomLapper 2026-08-17 (red-box capture in fusion-proofs).
"""

import base64
import os
import threading

_LOCK = threading.Lock()
_B64 = None
_BADGED_HWND = None
# Last failure reason, surfaced in fusion_state.drivingBadge.lastError - a badge that silently
# never appears is undebuggable from the outside (ralph, 2026-08-18: everything reported healthy
# while the badge never rendered).
_LAST_ERROR = [None]


def last_error():
    return _LAST_ERROR[0]


def _badge_b64():
    global _B64
    if _B64 is not None:
        return _B64
    p = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                     "resources", "adom-overlay-32.png")
    try:
        _B64 = base64.b64encode(open(p, "rb").read()).decode()
    except Exception:
        _B64 = ""
    return _B64


def _taskbar_via_cli(args: dict) -> bool:
    """Fallback when the in-process direct API is unavailable on this box: shell ab's own CLI.
    Same pattern the bridge already uses elsewhere (_find_adom_desktop_cli) - a capability that
    exists on the machine must never be unreachable just because one transport is down."""
    try:
        import json as _j
        import subprocess as _sp
        import sys as _sys
        _sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
        from server import _find_adom_desktop_cli          # late import: server owns the lookup
        exe = _find_adom_desktop_cli()
        if not exe:
            _LAST_ERROR[0] = "no ab CLI found for the taskbar fallback"
            return False
        cnw = _sp.CREATE_NO_WINDOW if hasattr(_sp, "CREATE_NO_WINDOW") else 0
        out = _sp.run([exe, "desktop_taskbar", _j.dumps(args)], capture_output=True, text=True,
                      timeout=20, creationflags=cnw).stdout
        if "Taskbar updated" in out:
            _LAST_ERROR[0] = None
            return True
        _LAST_ERROR[0] = "CLI fallback reply: %s" % out[:160]
        return False
    except Exception as e:
        _LAST_ERROR[0] = "CLI fallback exception: %s" % str(e)[:160]
        return False


def _taskbar(args: dict) -> bool:
    try:
        import ab_client
        if not ab_client.available():
            _LAST_ERROR[0] = "ab_client unavailable - trying the ab CLI fallback"
            return _taskbar_via_cli(args)
        r = ab_client.call("desktop_taskbar", args, timeout=15)
        raw = str(r)
        if not r:
            _LAST_ERROR[0] = "desktop_taskbar returned nothing (timeout?)"
            return False
        if "Taskbar updated" in raw:
            _LAST_ERROR[0] = None
            return True
        if isinstance(r, dict) and r.get("error"):
            _LAST_ERROR[0] = "desktop_taskbar error: %s" % str(r.get("error"))[:160]
            return _taskbar_via_cli(args)      # one honest retry down the other transport
        _LAST_ERROR[0] = "unrecognized desktop_taskbar reply: %s" % raw[:160]
        return False
    except Exception as e:
        _LAST_ERROR[0] = "exception: %s" % str(e)[:160]
        return False


_LAST_SET = [0.0]


def set_driving_badge(hwnd) -> bool:
    """Badge the window's taskbar button with the Adom mark ('Adom is driving Fusion').
    Throttled: re-sending the same badge to the same hwnd within 60s is a no-op, so
    per-verb callers do not spam desktop_taskbar."""
    global _BADGED_HWND
    if not hwnd:
        return False
    import time
    if _BADGED_HWND == int(hwnd) and (time.time() - _LAST_SET[0]) < 60:
        return True
    icon = _badge_b64()
    if not icon:
        _LAST_ERROR[0] = "badge asset missing or unreadable (resources/adom-overlay-32.png)"
        return False
    with _LOCK:
        ok = _taskbar({"hwnd": int(hwnd),
                       "overlay": {"icon": icon, "tooltip": "Adom is driving Fusion"}})
        if ok:
            _BADGED_HWND = int(hwnd)
            _LAST_SET[0] = time.time()
        return ok


def badge_active() -> bool:
    return _BADGED_HWND is not None


def clear_driving_badge(hwnd=None) -> bool:
    """Remove the badge (from the given hwnd, or whichever window we badged last)."""
    global _BADGED_HWND
    with _LOCK:
        target = int(hwnd) if hwnd else _BADGED_HWND
        if not target:
            return False
        ok = _taskbar({"hwnd": target, "overlay": {"badge": "none"}})
        if ok and (hwnd is None or _BADGED_HWND == int(hwnd)):
            _BADGED_HWND = None
        return ok


# ── Activity indicator: taskbar PROGRESS driven by measured verb durations ──────────────
# John's experiment (2026-08-21): instead of flashing the Fusion taskbar button to say
# "Adom is doing something", draw a real PROGRESS BAR for the duration the verb is actually
# expected to take, and clear it the moment the verb finishes or errors.
#
# The estimate is not invented: timings.py keeps a rolling measured history per verb on THIS
# machine, so the bar reflects what this box actually does. Before a verb has any history the
# bar runs INDETERMINATE (a marquee) rather than pretending to a percentage - the same
# honesty rule as the progress contract: never dress a guess as a measurement.

_PROGRESS_STOP = {}          # verb -> threading.Event, so a finished verb can stop its ticker


def _taskbar_progress(state: str, value=None) -> bool:
    """Paint (or clear) the progress bar on Fusion's own taskbar button."""
    try:
        from handlers.fusion_ui import _find_fusion_hwnd
        hwnd = _find_fusion_hwnd()
        if not hwnd:
            return False
        prog = {"state": state}
        if value is not None:
            prog["value"] = max(0, min(100, int(value)))
        return _taskbar({"hwnd": int(hwnd), "progress": prog})
    except Exception:
        return False


def start_activity_progress(verb: str, estimate_s: float, confidence: str = "typical"):
    """Begin a progress bar for `verb`, paced by its measured estimate. Returns a stopper."""
    import threading
    import time as _t
    stop = threading.Event()
    _PROGRESS_STOP[verb] = stop

    def _tick():
        started = _t.time()
        # No history yet -> marquee. A percentage we cannot justify is worse than no number.
        if not estimate_s or confidence == "unknown":
            _taskbar_progress("indeterminate")
            while not stop.wait(1.0):
                pass
            return
        fails = 0
        while not stop.wait(0.7):
            elapsed = _t.time() - started
            # 0 -> 95 across the estimate, then hold: the run is not done until it says so,
            # and a bar that hits 100 while work continues is the lie we already fixed once.
            pct = min(95, int(95 * elapsed / estimate_s)) if estimate_s > 0 else 0
            if _taskbar_progress("normal", pct):
                fails = 0
            else:
                # ⚠️ DO NOT GIVE UP ON THE FIRST MISS (John, 2026-08-21: show the bar DURING
                # fusion_start). The bar lives on Fusion's own taskbar button, and during a
                # cold launch that window does not exist yet - so the old first-paint-fails-
                # then-return meant the bar never appeared for the one slow operation that
                # most needed it. Keep trying; the hwnd shows up mid-launch.
                fails += 1
                if fails > 120:               # ~90s of no window at all - genuinely hopeless
                    return
    threading.Thread(target=_tick, daemon=True).start()
    return stop


def end_activity_progress(verb: str, failed: bool = False):
    """Stop the bar. On failure flash red briefly so the outcome is visible, then clear."""
    import time as _t
    stop = _PROGRESS_STOP.pop(verb, None)
    if stop:
        stop.set()
    try:
        if failed:
            _taskbar_progress("error", 100)
            _t.sleep(1.2)
        _taskbar_progress("none")
        # Also clear WINDOWS' OWN attention flash (John, 2026-08-21: "can u not orange
        # flash the windows taskbar for fusion?"). In progress mode the bridge never
        # raises a flash - but Windows flashes Fusion's button by itself whenever a
        # background Fusion raises a window while being driven. The user chose bars,
        # so ending a verb also cancels any accumulated native flash.
        try:
            from handlers.fusion_ui import _find_fusion_hwnd
            hwnd = _find_fusion_hwnd()
            if hwnd:
                _taskbar({"hwnd": int(hwnd), "flash": "stop"})
        except Exception:
            pass
    except Exception:
        pass
