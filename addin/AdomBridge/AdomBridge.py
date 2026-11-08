"""Adom Bridge — Fusion 360 Add-In.

Starts a localhost HTTP server on a background thread (port 8774).
Incoming commands are marshaled to the main Fusion thread via a CustomEvent,
so all Fusion API calls execute safely on the main thread.
"""

import adsk.core
import adsk.fusion
import traceback
import threading
import time
import json
import os
import builtins

# Module-level references kept alive for the add-in lifetime
_app = None
_ui = None
# One start-failure modal per Fusion session; repeats go to the log (#69).
_start_error_shown = False
_custom_event = None
_event_handler = None
_http_thread = None
_standing_down = False   # True in an idle sibling copy (issue #124)

CUSTOM_EVENT_ID = "AdomBridgeCommandEvent"

# ── Re-entrancy guard for command delivery (native stack, 2026-09) ────
#
# Proven failure (macOS sample of Fusion, 1644/1644 samples parked on this path):
#
#   HTTP thread     http_server.py  _app.fireCustomEvent(...)
#   main thread     XLayer10        FireCustomEventTask::onExecute
#   main thread     AdomBridge.py   CommandEventHandler.notify
#   main thread     AdomBridge.py   dispatch_command(...)
#                   <handler>       time.sleep -> nanosleep   <- handler blocks the main thread
#
# notify() used to call dispatch_command directly, with no queue. Handlers pump Fusion's
# event queue with adsk.doEvents() (30 call sites; cloud_documents.py alone has 16) - and
# that queue can still hold the FireCustomEventTask of the NEXT request, which the HTTP
# thread fired while this handler was running. doEvents() therefore DELIVERED a second
# command INSIDE the first handler: nested dispatch, on the main thread, with no way out.
#
# The guard makes notify() refuse the nested delivery. Fusion's task still runs (and
# returns) - it just does no work - so the queue drains and the outer handler survives.
# _main_thread_lock cannot do this job: the HTTP thread holds it, and it cannot order
# execution on the main thread.
_dispatch_guard = threading.Lock()
_in_dispatch = False     # True while a command is running inside notify()
_outer_command = ""      # that command's name, for the "suppressed nested delivery" line


class CommandEventHandler(adsk.core.CustomEventHandler):
    """Handles custom events fired from the HTTP background thread.

    Each event's additionalInfo is a JSON string with {request_id, command, args}.
    The handler dispatches to the appropriate command function, then stores
    the result so the HTTP thread can return it.
    """

    def __init__(self):
        super().__init__()

    def notify(self, args: adsk.core.CustomEventArgs):
        global _in_dispatch, _outer_command
        try:
            import json
            from . import http_server
            from .commands import dispatch_command

            info = json.loads(args.additionalInfo)
            request_id = info["request_id"]
            command = info["command"]
            cmd_args = info.get("args", {})

            # Re-entrancy guard: we are already inside dispatch_command (the outer
            # handler called adsk.doEvents(), which handed us this event). Refuse the
            # nested delivery - dispatching here would run a second command on top of
            # the first one's half-finished state on the main thread. See the module
            # comment above _dispatch_guard for the native stack this comes from.
            with _dispatch_guard:
                already_dispatching = _in_dispatch
                outer = _outer_command
                if not already_dispatching:
                    _in_dispatch = True
                    _outer_command = command
            if already_dispatching:
                # No set_result: the suppressed request's HTTP waiter times out on its
                # own ("Command timed out after Ns"), which is the honest answer - the
                # command never ran. Log one line so a nested delivery is never silent.
                try:
                    adsk.core.Application.log(
                        f"AdomBridge: suppressed nested delivery {command} while {outer} runs"
                    )
                except Exception:
                    pass
                return

            try:
                # Staleness check: if the HTTP waiter already timed out and
                # bailed, skip the work entirely. Saves main-thread time on
                # heavy exports whose caller already gave up.
                if not http_server.is_request_alive(request_id):
                    adsk.core.Application.log(
                        f"AdomBridge: skipping stale request {request_id[:8]} ({command}) — HTTP waiter gone"
                    )
                    return

                try:
                    result = dispatch_command(_app, command, cmd_args)
                except Exception as e:
                    result = {"success": False, "error": f"Command error: {e}\n{traceback.format_exc()}"}

                http_server.set_result(request_id, result)
            finally:
                # Always release the guard, even when a handler raised: a flag left set
                # by one bad command would suppress EVERY later delivery for the rest of
                # the Fusion session.
                with _dispatch_guard:
                    _in_dispatch = False
                    _outer_command = ""

        except Exception:
            traceback.print_exc()


def run(context):
    """Called by Fusion 360 when the add-in starts."""
    global _app, _ui, _custom_event, _event_handler, _http_thread

    try:
        _app = adsk.core.Application.get()
        _ui = _app.userInterface

        # ONE COPY PER FUSION (page issue #124). Fusion scans more than one AddIns folder and the
        # installer writes each, so two copies can load into this one Python interpreter. The
        # first copy to get here claims the process; a sibling stands down BEFORE it registers
        # anything, because the custom event ID is shared and a sibling's stop() unregistering
        # it would cut off the serving copy.
        global _standing_down
        _owner = getattr(builtins, "_adom_bridge_addin_owner", None)
        _here = os.path.dirname(os.path.abspath(__file__))
        if _owner and os.path.normcase(_owner) != os.path.normcase(_here):
            _standing_down = True
            adsk.core.Application.log(
                "Adom Bridge add-in: already running from %s; this copy (%s) stays idle" % (_owner, _here))
            return
        builtins._adom_bridge_addin_owner = _here
        _standing_down = False

        # Register the custom event for thread marshaling
        _custom_event = _app.registerCustomEvent(CUSTOM_EVENT_ID)
        _event_handler = CommandEventHandler()
        _custom_event.add(_event_handler)

        # Start the HTTP server on a background daemon thread
        from . import http_server
        try:
            _http_thread = http_server.start(CUSTOM_EVENT_ID, _app)
        except OSError:
            # Port 8774 already bound: a SIBLING copy of this add-in is serving. install()
            # writes every known AddIns location and newer Fusion builds scan more than one
            # (seen live on #69: FusionAddins AND API/AddIns both loaded), so a second copy
            # binding the port is normal operation, not an error the user can act on.
            # Stand down quietly; the serving copy has the job. (Colby, PR #62)
            adsk.core.Application.log(
                "Adom Bridge add-in: port 8774 already served by a sibling copy; this copy is standing down")
            return

        # Log to the Text Commands palette (non-blocking)
        adsk.core.Application.log(f"Adom Bridge add-in started (port 8774)")
        # A clean start retires the start-failure marker the bridge surfaces (issue #75).
        try:
            _mk = os.path.join(os.environ.get("TEMP") or os.path.expanduser("~"), "adom-fusion", "addin-start-failed.json")
            if os.path.isfile(_mk):
                os.remove(_mk)
        except Exception:
            pass

    except Exception:
        # Only use messageBox for errors — worth blocking to show the user
        # ONE modal, total. The first failure is worth blocking to show the user; repeats
        # (Fusion can retry the lifecycle, and several installed copies can each fail the
        # same way) turned one broken deploy into a dialog storm - twice per launch, then
        # once per user action (issue #69, Colby, 2026-08-28). After the first box, log.
        global _start_error_shown
        tb = traceback.format_exc()
        here = os.path.dirname(os.path.abspath(__file__))
        # a copy that failed to start must not keep a working sibling standing down (#124)
        try:
            if os.path.normcase(getattr(builtins, "_adom_bridge_addin_owner", "") or "") == os.path.normcase(here):
                builtins._adom_bridge_addin_owner = None
        except Exception:
            pass
        # Leave a marker the BRIDGE reads (fusion_readiness / fusion_addin_status): it knows
        # which AddIns dir is broken and re-syncs it, so the fix is one bridge start away.
        try:
            marker_dir = os.path.join(os.environ.get("TEMP") or os.path.expanduser("~"), "adom-fusion")
            os.makedirs(marker_dir, exist_ok=True)
            with open(os.path.join(marker_dir, "addin-start-failed.json"), "w", encoding="utf-8") as fh:
                json.dump({"addinDir": here, "at": time.time(), "traceback": tb[-4000:]}, fh)
        except Exception:
            pass
        try:
            adsk.core.Application.log(f"Adom Bridge failed to start from {here}:\n{tb}")
        except Exception:
            pass
        # ONE modal, total, and one a person can act on: a traceback about a missing module is
        # what Adonis read five times as "Fusion keeps crashing" (issue #75). Never a storm:
        # Fusion retries the lifecycle and several installed copies can each fail the same way.
        if _ui and not _start_error_shown:
            _start_error_shown = True
            last = tb.strip().splitlines()[-1] if tb.strip() else "unknown error"
            _ui.messageBox(
                "The Adom Bridge add-in could not start from:\n" + here + "\n\n"
                "It is out of date or incomplete on disk. To fix it: start Adom Bridge (it repairs "
                "the add-in automatically on start), then restart Fusion. Or delete that folder "
                "and restart Fusion; Adom Bridge reinstalls it.\n\nDetail: " + last[:300],
                "Adom Bridge")


def stop(context):
    """Called by Fusion 360 when the add-in stops."""
    global _app, _ui, _custom_event, _event_handler, _http_thread

    if _standing_down:
        return   # an idle sibling owns nothing: never stop the serving copy's server or event (#124)
    try:
        try:
            if os.path.normcase(getattr(builtins, "_adom_bridge_addin_owner", "") or "") == os.path.normcase(os.path.dirname(os.path.abspath(__file__))):
                builtins._adom_bridge_addin_owner = None   # a restart of this copy may claim again
        except Exception:
            pass
        # Stop HTTP server
        from . import http_server
        http_server.stop()
        try:
            from .commands import activity_palette
            activity_palette.teardown()
        except Exception:
            pass

        # Unregister custom event
        if _app and _custom_event:
            _app.unregisterCustomEvent(CUSTOM_EVENT_ID)

        _custom_event = None
        _event_handler = None
        _http_thread = None

    except Exception:
        # NEVER a modal from stop. Fusion re-fires the lifecycle on ordinary events, so
        # when the add-in is broken a stop-time messageBox costs the user one dialog PER
        # ACTION they take (seen live 2026-08-28, #69). run() already told them once.
        try:
            adsk.core.Application.log(f"Adom Bridge stop error:\n{traceback.format_exc()}")
        except Exception:
            pass
