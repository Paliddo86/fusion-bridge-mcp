"""Background HTTP server for the Adom Bridge add-in.

Runs on a daemon thread inside Fusion 360's process.
Incoming POST /command requests fire a CustomEvent to marshal execution
to the main Fusion thread, then wait for the result.

Threading model:
    - HTTP server is ThreadingHTTPServer: multiple HTTP threads can serve
      requests concurrently (for /health, status, etc.).
    - Fusion's main thread is a HARD SINGLETON. All Fusion API work is
      marshaled via CustomEvent and executed serially there.
    - A module-level _main_thread_lock serializes CustomEvent fires so that
      only one request has work pending on the main thread at any time.
      New HTTP threads block on the lock until the in-flight request
      completes. This prevents request stacking (which wastes main-thread
      cycles on exports whose HTTP waiter already gave up).
    - A staleness check in set_result() drops results whose HTTP waiter has
      already timed out. The CustomEventHandler also checks staleness
      BEFORE dispatching, so it can skip heavy work entirely.
"""

import json
import os
import threading
import time
import uuid
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler

import adsk.core

from .commands.cloud_documents import get_walk_progress

PORT = 8774

# Add-in version — read from the sibling manifest so /health + /status can
# REPORT it. The bridge compares this against the version it bundles to catch a
# STALE add-in (issue #55: an old Roaming add-in that never re-synced from the
# bridge cache, which made fusion_aps_open fail SILENTLY). Sourced from the
# manifest so a single bump is the source of truth (no hardcode to drift).
def _read_addin_version():
    try:
        mf = os.path.join(os.path.dirname(__file__), "AdomBridge.manifest")
        with open(mf, "r", encoding="utf-8") as f:
            return (json.load(f) or {}).get("version", "unknown")
    except Exception:
        return "unknown"

ADDIN_VERSION = _read_addin_version()

_server = None
_server_thread = None
_custom_event_id = None
_app = None

# Thread-safe result passing: request_id -> (Event, result_dict)
_result_events = {}
_result_data = {}
_lock = threading.Lock()

# Main thread singleton lock — only ONE request may have a CustomEvent
# in flight to Fusion's main thread at any time. HTTP threads block
# here to prevent request stacking.
_main_thread_lock = threading.Lock()

# Main thread liveness tracking
_last_main_thread_ts = 0.0  # time.time() of last successful set_result()
_pending_request_ids = set()  # request_ids currently waiting for main thread

# Busy state tracking — which command currently holds the main thread lock.
# Read by the /status endpoint (non-blocking) so ANY bridge can check if
# the add-in is busy, even cross-process. This is the single source of truth.
_busy_state_lock = threading.Lock()
_busy_state = None  # None or {"command": str, "started_at": float, "request_id": str}


def _set_busy(command: str, request_id: str):
    with _busy_state_lock:
        global _busy_state
        _busy_state = {"command": command, "started_at": time.time(), "request_id": request_id}


def _clear_busy():
    with _busy_state_lock:
        global _busy_state
        _busy_state = None


def _get_busy() -> dict | None:
    with _busy_state_lock:
        return dict(_busy_state) if _busy_state else None


# ── Stale-lock watchdog (v1.0.2) ──────────────────────────────────────
#
# Failure mode being defended against:
#   Fusion's native cloud SDK can wedge on a slow / failed cloud API call
#   while holding the Python GIL. While wedged, Python code in the addin
#   does NOT run — including the per-command timeout in _wait_for_result.
#   The HTTP thread eventually times out (after `timeout` seconds) and
#   returns to the client, but the `finally` block can't run because the
#   thread is blocked at the C++/Fusion boundary. Net result:
#     - _main_thread_lock stays held forever
#     - _busy_state stays populated forever
#     - Every new POST /command request blocks on _main_thread_lock.acquire()
#       for its `timeout` seconds, then returns main_thread_busy
#     - From the user's perspective, the addin is dead until Fusion restarts
#
# Defense: a daemon thread checks _busy_state every 5s. If a busy state
# has been active longer than the per-command timeout + WATCHDOG_GRACE,
# it raises _poisoned_until_idle so new POSTs are refused immediately with
# "addin_recovering" instead of piling up on the lock. The flag drops as soon
# as set_result() sees the in-flight handler return.
# It does NOT release _main_thread_lock any more: a release from a thread that
# does not own the lock let the next command be delivered NESTED inside the
# wedged handler's adsk.doEvents() (see _dispatch_guard in AdomBridge.py).

WATCHDOG_GRACE = 30  # seconds beyond per-command timeout before the recovery flag goes up
_watchdog_stop = threading.Event()
_watchdog_thread = None

# ── Recovery flag (replaces the cross-thread force-release) ───────────
#
# The watchdog used to call _main_thread_lock.release() from ITS OWN thread, and
# threading.Lock does not track an owner. That was the third leg of the nested-dispatch bug
# (see AdomBridge.py's _dispatch_guard for the native stack): a new POST acquired a lock the
# wedged handler still held, fired its event, and the wedged handler's adsk.doEvents() then
# delivered that command NESTED inside itself.
#
# Now the watchdog only raises a flag. While it is up, do_POST refuses new commands with
# errorCode "addin_recovering" (fast, honest answer instead of a 300 s hang) and the lock is
# left to the handler that owns it. The flag is cleared in set_result(), i.e. the moment the
# command that was in flight actually returns.
_poison_lock = threading.Lock()
_poisoned_until_idle = False


def _watchdog_note(text):
    """Append a watchdog line to the add-in's own log file (never Fusion's API from this thread)."""
    try:
        import os, time as _t
        p = os.path.join(os.path.expanduser("~"), ".adom", "fusion-bridge", "addin-watchdog.log")
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "a", encoding="utf-8") as f:
            f.write(_t.strftime("%Y-%m-%dT%H:%M:%S ") + text + "\n")
    except Exception:
        pass


def _is_poisoned() -> bool:
    with _poison_lock:
        return _poisoned_until_idle


def _set_poisoned():
    global _poisoned_until_idle
    with _poison_lock:
        _poisoned_until_idle = True


def _clear_poisoned():
    global _poisoned_until_idle
    with _poison_lock:
        _poisoned_until_idle = False


def _watchdog_loop():
    """Raise the recovery flag if a command has been busy far past its own timeout."""
    while not _watchdog_stop.is_set():
        try:
            busy = _get_busy()
            if busy:
                cmd = busy.get("command", "")
                started = busy.get("started_at", 0.0)
                elapsed = time.time() - started
                # Get the timeout that applied to this command
                cmd_timeout = PER_COMMAND_TIMEOUT.get(cmd, COMMAND_TIMEOUT)
                if elapsed > cmd_timeout + WATCHDOG_GRACE:
                    try:
                        # Not a Fusion API call: this runs on the watchdog thread, and the add-in
                        # keeps every API call on the main thread (issue #76 audit, 2026-09-06).
                        _watchdog_note(
                            f"AdomBridge: WATCHDOG marked the add-in recovering — "
                            f"'{cmd}' held for {elapsed:.1f}s "
                            f"(> {cmd_timeout}+{WATCHDOG_GRACE}s grace). "
                            f"Fusion main thread likely wedged on native cloud call. "
                            f"New commands are refused with addin_recovering until this "
                            f"one returns; the lock is NOT force-released, because a "
                            f"cross-thread release let the next command be delivered "
                            f"NESTED inside this one (see _dispatch_guard in AdomBridge.py)."
                        )
                    except Exception:
                        pass
                    # Raise the flag ONLY (2026-09): no _main_thread_lock.release() from a
                    # thread that does not own it, no _clear_busy(), no dropping of
                    # _pending_request_ids. Those three told the rest of the server "the main
                    # thread is free" while it was not - and set_result() is now the signal
                    # that ends the recovery window.
                    _set_poisoned()
        except Exception:
            pass  # watchdog must never die
        _watchdog_stop.wait(5)  # check every 5s


# Default per-command timeout. Heavy 3D exports (STEP/IGES/SAT/STL on
# panelized boards with 100+ placements) can take 120s+. Set generously.
COMMAND_TIMEOUT = 300  # seconds

# Per-command timeout overrides. Fast commands keep short timeouts so a
# hung main thread is surfaced quickly.
PER_COMMAND_TIMEOUT = {
    "get_app_state": 10,
    "document_info": 15,
    "take_screenshot": 30,
    "activate_document": 30,
    "close_document": 30,
    "close_all_documents": 60,
    # Exports can be slow on heavy designs
    "export_step": 300,
    "export_iges": 300,
    "export_sat": 300,
    "export_stl": 300,
    "export_3mf": 300,
    "export_usdz": 300,
    "export_obj": 300,
    "export_f3d": 300,
    "export_fbx": 300,
    "export_dxf": 120,
    "export_dwg": 120,
    "export_skp": 300,
    # Manufacturing exports (run EAGLE ULPs, can be slow)
    "export_gerbers": 180,
    "export_bom": 60,
    "export_cpl": 60,
    "export_board_image": 60,
    # Cloud tree walking / search — BFS over hundreds of folders
    "walk_cloud_tree": 600,
    "search_cloud_files": 180,
    # Electronics view switches and EAGLE/EDA orchestrations that used to fall through to the
    # 300 s default, so a hung one kept /health reporting "blocked" for five minutes.
    "board_stackup": 90,
    "show_2d_board": 120,
    "run_modeling_script": 180,
    "netlist": 150,
    "run_drc": 180,
    "routing_state": 120,
    "build_board": 180,
    "export_eagle_source": 150,
    "list_cloud_projects": 200,
}


def is_request_alive(request_id: str) -> bool:
    """Check whether an HTTP thread is still waiting for this request.

    Called from the main thread (CustomEventHandler) BEFORE doing heavy
    work, so we can skip exports whose HTTP waiter has already bailed.
    """
    with _lock:
        return request_id in _result_events


def set_result(request_id: str, result: dict):
    """Called from the main thread (CustomEventHandler) to deliver a result."""
    # A handler returned, so the in-flight command is finally over: end the recovery window
    # the watchdog opened in _watchdog_loop. This is the ONLY place the flag drops, which is
    # what makes "refuse new commands" bound to "the old handler is really done" instead of
    # to a timer.
    _clear_poisoned()
    with _lock:
        _last_main_thread_ts_local = time.time()
        # Update module-level timestamp
        global _last_main_thread_ts
        _last_main_thread_ts = _last_main_thread_ts_local

        _pending_request_ids.discard(request_id)

        event = _result_events.get(request_id)
        if event:
            # Normal path: HTTP thread is still waiting
            _result_data[request_id] = result
            event.set()
        # else: HTTP thread already timed out — discard result (don't leak)


def _thread_name_histogram(limit: int = 8) -> dict:
    """Live Python thread names, grouped, most common first (diagnostic only)."""
    try:
        counts = {}
        for t in threading.enumerate():
            n = (t.name or "?").split("-")[0]
            counts[n] = counts.get(n, 0) + 1
        return dict(sorted(counts.items(), key=lambda kv: -kv[1])[:limit])
    except Exception:
        return {}


def _wait_for_result(request_id: str, timeout: float) -> dict:
    """Wait for the main thread to deliver a result for this request."""
    event = threading.Event()
    with _lock:
        _result_events[request_id] = event

    got_result = event.wait(timeout=timeout)

    with _lock:
        _result_events.pop(request_id, None)
        result = _result_data.pop(request_id, None)
        if not got_result:
            # Timed out — remove from pending so set_result() won't leak
            _pending_request_ids.discard(request_id)

    if not got_result or result is None:
        return {"success": False, "error": f"Command timed out after {timeout}s"}

    return result


class AdomBridgeHandler(BaseHTTPRequestHandler):
    """HTTP handler for the add-in server."""

    def do_GET(self):
        if self.path == "/health":
            with _lock:
                pending = len(_pending_request_ids)
                last_ts = _last_main_thread_ts
            main_lock_held = _main_thread_lock.locked()

            # The threshold is the timeout that applied to the command ACTUALLY holding the
            # lock, not the 300 s default: a get_app_state that ran 12 s is already past its
            # 10 s timeout, so reporting "blocked" for the next five minutes lied.
            busy = _get_busy()
            busy_timeout = PER_COMMAND_TIMEOUT.get(busy["command"], COMMAND_TIMEOUT) if busy else COMMAND_TIMEOUT

            # Determine main thread status
            now = time.time()
            if last_ts == 0.0 and pending == 0:
                main_status = "idle"  # No commands processed yet, none pending
            elif main_lock_held and pending > 0 and (last_ts == 0.0 or (now - last_ts) > busy_timeout):
                main_status = "blocked"  # Commands pending but main thread not responding
            elif main_lock_held:
                main_status = "busy"  # A command is running on main thread
            else:
                main_status = "responsive"

            self._respond(200, {
                "status": "ok",
                "addin": "AdomBridge",
                "version": ADDIN_VERSION,
                "port": PORT,
                # THREAD DIAGNOSTICS (2026-08-30): Fusion reached 21,900 OS threads after 30
                # hours of driving and became unusable. This tells whether the Python side
                # (this server's request threads) is the leak or Fusion itself is.
                "pyThreads": threading.active_count(),
                "pyThreadNames": _thread_name_histogram(),
                "main_thread": main_status,
                "main_thread_locked": main_lock_held,
                "pending_commands": pending,
                "main_thread_last_seen": round(last_ts, 1) if last_ts > 0 else None,
                "walk_progress": get_walk_progress(),
            })
        elif self.path == "/status":
            # Non-blocking busy probe — does NOT acquire _main_thread_lock.
            # Any number of callers (from any bridge/container) can hit this
            # concurrently. Returns which command holds the lock, how long
            # it's been running, and walk progress if applicable.
            busy = _get_busy()
            walk = get_walk_progress()
            if busy:
                elapsed = round(time.time() - busy["started_at"], 1)
                resp = {
                    "busy": True,
                    "version": ADDIN_VERSION,
                    "pyThreads": threading.active_count(),
                    "busyCommand": busy["command"],
                    "elapsedSeconds": elapsed,
                    "requestId": busy["request_id"],
                }
                if walk:
                    resp["walkProgress"] = walk
                elif busy["command"] in ("walk_cloud_tree", "search_cloud_files") and elapsed > 5:
                    # Walk/search was dispatched (HTTP thread set _busy_state) but
                    # the main thread hasn't started the handler yet (_walk_progress
                    # is still None). This means the custom event is queued but the
                    # main thread's event loop isn't processing it — typically
                    # because a modal dialog has its own nested event loop.
                    resp["mainThreadStalled"] = True
                    resp["stalledHint"] = (
                        "Command dispatched but main thread has not started processing. "
                        "A modal dialog is likely blocking Fusion's event loop. "
                        "Dismiss dialogs via fusion_dismiss_blocking_dialogs, "
                        "then the command will auto-resume."
                    )
                self._respond(200, resp)
            else:
                self._respond(200, {"busy": False, "version": ADDIN_VERSION,
                                    "pyThreads": threading.active_count()})
        else:
            self._respond(404, {"error": "Not found"})

    def do_POST(self):
        if self.path != "/command":
            self._respond(404, {"error": "Not found"})
            return

        content_length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(content_length)

        try:
            request = json.loads(body)
        except json.JSONDecodeError as e:
            self._respond(400, {"success": False, "error": f"Invalid JSON: {e}"})
            return

        command = request.get("command", "")
        args = request.get("args", {})
        request_id = str(uuid.uuid4())

        # Per-command timeout
        timeout = PER_COMMAND_TIMEOUT.get(command, COMMAND_TIMEOUT)
        # Allow caller override (useful for very large models)
        caller_timeout = request.get("timeout")
        if isinstance(caller_timeout, (int, float)) and caller_timeout > 0:
            timeout = float(caller_timeout)

        # Recovery window: the watchdog saw a command overrun its own timeout + grace, so the
        # main thread is still inside that handler (Fusion's native layer wedges the session
        # and the handler cannot unwind). Refuse fast and honestly instead of firing an event
        # that would sit behind the wedged command and then be delivered NESTED inside it by
        # the wedged handler's adsk.doEvents(). The window closes on its own in set_result(),
        # when that handler returns.
        if _is_poisoned():
            busy = _get_busy()
            elapsed = round(time.time() - busy["started_at"], 1) if busy else None
            self._respond(200, {
                "success": False,
                "error": (
                    f"Add-in recovering — {busy['command']} overran its timeout and has not returned "
                    f"({elapsed}s)."
                    if busy else
                    "Add-in recovering — previous command overran its timeout and has not returned."
                ),
                "errorCode": "addin_recovering",
                "busyCommand": busy["command"] if busy else None,
                "elapsedSeconds": elapsed,
                "_hint": (
                    "The add-in is refusing new add-in commands until the one in flight returns, "
                    "so it cannot hand a command to Fusion while its main thread is still busy. "
                    "Do NOT retry in a loop; retry once after a few seconds, and if it is still "
                    "recovering, restart the add-in (fusion_restart_for_addin {save:true}) — that "
                    "is the only thing that clears a native wedge. Commands that still work: "
                    "fusion_state, fusion_screenshot_fusion, fusion_close_window."
                ),
            })
            return

        # Serialize CustomEvent fires through the main-thread lock so
        # requests don't stack. If another request is already in flight
        # to the main thread, we block here. We bound the wait so a
        # wedged main thread doesn't pile up indefinitely.
        lock_wait = timeout  # give us up to `timeout` seconds to acquire
        acquired = _main_thread_lock.acquire(timeout=lock_wait)
        if not acquired:
            busy = _get_busy()
            walk = get_walk_progress()
            elapsed = round(time.time() - busy["started_at"], 1) if busy else None
            error_msg = (
                f"Main thread busy — {busy['command']} running for {elapsed}s."
                if busy else
                f"Main thread busy — lock held for >{lock_wait}s."
            )
            resp = {
                "success": False,
                "error": error_msg,
                "errorCode": "main_thread_busy",
                "busyCommand": busy["command"] if busy else None,
                "elapsedSeconds": elapsed,
                "_hint": (
                    "Add-in is busy with a long-running command. Do NOT retry add-in commands. "
                    "Do NOT press Escape — the add-in is working, not stuck on a dialog. "
                    "Commands that still work: fusion_state, fusion_screenshot_fusion, "
                    "fusion_close_window."
                ),
            }
            if walk:
                resp["walkProgress"] = walk
            self._respond(200, resp)
            return

        try:
            _set_busy(command, request_id)

            # Track as pending before firing
            with _lock:
                _pending_request_ids.add(request_id)

            # Fire custom event to marshal to main thread
            try:
                _app.fireCustomEvent(
                    _custom_event_id,
                    json.dumps({"request_id": request_id, "command": command, "args": args}),
                )
            except Exception as e:
                with _lock:
                    _pending_request_ids.discard(request_id)
                self._respond(500, {"success": False, "error": f"Failed to fire event: {e}"})
                return

            # Wait for the main thread handler to deliver the result
            result = _wait_for_result(request_id, timeout=timeout)
            self._respond(200, result)
        finally:
            _clear_busy()
            # Kept defensive: the lock has no owner, so a release() can still race with the next
            # request and raise RuntimeError. That used to come from the watchdog force-release
            # (gone - see _watchdog_loop), and an unguarded release() escaping do_POST left the
            # client with no response at all: a silent hang instead of an answer.
            try:
                _main_thread_lock.release()
            except RuntimeError:
                pass

    def _respond(self, status: int, data: dict):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(data).encode("utf-8"))

    def log_message(self, format, *args):
        # Suppress default HTTP logging to avoid cluttering Fusion's console
        pass


def start(custom_event_id: str, app: adsk.core.Application) -> threading.Thread:
    """Start the HTTP server on a background daemon thread."""
    global _server, _server_thread, _custom_event_id, _app, _watchdog_thread

    _custom_event_id = custom_event_id
    _app = app

    # ThreadingHTTPServer so /health and concurrent requests don't block
    # behind a slow request. Main-thread work is still serialized via
    # _main_thread_lock + CustomEvent marshaling.
    _server = ThreadingHTTPServer(("127.0.0.1", PORT), AdomBridgeHandler)
    _server.daemon_threads = True

    _server_thread = threading.Thread(target=_server.serve_forever, daemon=True)
    _server_thread.start()

    # Start stale-lock watchdog (see _watchdog_loop docstring for rationale)
    _watchdog_stop.clear()
    _watchdog_thread = threading.Thread(target=_watchdog_loop, daemon=True, name="AdomBridge-Watchdog")
    _watchdog_thread.start()

    return _server_thread


def stop():
    """Stop the HTTP server."""
    global _server
    _watchdog_stop.set()
    if _server:
        _server.shutdown()
        _server = None
