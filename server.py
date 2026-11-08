#!/usr/bin/env python3
"""Adom Fusion 360 Bridge Server — localhost HTTP server for Fusion 360 integration.

Receives commands from the Adom Bridge (Tauri app) and controls
Fusion 360 via Win32 API, os.startfile, and the AdomBridge add-in.

Usage:
    python server.py
    python server.py --port 8773
"""

import json
import os
import shutil
import threading
import re
import time as _t2
import sys
import traceback
import urllib.request
import urllib.error
import urllib.parse
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from pathlib import Path

# Ensure the plugin root is on the path
sys.path.insert(0, str(Path(__file__).parent))

from fusion_detect import detect_fusion, ensure_fusion_running, wait_for_addin, _is_fusion_running, fusion_update_in_progress, find_licensing_dialog, family_windows, _incomplete_webdeploy_present, begin_background_envelope
from handlers.open_design import handle_open_design
from handlers.close_fusion import handle_close_fusion, handle_fusion_stop, handle_fusion_kill
from handlers.dismiss_recovery import dismiss_recovery_dialog
from handlers.fusion_ui import (
    ensure_reasonable_window,
    send_key_background,
    screenshot_fusion_window,
    screenshot_hwnd,
    click_fusion,
    send_key_to_fusion,
    close_window,
    get_fusion_window_info,
)
from handlers.dialog_classify import classify_blocking_dialogs, classify_launch_dialogs, close_dialog_bg, AUTO_DISMISS_CATEGORIES
from install_addin import install as install_addin
import aps
import describe
import ab_client

DEFAULT_PORT = 8773
ADDIN_PORT = 8774

# Read once at load so /status reports the same version we ship + bump.
try:
    BRIDGE_VERSION = (Path(__file__).parent / "BRIDGE_VERSION").read_text(encoding="utf-8").strip()
except Exception:
    BRIDGE_VERSION = "unknown"

# The add-in version this bridge BUNDLES (the one install_addin syncs into
# Fusion's Roaming AddIns dir). Read from our bundled manifest so it can't drift
# from a hardcode. Compared against the RUNNING add-in's reported version to
# catch a stale add-in (issue #55) - the running copy only re-syncs when Fusion
# is closed on bridge start, so a user who never restarted Fusion keeps an old
# add-in and commands like fusion_aps_open fail silently. We make it LOUD.
try:
    EXPECTED_ADDIN_VERSION = (json.loads(
        (Path(__file__).parent / "addin" / "AdomBridge" / "AdomBridge.manifest").read_text(encoding="utf-8")
    ) or {}).get("version", "unknown")
except Exception:
    EXPECTED_ADDIN_VERSION = "unknown"


def _addin_staleness(reported_version) -> dict:
    """Compare the running add-in's reported version to the bundled one.

    Returns {addinVersion, expectedAddinVersion, addinStale, _staleHint?}. A
    stale add-in is the #55 silent-failure: re-sync it by RESTARTING Fusion
    (fusion_stop then fusion_start) so the bridge's on-start install_addin can
    overwrite the Roaming copy (which is file-locked while Fusion runs)."""
    info = {
        "addinVersion": reported_version or "unknown",
        "expectedAddinVersion": EXPECTED_ADDIN_VERSION,
    }
    stale = (
        reported_version not in (None, "unknown")
        and EXPECTED_ADDIN_VERSION not in (None, "unknown")
        and str(reported_version) != str(EXPECTED_ADDIN_VERSION)
    )
    info["addinStale"] = bool(stale)
    if stale:
        info["_staleHint"] = (
            f"STALE add-in: Fusion is running add-in v{reported_version} but this bridge "
            f"bundles v{EXPECTED_ADDIN_VERSION}. Newer verbs (e.g. fusion_aps_open/open_by_urn) "
            f"can fail silently. Fix: fusion_stop then fusion_start - the bridge re-syncs the "
            f"add-in from its cache on start (only possible with Fusion CLOSED)."
        )
    return info

# Populated at startup
fusion_info = None

# Caller identity: WHO we are currently acting for (issue #348). ab stamps X-Adom-Caller-* on
# every request it dispatches to us; we echo those back on our own ab callbacks so the user sees
# "chip-fetcher tab 3 (via fusion)" instead of a nameless bridge.
#
# THREAD-LOCAL, deliberately. This is a ThreadingHTTPServer, so a module global would let
# concurrent requests clobber each other and attribute tab 3's work to tab 11 - precisely the
# failure this feature exists to prevent. Thread-local also fixes the stale-identity trap the
# contract calls out: a background thread (health poll, timer, crash cleanup) has no value bound,
# so it can never inherit some earlier request's name.
_caller_tls = threading.local()


def _set_caller_identity(ident: dict | None) -> None:
    """Bind the calling AI thread's identity to THIS request thread. Empty values are dropped."""
    _caller_tls.value = {k: v for k, v in (ident or {}).items() if v}


def _get_caller_identity() -> dict:
    """Identity bound to this thread, or {} when we are acting on our own behalf."""
    return getattr(_caller_tls, "value", None) or {}


def _stamp_caller(args: dict) -> dict:
    """Attribute an OUTGOING ab call (issue #348).

    Forwards the identity of the AI thread we are carrying out work for. When we act on our OWN
    behalf (health poll, timer, cleanup sweep) nothing is bound to this thread, so we name
    ourselves rather than forwarding a stale identity. An explicit caller in args always wins.

    Returns a COPY; the caller's dict is never mutated."""
    out = dict(args or {})
    if out.get("caller") or out.get("aiThread"):
        return out
    ident = _get_caller_identity()
    if ident.get("thread"):
        caller = {"aiThread": ident["thread"]}
        if ident.get("container"):
            caller["containerName"] = ident["container"]
    else:
        caller = {"aiThread": "fusion bridge (self)", "containerName": "local"}
    out["caller"] = caller
    return out


def _reclaim_seat_from_peers() -> int:
    """Free the Autodesk seat held by another machine by stopping Fusion on peer ADs.

    John (2026-07-06): "if the user asked you to do something with fusion you should
    just grab the license back. the user can always grab it back on the other machine,
    so there's no harm." So on a seat conflict we do NOT ask - we free the seat at the
    SOURCE via the cross-ab direct API (fully background + reliable, no CEF-dialog
    clicking), then relaunch locally with no conflict. Best-effort; returns the number
    of peers signalled (0 if the direct API/peers are unavailable)."""
    n = 0
    try:
        if not ab_client.available():
            return 0
        for peer in ab_client.peers():
            try:
                if ab_client.call("fusion_stop", {}, target=peer) is not None:
                    n += 1
            except Exception:
                pass
    except Exception:
        pass
    return n


def _resolve_seat_via_uia(dlg: dict) -> bool:
    """Resolve the 'Active Sessions Exceeded' seat dialog by UIA-invoking its native
    'Continue' button (Suspend is pre-selected) - entirely IN THE BACKGROUND.

    THE breakthrough (John 2026-07-06, after a long day of failing): UIA Invoke does
    NOT foreground the window, so this grabs the license without ever disturbing the
    user - unlike SendInput coordinate clicks (need foreground) or kill+relaunch (the
    server keeps the seat). Proven live: `desktop_ui_click {hwnd, name:"Continue"}`
    returned "Clicked in the BACKGROUND - the window did NOT come to the foreground"
    and the dialog cleared. The bridge calls the ab `desktop_ui_click` verb on ITSELF
    via the ab 1.9.84 direct API. Never raises."""
    try:
        hwnd = dlg.get("hwnd")
        if not hwnd or not ab_client.available():
            return False
        # TWO variants of the seat dialog, opposite buttons (John, 2026-08-21: "when you see
        # this dialog, always regrab the session"):
        #   "Active Sessions Exceeded"  -> Continue    (take the seat; Suspend is pre-selected)
        #   "Session Suspended"         -> Check again (RESTORE this box's suspended session -
        #                                  the other box took the seat and this one must ask
        #                                  for it back; Shut Down would sign this box out)
        # Try Continue first (the common variant), then Check again. desktop_ui_click on a
        # button that does not exist is a cheap no-op, so probing both costs nothing.
        for btn in ("Continue", "Check again"):
            res = ab_client.call("desktop_ui_click", {"hwnd": hwnd, "name": btn}, timeout=4)
            if res and (res.get("success") or res.get("status") == "ok"):
                if btn == "Check again":
                    print("[Fusion Bridge] seat: clicked 'Check again' to REGRAB the suspended "
                          "session (Session Suspended variant)")
                return True
        return False
    except Exception:
        return False


def _owned_popup_count(main_hwnd) -> int:
    """ownedPopupCount on a window via ab's desktop_screenshot_window - the SAME
    parent/child screenshot signal John kept pointing at (2026-07-06): a modal seat/
    error dialog shows up as an OWNED POPUP of the main Fusion window. This is the
    ground-truth 'is a dialog still up' check used to VERIFY a seat dialog actually
    cleared, instead of trusting a window-size heuristic (which false-'resolved' the
    823x262 'Suspend Remote Session' confirm). Returns the count, or -1 if unavailable."""
    try:
        if not main_hwnd or not ab_client.available():
            return -1
        res = ab_client.call("desktop_screenshot_window", {"hwnd": int(main_hwnd)}, timeout=4)
        if isinstance(res, dict):
            c = res.get("ownedPopupCount")
            if c is None and isinstance(res.get("data"), dict):
                c = res["data"].get("ownedPopupCount")
            if c is not None:
                return int(c)
    except Exception:
        pass
    return -1


def _resolve_seat_dialog(max_clicks: int = 3) -> dict:
    # max_clicks kept LOW (was 8): readiness calls this every tick and it SELF-HEALS across ticks, so
    # a single call must stay responsive - 8 clicks x (UIA click + screenshot-verify, each up to its
    # timeout) could blow readiness's whole budget and make the bridge look hung (John 2026-07-14).
    """Detect + resolve the seat/licensing modal deterministically, IN THE BACKGROUND,
    and VERIFY it truly cleared by SCREENSHOTTING the parent window's ownedPopupCount -
    not by re-running a size heuristic (that false-'resolved' bit John hard, 2026-07-06:
    the code clicked once, the size filter then failed to re-detect the short confirm
    variant, and readiness lied that the dialog was gone while it sat there blocking).

    Each pass: find the dialog -> UIA-invoke 'Continue' (background, Suspend pre-selected
    so it grabs the license) -> screenshot the parent's ownedPopupCount. Done only when
    BOTH find_licensing_dialog() sees nothing AND the parent shows 0 owned popups (or the
    screenshot signal is unavailable and the heuristic agrees). Returns
    {sawDialog, clicks, verified}. Never raises."""
    import time as _t
    clicks = 0
    saw = False
    verified = False
    for _ in range(max_clicks):
        try:
            dlg = find_licensing_dialog()
        except Exception:
            dlg = None
        if not dlg:
            verified = True
            break
        saw = True
        parent = dlg.get("owner")
        if _resolve_seat_via_uia(dlg):     # count only clicks that actually landed, so
            clicks += 1                    # seatDialogAutoResolved isn't a lie when ab_client is down
        _t.sleep(1.5)
        # Ground-truth confirm via parent/child screenshot (John's insisted-on check).
        cnt = _owned_popup_count(parent)
        if cnt == 0:
            verified = True
            break
        # cnt == -1 (screenshot unavailable): fall through, the next find_licensing_dialog
        # pass decides. cnt > 0: still up, loop and click again.
    return {"sawDialog": saw, "clicks": clicks, "verified": verified}


def _find_adom_desktop_cli() -> str | None:
    """Locate the host app's CLI so the bridge can drive it when the in-process ab_client is down.

    Accepts BOTH names. The host app was renamed Adom Bridge -> Adom Bridge in v2.0.0 (issue #537),
    so a box may carry either layout: a renamed install has "Adom Bridge"/adom-bridge-cli.exe, an
    older one still has "Adom Bridge"/adom-desktop-cli.exe. Looking for only one silently kills the
    CLI fallback on the other, so we try new-first then old and let whichever exists win.

    The hard rule survives the rename: shell the **-cli** exe only. Never the GUI exe (adom-bridge.exe
    / adom-desktop.exe) - launching that with verb args foregrounds the app on the user's screen every
    single call (issue #295). Prefer the loopback direct API over shelling at all (_ad_direct_api_call).
    """
    import shutil
    la = os.environ.get("LOCALAPPDATA", "")
    apps = (("Adom Bridge", "adom-bridge-cli.exe"), ("Adom Bridge", "adom-desktop-cli.exe"))
    if la:
        for folder, exe in apps:
            for parent in (la, os.path.join(la, "Programs")):
                c = os.path.join(parent, folder, exe)
                if os.path.exists(c):
                    return c
    # PATH: the CLI shim only. On Windows NEVER resolve the bare name - that is the GUI exe.
    if os.name != "nt":
        # Linux/mac: `adom-bridge` IS the CLI as of ab 2.0.35 (the -cli suffix was dropped; a
        # back-compat symlink lingers). Prefer the canonical name.
        for shim in ("adom-bridge", "adom-bridge-cli", "adom-desktop-cli", "adom-desktop"):
            hit = shutil.which(shim)
            if hit:
                return hit
        return None
    # Windows: the -cli shim ONLY. Never the bare name - there `adom-bridge.exe` is the GUI app,
    # and shelling it with verb args foregrounds it on the user's screen every call (issue #295).
    for shim in ("adom-bridge-cli", "adom-desktop-cli"):
        hit = shutil.which(shim)
        if hit:
            return hit
    return None


def _cli_notify_all(title: str, body: str, level: str, buttons=None) -> dict:
    """Deliver a toast to EVERY connected desktop via `adom-bridge --target all notify_user`. This is
    the reliable cross-ab path when the bridge's in-process ab_client is down (headless VM): the CLI
    joins the relay itself and `--target all` reaches the user's real machine (not just this VM).
    Best-effort + never raises. Returns {delivered, targets, error}. (John 2026-07-14: the notify MUST
    actually leave the box - a returned-but-unsent payload is the bug, not a feature.)"""
    import subprocess as _sp
    exe = _find_adom_desktop_cli()
    if not exe:
        return {"delivered": False, "error": "adom-bridge CLI not found"}
    # STICKY by default: a human-wall alert must NOT vanish in a few seconds (John 2026-07-14 - the
    # first toasts disappeared before he noticed them). scenario:reminder keeps it on screen until the
    # user acts; it REQUIRES >=1 button, so include one. durationLong is a belt-and-suspenders ~25s.
    # ⛔ A NOTICE THAT OFFERS AN ACTION MUST CARRY THE CHOICE (John, 2026-08-28: "i just
    # clicked 'got it' on the restart notice, but why do you not give me a choice? i have
    # no idea if 'got it' means you're restarting or not"). The default single "Got it"
    # stays for plain FYI toasts, but a caller with a real decision passes its own
    # buttons and reads the click back via notify_response. Never print a promise
    # ("we will save your documents first") on a toast whose only button is an
    # acknowledgement - that is the text making an offer the UI cannot keep.
    payload = json.dumps({"title": title, "body": body, "level": level,
                          "scenario": "reminder", "durationLong": True,
                          "buttons": buttons or [{"label": "Got it"}]})
    try:
        # LOCAL machine only. --target all put a question about THIS box's Fusion on every
        # machine the user owns (seen live 2026-08-31). An unattended box can opt back into
        # broadcast with the notifyBroadcast preference.
        _tgt = ["--target", "all"] if _get_bridge_pref("notifyBroadcast", False) else []
        r = _sp.run([exe] + _tgt + ["notify_user", payload],
                    capture_output=True, text=True, timeout=30,
                    creationflags=getattr(_sp, "CREATE_NO_WINDOW", 0))
        out = (r.stdout or "") + (r.stderr or "")
        # ab's CLI returns {status:'ok', action:'displayed'} per target; treat a zero exit or a
        # 'displayed'/'ok' in the output as delivered. The Windows exe may print nothing yet still
        # deliver, so a clean exit code is accepted too.
        delivered = (r.returncode == 0) or ("displayed" in out) or ('"status": "ok"' in out) or ("status':'ok" in out)
        nid = None
        try:
            import re as _re
            m = _re.search(r'"id"\s*:\s*"([0-9a-f]{8,})"', out)
            nid = m.group(1) if m else None
        except Exception:
            pass
        return {"delivered": bool(delivered), "targets": "all", "id": nid,
                "error": None if delivered else out[:200]}
    except Exception as e:
        return {"delivered": False, "error": str(e)[:200]}


def _set_user_restart_hold(nid):
    """The user chose to restart Fusion themselves; no automation may do it for them."""
    try:
        import pathlib as _pl, json as _js, time as _t, os as _os
        d = _pl.Path(_os.path.expanduser("~")) / ".adom" / "fusion-bridge"
        d.mkdir(parents=True, exist_ok=True)
        (d / "user_restart_hold.json").write_text(_js.dumps({"at": _t.time(), "notification": nid}))
    except Exception:
        pass


def _user_restart_hold_active():
    """True while the user has said 'I'll handle it' and the add-in has not come back.

    John, 2026-08-28: "i just clicked 'i'll handle it' when asked to restart fusion and
    then you quit fusion on me? i would have expected that i would restart fusion." Two
    racing chains each entered their launch stage, and the launch handler's running-but-
    silent branch restarted Fusion because that is its doctrine (#39). Nothing consulted
    the answer he had just given. This hold is that answer, made visible to every path
    that could restart Fusion. It clears itself the moment the add-in answers (he did
    the restart) or after 4 hours (he forgot; do not brick automation forever).
    """
    try:
        import pathlib as _pl, json as _js, time as _t, os as _os
        f = _pl.Path(_os.path.expanduser("~")) / ".adom" / "fusion-bridge" / "user_restart_hold.json"
        if not f.exists():
            return False
        at = float((_js.loads(f.read_text()) or {}).get("at") or 0)
        if _t.time() - at > 4 * 3600:
            f.unlink(missing_ok=True); return False
        if _addin_round_trip_ready(tries=1):
            f.unlink(missing_ok=True); return False      # they restarted; hold released
        return True
    except Exception:
        return False


def _watch_restart_choice(notification_id: str, timeout_s: float = 1800.0):
    """Wait (in the background) for the user's answer to the restart notice and honour it.

    "Save and restart" -> fusion_restart_for_addin {save:true}, which saves first and
    REFUSES rather than discarding a never-saved document. "I'll handle it" -> nothing:
    the user keeps control and the offer is simply recorded. Either way the choice lands
    in the action ledger, so "why did my Fusion restart" has an answer with a timestamp.
    """
    import threading as _th

    def _run():
        import time as _t
        deadline = _t.time() + timeout_s
        exe = _find_adom_desktop_cli()
        while exe and _t.time() < deadline:
            try:
                import subprocess as _sp, json as _js
                r = _sp.run([exe, "--target", "all", "notify_response",
                             _js.dumps({"id": notification_id})],
                            capture_output=True, text=True, timeout=30,
                            creationflags=getattr(_sp, "CREATE_NO_WINDOW", 0))
                out = (r.stdout or "")
                i = out.find("{")
                x = _js.loads(out[i:]) if i >= 0 else {}
                x = x.get("data") or x
                # ab reports the press as response: {"action": "<label>"} - a DICT. The
                # first version read `response` as a string, got a dict, treated it as falsy,
                # and fell into the decline branch WITHOUT recording anything. John pressed
                # "I'll handle it" (2026-08-28 21:55) and the ledger has no trace of it.
                # A choice the bridge cannot even see is a choice it cannot honour.
                resp = x.get("response")
                if isinstance(resp, dict):
                    resp = resp.get("action") or resp.get("button") or resp.get("id")
                btn = x.get("buttonId") or x.get("button") or resp
                if x.get("pending") is False or btn or x.get("dismissed"):
                    _bridge_action_ledger("restart_choice_seen",
                                          {"raw": str(btn)[:40], "notification": notification_id})
                    if btn in ("restart", "Save and restart"):
                        _bridge_action_ledger("restart_choice", {"choice": "restart",
                                                                  "notification": notification_id})
                        res = _handle_restart_for_addin(detect_fusion(), {"save": True})
                        _bridge_action_ledger("restart_for_addin_result",
                                              {"success": res.get("success"),
                                               "errorCode": res.get("errorCode")})
                        if not res.get("success"):
                            _cli_notify_all("Fusion restart did not complete",
                                            str(res.get("error") or res.get("errorCode"))[:200],
                                            "error")
                    else:
                        _bridge_action_ledger("restart_choice",
                                              {"choice": btn or "dismissed",
                                               "notification": notification_id})
                        # "I'll handle it" means: THE USER OWNS FUSION'S LIFECYCLE NOW.
                        # Record a hold so no automatic path restarts Fusion on their
                        # behalf until they do it themselves (cleared when the add-in
                        # next answers, i.e. they restarted).
                        _set_user_restart_hold(notification_id)
                    return
            except Exception:
                pass
            _t.sleep(3)
    _th.Thread(target=_run, daemon=True).start()


_AUTO_ADDIN_RESTART = {"at": 0.0}


def _begin_auto_addin_restart() -> bool:
    """Restart Fusion for the add-in in the background, once per 4 minutes, saving what can
    be saved. Only called when no unsaved document belongs to the user."""
    import time as _t, threading as _th
    if _t.time() - _AUTO_ADDIN_RESTART["at"] < 240:
        return False
    _AUTO_ADDIN_RESTART["at"] = _t.time()

    def _go():
        try:
            r = _handle_restart_for_addin(detect_fusion(), {"save": True, "auto": True})
            print("[Fusion Bridge] auto add-in restart: %s" % ({k: r.get(k) for k in ("success", "errorCode")},))
        except Exception as e:
            print("[Fusion Bridge] auto add-in restart failed: %s" % str(e)[:200])
    _th.Thread(target=_go, daemon=True).start()
    return True


def _addin_restart_info(fire_toast: bool = True) -> dict:
    """Everything a CALLER needs to tell a HUMAN that Fusion must restart, plus a toast.

    ⛔ THE ADONIS INCIDENT (John, 2026-08-27). A brand-new user installed ab, already had
    Fusion open, and drove the fusion bridge from the wiki through web-control. The bridge
    wrote a fresh add-in into Fusion's add-in dirs - Fusion only loads add-ins AT LAUNCH, so
    his running Fusion could not see it - and then said NOTHING. It printed the sync to a
    console nobody reads, returned a bare "add-in not responding", and the verbs failed. He
    had no way to know the product was fine and merely needed a relaunch; John and Colby had
    to tell him by hand. Once he restarted, the library import worked perfectly first time.
    Everything needed to warn him was already known at that moment.

    So this returns a payload a web-control caller can render as a QUESTION ("we need to
    restart Fusion; you have N documents open; shall we, and we will save them first"), and
    fires an ab toast so the human is told even if the caller ignores the payload.

    Document counts come from the add-in, which by definition is NOT answering here, so they
    are reported as unknown rather than guessed - `documentsKnown` says which. A count we
    cannot take is not a count we may invent.
    """
    import time as _t
    info = {
        "restartRequired": True,
        "reason": "addin_installed_while_fusion_running",
        "explanation": ("The AdomBridge add-in was installed or updated while Fusion was "
                        "already running. Fusion loads add-ins only at launch, so this "
                        "Fusion cannot see it until it restarts. Nothing is wrong with the "
                        "install and nothing is lost - it just needs a relaunch."),
        "userMessage": ("Fusion needs a quick restart to finish setting up the Adom add-in. "
                        "We can save your open documents first."),
        "remedyVerb": "fusion_restart_for_addin",
        "documentsKnown": False,
        "openDocuments": None,
        "unsavedDocuments": None,
    }
    # Best effort: if the add-in answers at all (an OLD copy still serving), we can count.
    try:
        st = _proxy_to_addin("get_app_state", {}, timeout=8) or {}
        docs = st.get("documents")
        if isinstance(docs, list):
            info["documentsKnown"] = True
            info["openDocuments"] = len(docs)
            info["unsavedDocuments"] = sum(1 for d in docs if not d.get("isSaved"))
            info["unsavedDocumentNames"] = [d.get("name") for d in docs if not d.get("isSaved")][:20]
            info["documentNames"] = [d.get("name") for d in docs][:20]
    except Exception:
        pass

    # OWNERSHIP DECIDES (John, 2026-08-30, two pointless toasts while he was at lunch:
    # "i've edited NONE of the fusion docs so clearly none of that is mine"). The bridge
    # registers every document it opens; a restart may only ASK when an unsaved document
    # the user made is at risk. With none, the caller auto-restarts and no toast exists.
    try:
        _unames = None
        if info.get("documentsKnown"):
            _unames = info.get("unsavedDocumentNames") or []
        elif _LAST_KNOWN_DOCS.get("docs") is not None:
            _unames = [n for (n, _sv) in _LAST_KNOWN_DOCS["docs"] if not _sv]
            info["documentsSource"] = "last known, %.0fs old" % (_t.time() - _LAST_KNOWN_DOCS["at"])
        if _unames is not None:
            info["userUnsaved"] = _user_unsaved_docs(_unames)
            info["autoRestart"] = not info["userUnsaved"]
        else:
            # No document list at all, from the add-in or memory. If the bridge launched
            # this Fusion itself, everything in it is bridge-era: restart. Only a Fusion
            # the user had open before the bridge ever spoke to it stays an ask.
            info["autoRestart"] = not _SESSION_DOCS_AT_START
    except Exception:
        info["autoRestart"] = False

    if fire_toast and not info.get("autoRestart"):
        # ONE toast per staged install, not per failed verb: Adonis would have had a dozen
        # verbs fail, and a dozen identical toasts is its own bad experience.
        try:
            import pathlib as _pl, json as _js, os as _os
            d = _pl.Path(_os.path.expanduser("~")) / ".adom" / "fusion-bridge"
            d.mkdir(parents=True, exist_ok=True)
            f = d / "addin_restart_toast.json"
            last = 0.0
            if f.exists():
                try:
                    last = float((_js.loads(f.read_text()) or {}).get("at") or 0)
                except Exception:
                    last = 0.0
            if _t.time() - last > 900:      # at most once every 15 minutes
                body = info["userMessage"]
                _yours = info.get("userUnsaved") or []
                if _yours:
                    body = ("Fusion needs a quick restart to finish setting up the Adom "
                            "add-in. Blocked only by %d unsaved document(s) that look like "
                            "YOURS: %s. Everything else open is Adom's own scratch and is "
                            "handled automatically. We will save yours first."
                            % (len(_yours), ", ".join(str(n) for n in _yours[:5])))
                elif info.get("documentsKnown"):
                    body = ("Fusion needs a quick restart to finish setting up the Adom "
                            "add-in. You have %d document(s) open%s - we will save them "
                            "first." % (info["openDocuments"],
                                        (", %d unsaved" % info["unsavedDocuments"])
                                        if info.get("unsavedDocuments") else ""))
                # Two real choices, and the bridge honours whichever is pressed. A user who
                # is mid-work keeps control with "I'll handle it"; one who wants it done gets
                # the save + restart the message promises. There is no button that means
                # "you have been told", because that leaves the user exactly where Adonis was.
                r = _cli_notify_all(
                    "Fusion needs a restart", body, "info",
                    buttons=[{"label": "Save and restart", "id": "restart"},
                             {"label": "I'll handle it", "id": "later"}])
                info["toast"] = {"delivered": bool(r.get("delivered")),
                                 "id": r.get("id"), "error": r.get("error"),
                                 "choices": ["Save and restart", "I'll handle it"]}
                f.write_text(_js.dumps({"at": _t.time(), "id": r.get("id")}))
                if r.get("id"):
                    _watch_restart_choice(r["id"])
            else:
                info["toast"] = {"delivered": False, "suppressed": "already toasted recently"}
        except Exception as e:
            info["toast"] = {"delivered": False, "error": str(e)[:200]}
    return info


def _handle_notify_owner(args: dict) -> dict:
    """LAST-RESORT: toast the user's MAIN desktop (cross-ab) to ask for a human step.

    John's standing rule (2026-07-07): the bridge/AI does EVERYTHING itself; the only
    legitimate uses are true human walls - password/2FA entry, UAC elevation, a physical
    action. When this bridge runs on an unattended VM, the toast must reach the machine
    the user is actually AT - ab_client.notify(reach_user=True) fans out to all peer ADs
    on the relay, so it lands on their main computer too. Returns which targets were hit."""
    title = args.get("title") or "Fusion bridge needs you"
    body = args.get("body") or args.get("message") or "A human step is required to continue."
    level = args.get("level") or "warning"
    if not ab_client.available():
        # ab_client is the bridge's IN-PROCESS ab API - it is routinely UNAVAILABLE when this bridge
        # runs on an unattended Hyper-V VM (found live 2026-07-14: the toast silently never reached the
        # user, and the AI "forgot" to relay it - exactly the failure John demanded we engineer out).
        # SELF-DELIVER via the adom-bridge CLI, which connects to the relay independently: `--target
        # all` fans the toast out to EVERY connected desktop (the VM + the user's real machines), so it
        # reliably lands on the computer the user is actually AT. No AI relay step to forget.
        cli = _cli_notify_all(title, body, level)
        if cli.get("delivered"):
            return {"success": True, "via": "cli:--target all", "targets": cli.get("targets"),
                    "_hint": ("Toast fanned out to ALL connected desktops (the user's main machine "
                              "included) via the adom-bridge CLI, because the bridge's in-process ab "
                              "API was unavailable (this VM). WAIT and poll fusion_readiness; do not "
                              "re-toast within a few minutes. Only notify when the box is ACTUALLY "
                              "ready for the user to act - do not toast for a step you can do yourself.")}
        # CLI fallback also failed - return the payload so the AI can relay as the true last resort.
        return {
            "success": False,
            "error": "ab in-process API unavailable AND the adom-bridge CLI fallback failed: "
                     + str(cli.get("error"))[:200],
            "notifyUser": {"title": title, "body": body, "level": level},
            "_hint": ("Relay the notifyUser payload yourself: `adom-bridge --target all notify_user "
                      "{title, body, level}` (or `--target <the user's host>`)."),
        }
    res = ab_client.notify(title, body, level=level, reach_user=True) or {}
    # reach_user fans out via target="all", whose response is a BROADCAST envelope
    # {results:{host:{action}}, summary:{ok,failed,total}, targets:[...]}. Recognize that shape for
    # success (summary.ok>0) - not just the single-target {status:"ok"} - and report the ACTUAL
    # desktops reached, so a toast that landed on the user's laptop isn't mis-reported as failed.
    summary = res.get("summary") or {}
    ok = bool(res.get("success") or res.get("status") == "ok" or summary.get("ok", 0) > 0)
    reached = res.get("targets") or (["self"] if ok else [])
    return {
        "success": ok,
        "via": "ab_client:direct(all)",
        "targets": reached,
        "_hint": ("Toast fanned out to ALL connected desktops (the user's machine included) via the "
                  "in-process ab direct API. This is the LAST RESORT - only use after exhausting "
                  "programmatic options (UIA background clicks, seat auto-resolve, warm-SSO sign-in), "
                  "and only when the box is ACTUALLY ready for the user to act. Now WAIT and poll "
                  "fusion_readiness for the state to clear; do not re-toast within a few minutes."),
    }


_last_fg_notice = [0.0]


def _notify_before_foreground(reason: str) -> None:
    """Baked-in courtesy toast BEFORE the bridge foregrounds Fusion (John, 2026-07-08).

    Some Fusion interactions (SendInput key/click, CEF modal dialogs) can ONLY be driven
    with Fusion in the FOREGROUND, which steals the user's focus mid-work. The standing
    rule: ALWAYS drive in the background; foreground ONLY as a last resort. When we truly
    must, TELL the user why (via an ab notify, so every Adom user learns the principle) and
    rib Fusion for not being AI-native enough to allow it. Debounced so a burst of keystrokes
    fires ONE notice, not dozens. Best-effort; never raises, never blocks the operation."""
    try:
        now = _time.time()
        if now - _last_fg_notice[0] < 45:   # one notice per foreground burst
            return
        _last_fg_notice[0] = now
        body = (
            f"Adom is briefly bringing Fusion to the FOREGROUND: {reason}. I ALWAYS try to do "
            "everything in the background and only foreground when I have NO option left. Fusion "
            "just isn't AI-native/fast enough yet to let me drive this part in the background - "
            "hopefully Autodesk makes their app fully AI-drivable soon so I can skip these "
            "workarounds. Sorry for the interruption!"
        )
        ab_client.notify("Adom is foregrounding Fusion (last resort)", body,
                         level="info", reach_user=True)
    except Exception:
        pass


def _handle_new_electronics_from_eagle(args: dict) -> dict:
    """Import a legacy EAGLE .sch (+ paired .brd) into a NEW Fusion electronics design
    so the parts actually INSTANTIATE (schematic + populated board), then land in the PCB
    editor. Proven live 2026-07-07 on winvm.

    ⭐ PREFER THE BACKGROUND PATH FIRST (learned 2026-07-08, the hard way): if you have a
    `.brd` with the parts ALREADY PLACED (`<elements>` with x/y + an embedded `<library>`),
    do NOT use this verb - call **`fusion_open_board {filePath: <.brd>}`** instead. It opens
    the board via `Document.newDesignFromLocal`, which INSTANTIATES the placed elements with
    ZERO modal file-dialogs and ZERO foreground (this verb's ImportSCHAndBRDCmd pops TWO
    native Open dialogs that STEAL the user's focus - modal dialogs always foreground, there
    is no background way to drive them). A hand-authored EAGLE `.brd` (well-formed XML: layers
    + board outline on layer 20 + libraries/packages + elements) imports cleanly this way.
    ➜ FOR 3D BODIES ON THAT BOARD (not flat pads): embed a `<packages3d>` section (each
    `<package3d name=... wip_urn="urn:adsk.wipprod:fs.file:vf...."/>` from a prior
    `fusion_build_library_3d` bind) in the `.brd`'s `<library>`, AND give every `<element>` a
    `package3d_urn="<that pin's wip_urn>"`. Then `fusion_open_board` + `fusion_show_3d_board`
    renders the REAL component 3D (Fusion resolves the urns from the Hub). No urn on the
    element = a flat pad. (If a urn is stale/unresolved, re-run `fusion_build_library_3d` for
    that part to mint a fresh one.)

    THIS VERB is only for a legacy `.sch` whose parts must be instantiated by Fusion's own
    importer: `Document.newDesignFromLocal <file.sch>` opens the schematic EDITOR but does NOT
    instantiate parts (board_info == 0); Fusion's REAL EAGLE importer is `ImportSCHAndBRDCmd`,
    which pops the two Open dialogs. This verb drives them via the ab direct API when present -
    but that FOREGROUNDS Fusion, so use the `.brd`+`fusion_open_board` route whenever you can
    place the parts yourself. The native `.fsch/.fbrd` container is opaque BINARY, un-authorable.

    args: {schPath: Windows path to the .sch (required), brdPath: Windows path to the .brd
           (optional; defaults to the .sch's sibling .brd)}
    """
    import time as _t
    sch = (args.get("schPath") or args.get("filePath") or "").replace("\\", "/")
    if not sch or not sch.lower().endswith(".sch"):
        return {"success": False, "error": "schPath (a .sch file on the Windows host) is required.",
                "_hint": "Author the EAGLE .sch + paired .brd (same basename, same folder), stage both "
                         "to Windows (send_files), then call with {schPath, brdPath}."}
    brd = (args.get("brdPath") or (sch[:-4] + ".brd")).replace("\\", "/")
    if not ab_client.available():
        # Do NOT dead-end here (old bug: hard-failed even on latest ab when this bridge process
        # didn't get ab's injected direct-API env). The RIGHT move is the background route.
        brd_guess = (args.get("brdPath") or (sch[:-4] + ".brd")).replace("\\", "/")
        return {"success": False, "errorCode": "use_open_board_instead",
                "error": "ab direct API unavailable - and this dialog-driven import FOREGROUNDS Fusion anyway.",
                "_hint": ("Don't drive the Open dialogs. If the parts are placed in the .brd (elements + "
                          "embedded library), call `fusion_open_board {filePath:\"" + brd_guess + "\"}` - it "
                          "instantiates them via newDesignFromLocal with NO dialogs and NO foreground (the "
                          "background way). For 3D bodies, embed <packages3d> + per-element package3d_urn in the "
                          ".brd (see this verb's docstring), then fusion_show_3d_board. Only if you truly have a "
                          "bare .sch that Fusion must import: fire 'Commands.Start ImportSCHAndBRDCmd' via "
                          "fusion_execute_text_command and drive the two Open dialogs (foreground; last resort).")}

    def _find_open_dialog():
        res = ab_client.call("desktop_list_windows", {}) or {}
        out = res.get("output") if isinstance(res, dict) else None
        if isinstance(out, str):
            try: out = json.loads(out)
            except Exception: out = None
        wins = ((out or res).get("data", out or res) or {}).get("windows", []) if isinstance(out or res, dict) else []
        for w in wins:
            if str(w.get("title", "")).strip().lower() == "open":
                return w.get("hwnd")
        return None

    def _pick(hwnd, basename):
        # select the file by accessible name, then click Open - both background UIA
        ab_client.call("desktop_ui_click", {"hwnd": hwnd, "name": basename})
        _t.sleep(1.0)
        ab_client.call("desktop_ui_click", {"hwnd": hwnd, "name": "Open"})

    import os as _os
    sch_base, brd_base = _os.path.basename(sch), _os.path.basename(brd)

    # 1) Fire Fusion's EAGLE importer (opens the first Open dialog).
    _proxy_to_addin("execute_text_command", {"command": "Commands.Start ImportSCHAndBRDCmd"}, timeout=15)

    # 2) Drive the two Open dialogs (sch, then brd). Each appears after a beat; retry.
    picked = []
    for want, base in (("sch", sch_base), ("brd", brd_base)):
        dlg = None
        for _ in range(20):
            _t.sleep(1.5)
            dlg = _find_open_dialog()
            if dlg:
                break
        if not dlg:
            # brd dialog may not appear if Fusion inferred the sibling automatically
            if want == "brd" and picked:
                break
            return {"success": False, "error": f"The '{want}' Open dialog never appeared.",
                    "_hint": "Screenshot the Fusion hwnd + READ screenshots[] for a blocking modal; the importer "
                             "may have errored on the EAGLE source. Verify the .sch/.brd are valid EAGLE.",
                    "data": {"picked": picked}}
        _pick(dlg, base)
        picked.append(base)
        _t.sleep(2.0)

    # 3) Verify by STATE - poll until the imported design is an electronics design.
    ready = False
    for _ in range(40):  # up to ~2 min; import is slow, esp. software-rendered
        _t.sleep(3)
        try:
            st = _proxy_to_addin("get_app_state", {}, timeout=8)
            d = st.get("data") or {}
            if d.get("isElectronics") and str(d.get("activeWorkspace", "")).lower() in ("pcb editor", "schematic editor", "board layout", "3d pcb"):
                ready = True
                break
        except Exception:
            pass  # app_state is None while a modal import dialog blocks - keep polling

    return {
        "success": ready,
        "imported": picked,
        "statusVerb": "fusion_board_info",
        "_hint": (
            "EAGLE design imported via Fusion's own ImportSCHAndBRDCmd (both Open dialogs driven in the "
            "background). It is now an electronics design in the PCB editor. NEXT: fusion_show_2d_board, "
            "then RATSNEST + 'AUTO ;' (fusion_electron_run) to autoroute, fusion_show_3d_board for the 3D. "
            "NOTE: board_info can read 0 right after import (wrong-view query) even though the board is "
            "populated - verify by screenshotting the Fusion hwnd (read screenshots[]) or fusion_show_2d_board "
            "+ WINDOW FIT. If 3D bodies are missing, the placed parts' package3d urns must resolve in THIS "
            "project - build the library's 3D into this project with fusion_build_library_3d (do NOT reuse "
            "another library's urns; they don't resolve cross-project)."
            if ready else
            "Import fired + both Open dialogs driven, but the design did not confirm as electronics within the "
            "budget (a slow VM import can run longer). Poll fusion_get_app_state, and screenshot the Fusion hwnd "
            "reading screenshots[] for a blocking import dialog."
        ),
    }



_FUSION_USER = {}       # {"user","email","fetchedAt"} - cleared when Fusion stops


def _ensure_fusion_user_async():
    """Cache WHO is signed into Fusion itself (app.userName / userId) without ever
    slowing a readiness poll: first ready poll with an empty cache kicks a daemon
    thread; the next poll reads the answer. Cleared on fusion stop/kill so an
    account switch shows the new identity. (John, 2026-08-22: the dashboard cards
    must show who is logged in - the auth test matrix runs several accounts.)"""
    if _FUSION_USER.get("user") or _FUSION_USER.get("fetching"):
        return
    _FUSION_USER["fetching"] = True
    import threading as _thu

    def _fetch():
        try:
            r = _proxy_to_addin("run_modeling_script",
                                {"script": "import adsk.core\n"
                                           "a=adsk.core.Application.get()\n"
                                           "result={'user': a.userName, 'id': a.userId}"},
                                timeout=20)
            d = (r.get("data") or {}).get("result") or {}
            if d.get("user"):
                _FUSION_USER.update(user=d.get("user"), userId=d.get("id"))
        except Exception:
            pass
        finally:
            _FUSION_USER.pop("fetching", None)
    _thu.Thread(target=_fetch, daemon=True).start()


def _fusion_modeling_orientation(user_id=None):
    """The current Fusion user's Default Modeling Orientation, read from Fusion's LOCAL
    options file (sceneUpOptionId in NGlobalOptions.xml): 'zup' (the ADOM standard), 'yup'
    (Fusion's factory default, which a FRESH Autodesk account gets), or None if unknown.

    Disk-only - the bridge runs on the same box, so this needs NO add-in and never disturbs a
    running Fusion. John, 2026-08-23: a fresh account (jlauer12) came up Y-up while his other
    account was Z-up; Adom is a Z-up world, so the bridge flags Y-up (especially on new users)
    so it can be corrected with fusion_set_preference {"modelingOrientation":"zup"}."""
    try:
        import os as _os
        uid = user_id or _FUSION_USER.get("userId")
        if not uid:
            return None
        path = _os.path.join(_os.environ.get("APPDATA", ""), "Autodesk",
                             "Neutron Platform", "Options", uid, "NGlobalOptions.xml")
        if not _os.path.exists(path):
            return None
        import re as _re
        m = _re.search(r'sceneUpOptionId[^>]*Value="(\d)"',
                       open(path, "r", encoding="utf-8", errors="ignore").read())
        return {"0": "yup", "1": "zup"}.get(m.group(1)) if m else None
    except Exception:
        return None


def _addin_round_trip_ready(tries: int = 2, gap: float = 1.5) -> bool:
    """Can the add-in actually EXECUTE a command, twice in a row?

    The distinction that issue #36 turned on: a listening socket proves the add-in's HTTP
    thread is up, not that Fusion's main thread will run our command. get_app_state is the
    cheapest real round-trip, so it is the honest readiness test.
    """
    import time as _t
    for i in range(tries):
        try:
            if not _proxy_to_addin("get_app_state", {}, timeout=6).get("success"):
                return False
        except Exception:
            return False
        if i + 1 < tries:
            _t.sleep(gap)
    return True



_LAUNCH_WATCH = [None]


def _start_launch_progress_watch():
    """Keep the taskbar launch bar alive AFTER fusion_start returns stillLaunching.

    Ticks 0->95 against this machine's measured cold-launch estimate from the persisted
    launch stamp, tolerating the window-does-not-exist-yet phase, until the add-in answers a
    real round-trip (bar completes + clears) or 2x estimate + 120s passes (error flash +
    clear). Single instance; respects the activity-indicator mode.
    """
    import threading as _th
    import time as _t
    if _activity_mode() not in ("progress", "both"):
        return
    if _LAUNCH_WATCH[0] and _LAUNCH_WATCH[0].is_alive():
        return
    import timings as _tm

    def _watch():
        from handlers.taskbar_overlay import _taskbar_progress
        try:
            started = float(_get_bridge_pref("launchStartedAt", 0) or _t.time())
        except Exception:
            started = _t.time()
        est, _conf = _tm.estimate("launch", 150, "cold")
        est = est or 150
        give_up = started + 2 * est + 120
        while _t.time() < give_up:
            if _addin_round_trip_ready(tries=1):
                _taskbar_progress("normal", 100)
                _t.sleep(1.0)
                _taskbar_progress("none")
                return
            pct = min(95, int(95 * (_t.time() - started) / est))
            _taskbar_progress("normal", pct)      # fails harmlessly until the window exists
            _t.sleep(2)
        _taskbar_progress("error", 100)
        _t.sleep(1.5)
        _taskbar_progress("none")

    t = _th.Thread(target=_watch, daemon=True)
    _LAUNCH_WATCH[0] = t
    t.start()


def _handle_launch(fusion_info: dict, args: dict) -> dict:
    """Launch Fusion 360 and optionally wait for the AdomBridge add-in."""
    # LIVE detect (not the stale bridge-start fusion_info snapshot) so a launch right
    # after a fresh install is recognized (with a current exe_path) and a launch after a
    # removal fails cleanly. Reassign so the whole handler uses fresh paths.
    fusion_info = detect_fusion()
    if not fusion_info.get("installed"):
        return {
            "success": False,
            "error": "Fusion 360 is not installed on this machine.",
            "errorCode": "fusion_not_installed",
            "_hint": "Fusion 360 isn't installed. Do NOT tell the user to install it themselves - OFFER to install it FOR them and do it on a yes: the fusion-onboarding skill silent-installs Fusion + drives the Autodesk sign-in. (For a plain 'what EDA tools are installed' check, ab's bridge_readiness reports this cleanly without erroring.)",
        }

    import time as _t0
    import timings as _tm
    _warm_start = _t0.time()
    if _is_fusion_running():
        addin_ok = wait_for_addin(timeout=5)
        # MEASURED, not asserted (issue #32): a warm start is a real sample too, and
        # it is the number that lets a UI say "3s warm" instead of hedging with the
        # cold-start worst case on every single launch.
        _el = _t0.time() - _warm_start
        if addin_ok:
            _tm.record("launch", _el, "warm")
        # ⚠️ success:true WITH addinConnected:false IS A LIE (page issue #39). This verb's
        # contract says it blocks until the add-in is ready, and fusion_get_app_state's hint
        # tells callers to rely on that - so a caller does the documented thing, reads
        # success:true, retries its real command and fails. The field they check said yes
        # while the field that mattered said no.
        #
        # DOCTRINE: the add-in only loads AT LAUNCH, so "running but no add-in" is not
        # something more waiting can fix - it needs a restart, and the bridge performs it
        # rather than returning a cheerful false positive. Fall through to the cold path.
        # ⛔ BUSY IS NOT MISSING - AND THIS PATH IS DESTRUCTIVE (page issue #41). The #39 fix
        # force-restarts when the add-in does not answer. But an add-in mid-way through a long
        # command does not answer EITHER, and a surface has measured 546s here. ab's loop read
        # an unparseable (busy) probe as "not running", called fusion_start, and this code
        # killed a perfectly healthy Fusion doing real work - destroying the very run it was
        # meant to rescue. 1.9.94 taught the not-responding path to check /status for busy; I
        # failed to apply it to the one path where being wrong DESTROYS something. Check first.
        _busy = None
        if not addin_ok:
            try:
                _st = _check_addin_status(timeout=3.0)
                _busy = _st if (_st and _st.get("busy")) else None
            except Exception:
                _busy = None
        if _busy:
            return {
                "success": False,
                "error": "Fusion is running and the add-in is BUSY with a long command - NOT "
                         "restarting it. Waiting is the remedy here, not a relaunch.",
                "errorCode": "fusion_addin_busy",
                "busyCommand": _busy.get("busyCommand"),
                "elapsedSeconds": _busy.get("elapsedSeconds"),
                "alreadyRunning": True,
                "startKind": "warm",
                "_hint": ("Do NOT call fusion_start again on this - it is exactly how a healthy "
                          "run gets killed. Poll fusion_readiness (it answers while the add-in "
                          "is busy) and compare elapsedSeconds against fusion_state.timings for "
                          "this machine; only past roughly 2x p90 is a restart justified."),
            }
        if not addin_ok and _user_restart_hold_active():
            return {"success": False, "errorCode": "user_owns_restart",
                    "alreadyRunning": True, "startKind": "warm",
                    "error": "Fusion is running without its add-in, and the user chose "
                             "'I'll handle it' on the restart notice - so the bridge will NOT "
                             "restart Fusion. The user owns that restart now.",
                    "_hint": "Do not call fusion_start or fusion_restart_for_addin. Wait for "
                             "the user to restart Fusion themselves (the hold releases the "
                             "moment the add-in answers), or ask them again with "
                             "fusion_restart_for_addin only if THEY change their mind."}
        if not addin_ok:
            _bridge_action_ledger("fusion_restart_for_silent_addin_considered",
                                  {"reason": "running but add-in not answering; entering persistence gate"})
        if not addin_ok:
            # ⛔ PERSISTENCE GATE (John, 2026-08-22, live during the ab library-matrix run):
            # "you better not be exiting fusion between library imports." A single missed 5s
            # probe between back-to-back imports is NOT "the add-in never loaded" - Fusion's
            # main thread can be digesting the previous import (the command already returned,
            # so /status shows busy:false) and answer again seconds later. Killing on one
            # missed probe exits Fusion mid-matrix. Re-probe over ~30s; kill ONLY if the
            # add-in stays dead the WHOLE window and never reports busy.
            for _re in range(3):
                _t0.sleep(8)
                if wait_for_addin(timeout=5):
                    addin_ok = True
                    break
                try:
                    _st2 = _check_addin_status(timeout=3.0)
                except Exception:
                    _st2 = None
                if _st2 and _st2.get("busy"):
                    return {
                        "success": False,
                        "error": "Fusion is running and the add-in reported BUSY during the "
                                 "persistence re-probe - NOT restarting it.",
                        "errorCode": "fusion_addin_busy",
                        "busyCommand": _st2.get("busyCommand"),
                        "elapsedSeconds": _st2.get("elapsedSeconds"),
                        "alreadyRunning": True,
                        "startKind": "warm",
                        "_hint": ("Do NOT call fusion_start again - poll fusion_readiness and "
                                  "wait; a busy add-in answers again when its command finishes."),
                    }
        if not addin_ok:
            print("[Fusion Bridge] warm start found NO add-in after a ~30s persistence window "
                  "(never busy, never answered) - restarting Fusion so the add-in loads "
                  "(issue #39, #41 busy guard, + the 2026-08-22 library-matrix persistence gate)")
            try:
                handle_fusion_kill(fusion_info, {})   # a non-serving add-in will not close politely
                _t0.sleep(3)                          # brief settle; the launch below is what matters
            except Exception:
                pass
            fusion_info = detect_fusion()
        else:
            return {
                "success": True,
                "output": "Fusion 360 is already running.",
                "addinConnected": addin_ok,
                "alreadyRunning": True,
                "startKind": "warm",
                "progress": _tm.terminal_block("launch", _warm_start,
                                               "Fusion is already running - reusing it",
                                               start_kind="warm", failed=False),
                "timings": _tm.summary().get("launch"),
                "resolvedPath": fusion_info.get("exe_path", ""),
            }

    # Relocate any crash recovery files BEFORE launching Fusion.
    # This prevents the "Recovered Documents" dialog from appearing at all.
    # Files are moved to ~/.adom/recovery/fusion/<timestamp>/ (not deleted).
    from handlers.dismiss_recovery import relocate_recovery_files
    reloc = {"moved": 0, "dest": None}
    try:
        reloc = relocate_recovery_files()
    except Exception:
        pass

    # Launch Fusion. ab's relay caps a single request at ~60s, but a FIRST launch
    # (fresh install: component downloads, updates, cloud sync) can take 2-4 min -
    # far past the cap. So we DON'T block on the add-in for the whole launch (that
    # dead-ended #63/Arav, and {"timeout":300} can't beat the relay cap). Instead:
    # launch + confirm the process, then wait for the add-in only within a budget
    # that keeps this request UNDER the relay cap; if it's still coming up, return a
    # clean stillLaunching response telling the caller to POLL fusion_readiness.
    import time as _t
    # BUDGET FROM ENTRY, NOT FROM HERE (caught regression-testing #39): the warm probe and
    # a force-restart already burned seconds before we got to this line. Measuring the
    # relay budget from here made the whole verb overrun ab's 90s window and time out -
    # trading a false success for a timeout is not a fix. _warm_start is function entry.
    _start = _warm_start
    # POLLABLE (issue #32 section 3): stamp the launch so fusion_readiness - the verb the
    # caller polls while this one returns stillLaunching - can report the SAME measured
    # progress block. Sticky in prefs so a bridge respawn mid-launch does not lose the bar.
    try:
        _set_bridge_pref("launchStartedAt", _start)
    except Exception:
        pass
    err = ensure_fusion_running(fusion_info, wait_addin=False)
    if err:
        return err

    # WATCH for launch/licensing dialogs the moment the process is up - IN CODE, so
    # a launch is never blind to them (the seat-conflict + streamed-app-error dialogs
    # are owned by AdskIdentityManager/FusionLauncher, invisible to the Fusion-scoped
    # classifier). Auto-dismiss BENIGN errors.
    launch_dialogs = classify_launch_dialogs()
    for _dlg in launch_dialogs:
        if _dlg.get("category") in AUTO_DISMISS_CATEGORIES:
            close_dialog_bg(_dlg.get("hwnd"))  # benign ack (WM_CLOSE == Cancel/OK)

    # ── DETERMINISTIC launch state loop (John 2026-07-06: track + handle state in
    #    CODE, in the BACKGROUND, never punt to the user, never foreground). Poll the
    #    add-in round-trip (the ground truth for "drivable") while, each tick:
    #      • AUTO-RESOLVING the seat/licensing dialog ("Active Sessions Exceeded") via
    #        UIA Invoke of its 'Continue' button - background, no foreground (THE fix;
    #        Suspend is pre-selected so Continue grabs the license). This is what
    #        un-sticks the launch: a blocked seat dialog made Fusion retry sign-in in a
    #        loop, foregrounding on every retry - the source of the endless fg-steal.
    #      • dismissing benign recovery/startup dialogs.
    #    Detection is deterministic (find_licensing_dialog by owning process + size,
    #    NOT the "Fusion360" title). No kill/relaunch (the server keeps the seat), no
    #    notify (we handle it ourselves). ──
    _RELAY_BUDGET = 50  # keep total handler time under ab's ~60s relay cap
    _deadline = _t.time() + max(6, int(_RELAY_BUDGET - (_t.time() - _start)))
    _seat_clicks = 0
    addin_ok = False
    while _t.time() < _deadline:
        # SEAT/LICENSING dialog takes PRIORITY over the add-in probe (fix, caught live
        # 2026-07-06): a STALE add-in port from a prior instance can answer while the
        # CURRENT Fusion sits blocked behind the seat dialog - so we must NOT break on
        # the add-in while the dialog is up. And the first UIA Invoke on a mid-render
        # dialog silently NO-OPs, so keep invoking 'Continue' EVERY tick until
        # find_licensing_dialog no longer sees it (verified gone), not just once.
        try:
            _lic = find_licensing_dialog()
        except Exception:
            _lic = None
        if _lic:
            # Resolve + SCREENSHOT-VERIFY it cleared (parent ownedPopupCount), not a
            # size heuristic that false-'resolved' the short confirm variant.
            if _seat_clicks < 10:
                _r = _resolve_seat_dialog(max_clicks=3)
                _seat_clicks += _r.get("clicks", 0)
            _t.sleep(1.0)
            continue
        # No seat dialog -> is the add-in actually serving? (ground truth for drivable)
        #
        # ⚠️ A PORT THAT ANSWERS IS NOT AN ADD-IN THAT CAN WORK (page issue #36, found by
        # ralph-testing the web-control page): `_probe_addin` proves a socket replied, and
        # this verb's own hint promises it "blocks until the add-in is ready". A caller
        # reasonably took that at face value, fired the next verb, and got "add-in not
        # responding" - three surface buttons failed in that window, and minutes later the
        # identical calls worked. So require a REAL command round-trip (get_app_state, the
        # cheapest thing the add-in actually executes on its main thread), and require it
        # TWICE: the first success can land while the add-in is still finishing startup,
        # and one more tick is far cheaper than handing the caller a false ready.
        if _probe_addin(timeout=1.5) and _addin_round_trip_ready():
            addin_ok = True
            # Give the human a window worth looking at (background, no activation).
            try:
                if _window_sizing_enabled():
                    _sz = ensure_reasonable_window()
                    if _sz.get("sized"):
                        print("[Fusion Bridge] window sized: %s" % _sz.get("why"))
            except Exception:
                pass
            break
        try:
            dismiss_recovery_dialog()
        except Exception:
            pass
        _t.sleep(2.5)

    if not addin_ok:
        # Add-in not up within our budget. On a first launch this is EXPECTED (still
        # initializing / signing in), not a failure and not a retry-able timeout.
        # ISSUE #32: this is THE response John quoted. It now carries a measured
        # progress block (percent + eta + cold/warm, seeded from this machine's own
        # rolling history) so the caller draws a real bar instead of printing a shrug.
        _prog = _tm.progress_block("launch", _start,
                                   "Fusion is starting - waiting for the Adom add-in to serve",
                                   start_kind="cold", fallback=150)
        # The RELAY window ends here but the LAUNCH does not - so the taskbar bar must not
        # die with the verb. Hand it to a watcher that keeps ticking against the launch
        # stamp until the add-in genuinely round-trips, then completes and clears (or flags
        # error and clears if the launch dies). The whole point of the bar is the slow part,
        # which is precisely the part after this return.
        try:
            _start_launch_progress_watch()
        except Exception:
            pass
        return {
            "success": True,
            "stillLaunching": True,
            "addinConnected": False,
            "startKind": "cold",
            "progress": _prog,
            "timings": _tm.summary().get("launch"),
            "statusVerb": "fusion_readiness",
            "recoveryFilesRelocated": reloc["moved"],
            "resolvedPath": fusion_info.get("exe_path", ""),
            "seatDialogResolved": _seat_clicks > 0,
            "backgroundLaunch": True,
            "_hint": (
                "Fusion was launched IN THE BACKGROUND (minimized, foreground-lock on) so it does not "
                "steal the user's focus - do NOT foreground it. "
                + ("A seat 'Active Sessions Exceeded' dialog appeared and was AUTO-RESOLVED in the "
                   "background via UIA (no foreground). " if _seat_clicks else "")
                + ((_tm.sentence() + ". ") if _tm.sentence() else "")
                + (("The add-in isn't up yet - about %ss remaining by this machine's own measured "
                    "history (read progress.percent / progress.etaSec on this response; the estimate "
                    "is revised as the launch proceeds). This is " % _prog.get("etaSec"))
                   if _prog.get("etaSec") is not None else
                   "The add-in isn't up yet and this machine has no measured launch history yet, so "
                   "progress.confidence is 'typical' until it learns. This is ")
                + "NOT a failure and NOT a retry-able timeout: do NOT re-call fusion_start. WHAT HAPPENS "
                "NEXT: POLL fusion_readiness every ~10s; it returns needsSignin/licensingDialog/updating "
                "if something is mid-flight (the bridge handles the seat dialog itself), and ready:true "
                "when it is drivable - then run your fusion_* verbs. NEVER poll blind past ~2 min: "
                "desktop_screenshot_window the Fusion hwnd and READ the response's screenshots[] array "
                "- every OWNED POPUP rides back as its own child shot (this is how you SEE a blocking "
                "dialog; never desktop_screenshot_screen, the user is doing other things)."
            ),
        }

    # Auto-dismiss the "Recovered Documents" dialog if it appeared on launch.
    # Even though we relocated files above, Fusion may still show recovery
    # dialogs if files were locked or if Fusion cached them.
    recovery_dismissed = dismiss_recovery_dialog()

    # Probe the add-in to check if a modal dialog is blocking.
    # If Fusion has a "Recovered Documents" or other modal up, the add-in's
    # main thread will be blocked and commands will timeout. We detect this
    # PROACTIVELY so the AI knows immediately, rather than waiting for a
    # 30-second command timeout.
    dialog_blocking = False
    if addin_ok:
        dialog_blocking = _check_main_thread_blocked()

    # TERMINAL FRAME CARRIES THE ACTUALS (issue #32 section 4): the real total goes
    # into this machine's rolling history, which is what makes the NEXT launch's
    # estimate "measured" instead of a literal someone typed.
    _tm.record("launch", _t.time() - _start, "cold")
    try:
        _set_bridge_pref("launchStartedAt", None)
    except Exception:
        pass

    output = "Fusion 360 launched successfully."
    if reloc["moved"] > 0:
        output += (
            f" Relocated {reloc['moved']} recovery file(s) to {reloc['dest']}"
            " (preserved for manual recovery if needed)."
        )
    if recovery_dismissed:
        output += " Dismissed 'Recovered Documents' dialog."
    if dialog_blocking:
        output = (
            "Fusion 360 launched but a MODAL DIALOG is blocking the UI. "
            "This is likely the 'Recovered Documents' dialog from a previous crash. "
            "Take a screenshot with desktop_screenshot_window to see what's showing, "
            "then dismiss it manually or use fusion_dismiss_recovery."
        )

    return {
        "success": True,
        "output": output,
        "addinConnected": addin_ok,
        "dialogBlocking": dialog_blocking,
        "recoveryDialogDismissed": recovery_dismissed,
        "recoveryFilesRelocated": reloc["moved"],
        "recoveryFilesPath": reloc["dest"],
        "startKind": "cold",
        "progress": _tm.terminal_block("launch", _start, "Fusion is up and the add-in is serving",
                                       start_kind="cold", failed=not addin_ok),
        "timings": _tm.summary().get("launch"),
        "resolvedPath": fusion_info.get("exe_path", ""),
    }


def _handle_dismiss_recovery(fusion_info: dict, args: dict) -> dict:
    """Dismiss the 'Recovered Documents' dialog if it's showing.

    Recovery files are relocated to ~/.adom/recovery/fusion/<timestamp>/
    instead of being deleted, preserving the user's safety net.
    """
    from handlers.dismiss_recovery import relocate_recovery_files

    # Relocate files first (dismiss_recovery_dialog also does this,
    # but we capture the result here for reporting)
    reloc = {"moved": 0, "dest": None}
    try:
        reloc = relocate_recovery_files()
    except Exception:
        pass

    dismissed = dismiss_recovery_dialog()

    parts = []
    if dismissed:
        parts.append("Dismissed recovery dialog(s).")
    else:
        parts.append("No recovery dialog found.")
    if reloc["moved"] > 0:
        parts.append(
            f"Relocated {reloc['moved']} recovery file(s) to {reloc['dest']}. "
            "The user can restore them from there if needed."
        )

    return {
        "success": True,
        "output": " ".join(parts),
        "dismissed": dismissed,
        "recoveryFilesRelocated": reloc["moved"],
        "recoveryFilesPath": reloc["dest"],
    }


def _post_open_screenshot(result: dict, settle_time: float = 2.0) -> dict:
    """Auto-screenshot Fusion after an open/switch command.

    Waits for Fusion to settle, then captures the full window (including
    any Qt dialogs or CEF overlays the API can't see). Attaches the
    screenshot path to the result so the AI can inspect it immediately
    without making a separate call.

    The screenshot is embedded into the result's 'data' field because the
    Tauri relay only forwards 'output', 'data', 'success', and 'error'.

    Also checks for blocking dialogs and warns in the response.
    """
    import re
    import time
    time.sleep(settle_time)

    # Check if main thread is blocked (dialog present)
    blocked = _check_main_thread_blocked()

    # Capture main Fusion window
    screenshot = screenshot_fusion_window()
    screenshot_info = {"screenshots": []}

    if screenshot.get("success"):
        screenshot_info["screenshots"].append({
            "type": "main_window",
            "savedTo": screenshot["savedTo"],
            "sizeKB": screenshot["sizeKB"],
        })
    else:
        screenshot_info["error"] = screenshot.get("error", "Screenshot failed")

    # Also capture ALL dialog windows — these are separate Qt windows
    # that the main window screenshot won't show
    window_info = get_fusion_window_info()
    if window_info.get("success"):
        dialogs = window_info.get("dialogs", [])
        for dialog in dialogs:
            try:
                title = dialog.get("title", "")
                hwnd = dialog.get("hwnd")
                # Skip tiny/hidden windows and banners (only screenshot real dialogs)
                rect = dialog.get("rect", {})
                w = rect.get("width", 0)
                h = rect.get("height", 0)
                if w < 50 or h < 50:
                    continue
                # Screenshot this dialog — sanitize label for filesystem
                label = re.sub(r'[<>:"/\\|?*]', '', title).replace(" ", "_")[:30]
                dialog_ss = screenshot_hwnd(hwnd, label=f"dialog-{label}")
                if dialog_ss.get("success"):
                    screenshot_info["screenshots"].append({
                        "type": "dialog",
                        "title": title,
                        "hwnd": hwnd,
                        "savedTo": dialog_ss["savedTo"],
                        "sizeKB": dialog_ss["sizeKB"],
                    })
            except Exception as e:
                screenshot_info.setdefault("errors", []).append(
                    f"Failed to screenshot dialog '{title}': {e}"
                )

    screenshot_info["message"] = (
        f"Auto-captured {len(screenshot_info['screenshots'])} Fusion window(s) after open. "
        "READ EACH screenshot file to check for blocking dialogs "
        "('What to design?', 'PCB out of date', 'Save changes?'). "
        "PCB/ELECTRONICS NAV RULE: a board is NOT a standalone file - the schematic, 2D board and 3D board are VIEWS inside ONE electronics DESIGN. Open the DESIGN document first (fusion_open_cloud_file or fusion_open_by_urn), THEN switch to its schematic and its 2D board (fusion_show_2d_board) and 3D board (fusion_show_3d_board). A Select-Electronics-Design-File picker IS that design list of its schematic + board. An EMPTY 2D board usually means you opened a derivative or the wrong file, or it is out of sync - reopen the parent electronics DESIGN and switch to its board view; do NOT open a .brd or .sch as a standalone file. "
        "Dismiss with fusion_send_key {\"key\": \"escape\"} if needed."
    )

    if blocked:
        screenshot_info["dialogBlocking"] = True
        screenshot_info["message"] = (
            "WARNING: A modal dialog is blocking Fusion. "
            "READ the screenshot to identify and dismiss it. " +
            screenshot_info["message"]
        )

    # Inject screenshot into the result's top-level 'data' dict.
    # The Tauri relay constructs its output as: json!({"message": output, "data": data})
    # so anything we put in result["data"] gets forwarded to the CLI.
    result.setdefault("data", {})
    if not isinstance(result["data"], dict):
        result["data"] = {}
    result["data"]["postOpenScreenshot"] = screenshot_info

    # UNIVERSAL: if an owned "Error" popup is up (e.g. malformed .lbr "has errors and
    # cannot be opened"), flip success -> False + surface it. An open that silently
    # errored must never be reported as ok.
    return _apply_failure_dialogs(result)


# ── Fusion's OWN error toasts, read programmatically (John, 2026-09-01) ──────
# Fusion draws many errors as CEF overlays INSIDE the canvas: no top-level window, so
# neither Win32 enumeration nor UIA can see them, and check_dialogs reported "no dialogs"
# while an Error box sat on the user's screen ("Cannot create a managed library from a
# document not saved in a Fusion Hub"). Fusion DOES log every one of them to its own
# AppLogFile as a `New Alert message` line, so the bridge reads that instead of guessing
# from pixels. Bridge-side on purpose: it works with the add-in wedged and needs no
# Fusion restart to deploy.
_ALERT_RE = re.compile(r"New Alert message\.\s*(?:Tooltip:\s*(?P<tip>.*?))?"
                       r"(?:,\s*Short Message:\s*(?P<short>.*?))?"
                       r"(?:,\s*Long Message:\s*(?P<long>.*?))?\s*$")


def _fusion_log_path(diag: dict = None):
    """Newest Fusion AppLogFile, or None. `diag` (optional dict) is filled with WHY it
    failed - a swallowed reason is how this shipped broken the first time."""
    d_ = diag if diag is not None else {}
    bases = []
    try:
        if sys.platform == "darwin":
            # macOS: the same tree under ~/Library/Application Support, but the per-user folder is
            # an install id we cannot guess - discover it instead. It is hex on some builds and a
            # 12-char alnum id (URQPRNR2EGYK) on others, so match alnum-with-a-digit: that keeps
            # real subdirs (CrashRecovery, Electron, Materials, ...) out of the candidate list.
            mac_root = os.path.join(os.path.expanduser("~"), "Library", "Application Support",
                                    "Autodesk", "Autodesk Fusion 360")
            bases.append(mac_root)
            try:
                for uid in sorted(os.listdir(mac_root)):
                    uid_dir = os.path.join(mac_root, uid)
                    if re.fullmatch(r"[0-9A-Za-z]{8,}", uid) and any(c.isdigit() for c in uid) \
                            and os.path.isdir(uid_dir):
                        bases.append(os.path.join(uid_dir, "logs"))
                        bases.append(uid_dir)
            except Exception:
                pass
        lad = os.environ.get("LOCALAPPDATA") or os.path.join(
            os.path.expanduser("~"), "AppData", "Local")
        for nm in ("Autodesk Fusion 360", "Autodesk Fusion"):
            bases.append(os.path.join(lad, "Autodesk", nm))
        # A bridge running as another user still finds the interactive user's logs.
        users = os.path.dirname(os.path.expanduser("~"))
        if os.path.isdir(users):
            for u in os.listdir(users):
                for nm in ("Autodesk Fusion 360", "Autodesk Fusion"):
                    bases.append(os.path.join(users, u, "AppData", "Local", "Autodesk", nm))
    except Exception as e:
        d_["baseError"] = str(e)[:160]
    d_["basesTried"] = bases[:6]
    best = None
    for base in bases:
        try:
            if not os.path.isdir(base):
                continue
            d_.setdefault("baseFound", base)
            for entry in os.listdir(base):
                ld = os.path.join(base, entry, "logs")
                if not os.path.isdir(ld):
                    continue
                for f in os.listdir(ld):
                    if f.startswith("AppLogFile") and f.endswith(".log"):
                        fp = os.path.join(ld, f)
                        try:
                            m = os.path.getmtime(fp)
                        except Exception:
                            continue
                        if best is None or m > best[0]:
                            best = (m, fp)
        except Exception as e:
            d_.setdefault("scanError", "%s: %s" % (base[-40:], str(e)[:120]))
    if best is None:
        d_.setdefault("why", "no AppLogFile*.log found under any base")
    return best[1] if best else None


def _fusion_alerts(since_sec: float = 300.0, tail_bytes: int = 400000) -> list:
    """Fusion's own alert toasts from the last `since_sec` seconds.

    Returns [{at, text}] newest last. Never raises."""
    import time as _t, datetime as _dt
    path = _fusion_log_path()
    if not path:
        return []
    try:
        size = os.path.getsize(path)
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            if size > tail_bytes:
                fh.seek(size - tail_bytes)
                fh.readline()
            lines = fh.readlines()
    except Exception:
        return []
    cutoff = _t.time() - max(1.0, since_sec)
    out = []
    for line in lines:
        if "New Alert message" not in line:
            continue
        stamp = None
        m0 = re.match(r"\s*(\d{8}T\d{6})", line)
        if m0:
            try:
                stamp = _dt.datetime.strptime(m0.group(1), "%Y%m%dT%H%M%S").timestamp()
            except Exception:
                stamp = None
        if stamp is not None and stamp < cutoff:
            continue
        m = _ALERT_RE.search(line)
        text = ""
        if m:
            text = (m.group("short") or m.group("tip") or "").strip()
        if not text:
            text = line.split("New Alert message.", 1)[-1].strip()[:400]
        out.append({"at": m0.group(1) if m0 else None, "text": text[:400]})
    return out


def _alert_watermark() -> str:
    """The stamp of Fusion's most recent alert RIGHT NOW.

    Every managed-library verb used to judge itself with _fusion_alerts(120), which reads a
    120-second window and therefore blamed an action for an alert raised by whatever happened
    two minutes earlier - a clean unlink reported failure because of a stale "you decided not
    to synchronize" toast from a prior dialog. Take this before an action and pass it to
    _alerts_since so a verb only ever sees its OWN alerts."""
    rows = _fusion_alerts(3600.0)
    return (rows[-1].get("at") or "") if rows else ""


def _alerts_since(watermark: str, since_sec: float = 300.0) -> list:
    """Alerts raised strictly AFTER the watermark stamp (see _alert_watermark)."""
    rows = _fusion_alerts(since_sec)
    if not watermark:
        return rows
    return [r for r in rows if (r.get("at") or "") > watermark]


def _with_fusion_alerts(resp: dict, since_sec: float = 300.0) -> dict:
    """Attach Fusion's own alert toasts to a dialog report, so an in-canvas CEF error is
    never invisible just because it has no window (John, 2026-09-01)."""
    try:
        alerts = _fusion_alerts(since_sec)
        if alerts:
            resp = dict(resp or {})
            resp["fusionAlerts"] = alerts
            resp["_hint"] = ("Fusion raised %d alert(s) in the last %ds - the newest is %r. "
                             "These are in-canvas CEF overlays with no top-level window, so "
                             "they do NOT appear in dialogs[]; read fusionAlerts and treat the "
                             "driving command as FAILED unless you know otherwise."
                             % (len(alerts), int(since_sec), alerts[-1].get("text", "")[:160]))
        elif not (resp or {}).get("dialogs"):
            resp = dict(resp or {})
            resp["fusionAlerts"] = []
            resp["_hint"] = ("No Fusion dialogs, owned popups, or alert toasts in the last "
                             "%ds." % int(since_sec))
    except Exception:
        pass
    return resp


def _capture_dialog_array(settle: float = 0.6) -> dict | None:
    """After a mutating op, enumerate + screenshot any owned popups / modal dialogs so
    the AI can SEE and ANALYZE them. Titles alone are not enough: a dialog's body text
    lives in CEF/Qt child controls that are NOT Win32-readable, so we attach a per-dialog
    screenshot (hwnd-targeted PrintWindow, background - never foreground/fullscreen).

    Cheap when nothing is up: it enumerates windows (Win32 EnumWindows) and only
    screenshots when a real dialog is present. Returns None when no dialog is up;
    otherwise {dialogsDetected, dialogs:[{hwnd,title,category,resolution,screenshot}],
    _hint}. Never raises - best effort.
    """
    import re as _re
    import time as _t
    if settle:
        _t.sleep(settle)
    try:
        info = get_fusion_window_info()
    except Exception:
        return None
    if not info.get("success"):
        return None
    raw = info.get("dialogs") or []
    if not raw:
        return None
    main_enabled = info.get("mainEnabled", True)
    try:
        classified = {c.get("hwnd"): c for c in classify_blocking_dialogs()}
    except Exception:
        classified = {}
    # Categories that mean "we could not identify it by title" - i.e. a generic
    # "Fusion360"-titled window. When the main window is still ENABLED, such a
    # window is a docked panel (Browser/Timeline), NOT a blocking modal, so we
    # suppress it to avoid crying wolf on every op. A real modal DISABLES the main
    # window (main_enabled False) and is always surfaced; known dialogs (recovery,
    # update, save prompts) are surfaced regardless of the enabled state.
    _GENERIC = {None, "generic_modal_read_screenshot", "unknown"}
    out = []
    for d in raw:
        rect = d.get("rect", {})
        if rect.get("width", 0) < 80 or rect.get("height", 0) < 50:
            continue  # skip banners / tiny docked tool windows
        hwnd = d.get("hwnd")
        title = d.get("title", "")
        c = classified.get(hwnd) or {}
        cat = c.get("category")
        if main_enabled and cat in _GENERIC:
            continue  # docked panel, not a modal - nothing is blocking the main window
        entry = {"hwnd": hwnd, "title": title}
        if cat:
            entry["category"] = cat
            entry["resolution"] = c.get("resolution")
        try:
            label = _re.sub(r'[<>:"/\\|?*]', "", title).replace(" ", "_")[:30] or "dialog"
            ss = screenshot_hwnd(hwnd, label=f"dlg-{label}")
            if ss.get("success"):
                entry["screenshot"] = ss["savedTo"]
        except Exception:
            pass
        out.append(entry)
    if not out:
        return None
    return {
        "dialogsDetected": len(out),
        "dialogs": out,
        "mainWindowEnabled": main_enabled,
        "_hint": (
            f"⚠️ {len(out)} Fusion dialog(s)/owned popup(s) are up after this operation. "
            "STOP and ANALYZE before the next call: READ each dialogs[].screenshot and use its "
            "category/resolution. Do NOT blind-dismiss - some confirms LOSE work (clicking Yes on a "
            "Hub-upload 'are you sure you want to close?' destroys the uploaded packages). Take the "
            "work-preserving action (often: click No, then wait for the upload to drain). See the "
            "fusion-driving skill."
        ),
    }


def _merge_dialog_array(result: dict) -> dict:
    """Attach the post-op dialog array (if any) to a mutating verb's result, so the AI
    always sees pending dialogs WITHOUT having to remember a separate call. The hint is
    mirrored to top-level _hint AND into data.dialogArray (data is forwarded reliably by
    the relay). No-op when no dialog is up."""
    if not isinstance(result, dict):
        return result
    try:
        arr = _capture_dialog_array()
    except Exception:
        arr = None
    if not arr:
        return result
    result.setdefault("data", {})
    if isinstance(result.get("data"), dict):
        result["data"]["dialogArray"] = arr
    existing = result.get("_hint")
    result["_hint"] = arr["_hint"] + ((" | " + existing) if existing else "")
    return _apply_failure_dialogs(result)


def _apply_failure_dialogs(result: dict) -> dict:
    """UNIVERSAL error-dialog guard — baked into every mutating/open verb so the AI
    can never again report a failed op as success.

    Right after an operation, an owned Fusion popup titled "Error" (e.g. "<file>.lbr
    has errors and cannot be opened") means the op FAILED even when the underlying API
    call returned ok (Fusion opened an EMPTY design behind the error). This classifies
    any up dialog; if one is in FAILURE_CATEGORIES it:
      1. SCREENSHOTS the popup (hwnd-targeted, background) so the AI SEES it,
      2. flips result.success -> False + sets errorCode/error/_hint,
      3. dismisses the pure-ack popup (WM_CLOSE) so it can't block the next command.
    Best-effort + cheap when nothing is up (title enumeration, no screenshot). Never raises."""
    try:
        from handlers.dialog_classify import classify_blocking_dialogs, FAILURE_CATEGORIES
        dialogs = classify_blocking_dialogs()
    except Exception:
        return result
    failures = [d for d in (dialogs or []) if d.get("category") in FAILURE_CATEGORIES]
    if not failures or not isinstance(result, dict):
        return result
    f = failures[0]
    shots = []
    for d in failures:
        try:
            ss = screenshot_hwnd(d.get("hwnd"), label="error-dialog")
            if ss.get("success"):
                shots.append(ss["savedTo"])
        except Exception:
            pass
    prev_err = result.get("error") or ""
    result["success"] = False
    result["errorCode"] = "fusion_operation_error"
    result["error"] = (
        f"Operation FAILED - Fusion '{f.get('title') or 'Error'}' dialog is up"
        + (f" (prior: {prev_err})" if prev_err else "")
    )
    result["_hint"] = (f.get("resolution") or
                       "Fusion shows an Error dialog; the operation failed. Read the screenshot.")
    result.setdefault("data", {})
    if isinstance(result.get("data"), dict):
        result["data"]["errorDialog"] = {
            "title": f.get("title"), "category": f.get("category"),
            "resolution": f.get("resolution"), "screenshots": shots,
        }
    # Pure OK-acknowledgement — dismiss so it doesn't wedge the next command.
    for d in failures:
        try:
            close_window(d.get("hwnd"))
        except Exception:
            pass
    return result


def _handle_open_cloud_file(fusion_info: dict, args: dict) -> dict:
    """Open a cloud file with post-open dialog detection + auto-screenshot.

    Proxies to the add-in's open_cloud_file command, then:
    1. Waits for Fusion to settle
    2. Checks if a modal dialog blocked the main thread
    3. Auto-screenshots the Fusion window so the AI can see what happened
    """
    # Send the open command — cloud files can take 30+ seconds to download/open
    result = _proxy_to_addin("open_cloud_file", args, timeout=45)

    if not result.get("success"):
        # Failure is often a modal blocking the add-in's main thread (multi-design
        # "Select Electronics Design File" picker, update nag, recovery, etc.) — which
        # otherwise surfaces as a misleading "add-in not responding". Classify the
        # actual blocking dialog so the caller gets the real cause + resolution.
        dialogs = classify_blocking_dialogs()
        if dialogs:
            result["dialogBlocking"] = True
            result["blockingDialogs"] = dialogs
            result["hint"] = (
                "A modal dialog is blocking Fusion — see blockingDialogs for the "
                "classified cause + resolution. The multi-link 'Select Electronics "
                "Design File' picker is a CEF dialog that must be resolved on the "
                "desktop (its list isn't keyboard/Win32 navigable); recovery prompts "
                "use fusion_dismiss_recovery; an update nag must be cleared on the desktop."
            )
        # Screenshot even on failure — shows what went wrong
        return _post_open_screenshot(result, settle_time=1.0)

    return _post_open_screenshot(result)


def _handle_open_with_screenshot(fusion_info: dict, args: dict, command: str) -> dict:
    """Proxy an open/switch command to the add-in, then auto-screenshot.

    Used for open_schematic, open_board, show_3d_board, show_2d_board —
    any command that changes the Fusion UI state and might trigger a dialog.
    """
    result = _proxy_to_addin(command, args, timeout=30)
    if result.get("success"):
        return _post_open_screenshot(result)
    return result


def _handle_screenshot_fusion(fusion_info: dict, args: dict) -> dict:
    """Screenshot Fusion main window or a specific dialog by HWND."""
    hwnd = args.get("hwnd") or args.get("dialogHwnd")
    if hwnd:
        result = screenshot_hwnd(int(hwnd), label="dialog")
    else:
        result = screenshot_fusion_window()
    if result.get("success"):
        # This bridge natively names the shot `savedTo`/`sizeKB`/`dimensions`, but ab's CLI
        # auto-pulls a screenshot into the CALLER'S container by keying on the ab-core field
        # names (fullPath/safePath/fullPathHost/safePathHost + fullWidth/fullHeight). Without
        # those, `savedTo` is a Windows-only path the container can't read and the shot looks
        # "null" from the cloud (fusion-bridge#: field-name mismatch, John 2026-08-22). Alias
        # them so the pull fires and callers get localSafePath/localFullPath just like desktop_*.
        _saved = result.get("savedTo")
        if _saved:
            for _k in ("fullPath", "safePath", "fullPathHost", "safePathHost"):
                result.setdefault(_k, _saved)
        _dims = result.get("dimensions") or {}
        if _dims.get("width"):
            result.setdefault("fullWidth", _dims.get("width"))
            result.setdefault("fullHeight", _dims.get("height"))
        result["output"] = json.dumps({
            "savedTo": _saved, "sizeKB": result.get("sizeKB"),
            "fullPath": _saved, "safePath": _saved,
            "fullPathHost": _saved, "safePathHost": _saved,
            "fullWidth": _dims.get("width"), "fullHeight": _dims.get("height"),
        })
    return result


def _handle_click_fusion(fusion_info: dict, args: dict) -> dict:
    """Click at coordinates within Fusion window or a specific dialog HWND."""
    x = args.get("x", 0.5)
    y = args.get("y", 0.5)
    relative = args.get("relative", True)
    hwnd = args.get("hwnd") or args.get("dialogHwnd")
    # SendInput clicks REQUIRE foreground (steals the user's focus). Announce it first -
    # prefer background verbs (desktop_ui_click by name); this path is the last resort.
    _notify_before_foreground(args.get("reason") or "clicking a control Fusion only accepts via a foreground click")
    result = click_fusion(x, y, relative, hwnd=hwnd)
    if result.get("success"):
        result["output"] = json.dumps(result.get("clickedAt", {}))
    return result


def _handle_send_key(fusion_info: dict, args: dict) -> dict:
    """Send a key to Fusion or a dialog - BACKGROUND PostMessage first (the KiCad-bridge
    lesson: real window messages beat foreground+SendInput); SendInput only on explicit
    {"method": "sendinput"} or when the posted key visibly failed, and then ALWAYS with
    the user-facing caption explaining WHY (John's rule, 2026-08-15)."""
    key = args.get("key", "")
    if not key:
        return {"success": False, "error": "No key specified. Use 'enter', 'escape', 'tab', etc."}
    hwnd = args.get("hwnd") or args.get("dialogHwnd")
    if args.get("method") != "sendinput":
        bg = send_key_background(key, hwnd=hwnd)
        if bg.get("success"):
            bg["output"] = f"Sent key '{key}' via background PostMessage (no focus taken)."
            return bg
    _notify_before_foreground(args.get("reason") or f"sending the '{key}' key, which this surface only accepts in the foreground")
    result = send_key_to_fusion(key, hwnd=hwnd)
    if result.get("success"):
        result["output"] = f"Sent key '{key}' to Fusion."
    return result


def _handle_close_window(fusion_info: dict, args: dict) -> dict:
    """Close a dialog window by sending WM_CLOSE (equivalent to clicking X).

    Unlike Escape, this works on dialogs that don't have Cancel/Escape handling
    (e.g. the Recovered Documents dialog). Does NOT force-kill — the dialog can
    still intercept WM_CLOSE and prompt for confirmation.
    """
    hwnd = args.get("hwnd")
    if not hwnd:
        return {"success": False, "error": "Missing required arg: hwnd"}
    result = close_window(int(hwnd))
    if result.get("success"):
        result["output"] = f"Sent WM_CLOSE to hwnd {hwnd}."
    return result


def _handle_window_info(fusion_info: dict, args: dict) -> dict:
    """Get Fusion window info including any dialog windows."""
    result = get_fusion_window_info()
    if result.get("success"):
        dialogs = result.get("dialogs", [])
        # ab's demo runner reads blockingDialogs[] off this verb (issue #30) - give it the
        # CLASSIFIED list (cause + resolution hint), not just raw titles.
        try:
            result["blockingDialogs"] = classify_blocking_dialogs() or []
        except Exception:
            result["blockingDialogs"] = []
        _license_classify_extend(result["blockingDialogs"])
        dialog_info = [f"  hwnd={d['hwnd']}: {d['title']}" for d in dialogs]
        output_parts = [
            f"Main: hwnd={result['hwnd']}, title='{result['title']}'",
        ]
        if dialogs:
            output_parts.append(f"Dialogs ({len(dialogs)}):")
            output_parts.extend(dialog_info)
        else:
            output_parts.append("No dialog windows detected.")
        if dialogs:
            hint = (f"{len(dialogs)} dialog(s) detected. Screenshot each via "
                    "desktop_screenshot_window {{\"hwnd\":<hwnd>}}, then dismiss with "
                    "fusion_dismiss_blocking_dialogs or fusion_send_key/fusion_close_window.")
        else:
            hint = "No dialogs — Fusion is clear. Safe to run fusion_* commands."
        result["output"] = json.dumps({
            "hwnd": result["hwnd"],
            "title": result["title"],
            "rect": result["rect"],
            "dialogs": dialogs,
            "blockingDialogs": result.get("blockingDialogs", []),
            "message": "\n".join(output_parts),
            "_hint": hint,
        })
    return result


def _handle_screenshot_all_fusion(fusion_info: dict, args: dict) -> dict:
    """Screenshot Fusion main window AND all dialog windows.

    Returns paths to all captured screenshots so the AI can see
    CEF overlay dialogs and separate Qt dialog windows.
    """
    screenshots = []

    # 1. Capture main Fusion window from screen (captures CEF overlays)
    main_result = screenshot_fusion_window()
    if main_result.get("success"):
        screenshots.append({
            "type": "main_window",
            "path": main_result["savedTo"],
            "sizeKB": main_result["sizeKB"],
        })

    # 2. List all Qt dialog windows (AI should use desktop_screenshot_window for each)
    info = get_fusion_window_info()
    if info.get("success") and info.get("dialogs"):
        for dialog in info["dialogs"]:
            screenshots.append({
                "type": "dialog",
                "title": dialog["title"],
                "hwnd": dialog["hwnd"],
                "rect": dialog["rect"],
                "hint": f"Use desktop_screenshot_window with hwnd={dialog['hwnd']} to capture this dialog.",
            })

    return {
        "success": True,
        "screenshots": screenshots,
        "dialogCount": len(info.get("dialogs", [])) if info.get("success") else 0,
        "_hint": f"Captured {len(screenshots)} screenshot(s). Screenshots saved on Windows — "
                 "use pull_file to get them to Docker, or Read tool to view each path directly. "
                 "If dialogs are present, dismiss with fusion_dismiss_blocking_dialogs.",
    }


def _handle_relocate_recovery(fusion_info: dict, args: dict) -> dict:
    """Relocate Fusion crash recovery files to ~/.adom/recovery/fusion/.

    Call this proactively before launching Fusion to prevent recovery
    dialogs from appearing. Files are preserved (not deleted) so the
    user can manually restore them if needed.
    """
    from handlers.dismiss_recovery import relocate_recovery_files
    try:
        reloc = relocate_recovery_files()
    except Exception as exc:
        return {
            "success": False,
            "error": f"Failed to relocate recovery files: {exc}",
        }

    if reloc["moved"] > 0:
        return {
            "success": True,
            "output": (
                f"Relocated {reloc['moved']} recovery file(s) to {reloc['dest']}. "
                "These files are preserved — the user can restore them from that "
                "directory if they need to recover unsaved work."
            ),
            "moved": reloc["moved"],
            "dest": reloc["dest"],
            "sources": reloc["sources"],
        }
    return {
        "success": True,
        "output": "No crash recovery files found.",
        "moved": 0,
    }


def _installer_running() -> bool:
    """Is the Fusion Client Downloader / streamer currently installing? Bridge-side
    check (subprocess from OUR process = no ab shell-approval gate)."""
    try:
        import subprocess as _sp
        out = _sp.run(["tasklist", "/FO", "CSV", "/NH"], capture_output=True, text=True,
                      timeout=15, creationflags=getattr(_sp, "CREATE_NO_WINDOW", 0)).stdout
        return ("streamer.exe" in out) or ("FusionDL.exe" in out) or ("Fusion Client Downloader" in out)
    except Exception:
        return False


def _handle_install_fusion(fusion_info: dict, args: dict) -> dict:
    """Install Fusion 360 FOR the user - no shell_execute, no ab approval gate.

    Declared as detect.installVerb (ab >=1.9.79) so bridge_readiness recommends
    THIS over the generic winget fallback. Downloads the official Fusion Client
    Downloader and runs it silently: --globalinstall when elevated, PER-USER when
    not (learned live: a non-admin shell makes --globalinstall die silently on
    UAC; the per-user install needs no elevation and lands in %LOCALAPPDATA%,
    which detect covers). Returns PROMPTLY - the stream takes 10-30 min; poll
    fusion_readiness until installed:true."""
    # LIVE detect only - never trust the bridge-start fusion_info snapshot, which can be
    # stale-True after Fusion was removed while the bridge kept running (found live on a
    # fresh VM 2026-07-05: this verb refused with "already installed" though disk was
    # NO_WEBDEPLOY, blocking the reinstall). detect_fusion() is the on-disk truth.
    if detect_fusion().get("installed"):
        return {"success": True, "alreadyInstalled": True,
                "_hint": "Fusion 360 is already installed - call fusion_start to launch it."}
    if _installer_running():
        return {"success": True, "installing": True, "statusVerb": "fusion_readiness",
                "_hint": "An install is ALREADY streaming (10-30 min). Poll fusion_readiness "
                         "until installed:true - do not start a second installer."}

    import tempfile, urllib.request as _ur, subprocess as _sp, ctypes as _ct
    stub = os.path.join(tempfile.gettempdir(), "FusionClientDownloader.exe")
    try:
        _ur.urlretrieve(
            "https://dl.appstreaming.autodesk.com/production/installers/Fusion%20Client%20Downloader.exe",
            stub)
    except Exception as e:
        return {"success": False, "error": f"Installer download failed: {e}",
                "errorCode": "installer_download_failed",
                "_hint": "Check the box is online; retry fusion_install_fusion. The stub URL is "
                         "Autodesk's official appstreaming installer."}
    try:
        elevated = bool(_ct.windll.shell32.IsUserAnAdmin()) if hasattr(_ct, "windll") else False
    except Exception:
        elevated = False
    cmd = [stub, "--quiet"] + (["--globalinstall"] if elevated else [])
    try:
        _sp.Popen(cmd, stdin=_sp.DEVNULL, stdout=_sp.DEVNULL, stderr=_sp.DEVNULL,
                  creationflags=getattr(_sp, "CREATE_NO_WINDOW", 0) | getattr(_sp, "DETACHED_PROCESS", 0))
    except Exception as e:
        return {"success": False, "error": f"Installer launch failed: {e}",
                "errorCode": "installer_launch_failed"}
    return {
        "success": True, "installing": True, "elevated": elevated,
        "mode": "globalinstall" if elevated else "per-user",
        "statusVerb": "fusion_readiness",
        "_hint": ("Fusion 360 install STARTED (" + ("system-wide" if elevated else
                  "per-user - no admin needed; a non-elevated --globalinstall dies silently on UAC") +
                  "). It streams several GB (10-30 min): poll fusion_readiness until installed:true "
                  "(it also reports installing:true while the streamer runs), then fusion_start. "
                  "NOTIFY the user it is underway (progress toast) and again at the Autodesk sign-in."),
    }


def _aps_quick_state() -> dict:
    """Cheap APS state for readiness: {configured, signedIn}. Never raises, never blocks."""
    try:
        import aps as _aps
        st = _aps.handle_status({}) or {}
        d = st.get("data", st)
        return {"configured": bool(d.get("configured")), "signedIn": bool(d.get("signedIn"))}
    except Exception:
        return {"configured": None, "signedIn": None}


def _handle_fusion_readiness(fusion_info: dict, args: dict) -> dict:
    """FAST readiness check for the AI: is the Fusion host app present + running + the bridge ready?
    Does NOT launch Fusion (fast-fails when it's not running). ab DETECTS Fusion (never installs it)
    via bridge.json 'detect', and auto-installs Python if missing (ab >=1.9.47) - this verb just
    reports state. Pairs with ab's bridge_readiness."""
    # ALWAYS live-detect. fusion_info is a bridge-START snapshot; Fusion may have been
    # installed OR UNINSTALLED since. Trusting a cached installed:True made readiness
    # report installed/running/ready:true on a machine where Fusion had been removed
    # (found live on a fresh VM 2026-07-05: NO_WEBDEPLOY on disk, yet readiness said
    # ready). detect_fusion() is a fast filesystem scan, so pay it every call.
    raw_installed = bool(detect_fusion().get("installed"))
    # `streaming` must be RELIABLE. _installer_running() is a tasklist check, but the Autodesk streamer
    # spawns short-lived per-chunk workers, so it reads False most of a live multi-GB stream (flickered
    # True only 3 of 25 polls during a real install, John 2026-07-14). OR in the disk-state signal - a
    # webdeploy hash dir with FusionLauncher.exe but a missing/truncated .ini - which is present for the
    # WHOLE stream. Now `installing` is trustworthy, not racy.
    streaming = _installer_running() or _incomplete_webdeploy_present()
    running = _is_fusion_running() if raw_installed else False
    # FRESH-INSTALL COMPLETENESS GUARD (John caught this live 2026-07-14 on a fresh Hyper-V VM):
    # the streamer writes FusionLauncher.exe BEFORE it finishes writing FusionLauncher.exe.ini, so
    # detect_fusion() flips installed:true mid-stream. Starting into that half-written install throws
    # "Error Launching Streamed Application ... FusionLauncher.exe.ini is missing or incomplete" and
    # leaves TWO corrupt webdeploy production dirs. So: while the streamer is running and Fusion is
    # NOT already up (i.e. this is a first install, not a background auto-update of a running Fusion),
    # the install is STILL STREAMING - report installed:false + installing:true so nothing calls
    # fusion_start yet. Only treat it as installed once the streamer has exited.
    if streaming and not running:
        installed = False
        installing = True
    else:
        installed = raw_installed
        installing = (not installed) and streaming

    # The add-in only serves once Fusion is PAST sign-in (its HTTP server runs on
    # Fusion's main thread, which the sign-in modal blocks). So a responding add-in
    # is the definitive "signed in + drivable" signal.
    addin_status = _check_addin_status(timeout=1.0) if running else None
    addin_ok = addin_status is not None

    # Fusion can be RUNNING yet not drivable: the first-run Autodesk sign-in modal
    # ("Signing in - Autodesk Fusion" / "Welcome to Fusion") blocks the main thread,
    # so the add-in never serves and every modeling verb hangs. The old code reported
    # ready:True here purely on installed+running, which was a lie during sign-in.
    # Detect it from the FUSION-PROCESS window title - NOT classify_launch_dialogs,
    # which scans ALL top-level windows and false-matches a stray Edge "Sign in -
    # Autodesk" BROWSER tab (caught 2026-07-05).
    #
    # ⚠️ CHECK IT WHENEVER RUNNING, not only when the add-in is silent (fix 2026-07-06,
    # John caught it lying): a STALE add-in HTTP server from a just-killed instance can
    # still hold port 8774 and answer, so `addin_ok` goes True while the CURRENT Fusion
    # sits stuck on "Signing in". Gating the sign-in check on `not addin_ok` then let
    # readiness report ready:true for a dead, stuck-signing-in window. The Fusion-process
    # window title is authoritative - if it says "Signing in", we are NOT ready, period.
    needs_signin = False
    if running:
        try:
            finfo = get_fusion_window_info() or {}
            _t = (finfo.get("title") or "").lower()
            needs_signin = ("signing in" in _t or "welcome to fusion" in _t
                            or ("sign in - autodesk" in _t and "fusion" in _t))
        except Exception:
            needs_signin = False
    # "Ready to drive" requires the add-in to actually RESPOND (not just the process to
    # exist) AND no sign-in modal. A responding add-in proves Fusion's main thread is
    # free; requiring it stops a half-launched or stuck Fusion from reading as ready.
    ready = installed and running and addin_ok and not needs_signin

    # SEAT/LICENSING dialog blocking the launch - detected DETERMINISTICALLY by owning
    # process + dialog size (not the "Fusion360" title). Checked WHENEVER running, not
    # only when the add-in is silent (fix 2026-07-06): a STALE add-in port from a prior
    # instance answers while the CURRENT Fusion sits blocked behind the seat dialog, so
    # gating on `not addin_ok` false-negatived `licensingDialog` while the dialog sat
    # right there on the user's screen.
    #
    # ⚠️ GAP FIX (John caught it live 2026-07-06): the launch loop only auto-resolves the
    # seat dialog during its ~50s window. But this dialog also appears LATER - minutes
    # after sign-in, when the license server notices too many sessions - long after that
    # loop exited, so it just sat there blocking while readiness passively reported
    # licensingDialog:true. Now readiness SELF-HEALS: whenever it detects the dialog it
    # AUTO-RESOLVES it in the background (UIA 'Continue' + screenshot-verify), so the seat
    # is handled deterministically in CODE no matter when it shows up - the AI never has
    # to notice or act. (John: "do all this tracking from your code so it's deterministic
    # ... the ai never follows skills.")
    seat_resolved = False
    licensing = False
    if running:
        try:
            licensing = find_licensing_dialog() is not None
        except Exception:
            licensing = False
        if licensing:
            try:
                _r = _resolve_seat_dialog()
                seat_resolved = _r.get("clicks", 0) > 0
                # Re-check: did it actually clear? (screenshot-verified inside.)
                licensing = find_licensing_dialog() is not None
            except Exception:
                pass
            if not licensing:
                # Cleared - re-probe the add-in; Fusion may be drivable now.
                try:
                    addin_status = _check_addin_status(timeout=1.5)
                    addin_ok = addin_status is not None
                    ready = installed and running and addin_ok and not needs_signin
                except Exception:
                    pass
    if licensing:
        ready = False  # a seat/licensing dialog blocking the UI is never "ready"

    # Fusion applying an AUTO-UPDATE (two+ webdeploy builds) crash-restarts itself every
    # ~30-60s, so a verb intermittently sees running:false. That is NOT a crash or a
    # bridge failure - surface it as a distinct non-fatal `updating` flag so a driving AI
    # waits/retries instead of giving up. Only check when installed but not currently
    # ready (avoid noise when Fusion is happily drivable).
    updating = False
    if installed and not ready:
        try:
            updating = fusion_update_in_progress()
        except Exception:
            updating = False

    # When Fusion is up + signed in, also check the add-in isn't STALE (issue #55) so
    # a fresh readiness call surfaces the mismatch instead of a later verb failing
    # silently. Reuse the add-in status already fetched above (no second probe).
    stale_info = {}
    if addin_ok:
        stale_info = _addin_staleness(addin_status.get("version"))

    if updating and not ready:
        hint = ("Fusion is applying an AUTO-UPDATE (multiple webdeploy builds present) and "
                "RESTARTS ITSELF every ~30-60s - this is NOT a crash or a bridge failure. Poll "
                "fusion_readiness; it stabilizes to ready:true once the update finishes (can take "
                "10-30 min). Meanwhile, wrap modeling/export verbs in a short retry/catch-loop to "
                "ride the up-windows rather than treating an intermittent 'not running' as fatal.")
    elif needs_signin:
        hint = ("Fusion is RUNNING but stuck at the first-run Autodesk SIGN-IN (main UI blocked, add-in "
                "cannot serve) - NOT ready. FULL PLAYBOOK: the `fusion-autodesk-signin` skill. Proven "
                "sequence + the pitfalls that cost hours on a fresh Hyper-V VM (John 2026-07-14): "
                "(1) BLANK white webview on first launch? fusion_stop then fusion_start - the sign-in "
                "renders 'Welcome to Fusion' + a 'Sign In' button only after a clean relaunch. "
                "(2) ASK the user FIRST (AskUserQuestion): do they have a warm Google/Apple/Microsoft "
                "session for the account LINKED to their Autodesk login? Offer to drive it. A truly "
                "fresh box has NO warm browser session, so 'Continue with Google' still hits a password "
                "wall. (3) Click Fusion 'Sign In' -> it opens the DEFAULT browser (Edge) to the OAuth. "
                "Edge windows DO exist even if PowerShell EnumWindows looks empty - that is CLIXML "
                "progress noise; set $ProgressPreference='SilentlyContinue', and just "
                "desktop_screenshot_window the Edge hwnd (PrintWindow works backgrounded). "
                "(4) In the browser: 'Continue with Google' -> type the user's EMAIL (not secret) -> if "
                "it asks for a PASSWORD/2FA, STOP and fusion_notify_owner {title, body} to toast their "
                "MAIN computer so THEY type it; NEVER enter password/2FA yourself. (5) Handoff back to "
                "Fusion is via the 'Autodesk Identity Manager' protocol - Edge shows a 'This site is "
                "trying to open...' overlay: tick 'Always allow' + click 'Open'. (6) SPEED MATTERS: the "
                "sign-in code EXPIRES in ~2 min; on a high-latency VM slow screenshot/click round-trips "
                "let it expire ('Sign-in request expired') and you loop. Minimize steps; if it expires, "
                "click Fusion 'Sign In' again for a FRESH code (the browser session stays warm). "
                "Then poll until ready:true. FIRST-TIME DEMO? Run fusion_demo instead - it drives this "
                "sign-in through the user's NATIVE browser (already signed into Autodesk) and then "
                "gives them a real guided tour (project -> schematic -> 2D board -> 3D board).")
    elif ready and stale_info.get("addinStale"):
        hint = stale_info["_staleHint"]
    elif ready:
        # AUTO-UPDATE, checked on EVERY launch and applied SILENTLY (John, 2026-07-22: "i just
        # always want the latest fusion... make this generally invisible to me"). Fusion nags
        # constantly; we click it in the BACKGROUND so the user never sees it. Sticky opt-out.
        _upd = _apply_fusion_update_silently()
        # APS is checked on EVERY launch now (John, 2026-07-22: "why aren't those just driven from
        # 1 login?"). Fusion sign-in and APS are both Autodesk logins; the user should authenticate
        # ONCE. They cannot share a token (Autodesk DPAPI-encrypts Fusion's store), but they CAN
        # share the warm browser SSO session - so APS consent is silent right after a Fusion login.
        _aps = _aps_quick_state()
        # MCP state, reported beside APS on every launch (both are search/drive surfaces the
        # AI should know about without probing). Cheap: a 0.4s loopback socket.
        _mcpmsg = (" Autodesk's MCP server is LIVE (fusion_mcp_* verbs work)."
                   if _probe_mcp_port() else
                   " Autodesk's MCP server is OFF - fusion_mcp_enable can turn it on for the "
                   "user (~10s, drives the Preferences dialog).")
        _updmsg = _mcpmsg
        if _upd.get("applied"):
            _updmsg += (" AUTO-UPDATE: Fusion was offering an update and I clicked %r for the user "
                       "IN THE BACKGROUND - it downloads now and the new build comes up on the next "
                       "launch. TELL THE USER it is handled; they never need to click the nag. "
                       "They can opt out with fusion_set_auto_update {enabled:false} (sticky)."
                       % _upd.get("button"))
        elif not _auto_update_enabled():
            _updmsg += (" AUTO-UPDATE is OFF (the user opted out, sticky) - do NOT click Fusion's "
                       "update prompts for them.")
        if _aps.get("signedIn"):
            hint = ("READY - drive Fusion (fusion_open_lbr / build_library_3d / etc.). APS cloud "
                    "search is signed in too, so use fusion_aps_search for ~2s cloud lookups."
                    + _updmsg)
        elif _aps.get("configured"):
            hint = ("READY to drive Fusion, but APS CLOUD SEARCH IS NOT SIGNED IN - cloud file "
                    "search will be slow/blocked. DO IT NOW, do not make the user log in a second "
                    "time: call fusion_aps_signin RIGHT AFTER a Fusion sign-in, while the browser "
                    "profile still has a warm Autodesk SSO session - it consents silently (no "
                    "password, no 2FA). Then fusion_aps_search works." + _updmsg)
        else:
            hint = ("READY to drive Fusion. APS cloud search is NOT SET UP (no client ID) - cloud "
                    "search is the slow path without it. Offer to set it up: fusion_aps_set_client_id "
                    "then fusion_aps_signin (silent while the browser SSO session is warm). APS is "
                    "how cloud search gets to ~2 seconds." + _updmsg)
    elif installed and running and licensing:
        hint = ("Fusion is blocked by a SEAT/LICENSING dialog ('Active Sessions Exceeded' / 'Suspend "
                "Remote Session') - detected DETERMINISTICALLY as an OWNED POPUP of the main Fusion "
                "window (its title is just 'Fusion360' and it can be as short as 262px, so it is found "
                "by owner+parent-screenshot, NOT a size guess). This verb JUST TRIED to auto-resolve it "
                "in the BACKGROUND (UIA 'Continue', Suspend pre-selected so it reclaims the seat) but it "
                "is still up - it may be mid-render (the first UIA Invoke no-ops). Just POLL "
                "fusion_readiness again in a few seconds; it keeps auto-resolving. Never coordinate-click "
                "or ask the user, and never kill/relaunch (the server keeps the seat).")
    elif installed and running:
        hint = ("Fusion is RUNNING but its add-in is NOT responding yet - NOT ready to drive. Either it "
                "is still finishing launch / signing in (poll fusion_readiness a few more times), or the "
                "add-in did not load (fusion_stop then fusion_start to re-deploy + re-load it). Do NOT "
                "fire modeling/export verbs until ready:true.")
    elif installed:
        hint = "Fusion is INSTALLED but not running - call fusion_start (blocks until the add-in is ready), then retry."
    else:
        hint = ("Fusion install is STREAMING (10-30 min) - keep polling fusion_readiness until installed:true."
                if installing else
                "Fusion 360 is NOT installed. OFFER to install it for the user, then call "
                "fusion_install_fusion (no shell approval needed) - it streams the free trial and "
                "this verb reports installing:true until done.")

    # READY MUST MEAN DRIVABLE (page issue #36). `addin_ok` above comes from an HTTP GET to
    # the add-in's own web server, which is served by its HTTP thread - it is up well before
    # Fusion's MAIN thread will execute a command. Callers poll this verb precisely to learn
    # "can I fire my next verb now", so confirm with one real round-trip before saying yes.
    # Cheap when the main thread is free (~50ms); when it is not, failing here is exactly the
    # answer the caller needed.
    if ready:
        try:
            if not _proxy_to_addin("get_app_state", {}, timeout=4).get("success"):
                ready = False
                result_note = ("the add-in's HTTP server is up but its main thread did not "
                               "execute a test command yet - not drivable this second")
            else:
                result_note = None
        except Exception:
            ready = False
            result_note = "the add-in did not complete a test command round-trip"
    else:
        result_note = None

    if ready:
        try:
            _ensure_fusion_user_async()
        except Exception:
            pass
    elif not running and _FUSION_USER:
        _FUSION_USER.clear()      # account switch shows the NEW identity after restart
    _orient = _fusion_modeling_orientation()
    _orient_note = ("" if _orient != "yup" else
                    " ⚠ Default Modeling Orientation is Y-UP on this account, but Adom is a "
                    "Z-UP environment (a fresh Autodesk account defaults to Y-up). Set it with "
                    "fusion_set_preference {\"modelingOrientation\":\"zup\"} - no restart needed.")
    result = {
        "success": True,
        "hostApp": "Fusion 360",
        "installed": installed,
        "installing": installing,
        "running": running,
        "ready": ready,
        # CAPABILITY, knowable before staging a single file (#43 follow-up): shape B of
        # fusion_create_library needs adom-lbr, which is Linux-container-only by platform
        # rule - so on Windows/Mac this is False and the caller should convert at
        # authoring time and send shape A (.lbr + .step).
        "kicadIngest": bool(_find_adom_lbr()),
        # WHO is signed into Fusion itself (cached; fetched in the background on the
        # first ready poll, so readiness never waits on a modeling script)
        "fusionUser": ({"user": _FUSION_USER.get("user"), "userId": _FUSION_USER.get("userId")}
                       if _FUSION_USER.get("user") else None),
        # ADOM IS A Z-UP WORLD (John, 2026-08-23): a fresh Autodesk account defaults to Y-up,
        # so surface the current account's Default Modeling Orientation and flag Y-up - most
        # relevant on a NEW user. Disk read, no add-in, never disrupts a driven session.
        "modelingOrientation": _orient,
        "bridgeVersion": BRIDGE_VERSION,
        **stale_info,
        "_hint": (((hint + " NOTE: " + result_note) if result_note else hint) + _orient_note),
    }
    # MEASURED PROGRESS WHILE LAUNCHING (issue #32): the poll verb must render the same
    # bar the launch verb started, or the caller is back to a spinner with no end.
    try:
        import time as _t_mod
        import timings as _tm
        result["timings"] = _tm.summary().get("launch")
        # READINESS IS THE LAUNCH OBSERVER (found live 2026-08-19): `fusion_start` is an
        # ab-CORE verb, not ours, so our own launch handler never runs on the real path and
        # never stamps the start. This verb sees every launch regardless of who began it -
        # so it stamps the first running-but-not-ready observation, reports progress from
        # it, and banks the MEASURED sample on the transition to ready. That is what makes
        # the estimate this machine's own truth instead of a literal (page issue #32).
        _lsa = _get_bridge_pref("launchStartedAt", None)
        # ⚠️ A STALE STAMP MUST NOT SILENCE US FOREVER (my own bug, caught live on
        # ConfRoomROG 2026-08-19): the progress branch only reports inside a 900s window
        # and the stamp is cleared only on ready, so once a launch passed 900s the box
        # reported NO progress and, because the heal guard is once-per-stamp, NO further
        # healing either - permanently stuck in a state the code could not escape, which
        # is the exact "gave up quietly" failure this whole contract exists to prevent.
        # A stale window is re-opened instead: fresh stamp, heal re-armed.
        if _lsa and not ready and (_t_mod.time() - float(_lsa)) > 900:
            _set_bridge_pref("launchStartedAt", _t_mod.time())
            _set_bridge_pref("launchHealedAt", None)
            _lsa = _get_bridge_pref("launchStartedAt", None)
        if not _lsa and running and not ready:
            _set_bridge_pref("launchStartedAt", _t_mod.time())
            _lsa = _get_bridge_pref("launchStartedAt", None)
        # FUSION CRASHED? That is not "slow", it is GONE - and tasklist still shows a
        # Fusion360.exe, so `running` stays true and readiness would otherwise report
        # "running but not ready" forever with nothing to explain it. Close the report and
        # relaunch: the add-in only loads at launch (found live on ConfRoomROG 2026-08-19,
        # TWO stacked crash reports behind a 10-minute stall).
        if not ready and running:
            try:
                from handlers.dialog_classify import RELAUNCH_CATEGORIES
                _crashed = [d for d in (classify_launch_dialogs() or [])
                            if d.get("category") in RELAUNCH_CATEGORIES]
                if _crashed:
                    for _d in _crashed:
                        close_dialog_bg(_d.get("hwnd"))
                    result["crashReportCleared"] = len(_crashed)
                    result["fusionCrashed"] = True
                    # A crash is NEW information, so it re-arms the heal even if one
                    # already ran for this launch - never give up on a box for good.
                    _set_bridge_pref("launchHealedAt", None)
                    _set_bridge_pref("launchStartedAt", _t_mod.time())
            except Exception:
                pass

        # SELF-HEAL A LAUNCH THAT WILL NEVER FINISH (doctrine; caught live on ConfRoomROG
        # 2026-08-19, Fusion up 362s with the add-in silent and NO dialog to explain it).
        # The add-in only loads AT LAUNCH, so more waiting can never fix a Fusion that came
        # up without it - the remedy is a restart, and the bridge must perform it rather
        # than report "not ready" forever and leave the human to figure it out. Once per
        # launch stamp, in the BACKGROUND so readiness stays inside the relay budget.
        if (not ready and running and not needs_signin and not licensing and not updating
                and _lsa and not _get_bridge_pref("launchHealedAt", None)):
            _p90 = ((_tm.stats("launch", "cold") or {}).get("p90") or 0)
            _overdue_by = max(180.0, 2.0 * float(_p90 or 0))
            if (_t_mod.time() - float(_lsa)) > _overdue_by:
                _set_bridge_pref("launchHealedAt", _t_mod.time())
                result["addinRestartTriggered"] = True

                _attempt = int(_get_bridge_pref("launchHealAttempts", 0) or 0) + 1
                # Four restarts without a serving add-in is no longer a slow launch. STOP
                # restarting, but KEEP REPORTING - going silent is the failure mode this
                # whole contract exists to prevent - and hand back a decidable
                # classification instead of another blind attempt.
                _exhausted = _attempt > 4
                if _exhausted:
                    result["addinRestartExhausted"] = True
                    result["_hint"] = (
                        "Fusion has been restarted %d times and the add-in still does not serve. "
                        "This is no longer a slow launch: screenshot the Fusion hwnd and read "
                        "screenshots[] for a blocking dialog, or check whether Fusion is crashing "
                        "at startup (fusionCrashed / crash_report)." % (_attempt - 1))
                if not _exhausted:
                    _set_bridge_pref("launchHealAttempts", _attempt)
                    result["addinRestartAttempt"] = _attempt

                def _heal(attempt=_attempt):
                    # ESCALATE: a graceful stop that did not take is not a reason to keep
                    # repeating it. Second attempt onward goes straight to the force path.
                    try:
                        if attempt >= 2:
                            dispatch_command("kill", {})
                        else:
                            dispatch_command("stop", {})
                        _t_mod.sleep(8)
                        dispatch_command("start", {})
                    except Exception:
                        pass
                try:
                    import threading as _th
                    if not _exhausted:
                        _th.Thread(target=_heal, daemon=True).start()
                    print("[Fusion Bridge] add-in never came up after %ds - restarting Fusion"
                          % int(_t_mod.time() - float(_lsa)))
                except Exception:
                    pass
        if ready:
            _set_bridge_pref("launchHealedAt", None)
            _set_bridge_pref("launchHealAttempts", 0)
            if _lsa:
                try:
                    _el = _t_mod.time() - float(_lsa)
                    if 3 <= _el <= 3600:      # ignore clock jumps and trivially-warm hits
                        _tm.record("launch", _el, "cold")
                except Exception:
                    pass
                _set_bridge_pref("launchStartedAt", None)
        elif _lsa and (_t_mod.time() - float(_lsa)) < 900:
            _label = ("Signing in to Autodesk" if needs_signin else
                      "Clearing the Autodesk seat dialog" if licensing else
                      "Applying a Fusion auto-update" if updating else
                      "Fusion is up - waiting for the Adom add-in to serve" if running else
                      "Fusion is starting")
            _pb = _tm.progress_block("launch", float(_lsa), _label,
                                     start_kind="cold", fallback=150)
            # SAY IT OUT LOUD when we are past what this machine has ever taken - a creeping
            # bar with no explanation is the same shrug in a nicer outfit (issue #32).
            if _pb.get("overdue") and _pb.get("confidence") == "unknown":
                _pb["stepLabel"] = (_label + " - this is longer than this machine has ever "
                                    "taken; restarting Fusion, since the add-in only loads at launch")
            result["progress"] = _pb
            result["statusVerb"] = "fusion_readiness"
    except Exception:
        pass
    if updating:
        result["updating"] = True
        result["statusVerb"] = "fusion_readiness"
    if needs_signin:
        result["needsSignin"] = True
        result["statusVerb"] = "fusion_readiness"
    # LAST-KNOWN state (John, 2026-08-17: "shouldn't i be fully setup already other than
    # fusion running?"): sign-in/add-in state is UNKNOWABLE while Fusion is closed, and a
    # dashboard that paints "unknown" as broken-red misreads a fully-configured box. So
    # persist the good observations and always report them: a closed Fusion with
    # lastSignedInAt set = "configured, dormant", not "needs setup".
    try:
        import datetime as _dt
        _now_iso = _dt.datetime.now().isoformat(timespec="seconds")
        if ready and running:
            _set_bridge_pref("lastReadyAt", _now_iso)
        if running and not needs_signin and ready:
            _set_bridge_pref("lastSignedInAt", _now_iso)
        result["lastReadyAt"] = _get_bridge_pref("lastReadyAt", None)
        result["lastSignedInAt"] = _get_bridge_pref("lastSignedInAt", None)
    except Exception:
        pass
    if licensing:
        result["licensingDialog"] = True
        result["statusVerb"] = "fusion_readiness"
    if seat_resolved:
        # We acted on it this call (whether or not it fully cleared yet).
        result["seatDialogAutoResolved"] = True
    # Update-offer visibility (John, 2026-08-16): a detected nag is ALWAYS reported, so the
    # AI either says "handled it" or OFFERS to run it - nothing detected goes unmentioned.
    if _LAST_UPDATE_OFFER.get("offered"):
        result["fusionUpdateOffered"] = True
        _handled = bool(locals().get("_upd", {}).get("applied")) if isinstance(locals().get("_upd"), dict) else False
        result["fusionUpdateHandled"] = _handled
        if _handled:
            result["fusionUpdateHint"] = ("Fusion offered an update and it was clicked in the "
                                          "background - tell the user it is handled.")
        else:
            result["fusionUpdateHint"] = ("Fusion is OFFERING AN UPDATE (auto-update %s). OFFER "
                                          "to run it: fusion_set_auto_update {enabled:true} or a "
                                          "one-off background click on the next sweep."
                                          % ("opted out" if not _auto_update_enabled() else "pending"))
    _asf = _addin_start_failure()
    if _asf:
        result["addinStartFailed"] = _asf
        result["_hint"] = _asf["_hint"] + " " + str(result.get("_hint") or "")

    return result


# ── Fusion preferences (theme / navigation / units) ──────────────────────────
# Set via the adsk preferences API inside the live session (run_modeling_script),
# so this is a SERVER-only orchestrator - no add-in redeploy. Friendly keys map to
# adsk enums; unknown/failed keys are reported per-key (some themes, e.g. Dark Gray
# / Classic, aren't shipped in every Fusion build - proven live 2026-07-05, only
# LightGray/DarkBlue/Device were available). Theme changes apply LIVE (no restart).
_PREF_SCRIPT = r'''
import json
gp = app.preferences.generalPreferences
T  = adsk.core.UserInterfaceThemes
PZ = adsk.core.PanZoomOrbitShortcuts
MO = adsk.core.DefaultModelingOrientations
DU = adsk.fusion.DistanceUnits

THEME = {'light':[T.LightGrayUserInterfaceTheme],'lightgray':[T.LightGrayUserInterfaceTheme],
         'dark':[T.DarkGrayUserInterfaceTheme,T.DarkBlueUserInterfaceTheme],
         'darkgray':[T.DarkGrayUserInterfaceTheme],'darkblue':[T.DarkBlueUserInterfaceTheme],
         'classic':[T.ClassicUserInterfaceTheme],'device':[T.DeviceUserInterfaceTheme],
         'auto':[T.DeviceUserInterfaceTheme]}
THEME_NAME  = {0:'classic',1:'lightgray',2:'darkblue',3:'darkgray',4:'device'}
ORBIT = {'fusion360':PZ.Fusion360PanZoomOrbitShortcut,'fusion':PZ.Fusion360PanZoomOrbitShortcut,
         'alias':PZ.AliasPanZoomOrbitShortcut,'inventor':PZ.InventorPanZoomOrbitShortcut,
         'solidworks':PZ.SolidWorksPanZoomOrbitShortcut,'tinkercad':PZ.TinkercadPanZoomOrbitShortcut,
         'powermill':PZ.PowerMillPanZoomOrbitShortcut}
ORBIT_NAME  = {0:'fusion360',1:'alias',2:'inventor',3:'solidworks',4:'tinkercad',5:'powermill'}
ORIENT = {'yup':MO.YUpModelingOrientation,'zup':MO.ZUpModelingOrientation}
ORIENT_NAME = {0:'yup',1:'zup'}
UNITS = {'mm':DU.MillimeterDistanceUnits,'cm':DU.CentimeterDistanceUnits,'m':DU.MeterDistanceUnits,
         'in':DU.InchDistanceUnits,'inch':DU.InchDistanceUnits,'ft':DU.FootDistanceUnits}
UNITS_NAME  = {0:'mm',1:'cm',2:'m',3:'in',4:'ft'}

def read_current():
    cur = {
      'theme': THEME_NAME.get(gp.userInterfaceTheme, gp.userInterfaceTheme),
      'activeTheme': THEME_NAME.get(gp.activeUserInterfaceTheme, gp.activeUserInterfaceTheme),
      'invertScrollZoom': bool(gp.isZoomDirectionReversed),
      'orbitScheme': ORBIT_NAME.get(gp.panZoomOrbitShortcuts, gp.panZoomOrbitShortcuts),
      'modelingOrientation': ORIENT_NAME.get(gp.defaultModelingOrientation, gp.defaultModelingOrientation),
      'gestureNav': bool(gp.isGestureBasedViewNavigationUsed),
      'cameraPivot': bool(gp.isCameraPivotEnabled),
    }
    try:
        cur['lengthUnit'] = UNITS_NAME.get(design.fusionUnitsManager.distanceDisplayUnits,
                                           design.fusionUnitsManager.distanceDisplayUnits)
    except Exception:
        pass
    return cur

def try_set(obj, attr, cands, want):
    err = None
    for c in cands:
        if c is None: continue
        try:
            setattr(obj, attr, c)
            return {'requested': want, 'ok': True}
        except Exception as e:
            err = str(e)
    return {'requested': want, 'ok': False, 'error': err or 'no valid value'}

reqs = json.loads(__REQS__)
applied = {}
for k, v in reqs.items():
    kv = v if isinstance(v, bool) else str(v).strip().lower()
    if k == 'theme':
        c = THEME.get(kv);  applied[k] = try_set(gp,'userInterfaceTheme', c or [], v) if c else {'requested':v,'ok':False,'error':'unknown theme (use light/darkblue/darkgray/classic/device/dark)'}
    elif k in ('invertScrollZoom','reverseZoom'):
        applied['invertScrollZoom'] = try_set(gp,'isZoomDirectionReversed',[bool(v)],v)
    elif k == 'orbitScheme':
        c = ORBIT.get(kv);  applied[k] = try_set(gp,'panZoomOrbitShortcuts',[c],v) if c is not None else {'requested':v,'ok':False,'error':'unknown orbitScheme'}
    elif k == 'modelingOrientation':
        c = ORIENT.get(kv); applied[k] = try_set(gp,'defaultModelingOrientation',[c],v) if c is not None else {'requested':v,'ok':False,'error':'unknown modelingOrientation (yup/zup)'}
    elif k == 'gestureNav':
        applied[k] = try_set(gp,'isGestureBasedViewNavigationUsed',[bool(v)],v)
    elif k == 'cameraPivot':
        applied[k] = try_set(gp,'isCameraPivotEnabled',[bool(v)],v)
    elif k == 'lengthUnit':
        c = UNITS.get(kv)
        if c is None: applied[k] = {'requested':v,'ok':False,'error':'unknown unit (mm/cm/m/in/ft)'}
        else:
            try: applied[k] = try_set(design.fusionUnitsManager,'distanceDisplayUnits',[c],v)
            except Exception as e: applied[k] = {'requested':v,'ok':False,'error':str(e)}
    else:
        applied[k] = {'requested': v, 'ok': False, 'error': 'unknown preference key'}

result = {'applied': applied, 'current': read_current()}
print(json.dumps(result))
'''


def _shape_pref_result(res: dict, wrote: bool) -> dict:
    if not res or not res.get("success"):
        return {"success": False,
                "error": (res or {}).get("error", "preferences script did not run"),
                "_hint": "fusion_set_preference/get_preferences need Fusion running + signed in "
                         "(fusion_readiness -> ready:true). If needsSignin, drive the sign-in first."}
    data = (res.get("data") or {}).get("result") or {}
    applied = data.get("applied", {}) or {}
    current = data.get("current", {}) or {}
    out = {"success": True, "applied": applied, "current": current}
    if wrote:
        oks = [k for k, v in applied.items() if v.get("ok")]
        fails = {k: v.get("error") for k, v in applied.items() if not v.get("ok")}
        if fails:
            out["partial"] = True
            out["_hint"] = ("Applied live: %s. FAILED: %s. Note: some themes (Dark Gray/Classic) "
                            "aren't shipped in every Fusion build - use 'darkblue' or 'device'. See "
                            "`current` for the resulting state." %
                            (", ".join(oks) or "(none)",
                             "; ".join("%s=%s" % (k, e) for k, e in fails.items())))
        else:
            out["_hint"] = ("Applied LIVE (no restart needed): %s. See `current` for resulting state."
                            % ", ".join(oks))
    else:
        out["_hint"] = ("Current Fusion preferences. Change any with fusion_set_preference, keys: "
                        "theme (light/darkblue/darkgray/classic/device/dark), invertScrollZoom (bool), "
                        "orbitScheme (fusion360/alias/inventor/solidworks/tinkercad/powermill), "
                        "modelingOrientation (yup/zup), gestureNav (bool), cameraPivot (bool), "
                        "lengthUnit (mm/cm/m/in/ft - applies to the active design).")
    return out


def _run_pref_script(script: str, timeout: int) -> dict:
    """Proxy a preferences script to the add-in, retrying ONCE on a transient add-in
    connection error. The first pref call can hit the add-in mid doc-transition (e.g.
    right after opening a Library, when run_modeling_script must create a Design) and
    come back 'not running'/'not responding' even though Fusion is up - seen live on
    the fresh-VM battery, where a ~1.5s retry cleared it. A genuinely-down Fusion just
    costs one extra 1.5s try."""
    res = _proxy_to_addin("run_modeling_script", {"script": script}, timeout=timeout)
    if res and res.get("success"):
        return res
    err = ((res or {}).get("error") or "").lower()
    if any(s in err for s in ("not running", "not responding", "add-in",
                              "connection", "timed out", "refused")):
        _time.sleep(1.5)
        res2 = _proxy_to_addin("run_modeling_script", {"script": script}, timeout=timeout)
        if res2 is not None:
            return res2
    return res


def _handle_set_preference(fusion_info: dict, args: dict) -> dict:
    reqs = {k: v for k, v in (args or {}).items() if k not in ("settle",)}
    if not reqs:
        return {"success": False, "error": "No preferences given.",
                "_hint": "Pass e.g. {\"theme\":\"dark\"} or {\"invertScrollZoom\":true,"
                         "\"orbitScheme\":\"solidworks\"}. Call fusion_get_preferences to see current values + keys."}
    script = _PREF_SCRIPT.replace("__REQS__", json.dumps(json.dumps(reqs)))
    res = _run_pref_script(script, timeout=30)
    return _shape_pref_result(res, wrote=True)


def _handle_get_preferences(fusion_info: dict, args: dict) -> dict:
    script = _PREF_SCRIPT.replace("__REQS__", json.dumps(json.dumps({})))
    res = _run_pref_script(script, timeout=20)
    return _shape_pref_result(res, wrote=False)


# ── APS state + AI hint builder ───────────────────────────────────────────────
# Every cloud-file path routes through APS now (the old in-app search is disabled —
# see _handle_deprecated_cloud_search). This helper reads live APS state and returns
# a rich, self-teaching hint bundle so the AI ALWAYS knows: is APS installed/configured?
# signed in? token live/expired? what the verbs are, that it must sign in, and that the
# best sign-in path is the user's NATIVE Chrome/Edge via ABE (adom-browser-extension).
_APS_VERB_MAP = (
    "fusion_aps_status (state) · fusion_aps_signin (sign in) · fusion_aps_search {query} · "
    "fusion_aps_open {name} · fusion_aps_browse · fusion_aps_recent · fusion_aps_file_info · "
    "fusion_aps_versions · fusion_aps_download · fusion_aps_upload · fusion_aps_create_folder · "
    "fusion_aps_set_browser/get_browser/forget_browser (remember the native browser+profile)"
)
_APS_WHY = (
    "WHY APS: Autodesk offers NO fast file-search API for Fusion. The only in-app option "
    "(walk/search the folder tree from the add-in) takes 30+ min and CAN CRASH Fusion "
    "(WinError 10054). We searched far and wide; APS (Autodesk Platform Services, server-indexed) "
    "is the ONLY thing that works — seconds, no Fusion needed, no crash. It needs a one-time "
    "OAuth sign-in; once set up it is fast and amazing. The old crashing search is now DISABLED."
)
_APS_ABE = (
    "BEST sign-in path: drive the user's OWN Chrome/Edge (already logged into Autodesk) via ABE "
    "(adom-browser-extension). If ABE is not installed, tell the user to install it first "
    "(wiki.adom.inc/adom/adom-browser-extension), then walk them through it. With ABE the bridge "
    "opens the Autodesk auth URL in their real signed-in profile and captures the token — no "
    "password typing. Fallback: fusion_aps_signin {allowDefaultBrowser:true}."
)


# ── SEARCH ROUTER: one live-state answer to "which file-search should the AI use?" ───────────
# John, 2026-07-24: "you now have a lot of ways to search for files, so even the ai will get
# confused by which verb to use... are your hints smart, i.e. they know if aps is configured or
# not so the hint is dynamic and does a bunch of lookup work... and give hints where you can
# tell it ideas on things you could enable and the complexity of enabling it?"
#
# There are three ways to find a cloud file, and the right one DEPENDS ON LIVE STATE:
#   1. fusion_aps_search        - APS Data Management. Needs APS signed in. Works with Fusion
#                                 CLOSED, headless, no subscription. ~2s.
#   2. fusion_mcp_call read/document/search - Autodesk's MCP server. Needs Fusion RUNNING +
#                                 subscription + the MCP toggle on. ~2s.
#   3. fusion_search_cloud_files - DISABLED (30+ min walk, crashed Fusion). Never.
#
# _search_router() probes all of it live (cheap: APS token file read + a 0.4s loopback socket)
# and every relevant hint renders the SAME ranked answer, including what could be ENABLED and
# what enabling costs (APS signin = silent on a warm SSO session, else a browser consent;
# MCP = fusion_mcp_enable drives the Preferences dialog, ~10s, brief announced foreground).
def _probe_mcp_port(timeout: float = 0.4) -> bool:
    """Is Autodesk's MCP server listening on loopback right now? The bridge runs ON the box, so
    this is a direct, cheap socket probe - no relay round-trip."""
    import socket
    try:
        s = socket.socket()
        s.settimeout(timeout)
        s.connect(("127.0.0.1", 27182))
        s.close()
        return True
    except Exception:
        return False


def _search_router() -> dict:
    """Live state of every file-search path + a ranked recommendation. Never raises."""
    aps_st = _aps_quick_state()                      # {configured, signedIn} - token-aware
    mcp_on = _probe_mcp_port()
    fusion_up = mcp_on  # the MCP server lives inside Fusion; port open implies running
    if not mcp_on:
        try:
            fusion_up = _is_fusion_running()
        except Exception:
            fusion_up = False

    aps_ready = bool(aps_st.get("signedIn"))
    options = []
    if aps_ready:
        options.append(("fusion_aps_search", "ready NOW (~2s, works even with Fusion closed)"))
    if mcp_on:
        options.append(("fusion_mcp_call {tool:'fusion_mcp_read', arguments:{queryType:'document',"
                        "operation:'search', name:'...'}}", "ready NOW (~2s, MCP server is live)"))

    enable = []
    if not aps_ready:
        if aps_st.get("configured"):
            enable.append("APS: one fusion_aps_signin - SILENT if run right after a Fusion "
                          "sign-in (warm browser SSO), else a one-click browser consent")
        else:
            enable.append("APS: needs one-time setup - fusion_aps_set_client_id (once per "
                          "company) then fusion_aps_signin (once per user)")
    if not mcp_on:
        if fusion_up:
            enable.append("MCP: fusion_mcp_enable turns it on FOR the user (drives the "
                          "Preferences dialog, ~10s, brief announced foreground); needs a "
                          "Fusion subscription")
        else:
            enable.append("MCP: needs Fusion running first (fusion_start), then "
                          "fusion_mcp_enable; needs a Fusion subscription")

    if options:
        best = options[0][0].split(" ")[0].split("{")[0]
        rec = "BEST SEARCH RIGHT NOW: " + "; also ".join("%s - %s" % o for o in options) + "."
    else:
        best = None
        # nothing ready -> show ALL enable paths + costs, not just the quickest, so the AI can
        # pick (e.g. it must know MCP needs fusion_start when Fusion is down).
        rec = ("NO file search is ready right now. To enable one: "
               + " | ".join(enable) + "." if enable else
               "NO file search is ready and none can be enabled in the current state.")
    if enable and options:
        rec += " Could also enable: " + " | ".join(enable) + "."

    return {"apsReady": aps_ready, "apsConfigured": bool(aps_st.get("configured")),
            "mcpLive": mcp_on, "fusionRunning": fusion_up,
            "bestSearch": best, "searchHint": rec}


def _aps_state_hint(extra: str = "") -> dict:
    """Live APS readiness + a self-teaching hint bundle for the AI. Never raises."""
    try:
        st = aps.handle_status({})
        d = st.get("data", {}) if isinstance(st, dict) else {}
    except Exception as e:  # pragma: no cover - defensive
        d = {"error": str(e)}
    configured = bool(d.get("configured"))
    signed_in = bool(d.get("signedIn"))
    live = bool(d.get("tokenLive"))
    if not configured:
        stage, todo = "not_configured", ("Register a PKCE app at https://aps.autodesk.com (Data "
                                         "Management API on), then fusion_aps_set_client_id + fusion_aps_signin.")
    elif not signed_in:
        stage, todo = "not_signed_in", "Sign in: fusion_aps_signin (prefer ABE / native browser)."
    elif not live:
        stage, todo = "token_expired", "Token expired/refresh failed — fusion_aps_signin again."
    else:
        stage, todo = "ready", "Ready — fusion_aps_search {\"query\":\"...\"} or fusion_aps_open {\"name\":\"...\"}."
    # SMART ROUTING (John 2026-07-24): an APS problem must not dead-end the AI when another
    # search path is live right now. The router probes MCP + Fusion state and says what is
    # usable NOW vs what could be enabled and at what cost.
    router = _search_router()
    parts = [f"APS {stage}.", todo, router["searchHint"], "VERBS: " + _APS_VERB_MAP, _APS_ABE, _APS_WHY]
    if extra:
        parts.insert(0, extra)
    return {
        "apsStage": stage, "apsConfigured": configured, "apsSignedIn": signed_in,
        "apsTokenLive": live, "searchRouter": router, "_hint": "  ".join(parts),
    }


def _aps_guarded(fn, args: dict):
    """Run an APS cloud handler, but FIRST ensure APS is signed-in + token-live. If not,
    short-circuit with the rich self-teaching hint bundle (state + verbs + sign-in + ABE)
    so the AI knows exactly what to do instead of getting an opaque auth failure. This is
    the 'check every time, in code' guard the user asked for."""
    state = _aps_state_hint()
    if state["apsStage"] != "ready":
        return {
            "success": False,
            "error": f"APS is {state['apsStage']} — sign in before cloud file operations.",
            "output": state["apsStage"],
            "data": {"apsNotReady": True, **state},
            "_hint": state["_hint"],
        }
    return fn(args)


def _handle_deprecated_cloud_search(command: str, args: dict) -> dict:
    """HARD-BLOCK the old in-app cloud search/walk (crashes Fusion). Redirect to APS,
    surfacing live APS state so the AI can immediately continue via the good path."""
    aps_state = _aps_state_hint()
    return {
        "success": False,
        "error": (f"fusion_{command} is DISABLED: the in-app Fusion cloud "
                  "search/walk takes 30+ min and CAN CRASH Fusion (WinError 10054). "
                  "Use fusion_aps_search {\"query\":\"...\"} (or fusion_aps_open to open by name) instead."),
        "output": "deprecated_cloud_search_disabled",
        "data": {"disabled": True, "use": "fusion_aps_search", **aps_state},
        "_hint": aps_state["_hint"],
    }


COMMAND_HANDLERS = {
    "describe": lambda fi, args: describe.handle_describe(args),
    "readiness": _handle_fusion_readiness,
    "set_preference": _handle_set_preference,
    "get_preferences": _handle_get_preferences,
    "install_fusion": _handle_install_fusion,
    "open_design": handle_open_design,
    "close": handle_close_fusion,  # deprecated alias - use stop (graceful) / kill (force)
    "stop": handle_fusion_stop,    # graceful: close docs + WM_CLOSE, NO force-kill
    "kill": handle_fusion_kill,    # force: taskkill /F (the desperate path)
    "launch": _handle_launch,
    "start": _handle_launch,  # alias — CLI's fusion_start delegates here on Docker
    "dismiss_recovery": _handle_dismiss_recovery,
    "relocate_recovery": _handle_relocate_recovery,
    "open_cloud_file": _handle_open_cloud_file,
    # Import a legacy EAGLE .sch (+ .brd) into a NEW populated Fusion electronics design via
    # Fusion's own ImportSCHAndBRDCmd (drives both Open dialogs in the background). This is the
    # ONLY way to author a board from EAGLE source - newDesignFromLocal opens the editor but does
    # NOT instantiate parts, and the .fsch/.fbrd binary container can't be built offline.
    "new_electronics_from_eagle": lambda fi, args: _handle_new_electronics_from_eagle(args),
    "screenshot_fusion": _handle_screenshot_fusion,
    "click_fusion": _handle_click_fusion,
    "send_key": _handle_send_key,
    "close_window": _handle_close_window,
    "window_info": _handle_window_info,
    "screenshot_all": _handle_screenshot_all_fusion,
    # On-demand dialog/owned-popup array: enumerate + screenshot every modal so the AI
    # can ANALYZE before acting. Use while polling a long op (e.g. a Hub upload) or any
    # time you suspect a dialog is up. Mutating verbs attach this automatically; this is
    # the manual entry point. See the fusion-driving skill.
    "check_dialogs": lambda fi, args: _with_fusion_alerts(
        _capture_dialog_array(settle=float(args.get("settle", 0.3)))
        or {"success": True, "dialogsDetected": 0, "dialogs": []},
        float(args.get("alertsSinceSec", 300))),
    # Fusion's own error/warning toasts (the CEF overlays no window enumeration can see).
    "managed_library_create": lambda fi, args: _handle_managed_library_create(fi, args),
    "web_signin": lambda fi, args: _handle_web_signin(fi, args),
    "managed_library_push": lambda fi, args: _handle_managed_library_push(fi, args),
    "managed_library_pull": lambda fi, args: _handle_managed_library_pull(fi, args),
    "managed_library_link": lambda fi, args: _handle_managed_library_link(fi, args),
    "managed_library_unlink": lambda fi, args: _handle_managed_library_unlink(fi, args),
    "library_manager": lambda fi, args: _handle_library_manager(fi, args),
    "alerts": lambda fi, args: {
        "success": True,
        "alerts": _fusion_alerts(float(args.get("sinceSec", 300))),
        "logFile": _fusion_log_path(),
        "diagnostics": (lambda _d: (_fusion_log_path(_d), _d)[1])({}),
        "_hint": ("Fusion draws many errors as in-canvas CEF overlays with NO top-level "
                  "window, so check_dialogs alone cannot see them; these come from Fusion's "
                  "own AppLogFile 'New Alert message' entries. An empty list means Fusion "
                  "raised no toast in the window - not that a command succeeded."),
    },
    "addin_status": lambda fi, args: _handle_addin_status(),
    # LAST-RESORT human escalation (John 2026-07-07): when the bridge is truly blocked
    # on the user (password/2FA, UAC), send an ab toast that reaches their MAIN machine
    # (reach_user=True fans out to peer ADs, so a bridge running on an unattended VM
    # still lands the toast where the user actually is). ALWAYS exhaust programmatic
    # options first - this exists so the AI has ONE deterministic call when it must ask.
    "notify_owner": lambda fi, args: _handle_notify_owner(args),
    # APS cloud search (pure HTTPS, works with Fusion closed). Lives in
    # COMMAND_HANDLERS so it returns BEFORE any Fusion-running gate.
    "aps_status": lambda fi, args: aps.handle_status(args),
    "aps_set_client_id": lambda fi, args: aps.handle_set_client_id(args),
    "aps_signin": lambda fi, args: aps.handle_signin(args),
    "aps_set_browser": lambda fi, args: aps.handle_set_browser(args),
    "aps_get_browser": lambda fi, args: aps.handle_get_browser(args),
    "aps_forget_browser": lambda fi, args: aps.handle_forget_browser(args),
    # Cloud-data verbs go through _aps_guarded: it checks signed-in + token-live EVERY call
    # and returns the rich APS hint bundle (state/verbs/sign-in/ABE) if not ready.
    "aps_search": lambda fi, args: _aps_guarded(aps.handle_search, args),
    "aps_browse": lambda fi, args: _aps_guarded(aps.handle_browse, args),
    "aps_recent": lambda fi, args: _aps_guarded(aps.handle_recent, args),
    "aps_file_info": lambda fi, args: _aps_guarded(aps.handle_file_info, args),
    "aps_versions": lambda fi, args: _aps_guarded(aps.handle_versions, args),
    "aps_download": lambda fi, args: _aps_guarded(aps.handle_download, args),
    "aps_create_folder": lambda fi, args: _aps_guarded(aps.handle_create_folder, args),
    "aps_upload": lambda fi, args: _aps_guarded(aps.handle_upload, args),
    "aps_open": lambda fi, args: _aps_guarded(lambda a: _handle_aps_open(fi, a), args),
    "aps_get": lambda fi, args: aps.handle_get(args),
    # DEPRECATED cloud search — HARD-BLOCKED here (in COMMAND_HANDLERS) so they return
    # BEFORE ever reaching the crashing add-in path. Redirect to APS + surface APS state.
    "search_cloud_files": lambda fi, args: _handle_deprecated_cloud_search("search_cloud_files", args),
    "walk_cloud_tree": lambda fi, args: _handle_deprecated_cloud_search("walk_cloud_tree", args),
}


# Surfaced inline so the AI doesn't draw the WRONG conclusion (it has, repeatedly):
# a "Read Only" / expired / trial / personal-use Fusion CAN open + view + browse files
# (Basic Access ~365 days) — it only blocks save/export/modify. NEVER blame a failed/slow
# OPEN on the subscription; the cause is the open path (URN resolution, the Electronics
# design picker, a slow assembly download, a stuck main thread).
_READONLY_OPEN_NOTE = ("NOTE: a 'Read Only'/expired/trial Fusion still opens+views files — if "
                       "it never opens, debug the open path (URN/picker/slow assembly), NOT the "
                       "license. Only save/export are blocked in read-only.")


def _handle_aps_open(fusion_info: dict, args: dict) -> dict:
    """Search the cloud (APS) for a file, then OPEN the best match in Fusion.

    APS finds it instantly by name across the whole team hub; the add-in's
    open_cloud_file (projectName + fileName + fileExtension) opens it.
    """
    query = (args.get("query") or args.get("fileName") or "").strip()
    if not query:
        return {"success": False, "error": "Missing query/fileName."}
    match = aps.find_one(query)
    if not match:
        return {"success": False, "error": f"No cloud file matching '{query}'.",
                "_hint": "Run fusion_aps_search to see candidates, or refine the query. "
                         + _READONLY_OPEN_NOTE}
    # Fusion cloud displayNames are NOT filenames — do NOT splitext (it mangled
    # "...(1.6mm gasket)" into a bogus extension). Pass the full name.
    name = match.get("name") or ""
    open_args = {"projectName": match.get("projectName"), "fileName": name}
    if not fusion_info.get("installed") or not _is_fusion_running():
        return {"success": True, "output": f"Found '{name}' in project {match.get('projectName')}.",
                "data": {"match": match, "openArgs": open_args},
                "_hint": "Fusion isn't running — call fusion_start, then fusion_aps_open again to open it."}
    # Open by the EXACT file URN (works for any nesting). The FIRST cloud-open can
    # take 60-90s (download) — longer than ab's relay timeout — so FIRE it in the
    # background and return immediately. Caller polls fusion_get_app_state. Unless
    # {wait:true} is passed (then block and return the open result).
    urn = match.get("id")

    def _do_open():
        r = _proxy_to_addin("open_by_urn", {"urn": urn}, timeout=180)
        if not r.get("success"):
            _proxy_to_addin("open_cloud_file", open_args, timeout=120)  # by-name fallback
        return r

    if args.get("wait"):
        result = _do_open()
        result.setdefault("data", {})
        if isinstance(result.get("data"), dict):
            result["data"]["match"] = match
        return _post_open_screenshot(result) if result.get("success") else result

    import threading as _threading
    _threading.Thread(target=_do_open, daemon=True).start()
    return {
        "success": True,
        "output": f"Found '{name}' in project {match.get('projectName')} — opening in Fusion.",
        "data": {"match": match, "opening": True},
        "statusVerb": "fusion_get_app_state",  # ab 1.9.9 convention — poll this for completion
        "_hint": "The cloud file is opening in Fusion in the background (first open can take "
                 "~60-90s; large ASSEMBLIES download all referenced parts and take longer). "
                 "Poll fusion_get_app_state until activeDocument is the file. Pass "
                 "{\"wait\": true} to block instead. " + _READONLY_OPEN_NOTE,
    }

# Commands that are proxied to the Fusion add-in (port 8774).
# Keys are the CLI-facing names (after stripping "fusion_" prefix).
ADDIN_COMMANDS = {
    "activity_banner",
    # Mechanical BOM + physical properties (PR #22, Oliver / BOM Forge). These live in the
    # ADD-IN, so they must be listed here or the bridge never proxies them and the verb
    # 404s no matter how well the handler works.
    "assembly_bom", "physical_properties",
    "get_app_state",
    "document_info",
    "activate_document",
    "import_step",  # add-in calls this "import_file" — mapped below
    "export_step", "export_stl", "export_3mf", "export_f3d", "export_fbx", "export_usdz",
    "export_dxf", "export_dwg", "export_iges", "export_obj", "export_sat", "export_skp",
    "get_design_info",
    "get_parameters", "set_parameter",
    "take_screenshot",
    # Electronics commands (EAGLE via Electron.run)
    "electron_run", "execute_text_command",
    "electron_zoom", "electron_pan", "electron_select",  # video-friendly view control
    "open_electronics", "list_text_commands",
    # Electronics source export (.fsch, .fbrd, .flbr via Document.CopyToDesktop)
    "export_source",
    # EAGLE-format export (.sch, .brd — extracted from .fsch/.fbrd ZIP container)
    "export_eagle_source",
    # Library file commands (EXPORT SCRIPT — open_lbr is orchestrated at bridge level)
    "export_lbr",
    # Document management
    "close_document",
    "close_all_documents",
    # Electronics file opening (open_schematic, open_board, show_3d_board, show_2d_board
    # are orchestrated at bridge level for auto-screenshot — not in this set)
    # Board data query
    "board_info",
    # Open any cloud file directly by its APS/Fusion URN (any folder depth).
    # fusion_aps_open also proxies this internally (fire-and-poll); registering it
    # here makes the direct fusion_open_by_urn verb work too instead of being
    # rejected as "Unknown command".
    "open_by_urn",
    # In-app parametric modeling — run an adsk.fusion script in the live session
    # (free path to programmatic CAD; the APS Fusion Automation API is the paid
    # cloud alternative).
    "run_modeling_script",
    # Cloud document management
    "save_to_cloud",
    "list_cloud_projects",
    "list_cloud_files",
    "delete_cloud_file",
    "create_cloud_folder",
    # open_cloud_file is orchestrated at bridge level (not direct proxy)
    # to detect blocking dialogs after open
    "check_recovery",
    "search_cloud_files",
    "walk_cloud_tree",
    "export_cloud_file",
    # Manufacturing exports
    "export_bom",
    "export_cpl",
    "export_gerbers",
    "export_board_image",
    "detect_layers",
}

# Map CLI command names → add-in command names (where they differ)
ADDIN_COMMAND_MAP = {
    "import_step": "import_file",
}

# Per-command timeout overrides for _proxy_to_addin. Heavy 3D exports on
# panelized boards (100+ placements) can take 120s+. The add-in-side
# timeout in http_server.py should be the source of truth; these are
# matched to that so urllib doesn't cut off before the add-in does.
ADDIN_COMMAND_TIMEOUTS = {
    "show_symbol": 180, "show_footprint": 180, "show_3d_package": 300, "make_test_project": 300, "bind_library_3d": 480, "show_library": 240, "show_project": 240,
    "demo_progress": 15,
    # getPhysicalProperties runs on Fusion's MAIN thread per component, so a big assembly
    # (or assembly_bom with includePhysicalProperties) is slow. Give both real headroom.
    "assembly_bom": 300,
    "physical_properties": 300,
    "export_step": 300,
    "export_iges": 300,
    "export_sat": 300,
    "export_stl": 300,
    "export_3mf": 300,
    "export_usdz": 300,
    "export_obj": 300,
    "export_f3d": 300,
    "export_fbx": 300,
    "export_skp": 300,
    "export_dxf": 120,
    "export_dwg": 120,
    "export_gerbers": 180,
    "export_bom": 60,
    "export_cpl": 60,
    "export_board_image": 60,
    "close_all_documents": 60,
    # Cloud tree walker / search — can hit hundreds of folders on large projects.
    # Must match or exceed http_server.py PER_COMMAND_TIMEOUT so urllib doesn't
    # cut off before the add-in does.
    "walk_cloud_tree": 600,
    "search_cloud_files": 180,
    # Opening a cloud design downloads + loads it; large assemblies and
    # electronics/PCB designs (which spin up the Electronics editor) can take
    # minutes. fusion_aps_open's fire-and-poll path is the preferred way in for
    # these — but when open_by_urn is proxied synchronously, give it room.
    "open_by_urn": 240,
    # Modeling scripts can build many features; give them room without being unbounded.
    "run_modeling_script": 180,
}

# ── Busy gate: prevent command stacking during long-running add-in work ──
# When walk_cloud_tree or search_cloud_files is running, the Fusion main thread
# is blocked for 30-300+ seconds. Any add-in command sent during that time would
# pile up behind _main_thread_lock in http_server.py, eating HTTP threads and
# potentially crashing the host. The gate rejects those commands immediately at
# the bridge level with progress info, BEFORE they reach the add-in.
import threading as _threading
import time as _time

_long_command_lock = _threading.Lock()
_long_command = None  # None or {"command": str, "startedAt": float}

LONG_RUNNING_COMMANDS = {"walk_cloud_tree", "search_cloud_files"}
# ⛔ COMMANDS THAT MUST RIDE THROUGH A BUSY MAIN THREAD (John, 2026-08-28, "prove to me
# it's working"). The activity banner exists to be shown DURING the long stages - bind,
# prove - and those stages hold the main thread. Both busy gates below refused it with
# main_thread_busy, so the feature could never appear at the one moment it was for: the
# chain fired it, the gate ate it, and my helper swallowed the error by design. Found only
# because a live run was watched with the palette registry open, not by trusting the
# helper's silence. A banner update is a sub-millisecond htmlContent set that is safe to
# queue behind a long command; it is the opposite of what the gate protects against.
BUSY_EXEMPT_COMMANDS = {"activity_banner"}

# Commands that change Fusion's state and can pop a modal dialog / owned popup the AI
# must see (a save confirm, the Hub "are you sure you want to close?" data-loss prompt,
# a recovery prompt, etc.). After these, the dispatcher auto-attaches the dialog array
# (_capture_dialog_array) + an analyze-this hint so the AI cannot fly blind. Read-only
# commands (get_app_state, document_info, board_info, exports) are intentionally excluded
# to avoid the per-call screenshot latency. See the fusion-driving skill.
MUTATING_COMMANDS = {
    "run_modeling_script", "execute_text_command", "electron_run",
    "close_document", "close_all_documents", "import_step",
    "set_parameter", "save_to_cloud", "delete_cloud_file",
}


def _set_long_command(command: str):
    with _long_command_lock:
        global _long_command
        _long_command = {"command": command, "startedAt": _time.time()}


def _clear_long_command():
    with _long_command_lock:
        global _long_command
        _long_command = None


def _get_long_command() -> dict | None:
    with _long_command_lock:
        if _long_command is None:
            return None
        return dict(_long_command)


def _get_busy_progress() -> dict | None:
    """Poll the add-in /status endpoint for walkProgress during a long command.

    Uses /status (not /health) because it's lighter and proven reliable under
    GIL contention during heavy walks. 3s timeout matches _check_addin_status.
    """
    status = _check_addin_status(timeout=3.0)
    if status and status.get("walkProgress"):
        return status["walkProgress"]
    return None


def _check_main_thread_blocked() -> bool:
    """Quick check: is Fusion's main thread blocked by a modal dialog?

    Sends a fast command (get_app_state) with a short timeout. If the add-in's
    HTTP server responds but the command times out, a modal dialog is blocking.
    Returns True if blocked, False if responsive.
    """
    try:
        body = json.dumps({"command": "get_app_state", "args": {}}).encode("utf-8")
        req = urllib.request.Request(
            f"http://127.0.0.1:{ADDIN_PORT}/command",
            data=body,
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(req, timeout=3) as resp:
            result = json.loads(resp.read())
        # If we got a response, main thread is fine
        if result.get("success"):
            return False
        # Add-in responded but with an error — check if it's a timeout
        if "timed out" in result.get("error", "").lower():
            return True
        return False
    except Exception as e:
        # HTTP timeout = main thread blocked (HTTP server is up but command didn't complete)
        if "timed out" in str(e).lower() or "timeout" in str(e).lower():
            return True
        # Connection refused = add-in not running (different problem)
        return False



def _addin_start_failure() -> dict | None:
    """The add-in's own start-failure marker (%TEMP%/adom-fusion/addin-start-failed.json),
    written by AdomBridge.run() when an import blew up (issue #75: a stale copy in the legacy
    AddIns dir). Surfaced by fusion_readiness and fusion_addin_status until the add-in starts
    cleanly again (a clean start deletes the marker)."""
    try:
        base = os.environ.get("TEMP") or os.path.expanduser("~")
        fp = os.path.join(base, "adom-fusion", "addin-start-failed.json")
        if not os.path.isfile(fp):
            return None
        with open(fp, "r", encoding="utf-8") as fh:
            m = json.load(fh)
        m["markerPath"] = fp
        m["_hint"] = ("Fusion loaded a BROKEN copy of the add-in from %s. The bridge re-syncs every "
                      "AddIns dir on start; restart Fusion with fusion_restart_for_addin {save:true} "
                      "so it loads the repaired copy. If it keeps failing, delete that folder and "
                      "restart Fusion: the bridge reinstalls it." % m.get("addinDir"))
        return m
    except Exception:
        return None

def _probe_addin(timeout: float = 0.5) -> dict | None:
    """Check if the Fusion add-in HTTP server is running.

    Uses a short timeout (default 0.5s) to avoid blocking the /health endpoint.
    The add-in runs on localhost so if it's up, it responds in <50ms.
    """
    try:
        req = urllib.request.Request(f"http://127.0.0.1:{ADDIN_PORT}/health", method="GET")
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.loads(resp.read())
    except Exception:
        return None


def _check_addin_status(timeout: float = 3.0) -> dict | None:
    """GET /status from the add-in. Returns dict or None.

    This is the cross-bridge busy probe — reads add-in busy state without
    acquiring the main thread lock. Safe to call from any bridge/container.
    Usually ~50ms, but during heavy walks GIL contention can push to 1-2s.
    """
    try:
        req = urllib.request.Request(f"http://127.0.0.1:{ADDIN_PORT}/status", method="GET")
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.loads(resp.read())
    except Exception:
        return None


_HUNG_SINCE = [0.0]  # first time is_fusion_hung() reported hung; 0 = not currently hung

# LONG-OP REGISTRY (#49 follow-up, 2026-08-22, from the ab matrix run's timing table): the
# bridge KNOWS when its own create_library chain is mid-flight, and a mint's cloud saveAs on
# a fresh account blocks Fusion's main thread without pumping for MINUTES - which is exactly
# what kill-grade hang "evidence" looks like (IsHungAppWindow true + 20s probe refused +
# 180s persistence). Parts died at ~400s with "no frames, bind null": the hang gate killed a
# WORKING Fusion mid-mint when a concurrent dismiss/status call hit the dispatch preamble.
# While this registry shows an op in flight (younger than the 900s ceiling), a non-pumping
# main thread is EXPECTED and _confirmed_hung must answer not-hung.
_LONG_OP = {"name": None, "since": 0.0}
# CANCEL A RUN (John, 2026-08-29: "you need a way to cancel a large run. that's a bad
# experience. a user will need cancel from the web controls"). A 31-part import had no way
# out short of killing the bridge. fusion_cancel_run marks the run here; the bind loop and
# the prove loop check between parts and stop at the next part boundary, keeping every urn
# already minted, closing scratch, and leaving the library open. runId -> {at, by}.
_CANCEL_REQUESTS = {}


def _cancel_requested(run_id: str):
    return _CANCEL_REQUESTS.get(str(run_id or "")) if run_id else None


def _confirmed_hung() -> dict:
    """A kill-grade hang verdict. IsHungAppWindow ALONE must never authorize a kill.

    Found the hard way (2026-08-21, three dead Fusions in one afternoon): a heavy
    main-thread modeling script (STEP import, saveAs) starves the add-in's HTTP thread
    via the GIL, so the 3s status probe AND the 5s health probe both time out, and the
    host window legitimately stops pumping messages - IsHungAppWindow goes true on a
    process that is WORKING. The old gate then taskkilled a healthy Fusion mid-bind.
    Same law as the busy guard (#41): a remediation that destroys state needs a
    STRONGER signal than one snapshot.

    Kill-grade means ALL of: (1) is_fusion_hung() true NOW, (2) a LONG status probe
    (20s) gets nothing - a merely GIL-starved add-in answers intermittently within
    that, (3) the condition has PERSISTED for 180s+ since first observed. Anything
    less returns hung:false with a `phase` explaining what to do (wait).
    """
    import time as _t
    # A bridge-dispatched long op (create_library mint/bind) legitimately wedges the main
    # thread for minutes - never declare hung while one is in flight and under its ceiling.
    # PROGRESS-KEYED, not a fixed wall-clock (#49 part 10, John 2026-08-23: a 94-connect part's
    # first-ever cloud mint ran ~1600s and the old 900s wall turned the gate back into the kill
    # the registry exists to prevent). Under the (raised) ceiling: always protected. PAST it:
    # still protected as long as Fusion is actively burning CPU (a slow-but-working mint) - only
    # a long-op that is BOTH past the ceiling AND idle (genuinely wedged) falls through to the
    # kill-grade rules below.
    if _LONG_OP["since"]:
        _lo_el = _t.time() - _LONG_OP["since"]
        _protected = _lo_el < 1800
        if not _protected:
            try:
                from fusion_detect import fusion_using_cpu
                _protected = fusion_using_cpu()
            except Exception:
                _protected = False
        if _protected:
            return {"hung": False, "phase": "long-op-in-flight",
                    "longOp": _LONG_OP["name"], "longOpElapsedSec": round(_lo_el, 1),
                    "keptBy": ("under-ceiling" if _lo_el < 1800 else "fusion-cpu-active")}
    try:
        from handlers.fusion_ui import is_fusion_hung
        _h = is_fusion_hung()
    except Exception:
        _h = {"hung": False, "signals": []}
    if not _h.get("hung"):
        _HUNG_SINCE[0] = 0.0
        return {"hung": False, "phase": "pumping"}
    if _check_addin_status(timeout=20.0) is not None:
        _HUNG_SINCE[0] = 0.0
        return {"hung": False, "phase": "busy-not-hung",
                "signals": _h.get("signals")}
    now = _t.time()
    if not _HUNG_SINCE[0]:
        _HUNG_SINCE[0] = now
    held = now - _HUNG_SINCE[0]
    if held < 180:
        return {"hung": False, "phase": "suspect", "heldSeconds": round(held, 1),
                "signals": _h.get("signals")}
    return {"hung": True, "phase": "confirmed", "heldSeconds": round(held, 1),
            "signals": _h.get("signals")}


def _handle_addin_status() -> dict:
    """Bridge-level handler for addin_status — wraps /status in standard format."""
    status = _check_addin_status(timeout=3.0)
    if status is None:
        # (2026-08-29) With the add-in's server stopped this verb answered status:"ok"
        # because the None was swapped for {"busy": False}. A probe that got no answer
        # must say so.
        return {"success": False, "status": "unreachable", "errorCode": "addin_not_answering",
                "error": "The Adom add-in did not answer on port %s. Fusion is %srunning."
                         % (ADDIN_PORT, "" if _is_fusion_running() else "NOT "),
                "_hint": "If Fusion is running, the add-in was installed or updated after launch "
                         "(or its server stopped): fusion_restart_for_addin {save:true} asks "
                         "and restarts. If Fusion is not running, fusion_start."}
    stale = _addin_staleness(status.get("version"))
    status = {**status, **stale}
    result = {"success": True, "output": json.dumps(status), **status}
    if stale.get("addinStale"):
        result["_hint"] = stale["_staleHint"]
    _asf = _addin_start_failure()
    if _asf and isinstance(result, dict):
        result["addinStartFailed"] = _asf
    return result


# Raw Fusion API failures (issue #289 item 3, Oliver). Fusion raises RuntimeError with text like
# "3 : Part Design documents can only contain one component" - no code, no remedy - so an AI has to
# infer the fix, and the first of these invalidates a whole modelling approach. Same idea as the
# dialog classifier, applied one layer down: match the text, attach a STABLE code and the real fix.
_API_ERROR_PATTERNS = (
    ("part design documents can only contain one component", "part_template_single_component",
     "This document came from Fusion's PART template, which permits exactly ONE component, so "
     "occurrences cannot be added. Documents created via documents.add(FusionDesignDocumentType) "
     "ARE assembly-capable. Either model with BODIES inside the single component, or create a new "
     "document through the API and build there. Fusion's API and UI defaults differ here, which is "
     "why this is surprising."),
    ("root component name cannot be changed", "root_rename_unsupported",
     "The root component always takes the DOCUMENT's name and cannot be renamed directly. Rename "
     "the document on save instead, or rename a sub-occurrence."),
    ("refers to a deleted object", "stale_api_handle",
     "A handle you are holding points at an object that was closed or deleted. Re-fetch it from "
     "the CURRENT app.activeDocument / activeProduct instead of reusing one from an earlier call."),
)


def _classify_api_error(result: dict) -> dict:
    """Give a raw Fusion API exception a stable errorCode + an actionable remedy (issue #289)."""
    if not isinstance(result, dict) or result.get("success") or result.get("errorCode"):
        return result
    data = result.get("data") if isinstance(result.get("data"), dict) else {}
    blob = " ".join(str(x) for x in (result.get("error", ""), result.get("output", ""),
                                     data.get("traceback", ""))).lower()
    for needle, code, hint in _API_ERROR_PATTERNS:
        if needle in blob:
            result["errorCode"] = code
            result["_hint"] = hint
            break
    return result


# Purchased-subassembly defaults for assembly_bom (issue #289 item 4, Oliver). Fusion's own
# Manage -> BOM renders EVERY subassembly as one collapsible row; the add-in shipped with only
# "with fasteners", so a purchased bearing or pulley unit was descended into and emitted as its
# modeled internals (240 Bearing Ball rows on Wirebening desk, which Fusion never shows at that
# level). Widened HERE at the bridge so no add-in redeploy is needed. An explicit caller-supplied
# treatAsUnit always wins. Note partNumber is NOT a usable signal: Fusion auto-fills it from the
# component name, so the bearing balls carry one too.
_ASSEMBLY_BOM_UNIT_DEFAULTS = ["with fasteners", "with fastener", "with bearings",
                               "bearing", "idler pulley", "pulley", "idler"]


_SESSION_SNAPSHOT_DONE = [False]


_LAST_KNOWN_DOCS = {"docs": None, "at": 0.0}   # [(name, isSaved)] from the last good get_app_state


def _snapshot_session_docs(st: dict):
    """Record what was open the FIRST time this bridge process heard from the add-in: those
    documents are the user's; everything the bridge opens later is registered as its own."""
    try:
        docs = (st.get("data") or {}).get("documents")
        if isinstance(docs, list):
            import time as _tl
            _LAST_KNOWN_DOCS["docs"] = [(str((d or {}).get("name") or ""), bool((d or {}).get("isSaved")))
                                        for d in docs]
            _LAST_KNOWN_DOCS["at"] = _tl.time()
    except Exception:
        pass
    if _SESSION_SNAPSHOT_DONE[0]:
        return
    try:
        docs = (st.get("data") or {}).get("documents")
        if not isinstance(docs, list):
            return
        for d in docs:
            n = str((d or {}).get("name") or "").lower()
            if n and not _is_bridge_doc_text(n) and not n.startswith("untitled"):
                _SESSION_DOCS_AT_START.add(n)
        _SESSION_SNAPSHOT_DONE[0] = True
    except Exception:
        pass


def _proxy_to_addin(command: str, args: dict, timeout: int = 30) -> dict:
    """Proxy a command to the Fusion add-in HTTP server.

    On timeout, probes /health to distinguish:
    - Add-in alive but main thread blocked (modal dialog) → distinct error
    - Add-in HTTP server crashed → connection error
    """
    if command == "assembly_bom" and not args.get("treatAsUnit"):
        args = dict(args)
        args["treatAsUnit"] = list(_ASSEMBLY_BOM_UNIT_DEFAULTS)

    body = json.dumps({"command": command, "args": args}).encode("utf-8")
    req = urllib.request.Request(
        f"http://127.0.0.1:{ADDIN_PORT}/command",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    # Save-prompt sentinel (issue #30): doc-switching commands can pop Fusion's unsaved-doc
    # modal mid-flight, orphaning this very call and wedging the add-in main thread. While
    # the call runs, a watcher auto-dismisses the prompt (bridge-created docs ONLY).
    _sentinel = None
    try:
        if command in _DOC_SWITCHING_ADDIN:
            _sentinel = _SavePromptSentinel(timeout + 10)
            _sentinel.__enter__()
    except Exception:
        _sentinel = None
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            result = json.loads(resp.read())

        if command == "get_app_state" and isinstance(result, dict) and result.get("success"):
            _snapshot_session_docs(result)
        # Check if the add-in itself returned a timeout (main thread didn't respond)
        if (not result.get("success")
                and "timed out" in result.get("error", "").lower()):
            return _diagnose_addin_timeout(command, result)

        # If the add-in rejected the command as unknown/unsupported, it may be a
        # STALE add-in that predates this verb (issue #55 - Drew's silent failure:
        # old add-in had no open_by_urn). Enrich with a version comparison + the
        # re-sync hint + a stable errorCode so the failure is LOUD, not silent.
        if not result.get("success"):
            err = (result.get("error") or "").lower()
            if any(s in err for s in ("unknown command", "no such command", "not supported", "unsupported command")):
                stale = _addin_staleness((_check_addin_status(timeout=1.0) or {}).get("version"))
                if stale.get("addinStale"):
                    result["errorCode"] = "addin_stale"
                    result["_hint"] = stale["_staleHint"]
                    result.update({k: stale[k] for k in ("addinVersion", "expectedAddinVersion", "addinStale")})

        # Raw Fusion API exceptions arrive as bare "3 : <text>" RuntimeError strings with no
        # errorCode and no remedy (issue #289 item 3). Classify them here, one layer below the
        # dialog classifier and in the same shape, so a caller can branch on a stable code.
        result = _classify_api_error(result)

        return result

    except urllib.error.URLError as e:
        if "Connection refused" in str(e) or "No connection" in str(e):
            return {
                "success": False,
                "error": "Fusion 360 AdomBridge add-in not running. "
                         "Install it with: python plugins/fusion360/install_addin.py, "
                         "then restart Fusion 360.",
            }
        # Could be a socket timeout — the HTTP request itself took too long
        if "timed out" in str(e).lower() or "timeout" in str(e).lower():
            return _diagnose_addin_timeout(command, {"error": str(e)})
        return {"success": False, "error": f"Add-in request failed: {e}"}
    except Exception as e:
        if "timed out" in str(e).lower() or "timeout" in str(e).lower():
            return _diagnose_addin_timeout(command, {"error": str(e)})
        return {"success": False, "error": f"Add-in request failed: {e}"}
    finally:
        # Grace-stop, not hard-stop: the wedge case is the prompt popping right AFTER the
        # caller's budget expired, so the watcher lingers ~12s past the return.
        if _sentinel is not None:
            try:
                _sentinel.soft_stop(12.0)
            except Exception:
                pass


def _diagnose_addin_timeout(command: str, original_result: dict) -> dict:
    """After a command timeout, probe /health to determine the cause.

    Returns a distinct error if the add-in is alive but its main thread
    is blocked (e.g. by a modal dialog in Fusion).  Always captures
    auto-screenshots of Fusion windows on timeout so the AI can see
    what dialog is blocking.
    """
    # Auto-screenshot on timeout — the most common cause is a blocking dialog
    # that's invisible to the API.  Capture Fusion windows so the AI can
    # identify and dismiss the dialog.
    timeout_screenshots = {}
    try:
        timeout_screenshots = _post_open_screenshot(command)
    except Exception:
        pass  # Best effort — don't let screenshot failure mask the real error

    # Identify WHICH modal is blocking (titles enumerate over Win32 even while the
    # add-in's main thread is stuck). Turns the opaque "add-in not responding" into
    # an actionable cause + resolution — and stops the needless restart loop.
    blocking_dialogs = classify_blocking_dialogs()

    health = _probe_addin(timeout=2.0)
    if health and health.get("status") == "ok":
        main_thread = health.get("main_thread", "unknown")
        pending = health.get("pending_commands", 0)
        if main_thread == "blocked" or pending > 0:
            # Before assuming a modal dialog, check /status — if the add-in
            # is busy with a known command, it's working (not dialog-blocked).
            status = _check_addin_status(timeout=3.0)
            if status and status.get("busy"):
                busy_cmd = status.get("busyCommand", "unknown")
                elapsed = status.get("elapsedSeconds", 0)
                walk = status.get("walkProgress")
                resp = {
                    "success": False,
                    "error": f"Fusion main thread busy — {busy_cmd} running for {elapsed}s.",
                    "errorCode": "main_thread_busy",
                    "busyCommand": busy_cmd,
                    "elapsedSeconds": elapsed,
                    "_hint": (
                        "Add-in is busy with a long-running command. Do NOT retry add-in commands. "
                        "Do NOT press Escape — the add-in is working, not stuck on a dialog. "
                        "Commands that still work: fusion_window_info, fusion_screenshot_fusion, "
                        "fusion_click_fusion, fusion_send_key, fusion_close_window."
                    ),
                }
                if walk:
                    resp["progress"] = walk
                return resp

            # If we recognized the blocking modal, name it and give the precise fix
            # instead of the generic "read the screenshots" guidance.
            if blocking_dialogs:
                titles = ", ".join(f"'{d['title']}' ({d['category']})" for d in blocking_dialogs)
                resolutions = " ".join(dict.fromkeys(d["resolution"] for d in blocking_dialogs))
                message = (f"The AdomBridge add-in is alive but its main thread is blocked by a "
                           f"modal dialog: {titles}. This is NOT an add-in crash — do not restart "
                           f"Fusion. {resolutions}")
            else:
                message = (f"The AdomBridge add-in is alive but its main thread is not "
                           f"responding (status: {main_thread}, pending: {pending}). "
                           f"Fusion 360 likely has a modal dialog open (Document Recovery, "
                           f"error, or update prompt) that is blocking execution. "
                           f"READ the screenshots to identify the dialog, then dismiss "
                           f"with fusion_send_key {{\"key\": \"escape\"}} or "
                           f"fusion_send_key {{\"key\": \"tab\"}} + {{\"key\": \"enter\"}}.")
            return {
                "success": False,
                "error": "addin_main_thread_blocked",
                "message": message,
                "blockingDialogs": blocking_dialogs,
                "data": {
                    "command": command,
                    "health": health,
                    "blockingDialogs": blocking_dialogs,
                    "postOpenScreenshot": timeout_screenshots,
                },
            }
        # Health says responsive but command still timed out — unusual
        return {
            "success": False,
            "error": "addin_command_timeout",
            "message": f"Command '{command}' timed out but the add-in reports main thread "
                       f"is {main_thread}. The command may be long-running or stuck.",
            "data": {
                "command": command,
                "health": health,
                "postOpenScreenshot": timeout_screenshots,
            },
        }

    # THREE faults live here now, not two, and BUSY is the one that looks most like death
    # (page issue #40). /status answers without taking the main-thread lock, so it can tell
    # us a long command is still running - while the command a caller just sent is queued
    # behind it and times out. Reporting that as "add-in not responding" is what made a
    # 5s-spaced test matrix conclude the add-in dies after one command: a single surface
    # measured 546s here, so nine minutes of perfectly healthy WORK reads as death.
    try:
        _st_busy = _check_addin_status(timeout=3.0)
    except Exception:
        _st_busy = None
    if _st_busy and _st_busy.get("busy"):
        return {
            "success": False,
            "error": "The add-in is BUSY running a long command, not dead - your command is "
                     "queued behind it.",
            "errorCode": "fusion_addin_busy",
            "busyCommand": _st_busy.get("busyCommand"),
            "elapsedSeconds": _st_busy.get("elapsedSeconds"),
            "hostResponding": True,
            "_hint": ("Do NOT restart Fusion and do not treat this as a crash. Fusion runs one "
                      "command at a time on its main thread, and some surfaces genuinely take "
                      "minutes (a staged first-open has measured 546s here). WAIT and re-poll - "
                      "fusion_state answers while the add-in is busy and reports mainThreadBusy "
                      "with the running command. Only treat it as broken if elapsedSeconds keeps "
                      "climbing past what fusion_state.timings says this machine has ever taken."),
        }
    # Health probe failed. TWO DIFFERENT FAULTS LIVE HERE and they need opposite
    # responses (page issue #38): the add-in being down is recoverable over IPC, a HUNG
    # HOST PROCESS is not. Reporting the second as the first sent a reader hunting a
    # blocking dialog that did not exist.
    _hung = _confirmed_hung()
    if _hung.get("phase") in ("suspect", "busy-not-hung"):
        return {
            "success": False,
            "error": "Fusion's main thread is not answering right now - most likely WORKING "
                     "on something heavy, not hung (the probes it starves are the very ones "
                     "this gate reads).",
            "errorCode": "fusion_addin_busy",
            "hostResponding": _hung.get("phase") == "busy-not-hung",
            "hungHeldSeconds": _hung.get("heldSeconds"),
            "_hint": "WAIT and re-poll. A real hang is only declared after 180s of continuous "
                     "silence including a 20s probe, and then the bridge restarts Fusion itself.",
        }
    if _hung.get("hung"):
        # DOCTRINE: this is mechanically fixable and the user should never have to do it.
        # A hung process will not close politely, so go straight to the force path.
        # (_confirmed_hung: kill-grade evidence only - persistence + a refused 20s probe -
        # after this gate taskkilled three healthy, working Fusions in one day, 2026-08-21.)
        _healed = False
        try:
            dispatch_command("kill", {})
            _t3 = __import__("time"); _t3.sleep(6)
            dispatch_command("start", {})
            _healed = True
        except Exception:
            pass
        return {
            "success": False,
            "error": "Fusion 360 itself is NOT RESPONDING (the host process stopped pumping "
                     "its message queue), so the add-in cannot answer.",
            "errorCode": "fusion_host_hung",
            "hungSignals": _hung.get("signals"),
            "ghostWindow": bool("GhostWindow" in (_hung.get("signals") or [])),
            "restartAttempted": _healed,
            "_hint": ("This is NOT a blocking dialog and NOT an add-in crash - do not go "
                      "looking for a modal, there is not one. Nothing reaches Fusion over IPC "
                      "while it is hung; only a restart clears it. "
                      + ("The bridge already force-restarted it (kill + start) - poll "
                         "fusion_readiness until ready:true." if _healed else
                         "Restart it with fusion_kill then fusion_start.")),
        }
    return {
        "success": False,
        "error": "AdomBridge add-in not responding (it may have crashed).",
        "errorCode": "fusion_addin_not_responding",
        "hostResponding": True,
        "_hint": "Fix it YOURSELF - never ask the user: restart Fusion via fusion_stop + "
                 "fusion_start (the bridge installs the add-in to ALL Fusion add-in dirs incl. "
                 "%APPDATA%\\Autodesk\\FusionAddins, and runOnStartup reloads it). The host "
                 "process IS responding - this is the add-in only, so no dialog is implied.",
    }



def _fusion_text_path(path: str) -> str:
    """A path that survives Fusion's executeTextCommand, which is SPACE-DELIMITED.

    Bug (first live run of fusion_install_library_bundle, 2026-08-29): the bridge built
    "Document.newDesignFromLocal C:/Users/john/Documents/Adom Libraries/x/part.lbr" raw, so
    Fusion opened ".../Adom" and said "3 : Filename not found". The add-in's #196 fix (quote,
    then 8.3 short path) never ran, because the bridge-side orchestration bypasses it with
    its own execute_text_command. Measured inside Fusion's Python on AdomLapper: the QUOTED
    form fails with error 3, the 8.3 short path with forward slashes opens first try. Any
    user whose profile folder has a space ("C:/Users/John Doe") hits this on EVERY import,
    so every bridge-side text command that embeds a path goes through here.
    """
    p = (path or "").replace("\\", "/")
    if " " not in p or os.name != "nt":
        return p
    try:
        import ctypes
        from ctypes import wintypes
        _gsp = ctypes.windll.kernel32.GetShortPathNameW
        _gsp.argtypes = [wintypes.LPCWSTR, wintypes.LPWSTR, wintypes.DWORD]
        _gsp.restype = wintypes.DWORD
        buf = ctypes.create_unicode_buffer(1024)
        if _gsp(p, buf, 1024) and buf.value and " " not in buf.value:
            return buf.value.replace("\\", "/")
    except Exception:
        pass
    return p

def _orchestrate_open_lbr(args: dict) -> dict:
    """Open an EAGLE .lbr library file in Fusion 360 Electronics.

    Uses Document.newDesignFromLocal (via the add-in's execute_text_command)
    to open the .lbr file, which auto-switches to the Electronics Library
    editor. Then optionally navigates to a symbol and verifies via export.

    This is orchestrated at bridge level to avoid add-in module caching
    issues and to allow multi-step operations with waits.
    """
    import tempfile
    import time

    # ACCEPT EVERY NAME A CALLER COULD REASONABLY USE (page issue #61, 2026-08-26).
    # This read ONLY "filePath" while docs/electronics-libraries.md and the sibling verb
    # fusion_build_library_3d both use "lbrPath". A caller passed lbrPath, we silently saw
    # an empty string, and then blamed THEIR file ("file-readable: [WinError 3] ... path
    # specified: <empty>"), sending them to re-stage a perfectly good 7682-byte library.
    # A caller should never have to guess our parameter spelling, and an empty path must
    # never be reported as a bad file.
    file_path = (args.get("filePath") or args.get("lbrPath") or args.get("libraryPath")
                 or args.get("path") or "")
    if not str(file_path).strip():
        return {"success": False, "errorCode": "no_path_argument",
                "error": "No library path was given, so nothing was opened. This verb "
                         "accepts filePath, lbrPath, libraryPath or path - the file itself "
                         "was never examined, so do NOT re-stage it.",
                "acceptedArgs": ["filePath", "lbrPath", "libraryPath", "path"],
                "receivedArgs": sorted(args.keys()),
                "_hint": "Re-call with a Windows-local path, e.g. fusion_open_lbr "
                         "{\"lbrPath\": \"C:/tmp/rayparts/tja1050.lbr\"}."}
    # ⛔ PARSE BEFORE FUSION EVER SEES IT (John caught the dialog live, 2026-08-21): a
    # 77-byte garbage .lbr staged by a caller sailed straight into Fusion, which raised
    # its blocking "has errors and cannot be opened" Error modal ON EVERY RETRY and left
    # an empty Untitled design behind each time. The file's validity is knowable offline
    # in milliseconds - an unparseable library must NEVER reach Fusion's parser.
    # PRE-ANALYZE before Fusion ever sees the file (John, 2026-08-21; the 77-byte
    # ads1220irvar corpse raised Fusion's blocking Error modal on every retry). Only the
    # HARD opens are gated here - a parseable library with soft findings (empty
    # devicesets etc.) still opens, because open_lbr's contract is opening, but a file
    # Fusion's parser will refuse never leaves this function. NOTE the _ospre alias:
    # this function has a LOCAL `import os` further down that makes `os` local to the
    # whole scope - referencing it bare here was the 1.9.128 UnboundLocalError.
    import os as _ospre
    _pfr = _preflight_lbr(file_path)
    _hard = {c["check"]: c for c in _pfr["checks"]}
    if not all(_hard.get(k, {}).get("ok", True)
               for k in ("file-readable", "size-sane", "xml-parses", "root-is-eagle")):
        _pff = file_path.replace("\\", "/")
        try:
            _sz = _ospre.path.getsize(_pff)
            _head = open(_pff, "rb").read(48)
        except Exception:
            _sz, _head = None, b""
        return {"success": False, "errorCode": "lbr_invalid",
                "preflight": _pfr,
                "error": "%s will not open in Fusion: %s - refusing to hand it over (the "
                         "only possible outcome is a blocking Error modal plus an orphaned "
                         "empty design)." % (file_path, "; ".join(_pfr["errors"])[:240]),
                "fileSize": _sz,
                "fileHead": repr(_head[:32]),
                "_hint": ("The file on THIS box is bad. Re-stage it from the source and "
                          "compare hashes - a transfer that truncates or re-encodes is the "
                          "usual culprit. fusion_preflight_lbr re-runs these checks in "
                          "isolation. Nothing was opened and no dialog was raised.")}
    # PRE-CLOSE a stale dirty copy of this same library (deterministic-close rule): opening
    # over an unsaved same-name document is how duplicate docs piled up (15 at peak) and how
    # later closes rained Save-As prompts. If a dirty copy is open, resolve it NOW - it is
    # bridge scratch; the .lbr on disk is the source of truth.
    try:
        _stem0 = file_path.replace("\\", "/").rsplit("/", 1)[-1].rsplit(".", 1)[0]
        _di0 = _proxy_to_addin("document_info", {}, timeout=6)
        for _d0 in ((_di0.get("data") or {}).get("openDocuments") or []):
            _n0 = str(_d0.get("name") or "").rstrip("*").strip()
            # ANY same-name doc, dirty or CLEAN (matrix round 2026-08-21, rp2350b/type-c):
            # a clean stale doc from an earlier open of an OLDER file made the new open
            # short-circuit ("already open"), the active-doc check then verified the
            # STALE document, and every EDIT prompted Create-new for parts that plainly
            # exist in the file on disk. The .lbr ON DISK is the source of truth - a
            # same-name Fusion doc, whatever its state, is a stale rendering surface.
            if _n0.lower() == _stem0.lower():
                _close_bridge_doc_verified(_n0)
    except Exception:
        pass
    symbol_name = args.get("symbolName", "")
    verify = args.get("verify", False)

    if not file_path:
        return {"success": False, "error": "No filePath specified"}

    file_path = file_path.replace("\\", "/")
    try:
        _note_bridge_doc(file_path)  # save-prompt sentinel may auto-discard this doc's edits
    except Exception:
        pass
    results = []

    # Step 1: Open the .lbr via Document.newDesignFromLocal.
    # ⚠️ TIMEOUT + VERIFY-BY-STATE (fixed 2026-07-07, caught live on a GPU-less Azure
    # VM): the open can take 60s+ on slow/software-rendered machines, so a fixed 30s
    # read timeout expired mid-open and the canned proxy error claimed the ADD-IN was
    # "not running" while the document was actually opening fine (fusion_build_library_3d
    # then aborted all parts on a lie). Now: a generous timeout, AND on ANY failure we
    # poll get_app_state for the expected document name - the doc actually being open
    # outranks whatever the synchronous return claimed.
    open_result = _proxy_to_addin("execute_text_command", {
        "command": f"Document.newDesignFromLocal {_fusion_text_path(file_path)}",
    }, timeout=120)

    if not open_result.get("success"):
        # Verify by STATE before failing: did the doc open anyway?
        expected = file_path.replace("\\", "/").rsplit("/", 1)[-1]
        expected = expected.rsplit(".", 1)[0].lower()
        opened_anyway = False
        for _ in range(20):  # up to ~60s of settling
            time.sleep(3)
            try:
                st = _proxy_to_addin("get_app_state", {}, timeout=8)
                active = str((st.get("data") or {}).get("activeDocument", "")).lower()
                if expected and expected in active:
                    opened_anyway = True
                    break
            except Exception:
                pass
        if not opened_anyway:
            return {
                "success": False,
                "error": f"Document.newDesignFromLocal failed: {open_result.get('error', 'unknown')}",
                "_hint": ("The open did not complete AND the document never became active. On slow/"
                          "software-rendered machines opens can take 60s+; this call already waited + "
                          "verified by state. Check fusion_check_dialogs for a blocking modal, then retry."),
                "data": {"filePath": file_path},
            }
        results.append("Document.newDesignFromLocal: ok (verified by app state after slow open)")
    else:
        results.append("Document.newDesignFromLocal: ok")

    # Step 2: Navigate to symbol if requested
    if symbol_name:
        time.sleep(3)  # Let Fusion finish opening and switching workspace

        sym_ref = symbol_name if symbol_name.endswith(".sym") else f"{symbol_name}.sym"
        edit_result = _proxy_to_addin("electron_run", {
            "command": f"EDIT {sym_ref}",
        }, timeout=10)
        results.append(f"EDIT {sym_ref}: success={edit_result.get('success')}")

        # Zoom to fit
        _proxy_to_addin("electron_run", {"command": "WINDOW FIT"}, timeout=5)
        results.append("WINDOW FIT: ok")

    # Step 3: Verify via EXPORT SCRIPT
    verification = None
    if verify:
        import uuid
        time.sleep(2)  # Let Fusion settle before export
        # Use a unique path to avoid "overwrite?" dialogs blocking the UI thread
        export_path = str(Path(tempfile.gettempdir()) / f"_adom_verify_{uuid.uuid4().hex[:8]}.scr")
        export_result = _proxy_to_addin("export_lbr", {"outputPath": export_path}, timeout=30)
        if export_result.get("success"):
            preview = export_result.get("data", {}).get("preview", "")
            has_symbol = False
            if symbol_name:
                has_symbol = (
                    f"'{symbol_name.upper()}.sym'" in preview
                    or f"'{symbol_name}.sym'" in preview
                )
            verification = {
                "exported": True,
                "fileSize": export_result.get("data", {}).get("fileSize", 0),
                "hasSymbol": has_symbol,
                "preview": preview[:500],
            }
        else:
            verification = {"exported": False, "error": export_result.get("error", "unknown")}
        results.append(f"Verification: exported={verification.get('exported', False)}")

    import os
    # Remember it so a later capture/EDIT can pre-check names offline instead of letting
    # Fusion raise "Create new footprint 'X'?" and dismissing it after the fact.
    _remember_open_library(file_path)
    # BUBBLE THE PART-NUMBER GAP UP WITH THE IMPORT (John, 2026-08-28). The import is NOT
    # blocked by missing part numbers - a library without them is still a real library and
    # the user asked for it - but the gap has to reach the top (the wiki's quick actions
    # UI, or web control) so they see "2 of my 5 components had no Mouser part number" and
    # can go fix it where the data lives. We are the only thing that has read the file.
    try:
        _pm = _part_metadata_report(file_path)
    except Exception:
        _pm = None
    response = {
        "success": True,
        "output": f"Opened library: {os.path.basename(file_path)}",
        "partMetadata": _pm,
        "data": {"results": results, "filePath": file_path},
        "_hint": (
            "Library opened in the Electronics Library editor (Content Manager). "
            "ALWAYS VERIFY VIA SCREENSHOT: a .lbr can FAIL to open ('<file>.lbr has errors and cannot be "
            "opened') while this call still returns success - that error is an OWNED POPUP. Grab "
            "desktop_screenshot_window on the Fusion main hwnd and CHECK ownedPopupCount + read the "
            "_screenshots[] array (ab v1.8.177+ captures owned dialogs invisible to a plain capture); "
            "ownedPopupCount>0 means an error/confirm dialog is up. "
            "NOTE: an adom-lbr .lbr is 2D ONLY - symbol + footprint + a PLACEHOLDER 3D package. "
            "To attach the real 3D chip, use fusion_attach_3d_package (it runs the Package3D generator + "
            "FINISH, which binds the 3D onto the deviceset). See the 'fusion-libraries' skill / LIBRARY_FINDINGS.md."
        ),
    }
    if symbol_name:
        response["data"]["symbolName"] = symbol_name
    if verification:
        response["data"]["verification"] = verification

    # Auto-screenshot after opening — catches blocking dialogs, same as other open commands
    return _post_open_screenshot(response)



def _fusion_front_doc_title():
    """Lower-cased title of Fusion's main window (the document IN FRONT), or None when it
    cannot be read. Fusion titles the main window '<document> - Autodesk Fusion'."""
    try:
        best = None
        for w in family_windows() or []:
            t = str(w.get("title") or "")
            if "autodesk fusion" not in t.lower():
                continue
            area = int(w.get("width") or 0) * int(w.get("height") or 0)
            if best is None or area > best[0]:
                best = (area, t)
        if not best:
            return None
        return best[1].lower().replace(" - autodesk fusion", "").strip()
    except Exception:
        return None


def _ensure_library_active(lbr_path: str, timeout_s: int = 60) -> dict:
    """Make the .lbr the ACTIVE document - verified, not assumed (caught live 2026-08-21).

    open_lbr returning success does NOT mean the library is in front: the open is async, and
    when the library is ALREADY open the call short-circuits leaving whatever document was
    active. Create3DPackage then refuses with "Please run the command in library document" -
    exactly what the first live bind of adom-lbr output hit, with Untitled still active.
    Polls until activeDocument matches, activating by name if open-but-not-front.
    """
    import time as _t
    stem = lbr_path.replace("\\", "/").rsplit("/", 1)[-1].rsplit(".", 1)[0].lower()
    deadline = _t.time() + timeout_s
    last_ws = None
    _reopened = {"done": False}
    _tries = {"n": 0}
    while _t.time() < deadline:
        try:
            st = _proxy_to_addin("get_app_state", {}, timeout=6)
            _d = st.get("data") or {}
            active = str(_d.get("activeDocument") or "")
            # THE WORKSPACE, not just the document (#49 parts 6/7, John 2026-08-23): the 3D
            # bind leaves the .lbr the ACTIVE DOCUMENT but the WORKSPACE on "Design" (the 3D
            # view), and EDIT then fails "Not in Electronics editor (current: Design)". So the
            # library is only truly ready when the doc matches AND the workspace is an
            # ELECTRONICS EDITOR (Library / Schematic / Board), never Design / the container.
            ws = str(_d.get("activeWorkspace") or "")
            last_ws = ws
            _wl = ws.lower()
            ws_ok = ("librar" in _wl or "schematic" in _wl or "board" in _wl
                     or "pcb" in _wl) and "electronics design" not in _wl
            if st.get("success") and stem in active.lower() and ws_ok:
                # THE WINDOW IN FRONT, not the API's activeDocument (2026-08-30): with two
                # libraries open, activeDocument said ch340g-symbols while lan7800's window
                # was in front, and EDIT (which acts on the window in front) raised "Create
                # new device" in the WRONG library. Fusion titles its main window after the
                # document in front, so the title is the truth; when it disagrees, close our
                # own copy and reopen it so it lands in front (it is a bridge document).
                _front = _fusion_front_doc_title()
                if _front is None or stem in _front:
                    return {"ok": True, "activeDocument": active, "activeWorkspace": ws,
                            "frontWindow": _front}
                if not _reopened.get("done"):
                    _reopened["done"] = True
                    try:
                        _close_bridge_doc_verified(stem)
                    except Exception:
                        pass
                    try:
                        _orchestrate_open_lbr({"lbrPath": lbr_path})
                    except Exception:
                        pass
                    _t.sleep(3)
                    continue
            # Re-activate EVERY iteration until the workspace actually switches - a single
            # activate can lose the race with the 3D step's workspace change (was tried once).
            # A library that reopened into the DESIGN workspace never flips on activation
            # alone (seen live on ConfRoomROG, 2026-08-31: ten activations, still "Design");
            # re-issuing the open (newDesignFromLocal) is what lands it in the Electronics
            # Library workspace, so after a few stuck rounds do that once.
            _tries["n"] += 1
            if st.get("success") and stem in active.lower() and not ws_ok and _tries["n"] >= 4 \
                    and not _tries.get("reopened"):
                _tries["reopened"] = True
                try:
                    _orchestrate_open_lbr({"lbrPath": lbr_path})
                except Exception:
                    pass
                _t.sleep(4)
                continue
            if st.get("success"):
                _proxy_to_addin("activate_document", {"name": stem}, timeout=10)
        except Exception:
            pass
        _t.sleep(1.5)
    return {"ok": False, "activeDocument": None, "activeWorkspace": last_ws}


def _create_3d_package(lbr_path: str, timeout: int = 60) -> dict:
    """The ONLY way to run Electron.Create3DPackage: verify the library is active first.

    Five call sites each did open-then-hope with their own sleep; this replaces all of them
    so the precondition cannot be forgotten at the next one.
    """
    act = _ensure_library_active(lbr_path)
    if not act.get("ok"):
        return {"success": False, "errorCode": "library_not_active",
                "error": "The library opened but never became the ACTIVE document, so "
                         "Create3DPackage would refuse ('Please run the command in library "
                         "document').",
                "_hint": "Another document may be mid-operation on Fusion's main thread. "
                         "Retry: the bridge polls 60s and activates by name before giving up."}
    return _proxy_to_addin("execute_text_command",
                           {"command": "Electron.Create3DPackage %s" % lbr_path},
                           timeout=timeout)


def _orchestrate_attach_3d_package(args: dict) -> dict:
    """Attach a real 3D model to a library package, end to end.

    Opens the .lbr (library active), runs Electron.Create3DPackage to enter the
    Package3DEnvironment showing the footprint, imports the STEP model and
    auto-orients it flat on the footprint, then executes Package3DStop (FINISH).
    Fusion then shows a modal Save dialog (an OWNED popup) — that single click is
    the only desktop-side step; this returns the exact instruction for it.

    args: {filePath: Windows path to the .lbr, modelPath: Windows path to the
           STEP, packageName: optional str}
    """
    import time
    file_path = (args.get("filePath") or "").replace("\\", "/")
    model_path = (args.get("modelPath") or "").replace("\\", "/")
    package = args.get("packageName", "")
    orient_flag = args.get("orient", True)  # 2026-07-07: expose orient (was hard-on)
    if not file_path or not model_path:
        return {"success": False,
                "error": "filePath (.lbr) and modelPath (.step) are required (Windows paths, e.g. C:/...).",
                "_hint": "Stage both onto Windows first (no container->Windows push verb). See the fusion-libraries skill."}
    steps = []
    # The attach flow raises documents AND modal dialogs over several minutes - arm its own
    # long envelope so the Save dialog at the end is still covered (2026-08-15: the dispatch
    # envelope had expired by dialog time and the dialog kept the user's foreground).
    if not args.get("foreground"):
        begin_background_envelope(600.0)
    open_res = _orchestrate_open_lbr({"filePath": file_path})
    if not open_res.get("success"):
        return {"success": False, "error": "open_lbr failed: " + str(open_res.get("error")), "data": {"steps": steps}}
    steps.append("open_lbr: ok (library active)")
    time.sleep(1)
    cp = _create_3d_package(file_path, timeout=40)
    if not cp.get("success"):
        return {"success": False, "error": "Create3DPackage failed: " + str(cp.get("error")),
                "_hint": "The library document must be ACTIVE, and the .lbr must be adom-lbr-generated "
                         "(a raw vendor EAGLE .lbr fails Fusion's XML parser).", "data": {"steps": steps}}
    steps.append("Create3DPackage: ok (Package3DEnvironment)")
    time.sleep(2)
    import_script = (
        "import adsk.core, adsk.fusion, math\n"
        "res={}\n"
        "d=adsk.fusion.Design.cast(app.activeProduct); root=d.rootComponent\n"
        "im=app.importManager\n"
        f"im.importToTarget(im.createSTEPImportOptions('{model_path}'), root)\n"
        "oc=root.occurrences.item(root.occurrences.count-1); bb=oc.boundingBox\n"
        "dx=bb.maxPoint.x-bb.minPoint.x; dy=bb.maxPoint.y-bb.minPoint.y; dz=bb.maxPoint.z-bb.minPoint.z\n"
        # Tall-part guard (2026-07-07): keep a part vertical if its largest dim is already Z; only
        # flatten a clearly-thin part whose thin axis isn't Z. Was: always tip smallest dim to Z,
        # which laid tall through-hole pins on their side. Honors the orient flag (default True).
        + ("axis=None\n" if not orient_flag else
           "thin=min(dx,dy,dz); big=max(dx,dy,dz)\n"
           "tall_z=(dz>=dx and dz>=dy)\n"
           "axis=None\n"
           "if (thin < 0.5*big) and not tall_z:\n"
           "    axis=(1,0,0) if (dy<=dx and dy<=dz) else ((0,1,0) if (dx<=dy and dx<=dz) else None)\n") +
        "if axis:\n"
        "    mat=oc.transform2.copy(); rot=adsk.core.Matrix3D.create()\n"
        "    rot.setToRotation(math.pi/2, adsk.core.Vector3D.create(*axis), adsk.core.Point3D.create(0,0,0))\n"
        "    mat.transformBy(rot); oc.transform2=mat\n"
        "    d.snapshots.add() if d.snapshots.hasPendingSnapshot else None\n"
        "res['dims_mm']=[round(dx*10,2),round(dy*10,2),round(dz*10,2)]\n"
        # OPTIONAL LAYERS, OFF BY DEFAULT (John, 2026-08-30): a STEP may carry named solids
        # beside the chip (Adom.HeatsinkVias, Adom.SolderBalls, Adom.LaserEtch). Fusion keeps
        # them as bodies with their own eye toggles; STEP itself has no hidden flag, so the
        # bridge switches every 'Adom.*' body except Adom.Chip off right after the import.
        # The user turns a layer on in the Browser when they want it.
        # A STEP layer arrives as an OCCURRENCE named after the solid group (Adom.HeatsinkVias:1)
        # whose bodies are plain Body1..N (seen on the LAN7800 board, 2026-08-30: hiding by
        # body name hid nothing and 16 via rings showed on the 3D PCB). Match the occurrence
        # AND the body names, and switch off both the occurrence and each body.
        "names=[]; hidden=[]\n"
        # ALL LAYERS ON BY DEFAULT (John, 2026-08-30): with package VARIANTS, a STEP bound to a
        # package is exactly what that package should show (the -HV model IS the vias), so
        # nothing is hidden unless the caller names a group in layersOff.
        "def _layer(n):\n"
        "    n=(n or '').split(':')[0]\n"
        "    return n.startswith('Adom.') and n in LAYERS_OFF\n"
        "def _walk(comp, group, hide):\n"
        "    for i in range(comp.bRepBodies.count):\n"
        "        b=comp.bRepBodies.item(i); names.append((group or '')+'/'+b.name)\n"
        "        if hide or _layer(b.name):\n"
        "            try:\n"
        "                b.isLightBulbOn=False; hidden.append((group or '')+'/'+b.name)\n"
        "            except Exception: pass\n"
        "    for j in range(comp.occurrences.count):\n"
        "        o=comp.occurrences.item(j); on=(o.component.name or o.name).split(':')[0]\n"
        "        g=group or (on if on.startswith('Adom.') else '')\n"
        "        h=hide or _layer(on)\n"
        "        if h and not hide:\n"
        "            try: o.isLightBulbOn=False\n"
        "            except Exception: pass\n"
        "        _walk(o.component, g, h)\n"
        f"LAYERS_OFF=set({sorted(set(str(x) for x in (args.get('layersOff') or [])))!r})\n"
        "try:\n"
        "    _walk(oc.component, '', False)\n"
        "except Exception as _e:\n"
        "    res['layersError']=str(_e)[:120]\n"
        "res['bodies']=names; res['layersHidden']=hidden\n"
        "result=res\n"
    )
    imp = _proxy_to_addin("run_modeling_script", {"script": import_script}, timeout=120)
    if not imp.get("success"):
        return {"success": False, "error": "import/orient failed: " + str(imp.get("error")), "data": {"steps": steps}}
    dims = (imp.get("data", {}) or {}).get("result", {})
    steps.append(f"import+orient: ok ({dims.get('dims_mm') if isinstance(dims, dict) else dims})")
    if isinstance(dims, dict) and dims.get("bodies") is not None:
        steps.append("bodies: %s; layers hidden: %s" % (dims.get("bodies"), dims.get("layersHidden")))
    time.sleep(1)
    _proxy_to_addin("run_modeling_script",
                    {"script": "import adsk.core\ncd=ui.commandDefinitions.itemById('Package3DStop')\nresult={'finished': bool(cd) and cd.execute()}\n"},
                    timeout=30)
    steps.append("FINISH (Package3DStop): executed")
    return {
        "success": True,
        "output": f"3D model placed + oriented on the footprint and FINISH executed for '{package or file_path}'. A Save dialog is now up.",
        "data": {"steps": steps, "package": package},
        "savePending": True,
        "_hint": (
            "FINAL STEP (desktop-side, one click): a Fusion 'Save' dialog is now up to save the 3D package. "
            "Click it: desktop_find_window {titleContains:'Save'} -> desktop_ui_click "
            "{hwnd, automationId:'QTApplication.QTFrameWindow.standardActions.SaveButton'}. "
            "THEN VERIFY with desktop_screenshot_window on the Fusion hwnd: check ownedPopupCount (errors are owned "
            "popups), and the deviceset's Package column should flip Placeholder->part-name + the 3D preview becomes "
            "the real chip (the preview LAGS a beat - re-grab). Full flow: fusion-libraries skill / LIBRARY_FINDINGS.md sect 11."
        ),
    }


# ── Fusion cloud FOLDER HYGIENE (never write loose files to a shared project ROOT) ───────────
# Hard lesson (2026-06-29): defaulting uploads to a project's ROOT folder dumped 100+ loose f3d
# files into the shared Adom team root and other employees complained. RULE: the bridge NEVER
# writes a file to a project root. It writes into an AI-OWNED "Adom AI Workspace" folder, with a
# per-task SUBfolder, keeping the cloud tidy. See the fusion-cloud-hygiene skill.
_DEFAULT_UPLOAD_PROJECT = "a.YnVzaW5lc3M6YWRvbTMjMjAyMzExMjk3MDM5NjAzMzE"  # the Adom business project
# The shared team ROOT folder of that project - OFF LIMITS for loose files (only the workspace
# folder itself may live here). Known roots we must refuse as a write target.
_KNOWN_ROOT_FOLDERS = {"urn:adsk.wipprod:fs.folder:co.jyO4vxQXR6S9zFpTQWnZAg"}
_AI_WORKSPACE_NAME = "Adom AI Workspace"   # the AI-owned work area (BRAND: "Adom AI", not "Claude")
_ws_folder_cache = {}


def _safe_folder_name(name: str) -> str:
    import re as _re
    return _re.sub(r'[<>:"/\\|?*]', "", str(name or "")).strip()[:60] or "task"


def _find_child_folder(project_id: str, parent_id: str, name: str):
    """folderId of a subfolder named `name` directly under parent_id, or None."""
    try:
        r = aps.handle_browse({"projectId": project_id, "folderId": parent_id})
        items = (r.get("data") or {}).get("items", []) if isinstance(r, dict) else []
        for it in items:
            if it.get("type") == "folders" and (it.get("name") or "").strip() == name:
                return it.get("id")
    except Exception:
        pass
    return None


def _ensure_workspace_folder(project_id: str, task: str = None):
    """Return a folderId inside the AI-owned 'Adom AI Workspace' (NEVER a project root). Creates the
    workspace folder (one tidy folder under the project root) + an optional per-task subfolder if
    missing. Cached per (project, task). Returns None if it cannot be resolved (caller must NOT then
    fall back to root)."""
    task = _safe_folder_name(task) if task else None
    key = (project_id, task or "")
    if key in _ws_folder_cache:
        return _ws_folder_cache[key]
    root = next(iter(_KNOWN_ROOT_FOLDERS))  # parent for the single workspace folder
    ws = _find_child_folder(project_id, root, _AI_WORKSPACE_NAME)
    if not ws:
        cr = aps.handle_create_folder({"projectId": project_id, "parentFolderId": root, "name": _AI_WORKSPACE_NAME})
        ws = (cr.get("data") or {}).get("folderId") if isinstance(cr, dict) and cr.get("success") else None
    folder = ws
    if task and ws:
        sub = _find_child_folder(project_id, ws, task)
        if not sub:
            cr = aps.handle_create_folder({"projectId": project_id, "parentFolderId": ws, "name": task})
            sub = (cr.get("data") or {}).get("folderId") if isinstance(cr, dict) and cr.get("success") else None
        folder = sub or ws
    if folder:
        _ws_folder_cache[key] = folder
    return folder


def _discover_upload_target(args: dict) -> tuple:
    """(projectId, folderId) for f3d uploads - ALWAYS a non-root, AI-owned folder.

    Defaults to 'Adom AI Workspace/<task>' (auto-created). If the caller explicitly passes a
    folderId that is a known project ROOT, it is REFUSED (we steer to the workspace instead) - the
    bridge must never write loose files to a shared root. Returns (projectId, folderId|None);
    folderId is None only if the workspace folder could not be created (caller must error, NOT
    fall back to root)."""
    project_id = args.get("projectId") or _DEFAULT_UPLOAD_PROJECT
    fid = args.get("folderId")
    if fid and fid in _KNOWN_ROOT_FOLDERS:
        fid = None  # explicit root -> refuse, steer to workspace
    if not fid:
        fid = _ensure_workspace_folder(project_id, args.get("task"))
    return (project_id, fid)


def _capture_labeled(label) -> str | None:
    """Background-capture the main Fusion window to a labeled PNG on the box
    (C:/tmp/conduit-screenshots). Returns the saved path or None. Never fullscreen
    (hwnd-targeted PrintWindow, so it captures Fusion in the background per fusion-driving)."""
    if not label:
        return None
    try:
        info = get_fusion_window_info()
        hwnd = info.get("hwnd")
        if not hwnd:
            return None
        import re as _re
        safe = _re.sub(r'[<>:"/\\|?*]', "", str(label)).replace(" ", "_")[:40]
        r = screenshot_hwnd(hwnd, label=safe)
        return r.get("savedTo") if r.get("success") else None
    except Exception:
        return None


def _inject_package3d_bindings(lbr_text: str, bindings: list) -> str:
    """Inject EAGLE <packages3d> + per-device <package3dinstances> into an .lbr - MERGE-AWARE
    and IDEMPOTENT (safe to re-run with any subset of parts).

    bindings: [{package: <pkg name>, wip_urn: <urn>}]. The function reads any bindings ALREADY in
    the .lbr, merges the new ones on top (new wins on conflict), strips all prior <packages3d> +
    <package3dinstances>, then re-emits the full merged set: a library-level <package3d> per part
    (between </packages> and <symbols>) and a <package3dinstances> inside every <device> that uses
    that package (right after </connects>). So calling it again with just the parts that failed last
    time ACCUMULATES instead of wiping the parts that already succeeded - the fix for the
    'a re-run dropped the earlier bindings' trap. Returns the new .lbr text."""
    import re as _re
    # 1. read existing bindings already in the file; new bindings override
    merged = {}
    em = _re.search(r"<packages3d>(.*?)</packages3d>", lbr_text, _re.S)
    if em:
        for pm in _re.finditer(r'<package3d name="([^"]+)"[^>]*wip_urn="([^"]+)"', em.group(1)):
            merged[pm.group(1)] = pm.group(2)
    for b in bindings:
        merged[b["package"]] = b["wip_urn"]
    # 2. strip ALL prior package3d markup so re-injection is clean (idempotent)
    lbr_text = _re.sub(r"\s*<packages3d>.*?</packages3d>", "", lbr_text, flags=_re.S)
    lbr_text = _re.sub(r"\s*<package3dinstances>.*?</package3dinstances>", "", lbr_text, flags=_re.S)
    # 3. library-level <packages3d> block (all merged parts)
    blocks = []
    for pkg, urn in merged.items():
        blocks.append(
            f'<package3d name="{pkg}" urn="" wip_urn="{urn}" locally_modified="yes" type="model">'
            f'<description>{pkg}</description>'
            f'<packageinstances><packageinstance name="{pkg}"/></packageinstances>'
            f'</package3d>'
        )
    pkgs3d = "<packages3d>\n" + "\n".join(blocks) + "\n</packages3d>\n"
    lbr_text = lbr_text.replace("</packages>", "</packages>\n" + pkgs3d, 1)

    # 4. per-device <package3dinstances>
    def _dev_repl(m):
        dev = m.group(0)
        pm = _re.search(r'package="([^"]+)"', dev)
        if pm and pm.group(1) in merged and "</connects>" in dev:
            inst = (f'<package3dinstances><package3dinstance package3d_urn="{merged[pm.group(1)]}"/>'
                    f'</package3dinstances>')
            dev = dev.replace("</connects>", "</connects>\n" + inst, 1)
        return dev

    return _re.sub(r'<device\b[^>]*>.*?</device>', _dev_repl, lbr_text, flags=_re.S)


def _pad1_xy_mm(lbr_path: str):
    """(x, y) in mm of pad/smd named "1" in the FIRST package of the .lbr, or None."""
    try:
        t = open(lbr_path, "r", encoding="utf-8", errors="replace").read()
        m = re.search(r"<package name=\"[^\"]*\">(.*?)</package>", t, re.S)
        body = m.group(1) if m else t
        pm = re.search(r'<(?:smd|pad) name="1" x="([-0-9.]+)" y="([-0-9.]+)"', body)
        if not pm:
            return None
        return (float(pm.group(1)), float(pm.group(2)))
    except Exception:
        return None


def _step_story_from_file(model_path: str) -> str:
    """What a STEP carries beyond the chip, read from the FILE before import (John,
    2026-08-30: the palette must say solder balls / heatsink vias while that STEP is being
    bound, not after). STEP is text; the Adom solid groups appear as PRODUCT names."""
    try:
        groups = []
        with open(model_path.replace("/", os.sep), "r", encoding="latin-1", errors="ignore") as fh:
            head = fh.read(4 * 1024 * 1024)
        for g in ("Adom.HeatsinkVias", "Adom.SolderBalls", "Adom.LaserEtch", "Adom.Board", "Adom.Pads"):
            if g in head:
                groups.append(g)
        words = {"Adom.HeatsinkVias": "the heatsink via barrels under the exposed pad",
                 "Adom.SolderBalls": "solder-ball hemispheres on every pad",
                 "Adom.LaserEtch": "the laser-etched part number",
                 "Adom.Board": "board stock", "Adom.Pads": "rendered pads"}
        if not groups:
            return "chip body only (the baseline model)"
        return "unlike the chip-only model this STEP carries " + " and ".join(words[g] for g in groups) + ", kept visible"
    except Exception:
        return ""


def _step_layer_story(bodies) -> str:
    """What makes this STEP different from a chip-only model, for the palette (John,
    2026-08-30: "tell them in the palette the nuances of what makes that step unique")."""
    try:
        groups = {}
        for b in bodies or []:
            g = str(b).split("/")[0]
            if g.startswith("Adom.") and g != "Adom.Chip":
                groups[g] = groups.get(g, 0) + 1
        if not groups:
            return "chip body only (the baseline model)"
        words = {"Adom.HeatsinkVias": "heatsink via barrels under the exposed pad",
                 "Adom.SolderBalls": "solder-ball hemispheres sitting on the pads",
                 "Adom.LaserEtch": "the laser-etched part number",
                 "Adom.Board": "board stock", "Adom.Pads": "rendered pads"}
        parts = ["%d %s" % (n, words.get(g, g[5:])) for g, n in sorted(groups.items())]
        return "unlike the chip-only model, this one carries " + " and ".join(parts) + ", all left visible"
    except Exception:
        return ""


def _orchestrate_make_3d_package(args: dict) -> dict:
    """Create a RENDERING component 3D-PACKAGE urn (footprint + chip), fully programmatically - NO GUI
    dialogs. A proper component 3D model contains the FOOTPRINT (pads + courtyard) AND the chip,
    merged and aligned, so the 3D viewer can verify the chip's pads land on the footprint pads.

    So: open the library, run Electron.Create3DPackage to load the package's FOOTPRINT into a
    generator doc, import the STEP onto it, orient it flat, then saveAs an .f3d (footprint + chip) -
    which skips the FINISH Save dialog + the two unbeatable "Fusion360" CEF modals entirely -
    aps_upload the .f3d, and return the fs.file:vf wip_urn to hand-write into the library's
    <packages3d>.

    Two gotchas this avoids: (1) importing the STEP into an EMPTY design gives a chip with NO
    footprint (an incomplete package); (2) a raw STEP upload does not render at all ("Thumbnail
    download failed"). See the fusion-multipart-libraries skill.

    args: {lbrPath: Windows .lbr whose FIRST package is the footprint, modelPath: Windows .step,
           projectId, folderId (both from fusion_aps_browse), fileName?: str, orient?: bool}
    """
    import os as _os, time as _time
    lbr_path = (args.get("lbrPath") or args.get("filePath") or "").replace("\\", "/")
    model_path = (args.get("modelPath") or "").replace("\\", "/")
    project_id, folder_id = _discover_upload_target(args)  # an AI-owned non-root folder (never project root)
    if not lbr_path or not model_path:
        return {"success": False,
                "error": "lbrPath (.lbr with the footprint) and modelPath (.step) are required (Windows paths)."}
    # No APS workspace folder (typically: APS not signed in) is NOT fatal here. The primary
    # path below saves the f3d STRAIGHT to the cloud through the add-in (doc.saveAs into a
    # workspace DataFolder the script creates itself), so APS is only needed at the upload
    # FALLBACK - the gate lives there now. The old up-front gate failed every chain on an
    # APS-less box whose happy path never touched APS (adom/adom-bridge#93, 2026-08-21).
    aps_folder_note = None
    if not folder_id:
        aps_folder_note = ("APS has no 'Adom AI Workspace' folder (fusion_aps_signin not done?) - "
                           "relying on the add-in's direct cloud save; the APS upload fallback is unavailable.")
    cap_label = args.get("captureLabel")  # when set, capture BEFORE (footprint) + AFTER (chip placed)
    task = _safe_folder_name(args.get("task") or "AI 3D packages")  # saveAs subfolder (never root)
    orient = args.get("orient", True)
    base = _os.path.basename(model_path).rsplit(".", 1)[0]
    name = (args.get("fileName") or (base + "_3d")).replace("'", "").replace(".f3d", "")
    _note_bridge_doc(name)   # the generator/3d document is the bridge's, never the user's
    f3d_name = name + ".f3d"
    # 1. open the library so the footprint exists; 2. Create3DPackage -> generator WITH the footprint loaded
    op = _orchestrate_open_lbr({"filePath": lbr_path})
    if not op.get("success"):
        return {"success": False, "error": "open_lbr failed: " + str(op.get("error"))}
    _time.sleep(1)
    cp = _create_3d_package(lbr_path, timeout=40)
    if not cp.get("success"):
        return {"success": False, "error": "Create3DPackage failed: " + str(cp.get("error")),
                "_hint": "The .lbr must be adom-lbr-generated; its FIRST package's footprint is loaded into the generator."}
    _time.sleep(2)
    # FOOTPRINT IDENTITY (John, 2026-08-29): the generator loads the FIRST package of the OPEN
    # library and names its document after that package. Read the name back and refuse when it
    # is not the package this mint is for: a chip on the wrong footprint must fail here, not
    # get uploaded, bound and "proven".
    _expected_pkg = ""
    try:
        import xml.etree.ElementTree as _ETg
        _pk = _ETg.parse(lbr_path).getroot().find(".//library/packages/package")
        _expected_pkg = (_pk.get("name") if _pk is not None else "") or ""
    except Exception:
        _expected_pkg = ""
    _gen_doc = ""
    try:
        _di = _proxy_to_addin("document_info", {}, timeout=8)
        _ad = (_di.get("data") or {}).get("activeDocument")
        _gen_doc = str((_ad.get("name") if isinstance(_ad, dict) else _ad) or "").rstrip("*").strip()
    except Exception:
        _gen_doc = ""
    if _expected_pkg and _gen_doc and _gen_doc.lower() != _expected_pkg.lower() \
            and not _gen_doc.lower().startswith("untitled"):
        return {"success": False, "errorCode": "wrong_footprint_loaded",
                "error": ("The Package3D generator loaded footprint %r but this mint is for package %r; "
                          "the chip would have landed on the wrong footprint." % (_gen_doc, _expected_pkg)),
                "_hint": "Mint each part on a library whose FIRST package is that part's footprint "
                         "(fusion_install_library_bundle splits a merged library per part for this reason)."}
    # BEFORE shot: the footprint (pads + courtyard) loaded in the 3D viewer, no chip yet.
    before_shot = None
    if cap_label:
        try:
            _proxy_to_addin("run_modeling_script",
                            {"script": "app.activeViewport.fit()\nresult={}"}, timeout=15)
        except Exception:
            pass
        _time.sleep(0.4)
        before_shot = _capture_labeled(f"{cap_label}_before")
    # AUTO-ORIENT (fixed 2026-07-07): the old logic always rotated the SMALLEST bbox dim to Z,
    # which is right for a flat SMD chip lying down but TIPS A TALL THROUGH-HOLE PART (machine pin,
    # connector) onto its side - its long axis is already Z and must stay vertical. Now: keep the
    # part vertical if its largest dim is already Z (tall_z), and only flatten a clearly THIN part
    # whose thin axis isn't Z yet. A caller can still force orient:false to skip entirely.
    orient_block = (
        "thin=min(dx,dy,dz); big=max(dx,dy,dz)\n"
        "tall_z=(dz>=dx and dz>=dy)\n"
        "is_flat=(thin < 0.5*big)\n"
        "axis=None\n"
        "if is_flat and not tall_z:\n axis=(1,0,0) if (dy<=dx and dy<=dz) else ((0,1,0) if (dx<=dy and dx<=dz) else None)\n"
        "if axis:\n mat=oc.transform2.copy(); r=adsk.core.Matrix3D.create()\n"
        " r.setToRotation(math.pi/2, adsk.core.Vector3D.create(*axis), adsk.core.Point3D.create(0,0,0))\n"
        " mat.transformBy(r); oc.transform2=mat\n"
    ) if orient else ""
    # PIN 1 (John, 2026-08-30: "the NUMBER ONE PROBLEM ee's expect us to solve"): the LAN7800
    # footprint had pad 1 bottom-left while the chip body's pin-1 dimple sat top-left. Neither
    # file is trusted: pad 1's position is read from the .lbr, the chip's pin-1 mark is found on
    # the imported body (the small circle on the top face nearest a corner), and when the two
    # sit in different corners the chip is rotated about Z by the multiple of 90 degrees that
    # puts its mark over pad 1. The result records what was found and what was done, and the
    # palette says it, so a rotation is never silent.
    _pin1 = _pad1_xy_mm(lbr_path)
    pin1_block = (
        f"PIN1={(_pin1[0] / 10.0, _pin1[1] / 10.0) if _pin1 else None!r}\n"
        "res_pin1={'pad1_mm': " + (f"[{_pin1[0]},{_pin1[1]}]" if _pin1 else "None") + ", 'mark_mm': None, 'rotatedDeg': 0, 'status': 'unchecked'}\n"
        "def _mark(occ):\n"
        "    bb2=occ.boundingBox; zmax=bb2.maxPoint.z; best=None\n"
        "    def _w(comp, depth):\n"
        "        nonlocal best\n"
        "        if depth>6: return\n"
        "        for i in range(comp.bRepBodies.count):\n"
        "            b=comp.bRepBodies.item(i)\n"
        "            try: vol=b.volume\n"
        "            except Exception: vol=0\n"
        "            if vol*1000 < 5: continue\n"   # the chip body, not a via or a ball
        "            for e in b.edges:\n"
        "                try:\n"
        "                    g=e.geometry\n"
        "                    if g.objectType not in (adsk.core.Circle3D.classType(), adsk.core.Arc3D.classType()): continue\n"
        "                    c=g.center\n"
        "                    if abs(c.z-zmax)>0.03 or g.radius>0.1: continue\n"
        "                    score=abs(c.x)+abs(c.y)\n"
        "                    if best is None or score>best[0]: best=(score,c.x,c.y)\n"
        "                except Exception: pass\n"
        "        for j in range(comp.occurrences.count): _w(comp.occurrences.item(j).component, depth+1)\n"
        "    _w(occ.component, 0)\n"
        "    return None if best is None else (best[1],best[2])\n"
        "try:\n"
        "    mk=_mark(oc)\n"
        "    if mk: res_pin1['mark_mm']=[round(mk[0]*10,2), round(mk[1]*10,2)]\n"
        "    if mk and PIN1:\n"
        "        import math as _m\n"
        "        want=_m.atan2(PIN1[1],PIN1[0]); have=_m.atan2(mk[1],mk[0])\n"
        "        k=int(round(((want-have)/(_m.pi/2)))) % 4\n"
        "        if k:\n"
        "            mat=oc.transform2.copy(); r=adsk.core.Matrix3D.create()\n"
        "            r.setToRotation(k*_m.pi/2, adsk.core.Vector3D.create(0,0,1), adsk.core.Point3D.create(0,0,0))\n"
        "            mat.transformBy(r); oc.transform2=mat\n"
        "            res_pin1['rotatedDeg']=k*90\n"
        "        res_pin1['status']='rotated' if k else 'aligned'\n"
        "    elif PIN1: res_pin1['status']='no_mark_on_chip'\n"
        "    else: res_pin1['status']='no_pad1_in_lbr'\n"
        "except Exception as _e: res_pin1['status']='error: '+str(_e)[:80]\n"
    )
    # 3. import the chip ONTO the footprint in the generator doc, orient, saveAs as f3d (footprint+chip)
    script = (
        "import adsk.core, adsk.fusion, math\n"
        "app=adsk.core.Application.get(); doc=app.activeDocument\n"
        "d=adsk.fusion.Design.cast(app.activeProduct); root=d.rootComponent\n"
        "im=app.importManager\n"
        f"im.importToTarget(im.createSTEPImportOptions('{model_path}'), root)\n"
        "oc=root.occurrences.item(root.occurrences.count-1); bb=oc.boundingBox\n"
        "dx=bb.maxPoint.x-bb.minPoint.x; dy=bb.maxPoint.y-bb.minPoint.y; dz=bb.maxPoint.z-bb.minPoint.z\n"
        + orient_block + pin1_block +
        "try:\n app.activeViewport.fit()\nexcept: pass\n"
        # FOLDER HYGIENE: saveAs into an 'Adom AI Workspace'/<task> subfolder, NEVER the project root.
        # activeProject is None on a fresh box until the user touches the data panel, so fall
        # back to the default upload project by id, then by name 'Main' (mirrors the add-in's
        # _resolve_project; hub->project listing only, never a recursive file walk).
        # The folder listing can be stale right after a failed run created the folder: add()
        # then fails CB_NAE ("another object with the same name already exists") even though
        # the scan missed it (LAN7800 variant runs, 2026-08-30). Re-scan before giving up.
        "def _sub(p,nm):\n"
        " import time as _ts\n"
        " for _try in range(4):\n"
        "  for i in range(p.dataFolders.count):\n"
        "   if str(p.dataFolders.item(i).name).lower()==str(nm).lower(): return p.dataFolders.item(i)\n"
        "  try: return p.dataFolders.add(nm)\n"
        "  except Exception as _fe:\n"
        "   if 'CB_NAE' not in str(_fe) and 'same name' not in str(_fe): raise\n"
        "   adsk.doEvents(); _ts.sleep(2.0)\n"
        " raise RuntimeError('cloud folder %s exists but cannot be resolved' % nm)\n"
        "res={}\n"
        # Optional layer bodies (Adom.HeatsinkVias, Adom.SolderBalls, Adom.LaserEtch) arrive
        # visible; switch them off so the user opts in from Fusion's Browser (John, 2026-08-30).
        # A STEP layer arrives as an OCCURRENCE named after the solid group (Adom.HeatsinkVias:1)
        # whose bodies are plain Body1..N (seen on the LAN7800 board, 2026-08-30: hiding by
        # body name hid nothing and 16 via rings showed on the 3D PCB). Match the occurrence
        # AND the body names, and switch off both the occurrence and each body.
        "names=[]; hidden=[]\n"
        # ALL LAYERS ON BY DEFAULT (John, 2026-08-30): with package VARIANTS, a STEP bound to a
        # package is exactly what that package should show (the -HV model IS the vias), so
        # nothing is hidden unless the caller names a group in layersOff.
        "def _layer(n):\n"
        "    n=(n or '').split(':')[0]\n"
        "    return n.startswith('Adom.') and n in LAYERS_OFF\n"
        "def _walk(comp, group, hide):\n"
        "    for i in range(comp.bRepBodies.count):\n"
        "        b=comp.bRepBodies.item(i); names.append((group or '')+'/'+b.name)\n"
        "        if hide or _layer(b.name):\n"
        "            try:\n"
        "                b.isLightBulbOn=False; hidden.append((group or '')+'/'+b.name)\n"
        "            except Exception: pass\n"
        "    for j in range(comp.occurrences.count):\n"
        "        o=comp.occurrences.item(j); on=(o.component.name or o.name).split(':')[0]\n"
        "        g=group or (on if on.startswith('Adom.') else '')\n"
        "        h=hide or _layer(on)\n"
        "        if h and not hide:\n"
        "            try: o.isLightBulbOn=False\n"
        "            except Exception: pass\n"
        "        _walk(o.component, g, h)\n"
        f"LAYERS_OFF=set({sorted(set(str(x) for x in (args.get('layersOff') or [])))!r})\n"
        "try:\n"
        "    _walk(oc.component, '', False)\n"
        "except Exception as _e:\n"
        "    res['layersError']=str(_e)[:120]\n"
        "res['bodies']=names; res['layersHidden']=hidden\n"
        "res['pin1']=res_pin1\n"
        "proj=None\n"
        "try: proj=app.data.activeProject\n"
        "except Exception as e: res['projErr']=str(e)[:60]\n"
        # Per-item guards are load-bearing: on some installs individual DataProject
        # property reads raise InternalValidationError (id.size()), and one poisoned
        # project must not abort the search for a healthy one.
        "if not proj:\n"
        " try:\n"
        "  hubs=app.data.dataHubs\n"
        "  for hi in range(hubs.count):\n"
        "   ps=hubs.item(hi).dataProjects\n"
        "   for pj2 in range(ps.count):\n"
        "    p2=ps.item(pj2)\n"
        "    ok=False\n"
        f"    try: ok=(p2.id=='{_DEFAULT_UPLOAD_PROJECT}')\n"
        "    except Exception: ok=False\n"
        "    if not ok:\n"
        "     try: ok=(p2.name=='Main')\n"
        "     except Exception: continue\n"
        "    if ok: proj=p2; break\n"
        "   if proj: break\n"
        " except Exception as e: res['projErr']=str(e)[:60]\n"
        # FRESH-ACCOUNT FALLBACK (#49, jlauer12 2026-08-22): a brand-new Autodesk account has
        # neither the Adom default-upload project nor one named 'Main', so the targeted search
        # above leaves proj=None and `proj.rootFolder` used to leak a raw AttributeError
        # ('NoneType' has no attribute 'rootFolder'). Doctrine: don't just NAME the gap - fill
        # it. Take ANY healthy project (rootFolder probe, since fresh installs also throw
        # InternalValidationError id.size() on poisoned entries), else CREATE an 'Adom'
        # project in the first hub that allows it. Only if all of that fails do we surface
        # the typed no_cloud_project error below.
        "if not proj:\n"
        " try:\n"
        "  hubs=app.data.dataHubs\n"
        "  for hi in range(hubs.count):\n"
        "   ps=hubs.item(hi).dataProjects\n"
        "   for pj2 in range(ps.count):\n"
        "    try:\n"
        "     p2=ps.item(pj2); p2.rootFolder\n"
        "     proj=p2; break\n"
        "    except Exception: continue\n"
        "   if proj: break\n"
        "  if not proj:\n"
        "   for hi in range(hubs.count):\n"
        "    try:\n"
        "     proj=hubs.item(hi).dataProjects.add('Adom'); break\n"
        "    except Exception as e: res['createErr']=str(e)[:60]\n"
        " except Exception as e: res['projErr2']=str(e)[:60]\n"
        "if not proj: res['errorCode']='no_cloud_project'\n"
        "try:\n"
        " if proj is None: raise RuntimeError('no usable cloud project on this account')\n"
        " rf=proj.rootFolder\n"
        f" wsf=_sub(_sub(rf,'Adom AI Workspace'),'{task}')\n"
        # A same-named f3d already in the folder (a re-run of the same package) makes saveAs
        # fail with CB_NAE ("another object with the same name already exists"); seen on the
        # LAN7800 variant run 2026-08-30. Retry once under a time-stamped name.
        " try:\n"
        f"  doc.saveAs('{name}', wsf, '3d package (footprint+chip)', '')\n"
        " except Exception as _se:\n"
        "  if 'CB_NAE' in str(_se) or 'same name' in str(_se):\n"
        f"   import time as _t0; _nm='{name}-'+_t0.strftime('%m%d%H%M%S')\n"
        "   doc.saveAs(_nm, wsf, '3d package (footprint+chip)', ''); res['renamedTo']=_nm\n"
        "  else: raise\n"
        # A first-ever cloud save on a fresh install syncs ASYNCHRONOUSLY: right after
        # saveAs, dataFile.id can still be the local cache path; the lineage urn appears
        # once the upload lands. Wait for it (bounded well under the 120s script timeout).
        " import time as _tt\n"
        " fid=None\n"
        " for w in range(40):\n"
        "  try: fid=doc.dataFile.id if doc.dataFile else None\n"
        "  except Exception: fid=None\n"
        "  if fid and 'dm.lineage:' in str(fid): break\n"
        "  adsk.doEvents(); _tt.sleep(1.5)\n"
        " res['path']=fid\n"
        " res['dims']=[round(dx*10,2),round(dy*10,2),round(dz*10,2)]\n"
        "except Exception as e: res['err']=str(e)[:80]\n"
        "result=res\n"
    )
    r = _proxy_to_addin("run_modeling_script", {"script": script}, timeout=120)
    res = (r.get("data", {}) or {}).get("result", {}) if isinstance(r, dict) else {}
    f3d_path = res.get("path") if isinstance(res, dict) else None
    dims = res.get("dims") if isinstance(res, dict) else None
    pin1 = res.get("pin1") if isinstance(res, dict) else None
    bodies = res.get("bodies") if isinstance(res, dict) else None
    # AFTER shot: the chip placed flat on its footprint (pads landing on pads) in the 3D viewer.
    after_shot = _capture_labeled(f"{cap_label}_after") if cap_label else None
    # doc.saveAs into a DataFolder IS a cloud save: dataFile.id returns the LINEAGE urn
    # (urn:adsk.wipprod:dm.lineage:<id>), and the binding's wip form is fs.file:vf.<same id>.
    # The old check demanded a local .f3d and treated this success as failure (caught live
    # 2026-08-15 on IN-S42ATA), then would have double-uploaded. Recognize it.
    if isinstance(f3d_path, str) and "dm.lineage:" in f3d_path:
        lineage_id = f3d_path.rsplit("dm.lineage:", 1)[-1]
        wip = "urn:adsk.wipprod:fs.file:vf.%s?version=1" % lineage_id
        if "0603WAF3300T5E" in str(args.get("lbrPath") or ""):
            try:
                _r3d_cache_write(wip)  # test projects' R1 gets real 3D from now on, this box
            except Exception:
                pass
        return {"success": True, "wip_urn": wip, "lineageUrn": f3d_path, "dims": dims,
                "pin1": pin1, "bodies": bodies, "renamedTo": res.get("renamedTo"),
                "fileName": f3d_name, "savedTo": "cloud (Adom AI Workspace)",
                "before": before_shot, "after": after_shot,
                "_hint": "Saved DIRECTLY to the cloud workspace - no upload step needed. "
                         "Bind this wip_urn into the library's <packages3d>."}
    if isinstance(res, dict) and res.get("errorCode") == "no_cloud_project":
        # #49 (fresh Autodesk account): name the ACTUAL cause with a typed code instead of
        # leaking 'NoneType has no attribute rootFolder'. The script above already tried
        # every hub for ANY healthy project AND tried to create an 'Adom' project.
        _detail = {k: res.get(k) for k in ("projErr", "projErr2", "createErr") if res.get(k)}
        return {"success": False, "errorCode": "no_cloud_project",
                "error": "3D minting uploads an .f3d to Autodesk cloud storage, and this "
                         "account has no usable cloud project (typical of a brand-new "
                         "Autodesk account). The bridge searched every hub for any healthy "
                         "project and also tried to CREATE an 'Adom' project; all failed: "
                         + (str(_detail) if _detail else "(no per-step detail)"),
                "before": before_shot, "after": after_shot,
                "_hint": "This is account setup, not a broken mint. Open Fusion's Data Panel "
                         "once (a fresh account provisions its default project on first "
                         "touch), or create any project in the hub, then re-run. "
                         "fusion_list_cloud_projects shows what the account can see."}
    if not f3d_path or not str(f3d_path).endswith(".f3d"):
        # Carry the add-in's OWN error up: res={} with the script error dropped on the
        # floor cost a whole diagnosis round (2026-08-21 - the real story was Fusion being
        # killed mid-script, visible only in r['error']).
        return {"success": False,
                "error": "f3d saveAs did not produce a .f3d path: " + str(res)
                         + (" | addin: " + str(r.get("error"))[:200]
                            if isinstance(r, dict) and r.get("error") else ""),
                "before": before_shot, "after": after_shot,
                "_hint": "Confirm modelPath is a valid Windows .step path and Fusion is running."}
    if not folder_id:
        return {"success": False, "errorCode": "no_work_folder",
                "error": "In-app cloud save failed (" + str(res)[:120] + ") and APS has no "
                         "'Adom AI Workspace' folder to upload the local f3d to.",
                "before": before_shot, "after": after_shot,
                "_hint": "Two independent paths can mint the 3D package: the add-in's direct cloud "
                         "save (needs Fusion running + signed in; the res/projErr above says why it "
                         "failed) or the APS upload fallback (needs fusion_aps_signin). Fix either. "
                         "The bridge never writes to a shared project ROOT."}
    up = aps.handle_upload({"projectId": project_id, "folderId": folder_id,
                            "localPath": f3d_path, "fileName": f3d_name})
    up_d = up.get("data", up) if isinstance(up, dict) else {}
    item_urn = (up_d or {}).get("itemUrn", "") if isinstance(up_d, dict) else ""
    try:
        _proxy_to_addin("run_modeling_script", {"script": "app.activeDocument.close(False)\nresult={}"}, timeout=20)
    except Exception:
        pass
    if "dm.lineage:" not in item_urn:
        # 409 = the file already EXISTS in the cloud folder (a previous mint uploaded it).
        # The data we need is sitting right there, so reuse the existing ITEM's lineage urn
        # instead of failing (doctrine: never return an error the code could fix). The items
        # in the folder listing carry dm.lineage ids, which is exactly what the wip_urn is
        # built from below - caught live when re-running the CR0603 chain after a
        # successful morning bind.
        _http_status = (up_d or {}).get("httpStatus") if isinstance(up_d, dict) else None
        if _http_status == 409:
            try:
                import aps as _aps409
                status2, listing = _aps409._aps_get(
                    "/data/v1/projects/%s/folders/%s/contents" % (project_id, folder_id))
                if status2 == 200 and isinstance(listing, dict):
                    for it in (listing.get("data") or []):
                        nm = str((it.get("attributes") or {}).get("displayName") or "")
                        iid = str(it.get("id") or "")
                        if "dm.lineage:" in iid and nm.lower().startswith(str(f3d_name).lower().rsplit(".f3d", 1)[0]):
                            item_urn = iid
                            print("[Fusion Bridge] upload 409: reusing existing cloud f3d "
                                  "%r -> %s" % (nm, iid[-40:]))
                            break
            except Exception:
                pass
        if "dm.lineage:" not in item_urn:
            return {"success": False, "error": "f3d upload did not return a lineage urn: " + str(up_d)}
    lid = item_urn.split("dm.lineage:")[-1]
    wip_urn = f"urn:adsk.wipprod:fs.file:vf.{lid}?version=1"
    return {
        "success": True,
        "wip_urn": wip_urn,
        "dims_mm": dims,
        "f3d": f3d_path,
        "before": before_shot,
        "after": after_shot,
        "_hint": (
            "DONE - a PROPER component 3D package (FOOTPRINT + chip, aligned) created with NO GUI dialogs. "
            "NEXT: hand-write this wip_urn into the library's EAGLE XML - a <package3d name=\"PKG\" urn=\"\" "
            "wip_urn=\"" + wip_urn + "\" locally_modified=\"yes\" type=\"model\"> (with <packageinstances>"
            "<packageinstance name=\"PKG\"/></packageinstances>) inside <packages3d> (between </packages> and "
            "<symbols>), PLUS <package3dinstances><package3dinstance package3d_urn=\"" + wip_urn + "\"/>"
            "</package3dinstances> inside the device after </connects>. Then fusion_open_lbr the merged .lbr. "
            "EXPECT: fusion_check_dialogs == 0 (no broken-ref), the device 3D preview shows footprint + chip "
            "(NOT 'Thumbnail download failed'), and opening the f3d shows the chip's pads landing on the "
            "footprint pads. "
            "SPEEDUP: for a many-part library, call this verb once per part, collect the urns, hand-merge ALL "
            "bindings in ONE pass, then fusion_open_lbr ONCE (don't open/save per part). "
            "PITFALLS: needs Fusion running (after a bridge_install respawn it can transiently report "
            "'not running' - fusion_start, then retry); a RAW STEP upload binds but renders NOTHING "
            "('Thumbnail download failed') so always go through this verb (it makes an f3d); NEVER FINISH the "
            "Package3D generator (Package3DStop) - its Save dialog + two opaque CEF modals are unbeatable, "
            "this verb saveAs-es instead. Full recipe: the fusion-multipart-libraries skill."
        ),
    }


_BUILD_IN_FLIGHT = {}          # lbrPath -> epoch started; page issue #62
_BUILD_FLIGHT_FILE = os.path.join(os.path.expanduser("~"), ".adom", "fusion-bridge",
                                  "builds_in_flight.json")


def _flight_load() -> dict:
    """In-flight builds recorded on DISK, because ab reaps and respawns this bridge.

    Peer lesson (pup thread, 2026-08-28): a guard that reads its own in-memory bookkeeping
    "passes vacuously" the moment that record is reset while the real condition persists.
    A multi-part 3D build runs for minutes and a respawn in the middle emptied this dict,
    so the build_in_progress refusal silently stopped firing. (The wrong_editor_captured
    check still caught the resulting frames, so this was wasted work rather than a false
    pass - but a guard that only sometimes fires is not a guard.)
    """
    try:
        import json as _js
        with open(_BUILD_FLIGHT_FILE, encoding="utf-8") as f:
            return {k: float(v) for k, v in (_js.load(f) or {}).items()}
    except Exception:
        return {}


def _flight_save():
    try:
        import json as _js, pathlib as _pl
        _pl.Path(_BUILD_FLIGHT_FILE).parent.mkdir(parents=True, exist_ok=True)
        with open(_BUILD_FLIGHT_FILE, "w", encoding="utf-8") as f:
            _js.dump(_BUILD_IN_FLIGHT, f)
    except Exception as e:
        print("[Fusion Bridge] could not persist in-flight builds (%s) - the "
              "build_in_progress guard will not survive a respawn" % e)


def _build_in_flight():
    """Paths whose async 3D build is still running (page issue #62, 2026-08-26).

    fusion_build_library_3d returns status:"started" and keeps working in a daemon thread.
    Nothing recorded that, so fusion_capture_library_views had no way to know a build was
    holding the 3D-package editor on screen - it captured that editor for every view and
    marked one ok:true with no chip in the frame. Evidence produced during a build is not
    evidence, so the capture path needs to be able to SEE the build.
    """
    import time as _t
    now = _t.time()
    # Merge the on-disk record so a respawn does not silently disarm this.
    for k, v in _flight_load().items():
        _BUILD_IN_FLIGHT.setdefault(k, v)
    # A build that has run absurdly long is treated as finished rather than blocking
    # captures forever - a stuck marker must not become a permanent refusal.
    return {k: v for k, v in _BUILD_IN_FLIGHT.items() if now - v < 3600}

def _reconcile_orphaned_runs():
    """At bridge start, fail every run the PREVIOUS bridge process left mid-flight.

    2026-08-28: run a34157c45412 sat at "proving DAC80504 (1 of 1), 95%, 0 frames" for
    twenty minutes with the add-in idle and no dialog on screen - because three
    bridge_installs had respawned the bridge while it ran. A chain is a daemon thread of
    the bridge process; a respawn kills it, but the run's persisted state keeps saying
    "proving" forever. The wiki then rendered a live progress bar over a corpse, and the
    single-flight guard attached the next press to the corpse. A respawn is the ONLY way
    a chain thread vanishes, so bridge start is the one reliable place to notice: any run
    persisted as in-progress here has no thread and must be marked failed, honestly.
    """
    import tempfile as _tf, json as _js, time as _t
    try:
        _p = os.path.join(_tf.gettempdir(), "fusion-prove-runs.json")
        if not os.path.exists(_p):
            return 0
        runs = _js.loads(open(_p, encoding="utf-8").read()) or {}
        orphaned = 0
        for rid, st in runs.items():
            if st.get("stage") not in ("done", "failed", None):
                st.update(stage="failed", success=False, verdict="not_proven",
                          errorCode="bridge_restarted_mid_run",
                          error=("The bridge process restarted (a bridge_install or crash) while "
                                 "this run was at '%s', which killed the run. Nothing after that "
                                 "stage happened. Start the import again." % st.get("stage")),
                          orphanedAt=_t.time())
                orphaned += 1
        if orphaned:
            open(_p, "w", encoding="utf-8").write(_js.dumps(runs))
            try:
                _PROVE_LAST[0] = dict(list(runs.values())[-1])
                open(os.path.join(_tf.gettempdir(), "fusion-prove-last.json"), "w",
                     encoding="utf-8").write(_js.dumps(_PROVE_LAST[0]))
            except Exception:
                pass
        _bridge_action_ledger("bridge_started", {"orphanedRunsFailed": orphaned})
        return orphaned
    except Exception as e:
        print("[Fusion Bridge] orphan reconcile failed: %s" % e)
        return 0


def _sweep_orphan_scratch_docs() -> dict:
    """Close scratch documents a chain that died with a bridge respawn left open.

    Only when a run is on file as bridge_restarted_mid_run and not yet swept; only documents
    that are bridge scratch by construction: never-modified 'Untitled*' and '<pkg>_3d' mint
    designs. A user's named or modified document is never touched. Ledgered.
    """
    import tempfile as _tf, json as _js
    _p = os.path.join(_tf.gettempdir(), "fusion-prove-runs.json")
    if not os.path.exists(_p):
        return {"swept": 0}
    runs = _js.loads(open(_p, encoding="utf-8").read()) or {}
    pending = [rid for rid, st in runs.items()
               if st.get("errorCode") == "bridge_restarted_mid_run" and not st.get("scratchSwept")]
    if not pending:
        return {"swept": 0}
    if not _addin_round_trip_ready(tries=1):
        return {"swept": 0, "why": "add-in not answering"}
    di = _proxy_to_addin("document_info", {}, timeout=8)
    closed = []
    for d in ((di.get("data") or {}).get("openDocuments") or []):
        n = str(d.get("name") or "").rstrip("*").strip()
        modified = bool(d.get("isModified") or d.get("modified") or str(d.get("name") or "").endswith("*"))
        if (n.lower().startswith("untitled") and not modified) or n.lower().endswith("_3d"):
            try:
                r = _close_bridge_doc_verified(n)
                if r.get("closed"):
                    closed.append(n)
            except Exception:
                pass
    for rid in pending:
        runs[rid]["scratchSwept"] = True
        runs[rid]["scratchSweptDocs"] = closed
    open(_p, "w", encoding="utf-8").write(_js.dumps(runs))
    _bridge_action_ledger("orphan_scratch_docs_swept", {"runs": pending, "closed": closed})
    return {"swept": len(closed), "closed": closed}


def _publish_run_state(run_id: str, state: dict):
    """Write a run's state to the SAME per-run store fusion_prove_result already reads.

    Colby, PR #58 / page issue #65: the detached fusion_build_library_3d swallowed its
    core result in the background thread, so callers had to grep the bound .lbr for
    wip_urns to find out what happened. Reusing this store means the build becomes
    pollable with NO new verb and NO new poll surface - fusion_prove_result {runId}
    already knows how to read it.
    """
    import tempfile as _tf
    try:
        _PROVE_LAST[0] = dict(state)
        open(os.path.join(_tf.gettempdir(), "fusion-prove-last.json"), "w",
             encoding="utf-8").write(json.dumps(state))
        _p = os.path.join(_tf.gettempdir(), "fusion-prove-runs.json")
        try:
            _runs = json.loads(open(_p, encoding="utf-8").read())
        except Exception:
            _runs = {}
        _runs[run_id] = dict(state)
        while len(_runs) > 20:
            _runs.pop(next(iter(_runs)))
        open(_p, "w", encoding="utf-8").write(json.dumps(_runs))
    except Exception:
        pass


def _orchestrate_build_library_3d(args: dict) -> dict:
    """Bind real 3D onto a multi-part library. Runs DETACHED by default (async=True) so it
    SURVIVES ab's 60s relay cap.

    ⚠️ LEARNED THE HARD WAY (2026-07-08): the cloud Package3D generate + Hub upload takes MINUTES
    on a real machine, but ab's relay hard-caps every request at ~60s and `timeoutSeconds` is NOT
    honored for this verb - so the old synchronous build got its thread KILLED at 60s, wrote no
    bound .lbr, and lost every wip_urn (3/4 pins on a demo board came back as flat pads because the
    4th never got a fresh urn). Fix: the build now runs in a BACKGROUND daemon thread and returns
    immediately; the caller POLLS the bound `outLbrPath` file (read_file) until it has one
    `<package3d ... wip_urn=urn:...>` per part. Pass `async:false` only on a fast machine where the
    whole build fits under 60s. Then embed those package3d + a per-`<element>` `package3d_urn` in a
    `.brd` and `fusion_open_board` + `fusion_show_3d_board` shows the REAL 3D bodies (background).

    args: {..., async?: bool (default TRUE - detached + pollable)}. See _build_library_3d_core.
    """
    combined = (args.get("lbrPath") or "").replace("\\", "/")
    out_path = (args.get("outLbrPath") or combined).replace("\\", "/")
    if args.get("async", True) and args.get("parts"):
        import threading as _th

        import time as _tt, uuid as _uu
        _BUILD_IN_FLIGHT[out_path] = _tt.time()
        _flight_save()
        _parts = args.get("parts") or []
        _run_id = _uu.uuid4().hex[:12]
        _bstate = {"runId": _run_id, "verb": "fusion_build_library_3d",
                   "stage": "starting", "boundLbr": out_path,
                   "partsTotal": len(_parts), "partsDone": 0, "parts": []}
        _publish_run_state(_run_id, _bstate)

        _descs = _library_descriptions((args.get("lbrPath") or "").replace("\\", "/"))

        def _on_part(entry, i, total):
            # PER-PART PROGRESS (Colby, PR #58): a 7-part build was a silent 10 minutes.
            _bstate["stage"] = "minting %s (%d/%d)" % (entry.get("package") or "part", i, total)
            _bstate["partsDone"] = i
            _bstate["parts"] = (_bstate.get("parts") or []) + [entry]
            _publish_run_state(_run_id, _bstate)
            # The in-Fusion banner can only land when the main thread is free, and during a
            # bind that is exactly HERE - between one part's STEP import and the next. On the
            # first watched real run (2026-08-28) it stayed absent for the whole bind stage
            # because nothing updated it at a yield point. Now a 7-part bind shows
            # "Binding 3D - minting X (3/7)" as each part lands.
            _banner("update", title="%s: binding 3D, part %d of %d" % (_lib_label(args.get("lbrPath")), i, total),
                    detail="%s. Importing its STEP and saving its 3D package"
                           % _named(entry.get("package") or "part", _descs),
                    percent=100.0 * i / max(1, total) * 0.4, eta_sec=None)

        def _run():
            try:
                _res = _build_library_3d_core(dict(args, _onPart=_on_part)) or {}
                # KEEP THE REPORT. The thread used to swallow it, so callers had to grep
                # the bound .lbr for urns to learn what happened (PR #58).
                _bstate.update(stage="done", success=bool(_res.get("success")),
                               result=_res, partsDone=len(_bstate.get("parts") or []))
            except Exception as _e:
                _bstate.update(stage="failed", success=False,
                               error="%s: %s" % (type(_e).__name__, _e))
            finally:
                # ALWAYS clear, even on failure: a marker that outlives its build would
                # refuse every later capture (page issue #62).
                _BUILD_IN_FLIGHT.pop(out_path, None)
                _flight_save()
                _publish_run_state(_run_id, _bstate)
        _th.Thread(target=_run, daemon=True).start()
        return {
            "success": True, "status": "started", "async": True,
            "runId": _run_id, "statusVerb": "fusion_prove_result",
            "boundLbr": out_path, "partsTotal": len(args.get("parts") or []),
            "_hint": (
                "⏳ 3D bind runs DETACHED (survives ab's 60s relay cap, which used to kill it + lose "
                "every wip_urn). POLL the boundLbr via read_file every ~30s until it has one "
                "<package3d name=... wip_urn=urn:...> PER PART (count == partsTotal). Do NOT re-fire "
                "while running (check fusion_addin_status.busy). When done, embed those <packages3d> + "
                "a per-<element> package3d_urn in your .brd, then fusion_open_board + fusion_show_3d_board "
                "for real 3D bodies (all background). If a urn stays unresolved in the 3D view, re-run "
                "for just that part - its f3d upload failed."),
        }
    return _build_library_3d_core(args)


def _build_library_3d_core(args: dict) -> dict:
    """Build a RENDERING multi-part 3D library in ONE call - the whole programmatic pipeline.

    For each part: make its footprint+chip f3d package (no GUI dialogs, optional BEFORE/AFTER
    screenshots), collect the wip_urn. Then inject ALL bindings into the combined .lbr in one
    pass (no hand XML surgery) and open the finished library ONCE. This is the verb to call for
    a basic-parts sampler / any many-part library - it replaces the per-part make_3d_package loop
    + manual binding the AI used to do.

    args: {
      lbrPath:  combined .lbr to bind + open (Windows path),
      parts:    [{package, lbrPath (per-part .lbr whose FIRST package is this footprint),
                  modelPath (.step)}],
      outLbrPath?: where to write the bound .lbr (default: overwrite lbrPath),
      capture?:    bool - capture before/after per part (default True),
      projectId?, folderId?: APS upload target (default: the MAIN-project upload folder),
      openWhenDone?: bool (default True)
    }
    """
    combined = (args.get("lbrPath") or "").replace("\\", "/")
    parts = args.get("parts") or []
    # PER-PART PROGRESS CALLBACK (Colby, PR #58). Passed IN-PROCESS by the detached
    # wrapper, never over the relay. A no-op in sync mode, so the sync contract is
    # unchanged - a 7-part build simply stops being a silent ten minutes.
    _cb = args.get("_onPart") if callable(args.get("_onPart")) else None
    _descs_sync = _library_descriptions(combined) if not _cb else {}

    def _emit_part(entry, i, total):
        if not _cb:
            # Sync mode (the import chain): the palette must still name the LIBRARY and the
            # part (John, 2026-08-30: "say what main library file you're working on ... as you
            # build up your bindings of 3d packages you don't make it obvious what the parent
            # task name is").
            try:
                _story = _step_story_from_file(entry.get("modelPath") or "")
                _banner("update", title="%s: binding 3D, part %d of %d" % (_lib_label(combined), i, total),
                        detail="%s. %s. Importing its STEP and saving its 3D package"
                               % (_named(entry.get("package") or "part", _descs_sync), _story),
                        percent=100.0 * i / max(1, total) * 0.4, eta_sec=None)
            except Exception:
                pass
        if _cb:
            try:
                _cb(entry, i, total)
            except Exception:
                pass
    if not combined or not parts:
        return {"success": False,
                "error": "lbrPath (combined .lbr) and parts[] are required.",
                "_hint": "parts: [{package, lbrPath (per-part footprint .lbr), modelPath (.step)}]. "
                         "projectId/folderId default to the MAIN-project upload folder."}
    out_path = (args.get("outLbrPath") or combined).replace("\\", "/")
    capture = args.get("capture", True)
    # Put this library's f3d in its OWN task subfolder under 'Adom AI Workspace' (never project root).
    import os as _os2
    task = args.get("task") or _os2.path.basename(combined).rsplit(".", 1)[0]
    project_id, folder_id = _discover_upload_target({**args, "task": task})
    # A missing APS workspace folder no longer gates the build: each part's mint saves
    # straight to the cloud through the add-in and only needs APS as its upload fallback
    # (adom/adom-bridge#93). folder_id=None flows down and the mint gates itself.
    results = []; bindings = []; shots = []
    cancelled = None
    for p in parts:
        pkg = p.get("package")
        per_lbr = (p.get("lbrPath") or "").replace("\\", "/")
        cancelled = _cancel_requested(args.get("_cancelKey"))
        if cancelled:
            # Stop HERE, at a part boundary: the parts already minted keep their urns and
            # are bound below; nothing half-done is left in Fusion.
            break
        # CLEAN UP PER PART, NOT AT THE END (John, 2026-08-29: "be better about clean up as you
        # progress"). Snapshot the open documents now; everything this part opens (its
        # generator Untitled, its <pkg>_3d design, its temporary library) is closed the moment
        # its urn is in hand, so a 20-part library never has 40 tabs waiting for a final sweep.
        try:
            _docs_before_part = _open_doc_names()
        except Exception:
            _docs_before_part = set()
        step = (p.get("modelPath") or "").replace("\\", "/")
        if not pkg or not per_lbr or not step:
            results.append({"package": pkg, "success": False,
                            "error": "package, lbrPath (per-part .lbr) and modelPath (.step) all required"})
            _emit_part(results[-1], len(results), len(parts))
            continue
        mk_args = {
            "lbrPath": per_lbr, "modelPath": step,
            "projectId": project_id, "folderId": folder_id,
            "fileName": f"{pkg}_3d", "task": task,
            "captureLabel": (pkg if capture else None),
            # Per-part orient passthrough (2026-07-07): a caller can set orient:false on a part
            # whose STEP is already correctly oriented (e.g. a tall through-hole pin). Defaults
            # to True, and make_3d_package's tall-part guard now keeps tall parts vertical anyway.
            "orient": p.get("orient", True),
        }
        mk = _orchestrate_make_3d_package(mk_args)
        # Retry once: the FIRST part of a run often fails transiently while Fusion settles from a
        # prior view/library; a single retry recovers it (the dropped-first-part trap).
        if not mk.get("success"):
            import time as _t
            _t.sleep(2)
            mk = _orchestrate_make_3d_package(mk_args)
        entry = {"package": pkg, "success": bool(mk.get("success"))}
        if mk.get("success"):
            entry["wip_urn"] = mk.get("wip_urn"); entry["dims_mm"] = mk.get("dims_mm")
            entry["pin1"] = mk.get("pin1"); entry["stepStory"] = _step_layer_story(mk.get("bodies"))
            bindings.append({"package": pkg, "wip_urn": mk["wip_urn"]})
            try:
                _p1 = mk.get("pin1") or {}
                _p1s = {"aligned": "pin 1 verified: the chip's mark sits over pad 1",
                        "rotated": "pin 1: rotated the chip %s deg so its mark sits over pad 1" % _p1.get("rotatedDeg"),
                        "no_mark_on_chip": "pin 1: no mark found on the chip body, alignment not checked",
                        "no_pad1_in_lbr": "pin 1: no pad 1 in the footprint, alignment not checked"}.get(
                            _p1.get("status") or "", "")
                _banner("update", title="%s: bound 3D, part %d of %d" % (_lib_label(combined), i, total),
                        detail="%s. %s. %s" % (_named(pkg, _descs_sync), entry["stepStory"], _p1s),
                        percent=100.0 * i / max(1, total) * 0.4, eta_sec=None)
            except Exception:
                pass
        else:
            entry["error"] = mk.get("error")
        if mk.get("before"):
            shots.append({"package": pkg, "stage": "before", "path": mk["before"]})
        if mk.get("after"):
            shots.append({"package": pkg, "stage": "after", "path": mk["after"]})
        results.append(entry)
        try:
            entry["scratchClosed"] = _close_docs_opened_since(_docs_before_part)
        except Exception:
            entry["scratchClosed"] = []
        _emit_part(entry, len(results), len(parts))
    # inject every binding in ONE pass, write the bound library. Read the ALREADY-BOUND output if it
    # exists so a re-run ACCUMULATES (merge-aware) onto prior successes instead of starting clean.
    bound = None
    if bindings:
        try:
            import os as _os
            src = out_path if _os.path.exists(out_path) else combined
            with open(src, "r", encoding="utf-8") as fh:
                txt = fh.read()
            txt = _inject_package3d_bindings(txt, bindings)
            with open(out_path, "w", encoding="utf-8") as fh:
                fh.write(txt)
            bound = out_path
        except Exception as e:
            return {"success": False, "error": f"binding injection failed: {e}",
                    "parts": results, "bindings": bindings, "screenshots": shots}
    # open the finished library ONCE (so check_dialogs reflects the merged result)
    opened = None
    if bound and args.get("openWhenDone", True):
        try:
            opened = bool(_orchestrate_open_lbr({"filePath": bound}).get("success"))
        except Exception:
            opened = False
    n_ok = sum(1 for r in results if r.get("success"))
    return {
        "success": n_ok > 0,
        "cancelled": bool(cancelled),
        "boundLbr": bound,
        "partsBound": n_ok,
        "partsTotal": len(parts),
        "parts": results,
        "screenshots": shots,
        "opened": opened,
        "_hint": (
            f"{n_ok}/{len(parts)} parts got a RENDERING footprint+chip 3D package; all bindings injected "
            f"into {bound} and the library opened once. NEXT: fusion_check_dialogs should be 0 (no "
            "broken-ref). For clean device previews + screenshots, save to the cloud and REOPEN from "
            "the cloud (fusion_aps_open) - the in-session view often gets an empty 'Untitled' shoved in "
            "front. Each device's lower-right 3D preview should show footprint + chip (NOT 'Thumbnail "
            "download failed'). The BEFORE (footprint) / AFTER (chip placed) PNGs are in screenshots[] "
            "on the Windows box (C:/tmp/conduit-screenshots) - pull them with desktop_pull_file (or "
            "re-capture via desktop_screenshot_window) for the demo video. "
            "AFTER (recommended): call fusion_capture_library_views per showcase part to grab the "
            "symbol / footprint / component (pin<->pad mapped) views - those are what make EEs trust "
            "the library, and the 3D-only shots miss them. "
            "PITFALLS: needs Fusion running. ⚠️ RELAY TIMEOUT: a many-part run takes minutes but the "
            "Adom Bridge relay times out the REQUEST at ~60s - you may get 'Request timed out' even "
            "though the build KEEPS RUNNING server-side and finishes. Do NOT assume it failed: wait, "
            "then verify by reading the boundLbr (count <package3d name=) + the before/after PNGs in "
            "C:/tmp/conduit-screenshots. If a part is missing, just re-run build_library_3d for the "
            "missing parts pointing outLbrPath at the SAME file - binding injection is now MERGE-AWARE "
            "and idempotent, so it accumulates onto the existing bindings (it no longer wipes the "
            "parts that already succeeded). Each part also self-retries once on a transient first-part "
            "failure. Full recipe: the fusion-multipart-libraries skill."
        ),
    }



def _lib_label(lbr_path) -> str:
    """The library's display name for palette titles: the file stem, or 'library'."""
    try:
        stem = str(lbr_path or "").replace("\\", "/").rsplit("/", 1)[-1].rsplit(".", 1)[0]
        return stem or "library"
    except Exception:
        return "library"


def _library_descriptions(lbr_path: str) -> dict:
    """{deviceset name -> description, package name -> description} for palette text (John,
    2026-08-29: "always give me the name/description of the chip so I know what it's for")."""
    out = {}
    try:
        md = _library_part_metadata(lbr_path)
        for ds in md.get("devicesets") or []:
            d = (ds.get("description") or "").strip()
            if d:
                out[str(ds.get("name") or "").lower()] = d
                for v in ds.get("variants") or []:
                    if v.get("package"):
                        out.setdefault(str(v["package"]).lower(), d)
    except Exception:
        pass
    return out


def _named(name: str, descs: dict, max_len: int = 70) -> str:
    """'CH340G: USB serial converter, UART...' or just the name when nothing is known."""
    d = (descs or {}).get(str(name or "").lower(), "")
    if not d:
        return str(name or "library")
    d = d if len(d) <= max_len else d[:max_len - 1].rstrip() + "…"
    return "%s: %s" % (name, d)

def _banner(action, title="", detail="", percent=None, eta_sec=None, tone="working"):
    """Drive the in-Fusion 'Adom is driving Fusion' palette. Fire-and-forget.

    Advisory only (John, 2026-08-28): the banner exists so a user looking AT Fusion during a
    10-15 minute import knows not to touch it - the taskbar bar vanishes the moment Fusion is
    maximised in front of them. It must never slow or fail the work it describes, so it is
    short-timeout and swallows every error. Same text the wiki sees.
    """
    # OFF THE CHAIN'S THREAD, WITH RETRIES (John, 2026-08-29, watching a ten-part run: "I don't
    # see the palette"). During bind and prove the add-in's main thread is busy almost all the
    # time, so an 8s fire-and-forget call mostly timed out and the palette was never even
    # created. Each call now runs in its own thread and retries with a longer window; the
    # chain never waits on it. _BANNER_STATE remembers the last message so the watchdog
    # (below) can re-assert it every few seconds for as long as a run is in flight.
    import threading as _bth
    if action in ("show", "update"):
        _BANNER_STATE.update(title=title, detail=detail, percent=percent, etaSec=eta_sec,
                             tone=tone, active=True)
    elif action == "hide":
        _BANNER_STATE["active"] = False

    def _send():
        # A terminal hide must land even if the add-in is busy saving/reopening the library
        # for a while: keep trying for a few minutes (a palette left behind after a run is
        # what John saw, 2026-08-30).
        for _to in ((8, 20, 30, 30, 30, 30, 30, 30) if action == "hide" else (8, 20, 30)):
            try:
                r = _proxy_to_addin("activity_banner",
                                    {"action": action, "title": title, "detail": detail,
                                     "percent": percent, "etaSec": eta_sec, "tone": tone}, timeout=_to)
                if r.get("success"):
                    return
            except Exception:
                pass
    _bth.Thread(target=_send, daemon=True).start()


_BANNER_STATE = {"active": False, "title": "", "detail": "", "percent": None, "etaSec": None,
                 "tone": "working", "watchdog": None}


def _banner_watchdog_start(run_id: str):
    """Re-assert the in-Fusion palette every 10s while a run is in flight.

    A palette can be lost mid-run (a sibling add-in copy's stop() tears it down, a busy main
    thread drops an update); a user looking at Fusion during a 15-minute import must never
    find it missing. The watchdog resends the last state until the run's terminal hide.
    """
    import threading as _bth, time as _bt

    def _loop():
        while _BANNER_STATE.get("watchdog") == run_id:
            if _BANNER_STATE.get("active"):
                try:
                    _proxy_to_addin("activity_banner",
                                    {"action": "update", "title": _BANNER_STATE.get("title"),
                                     "detail": _BANNER_STATE.get("detail"),
                                     "percent": _BANNER_STATE.get("percent"),
                                     "etaSec": _BANNER_STATE.get("etaSec"),
                                     "tone": _BANNER_STATE.get("tone")}, timeout=20)
                except Exception:
                    pass
            _bt.sleep(10)
    _BANNER_STATE["watchdog"] = run_id
    _bth.Thread(target=_loop, daemon=True).start()


def _banner_watchdog_stop(run_id: str):
    if _BANNER_STATE.get("watchdog") == run_id:
        _BANNER_STATE["watchdog"] = None


def _taskbar_progress(pct=None):
    """Show REAL progress on Fusion's Windows-taskbar button instead of orange flashes
    (John, 2026-08-23: he keeps seeing attention-flash highlights and no progress bar).

    Windows flashes a BACKGROUND window's button orange whenever the app requests
    attention (every dialog Fusion raises while the bridge drives it does this). We
    cannot stop Fusion REQUESTING it - but we can (a) clear the flash the moment we
    have handled whatever raised it, and (b) replace the signal with an honest
    progress bar during chain stages, paced by this machine's measured estimates.
    pct=None clears the bar. Best-effort: never raises, silently no-ops without ab."""
    try:
        if not ab_client.available():
            return
        info = get_fusion_window_info() or {}
        hwnd = info.get("hwnd") or (info.get("mainWindow") or {}).get("hwnd")
        if not hwnd:
            return
        args = {"hwnd": int(hwnd), "flash": "stop"}
        if pct is None:
            args["progress"] = {"state": "none"}
        else:
            args["progress"] = {"state": "normal",
                                "value": max(1, min(100, int(pct))), "max": 100}
        ab_client.call("desktop_taskbar", args, timeout=4)
    except Exception:
        pass


def _dismiss_dialogs_bg() -> list:
    """Dismiss any blocking Fusion dialog in the BACKGROUND, without stealing focus.

    Uses close_window (WM_CLOSE via PostMessage) = Cancel/No on the dialog - the background-safe
    dismiss. ⛔ NEVER use send_key/Escape for this: send_key calls SetForegroundWindow and YANKS
    Fusion to the foreground, disrupting the user (learned 2026-06-29). WM_CLOSE is also more reliable
    than Escape on Qt dialogs. Returns the titles dismissed."""
    try:
        info = get_fusion_window_info()
        dismissed = []
        for d in info.get("dialogs", []) or []:
            hwnd = d.get("hwnd")
            if hwnd:
                try:
                    close_window(hwnd)  # background WM_CLOSE = Cancel/No (never creates a stray)
                    dismissed.append(d.get("title") or "")
                except Exception:
                    pass
        return dismissed
    except Exception:
        return []


def _handle_dismiss_blocking_dialogs(fusion_info: dict, args: dict) -> dict:
    """NATIVE fusion_dismiss_blocking_dialogs (John, 2026-08-23: he hates seeing Fusion
    dialogs while the bridge drives, and the verb was falling to ab's BLIND WM_CLOSE
    fallback - adom-bridge#86 - which bypasses this bridge's classifier and cannot tell a
    harmless nag from a DATA-LOSS 'packages are uploading, sure you want to close?' prompt).

    This routes every open dialog through classify_blocking_dialogs() and answers each SAFELY
    in the BACKGROUND (WM_CLOSE = Cancel/No, never SetForegroundWindow, never Yes/close):
      - upload_in_progress: WM_CLOSE == 'No' == do NOT close == the upload keeps draining. We
        flag it so the caller polls hasActiveJobs before any retry (closing now still loses it).
      - everything else: WM_CLOSE == Cancel/No, the safe answer for every Fusion confirm.
    Returns what each dialog WAS (category) and how it was answered - so the caller never has to
    guess, and never gets a stray Yes.
    """
    classified = classify_blocking_dialogs()
    # `categories` narrows the sweep (the Rust readiness asks for ["crash_report"] only, the
    # janitor's exact scope); no filter keeps the full dismiss-everything behaviour.
    _want = set(a.strip() for a in (args.get("categories") or []) if str(a).strip())
    if _want:
        classified = [d for d in classified if d.get("category") in _want]
    if not classified:
        return {"success": True, "dismissed": [], "count": 0,
                "_hint": "No blocking Fusion dialog is open." if not _want else "No blocking dialog in the requested categories."}
    answered, upload_active = [], False
    for d in classified:
        hwnd, cat = d.get("hwnd"), d.get("category")
        safe = "No/Cancel (WM_CLOSE, background)"
        if cat == "upload_in_progress":
            upload_active = True
            safe = "No - declined close, Hub upload left to drain (NEVER Yes)"
        ok = False
        try:
            ok = bool(close_dialog_bg(hwnd))
        except Exception:
            ok = False
        answered.append({"title": d.get("title"), "category": cat,
                         "answeredWith": safe, "closed": ok,
                         "resolution": d.get("resolution")})
    hint = ("Answered %d blocking dialog(s) in the background (No/Cancel - never Yes, never "
            "focus-stolen)." % len(answered))
    if upload_active:
        hint += (" One was a Hub-UPLOAD 'lose these changes?' prompt: it was declined (No), but "
                 "the upload is STILL in flight - poll app.hasActiveJobs (fusion_state) until it "
                 "drains before any close/saveAs, or you WILL lose the 3D package.")
    _taskbar_progress(None) if not upload_active else _taskbar_progress(1)
    return {"success": True, "count": len(answered), "dismissed": answered,
            "uploadStillActive": upload_active, "_hint": hint}


COMMAND_HANDLERS["dismiss_blocking_dialogs"] = _handle_dismiss_blocking_dialogs


def _capture_3d_view(pkg: str, lbr_path: str, settle: float) -> dict:
    """Capture the DEDICATED 3D-package render as its own artifact (page issue #63).

    The 3D chip previously existed only as a thumbnail inside the ganged component view.
    Getting it standalone meant a separate fusion_show_3d_package call that re-attaches the
    STEP and re-renders - measured at a 180s timeout during the import campaign, and it
    took Fusion down with it under concurrent load. The 3D body is already built and saved
    by the time captures run, so entering the Package3D environment and photographing it is
    cheap: no re-attach, no re-render.
    """
    import time as _t
    entry = {"package": pkg, "view": "3d", "path": None, "ok": False}
    # A PLACEHOLDER IS NOT A RESULT (page issue #42). With no <package3d> bound, Fusion
    # renders a grey slab that photographs exactly like a real chip. Knowable offline.
    if lbr_path:
        try:
            if not _library_artifact_names(lbr_path).get("p3d"):
                entry.update({
                    "errorCode": "no_3d_package_bound",
                    "error": "No <package3d> is bound in that library, so the only thing to "
                             "photograph is Fusion's grey Placeholder - refusing to return "
                             "that as the 3D artifact.",
                    "_hint": "Run fusion_build_library_3d (or fusion_bind_library_3d) first, "
                             "then re-capture. A Placeholder looks like a successful 3D view "
                             "in a screenshot, which is why this refuses instead."})
                return entry
        except Exception:
            pass
    cp = _create_3d_package(lbr_path, timeout=60) if lbr_path else {"success": False,
                                                                    "error": "no libraryPath"}
    if not cp.get("success"):
        entry.update({"errorCode": "create_3d_package_failed",
                      "error": "Could not enter the Package3D environment: %s"
                               % str(cp.get("error"))[:200]})
        return entry
    try:
        if settle:
            _t.sleep(settle)
        entry["path"] = _capture_labeled("%s_3d" % pkg)
        entry["ok"] = bool(entry["path"])
    finally:
        # ALWAYS leave the Package3D environment, even if the capture threw. Staying in it
        # would make every LATER view capture the 3D editor - exactly the mislabelled-frame
        # failure reported in page issue #62.
        try:
            _proxy_to_addin("execute_text_command", {"command": "Package3DStop"}, timeout=20)
            _t.sleep(0.6)
            _d = _dismiss_dialogs_bg()
            if _d:
                print("[Fusion Bridge] Package3DStop raised a dialog (%s) - answered in the "
                      "background; see skills/fusion-no-modal-dialogs" % _d)
        except Exception as _e:
            print("[Fusion Bridge] Package3DStop failed: %s" % _e)
    return entry

def _orchestrate_capture_library_views(args: dict) -> dict:
    """Capture the LIBRARY-EDITOR views that make EEs trust a part: the schematic SYMBOL, the
    FOOTPRINT (pads + layer stack), and the COMPONENT/device view (Content Manager: the symbol +
    the package table with the footprint<->package Mapped check + pin/pad counts). These are what
    developers want to see - the 3D before/after shots alone don't show them.

    Requires the .lbr OPEN in the Electronics Library workspace (fusion_open_lbr first). For each
    package it runs the EAGLE/Electron EDIT <pkg>.sym / .pac / .dev, WINDOW FIT to frame, and a
    background hwnd screenshot (never fullscreen). Returns the saved PNG paths on the Windows box
    (C:/tmp/conduit-screenshots - pull them with desktop_pull_file).

    args: {packages: [<deviceset name>, ...] (or a single 'package'),
           views?: subset of ['component','symbol','footprint'] (default all three),
           settle?: seconds to wait after each EDIT before framing/capturing (default 1.5 - raise it
                    if a shot shows the PREVIOUS part, lower it to go faster on a snappy machine)}
    NOTE: ~2s per view * parts * views can exceed the ~60s relay request timeout - the captures still
    complete server-side; verify the PNGs landed in C:/tmp/conduit-screenshots and pull them.
    """
    import time as _t
    pkgs = args.get("packages") or ([args["package"]] if args.get("package") else [])
    if not pkgs:
        return {"success": False,
                "error": "packages: [<deviceset name>] (or package: <name>) required.",
                "_hint": "Open the .lbr first (fusion_open_lbr). Names are the deviceset names, e.g. ESP32-S3FN8."}
    # FOUR artifacts, in ASSEMBLY ORDER (page issues #60 and #63): symbol, footprint,
    # the dedicated 3D chip, then the ganged component view LAST as the closing proof.
    # The 3D chip used to exist only as a thumbnail inside the ganged view, so a caller
    # could not show it as its own artifact without a separate fusion_show_3d_package call
    # that re-attaches the STEP - slow (180s timeout observed) and crash-prone under load.
    view_ext = {"component": "dev", "symbol": "sym", "footprint": "pac", "3d": "3d",
                "dev": "dev", "sym": "sym", "pac": "pac", "p3d": "3d", "package3d": "3d"}
    label_of = {"dev": "component", "sym": "symbol", "pac": "footprint", "3d": "3d"}
    views = args.get("views") or ["symbol", "footprint", "3d", "component"]
    # EDIT is async - the editor takes a beat to actually SWITCH the view. Capturing too soon grabs
    # the PREVIOUS view (a mislabeled shot). Settle AFTER the EDIT (before fit/capture); tunable.
    settle = float(args.get("settle", 1.5))
    # ⛔ PRE-CHECK EVERY NAME OFFLINE, BEFORE ANY EDIT REACHES FUSION.
    # This verb used to fire EDIT on whatever it was handed and then call
    # _dismiss_dialogs_bg() to clear the "Create new footprint 'X'?" modal it had just
    # caused. That is backwards three ways: the modal freezes the add-in's main thread
    # (so every later verb reads as a crash), it dings the machine's alert sound at a user
    # who did not do anything, and dismissal is a RACE we lose whenever Fusion is slow.
    # The .lbr on disk already knows every symbol/package/deviceset name, so a name that
    # would prompt is knowable in milliseconds without Fusion. Resolve here; never ask.
    # REFUSE TO MANUFACTURE EVIDENCE DURING A BUILD (page issue #62, 2026-08-26).
    # fusion_build_library_3d is async; while it runs it holds the 3D-package editor on
    # screen, so EDIT <name>.sym/.pac land on that editor and every capture shows the part
    # being built rather than the view it is labelled with. One came back ok:true with no
    # chip in the frame. A false pass is worse than a refusal: it is evidence that lies.
    _flight = _build_in_flight()
    if _flight and not args.get("ignoreBuildInProgress"):
        import time as _ft
        return {"success": False, "errorCode": "build_in_progress",
                "error": "A 3D build is still running for %s - refusing to capture, because "
                         "the build holds the 3D-package editor on screen and every view "
                         "would show that editor instead of the view it is labelled with."
                         % ", ".join(sorted(_flight)),
                "buildsInFlight": [{"lbrPath": k, "runningSec": int(_ft.time() - v)}
                                   for k, v in sorted(_flight.items())],
                "_hint": "Wait for the build to finish, then re-capture - the same call "
                         "returns correct content once it is done. Detect completion by "
                         "polling fusion_get_app_state until the <part>_3d document reports "
                         "isSaved:true, or read the bound .lbr until it has one <package3d "
                         "wip_urn=...> per part. Pass ignoreBuildInProgress:true only if you "
                         "genuinely want a mid-build frame."}
    lbr_path = (args.get("libraryPath") or "").replace("\\", "/") or _remembered_open_library()
    resolved_names = {}
    name_errors = []
    if lbr_path:
        for pkg in pkgs:
            for v in views:
                ext = view_ext.get(v, v)
                if ext == "3d":
                    continue          # not an EDIT target; guarded by its own p3d check
                r, err = _resolve_artifact_name(pkg, ext, lbr_path)
                if err:
                    name_errors.append({"package": pkg, "view": label_of.get(ext, ext),
                                        "error": err.get("error"),
                                        "available": err.get("available"),
                                        "closestMatches": err.get("closestMatches")})
                else:
                    resolved_names[(pkg, ext)] = r
    out = []
    for pkg in pkgs:
        for v in views:
            ext = view_ext.get(v, v)
            if ext == "3d":
                out.append(_capture_3d_view(pkg, lbr_path, settle))
                continue
            if lbr_path and (pkg, ext) not in resolved_names:
                # Would have prompted. Report it as a skip; do NOT hand it to Fusion.
                _bad = next((e for e in name_errors
                             if e["package"] == pkg and e["view"] == label_of.get(ext, ext)), {})
                out.append({"package": pkg, "view": label_of.get(ext, ext), "path": None,
                            "ok": False, "skipped": "name_not_in_library",
                            "error": _bad.get("error"),
                            "available": (_bad.get("available") or [])[:20],
                            "closestMatches": _bad.get("closestMatches"),
                            "_hint": "No EDIT was issued and no dialog was raised - this name "
                                     "is not in the library, so opening it would have popped "
                                     "Fusion's blocking 'Create new ...?' modal. Pick a name "
                                     "from `available`."})
                continue
            name = resolved_names.get((pkg, ext), pkg)
            er = _proxy_to_addin("electron_run", {"command": f"EDIT {name}.{ext}", "confirm": "no"}, timeout=30)
            _t.sleep(settle)  # let the editor LOAD the new view before framing/capturing it
            # ALWAYS catch + dismiss any dialog the EDIT raised - in the BACKGROUND (no focus steal).
            # Common one: "Create new symbol/footprint '<name>'?" when <name> is a DEVICESET name but
            # the symbol/package is named by the shared combo. Dismissing (Cancel/No) avoids a stray.
            # BACKSTOP ONLY. With the pre-check above this should never fire; if it does,
            # the pre-check has a hole (a library opened by a route that did not record its
            # path, or a name EAGLE renamed on load) and that is a BUG to fix at the source,
            # not a dialog to keep sweeping. Log it loudly so it cannot stay invisible.
            dismissed = _dismiss_dialogs_bg()
            if dismissed:
                print("[Fusion Bridge] ⚠ PRE-CHECK HOLE: EDIT %s.%s still raised a dialog "
                      "(lbr_path=%r). Prevention failed; dismissal should not be load-bearing."
                      % (name, ext, lbr_path))
                _bridge_action_ledger("dialog_precheck_hole",
                                      {"name": name, "ext": ext, "lbrPath": lbr_path,
                                       "dismissed": dismissed})
            _proxy_to_addin("electron_run", {"command": "WINDOW FIT"}, timeout=20)
            _t.sleep(0.6)
            shot = _capture_labeled(f"{pkg}_{label_of.get(ext, ext)}")
            # ok MUST MEAN "the frame shows the view it is labelled with", not "a file was
            # written" (page issue #62). The reported failure captured the 3D-package
            # editor for every view and still marked one ok:true. We cannot read the pixels
            # from here, but we CAN ask Fusion which editor was actually on screen at
            # capture time, which is exactly what went wrong - so a frame taken outside the
            # Electronics Library editor is never reported as a pass.
            editor_ws, editor_ok = None, True
            try:
                _st = _proxy_to_addin("get_app_state", {}, timeout=15) or {}
                editor_ws = _st.get("activeWorkspace")
                if editor_ws and "electronics library" not in str(editor_ws).lower():
                    editor_ok = False
            except Exception:
                # "unknown, not a pass and not a failure" is what the comment USED to say
                # while the code left editor_ok True, i.e. it passed. A frame we cannot
                # attribute to the right editor is not evidence: if the add-in did not
                # answer, we do not know what was on screen, and ok:true would be a claim
                # we cannot support. Reported as unverified instead (peer lesson, pup
                # 2026-08-28: never default an unreadable condition to "fine").
                editor_ws, editor_ok = None, False
            entry = {"package": pkg, "view": label_of.get(ext, ext), "path": shot,
                     "dialogDismissed": dismissed or None,
                     "capturedInWorkspace": editor_ws,
                     "ok": (bool(er.get("success", True)) and bool(shot)
                            and not dismissed and editor_ok)}
            if not editor_ok and editor_ws is None:
                entry["errorCode"] = "editor_unverified"
                entry["error"] = ("Could not read which editor was active at capture time "
                                  "(the add-in did not answer), so this frame cannot be "
                                  "confirmed to show the view it is labelled with.")
                entry["_hint"] = ("Not necessarily a bad frame - it is an unverified one. "
                                  "Check fusion_addin_status and re-capture.")
            elif not editor_ok:
                entry["errorCode"] = "wrong_editor_captured"
                entry["error"] = ("Captured while %r was active, not the Electronics Library "
                                  "editor - the frame does not show the %s view it is "
                                  "labelled with." % (editor_ws, label_of.get(ext, ext)))
            out.append(entry)
    n_ok = sum(1 for o in out if o.get("ok"))
    return {
        "success": n_ok > 0,
        "captured": out,
        "_hint": (
            f"Captured {n_ok}/{len(out)} library-editor views to C:/tmp/conduit-screenshots (pull with "
            "desktop_pull_file). SYMBOL = full pinout; FOOTPRINT = pads + layer stack; COMPONENT = the "
            "Content Manager device view with the footprint<->package Mapped check + pin/pad counts. "
            "⚠️ NAMES: the COMPONENT (.dev) view opens by DEVICESET name (e.g. R-0603-10K). But SYMBOL "
            "(.sym) and FOOTPRINT (.pac) open by the SYMBOL / PACKAGE name - in a SHARED-FOOTPRINT "
            "library (one symbol/footprint per package, many value devicesets) that is the COMBO name "
            "(e.g. 'R-0603'), NOT the deviceset name ('R-0603-10K'). Passing a deviceset name to .sym/"
            ".pac makes Fusion pop 'Create new symbol/footprint?'; the bridge now auto-dismisses that in "
            "the BACKGROUND (Cancel/No, no focus steal - see entries with dialogDismissed) and marks the "
            "view not-ok, but you should pass the right name. For shared libraries the COMPONENT view "
            "alone shows symbol + footprint + 3D previews + Mapped, so it is usually enough. "
            "PITFALLS: the .lbr must be OPEN in the Electronics Library workspace (fusion_open_lbr first)."
        ),
    }


def _orchestrate_cleanup_cloud_files(args: dict) -> dict:
    """Precisely delete a LIST of cloud files by lineage urn - SAFE cleanup of AI-created clutter.

    Deletes ONLY the exact `fileIds` (lineage urns) given - never name-guessing, so it cannot touch a
    teammate's file in a shared folder. Loops server-side (one call deletes the whole list). Use this
    to clean up f3d files the bridge created. (Find the ids with fusion_aps_browse on the folder.)

    args: {fileIds: [<lineage urn>, ...], projectName?: str (default active), folderPath?: str
           (default root - where the file lives)}
    """
    file_ids = args.get("fileIds") or []
    if not file_ids:
        return {"success": False, "error": "fileIds [lineage urns] required.",
                "_hint": "Get them from fusion_aps_browse {projectId, folderId} (item .id)."}
    project_name = args.get("projectName", "")
    folder_path = args.get("folderPath", "")
    deleted = []; failed = []
    for fid in file_ids:
        try:
            r = _proxy_to_addin("delete_cloud_file",
                                {"fileId": fid, "projectName": project_name, "folderPath": folder_path},
                                timeout=40)
            if isinstance(r, dict) and r.get("success"):
                deleted.append(fid)
            else:
                failed.append({"fileId": fid, "error": (r.get("error") if isinstance(r, dict) else str(r))[:90]})
        except Exception as e:
            failed.append({"fileId": fid, "error": str(e)[:90]})
    return {
        "success": len(deleted) > 0 or not failed,
        "deletedCount": len(deleted),
        "failedCount": len(failed),
        "failed": failed[:25],
        "_hint": (
            f"Deleted {len(deleted)}/{len(file_ids)} cloud files by PRECISE lineage urn (no name-guessing, "
            "so teammates' files are untouched). NOTE: a large list takes minutes and the relay request "
            "may time out at ~60s while the deletes continue server-side - re-browse the folder to confirm "
            "the count dropped. Anything in failed[] usually means the file was already gone or you lack "
            "delete permission. See the fusion-cloud-hygiene skill."
        ),
    }


def _orchestrate_save_lbr(args: dict) -> dict:
    """Save the currently open Electronics library as a .flbr file.

    Uses Document.CopyToDesktop to export the library in Fusion's native
    .flbr format. The file can be re-opened later or uploaded to the cloud.
    """
    import os

    output_path = args.get("outputPath", "")
    if not output_path:
        return {"success": False, "error": "No outputPath specified"}

    output_path = output_path.replace("\\", "/")

    # Ensure path ends with .flbr
    if not output_path.lower().endswith(".flbr"):
        output_path += ".flbr"

    # Ensure parent directory exists
    parent = os.path.dirname(output_path)
    if parent and not os.path.exists(parent):
        try:
            os.makedirs(parent, exist_ok=True)
        except Exception as e:
            return {"success": False, "error": f"Cannot create directory {parent}: {e}"}

    # Execute Document.CopyToDesktop via the add-in
    result = _proxy_to_addin("execute_text_command", {
        "command": f"Document.CopyToDesktop {_fusion_text_path(output_path)}",
    }, timeout=30)

    if not result.get("success"):
        return {
            "success": False,
            "error": f"Document.CopyToDesktop failed: {result.get('error', 'unknown')}",
            "data": {"outputPath": output_path},
        }

    # Verify the file was created
    if os.path.exists(output_path):
        file_size = os.path.getsize(output_path)
        result = {
            "success": True,
            "output": f"Saved library to {os.path.basename(output_path)} ({file_size} bytes)",
            "data": {"outputPath": output_path, "fileSize": file_size},
        }
    else:
        result = {
            "success": True,
            "output": f"Document.CopyToDesktop executed (file may still be writing)",
            "data": {"outputPath": output_path},
        }

    # Auto-screenshot after save — catches blocking dialogs
    return _post_open_screenshot(result, settle_time=1.0)


# Package types = EPG's Scripts3d module names (each has runWithInput(params)).
# Kept in sync with Autodesk's ElectronicsPackageGenerator internal add-in.
_EPG_TYPES = [
    "axial_diode", "axial_fuse", "axial_polarized_capacitor", "axial_resistor",
    "bga", "chip", "chiparray2sideconvex", "chiparray2sideflat", "chiparray4sideflat",
    "chip_led", "cornerconcave", "crystal", "dfn2", "dfn3", "dfn4", "dip",
    "dip_socket", "dip_socket_dual_leaf", "dpak", "ecap", "female_standoff", "hc49",
    "header_right_angle", "header_right_angle_socket", "header_straight",
    "header_straight_socket", "male_female_standoff", "melf", "molded",
    "oscillator_j", "oscillator_l", "plcc", "qfn", "qfp", "radial_dipped_rect",
    "radial_ecap", "radial_inductor", "radial_round_led", "snap_lock", "sod",
    "sodfl", "soic", "soj", "son", "sot143", "sot223", "sot23", "sotfl",
    "surface_mount_header_female", "surface_mount_pin_header_right_angle",
    "surface_mount_pin_header_straight",
]

# EPG param keys that are NOT lengths (never mm->cm converted).
_EPG_NON_DIMENSION_KEYS = {"DPins", "EPins", "pins", "thermal", "color_r", "color_g", "color_b"}


def _orchestrate_generate_package(args: dict) -> dict:
    """Generate an IPC-compliant 3D package via Fusion's built-in
    ElectronicsPackageGenerator (EPG), optionally laser-etch a marking into the
    body top, and optionally export STEP - all in ONE headless add-in call.

    Proven live 2026-07-03 (0603 + '103' etch + 233KB STEP, 1.9s generation).
    """
    pkg_type = (args.get("type") or "").strip().lower()
    if pkg_type not in _EPG_TYPES:
        return {
            "success": False,
            "error": f"Unknown package type: {pkg_type!r}",
            "errorCode": "unknown_package_type",
            "supportedTypes": _EPG_TYPES,
            "_hint": "Pass type as one of supportedTypes (EPG Scripts3d module names). "
                     "Common: chip (0402/0603/0805 passives), soic, qfn, qfp, bga, sot23, "
                     "dfn2, melf, ecap, crystal, header_straight.",
        }

    raw_params = args.get("params") or {}
    units_cm = bool(args.get("unitsCm"))
    params = {}
    for k, v in raw_params.items():
        if (not units_cm) and isinstance(v, (int, float)) and k not in _EPG_NON_DIMENSION_KEYS:
            params[k] = v / 10.0  # mm (datasheet-native) -> cm (Fusion/EPG-native)
        else:
            params[k] = v

    etch = args.get("etch") or ""
    etch_style = (args.get("etchStyle") or "raised").strip().lower()  # raised (white, default) | engraved
    etch_depth_cm = float(args.get("etchDepthMm") or 0.03) / 10.0
    etch_height_mm = args.get("etchHeightMm")  # None = auto-fit
    output_step = args.get("outputStep") or ""
    bridge_version = BRIDGE_VERSION

    script = f"""
import sys, glob, os, importlib
import adsk.core, adsk.fusion

# Locate EPG across install conventions + versions (NEVER hardcode the webdeploy hash).
_bases = []
for env in ('ProgramFiles', 'ProgramFiles(x86)', 'ProgramW6432', 'LOCALAPPDATA'):
    root = os.environ.get(env)
    if root:
        _bases += glob.glob(os.path.join(root, 'Autodesk', 'webdeploy', 'production', '*',
                                         'Api', 'InternalAddins', 'ElectronicsPackageGenerator'))
if not _bases:
    raise RuntimeError('EPG_NOT_FOUND: ElectronicsPackageGenerator not present in this Fusion install')
epg_dir = max(_bases, key=os.path.getmtime)
parent = os.path.dirname(epg_dir)
if parent not in sys.path:
    sys.path.insert(0, parent)

mod = importlib.import_module('ElectronicsPackageGenerator.Scripts3d.{pkg_type}')

doc = app.documents.add(adsk.core.DocumentTypes.FusionDesignDocumentType)
design = adsk.fusion.Design.cast(app.activeProduct)
mod.runWithInput({params!r}, design)
root = design.rootComponent

inv = [{{'name': b.name, 'vol': round(b.volume, 6)}} for b in root.bRepBodies]
etched = None
if {etch!r}:
    # FIND THE TOP SURFACE the robust way (John's algorithm, 2026-07-04):
    # 1. bounding box of the WHOLE chip (all bodies);
    # 2. only faces whose centroid sits in the TOP 5% band of that bbox count;
    # 3. among those upward planar faces take the LARGEST AREA (the molded body
    #    top beats terminal tops that tie it on height);
    # 4. lay the text out with a 10% margin inside that face.
    gmin, gmax = 1e9, -1e9
    for b in root.bRepBodies:
        bb = b.boundingBox
        gmin = min(gmin, bb.minPoint.z)
        gmax = max(gmax, bb.maxPoint.z)
    band_z = gmax - 0.05 * (gmax - gmin)
    top_face, top_area = None, -1.0
    for b in root.bRepBodies:
        for f in b.faces:
            if isinstance(f.geometry, adsk.core.Plane) and f.geometry.normal.z > 0.9 \
                    and f.centroid.z >= band_z and f.area > top_area:
                top_face, top_area = f, f.area
    if top_face is None:
        raise RuntimeError('ETCH_NO_TOP_FACE: no upward planar face in the top 5% of the chip bbox')
    body = top_face.body
    sk = root.sketches.add(top_face)
    center = sk.modelToSketchSpace(top_face.centroid)
    # Measure the face in SKETCH space (model-space bbox axes can be SWAPPED vs
    # the sketch axes the text lays out in). Project the model bbox corners.
    bb = top_face.boundingBox
    p_min = sk.modelToSketchSpace(bb.minPoint)
    p_max = sk.modelToSketchSpace(bb.maxPoint)
    face_w = abs(p_max.x - p_min.x)   # extent along sketch-x (the default text baseline)
    face_h = abs(p_max.y - p_min.y)   # extent along sketch-y
    # ALWAYS run the text along the LONGEST axis of the top face (most room for
    # the marking - John's rule, 2026-07-04). If the long axis is sketch-y,
    # rotate the text 90deg rather than squeezing it onto the short axis.
    import math as _math
    rot_deg = 90 if face_h > face_w else 0
    long_len = max(face_w, face_h)
    short_len = min(face_w, face_h)
    w = long_len * 0.80               # 10% margin each side along the long axis
    box_h_cap = short_len * 0.80      # 10% margin each side across it
    # multi-line support (e.g. MPN + variant on line 2): fit per-line
    lines = {etch!r}.split(chr(10))
    n_lines = max(1, len(lines))
    max_chars = max(1, max(len(l) for l in lines))
    # width-aware auto-fit: glyph ~0.75*h wide; line block ~1.35*h per line
    h = ({etch_height_mm!r} / 10.0) if {etch_height_mm!r} else \
        max(0.015, min(box_h_cap / (1.35 * n_lines), w / (0.75 * max_chars)))
    if rot_deg:
        c1 = adsk.core.Point3D.create(center.x - box_h_cap/2, center.y - w/2, 0)
        c2 = adsk.core.Point3D.create(center.x + box_h_cap/2, center.y + w/2, 0)
    else:
        c1 = adsk.core.Point3D.create(center.x - w/2, center.y - box_h_cap/2, 0)
        c2 = adsk.core.Point3D.create(center.x + w/2, center.y + box_h_cap/2, 0)
    ti = sk.sketchTexts.createInput2({etch!r}, h)
    ti.setAsMultiLine(c1, c2,
                      adsk.core.HorizontalAlignments.CenterHorizontalAlignment,
                      adsk.core.VerticalAlignments.MiddleVerticalAlignment, 0)
    if rot_deg:
        try:
            ti.angle = _math.pi / 2.0
        except Exception:
            rot_deg = 0  # older API without angle - fall back to unrotated
    txt = sk.sketchTexts.add(ti)
    # MEASURE-AND-CORRECT (the margin is a CONTRACT): font metrics vary, so the
    # 0.75h glyph estimate can under-shoot ('ADOM-A' rendered edge-to-edge). After
    # placing, measure the text's real bbox; if it busts the 10%-margin box,
    # recreate at the scaled-down height. Never trust an estimate you can measure.
    fit_iterations = 0
    measured_w = None
    for _pass in range(2):
        tb = txt.boundingBox
        t_ext_x = abs(tb.maxPoint.x - tb.minPoint.x)
        t_ext_y = abs(tb.maxPoint.y - tb.minPoint.y)
        measured_w = max(t_ext_x, t_ext_y)   # the baseline-axis extent
        measured_c = min(t_ext_x, t_ext_y)   # the cross-axis extent (line stack)
        if measured_w <= w and measured_c <= box_h_cap:
            break
        scale = min(w / max(measured_w, 1e-6), box_h_cap / max(measured_c, 1e-6)) * 0.95
        h = max(0.008, h * scale)
        txt.deleteMe()
        ti = sk.sketchTexts.createInput2({etch!r}, h)
        ti.setAsMultiLine(c1, c2,
                          adsk.core.HorizontalAlignments.CenterHorizontalAlignment,
                          adsk.core.VerticalAlignments.MiddleVerticalAlignment, 0)
        if rot_deg:
            try:
                ti.angle = _math.pi / 2.0
            except Exception:
                pass
        txt = sk.sketchTexts.add(ti)
        fit_iterations += 1
    marking_geom = {{
        'chipBBox': {{'min': [round(gmin,5), 0, 0], 'maxZ': round(gmax,5)}},
        'chipBBoxZ': {{'min': round(gmin,5), 'max': round(gmax,5)}},
        'topBandZ': round(band_z,5),
        'topFace': {{'area': round(top_area,6), 'centroidZ': round(top_face.centroid.z,5),
                    'sketchExtents': {{'x': round(face_w,5), 'y': round(face_h,5)}},
                    'body': body.name}},
        'longAxis': 'sketch-y' if face_h > face_w else 'sketch-x',
        'rotatedDeg': rot_deg,
        'textBox': {{'c1': [round(c1.x,5), round(c1.y,5)], 'c2': [round(c2.x,5), round(c2.y,5)],
                    'marginPct': 10}},
        'textHeightMm': round(h*10,4), 'lines': n_lines, 'maxLineChars': max_chars,
        'heightAuto': not bool({etch_height_mm!r}),
        'textWidthMeasuredMm': round(measured_w*10,4) if measured_w else None,
        'fitIterations': fit_iterations,
        'why': {{'band': 'top 5% of whole-chip bbox excludes terminal tops that tie the body',
                'largestArea': 'molded body top beats small terminal faces in the band',
                'longAxis': 'text runs along the longest face axis for maximum room',
                'fit': 'h = min(boxH/(1.35*lines), boxW/(0.75*maxChars)) - glyphs ~0.75h wide'}},
    }}
    if {etch_style!r} == 'engraved':
        # engraved (sunken) cut - the pre-2026-07-04 default
        ext_in = root.features.extrudeFeatures.createInput(txt, adsk.fusion.FeatureOperations.CutFeatureOperation)
        ext_in.setDistanceExtent(False, adsk.core.ValueInput.createByReal(-{etch_depth_cm!r}))
        ext_in.participantBodies = [body]
        root.features.extrudeFeatures.add(ext_in)
    else:
        # RAISED WHITE marking (default): humans need CONTRAST - a thin positive
        # extrude painted white on the dark body reads like real silkscreen ink,
        # far better than a same-color emboss. Reuses EPG's own rgb-appearance
        # utility so the white survives Fusion rendering (+ colored STEP).
        ext_in = root.features.extrudeFeatures.createInput(txt, adsk.fusion.FeatureOperations.NewBodyFeatureOperation)
        ext_in.setDistanceExtent(False, adsk.core.ValueInput.createByReal({etch_depth_cm!r}))
        mark = root.features.extrudeFeatures.add(ext_in)
        from ElectronicsPackageGenerator.Utilities import addin_utility as _au
        for i in range(mark.bodies.count):
            mb = mark.bodies.item(i)
            mb.name = 'Marking' if i == 0 else 'Marking' + str(i + 1)
            _au.apply_rgb_appearance(app, design, mb, 255, 255, 255, 'AdomMarkingWhite')
    etched = {etch!r}

step_path = None
if {output_step!r}:
    em = design.exportManager
    opts = em.createSTEPExportOptions({output_step!r})
    em.execute(opts)
    step_path = {output_step!r}

# SIDECAR MANIFEST: every setting we picked + WHY + the bboxes we calculated,
# so a marking refresh (e.g. adding a variant name as a 2nd line) can rework the
# text without re-deriving anything. Written next to the STEP as <step>.manifest.json.
manifest = {{
    'schema': 'adom-fusion-generate-package/1',
    'bridgeVersion': {bridge_version!r},
    'type': {pkg_type!r},
    'paramsCm': {params!r},
    'paramsNote': 'paramsCm are EPG-native cm (mm inputs were /10)',
    'marking': ({{'text': {etch!r}, 'style': {etch_style!r},
                'depthMm': {etch_depth_cm!r} * 10, **marking_geom}} if etched else None),
    'bodies': inv,
    'outputs': {{'step': step_path}},
    'refresh': {{
        'howTo': 'To re-mark (e.g. add a variant on line 2): call fusion_generate_package again '
                 'with the SAME type+paramsCm (unitsCm:true) and the new multi-line etch text '
                 '(join lines with a newline). The generator is deterministic, so geometry and '
                 'placement reproduce exactly; this manifest carries the boxes/height the last '
                 'run chose for comparison.',
        'example': {{'type': {pkg_type!r}, 'unitsCm': True, 'params': 'paramsCm from this file',
                    'etch': ({etch!r} + chr(10) + 'VARIANT') if etched else 'MPN' + chr(10) + 'VARIANT'}},
    }},
}}
manifest_path = None
if step_path:
    manifest_path = step_path + '.manifest.json'
    import json as _json
    with open(manifest_path, 'w', encoding='utf-8') as _f:
        _json.dump(manifest, _f, indent=2)

vp = app.activeViewport
cam = vp.camera
cam.isSmoothTransition = False
cam.target = adsk.core.Point3D.create(0, 0, 0)
cam.eye = adsk.core.Point3D.create(0.25, -0.35, 0.45)
cam.upVector = adsk.core.Vector3D.create(0, 0, 1)
cam.isFitView = True
vp.camera = cam
vp.refresh()

result = {{'type': {pkg_type!r}, 'bodies': inv, 'etched': etched, 'stepPath': step_path,
           'manifest': manifest, 'manifestPath': manifest_path,
           'epgDir': epg_dir, 'doc': app.activeDocument.name}}
"""

    res = _proxy_to_addin("run_modeling_script", {"script": script}, timeout=110)
    if not res.get("success"):
        err = (res.get("error") or "")
        if "EPG_NOT_FOUND" in err:
            res["errorCode"] = "epg_not_found"
            res["_hint"] = ("Fusion's built-in ElectronicsPackageGenerator add-in was not found in "
                            "this install (ships with 2024+ Fusion under webdeploy .../Api/"
                            "InternalAddins). Update Fusion, or build the part with "
                            "fusion_make_3d_package from a STEP instead.")
        return res

    data = res.get("data") or {}
    inner = data.get("result") or {}
    return {
        "success": True,
        "output": json.dumps(inner),
        **inner,
        "_hint": ("Generated an IPC-compliant parametric package via Fusion's built-in EPG"
                  + (f", marked '{etch}' on the chip top as {'an engraved cut' if etch_style == 'engraved' else 'RAISED WHITE text (silkscreen-style - high contrast on the dark body)'}" if etch else "")
                  + (f", exported STEP to {output_step}" if output_step else "")
                  + ". NEXT: fusion_screenshot_fusion for a visual check; pull_file the STEP for KiCad. "
                    "HOW THE MARKING WORKS: the top surface is found from the WHOLE-chip bounding box - "
                    "only upward planar faces in the top 5% height band count, largest area wins - and "
                    "the text is laid out with a 10% margin inside that face, auto-fit width-aware "
                    "(override with etchHeightMm). Default style is raised+white for CONTRAST (humans "
                    "can't read a dark-on-dark emboss); pass etchStyle:'engraved' for a sunken laser cut. "
                    "The text ALWAYS runs along the LONGEST axis of the top face (rotated 90deg when "
                    "needed) for maximum room; the 10% margin is ENFORCED by measuring the placed text bbox "
                    "and shrink-to-fitting (see fitIterations in the manifest). Multi-line markings work - join lines with a newline "
                    "(e.g. 'LM358' + newline + 'ADOM-A' for MPN + variant; per-line auto-fit). "
                    "SIDECAR MANIFEST: when outputStep is set, <step>.manifest.json records every "
                    "setting picked + WHY + the calculated bboxes (chip bbox, top band, face extents, "
                    "text box, height, rotation) - to refresh a marking (add a variant line), re-call "
                    "this verb with the manifest's type + paramsCm (unitsCm:true) + the new etch text; "
                    "generation is deterministic so placement reproduces exactly. The manifest is also "
                    "returned inline as 'manifest'. "
                    "PITFALLS: params are mm by default (unitsCm:true for EPG-native cm); dimension names "
                    "follow each EPG generator (chip: D/E/A/L/L1; soic: A/A1/b/D/E/E1/e/L/DPins); raised "
                    "markings are separate 'Marking' bodies (white in Fusion + colored STEP)."),
    }


# ── Optimized-GLB pipeline (service-step2glb "molecule mode") ──────────────────
# A raw STEP->GLB tessellation (fusion_export_step + a plain step2glb convert) leaves
# EVERY solid/pad/via as its own draw call - a real board comes out ~16MB / ~25,000
# primitives that HALTS the viewer's GPU on rotation (John hit this live 2026-07-14).
# service-step2glb's molecule mode is the OCCT-side replacement for Colby's Blender
# molecule-converter: tessellate -> anchor to the MP machine pins -> (optional) bake
# silkscreen -> gold pins -> dedup/flatten/JOIN/weld/prune -> Draco. Same board comes
# out ~465KB / ~31 draw calls, auto-oriented. We call it straight from the bridge so
# a Fusion board becomes a wiki-grade GLB in ONE verb. Reachable from the box (it is a
# public *.adom.cloud host, same as the wiki the bridge already streams from).
STEP2GLB_URL = (os.environ.get("ADOM_STEP2GLB_URL")
                or "https://step2glb-gmdoncpxdwx0.adom.cloud").rstrip("/")


def _multipart_body(fields: dict, files: list):
    """Build a multipart/form-data body. files = [(field, filename, bytes, content_type)]."""
    boundary = "----AdomBridgeGLB%d%d" % (int(_time.time() * 1000), os.getpid())
    out = []
    for k, v in (fields or {}).items():
        out.append(("--" + boundary).encode())
        out.append(('Content-Disposition: form-data; name="%s"' % k).encode())
        out.append(b"")
        out.append(str(v).encode())
    for field, filename, data, ctype in (files or []):
        out.append(("--" + boundary).encode())
        out.append(('Content-Disposition: form-data; name="%s"; filename="%s"'
                    % (field, filename)).encode())
        out.append(("Content-Type: %s" % (ctype or "application/octet-stream")).encode())
        out.append(b"")
        out.append(data)
    out.append(("--" + boundary + "--").encode())
    out.append(b"")
    return b"\r\n".join(out), boundary


# A User-Agent is REQUIRED on every service call. The *.adom.cloud edge WAF 403s the
# default "Python-urllib/x.y" UA (confirmed live 2026-07-14) while a browser/curl UA
# gets 200. Set it on the POST AND both polls or the call fails with an opaque 403.
def _service_ua():
    return "adom-fusion-bridge/%s" % BRIDGE_VERSION


def _service_glb_submit(step_path: str, silk_top: str = None, silk_bottom: str = None,
                        pin: str = "medium", job_name: str = "fusion-board") -> dict:
    """POST a STEP (+ optional silk PNGs) to service-step2glb molecule mode. Returns
    {ok, jobId} - does NOT wait. Pure urllib (no requests on the box)."""
    try:
        with open(step_path, "rb") as f:
            step_bytes = f.read()
    except Exception as e:
        return {"ok": False, "error": "could not read STEP: %s" % e}
    files = [("step", os.path.basename(step_path), step_bytes, "application/step")]
    for field, p in (("silk_top", silk_top), ("silk_bottom", silk_bottom)):
        if p and os.path.exists(p):
            try:
                with open(p, "rb") as f:
                    files.append((field, os.path.basename(p), f.read(), "image/png"))
            except Exception:
                pass
    body, boundary = _multipart_body({}, files)
    headers = {
        "Content-Type": "multipart/form-data; boundary=" + boundary,
        "X-Client": "fusion-bridge/adom",
        "X-Job-Name": job_name,
        "User-Agent": _service_ua(),
    }
    url = "%s/convert?molecule=true&pin=%s" % (STEP2GLB_URL, urllib.parse.quote(pin))
    try:
        req = urllib.request.Request(url, data=body, headers=headers, method="POST")
        with urllib.request.urlopen(req, timeout=90) as resp:
            queued = json.loads(resp.read().decode("utf-8", "replace"))
    except Exception as e:
        return {"ok": False, "error": "service POST failed: %s" % e,
                "_hint": "Is %s reachable from this box? Override with ADOM_STEP2GLB_URL." % STEP2GLB_URL}
    job_id = queued.get("job_id")
    if not job_id:
        return {"ok": False, "error": "service did not return a job_id", "raw": queued}
    return {"ok": True, "jobId": job_id}


def _service_glb_fetch(job_id: str, out_path: str = None, poll_s: int = 0) -> dict:
    """Poll a service job for up to poll_s seconds; if complete, optionally write the
    GLB to out_path. Returns {ok, status, stats, glbBytes?, wrote?}. poll_s=0 = one check."""
    ua = _service_ua()
    stats, deadline, first = {}, _time.time() + max(0, poll_s), True
    while first or _time.time() < deadline:
        first = False
        try:
            preq = urllib.request.Request("%s/jobs/%s" % (STEP2GLB_URL, job_id), headers={"User-Agent": ua})
            with urllib.request.urlopen(preq, timeout=20) as r:
                stats = json.loads(r.read().decode("utf-8", "replace"))
        except Exception:
            _time.sleep(4); continue
        st = stats.get("status")
        if st == "complete":
            break
        if st == "error":
            return {"ok": False, "status": "error", "error": stats.get("error") or stats, "stats": stats}
        if _time.time() >= deadline:
            return {"ok": True, "status": st or "processing", "stats": stats}
        _time.sleep(6)
    if stats.get("status") != "complete":
        return {"ok": True, "status": stats.get("status") or "processing", "stats": stats}
    try:
        rreq = urllib.request.Request("%s/jobs/%s/result" % (STEP2GLB_URL, job_id), headers={"User-Agent": ua})
        with urllib.request.urlopen(rreq, timeout=60) as r:
            glb = r.read()
    except Exception as e:
        return {"ok": False, "status": "complete", "error": "download failed: %s" % e, "stats": stats}
    wrote = None
    if out_path:
        try:
            with open(out_path, "wb") as f:
                f.write(glb)
            wrote = out_path
        except Exception as e:
            return {"ok": False, "status": "complete", "error": "could not write GLB: %s" % e, "stats": stats}
    return {"ok": True, "status": "complete", "stats": stats, "glbBytes": glb, "wrote": wrote}


def _orchestrate_export_optimized_glb(args: dict) -> dict:
    """Fusion board -> wiki-grade optimized GLB in one call. Exports the active design's
    STEP, (optionally) the top/bottom silkscreen, runs it through service-step2glb's
    molecule pipeline (anchor + optional silk bake + gold pins + join/weld/prune + Draco),
    and writes the small, fast, auto-anchored GLB to outputPath. See STEP2GLB_URL above."""
    output_path = args.get("outputPath") or args.get("output_path")
    if not output_path:
        return {"success": False, "error": "No outputPath specified.",
                "_hint": 'Usage: fusion_export_optimized_glb {"outputPath":"C:/tmp/board.glb", "silkscreen":true, "pin":"medium"}'}
    if not output_path.lower().endswith(".glb"):
        output_path = output_path + ".glb"
    pin = args.get("pin", "medium")
    want_silk = args.get("silkscreen", True)
    # 1) Need a 3D Design product for STEP. Switch to the 3D board (no-op if already 3D).
    _proxy_to_addin("show_3d_board", {}, timeout=60)
    # 2) Export STEP next to the target GLB.
    step_path = output_path[:-4] + ".step"
    step_res = _proxy_to_addin("export_step", {"outputPath": step_path}, timeout=300)
    if not step_res.get("success"):
        return {"success": False, "error": "STEP export failed: %s" % (step_res.get("error") or step_res.get("message")),
                "step": step_res, "_hint": "The active design must be a 3D Design product (fusion_show_3d_board first)."}
    # 3) Optional silkscreen (best-effort; the GLB is great without it).
    silk_top = silk_bottom = None
    silk_note = None
    if want_silk:
        st = output_path[:-4] + "_silk_top.png"
        sb = output_path[:-4] + "_silk_bottom.png"
        rt = _proxy_to_addin("take_silkscreen_screenshot", {"outputPath": st, "layer": "top"}, timeout=90)
        rb = _proxy_to_addin("take_silkscreen_screenshot", {"outputPath": sb, "layer": "bottom"}, timeout=90)
        if rt.get("success"):
            silk_top = st
        if rb.get("success"):
            silk_bottom = sb
        if not (silk_top or silk_bottom):
            silk_note = "silkscreen capture unavailable on this design; GLB built without a baked silk texture."
    # 4) Submit to the molecule optimizer (fire-and-return: a big board's tessellation
    #    can run minutes, longer than the ab relay's request timeout, so we do NOT block
    #    the whole time here). Then bounded-wait up to `wait` seconds (default 90) so
    #    small boards still come back complete in one call.
    job_name = os.path.splitext(os.path.basename(output_path))[0]
    sub = _service_glb_submit(step_path, silk_top, silk_bottom, pin=pin, job_name=job_name)
    if not sub.get("ok"):
        return {"success": False, "error": sub.get("error"), "_hint": sub.get("_hint"),
                "stepPath": step_path, "note": "STEP exported OK; optimize submit failed."}
    job_id = sub["jobId"]
    wait_s = int(args.get("wait", 15))  # keep total (STEP export + wait) under the ~60s relay timeout
    fetched = _service_glb_fetch(job_id, out_path=output_path, poll_s=wait_s) if wait_s > 0 else {"ok": True, "status": "processing"}
    status_url = "%s/jobs/%s" % (STEP2GLB_URL, job_id)
    result_url = "%s/jobs/%s/result" % (STEP2GLB_URL, job_id)
    if fetched.get("status") == "complete" and fetched.get("wrote"):
        stats = fetched.get("stats") or {}
        size = len(fetched.get("glbBytes") or b"")
        return {
            "success": True, "status": "complete",
            "glbPath": output_path, "stepPath": step_path, "jobId": job_id,
            "silkTop": silk_top, "silkBottom": silk_bottom,
            "meshesBefore": stats.get("meshes_before"), "meshesAfter": stats.get("meshes_after"),
            "sizeBytes": size, "moleculeAnchored": stats.get("molecule_anchored"),
            "silkscreenApplied": stats.get("silkscreen_applied"), "note": silk_note,
            "message": "Optimized GLB written (%d KB, meshes %s->%s, anchored=%s, silk=%s) to %s" % (
                size // 1024, stats.get("meshes_before"), stats.get("meshes_after"),
                stats.get("molecule_anchored"), stats.get("silkscreen_applied"), output_path),
            "_hint": "Pull it with pull_file, then set it as component.parts.model_3d on a wiki component page. "
                     "Same optimizer as the molecule GLBs (Colby's pipeline).",
        }
    if not fetched.get("ok"):
        return {"success": False, "error": fetched.get("error"), "jobId": job_id, "stepPath": step_path,
                "statusUrl": status_url, "resultUrl": result_url}
    # Still processing after the bounded wait - hand back the job so the caller finishes it.
    return {
        "success": True, "status": fetched.get("status") or "processing",
        "pending": True, "jobId": job_id, "stepPath": step_path,
        "glbPath": output_path, "silkTop": silk_top, "silkBottom": silk_bottom, "note": silk_note,
        "statusUrl": status_url, "resultUrl": result_url,
        "message": "Optimize job %s submitted; still processing after %ds. Finish it with "
                   "fusion_fetch_optimized_glb {\"jobId\":\"%s\",\"outputPath\":\"%s\"} (re-call until complete)." % (
                       job_id, wait_s, job_id, output_path),
        "_hint": "Big boards tessellate for a few minutes. Either re-call fusion_fetch_optimized_glb "
                 "with this jobId (writes the GLB on the box when ready), or GET the resultUrl directly.",
    }


def _orchestrate_fetch_optimized_glb(args: dict) -> dict:
    """Fetch a previously-submitted optimize job's GLB (from fusion_export_optimized_glb's
    jobId). Bounded poll (default 90s); writes to outputPath when complete."""
    job_id = args.get("jobId")
    if not job_id:
        return {"success": False, "error": "No jobId.",
                "_hint": 'Usage: fusion_fetch_optimized_glb {"jobId":"...","outputPath":"C:/tmp/board.glb"}'}
    out_path = args.get("outputPath") or args.get("output_path")
    if out_path and not out_path.lower().endswith(".glb"):
        out_path = out_path + ".glb"
    wait_s = int(args.get("wait", 15))  # keep total (STEP export + wait) under the ~60s relay timeout
    res = _service_glb_fetch(job_id, out_path=out_path, poll_s=wait_s)
    status_url = "%s/jobs/%s" % (STEP2GLB_URL, job_id)
    result_url = "%s/jobs/%s/result" % (STEP2GLB_URL, job_id)
    if res.get("status") == "complete" and res.get("wrote"):
        stats = res.get("stats") or {}
        size = len(res.get("glbBytes") or b"")
        return {"success": True, "status": "complete", "glbPath": out_path, "jobId": job_id,
                "sizeBytes": size, "meshesAfter": stats.get("meshes_after"),
                "moleculeAnchored": stats.get("molecule_anchored"), "silkscreenApplied": stats.get("silkscreen_applied"),
                "message": "Optimized GLB written (%d KB) to %s" % (size // 1024, out_path)}
    if not res.get("ok"):
        return {"success": False, "error": res.get("error"), "jobId": job_id, "statusUrl": status_url, "resultUrl": result_url}
    return {"success": True, "status": res.get("status") or "processing", "pending": True, "jobId": job_id,
            "statusUrl": status_url, "resultUrl": result_url,
            "message": "Job %s still %s; re-call fusion_fetch_optimized_glb to finish." % (job_id, res.get("status") or "processing")}


def _describe_profile(p: dict) -> str:
    """Human, UNAMBIGUOUS name for a browser profile - never just 'your browser'.

    A power user has several (personal / work / media), so a demo that says "it's in your
    Chrome" is useless (John, 2026-07-20). Produce e.g.
    "Chrome - John Personal (jlauer12@gmail.com)" or "Edge - Default"."""
    browser = (p.get("browser") or "browser").strip()
    browser = {"chrome": "Chrome", "edge": "Edge", "brave": "Brave"}.get(browser.lower(), browser.title())
    name = (p.get("displayName") or "").strip()
    email = (p.get("email") or "").strip()
    bits = browser
    if name and email and name.lower() not in email.lower():
        bits += " - %s (%s)" % (name, email)
    elif email:
        bits += " - %s" % email
    elif name:
        bits += " - %s" % name
    elif p.get("profileDir"):
        bits += " - %s" % p.get("profileDir")
    return bits


def _ad_direct_api_call(verb: str, args: dict, timeout: float = 60) -> dict | None:
    """Call an ab verb over the loopback DIRECT API - the correct, non-foregrounding way for a
    bridge to reach ab when the in-process ab_client is down (issue #295). ab spawns the bridge
    with ADOM_DIRECT_API_URL in its env (fallback: ~/.adom/direct-api-port); POST
    {"command", "args"} to <base>/command. Returns the parsed result dict, or None if the direct
    API is unavailable or errors (so _ab_call can fall through). Never shells the GUI exe."""
    base = (os.environ.get("ADOM_DIRECT_API_URL") or "").strip()
    if not base:
        try:
            with open(os.path.join(os.path.expanduser("~"), ".adom", "direct-api-port")) as f:
                port = f.read().strip()
            if port:
                base = "http://127.0.0.1:%s" % port
        except Exception:
            return None
    if not base:
        return None
    try:
        body = json.dumps({"command": verb, "args": args}).encode("utf-8")
        headers = {"Content-Type": "application/json"}
        # The bridge token is what keeps a bridge's own callbacks exempt from ab's identity gate
        # (ab_client already sends it; this path used to omit it).
        tok = os.environ.get("ADOM_BRIDGE_TOKEN") or ""
        if tok:
            headers["X-Adom-Bridge-Token"] = tok
        # Echo the identity ab handed us, then name OURSELVES as the delegate (issue #348). The
        # thread alone would hide who ran it; the bridge alone would hide who asked. ab needs both
        # to render "chip-fetcher tab 3 (via fusion)". Delegate is ADDED, never forwarded.
        ident = _get_caller_identity()
        for key, hdr in (("thread", "X-Adom-Caller-Thread"),
                         ("container", "X-Adom-Caller-Container"),
                         ("reason", "X-Adom-Caller-Reason")):
            if ident.get(key):
                headers[hdr] = ident[key]
        headers["X-Adom-Caller-Delegate"] = "fusion"
        req = urllib.request.Request(base.rstrip("/") + "/command", data=body,
                                     headers=headers, method="POST")
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            r = json.loads(resp.read())
        if isinstance(r, dict):
            inner = r.get("output")
            if isinstance(inner, str) and inner.strip().startswith("{"):
                try:
                    return json.loads(inner)
                except Exception:
                    return r
            return r
    except Exception:
        return None
    return None


def _ab_call(verb: str, args: dict, timeout: int = 60) -> dict:
    """Call another ab verb (nbrowser_*, desktop_*) from inside this bridge.

    Prefers the in-process ab_client; then the loopback DIRECT API (issue #295); only as a last
    resort shells the host app's **-cli** exe (adom-bridge-cli.exe, NEVER the GUI adom-bridge.exe,
    which would foreground ab). Works on an unattended VM where ab_client is down. Never raises -
    returns {} on failure so callers can degrade to instructing the AI instead.

    Attribution (issue #348) is stamped into args HERE, once, so it survives on ALL THREE paths -
    the in-process client and the CLI subprocess cannot carry HTTP headers, and an explicit caller
    block in args is the highest-precedence form ab accepts. The direct-API path additionally sends
    the X-Adom-Caller-* headers so ab can record the delegation chain."""
    args = _stamp_caller(args)
    why = []
    try:
        if ab_client.available():
            r = ab_client.call(verb, args, timeout=timeout)
            if isinstance(r, dict):
                inner = r.get("output")
                if isinstance(inner, str) and inner.strip().startswith("{"):
                    import json as _j0
                    try:
                        return _j0.loads(inner)
                    except Exception:
                        return r
                return r
            why.append("ab_client.call returned %r" % type(r).__name__)
        else:
            why.append("ab_client unavailable")
    except Exception as e:
        why.append("ab_client raised %s" % e)
    # #295: prefer the loopback DIRECT API - it reaches ab without foregrounding anything.
    direct = _ad_direct_api_call(verb, args, timeout=timeout)
    if isinstance(direct, dict):
        return direct
    why.append("direct-api unavailable")
    try:
        import subprocess as _sp, json as _j
        exe = _find_adom_desktop_cli()  # the -cli exe only; never the GUI exe (#295)
        if not exe:
            return {"_adCallError": "; ".join(why + ["adom-bridge CLI not found"])}
        p = _sp.run([exe, verb, _j.dumps(args)], capture_output=True, text=True, timeout=timeout)
        out = (p.stdout or "").strip()
        if out.startswith("{"):
            d = _j.loads(out)
            inner = d.get("output")
            if isinstance(inner, str) and inner.strip().startswith("{"):
                try:
                    return _j.loads(inner)
                except Exception:
                    return d
            return d
        why.append("cli stdout not json: %s" % (out[:120] or (p.stderr or "")[:120]))
    except Exception as e:
        why.append("cli raised %s" % e)
    return {"_adCallError": "; ".join(why)}


# ── FUSION AUTO-UPDATE (John, 2026-07-22) ────────────────────────────────────────────────────
# "fusion ships updates non-stop and its annoying to click this. i just always want the latest
# fusion... just make this generally invisible to me and do that by default for all other adom
# users. but let them tell you they don't want you doing that and you make that a sticky setting."
#
# So: ON BY DEFAULT for everyone, applied silently in the BACKGROUND, opt-out is sticky per user.
# Fusion's own updater downloads in the background and swaps the build on the next launch, so all
# we have to do is stop making the human click "Update Now" on a nag panel.
def _bridge_prefs_path():
    import pathlib
    d = pathlib.Path(os.path.expanduser("~")) / ".adom" / "fusion-bridge"
    d.mkdir(parents=True, exist_ok=True)
    return d / "prefs.json"


def _get_bridge_pref(key: str, default=None):
    try:
        import json as _j
        p = _bridge_prefs_path()
        if p.exists():
            v = (_j.loads(p.read_text()) or {})
            return v.get(key, default)
    except Exception:
        pass
    return default


def _set_bridge_pref(key: str, value) -> bool:
    try:
        import json as _j
        p = _bridge_prefs_path()
        cur = {}
        if p.exists():
            try: cur = _j.loads(p.read_text()) or {}
            except Exception: cur = {}
        cur[key] = value
        p.write_text(_j.dumps(cur, indent=2))
        return True
    except Exception:
        return False


def _auto_update_enabled() -> bool:
    """Default TRUE. Only a user who explicitly opted out gets False, and that sticks."""
    return bool(_get_bridge_pref("autoUpdateFusion", True))


# Buttons Fusion puts on its update nag / Job Status panel, best-first.
# ⚠ TWO CLASSES (#50, 2026-08-23 - 9/9 chains lost Fusion mid-PROVE): "Update Now" only
# STARTS a background download (safe anytime), but "Install Now"/"Restart Now" EXIT Fusion
# to apply the update. The sweep used to click all four blindly on its 300s cadence, which
# lines up exactly with the observed clean exit at +344s into a chain. Exit-class buttons
# may now only be clicked when Fusion is verifiably IDLE (no long op, no recent driving,
# no open documents).
_UPDATE_BUTTONS = ("Update Now", "Update now", "Install Now", "Restart Now")
_UPDATE_BUTTONS_EXIT_CLASS = ("Install Now", "Restart Now")


def _bridge_action_ledger(action: str, detail: dict):
    """Append one background-action record to ~/.adom/fusion-bridge/actions.jsonl.

    The bridge log truncates on every respawn (found investigating #50: a morning of
    evidence was gone by the time the report landed), so anything the bridge does to
    Fusion BEHIND the user's back - update clicks, seat takeovers, dialog dismissals,
    doc closes, hang-gate kills - is also written here, append-only, survives respawns.
    One grep answers 'did the bridge touch Fusion at HH:MM?'. Never raises."""
    try:
        import json as _j, time as _t, pathlib as _pl
        p = _pl.Path(os.path.expanduser("~")) / ".adom" / "fusion-bridge" / "actions.jsonl"
        p.parent.mkdir(parents=True, exist_ok=True)
        rec = {"at": _t.strftime("%Y-%m-%d %H:%M:%S"), "epoch": round(_t.time(), 1),
               "action": action, "pid": os.getpid()}
        rec.update(detail or {})
        with open(p, "a", encoding="utf-8") as f:
            f.write(_j.dumps(rec) + "\n")
    except Exception:
        pass


try:
    import handlers.dismiss_recovery as _dr_mod
    _dr_mod.LEDGER = _bridge_action_ledger     # every recovery WM_CLOSE / Escape is recorded
except Exception:
    pass


def _host_op_active() -> bool:
    """THE shared invariant (#52 item 3): is a long op or recent driving in flight? Every
    BACKGROUND subsystem that would act ON the host app (synthetic input, window closes,
    update clicks) must stand down while this is true - so subsystem number four is born
    safe instead of being diagnosed in a six-hour thread. Seat takeover is the one
    documented exemption: the seat dialog BLOCKS the very op we are protecting."""
    import time as _t
    if _LONG_OP.get("name"):
        return True
    return bool(_LAST_DRIVE_AT[0] and (_t.time() - _LAST_DRIVE_AT[0]) < 180)


def _fusion_verifiably_idle() -> dict:
    """May the bridge do something that EXITS Fusion right now? Only when ALL of:
    no registered long op, no driving verb for 3+ minutes, and no open documents
    (best-effort; an unreachable add-in counts as NOT idle - safe default)."""
    import time as _t
    if _LONG_OP.get("name"):
        return {"idle": False, "why": "long op in flight: %s" % _LONG_OP["name"]}
    drove_ago = _t.time() - float(_LAST_DRIVE_AT[0] or 0)
    if _LAST_DRIVE_AT[0] and drove_ago < 180:
        return {"idle": False, "why": "driving verb %ds ago" % int(drove_ago)}
    try:
        st = _proxy_to_addin("document_info", {}, timeout=6)
        docs = (st.get("data") or {}).get("openDocuments") or []
        if not st.get("success"):
            return {"idle": False, "why": "add-in unreachable - cannot prove no open docs"}
        if docs:
            return {"idle": False, "why": "%d document(s) open" % len(docs)}
    except Exception:
        return {"idle": False, "why": "doc check failed - cannot prove idle"}
    return {"idle": True, "why": "no long op, no recent driving, no open documents"}


def _find_controls_any(obj, depth=0):
    """Envelope-proof extraction of UIA find_control results: ab has moved best/controls
    across response levels between versions (same drift family as coordMap). Returns
    {best, controls} from anywhere in the response, parsing stringified output."""
    if depth > 5:
        return {}
    if isinstance(obj, str) and obj[:1] in "[{":
        try:
            import json as _j
            return _find_controls_any(_j.loads(obj), depth + 1)
        except Exception:
            return {}
    if isinstance(obj, dict):
        if isinstance(obj.get("best"), dict) or isinstance(obj.get("controls"), list):
            return {"best": obj.get("best") or {}, "controls": obj.get("controls") or []}
        for k in ("data", "output", "result", "response"):
            got = _find_controls_any(obj.get(k), depth + 1)
            if got:
                return got
    return {}


_LAST_UPDATE_OFFER = {"offered": False, "at": 0}


def _find_update_offer(hwnd=None) -> dict:
    """Is Fusion offering an update right now? Detected via UIA (background, no foreground).
    Returns {offered, button, hwnd, detail} - never raises."""
    try:
        hw = hwnd or _fusion_window_hwnd()
        if not hw:
            return {"offered": False}
        # Scan the main window AND Fusion's owned popups - the update nag lives in the
        # "View Job Status" panel, which is a CHILD window and is only in the UIA tree while it
        # is open. Checking both is why this runs on a schedule rather than once.
        candidates = [int(hw)]
        try:
            for w in (family_windows(int(hw)) or []):
                h2 = w.get("hwnd") if isinstance(w, dict) else w
                if h2 and int(h2) != int(hw):
                    candidates.append(int(h2))
        except Exception:
            pass
        for h in candidates:
            for label in _UPDATE_BUTTONS:
                r = _find_controls_any(_ab_call("desktop_find_control", {"hwnd": h, "name": label}, timeout=20))
                best = r.get("best") or {}
                if best.get("name") and best.get("invokable"):
                    import time as _ut
                    _LAST_UPDATE_OFFER.update(offered=True, at=_ut.time(), button=best.get("name"))
                    return {"offered": True, "button": best.get("name"), "hwnd": h,
                            "detail": "Fusion is offering an update ('%s' is on screen)" % best.get("name")}
        return {"offered": False, "hwnd": hw}
    except Exception:
        return {"offered": False}


def _apply_fusion_update_silently(hwnd=None) -> dict:
    """Click Fusion's update button IN THE BACKGROUND so the download starts and the new build
    is applied on the next launch. No foreground, no caption - the user asked for this to be
    invisible. Returns {applied, button, why}."""
    if not _auto_update_enabled():
        return {"applied": False, "why": "user opted out (sticky pref autoUpdateFusion=false)"}
    offer = _find_update_offer(hwnd)
    if not offer.get("offered"):
        return {"applied": False, "why": "no update offered"}
    # ⚠ #50 gate: "Install Now"/"Restart Now" EXIT Fusion to apply the update. Clicking one
    # while a chain is running killed Fusion mid-PROVE 9/9 times (clean exit, no crash
    # artifact, matching this sweep's 300s cadence). Exit-class buttons only fire when
    # Fusion is verifiably idle; otherwise defer - the janitor retries every 300s and will
    # land it the moment the work is done. "Update Now" (download only) stays safe anytime.
    if offer["button"] in _UPDATE_BUTTONS_EXIT_CLASS:
        idle = _fusion_verifiably_idle()
        if not idle.get("idle"):
            _bridge_action_ledger("auto_update_deferred",
                                  {"button": offer["button"], "notIdle": idle.get("why")})
            print("[Fusion Bridge] auto-update: %r on screen but Fusion is NOT idle (%s) - "
                  "DEFERRED, will retry when idle" % (offer["button"], idle.get("why")))
            return {"applied": False, "deferred": True, "button": offer["button"],
                    "why": "exit-class update button deferred: %s" % idle.get("why")}
    r = _unwrap(_ab_call("desktop_ui_click",
                         {"hwnd": int(offer["hwnd"]), "name": offer["button"]}, timeout=30))
    ok = bool(r.get("invoked") or r.get("success"))
    if ok:
        print("[Fusion Bridge] auto-update: clicked %r in the background" % offer["button"])
    _bridge_action_ledger("auto_update_click",
                          {"button": offer["button"], "invoked": ok,
                           "exitClass": offer["button"] in _UPDATE_BUTTONS_EXIT_CLASS})
    return {"applied": ok, "button": offer["button"],
            "why": ("clicked %r in the background; Fusion downloads now and swaps on next launch"
                    % offer["button"]) if ok else "found the button but the UIA invoke did not take"}


def _handle_set_auto_update(fusion_info: dict, args: dict) -> dict:
    """Turn Fusion auto-updating on/off. STICKY - remembered for this user forever."""
    if "enabled" not in (args or {}):
        cur = _auto_update_enabled()
        return {"success": True, "autoUpdateFusion": cur,
                "_hint": ("Fusion auto-update is currently %s. It is ON BY DEFAULT: the bridge "
                          "clicks Fusion's 'Update Now' for the user in the BACKGROUND so they "
                          "always run the latest build and never see the nag. To opt out: "
                          "fusion_set_auto_update {\"enabled\": false} - that choice is STICKY."
                          % ("ON" if cur else "OFF")),
                "statusVerb": "fusion_readiness"}
    enabled = bool(args.get("enabled"))
    _set_bridge_pref("autoUpdateFusion", enabled)
    return {"success": True, "autoUpdateFusion": enabled,
            "narrate": ("I'll keep Fusion updated automatically in the background."
                        if enabled else
                        "I'll stop auto-updating Fusion. You'll see Autodesk's own update prompts."),
            "_hint": ("Saved and sticky (~/.adom/fusion-bridge/prefs.json). %s"
                      % ("Auto-update ON: the bridge silently clicks 'Update Now' whenever Fusion "
                         "offers one, so the user always runs the newest build. Tell them it is "
                         "handled and they never need to click it."
                         if enabled else
                         "Auto-update OFF: the bridge will NOT touch update prompts; the user "
                         "handles Autodesk's nag themselves. Do not override this.")),
            "statusVerb": "fusion_readiness"}


# Registered here, NOT in the COMMAND_HANDLERS literal: that dict is built at line ~1699,
# long before this function exists, so naming it there raised NameError at import and
# crash-looped the whole bridge (done exactly that, 2026-07-22).
COMMAND_HANDLERS["set_auto_update"] = _handle_set_auto_update


# ── SAVE-PROMPT SENTINEL (issue #30, 2026-08-16) ─────────────────────────────────────────────
# ab's live demo run proved that Fusion's unsaved-doc "Save / Don't Save / Cancel" modal,
# popped mid document-switch by our own show_* verbs, orphans the in-flight add-in call and
# can wedge the add-in main thread PERMANENTLY (only fusion_close {force} + fusion_start
# recovers). The show verbs themselves create the modified-doc state, so for BRIDGE-CREATED
# documents discarding is always correct: while a doc-switching command runs, a watcher
# scans for the prompt and clicks "Don't Save" IN THE BACKGROUND (UIA invoke, no focus).
#
# ⚠ DATA-SAFETY RULE: we only auto-discard when the dialog text names a document the bridge
# itself created this session (_BRIDGE_DOCS). A user's own unsaved design NEVER gets its
# Save prompt auto-answered - that would destroy their work. Unrecognized prompts are left
# standing and classified as before (classify_blocking_dialogs -> save_changes hint).

_BRIDGE_DOCS: set = set()          # lowercase doc-name fragments the bridge created
_UNATTRIBUTED_PROMPT_STRIKES: dict = {}   # hwnd -> consecutive sightings without attribution
_SIZE_ATTEMPTS: dict = {}                # hwnd -> sizing attempts (Fusion restores its OWN saved
                                         # geometry LATE in startup and can override an early
                                         # resize, so we retry until the window is confirmed
                                         # large, then stop touching it - John's live trace
                                         # 2026-08-18: sized to 92% at t+75s, Fusion re-restored
                                         # itself at t+90s.)
_NO_DONTSAVE_HWNDS: set = set()           # hwnds probed once with NO Don't-Save button (UIA-less)
_DONT_SAVE_LABELS = ("Don't Save", "Don’t Save")
# Names the bridge generates by CONVENTION - always ours, even after a respawn wiped the
# session registry (caught live 2026-08-16: respawn emptied _BRIDGE_DOCS and the sentinel
# refused its own ADOM-TEST doc's prompt, wedging two tours in a row).
_BRIDGE_DOC_BUILTINS = ("adom-test-", "adomdemosample", "adom-fusion-wrap",
                        # the tour's own bundled sample parts - bridge docs by definition
                        # (their Save prompt failed a third tour, 2026-08-16)
                        "ads8588sipm", "0603waf")


_SESSION_DOCS_AT_START: set = set()   # documents open when this bridge process started (the user's)


def _user_unsaved_docs(unsaved_names) -> list:
    """The unsaved documents that are the USER'S: not registered as bridge-opened, not a
    generator/scratch design that appeared after the bridge started. These are the only
    documents a restart may legitimately ask about."""
    out = []
    for n in (unsaved_names or []):
        raw = str(n or "")
        bare = raw.replace("*", "").split(" (")[0].strip().lower()
        if not bare:
            continue
        if _is_bridge_doc_text(bare):
            continue
        # John, 2026-08-30: "all your stale untitleds ... don't assume they're mine". Fusion
        # and the bridge's own scripts mint Untitled and *_3d documents; John names his work.
        if bare.startswith("untitled") or bare.endswith("_3d"):
            continue
        out.append(raw)
    return out


def _note_bridge_doc(name: str):
    """Register a document/library name as BRIDGE-CREATED (safe to auto-discard).
    Write-through to prefs.json so the registry SURVIVES bridge respawns."""
    n = str(name or "").rsplit("/", 1)[-1].rsplit("\\", 1)[-1]
    n = n.rsplit(".", 1)[0].strip().lower()
    if len(n) >= 3:
        _BRIDGE_DOCS.add(n)
        while len(_BRIDGE_DOCS) > 24:
            _BRIDGE_DOCS.pop()
        try:
            _set_bridge_pref("bridgeDocs", sorted(_BRIDGE_DOCS))
        except Exception:
            pass


try:  # reload the persisted registry at import (respawn survival)
    _BRIDGE_DOCS.update(x for x in (_get_bridge_pref("bridgeDocs", []) or [])
                        if isinstance(x, str) and len(x) >= 3)
except Exception:
    pass


def _is_bridge_doc_text(text: str):
    t = (text or "").lower()
    return (next((n for n in _BRIDGE_DOCS if n in t), None)
            or next((p for p in _BRIDGE_DOC_BUILTINS if p in t), None))


def _find_save_prompt() -> dict:
    """Is Fusion's unsaved-doc Save prompt up? Two shapes seen live: a CEF modal with a
    UIA-reachable "Don't Save" button, and a bare Qt dialog titled just "Save" with NO
    UIA tree at all (ConfRoomROG, 2026-08-16 - it blocked the main thread invisibly).
    Returns {found, hwnd, button|None, text} - never raises."""
    try:
        info = get_fusion_window_info() or {}
        for d in info.get("dialogs") or []:
            h = d.get("hwnd")
            if not h:
                continue
            title = str(d.get("title") or "").strip().lower()
            for label in (() if h in _NO_DONTSAVE_HWNDS else _DONT_SAVE_LABELS):
                r = _find_controls_any(_ab_call("desktop_find_control",
                                                {"hwnd": int(h), "name": label}, timeout=10))
                best = r.get("best") or {}
                if best.get("name") and best.get("invokable"):
                    text = ""
                    try:
                        t = _find_controls_any(_ab_call("desktop_find_control",
                                                        {"hwnd": int(h), "name": "save"}, timeout=10))
                        for c in ([t.get("best") or {}] + list(t.get("controls") or [])):
                            nm = str(c.get("name") or "")
                            if len(nm) > len(text):
                                text = nm
                    except Exception:
                        pass
                    return {"found": True, "hwnd": int(h), "button": best.get("name"), "text": text}
            if title in ("save", "save changes", "unsaved changes"):
                # UIA-less Qt variant: identified by title alone; no clickable button handle.
                _NO_DONTSAVE_HWNDS.add(h)   # never pay the 2x10s UIA probe for this hwnd again
                return {"found": True, "hwnd": int(h), "button": None, "text": ""}
    except Exception:
        pass
    return {"found": False}



def _close_idle_bridge_docs() -> dict:
    """Close BRIDGE-created documents that are sitting DIRTY while nothing is driving.

    John, 2026-08-20: "why are you always leaving annoying dialog boxes up like this?"
    Dismissing the Save prompt was only ever treating the symptom. The prompt exists
    because our surfaces/tour runs leave the staged document MODIFIED and open, so the
    next thing that tries to close it asks the user to decide about OUR throwaway edits.
    A prompt we have to dismiss is a mess we should not have made.

    Only touches documents in the bridge-doc registry, only while no driving verb has run
    for a few minutes (so a demo in progress is never yanked out from under itself), and
    always with save:false - these are our scratch edits, never the user's work.
    """
    import time as _t
    try:
        idle_for = _t.time() - float(_LAST_DRIVE_AT[0] or 0)
    except Exception:
        idle_for = 1e9
    if idle_for < 180:
        return {"closed": [], "why": "driving activity within 3 minutes - leaving documents alone"}
    try:
        st = _proxy_to_addin("document_info", {}, timeout=6)
    except Exception:
        return {"closed": [], "why": "add-in unreachable"}
    if not st.get("success"):
        return {"closed": [], "why": "no document info"}
    docs = (st.get("data") or {}).get("openDocuments") or []
    closed = []
    for d in docs:
        name = str(d.get("name") or "")
        dirty = bool(d.get("isModified") or d.get("modified") or name.endswith("*"))
        if not name or not dirty:
            continue
        if not _is_bridge_doc_text(name):
            continue          # the user's document - never ours to close
        try:
            r = _close_bridge_doc_verified(name)
            if r.get("closed"):
                closed.append(name)
                print("[Fusion Bridge] janitor: closed dirty bridge doc %r (verified gone, "
                      "any Save-As answered in the same call)" % name)
                _bridge_action_ledger("idle_doc_close", {"doc": name})
        except Exception:
            pass
    return {"closed": closed, "why": "closed %d dirty bridge doc(s)" % len(closed)}


def _dismiss_save_prompt_if_bridge_doc() -> dict:
    """Auto-dismiss the Save prompt ONLY for bridge-created docs. Ladder:
    1. Prompt text names a bridge doc -> UIA-click "Don't Save" (background, discards OUR
       throwaway edits). 2. Text unreadable (UIA-less Qt variant) but the MAIN WINDOW
       title names a bridge doc -> post ESCAPE (= Cancel: non-destructive for ANY doc,
       just unblocks the main thread; the doc stays open unsaved). UIA click falling
       back to Escape also covers ab's per-command permission gate blocking desktop_ui_click.
    A prompt for a USER's document is never touched. Returns {handled, why} - never raises."""
    prompt = _find_save_prompt()
    if not prompt.get("found"):
        return {"handled": False, "why": "no save prompt"}
    text = prompt.get("text") or ""
    matched = _is_bridge_doc_text(text)
    main_title = ""
    if not matched:
        try:
            main_title = str((get_fusion_window_info() or {}).get("title") or "")
        except Exception:
            pass
        matched = _is_bridge_doc_text(main_title)
    if not matched:
        # UNATTRIBUTABLE prompt (the spawning doc already closed, so no title names it -
        # exactly how a standing prompt looks after a close-flow; caught live 2026-08-17).
        # Escape = CANCEL: non-destructive for ANYONE's document (the close is cancelled,
        # the doc stays open, nothing saved or discarded). A modal that blocks the add-in
        # main thread for 2+ sweeps gets escaped rather than sat on; Don't Save is still
        # NEVER clicked without attribution.
        # ZOMBIE GATE (2026-08-17, found live on ConfRoomROG): "Save"-titled windows can be
        # non-modal GHOSTS that block nothing (main thread fully responsive, PrintWindow
        # black, no UIA). Escaping those every other tick is pointless input spam - only
        # escalate on an unattributed prompt when the add-in main thread is ACTUALLY
        # blocked (status probe busy or unanswerable).
        try:
            _st = _check_addin_status(timeout=2.0)
            _blocked = (_st is None) or bool(_st.get("busy"))
        except Exception:
            _blocked = True
        if not _blocked:
            return {"handled": False,
                    "why": "unattributed dialog but the main thread is responsive - zombie window, leaving it alone",
                    "hwnd": prompt["hwnd"]}
        n = _UNATTRIBUTED_PROMPT_STRIKES.get(prompt["hwnd"], 0) + 1
        _UNATTRIBUTED_PROMPT_STRIKES[prompt["hwnd"]] = n
        if n >= 2:
            try:
                esc = send_key_background("escape", hwnd=prompt["hwnd"])
                if esc.get("success"):
                    print("[Fusion Bridge] save-prompt sentinel: ESCAPED an unattributed prompt "
                          "after %d sightings (Cancel - non-destructive, main thread freed)" % n)
                    _UNATTRIBUTED_PROMPT_STRIKES.pop(prompt["hwnd"], None)
                    return {"handled": True, "why": "escaped unattributed prompt (Cancel) after %d sightings" % n}
            except Exception:
                pass
        return {"handled": False, "why": "unattributed save prompt - strike %d of 2 before Escape" % n,
                "hwnd": prompt["hwnd"]}
    if prompt.get("button"):
        r = _unwrap(_ab_call("desktop_ui_click",
                             {"hwnd": prompt["hwnd"], "name": prompt["button"]}, timeout=15))
        if r.get("invoked") or r.get("success"):
            print("[Fusion Bridge] save-prompt sentinel: clicked %r for bridge doc %r (background)"
                  % (prompt["button"], matched))
            return {"handled": True, "why": "clicked '%s' for bridge doc '%s'" % (prompt["button"], matched)}
    # ATTRIBUTED to a bridge doc, so "Don't Save" is the RIGHT answer, not Escape. Escape
    # cancels the close and leaves the doc dirty, which guarantees the same prompt returns -
    # that loop is exactly what John saw twice in a row (2026-08-20). Try the decisive
    # button first; Escape stays as the fallback when UIA is unavailable.
    try:
        r2 = _unwrap(_ab_call("desktop_ui_click",
                              {"hwnd": prompt["hwnd"], "name": "Don't Save"}, timeout=15))
        if r2.get("invoked") or r2.get("success"):
            print("[Fusion Bridge] save-prompt sentinel: clicked \'Don\'t Save\' for bridge doc %r "
                  "(attributed by window title)" % matched)
            _bridge_action_ledger("save_prompt_dismiss", {"doc": matched, "how": "dont_save"})
            return {"handled": True, "why": "clicked Don't Save for bridge doc '%s'" % matched}
    except Exception:
        pass
    # Escape = Cancel: always non-destructive, needs no UIA and no ab input grant.
    try:
        esc = send_key_background("escape", hwnd=prompt["hwnd"])
        if esc.get("success"):
            print("[Fusion Bridge] save-prompt sentinel: ESCAPED the prompt for bridge doc %r "
                  "(Cancel - doc stays open unsaved, main thread freed)" % matched)
            _bridge_action_ledger("save_prompt_dismiss", {"doc": matched, "how": "escape"})
            return {"handled": True, "why": "escaped (Cancel) for bridge doc '%s'" % matched}
    except Exception:
        pass
    return {"handled": False, "why": "dismiss attempts did not land", "hwnd": prompt["hwnd"]}


class _SavePromptSentinel:
    """Context manager: while a doc-switching add-in call runs, poll for the Save prompt
    and auto-dismiss it (bridge docs only) so the call never wedges behind the modal."""

    def __init__(self, max_seconds: float):
        import time as _t
        self.max_seconds = max(5.0, float(max_seconds))
        self._deadline = _t.time() + self.max_seconds
        self._stop = threading.Event()
        self._hits = []

    def soft_stop(self, grace_seconds: float = 12.0):
        """Shrink the watch window instead of killing it: keeps covering the just-returned
        call for a short grace period (the post-timeout prompt is the wedge case)."""
        import time as _t
        self._deadline = min(self._deadline, _t.time() + grace_seconds)

    def __enter__(self):
        def _watch():
            import time as _t
            while not self._stop.is_set() and _t.time() < self._deadline:
                try:
                    info = get_fusion_window_info() or {}
                    if info.get("dialogs"):
                        got = _dismiss_save_prompt_if_bridge_doc()
                        if got.get("handled"):
                            self._hits.append(got)
                except Exception:
                    pass
                self._stop.wait(1.0)
        self._thread = threading.Thread(target=_watch, daemon=True)
        self._thread.start()
        return self

    def __exit__(self, *exc):
        self._stop.set()
        return False


# Add-in commands that switch/open documents and can pop the Save prompt mid-flight.
_DOC_SWITCHING_ADDIN = {
    "open_lbr", "open_cloud_file", "open_by_urn", "open_schematic", "open_board",
    "show_schematic", "show_2d_board", "show_3d_board", "import_electronics",
    "execute_text_command", "electron_run", "new_electronics_from_eagle",
}


def _license_classify_extend(dialogs: list):
    """ab's #30 ask: the Autodesk seat modal ('Active Sessions Exceeded' / 'Suspend Remote
    Session') often carries a bare Win32 title, so the title-substring classifier files it
    as generic_modal - undecidable for a caller. find_licensing_dialog() detects it by
    OWNERSHIP + SHAPE instead, so when it fires, append the decidable category (mutates
    the passed list in place; never raises)."""
    try:
        if any(d.get("category") == "license_sessions_exceeded" for d in dialogs):
            return
        dlg = find_licensing_dialog()
        if not dlg:
            return
        auto = _seat_takeover_enabled()
        dialogs.append({
            "category": "license_sessions_exceeded",
            "title": dlg.get("title") or "",
            "hwnd": dlg.get("hwnd"),
            "resolution": (
                "Autodesk seat modal: this account's Fusion is active on another machine. "
                + ("Auto-takeover is ON - the bridge clicks Continue through both dialogs in the "
                   "background (~20s sweep; a fusion_start resolves it immediately). The other "
                   "machine's Fusion suspends RESUMABLY."
                   if auto else
                   "Auto-takeover is OPTED OUT (fusion_set_seat_takeover) - resolve the dialog "
                   "manually or free the seat with fusion_stop on the other machine.")),
            "autoHandled": auto,
        })
    except Exception:
        pass


def _handle_fusion_state(fusion_info: dict, args: dict) -> dict:
    """fusion_state - the render-me verb (#28; kicad_state is the reference shape).

    SERVER-SIDE ONLY on purpose: it must answer even when the add-in main thread is busy
    or wedged (ab's demo falls back to window_info + desktop screenshots today exactly
    because a state verb has to work in those moments). Returns running/window/dialog
    state + a fresh background screenshot of the Fusion window; activeDocument is
    best-effort via the add-in with a short budget.

    args: screenshot (default true) | imageAs: "path" (default) | "base64" (inline PNG,
    auto-refused over ~4MB - pull the path via read_file instead)."""
    running = _is_fusion_running()
    result = {"success": True, "running": running}
    # THESE TWO SURVIVE A CLOSED FUSION, ON PURPOSE (caught re-testing #82, 2026-08-20).
    # focusEvents is the QUIETNESS AUDIT LOG and timings is on-disk measured history -
    # neither needs the app to be up, and both are most likely to be asked for exactly when
    # it is NOT (after a run, reviewing what the bridge did). They used to sit below the
    # not-running early return, so an auditor closing Fusion lost the evidence.
    try:
        with _FOCUS_LOCK:
            result["focusEvents"] = list(_FOCUS_EVENTS[-20:])
    except Exception:
        result["focusEvents"] = []
    try:
        import timings as _tm0
        result["timings"] = _tm0.summary()
        _s0 = _tm0.sentence()
        if _s0:
            result["timingsSentence"] = _s0
    except Exception:
        pass
    if not running:
        result["_hint"] = ("Fusion is not running - nothing to render. fusion_start launches it "
                           "in the background; fusion_demo {mode:'tour'} cold-starts it itself. "
                           "focusEvents and timings ARE still reported - they do not need Fusion.")
        result["output"] = json.dumps({"running": False})
        return result
    info = get_fusion_window_info() or {}
    result["hwnd"] = info.get("hwnd")
    result["title"] = info.get("title")
    result["rect"] = info.get("rect")
    result["dialogs"] = info.get("dialogs") or []
    try:
        result["blockingDialogs"] = classify_blocking_dialogs() or []
    except Exception:
        result["blockingDialogs"] = []
    _license_classify_extend(result["blockingDialogs"])
    # Busy state (the add-in's own report; 3s budget, never blocks the state read).
    busy = None
    try:
        st = _check_addin_status(timeout=3.0)
        if st:
            result["addinResponding"] = True
            if st.get("busy"):
                busy = {"command": st.get("busyCommand"), "elapsedSeconds": st.get("elapsedSeconds")}
        else:
            result["addinResponding"] = False
    except Exception:
        result["addinResponding"] = False
    result["mainThreadBusy"] = busy
    try:
        with _FOCUS_LOCK:
            result["focusEvents"] = list(_FOCUS_EVENTS[-20:])
    except Exception:
        result["focusEvents"] = []
    # WHAT TO EXPECT, BEFORE THE CLICK (issue #32 section 2): this machine's own measured
    # launch history, so a UI can say "typically 45s cold, 3s warm here" instead of making
    # the user discover it by waiting.
    try:
        import time as _tnow
        import timings as _tm
        result["timings"] = _tm.summary()
        _s = _tm.sentence()
        if _s:
            result["timingsSentence"] = _s
        _lsa = _get_bridge_pref("launchStartedAt", None)
        if _lsa and not result.get("addinResponding") and (_tnow.time() - float(_lsa)) < 900:
            result["progress"] = _tm.progress_block(
                "launch", float(_lsa),
                "Fusion is starting - waiting for the Adom add-in to serve",
                start_kind="cold", fallback=150)
    except Exception:
        pass
    try:
        from handlers.taskbar_overlay import badge_active, last_error as _badge_err
        result["drivingBadge"] = {"active": badge_active(),
                                  "lastDrivingVerbAt": _LAST_DRIVE_AT[0] or None,
                                  "idleShedSeconds": _BADGE_IDLE_SECONDS,
                                  "enabled": _driving_badge_enabled(),
                                  "lastError": _badge_err()}
    except Exception:
        pass
    # Active document, best-effort (skip when busy - it would just be rejected).
    if result.get("addinResponding") and not busy:
        try:
            st = _proxy_to_addin("get_app_state", {}, timeout=5)
            result["activeDocument"] = (st.get("data") or {}).get("activeDocument")
        except Exception:
            pass
    # The screenshot: PrintWindow-based, background, no focus taken.
    if args.get("screenshot", True):
        try:
            shot = screenshot_fusion_window()
            if shot.get("success"):
                result["screenshot"] = shot.get("savedTo")
                if args.get("imageAs") == "base64":
                    import base64 as _b64
                    raw = open(shot["savedTo"], "rb").read()
                    if len(raw) <= 4 * 1024 * 1024:
                        result["screenshotBase64"] = _b64.b64encode(raw).decode("ascii")
                    else:
                        # DOWNSCALE instead of refusing (ralph, 2026-08-18: a PIL-less bridge
                        # saves 24MB BMPs, so every Dashboard surface card came back with no
                        # image at all). Same path the tour gallery uses.
                        small = _shot_b64_small(shot["savedTo"], max_w=1280)
                        if small:
                            result["screenshotBase64"] = small
                            result["screenshotNote"] = ("downscaled to <=1280px (raw capture is "
                                                        "%dMB) - pull result.screenshot for full size"
                                                        % (len(raw) // (1024 * 1024)))
                        else:
                            result["screenshotNote"] = ("capture is %dKB (>4MB inline cap) and could "
                                                        "not be downscaled - pull result.screenshot "
                                                        "via read_file." % (len(raw) // 1024))
        except Exception as e:
            result["screenshotError"] = str(e)
    hints = []
    if busy:
        hints.append("Main thread busy (%s, %ss) - add-in verbs are rejected right now; "
                     "window/screenshot verbs still work." % (busy["command"], busy["elapsedSeconds"]))
    if result["blockingDialogs"]:
        hints.append("A modal is up - see blockingDialogs for the classified cause + resolution.")
    if not hints:
        hints.append("Fusion is clear. The screenshot shows the current surface - this is the verb "
                     "to render any fusion_show_* result (they open in the background by design).")
    result["_hint"] = " ".join(hints)
    result["output"] = json.dumps({"running": True, "activeDocument": result.get("activeDocument"),
                                   "mainThreadBusy": busy, "blockingDialogs": result["blockingDialogs"],
                                   "screenshot": result.get("screenshot")})
    return result


COMMAND_HANDLERS["state"] = _handle_fusion_state


# ── AUTODESK FUSION MCP SERVER PROXY (John, 2026-07-23) ──────────────────────────────────────
# Autodesk + Anthropic shipped a Fusion MCP server: a LOCAL HTTP/JSON-RPC endpoint at
# http://127.0.0.1:27182/mcp that exposes Fusion's own text-to-CAD surface (read geometry,
# execute/update features, read Electronics design data).
#
# WHY THE BRIDGE PROXIES IT: that server binds LOOPBACK on the user's machine. It was designed
# for Claude Desktop running on the same box. An Adom AI runs in a CLOUD container and cannot
# reach the user's 127.0.0.1 at all. This bridge already runs on that machine, so it is exactly
# the right proxy: these verbs give the cloud AI Autodesk's MCP tools without reimplementing them.
#
# The server speaks MCP streamable-HTTP: you must `initialize`, capture the MCP-Session-Id
# response header, send notifications/initialized, and pass that header on every later call.
# Skipping it returns 400 "Missing MCP-Session-Id".
_MCP_URL = "http://127.0.0.1:27182/mcp"
_mcp_session_id = None
_mcp_lock = threading.Lock()


def _mcp_post(body: dict, sid: str = None, timeout: int = 30) -> dict:
    import urllib.request, urllib.error
    h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if sid:
        h["MCP-Session-Id"] = sid
    req = urllib.request.Request(_MCP_URL, method="POST",
                                 data=json.dumps(body).encode(), headers=h)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return {"status": r.status, "headers": dict(r.headers),
                    "body": r.read().decode("utf-8", "replace")}
    except urllib.error.HTTPError as e:
        return {"httpError": e.code, "headers": dict(e.headers),
                "body": e.read().decode("utf-8", "replace")[:1000]}
    except Exception as e:
        return {"error": "%s: %s" % (type(e).__name__, str(e)[:200])}


def _mcp_new_session() -> dict:
    """initialize + notifications/initialized. Returns {sessionId, serverInfo} or {error}."""
    global _mcp_session_id
    r = _mcp_post({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                   "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                              "clientInfo": {"name": "fusion-bridge",
                                             "version": BRIDGE_VERSION}}})
    if r.get("error") or r.get("httpError"):
        return {"error": r.get("error") or ("HTTP %s" % r.get("httpError")), "raw": r.get("body")}
    sid = None
    for k, v in (r.get("headers") or {}).items():
        if k.lower() == "mcp-session-id":
            sid = v
    info = {}
    try:
        info = (json.loads(r.get("body") or "{}").get("result") or {}).get("serverInfo") or {}
    except Exception:
        pass
    if sid:
        _mcp_post({"jsonrpc": "2.0", "method": "notifications/initialized"}, sid)
        _mcp_session_id = sid
    return {"sessionId": sid, "serverInfo": info}


def _mcp_rpc(method: str, params: dict = None, timeout: int = 60) -> dict:
    """One JSON-RPC call with automatic session (re)establishment."""
    global _mcp_session_id
    with _mcp_lock:
        if not _mcp_session_id:
            s = _mcp_new_session()
            if s.get("error"):
                return {"_mcpError": s["error"]}
        sid = _mcp_session_id
    body = {"jsonrpc": "2.0", "id": 7, "method": method}
    if params is not None:
        body["params"] = params
    r = _mcp_post(body, sid, timeout=timeout)
    # a dropped/rotated session shows up as 400 Missing/Invalid session; re-init once
    if r.get("httpError") in (400, 404) and "ession" in str(r.get("body", "")):
        with _mcp_lock:
            _mcp_session_id = None
            s = _mcp_new_session()
            if s.get("error"):
                return {"_mcpError": s["error"]}
            sid = _mcp_session_id
        r = _mcp_post(body, sid, timeout=timeout)
    if r.get("error") or r.get("httpError"):
        return {"_mcpError": r.get("error") or ("HTTP %s: %s" % (r.get("httpError"), r.get("body", "")[:200]))}
    try:
        return json.loads(r.get("body") or "{}")
    except Exception:
        return {"_mcpError": "unparseable response", "raw": (r.get("body") or "")[:400]}


_MCP_OFF_STATIC = (
    "Fusion's MCP server is not reachable on 127.0.0.1:27182. It is OFF by default. The bridge "
    "can turn it on FOR the user: call fusion_mcp_enable, which opens Preferences, expands "
    "General, selects API, ticks 'Fusion MCP Server (runs locally on this device)' and clicks "
    "Apply/OK. It needs Fusion running and briefly uses the foreground (announced with an "
    "on-screen caption). Manual path if you prefer: profile icon (top right) -> Preferences -> "
    "General -> API -> tick the box. NOTE the setting does NOT survive if Fusion is killed before "
    "Apply is clicked."
)


def _mcp_off_hint() -> str:
    """The MCP-unreachable hint, made state-aware: never tell the AI to go enable MCP when APS
    is signed in and could serve the request RIGHT NOW with zero setup."""
    r = _search_router()
    lead = ""
    if r.get("apsReady"):
        lead = ("NOTE FIRST: APS is signed in, so for FILE SEARCH you do not need MCP at all - "
                "fusion_aps_search works right now. Only enable MCP if you need its other surface "
                "(script text-to-CAD, screenshots, electronics object-model reads). ")
    elif r.get("apsConfigured"):
        lead = ("NOTE: for FILE SEARCH, APS is configured and only needs fusion_aps_signin "
                "(silent on a warm SSO session) - that may be less disruptive than enabling MCP. ")
    return lead + _MCP_OFF_STATIC


def _handle_mcp_status(fusion_info: dict, args: dict) -> dict:
    """Is Autodesk's Fusion MCP server up, and what does it expose?"""
    s = _mcp_new_session()
    if s.get("error"):
        # Enabled-ness is a persistent Fusion PREFERENCE, but it is only observable while
        # Fusion runs. Report the last GOOD observation so a closed Fusion reads as
        # "configured, dormant" instead of broken (John, 2026-08-17).
        return {"success": True, "enabled": False, "url": _MCP_URL, "error": s.get("error"),
                "lastEnabledAt": _get_bridge_pref("mcpLastEnabledAt", None),
                "fusionRunning": _is_fusion_running(),
                "_hint": _mcp_off_hint(), "statusVerb": "fusion_mcp_status"}
    tools = []
    tl = _mcp_rpc("tools/list")
    if not tl.get("_mcpError"):
        tools = [{"name": t.get("name"),
                  "description": (t.get("description") or "").split("\n")[0][:160]}
                 for t in ((tl.get("result") or {}).get("tools") or [])]
    try:
        import datetime as _dt
        _set_bridge_pref("mcpLastEnabledAt", _dt.datetime.now().isoformat(timespec="seconds"))
    except Exception:
        pass
    return {"success": True, "enabled": True, "url": _MCP_URL,
            "sessionId": s.get("sessionId"), "serverInfo": s.get("serverInfo"),
            "toolCount": len(tools), "tools": tools,
            "_hint": ("Autodesk's Fusion MCP server is LIVE. Its tools are Autodesk's own, not this "
                      "bridge's: call them with fusion_mcp_call {tool, arguments}. Use "
                      "fusion_mcp_tools for full input schemas and fusion_mcp_resources for the "
                      "Electronics entity schemas. These COMPLEMENT the fusion_* verbs: prefer a "
                      "native verb when one exists (it is tested and returns richer hints), and "
                      "reach for MCP for Autodesk's text-to-CAD surface."),
            "statusVerb": "fusion_mcp_status"}


def _handle_mcp_tools(fusion_info: dict, args: dict) -> dict:
    """Full tool list WITH input schemas."""
    tl = _mcp_rpc("tools/list")
    if tl.get("_mcpError"):
        return {"success": False, "enabled": False, "error": tl["_mcpError"], "_hint": _mcp_off_hint()}
    tools = (tl.get("result") or {}).get("tools") or []
    return {"success": True, "count": len(tools), "tools": tools,
            "_hint": "Call one with fusion_mcp_call {\"tool\":\"<name>\",\"arguments\":{...}}.",
            "statusVerb": "fusion_mcp_status"}


def _handle_mcp_call(fusion_info: dict, args: dict) -> dict:
    """Call ANY tool on Autodesk's Fusion MCP server."""
    tool = (args or {}).get("tool")
    if not tool:
        return {"success": False, "error": "Pass {tool, arguments}.",
                "_hint": "fusion_mcp_tools lists the available tools and their input schemas."}
    r = _mcp_rpc("tools/call", {"name": tool, "arguments": (args or {}).get("arguments") or {}},
                 timeout=int((args or {}).get("timeout") or 120))
    if r.get("_mcpError"):
        return {"success": False, "error": r["_mcpError"], "_hint": _mcp_off_hint()}
    res = r.get("result") or {}
    return {"success": not res.get("isError", False), "tool": tool, "result": res,
            "_hint": ("Autodesk MCP tool result. Content is usually a list of {type,text} blocks. "
                      "If it complains about no active document, open one first (fusion_aps_open) "
                      "- MCP acts on the ACTIVE Fusion document."),
            "statusVerb": "fusion_mcp_status"}


def _handle_mcp_resources(fusion_info: dict, args: dict) -> dict:
    """List, or read, Autodesk's MCP resources (the Electronics entity schemas)."""
    uri = (args or {}).get("uri")
    if uri:
        r = _mcp_rpc("resources/read", {"uri": uri})
        if r.get("_mcpError"):
            return {"success": False, "error": r["_mcpError"], "_hint": _mcp_off_hint()}
        return {"success": True, "uri": uri, "result": r.get("result"),
                "statusVerb": "fusion_mcp_status"}
    r = _mcp_rpc("resources/list")
    if r.get("_mcpError"):
        return {"success": False, "error": r["_mcpError"], "_hint": _mcp_off_hint()}
    res = (r.get("result") or {}).get("resources") or []
    return {"success": True, "count": len(res),
            "resources": [x.get("uri") for x in res],
            "_hint": "Read one with fusion_mcp_resources {\"uri\":\"resource://...\"}.",
            "statusVerb": "fusion_mcp_status"}


def _handle_mcp_enable(fusion_info: dict, args: dict) -> dict:
    """Turn Autodesk's MCP server ON by driving Fusion's Preferences dialog.

    Autodesk exposes NO API for this toggle (apiPreferences has debuggingPort and
    isDeveloperToolsEnabled but nothing for MCP), so the UI is the only route. Proven live
    2026-07-23. The tree children under General are rendered lazily by Qt and are NOT in the
    UIA tree, so the section click is image-space and DOES take the foreground - announced.
    """
    import time as _t
    if not _unwrap(_handle_fusion_readiness({}, {})).get("running"):
        return {"success": False, "error": "Fusion is not running.",
                "_hint": "fusion_start first, then fusion_mcp_enable."}
    already = _mcp_new_session()
    if not already.get("error"):
        return {"success": True, "enabled": True, "alreadyOn": True,
                "_hint": "Already enabled; nothing to do. fusion_mcp_status shows the tools."}

    _announce_foreground("turning on Fusion's MCP server in preferences")
    steps = []
    r = _unwrap(_ab_call("fusion_execute_text_command", {"command": "Commands.Start PreferencesCommand"}, timeout=45))
    _t.sleep(3)
    dlg = None
    for w in (_unwrap(_ab_call("desktop_list_windows", {}, timeout=40)).get("windows") or []):
        if str(w.get("title", "")).strip() == "Preferences":
            dlg = w.get("hwnd")
    if not dlg:
        return {"success": False, "error": "Preferences dialog did not open.",
                "steps": steps, "_hint": _mcp_off_hint()}
    steps.append("opened Preferences")

    def shot():
        s = _unwrap(_ab_call("desktop_screenshot_window", {"hwnd": int(dlg)}, timeout=45))
        cm = s.get("coordMap") or {}
        img = cm.get("image") or {}
        return cm.get("shotId"), (img.get("w") or 1400), (img.get("h") or 836)

    sid_, W, H = shot()
    if not sid_:
        return {"success": False, "error": "could not capture Preferences", "steps": steps}
    # fractional coords, measured on the real dialog (1400x836): expand arrow, then API row
    _ab_call("desktop_click", {"space": "image", "shotId": sid_, "hwnd": int(dlg),
                               "x": int(W * 0.029), "y": int(H * 0.092)}, timeout=30)   # expand General
    steps.append("expanded General")
    _t.sleep(2)
    sid_, W, H = shot()
    _ab_call("desktop_click", {"space": "image", "shotId": sid_, "hwnd": int(dlg),
                               "x": int(W * 0.071), "y": int(H * 0.135)}, timeout=30)   # API row
    steps.append("selected API")
    _t.sleep(2)
    sid_, W, H = shot()
    _ab_call("desktop_click", {"space": "image", "shotId": sid_, "hwnd": int(dlg),
                               "x": int(W * 0.493), "y": int(H * 0.569)}, timeout=30)   # the checkbox
    steps.append("ticked 'Fusion MCP Server'")
    _t.sleep(1)
    _ab_call("desktop_click", {"space": "image", "shotId": sid_, "hwnd": int(dlg),
                               "x": int(W * 0.839), "y": int(H * 0.948)}, timeout=30)   # Apply
    _t.sleep(2)
    _ab_call("desktop_click", {"space": "image", "shotId": sid_, "hwnd": int(dlg),
                               "x": int(W * 0.894), "y": int(H * 0.948)}, timeout=30)   # OK
    steps.append("clicked Apply then OK")
    _t.sleep(4)

    s = _mcp_new_session()
    ok = not s.get("error")
    return {"success": ok, "enabled": ok, "steps": steps,
            "sessionId": s.get("sessionId"), "serverInfo": s.get("serverInfo"),
            "narrate": ("Fusion's MCP server is on. I enabled it for you in Preferences."
                        if ok else "I drove the Preferences dialog but the MCP port is still closed."),
            "_hint": ("MCP server ENABLED - fusion_mcp_status lists its tools. Tell the user it is "
                      "handled; they do not need to touch Preferences."
                      if ok else
                      "The toggle did not take. Re-call fusion_mcp_enable - driving Preferences is "
                      "racy on a busy main thread and a second pass usually lands it. Do NOT hand the "
                      "user a Preferences walkthrough (describe.py says NEVER ask them to do this by "
                      "hand); if a second pass also fails, report what the steps[] show and that the "
                      "setting is discarded when Fusion restarts before Apply."),
            "statusVerb": "fusion_mcp_status"}


COMMAND_HANDLERS["mcp_status"] = _handle_mcp_status
COMMAND_HANDLERS["mcp_tools"] = _handle_mcp_tools
COMMAND_HANDLERS["mcp_call"] = _handle_mcp_call
COMMAND_HANDLERS["mcp_resources"] = _handle_mcp_resources
COMMAND_HANDLERS["mcp_enable"] = _handle_mcp_enable


# ── FUSION PREFERENCES, DRIVEN THROUGH THE UI (John, 2026-07-23) ─────────────────────────────
# Fusion's Python API exposes only a THIN slice of preferences (generalPreferences: theme, orbit,
# units; apiPreferences: debuggingPort, isDeveloperToolsEnabled). Everything else - including the
# Fusion MCP Server toggle - is UI-only.
#
# But the Preferences dialog IS reachable programmatically:
#   Commands.Start PreferencesCommand   opens it (no menu hunting, no profile-icon click)
# and its LEFT-HAND SECTION TREE is exposed to UIA, so sections can be found by name.
#
# The catch, learned live: the CHILD sections under General (API, Design, Manufacture,
# Electronics, Render, Drawing, Simulation) are rendered LAZILY by Qt and never appear in the
# UIA tree, even after desktop_ui_expand reports success. So navigating into a child section
# needs an image-space click, which takes the foreground and is therefore announced with a
# caption. Everything up to that point is background.
#
# ⛔ The setting is DISCARDED unless Apply/OK is clicked. Killing or restarting Fusion with the
# dialog still open loses it (that is exactly why an earlier MCP enable silently reverted).
_PREF_SECTIONS = {
    # section -> (parent-to-expand or None, fractional x, fractional y) on the 1400x836 dialog
    "general":     (None,      0.071, 0.092),
    "api":         ("general", 0.071, 0.135),
    "design":      ("general", 0.071, 0.179),
    "manufacture": ("general", 0.071, 0.222),
    "electronics": ("general", 0.071, 0.265),
    "render":      ("general", 0.071, 0.310),
    "drawing":     ("general", 0.071, 0.353),
    "material":    (None,      0.071, 0.483),
    "graphics":    (None,      0.071, 0.527),
    "network":     (None,      0.071, 0.570),
    "preview features": (None, 0.071, 0.744),
}


def _prefs_dialog_hwnd():
    for w in (_unwrap(_ab_call("desktop_list_windows", {}, timeout=40)).get("windows") or []):
        if str(w.get("title", "")).strip() == "Preferences":
            return w.get("hwnd")
    return None


def _prefs_shot(hwnd):
    s = _unwrap(_ab_call("desktop_screenshot_window", {"hwnd": int(hwnd)}, timeout=45))
    cm = s.get("coordMap") or {}
    img = cm.get("image") or {}
    return (cm.get("shotId"), img.get("w") or 1400, img.get("h") or 836,
            s.get("localSafePath"))


def _handle_prefs_open(fusion_info: dict, args: dict) -> dict:
    """Open Fusion Preferences, optionally navigate to a section, and hand back a screenshot the
    AI can click in. This is how you reach ANY preference, not just the API-exposed few."""
    import time as _t
    if not _unwrap(_handle_fusion_readiness({}, {})).get("running"):
        return {"success": False, "error": "Fusion is not running.",
                "_hint": "fusion_start first."}
    section = str((args or {}).get("section") or "").strip().lower()
    steps = []
    hwnd = _prefs_dialog_hwnd()
    if not hwnd:
        _announce_foreground("opening Fusion preferences")
        _ab_call("fusion_execute_text_command",
                 {"command": "Commands.Start PreferencesCommand"}, timeout=45)
        _t.sleep(3)
        hwnd = _prefs_dialog_hwnd()
        steps.append("opened Preferences")
    if not hwnd:
        return {"success": False, "error": "Preferences dialog did not open.", "steps": steps}

    if section:
        spec = _PREF_SECTIONS.get(section)
        if not spec:
            return {"success": False, "error": "Unknown section %r." % section,
                    "knownSections": sorted(_PREF_SECTIONS),
                    "_hint": ("Pass one of knownSections, or omit `section` and click the returned "
                              "shotId yourself with desktop_click {space:'image'}.")}
        parent, fx, fy = spec
        sid_, W, H, _p = _prefs_shot(hwnd)
        if parent:
            pspec = _PREF_SECTIONS[parent]
            # click the expand arrow, left of the parent label
            _ab_call("desktop_click", {"space": "image", "shotId": sid_, "hwnd": int(hwnd),
                                       "x": int(W * 0.029), "y": int(H * pspec[2])}, timeout=30)
            steps.append("expanded %s" % parent)
            _t.sleep(2)
            sid_, W, H, _p = _prefs_shot(hwnd)
        _ab_call("desktop_click", {"space": "image", "shotId": sid_, "hwnd": int(hwnd),
                                   "x": int(W * fx), "y": int(H * fy)}, timeout=30)
        steps.append("selected %s" % section)
        _t.sleep(2)

    sid_, W, H, path = _prefs_shot(hwnd)
    return {"success": True, "hwnd": hwnd, "section": section or "general",
            "shotId": sid_, "imageWidth": W, "imageHeight": H, "screenshot": path,
            "steps": steps, "knownSections": sorted(_PREF_SECTIONS),
            "_hint": ("Preferences is OPEN and showing this section. LOOK at the screenshot, then "
                      "click any control with desktop_click {space:'image', shotId, x, y, hwnd}. "
                      "⛔ Nothing is saved until you call fusion_prefs_close {save:true} (Apply+OK) "
                      "- killing or restarting Fusion first DISCARDS the change. Child sections "
                      "under General are lazily rendered and absent from the UIA tree, which is why "
                      "this verb hands you an image to click rather than control names."),
            "statusVerb": "fusion_get_preferences"}


def _handle_prefs_close(fusion_info: dict, args: dict) -> dict:
    """Apply+OK (save:true, default) or Cancel the Preferences dialog."""
    import time as _t
    save = (args or {}).get("save", True)
    hwnd = _prefs_dialog_hwnd()
    if not hwnd:
        return {"success": True, "closed": False,
                "_hint": "Preferences was not open; nothing to do."}
    sid_, W, H, _p = _prefs_shot(hwnd)
    if save:
        _ab_call("desktop_click", {"space": "image", "shotId": sid_, "hwnd": int(hwnd),
                                   "x": int(W * 0.839), "y": int(H * 0.948)}, timeout=30)  # Apply
        _t.sleep(2)
        _ab_call("desktop_click", {"space": "image", "shotId": sid_, "hwnd": int(hwnd),
                                   "x": int(W * 0.894), "y": int(H * 0.948)}, timeout=30)  # OK
    else:
        _ab_call("desktop_click", {"space": "image", "shotId": sid_, "hwnd": int(hwnd),
                                   "x": int(W * 0.951), "y": int(H * 0.948)}, timeout=30)  # Cancel
    _t.sleep(2)
    return {"success": True, "closed": _prefs_dialog_hwnd() is None, "saved": bool(save),
            "narrate": ("Saved your Fusion preferences." if save else "Closed preferences without saving."),
            "_hint": ("Applied and closed. Some settings (the MCP server is one) only take effect "
                      "once saved this way." if save else "Cancelled; nothing was changed."),
            "statusVerb": "fusion_get_preferences"}


COMMAND_HANDLERS["prefs_open"] = _handle_prefs_open
COMMAND_HANDLERS["prefs_close"] = _handle_prefs_close


def _fusion_signin_cfg_path():
    import pathlib
    d = pathlib.Path(os.path.expanduser("~")) / ".adom" / "fusion-signin"
    d.mkdir(parents=True, exist_ok=True)
    return d / "profile.json"


def _remembered_signin_profile():
    try:
        import json as _j
        p = _fusion_signin_cfg_path()
        if p.exists():
            return (_j.loads(p.read_text()) or {}).get("profile") or None
    except Exception:
        pass
    return None


def _remember_signin_profile(profile: str):
    try:
        import json as _j
        _fusion_signin_cfg_path().write_text(_j.dumps({"profile": profile}))
    except Exception:
        pass


def _chrome_authorize_urls(max_age_min: int = 15):
    """Scan Chrome + Edge profile History DBs for RECENT Autodesk-desktop OAuth authorize
    URLs (the Fusion Identity SDK ones - marked by `idsdk` / redirect to idmgr/callback).

    This is how we fix Fusion opening the WRONG browser profile: Fusion fires its OAuth at
    the OS-default browser, but the FULL authorize URL (client_id, PKCE challenge, state,
    request_id) lands in that browser's History. We read it back and re-open it in the
    profile the user actually authenticates Autodesk in. The URL is NOT tied to a browser
    (its redirect is accounts.autodesk.com/idmgr/callback + an autodesk:// protocol handoff
    matched by request_id), so completing it in ANY profile hands the token to the running
    Fusion. Verified live (John, 2026-07-21).

    Returns a list of {url, sourceProfileDir, browser, ageSec} newest first.
    """
    import glob, sqlite3, shutil, tempfile, time as _t
    la = os.environ.get("LOCALAPPDATA", "")
    roots = []
    if la:
        roots.append(("chrome", os.path.join(la, "Google", "Chrome", "User Data")))
        roots.append(("edge", os.path.join(la, "Microsoft", "Edge", "User Data")))
    now = _t.time()
    found = []
    for browser, root in roots:
        if not os.path.isdir(root):
            continue
        for hist in glob.glob(os.path.join(root, "*", "History")):
            prof_dir = os.path.basename(os.path.dirname(hist))
            tmp = os.path.join(tempfile.gettempdir(), "adom_hist_%s_%s.db" % (browser, prof_dir))
            try:
                shutil.copy2(hist, tmp)  # copy first - Chrome keeps History locked
                # Chrome buffers recent rows in the -wal; copy it too or a fresh authorize URL
                # is invisible (this was the bug: a just-opened sign-in did not appear).
                for ext in ("-wal", "-shm"):
                    if os.path.exists(hist + ext):
                        try: shutil.copy2(hist + ext, tmp + ext)
                        except Exception: pass
                con = sqlite3.connect(tmp)
                try: con.execute("PRAGMA journal_mode=WAL")
                except Exception: pass
                rows = con.execute(
                    "SELECT url, last_visit_time FROM urls "
                    "WHERE url LIKE '%developer.api.autodesk.com/authentication/v2/authorize%' "
                    "OR url LIKE '%idp.auth.autodesk.com/as/authorize%' "
                    "ORDER BY last_visit_time DESC LIMIT 6").fetchall()
                con.close()
            except Exception:
                continue
            for url, cts in rows:
                if "idsdk" not in url and "idmgr" not in url:
                    continue  # only the DESKTOP-app (Fusion) flow, not a web app
                epoch = (cts / 1_000_000) - 11644473600
                age = now - epoch
                if age <= max_age_min * 60:
                    found.append({"url": url, "sourceProfileDir": prof_dir,
                                  "browser": browser, "ageSec": int(age)})
    found.sort(key=lambda x: x["ageSec"])
    return found


def _fusion_window():
    """Find the main Fusion window via desktop_list_windows. Returns the whole entry (it already
    carries `rect`, so we get geometry in the SAME call - there is no desktop_window_info verb).
    Returns {} when not found."""
    wins = _unwrap(_ab_call("desktop_list_windows", {}, timeout=40)).get("windows") or []
    cand = {}
    for w in wins:
        t = str(w.get("title", ""))
        tl = t.lower()
        if "autodesk fusion" in tl or tl.startswith("signing in"):
            if "signing in" in tl or "welcome" in tl:   # prefer the sign-in/welcome window
                return w
            cand = cand or w
    return cand


def _fusion_window_hwnd():
    """Just the hwnd of the main Fusion window (int|None)."""
    return (_fusion_window() or {}).get("hwnd")


# ── NEVER STEAL THE USER'S FOREGROUND (John, 2026-07-22) ─────────────────────────────────────
# "try to NEVER bring windows to the foreground cuz its disruptive."
# desktop_click is SendInput and FOREGROUNDS the window. desktop_ui_click/desktop_ui_set are UIA
# Invoke/SetValue: programmatic, NO focus steal, NO cursor move, and Chromium exposes its a11y
# tree so page buttons/fields ARE reachable by accessible name. So: ALWAYS try background UIA
# first and only fall back to a foreground click for things with no UIA node (Fusion's Sign In
# button is a webview with none - that is the one documented exception).
# ── SIGN-IN WINDOW JANITOR (John, 2026-07-22) ────────────────────────────────────────────────
# "if you open a browser window for sign in, you must run a loop on a schedule to know when to
# close it so you never leave the user's desktop messy."
# We open browser windows to complete OAuth. Those windows are OURS and must not be left as
# litter once they have served their purpose. Every window we open is tracked here, and a
# background loop closes it as soon as the sign-in it belongs to is DONE (or it is clearly dead
# or too old). Never touches a window the user opened.
_signin_windows = []          # [{sessionId, profile, kind, openedAt}]
_signin_windows_lock = threading.Lock()
_JANITOR_MAX_AGE = 900
_last_update_sweep = 0.0        # 15 min: an OAuth window older than this is dead weight either way


def _track_signin_window(session_id, profile, kind="fusion"):
    if not session_id:
        return
    with _signin_windows_lock:
        _signin_windows.append({"sessionId": session_id, "profile": profile,
                                "kind": kind, "openedAt": _t2.time()})


def _close_tracked_window(w) -> bool:
    """Close one window WE opened. nbrowser_close_window only closes our own sessions."""
    r = _ab_call("nbrowser_close_window",
                 {"sessionId": w["sessionId"], "profile": w.get("profile")}, timeout=30)
    return isinstance(r, dict) and not r.get("_adCallError")


def _signin_goal_met(kind) -> bool:
    """Has the sign-in this window exists for actually completed?"""
    try:
        if kind == "aps":
            return bool(_aps_quick_state().get("signedIn"))
        return bool(_unwrap(_handle_fusion_readiness({}, {})).get("ready"))
    except Exception:
        return False


_last_seat_takeover_notice = [0.0]

# ── Driving badge lifecycle (John, 2026-08-17: "after we're done controlling fusion, shed
# the badge like nb does - after like 5 or 10 minutes of inactivity") ────────────────────
# Badge ON: any DRIVING verb dispatch while Fusion runs (set here, throttled in the helper).
# Badge OFF: the janitor sheds it once no driving verb has run for _BADGE_IDLE_SECONDS.
# PASSIVE reads are excluded from "driving" ON PURPOSE: the Dashboard polls readiness every
# 15s and pollers poll demo_progress every ~2s - counting those would pin the badge forever.
_BADGE_IDLE_SECONDS = 300   # John said 5-10 min; 5 keeps the taskbar honest sooner
_LAST_DRIVE_AT = [0.0]
_BADGE_PASSIVE = {
    "readiness", "state", "window_info", "describe", "demo_progress", "addin_status",
    "aps_status", "mcp_status", "get_preferences", "targets", "screenshot_fusion",
    "screenshot_all", "check_dialogs", "set_auto_update", "set_seat_takeover",
    "uninstall", "get_app_state", "board_info", "document_info",
}


def _note_driving_activity(command: str):
    """Called from dispatch for every verb: driving verbs stamp the activity clock and
    ensure the badge is on (async - a 6s desktop_taskbar call must never slow a verb)."""
    if command in _BADGE_PASSIVE:
        return
    import time as _t
    _LAST_DRIVE_AT[0] = _t.time()

    def _ensure():
        # RETRY, do not fire-and-forget: `start` is a driving verb dispatched BEFORE Fusion's
        # window exists, so a single attempt always found no hwnd and the badge never landed
        # (John caught exactly this after auto-green launched Fusion, 2026-08-18).
        try:
            from handlers.taskbar_overlay import set_driving_badge
            from fusion_detect import _fusion_main_hwnd_quick
            import time as _tt
            if not _driving_badge_enabled():
                return
            for _ in range(30):          # ~90s, covers a cold Fusion launch
                if _is_fusion_running():
                    hw = _fusion_main_hwnd_quick()
                    if hw and set_driving_badge(hw):
                        return
                _tt.sleep(3)
        except Exception:
            pass
    threading.Thread(target=_ensure, daemon=True).start()



def _window_sizing_enabled() -> bool:
    """Default TRUE: a Fusion window the user has to squint at is a bug, not a preference."""
    return bool(_get_bridge_pref("sizeFusionWindow", True))


def _handle_set_window_sizing(fusion_info: dict, args: dict) -> dict:
    """Read/toggle auto-sizing Fusion's window to ~92% of the screen after launch. STICKY."""
    if "enabled" not in (args or {}):
        cur = _window_sizing_enabled()
        return {"success": True, "sizeFusionWindow": cur,
                "_hint": ("Fusion window auto-sizing is %s. ON BY DEFAULT: after a launch the "
                          "bridge grows a small Fusion window to ~92%% of the monitor work area, "
                          "centered, WITHOUT activating it (SetWindowPos NOACTIVATE, never "
                          "ShowWindow/maximize, so your focus is never stolen). A window you "
                          "maximized or already sized large is left alone. Opt out: "
                          "fusion_set_window_sizing {\"enabled\": false} - STICKY."
                          % ("ON" if cur else "OFF")),
                "statusVerb": "fusion_state"}
    enabled = bool(args.get("enabled"))
    _set_bridge_pref("sizeFusionWindow", enabled)
    return {"success": True, "sizeFusionWindow": enabled,
            "narrate": ("I'll size Fusion's window to fill most of your screen when it launches."
                        if enabled else "I'll leave Fusion's window size exactly as Fusion opens it."),
            "statusVerb": "fusion_state"}


def _driving_badge_enabled() -> bool:
    """Default TRUE. The taskbar overlay is identity, not noise - but a user who dislikes it
    can switch it off permanently (John, 2026-08-18)."""
    return bool(_get_bridge_pref("showDrivingBadge", True))


def _handle_set_driving_badge(fusion_info: dict, args: dict) -> dict:
    """Read/toggle the 'Adom is driving Fusion' taskbar overlay badge. STICKY."""
    if "enabled" not in (args or {}):
        cur = _driving_badge_enabled()
        return {"success": True, "showDrivingBadge": cur,
                "_hint": ("The driving badge is currently %s. ON BY DEFAULT: while Adom drives "
                          "Fusion, its taskbar button wears a small Adom mark so the user knows "
                          "at a glance who is in control; it sheds after 5 minutes idle. Opt out: "
                          "fusion_set_driving_badge {\"enabled\": false} - STICKY."
                          % ("ON" if cur else "OFF")),
                "statusVerb": "fusion_state"}
    enabled = bool(args.get("enabled"))
    _set_bridge_pref("showDrivingBadge", enabled)
    if not enabled:
        try:
            from handlers.taskbar_overlay import clear_driving_badge
            clear_driving_badge()
        except Exception:
            pass
    return {"success": True, "showDrivingBadge": enabled,
            "narrate": ("I'll badge Fusion's taskbar button while I'm driving it."
                        if enabled else "I'll stop badging Fusion's taskbar button."),
            "_hint": "Sticky (~/.adom/fusion-bridge/prefs.json). Cleared immediately when turned off.",
            "statusVerb": "fusion_state"}


def _seat_takeover_enabled() -> bool:
    """Default TRUE (John: "always just take over the session"). Sticky opt-out for users
    who'd rather resolve seat conflicts themselves - mirrors autoUpdateFusion."""
    return bool(_get_bridge_pref("autoTakeOverSeat", True))


def _handle_set_seat_takeover(fusion_info: dict, args: dict) -> dict:
    """Read/toggle the mid-session seat-takeover sweep. STICKY per user. Scope note: this
    gates only the PASSIVE 20s sweep; a verb-driven fusion_start still grabs the seat
    (the user asked for Fusion, so taking the seat is implied - John's July rule)."""
    if "enabled" not in (args or {}):
        cur = _seat_takeover_enabled()
        return {"success": True, "autoTakeOverSeat": cur,
                "_hint": ("Seat takeover is currently %s. ON BY DEFAULT: when Autodesk pops the "
                          "'Active Sessions Exceeded' dialogs mid-session, the bridge clicks "
                          "Continue through BOTH in the background - this machine takes the seat, "
                          "the other machine's Fusion suspends RESUMABLY, one toast tells the user. "
                          "Opt out: fusion_set_seat_takeover {\"enabled\": false} - STICKY."
                          % ("ON" if cur else "OFF")),
                "statusVerb": "fusion_readiness"}
    enabled = bool(args.get("enabled"))
    _set_bridge_pref("autoTakeOverSeat", enabled)
    return {"success": True, "autoTakeOverSeat": enabled,
            "narrate": ("I'll automatically take the Fusion seat on this machine whenever "
                        "Autodesk flags the session limit." if enabled else
                        "I'll leave Autodesk seat-conflict dialogs for you to resolve yourself."),
            "_hint": ("Sticky (~/.adom/fusion-bridge/prefs.json). %s"
                      % ("Takeover ON: mid-session seat dialogs are clicked through silently."
                         if enabled else
                         "Takeover OFF: the sweep will NOT touch seat dialogs; fusion_start "
                         "still grabs the seat when the user asks for Fusion.")),
            "statusVerb": "fusion_readiness"}


# Registered HERE, next to the def - naming it in the COMMAND_HANDLERS literal (built ~3000
# lines earlier) is the import-time NameError that crash-looped the bridge on 2026-07-22.
COMMAND_HANDLERS["set_seat_takeover"] = _handle_set_seat_takeover
COMMAND_HANDLERS["set_driving_badge"] = _handle_set_driving_badge
COMMAND_HANDLERS["set_window_sizing"] = _handle_set_window_sizing


def _seat_sweep_tick():
    """ALWAYS take over the Autodesk seat (John, 2026-08-16: "always just take over the
    session"). The launch/readiness paths already auto-resolve the 'Active Sessions
    Exceeded' -> 'Suspend Remote Session' dialog pair, but a MID-SESSION seat conflict
    (Autodesk pops it whenever another machine signs in) sat unhandled until someone
    happened to poll readiness. This tick runs from the janitor loop every ~20s: cheap
    no-op when no dialog (local Win32 window walk), and when one IS up it clicks
    Continue on BOTH dialogs in the background (Suspend pre-selected = this machine
    takes the seat; the other machine's Fusion suspends resumably, nothing is lost).
    Toasts the user AFTER a real takeover so they know the other box got suspended."""
    # ⚠ SECTION SCOPING (v1.9.59, found by the badge-shed watcher): this tick has THREE
    # independent jobs - seat takeover, badge shed, save-prompt sweep. The seat section's
    # old early `return`s exited the WHOLE function on the no-dialog (normal!) path, so
    # the badge NEVER shed and the save sweep NEVER ran unless a seat dialog was up.
    # Each section is now self-contained; none may `return`.
    try:
        if _seat_takeover_enabled() and find_licensing_dialog():
            _seat_resolve_section()
    except Exception:
        pass

    # Keep Fusion's window a usable size (once per hwnd; never touches a maximized or
    # already-large window, never activates).
    try:
        if _window_sizing_enabled() and _is_fusion_running():
            from fusion_detect import _fusion_main_hwnd_quick as _hwq
            _h = _hwq()
            _n = _SIZE_ATTEMPTS.get(_h, 0) if _h else 99
            if _h and _n < 6:               # ~2 minutes of sweeps, then leave it alone forever
                _r = ensure_reasonable_window(hwnd=_h)
                _why = str(_r.get("why") or "")
                if "already" in _why:       # large enough or user-maximized -> converged, stop
                    _SIZE_ATTEMPTS[_h] = 99
                else:
                    _SIZE_ATTEMPTS[_h] = _n + 1
                    if _r.get("sized"):
                        print("[Fusion Bridge] window sized on sweep (attempt %d): %s" % (_n + 1, _why))
    except Exception:
        pass
    # Self-heal the badge: if driving is RECENT and Fusion is up but the badge is missing
    # (launch race, respawn, a manual clear), put it back on the next tick.
    try:
        from handlers.taskbar_overlay import badge_active as _ba, set_driving_badge as _sb
        from fusion_detect import _fusion_main_hwnd_quick as _hw
        if (_driving_badge_enabled() and not _ba() and _LAST_DRIVE_AT[0]
                and (_t2.time() - _LAST_DRIVE_AT[0]) <= _BADGE_IDLE_SECONDS and _is_fusion_running()):
            h = _hw()
            if h:
                _sb(h)
    except Exception:
        pass
    # Shed the driving badge after idle (John: "so the user knows we are DONE controlling").
    try:
        from handlers.taskbar_overlay import badge_active, clear_driving_badge
        if badge_active() and _LAST_DRIVE_AT[0] and (_t2.time() - _LAST_DRIVE_AT[0]) > _BADGE_IDLE_SECONDS:
            if clear_driving_badge():
                print("[Fusion Bridge] driving badge shed after %ds idle" % _BADGE_IDLE_SECONDS)
    except Exception:
        pass
    # DIRECT "Unsaved changes" catcher (John's blocked-screen screenshot, 2026-08-24:
    # a graceful stop wedged TEN MINUTES behind this dialog; the sentinel's Escape
    # fallback CANCELS the close, so the stop fought its own dialog). Find the dialog by
    # its title at the desktop level, prove the doc it names is OURS via UIA text, and
    # click Don't Save. Never touches a dialog naming a user's document.
    try:
        from handlers.dismiss_recovery import _find_window_by_title as _fwt2
        _uh = _fwt2("Unsaved changes")
        if _uh:
            _ours = None
            for _probe in tuple(_BRIDGE_DOCS)[:8] + _BRIDGE_DOC_BUILTINS:
                _rr = _find_controls_any(_ab_call("desktop_find_control",
                        {"hwnd": int(_uh), "contains": _probe}, timeout=8))
                if (_rr.get("best") or {}).get("name"):
                    _ours = _probe
                    break
            if _ours:
                _ck2 = _unwrap(_ab_call("desktop_ui_click",
                        {"hwnd": int(_uh), "name": "Don't Save"}, timeout=10))
                _bridge_action_ledger("unsaved_changes_dont_save",
                        {"hwnd": int(_uh), "docMatch": _ours,
                         "clicked": bool(_ck2.get("invoked") or _ck2.get("success"))})
                print("[Fusion Bridge] janitor: answered the Unsaved-changes dialog with "
                      "Don't Save (bridge doc %r)" % _ours)
    except Exception:
        pass
    # Same tick also sweeps a standing Save prompt for BRIDGE-CREATED docs (2026-08-16:
    # a bare "Save" Qt modal from a killed session blocked the main thread invisibly and
    # failed two tours in a row - the sentinel only ran during doc-switch calls).
    try:
        info = get_fusion_window_info() or {}
        if info.get("dialogs"):
            _dismiss_save_prompt_if_bridge_doc()
    except Exception:
        pass
    # PREVENTION beats dismissal: close our own dirty scratch docs while nothing is
    # driving, so the Save prompt never gets raised at the user in the first place.
    # Self-contained like every other section here - it must never be able to starve
    # the sections after it (the early-return bug that broke this sweep before).
    try:
        _close_idle_bridge_docs()
    except Exception:
        pass
    # A Create-new prompt from ANY route (not just our own EDIT) blocks the main thread.
    # Answer No: it means a name did not exist, and inventing an empty artifact would hide
    # that. Self-contained, like every other section in this sweep.
    try:
        for _d in (classify_blocking_dialogs() or []):
            if _d.get("category") == "create_confirm":
                _unwrap(_ab_call("desktop_ui_click", {"hwnd": _d.get("hwnd"), "name": "No"}, timeout=12))
                print("[Fusion Bridge] janitor: declined a stray Create-new prompt (%s)"
                      % _d.get("matchedOn", "title"))
                _bridge_action_ledger("create_confirm_declined", {"matchedOn": _d.get("matchedOn")})
    except Exception:
        pass
    # Autodesk's CRASH REPORTER ("Fusion Error Report", its own process - it outlives
    # Fusion, so it is NOT in the Fusion window family and no other sweep sees it).
    # John hit one on screen after the #50 seat-suspension day (2026-08-23). Two jobs:
    # (1) LEDGER it - a crash reporter is a timestamped "Fusion really crashed" marker,
    # exactly the forensic breadcrumb #50 lacked; (2) CLOSE it quietly so the user never
    # babysits it. Always "Close", never "Send Report": submitting crash data to Autodesk
    # is the user's call, not the janitor's. Self-contained like every section here.
    try:
        # LOCAL Win32 walk, not a relay verb: the relay envelope nests results under a
        # stringified `output` and the first version of this section parsed it wrong, so
        # a wedged black cer_dialog sat on John's taskbar for two hours (2026-08-23).
        # CER is its own process (cer_dialog.exe) that outlives Fusion; when its parent
        # died abruptly it renders BLACK and answers no UIA - escalate: WM_CLOSE, then
        # (only for a confirmed cer_dialog pid) a single-pid terminate. Never Send Report.
        import ctypes as _ct
        from handlers.dismiss_recovery import _find_window_by_title as _fwt
        _ch = _fwt("Fusion Error Report")
        if _ch and not _host_op_active():
            _u32 = _ct.windll.user32
            _pid = _ct.c_ulong(0)
            _u32.GetWindowThreadProcessId(_ct.c_void_p(int(_ch)), _ct.byref(_pid))
            _bridge_action_ledger("crash_reporter_seen", {"hwnd": int(_ch), "pid": _pid.value})
            _u32.PostMessageW(_ct.c_void_p(int(_ch)), 0x0010, 0, 0)   # WM_CLOSE
            _t2.sleep(2)
            if _u32.IsWindow(_ct.c_void_p(int(_ch))):
                # still standing: confirm the pid really is cer_dialog before terminating
                try:
                    import subprocess as _sp
                    _nm = _sp.run(["tasklist", "/FI", "PID eq %d" % _pid.value, "/FO", "CSV", "/NH"],
                                  capture_output=True, text=True, timeout=10,
                                  creationflags=getattr(_sp, "CREATE_NO_WINDOW", 0)).stdout
                    if "cer_dialog" in _nm.lower():
                        _sp.run(["taskkill", "/PID", str(_pid.value), "/F"],
                                capture_output=True, timeout=10,
                                creationflags=getattr(_sp, "CREATE_NO_WINDOW", 0))
                        _bridge_action_ledger("crash_reporter_killed",
                                              {"pid": _pid.value, "why": "wedged (WM_CLOSE ignored)"})
                except Exception:
                    pass
            else:
                _bridge_action_ledger("crash_reporter_dismissed", {"hwnd": int(_ch), "closed": True})
            print("[Fusion Bridge] janitor: Fusion crash reporter handled (ledgered; never "
                  "Send Report on the user's behalf)")
    except Exception:
        pass


def _seat_resolve_section():
    try:
        r = _resolve_seat_dialog(max_clicks=3)
        if r.get("clicks"):
            now = _t2.time()
            if now - _last_seat_takeover_notice[0] > 120:
                _last_seat_takeover_notice[0] = now
                try:
                    ab_client.notify(
                        "Fusion seat taken over on this machine",
                        "Autodesk flagged your account's session limit, so I clicked through the "
                        "'Active Sessions Exceeded' dialogs in the background and took the seat here. "
                        "Fusion on your other machine is SUSPENDED (resumable, nothing lost) - "
                        "using Fusion there will simply grab the seat back the same way.",
                        level="info", reach_user=True)
                except Exception:
                    pass
            print("[Fusion Bridge] seat sweep: took over the session (clicks=%s, verified=%s)"
                  % (r.get("clicks"), r.get("verified")))
            _bridge_action_ledger("seat_takeover",
                                  {"clicks": r.get("clicks"), "verified": r.get("verified")})
    except Exception:
        pass


def _signin_janitor_loop():
    """Poll until each tracked sign-in window can be closed, then close it."""
    while True:
        try:
            _t2.sleep(20)
            _seat_sweep_tick()
            # Periodic AUTO-UPDATE sweep. Fusion's update nag only enters the UIA tree while its
            # Job Status panel is open, so a single check at readiness time misses most of them.
            # Sweeping on a schedule catches the nag whenever Fusion actually shows it, and the
            # click is background so the user never sees any of it.
            global _last_update_sweep
            if _auto_update_enabled() and (_t2.time() - _last_update_sweep) > 300:
                _last_update_sweep = _t2.time()
                try:
                    if _unwrap(_handle_fusion_readiness({}, {})).get("running"):
                        _apply_fusion_update_silently()
                        # Same sweep also clears the Recovered Documents dialog whenever it
                        # shows up MID-SESSION or on a user-launched Fusion (launch-time
                        # relocation only covers bridge-driven starts). Info-only dialog:
                        # closing loses nothing (File > Recover Documents keeps the list),
                        # so it is ALWAYS safe to close in the background. (John, 2026-08-16:
                        # "anytime it comes up, just ignore it and close it".)
                        try:
                            if not _host_op_active():
                                dismiss_recovery_dialog(background=True)
                        except Exception:
                            pass
                except Exception:
                    pass
            with _signin_windows_lock:
                pending = list(_signin_windows)
            if not pending:
                continue
            done_goals = {}
            for w in pending:
                kind = w.get("kind", "fusion")
                if kind not in done_goals:
                    done_goals[kind] = _signin_goal_met(kind)
                aged = (_t2.time() - w["openedAt"]) > _JANITOR_MAX_AGE
                if done_goals[kind] or aged:
                    if _close_tracked_window(w):
                        print("[Fusion Bridge] janitor: closed %s sign-in window %s (%s)"
                              % (kind, w["sessionId"], "signed in" if done_goals[kind] else "expired"))
                    with _signin_windows_lock:
                        if w in _signin_windows:
                            _signin_windows.remove(w)
        except Exception:
            continue


def _start_signin_janitor():
    t = threading.Thread(target=_signin_janitor_loop, daemon=True, name="signin-janitor")
    t.start()
    return t


def _announce_foreground(reason: str) -> bool:
    """ALWAYS tell the user WHY, right before we steal their foreground.

    John, 2026-07-22: "anytime you bring something to the foreground, you should show an ad notify
    for 2 seconds telling the user why... or an ad caption." A caption is the gentler of the two
    (no toast stack, auto-clears), so that is what we use. Best-effort; never blocks the action.
    """
    try:
        _ab_call("desktop_caption",
                 {"text": "Adom: %s" % reason, "id": "fusion-bridge-foreground",
                  "expiresInMs": 2500}, timeout=15)
        return True
    except Exception:
        return False


def _click_background_first(hwnd, name=None, shot_id=None, x=None, y=None, reason=None) -> dict:
    """Click preferring the BACKGROUND UIA path (no focus steal, no cursor move). Only falls back
    to SendInput - which DOES foreground - when there is no UIA node, and in that case announces
    WHY to the user first. Returns {clicked, via, why, announced}."""
    if name:
        r = _unwrap(_ab_call("desktop_ui_click", {"hwnd": int(hwnd), "name": name}, timeout=30))
        if r.get("ok") or r.get("clicked") or r.get("invoked"):
            return {"clicked": True, "via": "uia_background", "announced": False,
                    "why": "UIA Invoke on %r (background, no focus steal)" % name}
    if shot_id is not None and x is not None and y is not None:
        announced = _announce_foreground(reason or "clicking %s for you" % (name or "a button"))
        _ab_call("desktop_click", {"space": "image", "shotId": shot_id,
                                   "x": int(x), "y": int(y), "hwnd": int(hwnd)}, timeout=30)
        return {"clicked": True, "via": "foreground_click", "announced": announced,
                "why": "no UIA node; used SendInput (DOES foreground - user was told why)"}
    return {"clicked": False, "via": None, "announced": False,
            "why": "no UIA node and no shot coords given"}


# ── HUMAN WALLS: notify the user, and tell the AI it can drive it ────────────────────────────
# John, 2026-07-22: "you should be sending me ad notifies when you need me to do something" AND
# "suggest to the ai via hints that you can drive the thing on your own cuz the adom user wants
# you to do everything automatically if you can". So on every wall we do BOTH: toast the human,
# and hand the AI an exact, mechanical way to clear it without them.
def _autodesk_window_hwnds() -> set:
    """hwnds of every Autodesk-ish BROWSER window right now (used to tell NEW from STALE)."""
    wins = _unwrap(_ab_call("desktop_list_windows", {}, timeout=40)).get("windows") or []
    out = set()
    for w in wins:
        tl = str(w.get("title", "")).lower()
        if "autodesk" in tl and "fusion" not in tl:
            out.add(w.get("hwnd"))
    return out


def _advance_credentials_page(hwnd, email) -> dict:
    """Self-drive the Autodesk credentials page as far as possible WITHOUT secrets
    (John, 2026-08-16: 'do as much self driving automation as you can... only let the
    AI know when you need it to know'). Background UIA only - no foreground, no focus
    steal: set the email/username field, invoke Next/Sign in, then classify what the
    page became. Returns {advanced, wall, detail}. NEVER touches password fields."""
    import time as _t
    out = {"advanced": False, "wall": "credentials", "detail": ""}
    try:
        found = _find_controls_any(_ab_call("desktop_find_control", {"hwnd": int(hwnd), "role": "edit"}, timeout=40))
        ctrls = found.get("controls") or []
        if not ctrls and found.get("best"):
            ctrls = [found["best"]]
        pwd = [c for c in ctrls if "password" in str(c.get("name", "")).lower()
               or "password" in str(c.get("automationId", "")).lower()]
        if pwd:
            out["wall"] = "password"
            out["detail"] = "password field present - the human step"
            return out
        emailish = [c for c in ctrls if any(k in (str(c.get("name", "")) + str(c.get("automationId", ""))).lower()
                                            for k in ("email", "username", "user id", "userid"))] or ctrls[:1]
        if email and emailish:
            c = emailish[0]
            r = _unwrap(_ab_call("desktop_ui_set", {"hwnd": int(hwnd),
                                                    "automationId": c.get("automationId"),
                                                    "name": c.get("name"), "value": email}, timeout=30))
            if r.get("success") or r.get("ok"):
                out["detail"] = "typed email %r into %r (background UIA)" % (email, c.get("name") or c.get("automationId"))
                for btn in ("Next", "NEXT", "Sign in", "Sign In", "Continue"):
                    rb = _unwrap(_ab_call("desktop_ui_click", {"hwnd": int(hwnd), "name": btn}, timeout=25))
                    if rb.get("success") or rb.get("ok") or rb.get("status") == "ok":
                        out["advanced"] = True
                        out["detail"] += "; invoked %r" % btn
                        break
                if out["advanced"]:
                    _t.sleep(4)
                    found2 = _unwrap(_ab_call("desktop_find_control", {"hwnd": int(hwnd), "role": "edit"}, timeout=40))
                    ctrls2 = found2.get("controls") or found2.get("matches") or []
                    if not ctrls2 and found2.get("best"):
                        ctrls2 = [found2["best"]]
                    if any("password" in (str(c.get("name", "")) + str(c.get("automationId", ""))).lower() for c in ctrls2):
                        out["wall"] = "password"
                        out["detail"] += "; page advanced to the PASSWORD step"
    except Exception as e:
        out["detail"] = "advance error: " + str(e)[:120]
    return out


def _detect_signin_wall(ignore_hwnds=None) -> dict:
    """Classify what the Autodesk sign-in is waiting on, from BROWSER WINDOW TITLES.
    Title-based on purpose: cheap, needs no OCR, and never touches/foregrounds the window.

    ignore_hwnds = windows that already existed BEFORE we opened this sign-in. Without it a dead
    'Sign-in request expired' / stale '2-step verification' tab from an earlier attempt gets
    reported as the live wall and we toast the user about nothing (seen live 2026-07-22).
    """
    ignore = set(ignore_hwnds or ())
    wins = _unwrap(_ab_call("desktop_list_windows", {}, timeout=40)).get("windows") or []
    best = {}
    for w in wins:
        if w.get("hwnd") in ignore:
            continue
        t = str(w.get("title", "")); tl = t.lower()
        if "2-step verification" in tl or "confirm sign-in" in tl:
            return {"wall": "twofactor_email_code", "hwnd": w.get("hwnd"), "title": t}
        if "identity manager" in tl:
            best = {"wall": "protocol_handoff", "hwnd": w.get("hwnd"), "title": t}
        elif "sign in - autodesk" in tl and not best:
            best = {"wall": "credentials", "hwnd": w.get("hwnd"), "title": t}
    return best


_WALL_COPY = {
    "twofactor_email_code": ("Autodesk needs your 6-digit code",
                             "Autodesk emailed a verification code to finish signing Fusion in. "
                             "Enter it in the Chrome window, or tell your AI to fetch it for you."),
    "credentials": ("Autodesk sign-in needs you",
                    "The Autodesk sign-in is asking for credentials. I never type your password "
                    "or 2FA - please complete it in the Chrome window."),
    "password": ("Just your password, everything else is done",
                 "Adom drove the Fusion sign-in all the way: email entered, page advanced. "
                 "The ONLY step left is your password (and I never type secrets). Type it in "
                 "the browser window and everything continues automatically."),
    "protocol_handoff": ("Allow Fusion to finish signing in",
                         "Chrome is asking permission to open Autodesk Identity Manager. Click "
                         "Open to hand the login back to Fusion."),
}


def _resolve_and_advance_wall(wall_info, email) -> dict:
    """Given a detected wall, push it as far as code can go (email typing on the
    credentials page), refining 'credentials' into 'password' when that is what
    remains. Returns possibly-updated wall_info with 'advance' detail attached."""
    if wall_info.get("wall") == "credentials" and wall_info.get("hwnd"):
        adv = _advance_credentials_page(wall_info["hwnd"], email)
        wall_info = dict(wall_info)
        wall_info["wall"] = adv.get("wall") or wall_info["wall"]
        wall_info["advance"] = adv.get("detail")
        wall_info["advanced"] = adv.get("advanced")
    return wall_info


def _notify_signin_wall(wall: str) -> dict:
    """Toast the user's real desktop about a human wall. Best-effort, never raises."""
    title, body = _WALL_COPY.get(wall, ("Fusion sign-in needs you", "Please check the sign-in window."))
    return _cli_notify_all(title, body, "warning")


def _wall_hint(wall: str, notified: bool) -> str:
    """The AI-facing instruction for a wall. Per John: do as much as possible IN CODE, and use
    HINTS (not skills) to make the calling AI act correctly."""
    common = ("I ALREADY TOASTED THE USER about this (notifyDelivered=%s) - you do not need to ask "
              "me to, and do not nag them again. " % notified)
    if wall == "twofactor_email_code":
        return (common +
                "WALL: Autodesk emailed a 6-DIGIT CODE. THE USER WANTS YOU TO DO THIS FOR THEM if "
                "you can, so TRY THIS FIRST, do not just hand it back to them: if `adom-google` is "
                "available in YOUR container, read the code out of their Gmail and submit it - "
                "  adom-google gmail search 'from:autodesk verification code' --limit 1 \n"
                "then pull the 6 digits and call  fusion_signin_2fa {\"code\":\"123456\"}  which "
                "types it in the BACKGROUND (no foreground steal) and submits. If `adom-google` is "
                "NOT installed, TELL THE USER that installing adom-google would let you finish "
                "logins like this automatically next time, then let them type the code. NEVER ask "
                "them for the code if you can read it yourself.")
    if wall == "password":
        return (common +
                "WALL: PASSWORD ONLY. The bridge already typed the user's email and advanced the "
                "page (see steps[]) - the single remaining human act is typing their password in "
                "the browser window. Do NOT ask them for the password, do NOT try to type it, and "
                "do NOT re-drive the page. After they type it, the flow continues automatically - "
                "poll fusion_readiness until ready:true (a 2FA wall may follow; if it does you may "
                "be able to finish it yourself via fusion_signin_2fa + adom-google Gmail).")
    if wall == "protocol_handoff":
        return (common +
                "WALL: Chrome's native 'Open Autodesk Identity Manager?' dialog. Try to clear it "
                "YOURSELF in the background: desktop_find_control on that dialog then "
                "desktop_ui_click {name:'Open'} (UIA, no focus steal). Only if UIA finds no node, "
                "fall back to a foregrounded desktop_click. Then poll fusion_readiness.")
    return (common +
            "WALL: the sign-in wants credentials. NEVER type their password or 2FA. If the browser "
            "autofills, you may submit it. Otherwise let the user finish in the window I opened.")


def _unwrap(r):
    """ab verb responses come back either flat or nested under `data` (the CLI puts the real
    payload in output.data). Always look through both - forgetting this silently broke the
    server-side Sign In click, because coordMap/shotId live one level down."""
    if not isinstance(r, dict):
        return {}
    inner = r.get("data")
    if isinstance(inner, dict) and inner:
        return inner
    return r


def _find_coordmap(obj, depth=0):
    """Recursively find a coordMap (with shotId) anywhere in a verb response - ab has
    moved it across envelope levels between versions (top-level sibling of data in some,
    nested under data in others, stringified inside output in 2.0.46's direct API).
    Returns the coordMap dict or {}."""
    if depth > 5:
        return {}
    if isinstance(obj, str) and obj[:1] in "[{":
        try:
            import json as _j
            return _find_coordmap(_j.loads(obj), depth + 1)
        except Exception:
            return {}
    if isinstance(obj, dict):
        cm = obj.get("coordMap")
        if isinstance(cm, dict) and cm.get("shotId"):
            return cm
        if obj.get("shotId"):
            return obj
        for k in ("data", "output", "result", "response"):
            got = _find_coordmap(obj.get(k), depth + 1)
            if got:
                return got
    return {}


def _fusion_click_signin() -> dict:
    """Click Fusion's 'Welcome to Fusion' -> Sign In button SERVER-SIDE, ALWAYS, on the user's
    behalf (John, 2026-07-22: "you should ALWAYS just click sign in on behalf of the adom
    users"). The button is a webview with no UIA node, so this is an image-space click.

    Runs on-box, so it costs no remote round-trip against Fusion's ~2-min OAuth expiry.
    Returns {clicked: bool, why: str} - never raises.
    """
    import time as _t
    win = _fusion_window()
    hwnd = win.get("hwnd") or _unwrap(_handle_fusion_readiness({}, {})).get("hwnd")
    if not hwnd:
        return {"clicked": False, "why": "could not find the Fusion window"}

    # A MINIMIZED Fusion captures as a ~237x39 title-bar sliver, so the click lands on nothing.
    # Restore it first (seen live: rect was at -32000,-32000, the Windows minimized position).
    rect = win.get("rect") or {}
    if (rect.get("left") is not None and rect["left"] <= -30000) or \
       (0 < (rect.get("width") or 0) < 600) or (0 < (rect.get("height") or 0) < 400):
        _ab_call("desktop_set_window_state", {"hwnd": int(hwnd), "state": "restore"}, timeout=30)
        _t.sleep(1.5)

    _raw = _ab_call("desktop_screenshot_window", {"hwnd": int(hwnd)}, timeout=45)
    shot = _unwrap(_raw)
    # ab has moved coordMap across envelope levels between versions (sibling of data /
    # nested / stringified in output on 2.0.46) - search recursively, never guess a level.
    cm = _find_coordmap(_raw)
    sid = cm.get("shotId")
    if not sid:
        # VERSION-PROOF fallback (ab 2.0.46's direct API returns no coordMap at all):
        # compute the click in SCREEN space from the window rect - no shotId needed.
        w_rect = (win.get("rect") or {})
        L = w_rect.get("left"); T = w_rect.get("top")
        W = w_rect.get("width") or 0; H = w_rect.get("height") or 0
        if L is not None and W > 400 and H > 300:
            sx = int(L + W * 0.5); sy = int(T + H * 0.57)
            rr = _unwrap(_ab_call("desktop_click", {"x": sx, "y": sy, "hwnd": int(hwnd),
                                                    "reason": "signing Fusion in for you (clicking Sign In, screen-space fallback)"},
                                  timeout=30))
            okc = rr.get("success") or rr.get("ok") or rr.get("status") == "ok"
            return {"clicked": bool(okc),
                    "why": "screen-space fallback click at (%d,%d) rect %sx%s: %s" % (sx, sy, W, H, "ok" if okc else str(rr.get("error"))[:80]),
                    "via": "screen-fallback"}
    if not sid:
        return {"clicked": False,
                "why": "screenshot returned no shotId (hwnd=%s raw_keys=%s err=%s)" % (
                    hwnd,
                    sorted(list(_raw.keys()))[:8] if isinstance(_raw, dict) else type(_raw).__name__,
                    str((_raw or {}).get("error"))[:100] if isinstance(_raw, dict) else "")}
    img = cm.get("image") or {}
    w = shot.get("width") or img.get("w") or img.get("width") or cm.get("imageWidth") or 1400
    h = shot.get("height") or img.get("h") or img.get("height") or cm.get("imageHeight") or 860
    # Background UIA first (no focus steal). Fusion's Sign In is a WEBVIEW button with no UIA
    # node, so this almost always falls through to the image-space click - that is the documented
    # exception to the never-foreground rule, not a licence to foreground elsewhere.
    r = _click_background_first(hwnd, name="Sign In", shot_id=sid,
                                x=int(w * 0.5), y=int(h * 0.57),
                                reason="signing Fusion in for you (clicking Sign In)")
    return {"clicked": r.get("clicked"),
            "why": "%s (image %dx%d)" % (r.get("why"), w, h),
            "via": r.get("via")}


def _capture_fresh_authorize(wait_sec: int = 30, max_age: int = 120):
    """Poll the browser History for a FRESH (< max_age s) Fusion authorize URL for up to
    wait_sec, ON-BOX. Collapsing capture into one server-side wait (instead of the caller
    re-polling over the flaky relay) is what lets the whole chain finish inside Fusion's
    ~2-min request window. Returns the auth dict or None."""
    import time as _t
    deadline = _t.time() + max(1, wait_sec)
    while True:
        urls = _chrome_authorize_urls()
        if urls and urls[0]["ageSec"] <= max_age:
            return urls[0]
        if _t.time() >= deadline:
            return urls[0] if urls else None
        _t.sleep(2)


_SSO_BUTTONS = {"google": "Continue with Google", "apple": "Continue with Apple",
                "microsoft": "Continue with Microsoft", "facebook": "Continue with Facebook"}


def _drive_sso_signin(profile: str, tab_id, email: str, provider: str = "google") -> dict:
    """Finish the Autodesk sign-in over CDP using SSO - NO password, NO 2FA, NO foreground.

    PROVEN LIVE (John, 2026-07-22) after password/2FA attempts kept stalling. If the browser
    PROFILE is already signed into the identity provider (a work Google account is the common
    case), "Continue with Google" -> pick the account -> "Open Product" completes the whole login
    silently and hands the token back to Fusion via the autodesk:// callback.

    TWO THINGS MATTER:
      1. It must be a FRESH flow. Resuming a flowId that already advanced past the provider
         choice lands you on the PASSWORD screen with no way back - that dead end cost us an hour.
      2. Click the provider BEFORE typing any email; entering an email commits to the password path.
    """
    steps = []

    def ev(expr):
        r = _unwrap(_ab_call("nbrowser_eval", {"profile": profile, "tabId": tab_id,
                                               "expression": expr}, timeout=45))
        return r.get("result") or r.get("value")

    def click(text=None, selector=None):
        a = {"profile": profile, "tabId": tab_id}
        if text: a["text"] = text
        if selector: a["selector"] = selector
        r = _unwrap(_ab_call("nbrowser_click", a, timeout=45))
        return bool(r.get("ok") or r.get("success") or r.get("changed"))

    btn = _SSO_BUTTONS.get((provider or "google").lower(), _SSO_BUTTONS["google"])
    if not click(text=btn):
        return {"done": False, "steps": steps,
                "why": "%r not on the page - this is probably NOT a fresh flow (a resumed flowId "
                       "goes straight to the password screen)." % btn}
    steps.append("clicked %r" % btn)
    _t2.sleep(7)

    # provider account chooser - pick by the exact email
    if email and click(text=email):
        steps.append("picked the %s account" % email)
        _t2.sleep(8)

    state = str(ev("JSON.stringify({u:location.href.slice(0,120),b:document.body.innerText.slice(0,200)})") or "")
    if "You're signed in" in state or "signed in" in state.lower():
        steps.append("provider returned 'You're signed in'")
        # hand the token back to Fusion (autodesk:// protocol callback)
        if click(text="Open Product"):
            steps.append("clicked 'Open Product' to hand the token to Fusion")
        return {"done": True, "steps": steps, "state": state[:200]}
    return {"done": False, "steps": steps, "state": state[:200],
            "why": "did not reach the signed-in page; a provider password/2FA may be required"}


def _orchestrate_signin_2fa(args: dict) -> dict:
    """Submit Autodesk's emailed 6-digit verification code, IN THE BACKGROUND.

    Exists so the AI can finish a login end-to-end for the user (read the code from Gmail with
    adom-google, then call this) instead of parking them on a 2FA screen. Uses UIA SetValue +
    Invoke, so it never steals focus or moves the cursor.
    """
    code = str(args.get("code") or "").strip()
    digits = "".join(ch for ch in code if ch.isdigit())
    if len(digits) < 4:
        return {"success": False, "errorCode": "bad_code",
                "error": "Pass the emailed code, e.g. fusion_signin_2fa {\"code\":\"123456\"}.",
                "_hint": ("Read it from the user's Gmail if you can: "
                          "adom-google gmail search 'from:autodesk verification code' --limit 1 "
                          "-> take the 6 digits -> call this verb again. If adom-google is not "
                          "installed, tell the user it would let you automate this."),
                "statusVerb": "fusion_signin_2fa"}

    wall = _detect_signin_wall()
    hwnd = args.get("hwnd") or wall.get("hwnd")
    if not hwnd:
        return {"success": False, "errorCode": "no_2fa_window",
                "error": "No Autodesk 2-step-verification window found.",
                "_hint": ("Nothing is waiting on a code right now. Check fusion_readiness - if "
                          "needsSignin is still true, run fusion_signin again to restart the flow."),
                "statusVerb": "fusion_readiness"}

    # Find the code field WITHOUT touching the foreground. Autodesk renders either one input or a
    # segmented set, so try the obvious accessible names, then fall back to the first edit control.
    found = _unwrap(_ab_call("desktop_find_control",
                             {"hwnd": int(hwnd), "role": "edit"}, timeout=40))
    ctrls = found.get("controls") or found.get("matches") or []
    steps = []
    filled = False
    if len(ctrls) >= len(digits) and len(ctrls) >= 4:
        # segmented: one box per digit
        for i, ch in enumerate(digits[:len(ctrls)]):
            _ab_call("desktop_ui_set", {"hwnd": int(hwnd),
                                        "automationId": ctrls[i].get("automationId"),
                                        "name": ctrls[i].get("name"), "value": ch}, timeout=20)
        filled = True
        steps.append("typed %d digits into %d segmented boxes (background UIA)" % (len(digits), len(ctrls)))
    elif ctrls:
        c = ctrls[0]
        r = _unwrap(_ab_call("desktop_ui_set", {"hwnd": int(hwnd),
                                                "automationId": c.get("automationId"),
                                                "name": c.get("name"), "value": digits}, timeout=20))
        filled = bool(r.get("ok") or r.get("set") or r is not None)
        steps.append("typed the code into %r (background UIA)" % (c.get("name") or c.get("automationId")))

    if not filled:
        return {"success": False, "errorCode": "code_field_not_found",
                "error": "Could not find the code field via UIA.",
                "controlsSeen": ctrls[:8],
                "_hint": ("The code input was not in the UIA tree (shadow DOM). Run "
                          "desktop_find_control {hwnd:%s} to see what IS exposed. Last resort is a "
                          "FOREGROUNDED desktop_click on the field then desktop_type - avoid that "
                          "if you can, foregrounding is disruptive to the user." % hwnd),
                "statusVerb": "fusion_signin_2fa"}

    sub = _click_background_first(hwnd, name="Next")
    if not sub.get("clicked"):
        sub = _click_background_first(hwnd, name="Verify")
    steps.append("submitted via %s" % (sub.get("via") or "no submit control found"))

    return {"success": True, "submitted": True, "steps": steps,
            "narrate": "I read the code in and submitted it for you, without touching your screen.",
            "_hint": ("Code submitted in the BACKGROUND. Now poll fusion_readiness until ready:true. "
                      "If a Chrome 'Open Autodesk Identity Manager?' dialog appears, clear it with "
                      "desktop_ui_click {name:'Open'} (background) - fusion_signin reports that wall "
                      "too. If it says the code was wrong, the email may have a NEWER code."),
            "statusVerb": "fusion_readiness"}


def _orchestrate_signin(args: dict) -> dict:
    """Sign Fusion in through the CORRECT browser profile - the fix for Fusion firing its
    OAuth at the OS-default browser (often the wrong Autodesk account). See the
    fusion-multiprofile-signin skill.

    Staged: pick the target profile (remembered / arg / probed / ask) -> ensure Fusion has
    emitted its OAuth URL (click Sign In if not) -> read that URL from the wrong browser's
    History -> re-open it in the TARGET profile in the BACKGROUND -> report where it waits.

    auto=True runs the whole chain in ONE server-side call: click Fusion's Sign In, poll on-box
    for the fresh authorize URL, then re-open it in the target profile - so the capture->reopen
    round-trips happen on the box (fast) and fit inside Fusion's ~2-min request expiry even when
    the remote relay is slow. This is the reliable path; the staged/manual path is the fallback.
    """
    steps = []
    # ALWAYS click Sign In for the user unless they explicitly opt out (auto:false). Adom users
    # should never be told "now go click Sign In yourself" - we do it for them and SAY so.
    auto = args.get("auto")
    auto = True if auto is None else bool(auto)
    wait_sec = int(args.get("waitSec") or 32)
    target = args.get("profile")           # explicit override, e.g. "chrome:john@adom.inc"
    if target:
        _remember_signin_profile(target)
    if not target:
        target = _remembered_signin_profile()

    rd = _handle_fusion_readiness({}, {})
    if not rd.get("running"):
        # Self-drive (John, 2026-08-16): never hand "go start Fusion" back to the caller.
        steps.append("Fusion was not running - launching it (background) as part of sign-in")
        try:
            dispatch_command("start", {})
        except Exception as e:
            steps.append("launch failed: " + str(e)[:80])
        import time as _lt
        deadline = _lt.time() + 120
        while _lt.time() < deadline:
            _lt.sleep(8)
            rd = _handle_fusion_readiness({}, {})
            if rd.get("running"):
                break
        if not rd.get("running"):
            return {"success": False, "stage": "launch_failed", "done": False, "steps": steps,
                    "error": "Fusion did not come up within 120s of launch.",
                    "_hint": "Check fusion_readiness; the box may be mid-update. Retry fusion_signin.",
                    "statusVerb": "fusion_readiness"}
        _lt.sleep(5)
    if not rd.get("needsSignin"):
        return {"success": True, "stage": "already", "done": True, "steps": steps,
                "narrate": "Fusion is already signed in.",
                "_hint": "Already signed in (ready:%s). Nothing to do." % rd.get("ready"),
                "statusVerb": "fusion_signin"}

    # profiles known to ABE (for target selection + naming)
    profs = _ab_call("nbrowser_profiles", {}, timeout=40)
    pdata = profs.get("data", profs) if isinstance(profs, dict) else {}
    choices = [p for p in (pdata.get("profiles") or []) if isinstance(p, dict) and p.get("profile")]

    # read Fusion's authorize URL from browser history (it fires the OS-default browser)
    urls = _chrome_authorize_urls()
    fresh = urls[0] if (urls and urls[0]["ageSec"] <= 150) else None
    if auto and not fresh:
        # ONE server-side pass: click Fusion's Sign In FOR THE USER, then poll on-box for the URL.
        ck = _fusion_click_signin()
        clicked = ck.get("clicked")
        steps.append("clicked Fusion 'Sign In' for the user" if clicked
                     else "could not click Fusion 'Sign In' (%s)" % ck.get("why"))
        got = _capture_fresh_authorize(wait_sec=wait_sec, max_age=140)
        if got and got["ageSec"] <= 150:
            urls = [got]
        elif got:
            urls = [got]  # stale; fall through to the stale branch which tells us to reset
        else:
            urls = []
    if not urls:
        return {"success": True, "stage": "click_signin", "done": False, "steps": steps,
                "narrate": ("I clicked Fusion's Sign In for you, but Fusion has not written its "
                            "sign-in URL to a browser yet. Give it a few seconds."),
                "_hint": ("THIS VERB CLICKS FUSION'S 'Sign In' FOR THE USER - never tell them to go "
                          "click it themselves, and do not click it yourself. It just did (see "
                          "steps[]), but no Fusion OAuth URL has landed in any browser's history "
                          "yet. Simply CALL fusion_signin AGAIN in a few seconds; it will click if "
                          "needed, wait on-box for the URL, and re-open it in the right profile. "
                          "If steps[] says the click FAILED, that is a bridge bug worth reporting - "
                          "the usual causes are a minimized Fusion window (this verb restores it) "
                          "or the sign-in wall not being up yet (check fusion_readiness "
                          "needsSignin:true)."),
                "statusVerb": "fusion_signin"}
    auth = urls[0]
    steps.append("captured Fusion OAuth URL from %s profile '%s' (%ss old)"
                 % (auth["browser"], auth["sourceProfileDir"], auth["ageSec"]))
    # Fusion's sign-in request EXPIRES in ~2 min. A URL older than that completes the LOGIN but
    # Fusion no longer waits on its request_id -> "Sign-in request expired", no handoff. Never
    # silently reuse it; force a fresh request. (Learned live 2026-07-21.)
    if auth["ageSec"] > 150:
        return {"success": True, "stage": "stale", "done": False, "steps": steps,
                "signinAuthUrl": auth["url"], "signinAgeSec": auth["ageSec"],
                "_hint": ("The newest Fusion sign-in URL is %ss old - Fusion expires the request in "
                          "~2 min, so completing it yields 'Sign-in request expired' and NO handoff. "
                          "Get a FRESH request: fusion_stop then fusion_start (resets Fusion to a "
                          "clean 'Welcome to Fusion'), click Fusion's Sign In, and call fusion_signin "
                          "again PROMPTLY. Completion is near-instant when the target profile is "
                          "already signed into Autodesk (auto-consents). Skill: "
                          "fusion-multiprofile-signin." % auth["ageSec"]),
                "statusVerb": "fusion_signin"}

    # choose the TARGET profile if not already fixed
    reason = ""
    if not target:
        # probe each live profile for a real Autodesk session; prefer signed-in, then work over consumer
        for c in choices:
            if not c.get("live"):
                continue
            ls = _ab_call("nbrowser_login_state",
                          {"profile": c["profile"], "url": "https://accounts.autodesk.com/"}, timeout=40)
            lsd = ls.get("data", ls) if isinstance(ls, dict) else {}
            c["_ad"] = "signed-in" if lsd.get("loggedIn") else ("signed-out" if lsd.get("ok") is not None else "unprobed")
            c["_conf"] = lsd.get("confidence"); c["_cookies"] = lsd.get("cookieCount")
        signed = [c for c in choices if c.get("_ad") == "signed-in"]
        if signed:
            signed.sort(key=lambda c: ({"high": 3, "medium": 2, "low": 1}.get(c.get("_conf"), 0), c.get("_cookies") or 0), reverse=True)
            target = signed[0]["profile"]; reason = "it holds a live Autodesk session"
        else:
            _CONSUMER = ("gmail.com", "outlook.com", "hotmail.com", "yahoo.com", "icloud.com", "live.com")
            work = [c for c in choices if c.get("live") and c.get("email") and not any((c["email"] or "").lower().endswith("@" + d) for d in _CONSUMER)]
            if work:
                target = work[0]["profile"]; reason = "it is a work/corporate profile (no live Autodesk session detected - CONFIRM)"
    if target:
        _remember_signin_profile(target)
        reason = reason or "you told me to use it"

    if not target:
        return {"success": True, "stage": "ask_profile", "done": False, "steps": steps,
                "signinAuthUrl": auth["url"], "profileChoices": choices,
                "_hint": ("Captured Fusion's OAuth URL but cannot tell which profile is your Autodesk "
                          "account. ASK the user which profile their Autodesk login belongs to, then "
                          "call fusion_signin {profile:'chrome:<their-account>'} - I will remember it. "
                          "profileChoices lists every profile."),
                "statusVerb": "fusion_signin"}

    tdesc = next((_describe_profile(c) for c in choices if c.get("profile") == target), target)
    import time as _tt
    sess = "fusion-signin-%d" % int(_tt.time())   # unique per attempt (a reused id can be refused)
    # ── Is Fusion ALREADY using the right profile? (John, 2026-07-22: "they both opened in my
    # work profile, so you're being lazy") ──────────────────────────────────────────────────
    # Chrome profile DIRECTORIES are named Default / Profile 1 / Profile 2, and the user's work
    # account is very often just "Default". We capture the authorize URL from a profile DIR, but
    # the target is an EMAIL (chrome:you@work.com). Without mapping email -> dir we cannot tell
    # they are the SAME profile, so we "fixed" a non-problem by re-opening the identical URL in
    # the identical profile - a redundant second sign-in window on the user's screen.
    target_dir = None
    for c in choices:
        if c.get("profile") == target:
            target_dir = c.get("profileDir") or c.get("dir")
            break
    if target_dir and auth.get("sourceProfileDir") and \
       str(target_dir).strip().lower() == str(auth["sourceProfileDir"]).strip().lower():
        steps.append("Fusion already opened the sign-in in %s (profile dir %r) - no re-open needed"
                     % (target, target_dir))
        wall0 = _detect_signin_wall()
        _email0 = args.get("email") or next((c.get("email") for c in choices if c.get("profile") == target and c.get("email")), None)
        if wall0.get("wall"):
            wall0 = _resolve_and_advance_wall(wall0, _email0)
        notified0 = bool(_notify_signin_wall(wall0["wall"]).get("delivered")) if wall0.get("wall") else False
        return {"success": True, "stage": "already_right_profile", "done": False, "steps": steps,
                "signinProfile": target, "signinProfileDir": target_dir,
                "signinWhere": tdesc if False else target,
                "signinWall": wall0.get("wall"), "signinWallHwnd": wall0.get("hwnd"),
                "notifyDelivered": notified0,
                "signinWallHint": _wall_hint(wall0["wall"], notified0) if wall0.get("wall") else None,
                "signinAuthUrl": auth["url"],
                "narrate": ("Fusion opened its sign-in in the RIGHT profile already (%s), so I did "
                            "not open a second window." % target),
                "_hint": ("NO re-open was needed - Fusion's OS-default browser IS the user's Autodesk "
                          "profile here (dir %r). Do NOT open another window; that just litters their "
                          "screen with a duplicate sign-in. The existing window is the one to finish. "
                          "%s" % (target_dir,
                                  _wall_hint(wall0["wall"], notified0) if wall0.get("wall")
                                  else "Poll fusion_readiness until ready:true.")),
                "statusVerb": "fusion_readiness"}

    # snapshot the Autodesk windows that already exist, so a STALE expired tab from an earlier
    # attempt is never mistaken for this attempt's wall
    pre_hwnds = _autodesk_window_hwnds()
    _open_args = {"sessionId": sess, "url": auth["url"], "background": True,
                  "profile": target, "thread": "fusion-signin",
                  "purpose": "Fusion Autodesk sign-in (correct profile)"}
    r = _ab_call("nbrowser_open_window", _open_args, timeout=60)
    rd_open = r.get("data", r) if isinstance(r, dict) else {}
    opened = bool(rd_open.get("sessionId") or r.get("ok") or r.get("success")
                  or rd_open.get("opened") or ("opened in the BACKGROUND" in str(r.get("_hint", ""))))
    open_via = "extension" if opened else None
    if not opened:
        # EXTENSION-FREE fallback: nbrowser_open_window needs the Adom extension installed in
        # that profile. nbrowser_open_os_window does not - it opens the URL in the real profile
        # at the OS level, which is all an OAuth consent needs. This keeps the multi-profile fix
        # working on machines with no extension (same gap as APS issue #12).
        r2 = _ab_call("nbrowser_open_os_window", _open_args, timeout=60)
        if isinstance(r2, dict) and not r2.get("_adCallError"):
            # ok:false just means the hwnd was not resolved in ~8s; the launch usually still fired
            opened = bool(r2.get("ok") or r2.get("hwnd") or "Launch fired" in str(r2.get("_hint", "")))
            if opened:
                open_via = "extension_free"
                r = r2
    steps.append((("re-opened it in %s%s" % (tdesc, " (extension-free)" if open_via == "extension_free" else ""))
                  if opened else ("could not open %s (raw: %s)" % (tdesc, str(r)[:160]))))
    if opened:
        # hand it to the janitor so it is never left as litter on the user's desktop
        _track_signin_window(sess, target, kind="fusion")

    # ── FINISH IT: drive the SSO path ourselves (no password, no 2FA, no foreground) ──────────
    # John, 2026-07-22: the goal is a SIGNED-IN Fusion, not a handed-off checklist. When we own
    # the window (nbrowser_open_window => agent-opened => CDP-drivable) and the profile is already
    # signed into the identity provider, this completes the whole login silently.
    sso = None
    if opened and open_via == "extension" and args.get("driveSso", True):
        tabs = _unwrap(_ab_call("nbrowser_list_tabs", {"profile": target}, timeout=45)).get("tabs") or []
        my_tab = None
        for t in tabs:
            if "autodesk" in str(t.get("url", "")).lower() and t.get("owner") == "agent-opened":
                my_tab = t.get("tabId") or t.get("id")
        if my_tab:
            email = None
            for c in choices:
                if c.get("profile") == target:
                    email = c.get("email")
            sso = _drive_sso_signin(target, my_tab, email, args.get("ssoProvider", "google"))
            steps.extend(sso.get("steps") or [])
            if sso.get("done"):
                for _ in range(6):
                    _t2.sleep(5)
                    if _unwrap(_handle_fusion_readiness({}, {})).get("ready"):
                        steps.append("Fusion reports ready:true - SIGNED IN")
                        # ── ONE LOGIN, BOTH SYSTEMS (John, 2026-07-22) ───────────────────────
                        # "why should the user have to login twice to autodesk to sign in to
                        # fusion and setup aps? why aren't those just driven from 1 login?"
                        # Right now the browser profile has a WARM Autodesk SSO session (we just
                        # used it). APS consent rides that session silently, so do it HERE while
                        # it is warm instead of making the user authenticate a second time.
                        aps_state = _aps_quick_state()
                        if aps_state.get("configured") and not aps_state.get("signedIn"):
                            try:
                                import aps as _apsmod
                                _apsmod.start_signin({"profile": target})
                                steps.append("APS not signed in - started its consent on the SAME "
                                             "warm SSO session (no second login for the user)")
                                for _ in range(5):
                                    _t2.sleep(4)
                                    if _aps_quick_state().get("signedIn"):
                                        steps.append("APS signed in too - one login covered both")
                                        break
                            except Exception as e:
                                steps.append("APS auto-setup skipped (%s)" % str(e)[:80])
                        aps_state = _aps_quick_state()
                        return {"success": True, "stage": "signed_in", "done": True, "steps": steps,
                                "signinProfile": target, "signinWhere": tdesc,
                                "signinOpenedVia": open_via,
                                "aps": aps_state,
                                "narrate": ("Fusion is signed in as %s. No password or 2FA was needed - "
                                            "I used the browser profile's existing SSO session.%s"
                                            % (email or target,
                                               " APS cloud search is set up too, from the same login."
                                               if aps_state.get("signedIn") else "")),
                                "_hint": ("DONE - Fusion is signed in (ready:true). Nothing else to do; "
                                          "do not tell the user to sign in. %s Drive Fusion normally now."
                                          % ("APS is signed in as well, so fusion_aps_search is live."
                                             if aps_state.get("signedIn") else
                                             "APS is NOT signed in - run fusion_aps_signin NOW while the "
                                             "browser SSO session is still warm so it consents silently.")),
                                "statusVerb": "fusion_readiness"}

    # Did the reopened sign-in immediately hit a HUMAN WALL (2FA, credentials, protocol dialog)?
    # If so, toast the user AND tell the AI how to clear it for them.
    wall = _detect_signin_wall(ignore_hwnds=pre_hwnds) if opened else {}
    _email1 = args.get("email") or next((c.get("email") for c in choices if c.get("profile") == target and c.get("email")), None)
    if wall.get("wall"):
        wall = _resolve_and_advance_wall(wall, _email1)
    wall_kind = wall.get("wall")
    notified = False
    if wall_kind:
        notified = bool(_notify_signin_wall(wall_kind).get("delivered"))
        steps.append("hit %s wall; toasted the user (delivered=%s)" % (wall_kind, notified))

    if not opened:
        # ABE-less box fallback (John, 2026-08-16: "push through as far as you can go from
        # your code"): the OAuth page is ALREADY OPEN in the browser Fusion fired (that is
        # where we captured the URL). No reopen is possible without the extension - so
        # DRIVE THE EXISTING WINDOW instead: detect the wall, type the email, advance to
        # the password step, toast the human there.
        wall_live = _detect_signin_wall()
        if wall_live.get("wall"):
            _email_fb = args.get("email")
            wall_live = _resolve_and_advance_wall(wall_live, _email_fb)
            wk = wall_live.get("wall")
            notified_fb = bool(_notify_signin_wall(wk).get("delivered")) if wk else False
            steps.append("no extension on this box - drove the EXISTING browser window instead: %s"
                         % str(wall_live.get("advance") or "wall %r" % wk))
            return {"success": True, "stage": "driving_existing_window", "done": False,
                    "steps": steps, "signinWall": wk, "signinWallHwnd": wall_live.get("hwnd"),
                    "advanced": wall_live.get("advanced"), "notifyDelivered": notified_fb,
                    "signinWallHint": _wall_hint(wk, notified_fb),
                    "signinAuthUrl": auth["url"],
                    "narrate": ("This machine has no Adom browser extension, so I drove the sign-in "
                                "window Fusion already opened: %s." % str(wall_live.get("advance") or wk)),
                    "_hint": ("No ABE here, so the EXISTING browser window was driven directly "
                              "(email typed via background UIA where possible). " +
                              _wall_hint(wk, notified_fb)),
                    "statusVerb": "fusion_readiness"}
    return {"success": True, "stage": "opened" if opened else "open_failed",
            "done": False, "steps": steps, "signinWhere": tdesc, "signinProfile": target,
            "signinOpenedVia": open_via,
            "signinWall": wall_kind, "signinWallHwnd": wall.get("hwnd"),
            "notifyDelivered": notified,
            "signinWallHint": _wall_hint(wall_kind, notified) if wall_kind else None,
            "signinProfileReason": reason, "signinSession": sess if opened else None,
            "signinWrongBrowser": "%s / %s" % (auth["browser"], auth["sourceProfileDir"]),
            "signinAuthUrl": auth["url"],
            "narrate": (("Fusion opened its sign-in in the WRONG browser (%s), so I moved the real "
                         "Autodesk sign-in URL into %s - I picked that because %s. Finish it there, "
                         "or I can drive it, and Fusion will pick up the login automatically."
                         % (auth["sourceProfileDir"], tdesc, reason)) if opened else
                        "I captured Fusion's sign-in URL but could not open the target profile."),
            "_hint": (("The REAL Fusion OAuth URL is now open in %s (session 'fusion-signin'). It "
                       "completes back to the running Fusion via the idmgr/callback + autodesk:// "
                       "protocol handoff (request_id match), so the browser profile no longer has to "
                       "be the OS default - THIS is the fix for Fusion signing into the wrong "
                       "account. TELL the user the exact profile (never 'your browser'), say WHY it "
                       "was chosen (signinProfileReason), and offer: they finish it, you foreground "
                       "it, or (with their OK) you drive 'Continue with Google/Apple/Microsoft'. "
                       "NEVER type their password/2FA. Then poll fusion_readiness until ready:true. "
                       "The wrong-browser tab (%s) can be closed. If the wrong ACCOUNT completes "
                       "anyway, sign Fusion out and re-run. Skill: fusion-multiprofile-signin."
                       % (tdesc, auth["sourceProfileDir"])) if opened else
                      "Open failed: %s. Retry, or open auth url yourself with nbrowser_open_window "
                      "{profile:'%s', url:<signinAuthUrl>}." % (r.get("_adCallError"), target)),
            "statusVerb": "fusion_signin"}




# ═══════════════════════════════════════════════════════════════════════════════
# Library SHOW verbs + bare-artifact wrapping (issue #29, John 2026-08-14):
# first-class opens for each library content type (symbol / footprint / 3D chip),
# accepting EITHER an existing .lbr + name OR a bare Fusion/EAGLE-formatted XML
# fragment which gets WRAPPED in a generated minimal library so it shows in
# Fusion's Library Manager. 3D chips ride the attach-after-open path because
# embedded <packages3d> in a hand-built .lbr is exactly the Fusion quirk that
# does not work reliably - attach post-open DOES.
# ═══════════════════════════════════════════════════════════════════════════════

_LBR_WRAP_SETTINGS = '<settings><setting alwaysvectorfont="no"/></settings>'
_LBR_WRAP_GRID = '<grid distance="0.1" unitdist="inch" unit="inch" style="lines" multiple="1" display="no" altdistance="0.01" altunitdist="inch" altunit="inch"/>'
_LBR_WRAP_LAYERS = '<layers>\n<layer number="1" name="Top" color="4" fill="1" visible="yes" active="yes"/>\n<layer number="16" name="Bottom" color="1" fill="1" visible="yes" active="yes"/>\n<layer number="17" name="Pads" color="2" fill="1" visible="yes" active="yes"/>\n<layer number="18" name="Vias" color="2" fill="1" visible="yes" active="yes"/>\n<layer number="19" name="Unrouted" color="6" fill="1" visible="yes" active="yes"/>\n<layer number="20" name="Dimension" color="15" fill="1" visible="yes" active="yes"/>\n<layer number="21" name="tPlace" color="7" fill="1" visible="yes" active="yes"/>\n<layer number="22" name="bPlace" color="7" fill="1" visible="yes" active="yes"/>\n<layer number="23" name="tOrigins" color="15" fill="1" visible="yes" active="yes"/>\n<layer number="24" name="bOrigins" color="15" fill="1" visible="yes" active="yes"/>\n<layer number="25" name="tNames" color="7" fill="1" visible="yes" active="yes"/>\n<layer number="26" name="bNames" color="7" fill="1" visible="yes" active="yes"/>\n<layer number="27" name="tValues" color="7" fill="1" visible="yes" active="yes"/>\n<layer number="28" name="bValues" color="7" fill="1" visible="yes" active="yes"/>\n<layer number="29" name="tStop" color="7" fill="3" visible="no" active="yes"/>\n<layer number="30" name="bStop" color="7" fill="6" visible="no" active="yes"/>\n<layer number="31" name="tCream" color="7" fill="4" visible="no" active="yes"/>\n<layer number="32" name="bCream" color="7" fill="5" visible="no" active="yes"/>\n<layer number="33" name="tFinish" color="6" fill="3" visible="no" active="yes"/>\n<layer number="34" name="bFinish" color="6" fill="6" visible="no" active="yes"/>\n<layer number="35" name="tGlue" color="7" fill="4" visible="no" active="yes"/>\n<layer number="36" name="bGlue" color="7" fill="5" visible="no" active="yes"/>\n<layer number="37" name="tTest" color="7" fill="1" visible="no" active="yes"/>\n<layer number="38" name="bTest" color="7" fill="1" visible="no" active="yes"/>\n<layer number="39" name="tKeepout" color="4" fill="11" visible="no" active="yes"/>\n<layer number="40" name="bKeepout" color="1" fill="11" visible="no" active="yes"/>\n<layer number="41" name="tRestrict" color="4" fill="10" visible="no" active="yes"/>\n<layer number="42" name="bRestrict" color="1" fill="10" visible="no" active="yes"/>\n<layer number="43" name="vRestrict" color="2" fill="10" visible="no" active="yes"/>\n<layer number="44" name="Drills" color="7" fill="1" visible="no" active="yes"/>\n<layer number="45" name="Holes" color="7" fill="1" visible="no" active="yes"/>\n<layer number="46" name="Milling" color="3" fill="1" visible="no" active="yes"/>\n<layer number="47" name="Measures" color="7" fill="1" visible="no" active="yes"/>\n<layer number="48" name="Document" color="7" fill="1" visible="yes" active="yes"/>\n<layer number="49" name="Reference" color="7" fill="1" visible="yes" active="yes"/>\n<layer number="91" name="Nets" color="2" fill="1" visible="yes" active="yes"/>\n<layer number="92" name="Busses" color="1" fill="1" visible="yes" active="yes"/>\n<layer number="93" name="Pins" color="2" fill="1" visible="no" active="yes"/>\n<layer number="94" name="Symbols" color="4" fill="1" visible="yes" active="yes"/>\n<layer number="95" name="Names" color="7" fill="1" visible="yes" active="yes"/>\n<layer number="96" name="Values" color="7" fill="1" visible="yes" active="yes"/>\n<layer number="97" name="Info" color="7" fill="1" visible="yes" active="yes"/>\n<layer number="98" name="Guide" color="6" fill="1" visible="yes" active="yes"/>\n</layers>'


def _lbr_wrap_build(name, symbol_xml=None, package_xml=None):
    """Build a minimal, Fusion-parseable EAGLE .lbr wrapping bare artifacts.

    A deviceset is generated only when a symbol exists (a gate needs a symbol);
    a bare footprint still lists under Packages. 3D packages are NEVER embedded
    here (the known-flaky path); use stepPath on show_3d_package to attach after
    the open instead.
    """
    symbols = symbol_xml or ""
    packages = package_xml or ""
    devicesets = ""
    if symbol_xml:
        connects = ""
        variant = '<device name=""/>'
        if package_xml:
            pkg_m = re.search(r'<package\s+name="([^"]+)"', package_xml)
            pkg_name = pkg_m.group(1) if pkg_m else name
            variant = '<device name="" package="%s"><technologies><technology name=""/></technologies></device>' % pkg_name
        devicesets = (
            '<deviceset name="%s" prefix="U"><description>Adom wrapper</description>'
            '<gates><gate name="G$1" symbol="%s" x="0" y="0"/></gates>'
            '<devices>%s</devices></deviceset>' % (name, name, variant)
        )
    return (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<!DOCTYPE eagle SYSTEM "eagle.dtd">\n'
        '<eagle version="9.6.2">\n<drawing>\n'
        + _LBR_WRAP_SETTINGS + '\n' + _LBR_WRAP_GRID + '\n' + _LBR_WRAP_LAYERS + '\n'
        + '<library>\n<description>Adom wrapper library: ' + name + '</description>\n'
        + '<packages>' + packages + '</packages>\n'
        + '<symbols>' + symbols + '</symbols>\n'
        + '<devicesets>' + devicesets + '</devicesets>\n'
        + '</library>\n</drawing>\n</eagle>\n'
    )


def _lbr_wrap_write(name, symbol_xml=None, package_xml=None):
    """Write a wrapper .lbr to the wrap dir; returns (path, None) or (None, err)."""
    import os
    import tempfile
    for frag, kind, tag in ((symbol_xml, "symbolXml", "symbol"), (package_xml, "packageXml", "package")):
        if frag and not re.match(r"\s*<" + tag + r"[\s>]", frag):
            return None, ("%s must be a bare <%s ...>...</%s> XML fragment "
                          "(got something else). To open a whole library, pass libraryPath instead." % (kind, tag, tag))
    safe = re.sub(r"[^A-Za-z0-9_-]", "_", name)[:60] or "wrapped"
    d = os.path.join(tempfile.gettempdir(), "adom-fusion-wrap")
    os.makedirs(d, exist_ok=True)
    path = os.path.join(d, safe + ".lbr").replace("\\", "/")
    try:
        _note_bridge_doc(safe)  # generated wrapper = bridge doc (sentinel may discard its edits)
    except Exception:
        pass
    try:
        with open(path, "w", encoding="utf-8") as f:
            f.write(_lbr_wrap_build(name, symbol_xml, package_xml))
    except Exception as e:
        return None, "could not write wrapper library: " + str(e)
    return path, None


def _artifact_name_from(args, *frag_keys):
    """Resolve the artifact name: explicit arg wins, else parsed from the fragment."""
    name = (args.get("name") or "").strip()
    if name:
        return name
    for k in frag_keys:
        frag = args.get(k) or ""
        m = re.search(r'name="([^"]+)"', frag)
        if m:
            return m.group(1)
    return ""



def _remember_open_library(lbr_path: str):
    """Record the .lbr we just opened so LATER verbs can pre-check names without a path.

    ⛔ THIS EXISTS TO PREVENT A MODAL, NOT TO BE TIDY (John, 2026-08-26, third report of the
    same dialog). `_resolve_artifact_name` could always stop a Create-new prompt, but it
    needs the .lbr on disk, and `fusion_capture_library_views` never took a libraryPath - so
    the ONE verb that EDITs the most names was the one verb structurally unable to run the
    guard, and it "handled" the prompt by raising it and dismissing it afterwards. Persisted
    (not just module state) because ab reaps and respawns this bridge between calls, which
    would drop in-memory state exactly when a capture follows an open.
    """
    # LOCAL imports + a LOUD except. Both matter: this module imports os/time inside
    # functions, so a module-level `time.time()` here raised NameError - and the bare
    # `except: pass` I first wrote ATE it, leaving the guard silently disarmed while the
    # code read as correct. A prevention guard that fails quietly is worse than none,
    # because it removes the pressure to notice.
    try:
        import pathlib as _pl, json as _js, os as _os, time as _tm
        d = _pl.Path(_os.path.expanduser("~")) / ".adom" / "fusion-bridge"
        d.mkdir(parents=True, exist_ok=True)
        (d / "open_library.json").write_text(_js.dumps(
            {"libraryPath": lbr_path.replace("\\", "/"), "at": _tm.time()}))
    except Exception as e:
        print("[Fusion Bridge] ⚠ could not remember the open library (%s: %s) - the "
              "Create-new-dialog pre-check will fall back to SET CONFIRM only"
              % (type(e).__name__, e))


def _remembered_open_library():
    """The last .lbr opened, if it is still on disk. None when we genuinely do not know."""
    try:
        import pathlib as _pl, json as _js, os as _os
        f = _pl.Path(_os.path.expanduser("~")) / ".adom" / "fusion-bridge" / "open_library.json"
        if not f.exists():
            return None
        lp = (_js.loads(f.read_text()) or {}).get("libraryPath") or ""
        return lp if lp and _os.path.exists(lp) else None
    except Exception as e:
        print("[Fusion Bridge] ⚠ could not read the remembered library (%s: %s)"
              % (type(e).__name__, e))
        return None


def _library_artifact_names(lbr_path: str) -> dict:
    """The names that actually EXIST in a .lbr: {"sym": [...], "pac": [...], "dev": [...]}.

    EAGLE/Fusion libraries are XML, so the <symbol>/<package>/<deviceset> name attributes
    are the source of truth. Empty dict when the file is unreadable - callers must treat
    "I could not check" as "do not guess", never as "it is missing".
    """
    try:
        import xml.etree.ElementTree as _ET
        root = _ET.parse(lbr_path).getroot()
    except Exception:
        return {}
    def names(tag):
        return [e.get("name") for e in root.iter(tag) if e.get("name")]
    # Deviceset -> its gate symbols / device packages. A deviceset's symbol and footprint
    # are NOT required to share its name (the wiki's ABM8-16MHZ sits on symbol+package
    # "Crystal_GND24", SRN6045-100M on package "L"), so a prove step that EDITs
    # <deviceset>.sym must follow the gate, not guess the name (2026-08-29: three parts of
    # a ten-part library "not proven" for exactly this).
    dev_sym, dev_pac = {}, {}
    for ds in root.iter("deviceset"):
        dn = ds.get("name")
        if not dn:
            continue
        dev_sym[dn] = sorted({g.get("symbol") for g in ds.iter("gate") if g.get("symbol")})
        dev_pac[dn] = sorted({d.get("package") for d in ds.iter("device") if d.get("package")})
    return {"sym": names("symbol"), "pac": names("package"), "dev": names("deviceset"),
            "p3d": [e.get("name") for e in root.iter("package3d") if e.get("name")],
            "dev_sym": dev_sym, "dev_pac": dev_pac}


def _resolve_artifact_name(name: str, suffix: str, lbr_path: str):
    """Resolve `name` to something that EXISTS, or say it does not - WITHOUT asking Fusion.

    ⚠️ PREVENTION, NOT CLEANUP (John, 2026-08-21: "you should never allow dialogs like that
    to ever even show cuz they screw you up"). `EDIT <name>.<suffix>` on a name that is not
    in the library pops Fusion's "Create new footprint 'X'?" modal, which blocks the add-in's
    main thread until somebody answers it - so a typo became a hang, and every later verb
    read as "add-in not responding". This bridge KNEW that: the old hint here told callers to
    run fusion_dismiss_blocking_dialogs afterwards. Telling the caller to sweep up a mess we
    chose to make is exactly backwards.

    Returns (resolved_name, None) or (None, error_dict). Exact match wins; then a unique
    case-insensitive match, because EAGLE names round-trip through tools that change case.
    """
    _names = _library_artifact_names(lbr_path)
    have = _names.get(suffix if suffix in ("sym", "pac", "dev") else "", [])
    if have and name not in have and suffix in ("sym", "pac"):
        # The caller named the DEVICESET (the part); follow it to the one symbol / package
        # it actually uses. Ambiguous (multi-gate, multi-package) falls through to the
        # matching below and its honest error.
        via = (_names.get("dev_sym" if suffix == "sym" else "dev_pac") or {}).get(name) or []
        if len(via) == 1 and via[0] in have:
            print("[Fusion Bridge] resolved %s.%s via its deviceset -> %s" % (name, suffix, via[0]))
            return via[0], None
    if not have:
        # REFUSE, do not wave it through (reversed 2026-08-21, John caught the prompt
        # live): "cannot read the library -> do not block" meant a corrupt .lbr let the
        # EDIT reach Fusion, which popped "Create new footprint 'X'?" - the exact modal
        # this resolver exists to prevent. An unreadable or artifact-less library makes
        # the prompt CERTAIN, so certainty of a prompt is exactly when to block.
        return None, {
            "success": False, "errorCode": "library_unreadable",
            "error": "Cannot read any %s names from %s - the library is unparseable or "
                     "empty, so EDIT was NOT issued (it would certainly raise Fusion's "
                     "Create-new prompt)." % (suffix, lbr_path),
            "_hint": "Validate the .lbr first (fusion_open_lbr now refuses garbage with "
                     "lbr_invalid + the file size/head). Re-stage the file from its source "
                     "and compare hashes."}
    if name in have:
        return name, None
    ci = [h for h in have if h.lower() == str(name).lower()]
    if len(ci) == 1:
        return ci[0], None
    # A CALLER SHOULD NEVER PARSE OUR FILE FORMAT TO TALK TO US (page issue #42). ab ended up
    # regexing <symbol name="..."> out of the .lbr because the trailing character differed
    # (file CL21B104KBCNFNC.lbr vs deviceset CL21B104KBCNFN). We have the library open and we
    # know the names, so resolve the near-miss ourselves and, when we cannot, put the names in
    # the error so the retry is obvious without reading XML.
    import difflib as _dl
    close = _dl.get_close_matches(str(name), have, n=5, cutoff=0.6)
    stem = str(name).rsplit("/", 1)[-1].rsplit(".", 1)[0]
    pref = [h for h in have if h.lower().startswith(stem.lower()[:max(4, len(stem) - 2)])]
    if len(close) == 1 and close[0].lower() != str(name).lower():
        print("[Fusion Bridge] resolved near-miss %r -> %r (issue #42)" % (name, close[0]))
        return close[0], None
    if len(pref) == 1:
        print("[Fusion Bridge] resolved by stem %r -> %r (issue #42)" % (name, pref[0]))
        return pref[0], None
    return None, {
        "success": False,
        "error": "No %s named %r in that library, so the EDIT was NOT issued." % (
            {"sym": "symbol", "pac": "footprint", "dev": "device"}.get(suffix, suffix), name),
        "errorCode": "artifact_not_in_library",
        "available": have[:40],
        "closestMatches": close,
        "_hint": ("Deliberately refused BEFORE touching Fusion: EDIT on a missing name pops a "
                  "'Create new ...?' modal that blocks the add-in's main thread and makes every "
                  "later verb look like a crash. Pick a name from `available` (they are the "
                  "<symbol>/<package>/<deviceset> name attributes in the .lbr, the source of "
                  "truth) and re-call. Nothing was opened and no dialog was raised."),
    }


def _show_library_view(name, suffix, lbr_path=None):
    """EDIT to a library view + fit + background screenshot. Library must be open.

    Verifies the name EXISTS first when we know the .lbr, so Fusion is never asked to open
    something that would make it prompt (see _resolve_artifact_name).
    """
    import time
    if lbr_path:
        resolved, err = _resolve_artifact_name(name, suffix, lbr_path)
        if err:
            return err
        if resolved != name:
            print("[Fusion Bridge] resolved %r -> %r from the library (avoids a Create-new prompt)"
                  % (name, resolved))
            name = resolved
        # ⚠️ EDIT DRIVES THE ACTIVE DOCUMENT, NOT A PATH (page issue #47, root cause of
        # failure modes 1 AND 2): in a batch, several libraries (and f3ds) are open at
        # once, and EDIT lands in whichever is active. The artifact scan read the right
        # FILE while EDIT drove the WRONG document - so Fusion offered to CREATE a part
        # that plainly existed, or refused with "Not in Electronics editor". Make the
        # target library active, verified, before every EDIT.
        act = _ensure_library_active(lbr_path, timeout_s=30)
        if not act.get("ok"):
            # One honest retry (run 93ccf05278ac, 2026-08-29): between two frames of the same
            # part Fusion reported NO active document for a while (a cloud job settling), the
            # footprint frame was skipped and a good library scored not_proven. If the
            # library is not open at all, reopen it from disk; then give it another 60s.
            import time as _tra
            _tra.sleep(8)
            try:
                _lstem = lbr_path.replace("\\", "/").rsplit("/", 1)[-1].rsplit(".", 1)[0].lower()
                _ln = _open_doc_names()
                _lb = {re.sub(r"\s*\([^)]*\)\s*$", "", n.replace("*", "")).strip().lower() for n in _ln}
                if _ln and _lstem not in _lb:      # known-closed only; never reopen on "unknown"
                    _orchestrate_open_lbr({"filePath": lbr_path})
            except Exception:
                pass
            act = _ensure_library_active(lbr_path, timeout_s=60)
        if not act.get("ok"):
            return {"success": False, "errorCode": "library_not_active",
                    "error": "Could not make %s the ACTIVE document (active: %r), so EDIT "
                             "was not issued - it would have driven the wrong library." % (
                                 lbr_path.rsplit("/", 1)[-1], act.get("activeDocument")),
                    "_hint": "fusion_open_lbr the library, then re-call. In a batch, close "
                             "the previous part's documents first (the create_library chain "
                             "now does this itself)."}
    # BUSY-RETRY (matrix round 2026-08-21, ads1220irvar lost 2 frames to "add-in is BUSY"):
    # on a shared box another caller's long command can hold the main thread exactly when
    # a capture's EDIT lands. Busy is transient by definition - wait it out (up to 90s)
    # instead of failing the frame.
    edit = None
    for _busy_try in range(7):
        edit = _proxy_to_addin("electron_run", {"command": "EDIT %s.%s" % (name, suffix),
                                             "confirm": "no"}, timeout=15)
        _etxt = str(edit.get("error") or "")
        if edit.get("success"):
            break
        # NOT-IN-ELECTRONICS-EDITOR RETRY (#49 parts 6/7, 2026-08-23): the 3D step can leave
        # Fusion on the Design workspace, and even after _ensure_library_active a stray EDIT
        # can land a beat before the switch completes. Re-ensure the library editor and retry
        # rather than losing the frame - the same "transient, wait it out" law as BUSY.
        if lbr_path and ("not in electronics editor" in _etxt.lower()
                         or "current: design" in _etxt.lower()):
            print("[Fusion Bridge] EDIT %s.%s hit Design workspace (attempt %d) - re-entering "
                  "the library editor and retrying" % (name, suffix, _busy_try + 1))
            _ensure_library_active(lbr_path, timeout_s=20)
            time.sleep(1.0)
            continue
        if "BUSY" not in _etxt and "busy" not in _etxt:
            break
        print("[Fusion Bridge] EDIT %s.%s deferred - main thread busy (attempt %d), waiting 15s"
              % (name, suffix, _busy_try + 1))
        time.sleep(15)
    # WE MAY NOT HAVE BEEN ABLE TO PRE-CHECK (no libraryPath, nothing remembered, or the
    # library was opened by another route). If Fusion prompted anyway, answer it HERE - a
    # modal we caused must never outlive the call that caused it, and it is holding the main
    # thread hostage while it sits there (John saw this dialog twice, 2026-08-21).
    try:
        _pending = [d for d in (classify_blocking_dialogs() or [])
                    if d.get("category") == "create_confirm"]
        if _pending:
            # A create_confirm on screen is not automatically OURS (run 7d4427d443f5: a
            # footprint EDIT whose name WAS in the library got failed because a stale
            # Create-new prompt from an earlier interaction was still standing). Read the
            # dialog's own text: only a prompt that names THIS artifact fails THIS call.
            # Foreign/stale prompts are still declined (never left blocking the main
            # thread), but the EDIT's own outcome decides the result.
            _about_this = False
            for _d in _pending:
                try:
                    from handlers.dialog_classify import _dialog_text as _dtxt
                    _body = "%s %s" % (_dtxt(_d.get("hwnd")) or "", _d.get("title") or "")
                except Exception:
                    _body = _d.get("title") or ""
                if str(name).lower() in _body.lower():
                    _about_this = True
                _unwrap(_ab_call("desktop_ui_click", {"hwnd": _d.get("hwnd"), "name": "No"}, timeout=12))
            if not _about_this:
                print("[Fusion Bridge] declined a STALE Create-new prompt that does not mention "
                      "%r - clearing it and trusting the EDIT's own result" % name)
        if _pending and _about_this:
            print("[Fusion Bridge] declined a Create-new prompt for %r - the name is not in the "
                  "library (answering Yes would fabricate an empty artifact)" % name)
            return {
                "success": False,
                "error": "No %s named %r in the open library. Fusion offered to CREATE it; the "
                         "bridge declined." % ({"sym": "symbol", "pac": "footprint",
                                                "dev": "device"}.get(suffix, suffix), name),
                "errorCode": "artifact_not_in_library",
                "promptDeclined": True,
                "_hint": ("The prompt was answered (No) inside this call, so nothing is left "
                          "blocking Fusion's main thread. Answering Yes would have created an "
                          "EMPTY artifact and made this look like it worked. Pass libraryPath so "
                          "the bridge can verify names up front and never raise the prompt at all."),
            }
    except Exception:
        pass
    if not edit.get("success"):
        return {"success": False, "error": "EDIT %s.%s failed: %s" % (name, suffix, edit.get("error")),
                "_hint": ("The name must match the deviceset/artifact inside the OPEN library exactly "
                          "(EDIT on a missing name pops a 'Create new?' dialog - run "
                          "fusion_dismiss_blocking_dialogs if one appeared). Not sure of the name? "
                          "fusion_get_app_state shows the active document; the .lbr's <symbol>/<package> "
                          "name attributes are the source of truth.")}
    _proxy_to_addin("electron_run", {"command": "WINDOW FIT"}, timeout=8)
    time.sleep(1.0)
    # REPORT WHAT WE ACTUALLY OPENED (page issue #40/#42, and kicad's shape: it returns
    # symbolNames / footprintName). ab's client was reading our .lbr to learn what to ask
    # for; the cure is that the verb SAYS what it opened and what else is in there, so the
    # next call needs no discovery step and no file parsing.
    out = {"success": True, "view": suffix, "name": name, "resolvedName": name}
    if lbr_path:
        _n = _library_artifact_names(lbr_path)
        if _n:
            out["libraryPath"] = lbr_path
            out["symbolNames"] = _n.get("sym", [])
            out["packageNames"] = _n.get("pac", [])
            out["devicesetNames"] = _n.get("dev", [])
            out["package3dNames"] = _n.get("p3d", [])
            out["has3d"] = bool(_n.get("p3d"))
    try:
        shot = _handle_screenshot_fusion({}, {})
        p = shot.get("savedTo") or shot.get("localSafePath")
        if shot.get("success") and p:
            out["screenshot"] = p
    except Exception:
        pass
    return out


def _last_library() -> str:
    """The .lbr we most recently opened, across respawns."""
    if _LAST_LIBRARY[0]:
        return _LAST_LIBRARY[0]
    try:
        return _get_bridge_pref("lastLibraryPath", "") or ""
    except Exception:
        return ""


_LAST_LIBRARY = [""]          # the .lbr most recently opened by this bridge


def _remember_library(path: str):
    """Record the library BOTH in memory and in the persisted pref.

    Setting only _LAST_LIBRARY[0] looks identical in every test that does not restart the
    bridge, and then a respawn (ab auto-updates the bridge whenever a new version is
    published, including MID-RUN) silently forgets it - after which prove_library refuses
    with library_path_required even though the library is open on screen. Seen live
    2026-09-01, resuming a run the publisher's own release had orphaned."""
    _LAST_LIBRARY[0] = path
    try:
        _set_bridge_pref("lastLibraryPath", path)
    except Exception:
        pass


def _show_prepare_library(args, symbol_xml=None, package_xml=None, name=""):
    """Open the target library: explicit path, generated wrapper, or already-open.

    Returns (errdict|None, wrapped_path|None)."""
    library_path = (args.get("libraryPath") or "").replace("\\", "/")
    wrapped = None
    if symbol_xml or package_xml:
        if library_path:
            return {"success": False,
                    "error": "Pass EITHER libraryPath OR a bare XML fragment, not both.",
                    "_hint": "libraryPath opens an existing library; a fragment gets wrapped in a generated one."}, None
        wrapped, err = _lbr_wrap_write(name, symbol_xml, package_xml)
        if err:
            return {"success": False, "error": err}, None
        library_path = wrapped
    if library_path:
        opened = _orchestrate_open_lbr({"filePath": library_path})
        if not opened.get("success"):
            return {"success": False, "error": "open_lbr failed: " + str(opened.get("error")),
                    "data": {"libraryPath": library_path, "wrapped": bool(wrapped)}}, None
        # REMEMBER IT. The name-only call path ("the library is already open") had no .lbr to
        # check against, so the Create-new guard silently did nothing and the blind EDIT went
        # out anyway - which is why John saw the dialog AGAIN after the first fix. Whatever we
        # last opened IS the library those calls mean.
        _remember_library(library_path)   # survives a bridge respawn
    return None, wrapped


def _orchestrate_show_symbol(args: dict) -> dict:
    """Open a schematic SYMBOL in Fusion's library editor.

    args: name (deviceset/symbol name), plus ONE of:
      libraryPath - existing .lbr to open first (omit if the library is already open)
      symbolXml   - a bare <symbol ...>...</symbol> fragment; a wrapper library is
                    generated around it so it shows in the Library Manager.
    """
    symbol_xml = args.get("symbolXml")
    name = _artifact_name_from(args, "symbolXml")
    if not name:
        return {"success": False, "error": "name required (or a symbolXml fragment with a name attribute)."}
    err, wrapped = _show_prepare_library(args, symbol_xml=symbol_xml, name=name)
    if err:
        return err
    out = _show_library_view(name, "sym", wrapped or (args.get("libraryPath") or "").replace("\\", "/") or _last_library())
    if wrapped and out.get("success"):
        out["wrappedLibrary"] = wrapped
        out["_hint"] = ("Bare symbol was wrapped in a generated library at %s (a deviceset with one "
                       "gate was synthesized so the Library Manager lists it). To pair it with a "
                       "footprint, re-call with both symbolXml and packageXml." % wrapped)
    return out


def _orchestrate_show_footprint(args: dict) -> dict:
    """Open a FOOTPRINT (package) in Fusion's library editor.

    args: name, plus ONE of libraryPath | packageXml (bare <package> fragment,
    wrapped in a generated library)."""
    package_xml = args.get("packageXml")
    name = _artifact_name_from(args, "packageXml")
    if not name:
        return {"success": False, "error": "name required (or a packageXml fragment with a name attribute)."}
    err, wrapped = _show_prepare_library(args, package_xml=package_xml, name=name)
    if err:
        return err
    out = _show_library_view(name, "pac", wrapped or (args.get("libraryPath") or "").replace("\\", "/") or _last_library())
    if wrapped and out.get("success"):
        out["wrappedLibrary"] = wrapped
        out["_hint"] = "Bare footprint was wrapped in a generated library at " + wrapped + "."
    return out


_SHOW_3D_FALLBACK_PACKAGE = (
    '<package name="%s"><description>Adom auto footprint (3D carrier)</description>'
    '<smd name="1" x="-1.27" y="0" dx="1.2" dy="1.2" layer="1"/>'
    '<smd name="2" x="1.27" y="0" dx="1.2" dy="1.2" layer="1"/>'
    '<wire x1="-2.2" y1="1.6" x2="2.2" y2="1.6" width="0.127" layer="21"/>'
    '<wire x1="-2.2" y1="-1.6" x2="2.2" y2="-1.6" width="0.127" layer="21"/>'
    '<text x="-2.2" y="2" size="1" layer="25">&gt;NAME</text></package>'
)


def _orchestrate_show_3d_package(args: dict) -> dict:
    """Open the 3D CHIP (Package3DEnvironment) for a library package; optionally
    attach a STEP model first (the reliable add-3D-AFTER-the-open path - embedded
    <packages3d> in hand-built libraries is the Fusion quirk that fails).

    args: name, then:
      libraryPath           - existing .lbr (with packageXml/stepPath absent, just shows it)
      packageXml            - bare <package> fragment to wrap (footprint carrier)
      stepPath              - Windows path to a STEP; attached onto the footprint via the
                              attach machinery. With NO packageXml/libraryPath, a minimal
                              2-pad carrier footprint is generated so the STEP has a home.
    """
    package_xml = args.get("packageXml")
    step_path = (args.get("stepPath") or "").replace("\\", "/")
    name = _artifact_name_from(args, "packageXml") or (
        re.sub(r"[^A-Za-z0-9_-]", "_", step_path.rsplit("/", 1)[-1].rsplit(".", 1)[0])[:60] if step_path else "")
    if not name:
        return {"success": False, "error": "name required (or derivable from packageXml/stepPath)."}
    library_path = (args.get("libraryPath") or "").replace("\\", "/")
    wrapped = None
    if not library_path:
        if not package_xml and step_path:
            package_xml = _SHOW_3D_FALLBACK_PACKAGE % name
        if not package_xml:
            return {"success": False,
                    "error": "Need a footprint to host the 3D: pass libraryPath, packageXml, or stepPath.",
                    "_hint": "A 3D package lives ON a footprint. stepPath alone works: a minimal 2-pad carrier footprint is generated."}
        wrapped, werr = _lbr_wrap_write(name, package_xml=package_xml)
        if werr:
            return {"success": False, "error": werr}
        library_path = wrapped
    if step_path:
        res = _merge_dialog_array(_orchestrate_attach_3d_package(
            {"filePath": library_path, "modelPath": step_path, "packageName": name}))
        # THE ATTACH-FAILED BRANCH OF THE PLACEHOLDER GUARD (page issue #44): when the
        # attach we just attempted did not actually land a <package3d> in the library,
        # what renders next is Fusion's grey Placeholder - and this verb used to return
        # the attach result's success over it. Same offline check as no_3d_package_bound,
        # run on the branch where the bridge TRIED and failed rather than was never asked.
        _p3d_after = _library_artifact_names(library_path).get("p3d", [])
        if not _p3d_after:
            _up_err = str(res.get("error") or res.get("attachError") or "attach reported no error but bound nothing")
            return {"success": False,
                    "errorCode": "3d_attach_failed",
                    "error": "Attaching the STEP failed, so the library still has no 3D "
                             "package - refusing to show Fusion's Placeholder as if it "
                             "were the part.",
                    "attachError": _up_err[:400],
                    "libraryPath": library_path,
                    "_hint": ("The upstream failure is in attachError (a 409 here usually "
                              "means the f3d already exists in the cloud - the bind path "
                              "reuses it as of 1.9.11x, so retry fusion_bind_library_3d "
                              "and read ITS message). Nothing was rendered because a "
                              "Placeholder screenshot photographs exactly like a real "
                              "chip and must never count as one.")}
        if wrapped:
            res["wrappedLibrary"] = wrapped
        res["view"] = "3d-package"
        return res
    opened = _orchestrate_open_lbr({"filePath": library_path})
    if not opened.get("success"):
        return {"success": False, "error": "open_lbr failed: " + str(opened.get("error"))}
    # ⚠️ A PLACEHOLDER IS NOT A RESULT (page issue #42). With no <package3d> bound, Fusion
    # renders its grey Placeholder slab and this verb used to return success - ab's card
    # captured that, counted three artifacts, and reported the library PROVEN while a human
    # looking at the screenshot saw a featureless block where their capacitor should be.
    # Technically truthful, practically a false pass. Same class as a 237x39 "screenshot":
    # a non-result reported as success. The .lbr answers this offline, before we render.
    _p3d = _library_artifact_names(library_path).get("p3d", [])
    if not _p3d:
        return {"success": False,
                "error": "That library has no 3D package bound, so the only thing to show is "
                         "Fusion's grey Placeholder - refusing to report that as a 3D package.",
                "errorCode": "no_3d_package_bound",
                "libraryPath": library_path,
                "_hint": ("Bind a real model first: re-call fusion_show_3d_package with "
                          "stepPath pointing at the STEP file and the bridge attaches it onto "
                          "the footprint, then shows it. A Placeholder rendering LOOKS like a "
                          "successful 3D view in a screenshot, which is why this refuses "
                          "rather than letting a caller count it as proof.")}
    cp = _create_3d_package(library_path, timeout=60)
    if not cp.get("success"):
        return {"success": False, "error": "Create3DPackage failed: " + str(cp.get("error")),
                "_hint": "The library must be Fusion-parseable (adom-lbr style); raw vendor .lbr files fail Fusion's parser."}
    out = {"success": True, "view": "3d-package", "name": name, "libraryPath": library_path}
    try:
        shot = _handle_screenshot_fusion({}, {})
        p = shot.get("savedTo") or shot.get("localSafePath")
        if shot.get("success") and p:
            out["screenshot"] = p
    except Exception:
        pass
    out["_hint"] = ("You are in the Package3DEnvironment. To leave WITHOUT saving, run "
                    "fusion_execute_text_command with Package3DStop; the bridge answers the FINISH Save dialog itself (never you) "
                    "(fusion_dismiss_blocking_dialogs). To BIND a real 3D model here, re-call with stepPath.")
    return out


# ── fusion_demo tour mode: the six-content-type showcase with structured progress ──

_DEMO_TOURS: dict = {}
_DEMO_TOURS_LOCK = threading.Lock()


def _demo_tour_dir():
    import pathlib
    d = pathlib.Path(os.path.expanduser("~")) / ".adom" / "fusion-bridge" / "tours"
    d.mkdir(parents=True, exist_ok=True)
    return d


def _demo_tour_persist(state: dict):
    """Snapshot a FINISHED tour to disk (ab's #30 grace-window ask, 2026-08-16): their
    2.5s poller got 'unknown demoId' for the final frame because a bridge_install
    respawned this process between polls, wiping the in-memory dict - tours were never
    reaped, the PROCESS was. The disk snapshot makes the done frame (with its shots)
    pollable across respawns; keep the last 8, best-effort."""
    try:
        d = _demo_tour_dir()
        snap = {k: v for k, v in state.items() if k != "started"}
        (d / (str(state.get("demoId")) + ".json")).write_text(json.dumps(snap))
        old = sorted(d.glob("tour-*.json"), key=lambda p: p.stat().st_mtime)
        for p in old[:-8]:
            p.unlink()
    except Exception:
        pass


def _demo_tour_load(demo_id: str):
    try:
        p = _demo_tour_dir() / (demo_id + ".json")
        if p.is_file():
            return json.loads(p.read_text())
    except Exception:
        pass
    return None


def _bmp_downscale_png(path: str, max_w: int = 640):
    """Pure-STDLIB BMP -> small PNG (nearest-neighbor). Exists because ab's provisioned
    python has NO PIL, so screenshot captures fall back to raw 24MB BMPs - and a 24MB BMP
    is 31MB as base64, which no relay frame should ever carry (caught live 2026-08-16:
    every tour stepShot silently skipped, and the done gallery would have been ~280MB).
    BMP is uncompressed BGR rows and PNG needs only zlib+struct, so no dependency."""
    import struct
    import zlib
    raw = open(path, "rb").read()
    if raw[:2] != b"BM":
        raise ValueError("not a BMP")
    pix_off = struct.unpack_from("<I", raw, 10)[0]
    w = struct.unpack_from("<i", raw, 18)[0]
    h = struct.unpack_from("<i", raw, 22)[0]
    bpp = struct.unpack_from("<H", raw, 28)[0]
    comp = struct.unpack_from("<I", raw, 30)[0]
    if comp != 0 or bpp not in (24, 32) or w <= 0 or h == 0:
        raise ValueError("unsupported BMP (bpp=%s comp=%s)" % (bpp, comp))
    bottom_up = h > 0
    h = abs(h)
    bypp = bpp // 8
    stride = ((w * bypp) + 3) & ~3
    ow = min(max_w, w)
    oh = max(1, int(h * ow / w))
    xs = [int(x * w / ow) * bypp for x in range(ow)]
    rows = []
    for y in range(oh):
        sy = int(y * h / oh)
        row_off = pix_off + ((h - 1 - sy) if bottom_up else sy) * stride
        line = bytearray(1 + ow * 3)  # PNG filter byte 0 + RGB
        i = 1
        for xo in xs:
            o = row_off + xo
            line[i] = raw[o + 2]      # R (BMP stores BGR)
            line[i + 1] = raw[o + 1]  # G
            line[i + 2] = raw[o]      # B
            i += 3
        rows.append(bytes(line))
    def chunk(tag, data):
        return (struct.pack(">I", len(data)) + tag + data
                + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))
    ihdr = struct.pack(">IIBBBBB", ow, oh, 8, 2, 0, 0, 0)
    idat = zlib.compress(b"".join(rows), 6)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat)
            + chunk(b"IEND", b""))


def _shot_b64_small(path: str, max_w: int = 640):
    """Base64 evidence frame for ab's stepShot contract (~640px wide). PIL downscale when
    available; the stdlib BMP downscaler when not (ab's python ships without PIL); else
    the raw file if it is small enough for the relay envelope."""
    import base64
    try:
        try:
            from PIL import Image
            import io
            im = Image.open(path)
            if im.width > max_w:
                im = im.resize((max_w, max(1, int(im.height * max_w / im.width))))
            buf = io.BytesIO()
            im.convert("RGB").save(buf, format="PNG")
            return base64.b64encode(buf.getvalue()).decode()
        except ImportError:
            pass
        if path.lower().endswith(".bmp"):
            return base64.b64encode(_bmp_downscale_png(path, max_w)).decode()
        raw = open(path, "rb").read()
        if len(raw) <= 1_500_000:
            return base64.b64encode(raw).decode()
    except Exception:
        pass
    return None


def _os_path_sibling(path, ext):
    """<same dir>/<same stem><ext> if it exists, else None."""
    try:
        import os as _o
        cand = _o.path.splitext(path)[0] + ext
        return cand.replace("\\", "/") if _o.path.isfile(cand) else None
    except Exception:
        return None


def _demo_tour_sample_lbr():
    """The bundled sample library staged into the wrap dir (self-sufficient demo data)."""
    import os
    import shutil
    import tempfile
    src = os.path.join(os.path.dirname(os.path.abspath(__file__)), "resources", "samples", "ADS8588SIPM.fusion.lbr")
    if not os.path.isfile(src):
        return None
    d = os.path.join(tempfile.gettempdir(), "adom-fusion-wrap")
    os.makedirs(d, exist_ok=True)
    dst = os.path.join(d, "AdomDemoSample.lbr")
    # The sample's STEP travels with it: the tour's 3D stop BINDS a real body (2026-09-03,
    # John's demo review: the "3D chip" frame showed only the footprint and the 3D board
    # carried the Placeholder slab while the caption claimed a minted chip).
    try:
        import shutil as _sh
        _step_src = os.path.join(os.path.dirname(src), "ADS8588SIPM.step")
        if os.path.isfile(_step_src):
            _sh.copyfile(_step_src, os.path.join(d, "AdomDemoSample.step"))
    except Exception:
        pass
    shutil.copyfile(src, dst)
    return dst.replace("\\", "/")


# Per-step narration (John, 2026-08-24: "show ab captions for each step ... explaining
# what each step is doing ... and all the cool stuff you needed to have in place to make
# that work"). One sentence of WHAT + one of the MACHINERY. Written to be READ ALOUD
# (adom-tts renders these on the dashboard when audio narration is on) and shown as a
# desktop caption over the live demo, so a screenshot appearing always has its why.
_TOUR_EXPLAIN = {
    "launch": "Starting Fusion in the background. The bridge launches it detached and minimized, takes the one Autodesk seat if another machine holds it, and never steals your focus - and the AdomBridge add-in loads itself inside Fusion and opens a local command port the bridge drives everything through.",
    "stage": "Staging the sample part. This library was built from open KiCad source by adom dash l b r and lands on disk as a real EAGLE library file Fusion can open natively.",
    "library": "Opening the electronics library. The bridge drives Fusion's own Electronics Library editor through the add-in - no clicks, no foreground, the same path a human would take.",
    "symbol": "The symbol editor: the part's full pinout, converted pin for pin from KiCad source. The add-in switches views with Fusion's own edit commands.",
    "footprint": "The footprint editor: real pads and layers, exactly the copper a board would get - never a placeholder.",
    "device": "The component view: every symbol pin mapped to its footprint pad. The bridge verifies this mapping offline before Fusion ever opens the file, so dead wiring cannot photograph well.",
    "3d-package": "The 3D chip - the showpiece. The bridge imports the part's STEP model onto the footprint in Fusion's 3D package generator, orients it flat automatically, checks pin one against pad one, saves it into your Autodesk cloud hub, and binds the resulting cloud model into the library, so every future board carries the real body. This takes a couple of minutes; it is real work, not a preview.",
    "datahub": "Your Fusion Data Hub, through the add-in: hubs, projects and folders listed over Fusion's own cloud API - the same storage the 3D chip was just saved into.",
    "aps": "APS cloud search: the bridge's own server-indexed search over your Fusion cloud files, about two seconds per query after a single sign-in, and never charged.",
    "mcp": "Autodesk's official MCP server, answering over the bridge. Useful - but its toolset is small and read-mostly. The bridge's one hundred plus fusion verbs go far beyond it: electronics editors, text commands, cloud files, evidence captures and a dozen export formats.",
    "make-project": "Generating a complete test project around the part: schematic and board authored from EAGLE source and instantiated by Fusion, entirely in the background.",
    "open-design": "Opening the cloud electronics design from your hub - the bridge resolves it by name and opens the parent design, never a child view.",
    "schematic": "The schematic view of the generated project - the part wired into a real sheet.",
    "2d-board": "The 2D board layout - the same footprint you saw in the library, now placed on copper.",
    "3d-board": "The 3D board - and the chip body on it is the exact cloud model the bridge minted and bound earlier in this tour.",
    "3d-board-unbound": "The 3D board. The 3D bind did not complete on this run, so the chip shows Fusion's placeholder outline here; the library import chain binds the real body.",
    "exports": "Exports: STEP and STL written straight from the add-in. The same pipeline covers F 3 D, G L B, 3 M F, IGES, SAT, D X F, gerbers and BOM - every format a downstream tool could ask for.",
}


# ── The on-screen remote (John, 2026-09-01): ab's floaty glass PLAY/PAUSE/NEXT panel ──
# desktop_demo_panel is a shared transport: its buttons only QUEUE presses for the driver
# (us) to poll, they never run anything, so it adds no permission surface. We map presses
# onto the tour's existing transport flags (paused / skipNext / replayPrev / abort), the
# same ones fusion_demo_control sets, so the dashboard's buttons and the desktop remote
# drive one mechanism and can be used interchangeably mid-tour.
#
# ⛔ ONE CONSUMER PER PANEL ID. `poll` DRAINS the press queue. Proven live 2026-09-03: a
# CLI `desktop_demo_panel {action:"poll"}` fired from a terminal to "check" the panel ate
# the user's PAUSE press, the panel flipped its face to paused, and the tour - the real
# consumer - never saw it. Verify a remote-driven tour ONLY through fusion_demo_progress
# (remotePresses / paused), never by polling the panel yourself.
#
# Presses are drained by a background ticker every ~2s for the whole tour (below), so a
# press lands in the flags immediately and the panel's face stays in sync; the tour still
# HONORS the flags only at step boundaries, never mid-verb (a verb in flight cannot be
# interrupted safely, and a half-finished Fusion op is worse than a late pause).
_TOUR_PANEL_ID = "fusion-tour"


def _tour_panel_show(state, stops, stage, playing=True, reason=""):
    """Create/update the remote (idempotent by id). Best-effort, never raises."""
    try:
        _ab_call("desktop_demo_panel", {
            "action": "show", "id": _TOUR_PANEL_ID, "title": "Fusion bridge tour",
            "stops": stops, "stage": int(stage), "state": "playing" if playing else "paused",
            "position": "bottom-right",
            "reason": reason or "Guided Fusion tour: this remote pauses, skips, or replays the tour steps"},
            timeout=10)
        state["remote"] = {"shown": True, "stage": int(stage)}
    except Exception:
        pass


def _tour_panel_pump(state):
    """Drain the remote's presses into the tour's transport flags. Returns the list of
    button names seen (for the progress stream). Never raises."""
    seen = []
    try:
        r = _ab_call("desktop_demo_panel", {"action": "poll", "id": _TOUR_PANEL_ID}, timeout=8) or {}
        events = _find_key(r, "events") or []
        for ev in events:
            b = str((ev or {}).get("button") or "").lower()
            seen.append(b)
            if b == "pause":
                state["paused"] = True
            elif b == "play":
                state["paused"] = False
            elif b == "next":
                state["skipNext"] = True; state["paused"] = False
            elif b == "back":
                state["replayPrev"] = True; state["paused"] = False
            elif b == "stop":
                state["abort"] = True; state["paused"] = False
        if seen:
            state.setdefault("remotePresses", []).extend(seen[-8:])
    except Exception:
        pass
    return seen


def _tour_panel_hide():
    try:
        _ab_call("desktop_demo_panel", {"action": "hide", "id": _TOUR_PANEL_ID}, timeout=8)
    except Exception:
        pass


# The showcase John asked for (2026-09-01): library in, then symbol, footprint, 3D chip,
# the ganged component view, a schematic with that part, the 2D board, the 3D board.
# Drops the platform stops (datahub / aps / mcp / exports) which belong to the full tour.
_TOUR_PRESETS = {
    "electronics": ["launch", "stage", "library", "symbol", "footprint", "3d-package", "device",
                    "make-project", "open-design", "schematic", "2d-board", "3d-board"],
}


def _tour_caption(text: str, seconds: int = 45):
    """One desktop caption per tour step (id-replace: each step's caption replaces the
    last, so the screen always narrates the CURRENT step). Best-effort, never raises."""
    try:
        # dash-demo fleet ruling: ALWAYS the bridge's DEFAULT caption size and position -
        # seven apps, one caption look. If the default is wrong, fix it in ab for everyone.
        _ab_call("desktop_caption", {"text": text, "id": "fusion-tour",
                                     "duration": max(15000, min(int(seconds) * 2000, 240000))},
                 timeout=10)
    except Exception:
        pass


def _orchestrate_demo_tour(args: dict) -> dict:
    """Start the async six-content-type tour; poll fusion_demo_progress {demoId}."""
    import time
    import uuid
    demo_id = "tour-" + uuid.uuid4().hex[:8]
    include_design = bool(args.get("includeDesign", True))
    query = args.get("query") or ""
    # Cold-start (issue #30, verified live: a cold box gave a 100s bar ending in
    # nothing): when Fusion/add-in is not responding, the tour launches it itself,
    # as a REPORTED step so the front-end's bar covers the wait.
    cold = True
    try:
        probe = _proxy_to_addin("get_app_state", {}, timeout=8)
        cold = not probe.get("success")
    except Exception:
        pass
    import timings as _tm
    def _est(phase, seeded):
        """Measured p50 for this phase on THIS machine, else the seeded literal."""
        return int(round(_tm.estimate(phase, seeded, "cold" if phase == "launch" else None)[0]))
    steps = ([("launch", "Starting Fusion (cold start)", _est("launch", 120))] if cold else []) + [
        ("stage", "Staging the sample library", _est("stage", 3)),
        ("library", "Opening the electronics library", _est("library", 35)),
        ("symbol", "Symbol: full pinout in the symbol editor", _est("symbol", 8)),
        ("footprint", "Footprint: pads + layers in the footprint editor", _est("footprint", 8)),
        ("device", "Component view: symbol + package mapping", _est("device", 8)),
        ("3d-package", "3D chip: minted from the STEP, saved to your hub, bound into the library", _est("3d-package", 150)),
        # The platform side (John, 2026-08-24: "show off all aspects of your bridge
        # including aps and data hub and the add-in and the mcp server chatting and the
        # limits of the mcp server that you go way beyond"). Each degrades gracefully
        # (signed-out APS, disabled MCP) - the caption then narrates the honest state.
        ("datahub", "Data Hub: your cloud projects through the add-in", _est("datahub", 10)),
        ("aps", "APS cloud search: ~2s indexed lookups", _est("aps", 8)),
        ("mcp", "MCP: Autodesk's own AI server (and its limits)", _est("mcp", 10)),
    ]
    if include_design:
        # Design side is SELF-SUFFICIENT by default (John, 2026-08-15: customers have
        # only a component page, never a canonical cloud board): generate + import a
        # test project around the sample part. A query switches to the cloud-open path.
        steps += ([("open-design", "Opening the cloud electronics design", _est("open-design", 150))] if query
                  else [("make-project", "Generating + importing a test project around the sample part", _est("make-project", 60))])
        steps += [
            ("schematic", "Schematic view", _est("schematic", 12)),
            ("2d-board", "2D board layout", _est("2d-board", 12)),
            ("3d-board", "3D board", _est("3d-board", 20)),
            ("exports", "Exporting: STEP + STL (of a dozen formats)", _est("exports", 25)),
        ]
    preset = (args.get("preset") or "").lower()
    if preset in _TOUR_PRESETS:
        keep = _TOUR_PRESETS[preset]
        steps = sorted([s for s in steps if s[0] in keep], key=lambda s: keep.index(s[0]))
    remote = bool(args.get("remote"))
    est = sum(s[2] for s in steps)
    state = {"demoId": demo_id, "percent": 0, "elapsedSec": 0, "estimatedSec": est,
             "step": steps[0][0], "stepLabel": steps[0][1], "done": False, "failed": False,
             "steps": [s[0] for s in steps], "shots": [], "started": time.time()}
    with _DEMO_TOURS_LOCK:
        _DEMO_TOURS[demo_id] = state

    def _wait_idle(max_s=180):
        """Fusion's main thread keeps grinding long ops after our proxy timeout returns
        (Create3DPackage on a cold session ran 120s+ and broke the next step's open,
        issue #30 cold-start validation). Gate on app-state responding again."""
        deadline = time.time() + max_s
        while time.time() < deadline:
            try:
                if _proxy_to_addin("get_app_state", {}, timeout=8).get("success"):
                    return True
            except Exception:
                pass
            time.sleep(5)
        return False

    def _snap(label):
        # {title, image} is the ab web-control harness's gallery item contract (issue #29
        # reply, harness live in ab 2.0.39) - label kept as an alias.
        try:
            r = _handle_screenshot_fusion({}, {})
            # server-side helper returns savedTo (localSafePath is the CLI layer's name -
            # checking only that dropped EVERY tour shot, caught live 2026-08-15)
            p = r.get("savedTo") or r.get("localSafePath")
            if r.get("success") and p:
                state["shots"].append({"title": label, "label": label, "path": p})
        except Exception:
            pass

    def _run():
        fg = bool(args.get("foreground"))
        if not fg:
            begin_background_envelope(float(est + 60))
        done_est = 0
        _docs_before_tour = _open_doc_names()   # tour hygiene: close only what WE open
        _stop_labels = [s[1] for s in steps]
        _pump_stop = threading.Event()
        if remote:
            _tour_panel_show(state, _stop_labels, 0, playing=True)

            def _pump_loop():
                # Continuous drain (see the ONE CONSUMER note above). Re-shows the panel
                # only when the paused face changed, so the panel is not redrawn every tick.
                last_face = None
                while not _pump_stop.is_set():
                    seen = _tour_panel_pump(state)
                    face = bool(state.get("paused"))
                    if seen or face != last_face:
                        _tour_panel_show(state, _stop_labels, max(0, _step_ref[0] - 1),
                                         playing=not face)
                        last_face = face
                    _pump_stop.wait(2.0)
            _step_ref = [0]
            threading.Thread(target=_pump_loop, daemon=True, name="fusion-tour-remote").start()
        try:
            _VIEW_STEPS = {"symbol", "footprint", "device", "schematic", "2d-board", "3d-board", "library"}
            _last_view_step = None
            _step_i = 0
            while _step_i < len(steps):
                key, label, dur = steps[_step_i]
                _step_i += 1
                # ── TRANSPORT (fusion_demo_control + the on-screen remote): boundary
                # checks, never mid-verb. The remote's presses are drained continuously by
                # the ticker onto the same flags the dashboard's buttons set; here we only
                # advance the panel's stop counter and honor the flags. ──
                if remote:
                    _step_ref[0] = _step_i
                    _tour_panel_show(state, _stop_labels, _step_i - 1, playing=not state.get("paused"))
                if state.pop("abort", None):
                    raise RuntimeError("tour exited by the user (transport control)")
                while state.get("paused") and not state.get("abort"):
                    state.update(stepLabel="Paused - press play to continue", stepCaption="Paused.")
                    time.sleep(1.5)
                if state.pop("abort", None):
                    raise RuntimeError("tour exited by the user (transport control)")
                if state.pop("skipNext", None):
                    state.setdefault("skippedSteps", []).append(key)
                    done_est += dur
                    continue
                if state.pop("replayPrev", None) and _last_view_step:
                    _step_i -= 1                      # come back to the current step after
                    key, label, dur = _last_view_step  # re-run the last view step once
                if key in _VIEW_STEPS:
                    _last_view_step = (key, label, dur)
                if not fg:
                    # RE-ARM per step: one est-sized envelope EXPIRED mid-tour on real runs
                    # (485-890s vs ~376s armed) and later steps stole the user's foreground
                    # while they typed in other threads (John, live during ab's demo,
                    # 2026-08-16). Nesting is refcounted, so re-arming is free.
                    begin_background_envelope(float(dur * 3 + 240))
                _step_started = time.time()
                # NARRATION (John, 2026-08-24): the caption teaches what this step is doing
                # and the machinery underneath, on-screen (desktop caption, id-replaced per
                # step) and in the progress stream (stepCaption - the dashboard shows it and
                # renders it as audio when narration is on).
                _explain = _TOUR_EXPLAIN.get(key) or label
                if key == "3d-board" and not state.get("bound3d"):
                    _explain = _TOUR_EXPLAIN["3d-board-unbound"]
                if args.get("captions", True):
                    _tour_caption(_explain, dur)
                # LIVE, per-step ETA (issue #32): the tour reports the same contract block
                # every other slow verb does, so ONE renderer draws every bar in Adom.
                state.update(step=key, stepLabel=label, stepCaption=_explain,
                             percent=int(100 * done_est / est),
                             elapsedSec=int(time.time() - state["started"]),
                             estimatedSec=est,
                             etaSec=max(0, int(est - (time.time() - state["started"]))),
                             confidence=_tm.estimate(key, dur)[1],
                             startKind=("cold" if cold else "warm"),
                             progress={"phase": key, "stepLabel": label,
                                       "percent": int(100 * done_est / est),
                                       "elapsedSec": int(time.time() - state["started"]),
                                       "estimatedSec": est,
                                       "etaSec": max(0, int(est - (time.time() - state["started"]))),
                                       "confidence": _tm.estimate(key, dur)[1],
                                       "startKind": ("cold" if cold else "warm")})
                if key == "stage" and args.get("sampleLbr"):
                    pass  # handled in the stage branch below
                if key == "launch":
                    dispatch_command("start", {})
                    ready = False
                    seat_attempts = 0
                    for _ in range(30):
                        time.sleep(5)
                        try:
                            if _proxy_to_addin("get_app_state", {}, timeout=8).get("success"):
                                ready = True
                                break
                        except Exception:
                            pass
                        # ab's #30 fleet finding (2026-08-16): a single-seat account with Fusion
                        # signed in elsewhere wedges the cold start at 0% behind 'Active Sessions
                        # Exceeded'. Self-heal (take the seat, John's standing call) and REPORT it
                        # as its own state; if takeover cannot land (opt-out, or the UIA path is
                        # unavailable), fail FAST with the decidable classification instead of
                        # burning the whole 150s budget.
                        try:
                            _dismiss_save_prompt_if_bridge_doc()
                        except Exception:
                            pass
                        try:
                            if find_licensing_dialog():
                                if _seat_takeover_enabled():
                                    state.update(stepLabel="Taking over the Autodesk seat (another machine holds it)")
                                    r = _resolve_seat_dialog(max_clicks=3)
                                    seat_attempts += 1
                                    if r.get("clicks"):
                                        state["seatTakeover"] = True
                                        state.setdefault("warnings", []).append(
                                            "license_sessions_exceeded: took the seat over in the background "
                                            "(the other machine's Fusion suspended resumably)")
                                    if seat_attempts >= 3 and find_licensing_dialog():
                                        raise RuntimeError(
                                            "license_sessions_exceeded: the Autodesk seat is held on another "
                                            "machine and background takeover did not land. Free it there "
                                            "(fusion_stop on that box) and re-run the tour.")
                                else:
                                    raise RuntimeError(
                                        "license_sessions_exceeded: seat held on another machine and "
                                        "auto-takeover is opted out (fusion_set_seat_takeover). Resolve the "
                                        "'Active Sessions Exceeded' dialog or re-enable takeover, then re-run.")
                        except RuntimeError:
                            raise
                        except Exception:
                            pass
                    if not ready:
                        # Not ready usually means the SIGN-IN wall on a fresh box. Drive it as far
                        # as code can (email typed, page advanced), toast the human at the exact
                        # remaining step, and fail the tour with instructions instead of a shrug.
                        rd = _handle_fusion_readiness({}, {})
                        if rd.get("needsSignin"):
                            state.update(step="signin", stepLabel="Signing in (one-time, driving it for you)")
                            si = _orchestrate_signin({"auto": True})
                            wallname = si.get("signinWall") or "credentials"
                            state.setdefault("warnings", []).append(
                                "sign-in required: drove to wall %r; user toasted; %s" % (wallname, str(si.get("stage"))))
                            raise RuntimeError(
                                "One-time Autodesk sign-in needed on this machine. I drove it as far "
                                "as possible and toasted the user at the %r step - after they finish, "
                                "re-run the tour and it completes." % wallname)
                        raise RuntimeError("Fusion launched but its add-in never became ready (150s)")
                elif key == "stage":
                    custom = (args.get("sampleLbr") or "").replace("\\", "/")
                    if custom:
                        # Tour ANY wiki part (John, 2026-08-16: "do the tour using an adom basic
                        # parts led"): caller stages the part's .lbr (send_files) and passes its
                        # Windows path; the deviceset is read from the file.
                        import os as _os
                        if not _os.path.isfile(custom):
                            raise RuntimeError("sampleLbr not found on this machine: %s" % custom)
                        state["sampleLbr"] = custom
                        m = re.search(r'<deviceset name="([^"]+)"', open(custom, encoding="utf-8", errors="replace").read())
                        state["deviceset"] = m.group(1) if m else "UNKNOWN"
                    else:
                        lbr = _demo_tour_sample_lbr()
                        if not lbr:
                            raise RuntimeError("bundled sample library missing from the bridge install")
                        state["sampleLbr"] = lbr
                        state["deviceset"] = "ADS8588SIPM"
                elif key == "library":
                    r = _orchestrate_open_lbr({"filePath": state["sampleLbr"]})
                    if not r.get("success"):
                        raise RuntimeError("open_lbr: " + str(r.get("error")))
                    _snap(label)
                elif key in ("symbol", "footprint", "device"):
                    suffix = {"symbol": "sym", "footprint": "pac", "device": "dev"}[key]
                    r = _show_library_view(state.get("deviceset") or "ADS8588SIPM", suffix)
                    if not r.get("success"):
                        raise RuntimeError(str(r.get("error")))
                    if r.get("screenshot"):
                        state["shots"].append({"title": label, "label": label, "path": r["screenshot"]})
                elif key == "3d-package":
                    # REAL BIND, not a peek at the generator (John, 2026-09-03). Same core
                    # the library-import chain uses: STEP onto the footprint, oriented,
                    # saved as f3d in the hub, bound into the sample library. Everything
                    # after this stop (device view, 2D board, 3D board) then carries the
                    # real body instead of the Placeholder slab.
                    _step_path = _os_path_sibling(state["sampleLbr"], ".step")
                    _pac = ((_library_artifact_names(state["sampleLbr"]) or {}).get("pac") or [None])[0]
                    bound = False
                    if _step_path and _pac:
                        _wait_idle()
                        br = _build_library_3d_core({
                            "lbrPath": state["sampleLbr"],
                            "parts": [{"package": _pac, "lbrPath": state["sampleLbr"],
                                       "modelPath": _step_path}],
                            "outLbrPath": state["sampleLbr"], "capture": True}) or {}
                        bound = bool(br.get("success")) and int(br.get("partsBound") or 0) >= 1
                        state["bind"] = {"success": bound, "partsBound": br.get("partsBound"),
                                         "error": br.get("error")}
                        # the bind core captures the chip on its footprint; keep that frame
                        for shot in (br.get("screenshots") or []):
                            if shot.get("stage") in ("after", "3d") and shot.get("path"):
                                state["shots"].append({"title": label, "label": label, "path": shot["path"]})
                                break
                    state["bound3d"] = bound
                    if bound:
                        _wait_idle()
                        r = _orchestrate_open_lbr({"filePath": state["sampleLbr"]})
                        if not any(sh.get("title") == label for sh in state["shots"]):
                            _snap(label)
                    else:
                        # Honest fallback: show the generator with the footprint, and SAY
                        # so - the 3D-board caption must not claim a body that is not there.
                        state.setdefault("warnings", []).append(
                            "3D bind did not complete (%s); showing the package generator instead"
                            % ((state.get("bind") or {}).get("error") or "no STEP staged"))
                        cp = _create_3d_package(state["sampleLbr"], timeout=60)
                        if cp.get("success"):
                            time.sleep(3)
                            _snap(label)
                            _proxy_to_addin("run_modeling_script",
                                            {"script": "import adsk.core\ncd=ui.commandDefinitions.itemById('Package3DStop')\nresult={'finished': bool(cd) and cd.execute()}\n"},
                                            timeout=30)
                            time.sleep(2)
                            try:
                                _dismiss_dialogs_bg()
                            except Exception:
                                pass
                            _proxy_to_addin("close_document", {"save": False}, timeout=30)
                            time.sleep(1)
                        else:
                            state.setdefault("failedSteps", []).append(key)
                elif key == "make-project":
                    _wait_idle()
                    r = _orchestrate_make_test_project({"libraryPath": state["sampleLbr"]})
                    if r.get("success"):
                        time.sleep(3)
                        _snap(label)
                    else:
                        state.setdefault("failedSteps", []).append(key)
                        state.setdefault("warnings", []).append("make-project: " + str(r.get("error"))[:120])
                        state["designOpenFailed"] = True
                elif key == "open-design":
                    _wait_idle()
                    dispatch_command("open_cloud_file", {"fileName": query, "wait": False})
                    deadline = time.time() + 170
                    design_open = False
                    while time.time() < deadline:
                        st = _proxy_to_addin("get_app_state", {}, timeout=10)
                        if query.lower() in str((st.get("data") or {}).get("activeDocument", "")).lower():
                            design_open = True
                            break
                        time.sleep(5)
                    if design_open:
                        time.sleep(3)
                        _snap(label)
                    else:
                        # A screenshot of NOT-the-design is worse than an honest failure:
                        # record it and let the dependent view steps skip themselves.
                        state.setdefault("failedSteps", []).append(key)
                        state.setdefault("warnings", []).append(
                            "open-design: %r never became the active document within 170s" % query)
                        state["designOpenFailed"] = True
                elif key == "datahub":
                    _wait_idle()   # ralph run 649s: the 3D save was still grinding; busy != broken
                    r = _proxy_to_addin("list_cloud_projects", {}, timeout=45)
                    if r.get("success"):
                        _d = r.get("data") or {}
                        _projs = _d.get("projects") or _d.get("dataProjects") or []
                        state.setdefault("highlights", {})["datahub"] = {
                            "projects": len(_projs),
                            "names": [str(p.get("name") or p)[:40] for p in _projs[:6]]}
                    else:
                        state.setdefault("warnings", []).append(
                            "datahub: " + str(r.get("error"))[:100])
                elif key == "aps":
                    _q = (state.get("deviceset") or "ADS8588").split("/")[-1][:20]
                    _t0 = time.time()
                    r = dispatch_command("aps_search", {"query": _q, "limit": 5})
                    if r.get("success"):
                        _hits = r.get("results") or r.get("items") or (r.get("data") or {}).get("results") or []
                        state.setdefault("highlights", {})["aps"] = {
                            "query": _q, "hits": len(_hits), "seconds": round(time.time() - _t0, 1)}
                    else:
                        state.setdefault("warnings", []).append(
                            "aps: %s (sign in once with fusion_aps_signin and this step lights up)"
                            % str(r.get("error"))[:80])
                elif key == "mcp":
                    r = dispatch_command("mcp_tools", {})
                    if r.get("success"):
                        _tools = r.get("tools") or (r.get("data") or {}).get("tools") or []
                        _names = [str(t.get("name") or t)[:32] for t in _tools][:20]
                        state.setdefault("highlights", {})["mcp"] = {
                            "toolCount": len(_tools), "tools": _names,
                            "beyond": ("The bridge's fusion_* surface is ~120 verbs: electronics "
                                       "editors, EAGLE text commands, cloud files, APS search, "
                                       "evidence captures, and 14 export formats - none of which "
                                       "the MCP toolset offers.")}
                    else:
                        state.setdefault("warnings", []).append(
                            "mcp: %s (fusion_mcp_enable turns it on in ~10s)" % str(r.get("error"))[:80])
                elif key == "exports":
                    _wait_idle()
                    _exp_dir = "C:/tmp/fusion-tour-exports"
                    _got = []
                    for _fmt, _verb in (("step", "export_step"), ("stl", "export_stl")):
                        _p = "%s/%s.%s" % (_exp_dir, (state.get("deviceset") or "tour"), _fmt)
                        r = dispatch_command(_verb, {"outputPath": _p})
                        if r.get("success"):
                            _got.append(_fmt.upper())
                        else:
                            state.setdefault("warnings", []).append(
                                "%s: %s" % (_verb, str(r.get("error"))[:80]))
                    if _got:
                        state.setdefault("highlights", {})["exports"] = {
                            "formats": _got, "dir": _exp_dir,
                            "also": ["F3D", "GLB", "3MF", "IGES", "SAT", "DXF", "DWG", "OBJ",
                                     "FBX", "SKP", "USDZ", "gerbers", "BOM", "CPL"]}
                        _snap(label)
                elif key in ("schematic", "2d-board", "3d-board"):
                    if state.get("designOpenFailed"):
                        state.setdefault("failedSteps", []).append(key)
                        continue
                    cmd = {"schematic": "show_schematic", "2d-board": "show_2d_board", "3d-board": "show_3d_board"}[key]
                    r = dispatch_command(cmd, {})
                    if r.get("success"):
                        time.sleep(2)
                        _snap(label)
                    else:
                        state.setdefault("failedSteps", []).append(key)
                        state.setdefault("warnings", []).append("%s: %s" % (key, str(r.get("error"))[:120]))
                # Every finished step is a MEASURED sample - this is what turns the next run's
                # estimate from a literal into this machine's own p50 (issue #32).
                try:
                    _tm.record(key, time.time() - _step_started,
                               "cold" if key == "launch" and cold else None)
                except Exception:
                    pass
                done_est += dur
                state.update(percent=int(100 * done_est / est),
                             elapsedSec=int(time.time() - state["started"]))
            n_shots = len(state.get("shots") or [])
            if args.get("captions", True):
                _tour_caption("Tour complete: library, symbol, footprint, 3D chip, Data Hub, "
                              "APS search, MCP, a generated project in schematic, 2D and 3D, "
                              "and real exports - all driven in the background by the bridge.", 20)
            state.update(done=True, percent=100, step="done", stepLabel="Tour complete",
                         narrate="Fusion tour complete: %d views captured across the electronics library%s." % (
                             n_shots, " and the board design" if include_design and query else ""),
                         elapsedSec=int(time.time() - state["started"]))
        except Exception as e:
            state.setdefault("failedSteps", []).append(state.get("step"))
            state.update(failed=True, done=True, error=str(e)[:300],
                         narrate="Fusion tour stopped at '%s': %s" % (state.get("stepLabel"), str(e)[:120]),
                         elapsedSec=int(time.time() - state["started"]))
        finally:
            # TOUR HYGIENE (John's 9-open-docs screenshot, 2026-08-24: SIX copies of
            # ADOM-TEST-ADS8588SIPM left behind): same guarantee the create_library chain
            # got in 1.9.182/188 - close every doc the tour opened, plus unmodified
            # Untitleds, success or failure. Pre-existing docs are never touched.
            try:
                _SWEEP_LAST_DEFERRED[:] = []
                state["tourDocsClosed"] = _close_docs_opened_since(_docs_before_tour)
                if _SWEEP_LAST_DEFERRED:
                    state["tourDocsDeferred"] = list(_SWEEP_LAST_DEFERRED)
            except Exception as _swe:
                # Never silent (2026-09-03: tourDocsClosed came back None while five
                # ADOM-TEST tabs sat open, and nothing said why).
                state["tourDocsClosed"] = []
                state["tourDocsSweepError"] = str(_swe)[:200]
            if args.get("captions", True):
                try:
                    _ab_call("desktop_caption", {"action": "hide", "id": "fusion-tour"}, timeout=8)
                except Exception:
                    pass
            if remote:
                _pump_stop.set()
                _tour_panel_hide()
            _demo_tour_persist(state)

    threading.Thread(target=_run, daemon=True, name="fusion-demo-tour").start()
    return {"success": True, "demoId": demo_id, "estimatedSec": est,
            "steps": [{"step": s[0], "label": s[1], "estimatedSec": s[2],
                       "caption": _TOUR_EXPLAIN.get(s[0]) or s[1]} for s in steps],
            "statusVerb": "fusion_demo_progress",
            "_hint": ("Tour started ASYNC (background by default - pass foreground:true only if the "
                      "user wants to WATCH). Poll fusion_demo_progress {\"demoId\": \"%s\"} about "
                      "once a second and render percent/stepLabel as a real progress bar. The final "
                      "poll (done:true) carries the screenshot array.") % demo_id}


def _handle_demo_control(args: dict) -> dict:
    """Transport control for a running tour (John, 2026-08-24: "play/pause/next/prev/exit
    ... an easy way to stop the demo"). Granularity is BETWEEN steps: a running Fusion verb
    cannot be preempted, so pause/skip/exit take effect at the next step boundary (a few
    seconds to a minute). actions: pause | resume | next (skip the upcoming step) |
    prev (re-run the last completed VIEW step) | exit (abort + close the tour's docs)."""
    demo_id = args.get("demoId") or ""
    action = (args.get("action") or "").lower()
    with _DEMO_TOURS_LOCK:
        state = _DEMO_TOURS.get(demo_id)
        if state is None and not demo_id and len(_DEMO_TOURS) == 1:
            state = list(_DEMO_TOURS.values())[0]
    if state is None or state.get("done"):
        return {"success": False, "error": "no running tour" + (" with id " + demo_id if demo_id else ""),
                "_hint": "Controls only apply while a tour is running."}
    if action == "pause":
        state["paused"] = True
    elif action == "resume":
        state["paused"] = False
    elif action == "next":
        state["skipNext"] = True
        state["paused"] = False
    elif action == "prev":
        state["replayPrev"] = True
        state["paused"] = False
    elif action == "exit":
        state["abort"] = True
        state["paused"] = False
    else:
        return {"success": False, "error": "action must be pause|resume|next|prev|exit"}
    return {"success": True, "action": action, "paused": bool(state.get("paused")),
            "_hint": "Takes effect at the next step boundary - a running Fusion verb finishes first."}


def _handle_demo_progress(args: dict) -> dict:
    demo_id = args.get("demoId") or ""
    with _DEMO_TOURS_LOCK:
        state = _DEMO_TOURS.get(demo_id)
        # Fall back to "the single running tour" ONLY when no id was given at all -
        # a WRONG id must refuse (the suite's #unknown negative test enforces this).
        if state is None and not demo_id and len(_DEMO_TOURS) == 1:
            state = list(_DEMO_TOURS.values())[0]
    persisted = False
    if state is None and demo_id:
        # Respawn-survival (ab #30): finished tours are snapshotted to disk, so the final
        # frame stays pollable even after bridge_install replaced this process mid-poll.
        state = _demo_tour_load(demo_id)
        persisted = state is not None
    if state is None:
        return {"success": False, "error": "unknown demoId " + demo_id,
                "_hint": "Start a tour first: fusion_demo {\"mode\": \"tour\"}. Its response carries the demoId. Finished tours stay pollable (disk snapshot) even across bridge respawns - an unknown id here means it never ran on this box."}
    out = {"success": True}
    out.update({k: v for k, v in state.items() if k != "started"})
    if persisted:
        out["fromSnapshot"] = True
    # ── stepShot: the DURING evidence (ab's exact contract, #30 2026-08-16): one shot per
    # completed step, riding the FIRST progress frame after that step completes, ~640px
    # wide base64 - the bridge knows the exact completion moment and surface, the page
    # does not. Delivery cursor lives in the tour state; encode failure still advances so
    # one unreadable file can never stall the stream. The final shots gallery (below and
    # at done:true) is unchanged - that is the AFTER evidence.
    if not persisted and args.get("sinceShot") is None:
        # stepShot only for callers that do NOT window with sinceShot: its delivery cursor
        # is per-TOUR, so two windowing viewers would steal frames from each other, and
        # windowing callers already get every shot exactly once (2026-08-24 gallery fix).
        _sent = int(state.get("stepShotSent") or 0)
        _all = list(state.get("shots") or [])
        if _sent < len(_all):
            s = _all[_sent]
            img = _shot_b64_small(s.get("path") or "")
            state["stepShotSent"] = _sent + 1
            if img:
                out["stepShot"] = {"step": s.get("label"), "title": s.get("title") or s.get("label"),
                                   "image": img}
    # ── Progressive per-step EVIDENCE (John, 2026-08-16): shots stream to the caller AS
    # each step completes, not only at done:true. Contract for a live evidence gallery:
    #   poll fusion_demo_progress {demoId, imagesAs:"base64", sinceShot:<count you have>}
    # -> shots = ONLY the new ones (each with .image base64), shotCount = total so far.
    # sinceShot keeps 1/sec polls cheap: unchanged evidence is never re-encoded/re-sent.
    # Shot dicts are COPIED before encoding - mutating state's own dicts embedded base64
    # into the shared tour state (memory bloat + every later poll re-shipping it all).
    shots = list(out.get("shots") or [])
    out["shotCount"] = len(shots)
    try:
        since = max(0, int(args.get("sinceShot") or 0))
    except Exception:
        since = 0
    if since:
        shots = shots[since:]
    imgs = args.get("imagesAs") or ("base64" if state.get("done") else "paths")
    if imgs in ("base64", "both"):
        import base64
        encoded = []
        for s in shots:
            s = dict(s)
            try:
                raw = open(s["path"], "rb").read()
                if len(raw) <= 4 * 1024 * 1024:
                    s["image"] = base64.b64encode(raw).decode()
                else:
                    # PIL-less bridge python saves raw 24MB BMPs - NEVER ship those inline
                    # (9 x 31MB base64 = a ~280MB frame). Downscaled evidence instead.
                    img = _shot_b64_small(s["path"], max_w=1280)
                    if img:
                        s["image"] = img
                        s["imageNote"] = "downscaled to <=1280px (original is a %dMB capture; pull_file the path for full size)" % (len(raw) // (1024 * 1024))
            except Exception:
                pass
            encoded.append(s)
        shots = encoded
    out["shots"] = shots
    if not state.get("done"):
        out["_hint"] = ("Tour running (%s). shots[] carries evidence for every COMPLETED step "
                        "already - render it live; pass sinceShot=%d on the next poll to receive "
                        "only new shots." % (out.get("stepLabel"), out["shotCount"]))
    return out




# ── Async wrapper for export_optimized_glb (2026-08-14): the verb's STEP-export phase
# alone can outlive ab's 60s relay window (fresh-session Fusion), so the sync form dies
# at the relay even though the work completes. async:true fires the whole pipeline in a
# thread and returns an exportId instantly; fusion_fetch_optimized_glb {exportId} reports
# phase -> jobId -> complete. Same pattern as the demo tour.
_GLB_ASYNC: dict = {}
_GLB_ASYNC_LOCK = threading.Lock()


def _start_glb_export_async(args: dict) -> dict:
    import time
    import uuid
    export_id = "glbx-" + uuid.uuid4().hex[:8]
    state = {"exportId": export_id, "phase": "exporting-step", "done": False,
             "failed": False, "started": time.time()}
    with _GLB_ASYNC_LOCK:
        _GLB_ASYNC[export_id] = state

    inner = {k: v for k, v in args.items() if k != "async"}
    inner.setdefault("wait", 20)

    def _run():
        begin_background_envelope(400.0)
        try:
            res = _orchestrate_export_optimized_glb(inner)
            state["result"] = res
            if res.get("glbPath") and res.get("success"):
                state.update(phase="complete", done=True)
                return
            job = res.get("jobId")
            if not (res.get("success") and job):
                state.update(phase="failed", failed=True, done=True,
                             error=str(res.get("error"))[:200])
                return
            state.update(phase="optimizing", jobId=job)
            deadline = time.time() + 600
            while time.time() < deadline:
                f = _orchestrate_fetch_optimized_glb(
                    {"jobId": job, "outputPath": inner.get("outputPath"), "wait": 20})
                state["result"] = f
                if f.get("glbPath") and not f.get("pending"):
                    state.update(phase="complete", done=True)
                    return
                if not f.get("success") and not f.get("pending"):
                    state.update(phase="failed", failed=True, done=True,
                                 error=str(f.get("error"))[:200])
                    return
                time.sleep(5)
            state.update(phase="failed", failed=True, done=True, error="optimizer did not finish within 10 min")
        except Exception as e:
            state.update(phase="failed", failed=True, done=True, error=str(e)[:200])

    threading.Thread(target=_run, daemon=True, name="glb-export-async").start()
    return {"success": True, "pending": True, "exportId": export_id,
            "statusVerb": "fusion_fetch_optimized_glb",
            "_hint": ("Export running ASYNC (the sync form can outlive the relay window on a cold "
                      "Fusion session). Poll fusion_fetch_optimized_glb {\"exportId\": \"%s\"} "
                      "every few seconds: phase walks exporting-step -> optimizing -> complete, and "
                      "the final poll carries glbPath/sizeBytes.") % export_id}


def _glb_export_async_status(args: dict) -> dict:
    import time
    export_id = args.get("exportId") or ""
    with _GLB_ASYNC_LOCK:
        state = _GLB_ASYNC.get(export_id)
    if state is None:
        return {"success": False, "error": "unknown exportId " + export_id,
                "_hint": "Start one with fusion_export_optimized_glb {async: true}; its response carries the exportId."}
    out = {"success": not state.get("failed"), "exportId": export_id,
           "phase": state.get("phase"), "done": state.get("done"),
           "elapsedSec": int(time.time() - state["started"])}
    if state.get("jobId"):
        out["jobId"] = state["jobId"]
    if state.get("error"):
        out["error"] = state["error"]
    if state.get("done") and isinstance(state.get("result"), dict):
        for k in ("glbPath", "sizeBytes", "meshesAfter", "moleculeAnchored", "silkscreenApplied"):
            if k in state["result"]:
                out[k] = state["result"][k]
    return out




# ── fusion_make_test_project (John, 2026-08-15): the Web Control demo's PROJECT button.
# "None of our other adom user customers will even have a BQ25792" - a customer's session
# has exactly ONE thing: a component page's .lbr. This verb closes the circle from it:
# it GENERATES a minimal test circuit (the part + a generated series resistor on a small
# board) as real EAGLE .sch/.brd server-side, then imports the pair as a NEW electronics
# design. No cloud, no sign-in, no pre-existing content. The schematic/2D/3D buttons then
# work on the result.

def _mtp_extract(lbr_text, deviceset, device=None):
    """Pull the pieces the generator needs out of the .lbr."""
    def block_of(tag, name):
        m = re.search(r'<%s name="%s"[\s>].*?</%s>' % (tag, re.escape(name), tag), lbr_text, re.S)
        return m.group(0) if m else None
    ds = block_of("deviceset", deviceset)
    if not ds:
        return None, "deviceset %r not found in the library" % deviceset
    gate = re.search(r'<gate name="([^"]+)" symbol="([^"]+)"', ds)
    prefix_m = re.search(r'<deviceset name="[^"]+"[^>]*prefix="([^"]+)"', ds)
    # PACKAGE VARIANTS (John, 2026-08-30): pick the named device ("-HV", "-SB", ...) so the
    # test board carries THAT footprint (its vias, its rounded pads); default is the first.
    dev = None
    if device is not None:
        dev = re.search(r'<device name="%s" package="([^"]+)"' % re.escape(device), ds)
        if not dev:
            return None, "deviceset %r has no device named %r" % (deviceset, device)
    if dev is None:
        dev = re.search(r'<device name="[^"]*" package="([^"]+)"', ds)
    if not (gate and dev):
        return None, "deviceset %r has no gate/package device mapping" % deviceset
    connects = re.findall(r'<connect gate="[^"]+" pin="([^"]+)" pad="([^"]+)"', ds)
    # Strip 3D binding refs from the embedded deviceset: a package3dinstance whose
    # packages3d section is not ALSO embedded is a dangling ref, and Fusion then
    # renders the schematic EMPTY (caught 2026-08-15: bound-library test projects
    # imported blank sheets). The test project needs no 3D binding.
    ds = re.sub(r'<package3dinstances>.*?</package3dinstances>', '', ds, flags=re.S)
    sym = block_of("symbol", gate.group(2))
    pkg = block_of("package", dev.group(1))
    if not (sym and pkg and connects):
        return None, "library is missing the symbol/package/connects for %r" % deviceset
    layers = re.search(r'<layers>.*?</layers>', lbr_text, re.S)
    return {"deviceset": ds, "symbol": sym, "package": pkg, "connects": connects,
            "symName": gate.group(2), "gateName": gate.group(1), "pkgName": dev.group(1),
            "device": (device or ""),
            "prefix": (prefix_m.group(1) if prefix_m else "U"),
            "layers": layers.group(0) if layers else ""}, None


_MTP_R_SYM = ('<symbol name="ADOMR"><wire x1="-2.54" y1="1.016" x2="2.54" y2="1.016" width="0.254" layer="94"/>'
 '<wire x1="2.54" y1="1.016" x2="2.54" y2="-1.016" width="0.254" layer="94"/>'
 '<wire x1="2.54" y1="-1.016" x2="-2.54" y2="-1.016" width="0.254" layer="94"/>'
 '<wire x1="-2.54" y1="-1.016" x2="-2.54" y2="1.016" width="0.254" layer="94"/>'
 '<pin name="1" x="-5.08" y="0" visible="off" length="short"/>'
 '<pin name="2" x="5.08" y="0" visible="off" length="short" rot="R180"/>'
 '<text x="-2.54" y="1.524" size="1.778" layer="95">&gt;NAME</text>'
 '<text x="-2.54" y="-3.302" size="1.778" layer="96">&gt;VALUE</text></symbol>')
_MTP_R_PKG = ('<package name="ADOMR0603"><description>Resistor 0603</description>'
 '<smd name="1" x="-0.85" y="0" dx="1" dy="1" layer="1"/>'
 '<smd name="2" x="0.85" y="0" dx="1" dy="1" layer="1"/>'
 '<wire x1="-0.4" y1="0.5" x2="0.4" y2="0.5" width="0.127" layer="21"/>'
 '<wire x1="-0.4" y1="-0.5" x2="0.4" y2="-0.5" width="0.127" layer="21"/>'
 '<text x="-1" y="0.8" size="0.8" layer="25">&gt;NAME</text></package>')
_MTP_R_DS = ('<deviceset name="ADOMR" prefix="R"><description>generated test resistor</description>'
 '<gates><gate name="G$1" symbol="ADOMR" x="0" y="0"/></gates>'
 '<devices><device name="" package="ADOMR0603">'
 '<connects><connect gate="G$1" pin="1" pad="1"/><connect gate="G$1" pin="2" pad="2"/></connects>'
 '<technologies><technology name=""/></technologies></device></devices></deviceset>')
_MTP_SCH_LAYERS = ('<layers>'
 '<layer number="91" name="Nets" color="2" fill="1" visible="yes" active="yes"/>'
 '<layer number="92" name="Busses" color="1" fill="1" visible="yes" active="yes"/>'
 '<layer number="93" name="Pins" color="2" fill="1" visible="no" active="yes"/>'
 '<layer number="94" name="Symbols" color="4" fill="1" visible="yes" active="yes"/>'
 '<layer number="95" name="Names" color="7" fill="1" visible="yes" active="yes"/>'
 '<layer number="96" name="Values" color="7" fill="1" visible="yes" active="yes"/>'
 '<layer number="97" name="Info" color="7" fill="1" visible="yes" active="yes"/>'
 '<layer number="98" name="Guide" color="6" fill="1" visible="yes" active="yes"/></layers>')
_MTP_HEAD = ('<?xml version="1.0" encoding="utf-8"?>\n<!DOCTYPE eagle SYSTEM "eagle.dtd">\n'
 '<eagle version="9.6.2">\n<drawing>\n<settings><setting alwaysvectorfont="no"/></settings>'
 '<grid distance="0.1" unitdist="inch" unit="inch" style="lines" multiple="1" display="no" '
 'altdistance="0.01" altunitdist="inch" altunit="inch"/>')


_R3D_CACHE = None  # loaded once per bridge process


def _mtp_resistor():
    """The REAL series resistor for test projects: the wiki basic part Royalohm
    0603WAF3300T5E (330R 0603), bundled in resources/samples (John, 2026-08-15: a
    synthetic 3D-less resistor 'proves' nothing - real parts only). Returns
    (extract, lbr_text) or (None, None) when the bundle is missing.
    Its 3D urn is hub-scoped, so it is minted ONCE per box (make_3d_package on the
    bundled lbr auto-caches it in ~/.adom/fusion-bridge/r3d-urn.json) and reused."""
    import os
    p = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                     "resources", "samples", "0603WAF3300T5E.lbr")
    if not os.path.isfile(p):
        return None, None
    txt = open(p, encoding="utf-8", errors="replace").read()
    ex, err = _mtp_extract(txt, "0603WAF3300T5E")
    return (None, None) if err else (ex, txt)


def _r3d_cached_urn():
    global _R3D_CACHE
    if _R3D_CACHE is not None:
        return _R3D_CACHE
    import json as _json
    import os
    p = os.path.expanduser("~/.adom/fusion-bridge/r3d-urn.json")
    try:
        _R3D_CACHE = _json.load(open(p)).get("0603WAF3300T5E", "")
    except Exception:
        _R3D_CACHE = ""
    return _R3D_CACHE


def _r3d_cache_write(urn):
    import json as _json
    import os
    d = os.path.expanduser("~/.adom/fusion-bridge")
    os.makedirs(d, exist_ok=True)
    _json.dump({"0603WAF3300T5E": urn}, open(os.path.join(d, "r3d-urn.json"), "w"))


def _mtp_generate(lib_name, ex, part_name, urn="", rex=None, r_urn="", solo=True):
    """Build the .sch/.brd pair. R1 is the REAL bundled resistor part (rex) when
    available, falling back to the legacy synthetic ADOMR. Wiring: R1.pad2-side pin ->
    the part's first connected pin; GND -> its second; VCC -> R1's first pin."""
    pins = [p for p, _ in ex["connects"]]
    pads = {p: pad for p, pad in ex["connects"]}
    p1, p2 = pins[0], (pins[1] if len(pins) > 1 else pins[0])
    lib = ('<library name="%s"><packages>%s</packages>' % (lib_name, ex["package"])
           + (('<packages3d><package3d name="' + part_name + '" urn="" wip_urn="' + urn + '" locally_modified="yes" type="model">'
               '<packageinstances><packageinstance name="' + ex["pkgName"] + '"/></packageinstances>'
               '</package3d></packages3d>') if urn else '')
           + '<symbols>%s</symbols><devicesets>%s</devicesets></library>' % (ex["symbol"], ex["deviceset"]))
    if rex:
        r_lib_name, r_ds, r_gate = "adom-resistor", "0603WAF3300T5E", rex["gateName"]
        r_pins = [p for p, _ in rex["connects"]]
        r_pin1, r_pin2 = r_pins[0], (r_pins[1] if len(r_pins) > 1 else r_pins[0])
        r_pads = {p: pad for p, pad in rex["connects"]}
        rlib = ('<library name="adom-resistor"><packages>%s</packages>' % rex["package"]
                + (('<packages3d><package3d name="' + r_ds + '" urn="" wip_urn="' + r_urn + '" locally_modified="yes" type="model">'
                    '<packageinstances><packageinstance name="' + rex["pkgName"] + '"/></packageinstances>'
                    '</package3d></packages3d>') if r_urn else '')
                + '<symbols>%s</symbols><devicesets>%s</devicesets></library>' % (rex["symbol"], rex["deviceset"]))
        r_part = '<part name="R1" library="adom-resistor" deviceset="%s" device="" value="330"/>' % r_ds
        r_pkg_name = rex["pkgName"]
    else:
        r_lib_name, r_ds, r_gate = "adomdemo", "ADOMR", "G$1"
        r_pin1, r_pin2 = "1", "2"
        r_pads = {"1": "1", "2": "2"}
        rlib = ('<library name="adomdemo"><packages>%s</packages><symbols>%s</symbols><devicesets>%s</devicesets></library>'
                % (_MTP_R_PKG, _MTP_R_SYM, _MTP_R_DS))
        r_part = '<part name="R1" library="adomdemo" deviceset="ADOMR" device="" value="330"/>'
        r_pkg_name = "ADOMR0603"
    g = ex["gateName"]
    # Correct EE designator from the deviceset's own prefix (John, 2026-08-16: "why is
    # there a D1 and R1 in a capacitor demo?"): C->C1, D->D1, U->U1; the series resistor
    # is R1, so a resistor-under-test becomes R2.
    pfx = (ex.get("prefix") or "U").strip() or "U"
    part_ref = (pfx + "2") if (pfx.upper() == "R" and not solo) else (pfx + "1")
    # FR4 sized from the package's FULL drawing (silk, docu, courtyard), never a fixed box
    _geo = _test_board_geometry(ex["package"], margin=3.0, with_resistor=not solo)
    if solo:
        # SOLO layout (John, 2026-08-16: "when demoing a single component, you don't need
        # extra components... it's just confusing"): JUST the part - one symbol on the
        # sheet, one element centered on a small board. No resistor, no nets.
        lib_solo = ('<library name="%s"><packages>%s</packages>' % (lib_name, ex["package"])
               + (('<packages3d><package3d name="' + part_name + '" urn="" wip_urn="' + urn + '" locally_modified="yes" type="model">'
                   '<packageinstances><packageinstance name="' + ex["pkgName"] + '"/></packageinstances>'
                   '</package3d></packages3d>') if urn else '')
               + '<symbols>%s</symbols><devicesets>%s</devicesets></library>' % (ex["symbol"], ex["deviceset"]))
        sch = (_MTP_HEAD + _MTP_SCH_LAYERS +
         '<schematic xreflabel="%F%N/%S.%C%R" xrefpart="/%S.%C%R">'
         '<libraries>' + lib_solo + '</libraries>'
         '<attributes/><variantdefs/><classes><class number="0" name="default" width="0" drill="0"/></classes>'
         '<parts><part name="' + part_ref + '" library="' + lib_name + '" deviceset="' + part_name + '" device="' + (ex.get("device") or "") + '"/></parts>'
         '<sheets><sheet><plain>'
         '<text x="0" y="25.4" size="2.54" layer="97">' + part_name + ' - Adom component demo</text>'
         '</plain><instances>'
         '<instance part="' + part_ref + '" gate="' + ex["gateName"] + '" x="25.4" y="12.7"/>'
         '</instances><busses/><nets/></sheet></sheets></schematic></drawing></eagle>\n')
        brd = (_MTP_HEAD + ex["layers"] +
         '<board><plain>' + _test_board_outline(_geo["w"], _geo["h"]) + '</plain>'
         '<libraries><library name="' + lib_name + '"><packages>' + ex["package"] + '</packages>'
         + (('<packages3d><package3d name="' + part_name + '" urn="" wip_urn="' + urn + '" locally_modified="yes" type="model">'
             '<packageinstances><packageinstance name="' + ex["pkgName"] + '"/></packageinstances>'
             '</package3d></packages3d>') if urn else '')
         + '</library></libraries>'
         '<attributes/><variantdefs/><classes><class number="0" name="default" width="0" drill="0"/></classes>'
         '<designrules name="default"/><autorouter/>'
         '<elements><element name="' + part_ref + '" library="' + lib_name + '" package="' + ex["pkgName"] + '" value="" x="' + str(_geo["px"]) + '" y="' + str(_geo["py"]) + '"'
         + ((' package3d_urn="' + urn + '"') if urn else '') + '/></elements>'
         '<signals/></board></drawing></eagle>\n')
        return sch, brd
    sch = (_MTP_HEAD + _MTP_SCH_LAYERS +
     '<schematic xreflabel="%F%N/%S.%C%R" xrefpart="/%S.%C%R">'
     '<libraries>' + lib + rlib + '</libraries>'
     '<attributes/><variantdefs/><classes><class number="0" name="default" width="0" drill="0"/></classes>'
     '<parts>'
     '<part name="' + part_ref + '" library="' + lib_name + '" deviceset="' + part_name + '" device="' + (ex.get("device") or "") + '"/>'
     + r_part +
     '</parts><sheets><sheet><plain>'
     '<text x="0" y="30.48" size="2.54" layer="97">Adom generated test circuit: ' + part_name + '</text>'
     '</plain><instances>'
     '<instance part="R1" gate="' + r_gate + '" x="12.7" y="12.7"/>'
     '<instance part="' + part_ref + '" gate="' + g + '" x="38.1" y="12.7"/>'
     '</instances><busses/><nets>'
     '<net name="N1" class="0"><segment>'
     '<wire x1="17.78" y1="12.7" x2="25.4" y2="12.7" width="0.1524" layer="91"/>'
     '<pinref part="R1" gate="' + r_gate + '" pin="' + r_pin2 + '"/><pinref part="' + part_ref + '" gate="' + g + '" pin="' + p1 + '"/>'
     '</segment></net>'
     '<net name="VCC" class="0"><segment>'
     '<wire x1="7.62" y1="12.7" x2="2.54" y2="12.7" width="0.1524" layer="91"/>'
     '<pinref part="R1" gate="' + r_gate + '" pin="' + r_pin1 + '"/></segment></net>'
     '<net name="GND" class="0"><segment>'
     '<pinref part="' + part_ref + '" gate="' + g + '" pin="' + p2 + '"/></segment></net>'
     '</nets></sheet></sheets></schematic></drawing></eagle>\n')
    brd = (_MTP_HEAD + ex["layers"] +
     '<board><plain>' + _test_board_outline(_geo["w"], _geo["h"]) + '</plain>'
     '<libraries>' + lib.replace('<symbols>' + ex["symbol"] + '</symbols><devicesets>' + ex["deviceset"] + '</devicesets>', '')
     + rlib.replace('<symbols>' + (rex["symbol"] if rex else _MTP_R_SYM) + '</symbols><devicesets>' + (rex["deviceset"] if rex else _MTP_R_DS) + '</devicesets>', '')
     + '</libraries>'
     '<attributes/><variantdefs/><classes><class number="0" name="default" width="0" drill="0"/></classes>'
     '<designrules name="default"/><autorouter/>'
     '<elements>'
     '<element name="R1" library="' + r_lib_name + '" package="' + r_pkg_name + '" value="330" x="' + str(_geo["rx"]) + '" y="' + str(_geo["ry"]) + '"'
     + ((' package3d_urn="' + r_urn + '"') if (rex and r_urn) else '') + '/>'
     '<element name="' + part_ref + '" library="' + lib_name + '" package="' + ex["pkgName"] + '" value="" x="' + str(_geo["px"]) + '" y="' + str(_geo["py"]) + '"'
     + ((' package3d_urn="' + urn + '"') if urn else '') + '/>'
     '</elements><signals>'
     '<signal name="N1"><contactref element="R1" pad="' + r_pads[r_pin2] + '"/><contactref element="' + part_ref + '" pad="' + pads[p1] + '"/>'
     + ('<wire x1="%s" y1="%s" x2="%s" y2="%s" width="0.3" layer="1"/></signal>' % _geo["wire"]) +
     '<signal name="VCC"><contactref element="R1" pad="' + r_pads[r_pin1] + '"/></signal>'
     '<signal name="GND"><contactref element="' + part_ref + '" pad="' + pads[p2] + '"/></signal>'
     '</signals></board></drawing></eagle>\n')
    return sch, brd


def _pkg_extent_mm(package_xml: str):
    """(minx, miny, maxx, maxy) of EVERYTHING a package draws: pads, smds, wires, circles,
    rectangles, polygons and text on ANY layer (silkscreen, docu, courtyard included). The
    test board's FR4 must enclose all of it (John, 2026-08-30: "make the fr4 big enough so i
    can see the whole silkscreen ... pin 1 indicators")."""
    import re as _re
    xs, ys = [], []
    try:
        for m in _re.finditer(r'<smd [^>]*x="([-0-9.]+)" y="([-0-9.]+)" dx="([-0-9.]+)" dy="([-0-9.]+)"', package_xml):
            x, y, dx, dy = map(float, m.groups())
            r = max(dx, dy) / 2.0   # rotation-safe
            xs += [x - r, x + r]; ys += [y - r, y + r]
        for m in _re.finditer(r'<pad [^>]*x="([-0-9.]+)" y="([-0-9.]+)"[^>]*?(?:diameter="([-0-9.]+)")?', package_xml):
            x, y = float(m.group(1)), float(m.group(2)); r = (float(m.group(3)) if m.group(3) else 1.0) / 2.0
            xs += [x - r, x + r]; ys += [y - r, y + r]
        for m in _re.finditer(r'<wire x1="([-0-9.]+)" y1="([-0-9.]+)" x2="([-0-9.]+)" y2="([-0-9.]+)" width="([-0-9.]+)"', package_xml):
            x1, y1, x2, y2, w = map(float, m.groups())
            xs += [x1 - w, x1 + w, x2 - w, x2 + w]; ys += [y1 - w, y1 + w, y2 - w, y2 + w]
        for m in _re.finditer(r'<circle x="([-0-9.]+)" y="([-0-9.]+)" radius="([-0-9.]+)" width="([-0-9.]+)"', package_xml):
            x, y, r, w = map(float, m.groups()); r += w
            xs += [x - r, x + r]; ys += [y - r, y + r]
        for m in _re.finditer(r'<rectangle x1="([-0-9.]+)" y1="([-0-9.]+)" x2="([-0-9.]+)" y2="([-0-9.]+)"', package_xml):
            x1, y1, x2, y2 = map(float, m.groups()); xs += [x1, x2]; ys += [y1, y2]
        for m in _re.finditer(r'<vertex x="([-0-9.]+)" y="([-0-9.]+)"', package_xml):
            xs.append(float(m.group(1))); ys.append(float(m.group(2)))
        # >NAME / >VALUE labels are movable and sit outside the part; they do not size the board.
    except Exception:
        pass
    if not xs or not ys:
        return (-3.0, -3.0, 3.0, 3.0)
    return (min(xs), min(ys), max(xs), max(ys))


def _test_board_geometry(package_xml: str, margin: float = 3.0, with_resistor: bool = False) -> dict:
    """Board outline + element positions so the whole package (silk, docu, courtyard) sits on
    FR4 with `margin` mm around it; the series resistor, when present, gets its own bay on the
    left. Returns {w, h, px, py, rx, ry, wire}."""
    x0, y0, x1, y1 = _pkg_extent_mm(package_xml)
    pw, ph = (x1 - x0), (y1 - y0)
    bay = 7.0 if with_resistor else 0.0        # room for a 0603 + its own margin
    w = round(bay + pw + 2 * margin, 2)
    h = round(max(ph + 2 * margin, 9.0 if with_resistor else 0.0), 2)
    px = round(bay + margin - x0, 3)           # element origin so its minx lands at bay+margin
    py = round(h / 2.0 - (y0 + y1) / 2.0, 3)   # vertically centred
    rx, ry = 3.5, round(h / 2.0, 3)
    return {"w": w, "h": h, "px": px, "py": py, "rx": rx, "ry": ry,
            "wire": (rx + 0.85, ry, round(px + x0 - 0.3, 3), ry), "extent": [x0, y0, x1, y1]}


def _test_board_outline(w: float, h: float) -> str:
    return ('<wire x1="0" y1="0" x2="%s" y2="0" width="0.2" layer="20"/>'
            '<wire x1="%s" y1="0" x2="%s" y2="%s" width="0.2" layer="20"/>'
            '<wire x1="%s" y1="%s" x2="0" y2="%s" width="0.2" layer="20"/>'
            '<wire x1="0" y1="%s" x2="0" y2="0" width="0.2" layer="20"/>' % (w, w, w, h, w, h, h, h))


_TEST_PROJECT: dict = {}
try:    # restore the last generated test project so the DESIGN surfaces (project/schematic/
        # 2d/3d) survive a bridge respawn - an install used to wipe this and those four verbs
        # failed with "query or urn required" until something re-staged (ralph, 2026-08-18).
    _tp = _get_bridge_pref("testProject", None)
    if isinstance(_tp, dict) and _tp.get("sch") and _tp.get("brd"):
        if os.path.isfile(_tp["sch"]) and os.path.isfile(_tp["brd"]):
            _TEST_PROJECT = _tp
except Exception:
    pass


def _test_project_activate(kind):
    """Activate the test project's sch or brd DOCUMENT (newDesignFromLocal re-open =
    activate). Returns True when the doc is confirmed active."""
    import time as _t
    if not _TEST_PROJECT:
        return False
    path = _TEST_PROJECT.get(kind)
    if not path:
        return False
    _proxy_to_addin("execute_text_command", {"command": "Document.newDesignFromLocal %s" % _fusion_text_path(path)}, timeout=60)
    for _ in range(6):
        _t.sleep(2)
        st = _proxy_to_addin("get_app_state", {}, timeout=10)
        if _TEST_PROJECT["docBase"] in str((st.get("data") or {}).get("activeDocument", "")).lower():
            return True
    return False


def _orchestrate_make_test_project(args: dict) -> dict:
    """Generate + import a minimal test project around a library part.

    args: libraryPath (required, Windows path .lbr), deviceset (optional: defaults to
    the library's first deviceset).
    """
    import os
    import tempfile
    lbr_path = (args.get("libraryPath") or "").replace("\\", "/")
    if not lbr_path:
        return {"success": False, "error": "libraryPath (.lbr, Windows path) required.",
                "_hint": "Stage the component page's .lbr onto the box first (send_files)."}
    try:
        lbr_text = open(lbr_path, encoding="utf-8", errors="replace").read()
    except Exception as e:
        return {"success": False, "error": "could not read %s: %s" % (lbr_path, e)}
    deviceset = args.get("deviceset")
    if not deviceset:
        m = re.search(r'<deviceset name="([^"]+)"', lbr_text)
        if not m:
            return {"success": False, "error": "no deviceset found in the library"}
        deviceset = m.group(1)
    ex, err = _mtp_extract(lbr_text, deviceset, args.get("device"))
    if err:
        return {"success": False, "error": err}
    lib_name = re.sub(r"[^A-Za-z0-9_-]", "_", os.path.basename(lbr_path).rsplit(".", 1)[0])
    # The source library's 3D binding (if any) rides into the generated BOARD - each
    # element gets package3d_urn so the 3D PCB renders the REAL component model, not a
    # generic extrusion (John caught every test board showing black boxes, 2026-08-15).
    # THIS package's 3D model, not the first urn in the file (variants each bind their own)
    urn_m = None
    try:
        _pk_esc = re.escape(ex["pkgName"])
        for _p3 in re.finditer(r'<package3d [^>]*wip_urn="([^"]+)"[^>]*>(.*?)</package3d>', lbr_text, re.S):
            if re.search(r'<packageinstance name="%s"/>' % _pk_esc, _p3.group(2)):
                urn_m = re.match(r'(.*)', _p3.group(1)); break
    except Exception:
        urn_m = None
    if urn_m is None:
        urn_m = re.search(r'wip_urn="([^"]+)"', lbr_text)
    solo = not bool(args.get("withResistor"))
    rex, _rtxt = (None, None) if solo else _mtp_resistor()
    r_urn = _r3d_cached_urn() if rex else ""
    sch_text, brd_text = _mtp_generate(lib_name, ex, deviceset,
                                       urn_m.group(1) if urn_m else "", rex=rex, r_urn=r_urn, solo=solo)
    d = os.path.join(tempfile.gettempdir(), "adom-fusion-wrap")
    os.makedirs(d, exist_ok=True)
    base = "ADOM-TEST-" + re.sub(r"[^A-Za-z0-9_-]", "_", deviceset)[:40]
    sch_path = os.path.join(d, base + ".sch").replace("\\", "/")
    brd_path = os.path.join(d, base + ".brd").replace("\\", "/")
    open(sch_path, "w", encoding="utf-8").write(sch_text)
    open(brd_path, "w", encoding="utf-8").write(brd_text)
    # ⛔ DIALOG-FREE ONLY (John, 2026-08-15, after ImportSCHAndBRDCmd's native Open dialogs
    # wedged his Fusion): Document.newDesignFromLocal opens each file with ZERO dialogs and
    # ZERO foreground - the same primitive open_lbr/open_board use. The .brd (elements
    # placed + embedded libraries) INSTANTIATES the board; the .sch opens as its own
    # drawn schematic document. Never route this through new_electronics_from_eagle.
    import time as _t
    steps = []
    for label, fpath in (("board", brd_path), ("schematic", sch_path)):
        r = _proxy_to_addin("execute_text_command",
                            {"command": "Document.newDesignFromLocal %s" % _fusion_text_path(fpath)}, timeout=120)
        okd = bool(r.get("success"))
        if not okd:
            expected = fpath.rsplit("/", 1)[-1].rsplit(".", 1)[0].lower()
            for _ in range(10):
                _t.sleep(3)
                st = _proxy_to_addin("get_app_state", {}, timeout=10)
                if expected in str((st.get("data") or {}).get("activeDocument", "")).lower():
                    okd = True
                    break
        steps.append("%s open: %s" % (label, "ok" if okd else "FAILED: " + str(r.get("error"))[:80]))
        if not okd:
            return {"success": False, "schPath": sch_path, "brdPath": brd_path,
                    "error": "%s open failed: %s" % (label, str(r.get("error"))[:150]),
                    "data": {"steps": steps}}
        _t.sleep(2)
    global _TEST_PROJECT
    _TEST_PROJECT = {"sch": sch_path, "brd": brd_path,
                     "docBase": base.lower()}
    # PERSIST: the design surfaces (show_project/schematic/2d/3d) resolve through this handle,
    # and a bridge respawn - which happens on every install - used to wipe it, so those four
    # surfaces failed with "query or urn required" until the caller re-staged (ralph, 2026-08-18).
    # The paths live on the box, so they stay valid across respawns.
    try:
        _set_bridge_pref("testProject", _TEST_PROJECT)
    except Exception:
        pass
    try:
        _note_bridge_doc(base)  # generated test docs = bridge docs (sentinel may discard)
    except Exception:
        pass
    out = {"success": True, "schPath": sch_path, "brdPath": brd_path,
           "deviceset": deviceset, "steps": steps,
           "circuit": ("JUST %s on a small board (solo demo layout)" % deviceset) if solo
                      else "R1 (330) in series with %s; VCC/GND stubs" % deviceset,
           "_hint": ("Test project opened as TWO documents (board instantiated with placed parts, "
                     "schematic as its drawn sheet) via the dialog-free Document.newDesignFromLocal "
                     "path. The BOARD document is active: fusion_show_2d_board / fusion_show_3d_board "
                     "work now; for the schematic view activate its document (it opened last... the "
                     "SCHEMATIC document is actually active) - board legs may need the board doc "
                     "re-activated via Document.newDesignFromLocal or open_board. Generated from "
                     "NOTHING but the component's .lbr - no cloud, no sign-in.")}
    return out


def _sweep_untitled_scratch(max_close: int = 24) -> list:
    """Close UNMODIFIED 'Untitled' scratch documents (page issue #59, 2026-08-26).

    The library/bind flow left ~5 Untitled docs per import - 16 after three - and no
    caller could clear them, because close_all_documents skipped every Untitled. They are
    scratch by construction: an UNMODIFIED Untitled has no data file and no edits, so
    closing one cannot lose anything. Modified ones are never touched here; those may hold
    real unsaved work and need an explicit force. Fusion keeps a single blank doc alive on
    its own, which is the 'keep exactly one' the reporter asked for.
    """
    closed = []
    try:
        prev_total = None
        for _ in range(max_close):
            st = _proxy_to_addin("document_info", {}, timeout=8)
            docs = (st.get("data") or {}).get("openDocuments") or []
            untitled = [d for d in docs
                        if str(d.get("name") or "").lower().startswith("untitled")
                        and not (d.get("isModified") or d.get("modified"))]
            # ⚠ FUSION RE-CREATES A BLANK DOC whenever the last one closes. The add-in's own
            # close_all_documents carries this warning ("do NOT loop on documents.count ...
            # turns while count > 0 into an infinite loop") and I reintroduced it here: on a
            # workspace holding ONE Untitled this closed it 24 times, each time racing a
            # fresh blank, and terminated only because it hit the cap. Leaving the last
            # Untitled alone is also the correct end state - Fusion keeps exactly one.
            if len(untitled) <= 1 and len(docs) == len(untitled):
                break
            if not untitled:
                break
            # NO-PROGRESS GUARD: if the total did not fall after a close, we are fighting the
            # re-create rather than draining a backlog. Stop instead of spinning to the cap.
            if prev_total is not None and len(docs) >= prev_total:
                break
            prev_total = len(docs)
            victim = untitled[0]
            r = _proxy_to_addin("close_document",
                                {"name": victim.get("name"), "save": False}, timeout=20)
            if not r.get("success"):
                break            # stop on the first refusal rather than spin
            closed.append(str(victim.get("name")))
        if closed:
            print("[Fusion Bridge] swept %d Untitled scratch doc(s): %s"
                  % (len(closed), ", ".join(closed[:6])))
            _bridge_action_ledger("untitled_scratch_swept", {"count": len(closed)})
    except Exception:
        pass
    return closed


def _orchestrate_bind_library_3d(args: dict) -> dict:
    """ONE CALL for the correct library (John's bar: the 3D renders in the Content
    Manager): mint the part's 3D as a cloud f3d, inject the wip_urn binding into the
    .lbr ON DISK, and reopen the library. Needs a signed-in Fusion session (the one
    cloud-dependent step in the circle).

    args: {libraryPath: Windows .lbr (required), stepPath: Windows .step (required),
           deviceset: optional (defaults to the library's first)}
    """
    import time as _t
    lbr_path = (args.get("libraryPath") or "").replace("\\", "/")
    step_path = (args.get("stepPath") or "").replace("\\", "/")
    if not (lbr_path and step_path):
        return {"success": False, "error": "libraryPath (.lbr) and stepPath (.step) required (Windows paths)."}
    try:
        txt = open(lbr_path, encoding="utf-8", errors="replace").read()
    except Exception as e:
        return {"success": False, "error": "cannot read %s: %s" % (lbr_path, e)}
    deviceset = args.get("deviceset")
    if not deviceset:
        m = re.search(r'<deviceset name="([^"]+)"', txt)
        if not m:
            return {"success": False, "error": "no deviceset in the library"}
        deviceset = m.group(1)
    pkg_m = re.search(r'<package name="([^"]+)"', txt)
    pkg_name = pkg_m.group(1) if pkg_m else deviceset
    # IDEMPOTENT (caught live 2026-08-21): re-binding an already-bound library re-uploaded
    # the f3d and hit a 409 name collision in the cloud folder - failing a chain that had
    # NOTHING left to do. A .lbr whose package3d already carries a urn is bound; verify and
    # return success instead of re-minting.
    _existing = re.search(r'<package3d[^>]*urn="([^"]+)"', txt)
    if _existing and not args.get("force"):
        return {"success": True, "alreadyBound": True, "wip_urn": _existing.group(1),
                "deviceset": deviceset, "libraryPath": lbr_path,
                "_hint": ("This library already carries a package3d urn - nothing to mint. "
                          "Pass force:true to re-bind (e.g. after replacing the STEP). "
                          "fusion_check_3d_thumbnail verifies the cloud thumbnail rendered.")}
    mint = _apply_failure_dialogs(_orchestrate_make_3d_package(
        {"lbrPath": lbr_path, "modelPath": step_path,
         "fileName": "ADOM-%s-3D" % re.sub(r"[^A-Za-z0-9_-]", "_", deviceset)[:40]}))
    urn = mint.get("wip_urn") or ""
    if not urn:
        return {"success": False, "error": "mint failed: " + str(mint.get("error"))[:150],
                "_hint": str(mint.get("_hint") or "")[:150]}
    # WRITE-BACK NOTICE (#52 item 8): the minted binding is written INTO the caller's
    # .lbr on purpose (the file on disk is what Fusion opens, and it must open BOUND) -
    # but silently mutating an input cost the matrix runner a pristine-restore ritual.
    # Keep a one-time .prebind backup beside it and SAY the mutation out loud.
    try:
        import shutil as _sh
        _bak = lbr_path + ".prebind"
        if not os.path.exists(_bak):
            _sh.copyfile(lbr_path, _bak)
    except Exception:
        pass
    txt = re.sub(r'<packages3d>.*?</packages3d>\n?', '', txt, flags=re.S)
    binding = ('<packages3d><package3d name="%s" urn="" wip_urn="%s" locally_modified="yes" type="model">'
               '<packageinstances><packageinstance name="%s"/></packageinstances>'
               '</package3d></packages3d>' % (deviceset, urn, pkg_name))
    txt = txt.replace('</packages>', '</packages>\n' + binding, 1)
    txt = re.sub(r'<package3dinstances>.*?</package3dinstances>', '', txt, flags=re.S)
    txt = txt.replace('<technologies>',
                      '<package3dinstances><package3dinstance package3d_urn="%s"/></package3dinstances><technologies>' % urn, 1)
    try:
        open(lbr_path, "w", encoding="utf-8").write(txt)
    except Exception as e:
        return {"success": False, "error": "binding write failed: %s" % e, "wip_urn": urn}
    _t.sleep(1)
    reopened = _orchestrate_open_lbr({"filePath": lbr_path})
    _reopen_ok = bool(reopened.get("success"))
    # THE BIND ITSELF IS DONE once the urn is minted AND written to the .lbr on disk
    # (#49 / #47 failure-mode-3, John 2026-08-23). The re-open here is only a refresh so the
    # prove stage can screenshot the 3D - its failure must NEVER (a) present as a bind failure
    # with error:null, nor (b) discard a real, minted, PERSISTED urn and abort the chain. Two
    # rules from that report: never return error:null on a failure, and a good urn lets the
    # chain continue. So: success rides on the urn+write (the actual binding work), and the
    # re-open is reported separately with its reason surfaced.
    if not _reopen_ok:
        _why = str(reopened.get("error") or reopened.get("_hint")
                   or "library re-open returned success:false with no detail (open_lbr)")[:220]
        return {"success": True, "wip_urn": urn, "libraryPath": lbr_path,
                "deviceset": deviceset, "bound": True, "reopened": False, "mutatedInput": True,
                "prebindBackup": lbr_path + ".prebind",
                "reopenError": _why,
                "_hint": ("Binding minted (%s) and WRITTEN to the .lbr - the bind is real and "
                          "persisted. Re-opening the library into Fusion failed (reopenError "
                          "above); the prove stage opens the library itself, so the chain "
                          "continues. If prove's 3D frame is missing, the library just needs a "
                          "clean re-open once Fusion is idle." % urn)}
    # #59: the mint spawns scratch Untitled docs. Clear them before returning so a batch
    # caller's workspace stays bounded instead of growing ~5 docs per import.
    _swept = _sweep_untitled_scratch()
    return {"success": True, "wip_urn": urn, "mutatedInput": True,
            "untitledScratchClosed": len(_swept),
            "prebindBackup": lbr_path + ".prebind",
            "libraryPath": lbr_path, "deviceset": deviceset,
            "bound": True, "reopened": True,
            "_hint": ("NOTE: the input .lbr was REWRITTEN with the minted package3d binding "
                      "(account-scoped; pristine copy kept at <libraryPath>.prebind). "
                      "Library re-opened WITH the 3D binding. The Content Manager's 3D preview "
                      "resolves after Autodesk's thumbnail service catches up (~2-3 min for a "
                      "fresh mint) - EDIT %s.dev then screenshot to verify the render." % deviceset)}


def _lbr_has_package3d(lbr_path: str):
    """Does this .lbr carry a bound package3d? Returns True/False, or None when the file
    cannot be read (unknown is NOT False - a caller must be able to tell them apart)."""
    try:
        txt = open(lbr_path, encoding="utf-8", errors="replace").read()
    except Exception:
        return None
    m = re.search(r'<package3d[^>]*(?:\burn|\bwip_urn)="([^"]+)"', txt)
    return bool(m and m.group(1).strip())


def _orchestrate_show_library(args: dict) -> dict:
    """Eight-surfaces contract (ab SDK, issue #30): the LIBRARY surface by its smallest
    name. libraryPath opens it (open_lbr); name alone handles the already-open case."""
    import time as _t
    lbr = (args.get("libraryPath") or "").replace("\\", "/")
    name = (args.get("name") or "").strip()
    if lbr:
        opened = _orchestrate_open_lbr({"filePath": lbr})
        if not opened.get("success"):
            return {"success": False, "error": "open_lbr failed: " + str(opened.get("error"))[:150]}
        _t.sleep(2)
    elif name:
        st = _proxy_to_addin("get_app_state", {}, timeout=10)
        active = str((st.get("data") or {}).get("activeDocument", "")).lower()
        if name.lower() not in active:
            return {"success": False,
                    "error": "library %r is not the active document and no libraryPath was given" % name,
                    "_hint": "Pass libraryPath (WINDOWS .lbr path) to open it; name alone only handles the already-open case."}
    else:
        return {"success": False, "error": "libraryPath or name required."}
    # ── FINDING 2 (#58): ASSERT THE LIBRARY IS ACTUALLY THE ACTIVE DOCUMENT ────────────
    # One call returned success:true with Fusion sitting on an empty Untitled(1) DESIGN
    # tab, no Electronics panels at all, and nothing in the response distinguished it from
    # the nine that worked. Worse, the reporter's blank-frame smoke alarm RATED IT BEST:
    # 99.5% ink coverage vs 94.5-94.7% for the real ones, because a uniform dark canvas is
    # "inked" everywhere. A screenshot cannot be the proof here; the document name can.
    want = (name or (lbr.rsplit("/", 1)[-1].rsplit(".", 1)[0] if lbr else "")).strip()
    active = ""
    try:
        st = _proxy_to_addin("get_app_state", {}, timeout=12)
        active = str((st.get("data") or {}).get("activeDocument", "") or "")
    except Exception:
        pass
    if want and active and want.lower() not in active.lower():
        return {"success": False, "errorCode": "library_not_active",
                "error": "opened %r but the ACTIVE document is %r - the view you would "
                         "screenshot is not the library." % (want, active),
                "activeDocument": active, "expected": want,
                "_hint": ("Fusion switched or left another tab in front. Re-call "
                          "fusion_show_library {libraryPath} once; if it repeats, close the "
                          "stray document (fusion_close_document) and retry.")}

    out = {"success": True, "view": "library", "activeDocument": active or None}

    # ── THE HINT REQUEST (#58): say when there is no 3D to show ────────────────────────
    # The reporter called this verb alone, got a grey Placeholder in the 3D pane on 10/10
    # parts, and filed it as a bridge bug before finding their own documentation saying
    # fusion_bind_library_3d must run first. They are right that the bridge KNOWS this and
    # said nothing. threeDBound is null when unknowable (name-only call), never False.
    bound = _lbr_has_package3d(lbr) if lbr else None
    out["threeDBound"] = bound
    if bound is False:
        out["_hint"] = ("This library has NO package3d, so Fusion renders a grey Placeholder "
                        "slab in the 3D pane - that is not a captured 3D artifact. Run "
                        "fusion_bind_library_3d {libraryPath, stepPath} FIRST, then re-open. "
                        "(fusion_create_library does the whole chain for you.)")
    elif bound is None and lbr:
        out["_hint"] = ("Could not read the .lbr to tell whether a package3d is bound; if the "
                        "3D pane shows a grey Placeholder, run fusion_bind_library_3d first.")

    # ── FINDING 3 (#58): ten calls left ten tabs open ──────────────────────────────────
    # Not closed automatically: showing a library is a "leave it on screen" verb, so
    # closing it would defeat the call. Reported instead, so a batch caller can see the
    # accumulation it is causing rather than discovering it ten tabs later.
    try:
        _docs = _open_doc_names()
        if len(_docs) > 3:
            out["openDocuments"] = len(_docs)
            out["_hintDocs"] = ("%d documents are open in Fusion. show_library deliberately "
                                "leaves the library on screen, so a batch of calls accumulates "
                                "tabs - close them with fusion_close_all_documents between "
                                "runs." % len(_docs))
    except Exception:
        pass

    try:
        shot = _handle_screenshot_fusion({}, {})
        p = shot.get("savedTo") or shot.get("localSafePath")
        if shot.get("success") and p:
            out["screenshot"] = p
    except Exception:
        pass
    return out


def _orchestrate_show_project(args: dict) -> dict:
    """Eight-surfaces contract: bring the PROJECT/design to its top level.
    - No args + a generated test project in this session: activates its board doc.
    - query: already-active match wins; else fire open_cloud_file + poll.
    - urn: open_by_urn passthrough."""
    import time as _t
    query = (args.get("query") or args.get("fileName") or "").strip()
    urn = (args.get("urn") or "").strip()
    if not query and not urn:
        if _TEST_PROJECT:
            ok = _test_project_activate("brd")
            out = {"success": bool(ok), "view": "project", "document": _TEST_PROJECT["docBase"],
                   "source": "generated test project"}
            if not ok:
                out["error"] = "could not activate the test project's board document"
            else:
                try:
                    shot = _handle_screenshot_fusion({}, {})
                    p = shot.get("savedTo") or shot.get("localSafePath")
                    if shot.get("success") and p:
                        out["screenshot"] = p
                except Exception:
                    pass
            return out
        return {"success": False, "error": "query (cloud design name) or urn required - or run fusion_make_test_project first."}
    if urn:
        return dispatch_command("open_by_urn", {"urn": urn})
    st = _proxy_to_addin("get_app_state", {}, timeout=10)
    active = str((st.get("data") or {}).get("activeDocument", "")).lower()
    if query.lower() in active:
        out = {"success": True, "view": "project", "document": active, "source": "already open"}
    else:
        dispatch_command("open_cloud_file", {"fileName": query, "wait": False})
        deadline = _t.time() + 170
        opened = False
        while _t.time() < deadline:
            st = _proxy_to_addin("get_app_state", {}, timeout=10)
            if query.lower() in str((st.get("data") or {}).get("activeDocument", "")).lower():
                opened = True
                break
            _t.sleep(5)
        if not opened:
            return {"success": False, "error": "%r never became the active document within 170s" % query,
                    "_hint": "Check the name with fusion_aps_search; cloud opens on cold sessions take minutes."}
        out = {"success": True, "view": "project", "document": query, "source": "cloud open"}
    try:
        shot = _handle_screenshot_fusion({}, {})
        p = shot.get("savedTo") or shot.get("localSafePath")
        if shot.get("success") and p:
            out["screenshot"] = p
    except Exception:
        pass
    return out


def _orchestrate_demo(args: dict) -> dict:
    """FIRST-TIME-USER DEMO: take a brand-new user from nothing to "wow" in one verb.

    Written because HD's installer ran a "demo" that just opened Fusion and SAT on the
    sign-in page doing nothing (John, 2026-07-20). A demo must actually finish the sign-in,
    set up APS, and SHOW the user real work: an electronics PROJECT -> SCHEMATIC -> 2D BOARD
    -> 3D BOARD, plus a live APS cloud search.

    STAGED + RESUMABLE. Each call advances as far as it safely can and returns:
      stage      - what it just did / is waiting on
      done       - whether the demo finished
      narrate    - a first-time-user-friendly line the CALLER should say + toast
      screenshots- images to show the user for this stage
      _hint      - exactly what the AI must do next (including nbrowser_* for the browser leg)

    The bridge owns Fusion; it does NOT own the browser. So when the sign-in needs a browser
    we hand the caller precise `nbrowser_*` (ABE) instructions - the user's NATIVE browser is
    already signed into Autodesk, whereas pup is anonymous and would force a fresh login.
    """
    stage = (args.get("stage") or "auto").strip()
    demo_query = args.get("query") or ""          # optional: which design to demo
    steps: list = []
    shots: list = []

    def _shot(label):
        try:
            r = _handle_screenshot_fusion({}, {})
            p = r.get("savedTo") or r.get("localSafePath")
            if r.get("success") and p:
                shots.append({"title": label, "label": label, "path": p})
        except Exception:
            pass

    def _out(stage_name, narrate, hint, done=False, **extra):
        return {"success": True, "demo": True, "stage": stage_name, "done": done,
                "steps": steps, "narrate": narrate, "screenshots": shots,
                "statusVerb": "fusion_demo", "_hint": hint, **extra}

    # ── 1. Fusion present + running ─────────────────────────────────────────────
    rd = _handle_fusion_readiness({}, {})
    if not rd.get("installed"):
        return _out("not_installed",
            "Fusion 360 isn't installed yet - want me to install it? It's a free 30-day trial with "
            "everything switched on, so the whole tour works: real boards, schematics, 3D, Gerber and "
            "BOM exports, cloud search. After the trial, Fusion for Personal Use stays free for "
            "non-commercial work (hobby PCBs up to 2 layers), and even an expired install can still "
            "OPEN and VIEW your designs.",
            "OFFER the install - do not just report it is missing. Say what they GET: the free 30-day "
            "trial is FULL-FEATURED (every step of this demo works: electronics, schematic, 2D/3D "
            "board, Gerbers/BOM/CPL, APS cloud search). Be honest about after: Fusion for Personal Use "
            "is free for non-commercial use but LIMITS electronics (about 2 layers / 2 schematic "
            "sheets / small board area) and some exports; an expired or read-only install can still "
            "OPEN + VIEW + browse designs (it only blocks save/export/modify), so their work is never "
            "locked away. Then call fusion_install_fusion (no shell approval needed; streams the "
            "installer, 10-30 min), poll fusion_readiness until installed:true, and call fusion_demo "
            "again to continue the tour. Full flow: the fusion-onboarding skill.")
    if not rd.get("running"):
        steps.append("launched Fusion")
        _handle_launch({}, {})
        rd = _handle_fusion_readiness({}, {})

    # ── 2. SIGN-IN: finish it, do not sit on it ─────────────────────────────────
    if rd.get("needsSignin"):
        # Delegate to the multi-profile sign-in fix (reads Fusion's OAuth URL out of the WRONG
        # default browser and re-opens it in the RIGHT profile). Returns its own rich stage.
        si = _orchestrate_signin({"profile": args.get("signinProfile")})
        si["demo"] = True
        if si.get("stage") in ("opened", "click_signin", "ask_profile", "open_failed"):
            si.setdefault("done", False)
            si["_hint"] = (si.get("_hint", "") + "  (This is the fusion_demo sign-in stage - after "
                           "the user is signed in and fusion_readiness is ready:true, call fusion_demo "
                           "again to continue the tour: APS -> project -> schematic -> 2D -> 3D.)")
            return si
        # DO IT, don't just describe it: open the Autodesk sign-in in the user's OWN browser,
        # in the BACKGROUND so we never yank them out of what they're doing. Then tell the AI
        # exactly WHERE it is waiting and offer the three ways forward.
        nb = _ab_call("nbrowser_readiness", {}, timeout=40)
        nb_data = nb.get("data", nb) if isinstance(nb, dict) else {}
        nb_state = nb_data.get("state")
        opened, where, profile_used = False, "", ""
        pick_reason, ambiguous = "", False
        choices = []
        if nb_state == "ready":
            profs = _ab_call("nbrowser_profiles", {}, timeout=40)
            pdata = profs.get("data", profs) if isinstance(profs, dict) else {}
            # profiles are DICTS: {profile,label,email,browser,displayName,profileDir,active,live,...}
            for p in (pdata.get("profiles") or []):
                if not isinstance(p, dict):
                    continue
                if p.get("blocked") or p.get("unresolved") or not p.get("profile"):
                    continue
                choices.append({
                    "profile": p.get("profile"), "browser": (p.get("browser") or "").lower(),
                    "email": p.get("email") or "", "displayName": p.get("displayName") or "",
                    "profileDir": p.get("profileDir") or "",
                    "active": bool(p.get("active")), "live": bool(p.get("live")),
                    "asleep": bool(p.get("asleep")),
                    "extensionInstalled": p.get("extensionInstalled", True),
                    "describe": _describe_profile(p),
                })
            # PROBE each profile for a REAL Autodesk session - never guess by "active".
            # (John, 2026-07-20: picking the active profile grabbed his PERSONAL Chrome with
            # zero analysis. A power user's Autodesk login can live in any profile, so ASK THE
            # BROWSER, don't assume.) nbrowser_login_state reports auth cookies per profile.
            for c in choices:
                if not c["live"]:
                    c["autodesk"] = "unprobed(asleep)"
                    continue
                ls = _ab_call("nbrowser_login_state",
                              {"profile": c["profile"], "url": "https://accounts.autodesk.com/"},
                              timeout=45)
                lsd = ls.get("data", ls) if isinstance(ls, dict) else {}
                if lsd.get("ok") is None and lsd.get("loggedIn") is None:
                    c["autodesk"] = "unprobed"
                else:
                    c["autodesk"] = "signed-in" if lsd.get("loggedIn") else "signed-out"
                c["autodeskConfidence"] = lsd.get("confidence")
                c["autodeskCookies"] = lsd.get("cookieCount")
            _CONF = {"high": 3, "medium": 2, "low": 1}
            signed = [c for c in choices if c.get("autodesk") == "signed-in"]
            signed.sort(key=lambda c: (_CONF.get(c.get("autodeskConfidence"), 0),
                                       c.get("autodeskCookies") or 0), reverse=True)
            pick = signed[0] if signed else None
            if pick:
                pick_reason = ("it is the profile actually signed into Autodesk (%s auth cookies, %s "
                               "confidence)" % (pick.get("autodeskCookies"), pick.get("autodeskConfidence")))
            else:
                # NOTHING is signed in - do not silently guess. Prefer a work/corporate identity
                # over a consumer mailbox (Autodesk seats are usually work accounts), but SAY SO
                # and offer the alternatives.
                _CONSUMER = ("gmail.com", "outlook.com", "hotmail.com", "yahoo.com", "icloud.com", "live.com")
                def _work_first(c):
                    em = (c.get("email") or "").lower()
                    return (0 if (em and not any(em.endswith("@" + d) for d in _CONSUMER)) else 1,
                            0 if c["live"] else 1)
                ranked = sorted([c for c in choices if c["live"]] or choices, key=_work_first)
                pick = ranked[0] if ranked else None
                pick_reason = ("no profile has a live Autodesk session, so I picked the most likely "
                               "one (work/corporate identity first) - CONFIRM WITH THE USER")
            ambiguous = not signed or len(signed) > 1
            if pick:
                profile_used = pick["profile"]
                where = pick["describe"]
            oa = {"sessionId": "fusion-signin", "url": "https://accounts.autodesk.com/",
                  "background": True, "thread": "fusion-demo", "purpose": "Autodesk sign-in for Fusion"}
            if profile_used:
                oa["profile"] = profile_used
            r = _ab_call("nbrowser_open_window", oa, timeout=60)
            opened = bool((r.get("data", r) if isinstance(r, dict) else {}).get("sessionId") or r.get("ok") or r.get("success"))
            if not where:
                where = "your browser"
        if opened:
            steps.append("opened Autodesk sign-in in %s (background)" % (where or "browser"))
        else:
            steps.append("could not auto-open the browser sign-in: %s" % (nb.get("_adCallError") or nb_state or "unknown"))
        if opened:
            narrate = ("I've opened the Autodesk sign-in in %s - I chose that profile because %s. "
                       "It's sitting in the background so it won't interrupt what you're doing. "
                       "Finish it whenever you like, tell me to bring it up front, or I can complete "
                       "it for you. If that's the wrong account, say which profile to use."
                       % (where, pick_reason))
        else:
            narrate = ("Fusion needs your Autodesk sign-in. I'd like to use your own browser - you're "
                       "probably already signed in there, so it should take one click.")
        return _out("signin", narrate,
            ("FINISH THE SIGN-IN - never leave a first-time user parked here. " +
             ("A background sign-in tab is ALREADY OPEN in %s (session 'fusion-signin'). " % where
              if opened else "Auto-open failed - open it yourself: nbrowser_open_window "
              "{sessionId:'fusion-signin', url:'https://accounts.autodesk.com/', background:true, "
              "profile:'<chrome:their-account>'}. ") +
             "The profile was CHOSEN BY EVIDENCE, not guessed: each live profile was probed with "
             "nbrowser_login_state against accounts.autodesk.com, and the one holding a real Autodesk "
             "session wins (see signinProfileReason + per-profile autodesk/autodeskConfidence/"
             "autodeskCookies in signinProfileChoices). RELAY THAT REASON to the user. If "
             "signinProfileAmbiguous is true (nothing signed in, or several are), ASK them which "
             "account their Autodesk login belongs to instead of assuming. "
             "NAME THE EXACT PROFILE - never say just 'your browser' or 'your Chrome'. Power users "
             "run several profiles (personal / work / media), so say the browser AND the identity "
             "from `signinWhere` (e.g. 'Chrome - John Personal (jlauer12@gmail.com)'). "
             "`signinProfileChoices` lists every profile with describe/email/displayName/active - if "
             "there is more than one, say which you used and OFFER TO SWITCH (re-run with a different "
             "profile). If the one you want shows extensionInstalled:false it needs the one-time "
             "extension install in THAT profile. "
             "TELL THE USER WHERE IT IS WAITING and OFFER ALL THREE: (a) they finish it themselves "
             "whenever they want, (b) you foreground that window for them "
             "(nbrowser_switch_window / browser window state), or (c) THEY LET YOU DRIVE IT - ask "
             "first (AskUserQuestion), then click through 'Continue with Google/Apple/Microsoft' "
             "using their warm session. Use the NATIVE browser (ABE), NEVER pup: pup is anonymous so "
             "Autodesk demands a full fresh login, while their real profile is usually already signed "
             "in. NEVER type their password or 2FA - if a secret is demanded, fusion_notify_owner "
             "toasts them to type it. Then click Fusion's own 'Sign In' (the webview exposes no UIA "
             "control, so use an image-space desktop_click on the button, or foreground + click) to "
             "fire the OAuth handoff; accept the 'Autodesk Identity Manager' overlay ('Always allow' "
             "+ 'Open'). Codes expire in ~2 min - if it expires, click Sign In again. Poll "
             "fusion_readiness until ready:true, then call fusion_demo again. Playbook: "
             "fusion-autodesk-signin."),
            needsSignin=True, signinOpened=opened, signinWhere=where,
            signinProfile=profile_used, signinSession="fusion-signin" if opened else None,
            signinProfileChoices=choices, signinProfileReason=pick_reason,
            signinProfileAmbiguous=ambiguous, nbState=nb_state)

    if not rd.get("ready"):
        return _out("waiting",
            "Fusion is starting up - one moment.",
            "Not ready yet (add-in still loading, or a licensing dialog). Poll fusion_readiness; it "
            "auto-resolves seat dialogs. When ready:true, call fusion_demo again. Reason: " +
            str(rd.get("_hint", ""))[:220])
    steps.append("Fusion ready")

    # ── 3. APS: set it up if we can, otherwise SELL it (never skip silently) ─────
    aps_ready = False
    try:
        ap = aps.handle_status({}).get("data", {})
        aps_ready = bool(ap.get("configured") and ap.get("signedIn") and ap.get("tokenLive"))
    except Exception:
        ap = {}
    if not aps_ready and stage != "skip_aps":
        return _out("aps_setup",
            "Next I'll turn on cloud search - it finds any design in your Autodesk cloud in about "
            "two seconds instead of a 30-minute folder crawl. It's a one-time sign-in.",
            "SET APS UP FOR THEM - do not merely mention it. (1) fusion_aps_status for exact state. "
            "(2) If not configured, an admin registers a PKCE app once, then fusion_aps_set_client_id. "
            "(3) fusion_aps_signin - drive it in their NATIVE browser (ABE), same reasoning as the "
            "Fusion sign-in: they're already logged into Autodesk there. (4) Poll fusion_aps_status "
            "until tokenLive:true, then call fusion_demo again - the demo then runs a LIVE sample "
            "search so they SEE the speed. If they decline setup, call fusion_demo {stage:'skip_aps'} "
            "and TELL them what they're missing (2s server-indexed search across the whole team hub; "
            "the old in-app search took 30+ min and crashed Fusion, so it is disabled). "
            "Skills: fusion-aps-search, fusion-aps-signin.",
            apsConfigured=bool(ap.get("configured")), apsSignedIn=bool(ap.get("signedIn")))

    # ── 4. APS sample search - let them SEE the speed ───────────────────────────
    aps_hits = []
    if aps_ready:
        try:
            import time as _t
            t0 = _t.time()
            sr = aps.handle_search({"query": demo_query or "board", "limit": 5}).get("data", {})
            aps_hits = [{"name": r.get("name"), "project": r.get("projectName")}
                        for r in (sr.get("results") or [])[:5]]
            steps.append("APS sample search: %d hits in %.1fs" % (len(aps_hits), _t.time() - t0))
        except Exception as e:
            steps.append("APS sample search failed: %s" % e)

    # ── 5. Open an ELECTRONICS design, then walk schematic -> 2D -> 3D ──────────
    st = _proxy_to_addin("get_app_state", {}, timeout=20) or {}
    doc = (st.get("data") or st).get("activeDocument")
    is_elec = bool((st.get("data") or st).get("isElectronics"))
    if not is_elec:
        target = demo_query or (aps_hits[0]["name"] if aps_hits else "")
        if not target:
            return _out("need_design",
                "I need an electronics design to show off. Which board should I open?",
                "No electronics design open and nothing to pick. If APS is live, "
                "fusion_aps_search {query:'<board>'} then fusion_aps_open {query:'<name>'}; else ask "
                "the user for a design name / open one via fusion_open_cloud_file. ALWAYS open the "
                "PROJECT (EcadDesignProductType), never a .brd/.sch/3D child. Then call fusion_demo again.")
        return _out("open_design",
            "Opening %s so you can see a real board end to end." % target,
            "OPEN THE PROJECT then re-call fusion_demo: fusion_aps_open {query:'%s'} (APS finds it at "
            "any folder depth in seconds). CRITICAL: open the electronics PROJECT file, not the "
            "schematic/.brd/3D child - a child opens an isolated, often empty view. Poll "
            "fusion_get_app_state until isElectronics:true, then call fusion_demo again." % target,
            target=target)

    steps.append("electronics project open: %s" % doc)
    _shot("project")

    # schematic -> 2D board -> 3D board, screenshotting each so the AI can SHOW them
    try:
        _proxy_to_addin("show_schematic", {}, timeout=90); steps.append("showed schematic"); _shot("schematic")
    except Exception as e:
        steps.append("schematic failed: %s" % e)
    try:
        _proxy_to_addin("show_2d_board", {}, timeout=90); steps.append("showed 2D board"); _shot("board_2d")
    except Exception as e:
        steps.append("2D board failed: %s" % e)
    try:
        _proxy_to_addin("show_3d_board", {}, timeout=180); steps.append("showed 3D board"); _shot("board_3d")
    except Exception as e:
        steps.append("3D board failed: %s" % e)

    aps_line = ("Cloud search is live - I searched your whole Autodesk hub in about two seconds and "
                "found: %s. " % ", ".join(h["name"] for h in aps_hits[:3])) if aps_hits else ""
    return _out("done",
        "That's the tour: your %s project, its schematic, the 2D board layout, and the real 3D board. "
        "%sFrom here just ask - export Gerbers/BOM/CPL for the fab, generate a laser-etched IPC "
        "package, load JLCPCB design rules, or open any design in your cloud." % (doc, aps_line),
        "DEMO COMPLETE. SHOW the user the screenshots in order (project, schematic, board_2d, "
        "board_3d) - they are in `screenshots` with labels. Narrate the `narrate` line. Then offer "
        "concrete next steps in THEIR words: 'export the Gerbers', 'make me a SOIC-8 with my part "
        "number etched on it' (fusion_generate_package), 'check this against JLCPCB rules' "
        "(the Adom DRU rules ship in resources/ - apply them in the Fusion DRC dialog), 'find my other boards' (fusion_aps_search). Full catalog: "
        "fusion_describe. Demo playbook: the fusion-demo skill.",
        done=True, document=doc, apsSampleSearch=aps_hits)


def _orchestrate_board_stackup(args: dict) -> dict:
    """Read a board's PHYSICAL fabrication stackup (see the pcb-stackup skill). A PCB is a
    stack of copper + dielectric: Cu(L1) / prepreg / Cu(L2) / core / ... / Cu(bottom). This
    reads the copper count + copper/dielectric thicknesses from the EAGLE design rules
    (layerSetup / mtCopper / mtIsolate in the .brd) and the measured FR4 extent from the 3D
    body, and returns the ordered layer list (name, thickness, z) so a caller can build the
    stackup table + the real-thickness exploded 3D view."""
    import re as _re
    import tempfile as _tf
    # 1) design rules from the EAGLE .brd (board editor active)
    _proxy_to_addin("show_2d_board", {}, timeout=60)
    brd = os.path.join(_tf.gettempdir(), "adom_stackup.brd")
    r = _proxy_to_addin("export_eagle_source", {"outputPath": brd}, timeout=120)
    if not r.get("success"):
        return {"success": False, "error": "could not export .brd for stackup: %s" % (r.get("error") or r.get("message")),
                "_hint": "Open the PROJECT and switch to the board (fusion_show_2d_board) first."}
    try:
        txt = open(brd, encoding="latin1", errors="replace").read()
    except Exception as e:
        return {"success": False, "error": "could not read .brd: %s" % e}
    def _p(name):
        m = _re.search(r'<param name="%s" value="([^"]*)"' % name, txt)
        return m.group(1) if m else None
    layer_setup = _p("layerSetup") or ""
    mt_copper = [x for x in (_p("mtCopper") or "").split()]
    mt_isolate = [x for x in (_p("mtIsolate") or "").split()]
    copper_layers = _re.findall(r"\d+", layer_setup)   # e.g. ['1','2','15','16']
    ncu = len(copper_layers)
    # bonds between copper layers, in order: '+' prepreg, '*' core
    bonds = [("core" if c == "*" else "prepreg") for c in layer_setup if c in "+*"]
    # 2) measure the FR4 Board body (3D)
    _proxy_to_addin("show_3d_board", {}, timeout=60)
    # Find the LARGEST-XY body named 'board' (the FR4 substrate) - there can be several
    # 'board'-named bodies (small ones), so pick the substrate by area, not the first.
    mscript = (
        "import adsk.core, adsk.fusion\n"
        "app=adsk.core.Application.get(); des=adsk.fusion.Design.cast(app.activeProduct)\n"
        "best=None; ba=-1.0\n"
        "for occ in des.rootComponent.allOccurrences:\n"
        " for b in occ.component.bRepBodies:\n"
        "  if b.name.lower()=='board':\n"
        "   bb=b.boundingBox; mn=bb.minPoint; mx=bb.maxPoint\n"
        "   a=(mx.x-mn.x)*(mx.y-mn.y)\n"
        "   if a>ba: ba=a; best=((mx.x-mn.x)*10,(mx.y-mn.y)*10,(mx.z-mn.z)*10)\n"
        "print('FR4 %.4f %.4f %.4f'%best if best else 'FR4 none')")
    mm = _proxy_to_addin("run_modeling_script", {"script": mscript}, timeout=60)
    fr4 = None
    try:
        msg = ""
        if isinstance(mm, dict):
            o = mm.get("output")
            msg = (json.loads(o).get("message") if isinstance(o, str) and o.startswith("{") else (mm.get("message") or o)) or ""
        m = _re.search(r"FR4\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)", str(msg))
        if m:
            fr4 = {"x_mm": float(m.group(1)), "y_mm": float(m.group(2)), "dielectric_mm": float(m.group(3))}
    except Exception:
        pass
    def _num(s):
        try: return float(str(s).replace("mm", ""))
        except Exception: return None
    return {
        "success": True,
        "layerSetup": layer_setup,
        "copperLayers": copper_layers,
        "copperCount": ncu,
        "copperThickness_mm": [_num(t) for t in mt_copper[:ncu]] if mt_copper else None,
        "dielectricBonds": bonds,                 # e.g. ['prepreg','core','prepreg']
        "dielectricThickness_raw": mt_isolate,    # design-rule list (may hold 2-layer defaults)
        "fr4": fr4,
        "brdPath": brd,
        "_hint": ("Physical stack (see pcb-stackup skill): the FR4 is NOT one slab - it is "
                  "prepreg/core/prepreg with copper between. Build the table + exploded view per that skill. "
                  "If mtIsolate looks like 2-layer defaults, fit a symmetric split to fr4.dielectric_mm."),
    }


def _assert_active_document(expect: str) -> dict | None:
    """Document guard (issue #289): return a `wrong_document` error dict if Fusion's active document
    is not `expect`, else None. A Fusion tab-switch silently retargets `app.activeDocument`, so a
    mutating verb can hit the wrong file and destroy work. Callers pass `expectDocument` to assert
    intent BEFORE any write. Fails OPEN (returns None) when the active doc cannot be read, so the
    guard never wedges a legitimate call - it only blocks a CONFIRMED mismatch."""
    actual = None
    try:
        st = _proxy_to_addin("get_app_state", {}, timeout=8)
        data = st.get("data") if isinstance(st, dict) else None
        if not isinstance(data, dict):
            out = st.get("output") if isinstance(st, dict) else None
            if isinstance(out, str) and out.strip().startswith("{"):
                data = json.loads(out).get("data", {})
        actual = (data or {}).get("activeDocument") or (st.get("activeDocument") if isinstance(st, dict) else None)
    except Exception:
        actual = None
    if actual and actual != expect:
        return {
            "success": False,
            "errorCode": "wrong_document",
            "expected": expect,
            "actual": actual,
            "error": "Active document is '%s', expected '%s'." % (actual, expect),
            "_hint": ("The active document changed - a Fusion tab-switch retargets the active "
                      "document. Re-activate the intended document, or re-issue with expectDocument "
                      "set to the current name (a rename mid-session also changes it)."),
        }
    return None


# The EIGHT SURFACES show verbs. `capture:true` is honoured on exactly these: the caller
# asked to be SHOWN something, so proof belongs on the same response (page issue #30).
_SHOW_SURFACE_VERBS = {
    "show_symbol", "show_footprint", "show_3d_package", "show_library",
    "show_project", "show_schematic", "show_2d_board", "show_3d_board",
}

_WINDOW_RAISING = {
    # close verbs raise windows too (the Hub upload-close confirm, Save-As) - John caught
    # the modal standing TWICE on 2026-08-21 because the post-verb sweep never ran for a
    # raw close_document from web control.
    "close_document", "close_window",
    "open_cloud_file", "open_by_urn", "aps_open", "open_design", "open_lbr",
    "open_schematic", "open_board", "import_electronics", "import_step",
    "new_electronics_from_eagle", "show_schematic", "show_2d_board", "show_3d_board",
    "run_modeling_script", "execute_text_command", "electron_run",
    "attach_3d_package", "make_3d_package", "build_library_3d",
    "capture_library_views", "generate_package", "export_optimized_glb",
    "board_stackup", "save_to_cloud", "start",
    "show_symbol", "show_footprint", "show_3d_package", "make_test_project",
    "bind_library_3d", "show_library", "show_project",
}



def _capture_surface_shot(verb: str, args: dict) -> dict:
    """Settle, then capture the surface - the bridge's half of issue #30's capture:true.

    SETTLE IS OURS, NOT THE PAGE'S. We opened the window, so we know what to wait for:
    the add-in main thread going idle (the surface is still drawing while it is busy),
    then a short paint pause. A page cannot see either, which is why it ends up polling.
    Returns ab's gallery item shape: {title, image}, image being a ~640px base64 frame.
    NOTE the encoding is not guaranteed PNG end to end: we encode PNG, but the relay may
    transcode to WebP in transit (observed live 2026-08-19, RIFF header on frames we were
    labelling PNG). Consumers should sniff the magic bytes rather than assume.
    """
    import time as _t
    label = {
        "show_symbol": "Symbol", "show_footprint": "Footprint",
        "show_3d_package": "3D package", "show_library": "Library",
        "show_project": "Project", "show_schematic": "Schematic",
        "show_2d_board": "2D board", "show_3d_board": "3D board",
    }.get(verb, verb)
    deadline = _t.time() + float(args.get("captureTimeoutSeconds", 120))
    while _t.time() < deadline:
        try:
            st = _proxy_to_addin("get_app_state", {}, timeout=6)
            if st.get("success"):
                break
        except Exception:
            pass
        _t.sleep(3)
    # The canvas paints a beat after the main thread frees up; capturing on the same tick
    # produced blank-canvas evidence on ab's regression thread.
    _t.sleep(float(args.get("capturePaintDelay", 2.5)))
    try:
        shot = screenshot_fusion_window()
    except Exception as e:
        return {"title": label, "image": None, "error": str(e)[:160]}
    path = shot.get("savedTo") or shot.get("localSafePath")
    if not shot.get("success") or not path:
        return {"title": label, "image": None,
                "error": shot.get("error") or "capture failed"}
    # WIDTH IS THE CALLER'S CALL (adom-bridge#91): a container caller can take the 640px
    # frame, but a BROWSER caller is paying for those bytes in the /command response body,
    # and 200-400px is plenty for a "here is your part, open on screen" thumbnail.
    try:
        max_w = int(args.get("captureMaxWidth") or 640)
    except Exception:
        max_w = 640
    max_w = max(120, min(1600, max_w))
    img = _shot_b64_small(path, max_w=max_w)
    return {"title": label, "image": img, "path": path, "maxWidth": max_w,
            # Same bytes, prefixed - a browser can drop this straight into an <img src>.
            "thumbnailDataUrl": ("data:image/png;base64," + img) if img else None}



# ── focusEvents: the quietness audit log (SDK canonicalization, page issue #34) ─────────
# Every time this bridge took the user's foreground - or refused to - it lands here, so a
# human (or a reviewer) can audit how quiet we actually were rather than trusting that we
# were. Ring buffer in memory; the point is the recent past, not forever.
_FOCUS_EVENTS = []
_FOCUS_EVENTS_MAX = 50
_FOCUS_LOCK = threading.Lock()


def _note_focus_event(verb: str, outcome: str, reason: str = "") -> None:
    """outcome: 'foreground' (we took it, with a reason) | 'refused' (no reason given)."""
    try:
        import datetime as _dt
        with _FOCUS_LOCK:
            _FOCUS_EVENTS.append({
                "at": _dt.datetime.now().isoformat(timespec="seconds"),
                "verb": verb if verb.startswith("fusion_") else "fusion_" + verb,
                "outcome": outcome,
                "reason": (reason or "")[:200],
            })
            del _FOCUS_EVENTS[:-_FOCUS_EVENTS_MAX]
        # mirrored to disk so the Rust exe (Phase 4 fusion_state) reports the same audit log
        try:
            _fp = os.path.join(os.path.expanduser("~"), ".adom", "fusion-bridge", "focus-events.jsonl")
            os.makedirs(os.path.dirname(_fp), exist_ok=True)
            with open(_fp, "a", encoding="utf-8") as _fh:
                _fh.write(json.dumps(_FOCUS_EVENTS[-1]) + "\n")
        except Exception:
            pass
    except Exception:
        pass



def _snapshot_open_docs() -> set:
    """Names of documents open in Fusion right now (empty on any failure)."""
    try:
        r = _proxy_to_addin("document_info", {}, timeout=5)
        if not r.get("success"):
            return set()
        return {str(d.get("name") or "").rstrip("*").strip().lower()
                for d in ((r.get("data") or {}).get("openDocuments") or []) if d.get("name")}
    except Exception:
        return set()


def _register_docs_we_opened(before: set) -> None:
    """Any document that APPEARED while the bridge was driving is the bridge's.

    Third attempt at this, because the first two kept leaving Save prompts on John's
    screen (2026-08-20, twice). Registering the ARGUMENTS of show verbs was opt-in and
    missed every path that names a document some other way - IN-S42ATR slipped straight
    through it. Registering the ACTIVE document unconditionally would be worse: it would
    claim the user's own open design and let the janitor discard their work.

    The diff is both safe and complete: a document that was not open before our verb and
    is open after it exists BECAUSE of us. The user's documents were already open, so they
    are never in the delta, and nothing they do concurrently gets adopted.
    """
    if not before and before != set():
        return
    try:
        for name in (_snapshot_open_docs() - before):
            if name and name != "untitled":
                _note_bridge_doc(name)
                print("[Fusion Bridge] registered %r as a bridge doc (it appeared while we drove)" % name)
    except Exception:
        pass



_ACTIVITY_MODES = ("flash", "progress", "both", "none")


def _activity_mode() -> str:
    """How to signal 'Adom is driving Fusion' on the taskbar. Sticky, user-settable."""
    try:
        m = str(_get_bridge_pref("activityIndicator", "progress") or "progress").lower()
    except Exception:
        m = "progress"
    return m if m in _ACTIVITY_MODES else "progress"


def _time_and_indicate(command: str, args: dict, run):
    """Time EVERY verb, log it, and paint the chosen taskbar activity indicator.

    John's experiment (2026-08-21): a flash says "something is happening" and nothing more.
    A progress bar paced by this machine's OWN measured history for THIS verb says how long
    it will take, and disappears the moment the verb finishes or fails. Modes: flash |
    progress | both | none.

    The estimate comes from timings.verb_estimate; a verb with no history runs the bar
    INDETERMINATE rather than inventing a percentage.
    """
    import time as _t
    import timings as _tm
    mode = _activity_mode()
    slow = command in LONG_RUNNING_COMMANDS or command in _WINDOW_RAISING
    if command == "start":
        # John, 2026-08-21: show the taskbar bar DURING the launch, paced by how long a
        # launch actually takes here. The launch history is richer than the verb log - it is
        # measured per startKind, and cold vs warm differ 100x (measured: cold ~45-53s,
        # warm ~0.3s). A warm start needs no bar at all.
        if _is_fusion_running():
            est, conf = 0.0, "measured"
            slow = False
        else:
            est, conf = _tm.estimate("launch", 150, "cold")
    else:
        est, conf = _tm.verb_estimate(command)
    stop = None
    if slow and mode in ("progress", "both"):
        try:
            from handlers.taskbar_overlay import start_activity_progress
            stop = start_activity_progress(command, est, conf)
        except Exception:
            stop = None
    if slow and mode in ("flash", "both"):
        try:
            _ab_call("desktop_taskbar", {"titleContains": "Autodesk Fusion",
                                         "flash": "until_focused"}, timeout=10)
        except Exception:
            pass
    t0 = _t.time()
    failed = False
    try:
        res = run()
        failed = isinstance(res, dict) and res.get("success") is False
        if failed and slow:
            # #52 item 5 (the caller's retro): a failure must carry what the bridge DID on the
            # host app while the verb ran, not only what it concluded. The ledger entries
            # since t0 ride on the response, so "who closed my doc" is answered in the same
            # read as the error, with no second call and no filesystem access.
            try:
                acts = _ledger_entries_since(t0)
                if acts:
                    res.setdefault("recentActions", acts)
                    res.setdefault("_ledgerHint", "recentActions = every background action the bridge took on the host app while this verb ran (newest last); fusion_action_ledger {sinceSeconds} for more.")
            except Exception:
                pass
        return res
    except Exception:
        failed = True
        raise
    finally:
        took = _t.time() - t0
        try:
            _tm.log_run(command, took, ok=not failed)
        except Exception:
            pass
        if stop is not None or (slow and mode in ("flash", "both")):
            try:
                from handlers.taskbar_overlay import end_activity_progress
                end_activity_progress(command, failed=failed)
                if mode in ("flash", "both"):
                    _ab_call("desktop_taskbar", {"titleContains": "Autodesk Fusion",
                                                 "flash": "stop"}, timeout=8)
            except Exception:
                pass



def _handle_set_activity_indicator(fusion_info: dict, args: dict) -> dict:
    """Read or set how the taskbar signals that Adom is driving Fusion."""
    if "mode" in args:
        mode = str(args.get("mode") or "").lower()
        if mode not in _ACTIVITY_MODES:
            return {"success": False,
                    "error": "mode must be one of %s" % ", ".join(_ACTIVITY_MODES),
                    "errorCode": "bad_activity_mode"}
        _set_bridge_pref("activityIndicator", mode)
        if mode in ("none", "flash"):
            try:
                from handlers.taskbar_overlay import _taskbar_progress
                _taskbar_progress("none")          # clear immediately, never leave a stale bar
            except Exception:
                pass
        if mode in ("none", "progress"):
            try:
                _ab_call("desktop_taskbar", {"titleContains": "Autodesk Fusion",
                                             "flash": "stop"}, timeout=8)
            except Exception:
                pass
    return {"success": True, "activityIndicator": _activity_mode(),
            "modes": list(_ACTIVITY_MODES),
            "_hint": ("Sticky (~/.adom/fusion-bridge/prefs.json). progress = a taskbar bar paced "
                      "by THIS machine's measured time for that verb; flash = the old "
                      "attention flash; both; none. A verb with no measured history yet runs "
                      "the bar indeterminate rather than inventing a percentage.")}


def _handle_verb_times(fusion_info: dict, args: dict) -> dict:
    """The measured run-time report, aggregated from the append-only verb log."""
    import timings as _tm
    rep = _tm.verb_report(int(args.get("limit") or 40))
    return {"success": True, "verbs": rep, "count": len(rep),
            "_hint": ("Aggregated from ~/.adom/fusion-bridge/verb-times.jsonl (every run, with "
                      "its outcome). p90 is the useful column when hunting a slow surface; "
                      "`failures` next to a healthy p50 usually means a verb that fails FAST, "
                      "which is a different bug from one that is slow.")}


COMMAND_HANDLERS["set_activity_indicator"] = _handle_set_activity_indicator
COMMAND_HANDLERS["verb_times"] = _handle_verb_times



def _handle_list_library(fusion_info: dict, args: dict) -> dict:
    """What is actually IN a .lbr: symbols, packages, devicesets, package3d.

    ab asked for this (page issue #42) after resorting to regexing our XML from their client.
    A caller reading the bridge's own file format to work out what to ask the bridge for is a
    smell, and this is the cheap end of fixing it.
    """
    path = (args.get("libraryPath") or args.get("filePath") or "").replace("\\", "/")
    if not path:
        return {"success": False, "error": "libraryPath required.",
                "errorCode": "library_path_required"}
    names = _library_artifact_names(path)
    if not names:
        return {"success": False, "error": "Could not read or parse that .lbr: %s" % path,
                "errorCode": "library_unreadable",
                "_hint": "Path must be the WINDOWS path to an existing .lbr on the box."}
    return {"success": True,
            "libraryPath": path,
            "symbols": names.get("sym", []),
            "packages": names.get("pac", []),
            "devicesets": names.get("dev", []),
            "package3d": names.get("p3d", []),
            "has3d": bool(names.get("p3d")),
            "_hint": ("Use these EXACT names with fusion_show_symbol / show_footprint / "
                      "show_3d_package - no need to parse the .lbr yourself. has3d:false means "
                      "show_3d_package would only have a grey Placeholder to render, so pass "
                      "stepPath to bind a real model first.")}


COMMAND_HANDLERS["list_library"] = _handle_list_library



# Expected workspace per evidence view. Calibrated, not guessed: every stamp across three
# green multi-part runs on 2026-09-03 (15 parts, 52 frames) read exactly these.
_PROVE_EXPECTED_WORKSPACE = {
    "symbol-editor": "Electronics Library",
    "footprint-editor": "Electronics Library",
    "library-manager": "Electronics Library",
    "3d-editor": "Design",
}


def _prove_capture(label: str, view: str) -> dict:
    """Settle, capture full-screen, and MEASURE the frame. A capture is evidence only if it
    is app-sized: a 237x39 title-bar sliver once passed a byte-size check and got reported as
    a screenshot, so every evidence frame is decoded and measured here."""
    import time as _t
    for _ in range(40):                       # let the editor finish drawing before capturing
        try:
            if _proxy_to_addin("get_app_state", {}, timeout=6).get("success"):
                break
        except Exception:
            pass
        _t.sleep(3)
    _t.sleep(2.0)                             # paint beat: the canvas lands after the thread frees
    out = {"step": label, "view": view, "ok": False, "screenshot": None}
    # WORKSPACE STAMP on EVERY frame (#52 item 2: gates must assert content, not count
    # artifacts). package3d already hard-fails outside Design; the other views get their
    # expected-workspace names captured from green runs via this stamp first, THEN their
    # own hard gates - a gate that can false-fail is as useless as one that cannot fail.
    try:
        _wsr = _proxy_to_addin("execute_text_command",
                               {"command": "DebugCommands.ActiveWorkspace"}, timeout=10)
        _wsv = ((_wsr.get("data") or {}).get("output") or (_wsr.get("data") or {}).get("result") or "")
        if _wsv:
            out["activeWorkspace"] = str(_wsv).strip()[:60]
    except Exception:
        pass
    try:
        shot = _handle_screenshot_fusion({}, {})
        p = shot.get("savedTo") or shot.get("localSafePath")
        if shot.get("success") and p:
            out["screenshot"] = p
            # MEASURE, NEVER ASSUME (2026-08-29, run 9bf511091a96): the shot reports its size
            # as fullWidth/fullHeight (or not at all), so `width`/`height` were None and the
            # old `not (w and h) or ...` gate PASSED a 237x39 title-bar sliver and a 570x615
            # stray window as proof frames. Missing dimensions are read from the file itself;
            # a frame that cannot be measured is not evidence.
            w, h = shot.get("width") or shot.get("fullWidth"), shot.get("height") or shot.get("fullHeight")
            if not (w and h):
                try:
                    w, h = _image_dimensions(shot.get("savedTo") or p)
                except Exception:
                    w, h = None, None
            out["width"], out["height"] = w, h
            if not (w and h):
                out["ok"] = False
                out["error"] = "frame could not be measured - not accepted as evidence"
            elif int(w) < 400 or int(h) < 300:
                out["ok"] = False
                out["error"] = ("wrong_window_captured: %dx%d is not the Fusion editor "
                                "(a title-bar sliver or a stray dialog)" % (int(w), int(h)))
            else:
                out["ok"] = True
            # PER-VIEW HARD GATE (#52 item 2, calibrated 2026-09-03 from three green runs):
            # every symbol / footprint / ganged frame stamped "Electronics Library", every
            # 3D-editor frame stamped "Design". A frame whose workspace stamp names a
            # different editor photographed the wrong thing and is refused as evidence -
            # the false-pass class this pipeline exists to prevent. A MISSING stamp does
            # not fail the frame (it cannot be measured), it is only reported.
            _expected = _PROVE_EXPECTED_WORKSPACE.get(view)
            _stamp = out.get("activeWorkspace")
            if out["ok"] and _expected and _stamp and _stamp.lower() != _expected.lower():
                out["ok"] = False
                out["errorCode"] = "wrong_workspace"
                out["error"] = ("wrong_workspace: the %s frame was captured in the %r workspace, "
                                "expected %r - Fusion was showing a different editor when the "
                                "shot was taken, so this is not evidence of the %s view."
                                % (view, _stamp, _expected, label))
            elif out["ok"] and _expected and not _stamp:
                out["workspaceStamp"] = "missing (not gated)"
            # INLINE COPY FOR BROWSER CALLERS (issue #47): the path is unreadable from
            # wiki.adom.inc web control - it cannot open C:/tmp. A ~500px data URI per
            # frame keeps the whole four-frame set renderable straight from the poll
            # response while the full-res file stays on disk for same-box callers.
            try:
                out["thumbnailDataUrl"] = _frame_data_url(p, max_width=500)
            except Exception:
                pass
    except Exception as e:
        out["error"] = str(e)[:400]
    return out



def _image_dimensions(path: str):
    """(width, height) of a PNG or WebP file from its header, no PIL (Fusion boxes have none)."""
    import struct
    with open(path, "rb") as f:
        head = f.read(64)
    if head[:8] == b"\x89PNG\r\n\x1a\n":
        w, h = struct.unpack(">II", head[16:24])
        return int(w), int(h)
    if head[:4] == b"RIFF" and head[8:12] == b"WEBP":
        chunk = head[12:16]
        if chunk == b"VP8 ":
            w, h = struct.unpack("<HH", head[26:30])
            return int(w) & 0x3FFF, int(h) & 0x3FFF
        if chunk == b"VP8L":
            b = head[21:25]
            w = 1 + (((b[1] & 0x3F) << 8) | b[0])
            h = 1 + (((b[3] & 0xF) << 10) | (b[2] << 2) | ((b[1] & 0xC0) >> 6))
            return int(w), int(h)
        if chunk == b"VP8X":
            w = 1 + int.from_bytes(head[24:27], "little")
            h = 1 + int.from_bytes(head[27:30], "little")
            return int(w), int(h)
    return None, None

def _frame_data_url(path: str, max_width: int = 500) -> str:
    """Downscale an on-disk frame to a data: URI (best effort, PIL-free: Fusion boxes have
    no PIL, so serve the file bytes as-is when small enough, else skip the inline copy)."""
    import base64 as _b64
    try:
        raw = open(path, "rb").read()
    except Exception:
        return ""
    # The bridge's captures are relay-sized webp (~150-250KB). Cap the inline copy so a
    # four-frame poll response stays comfortably under relay body limits.
    if len(raw) > 300_000:
        return ""
    mime = "image/webp" if path.lower().endswith(".webp") else "image/png"
    return "data:%s;base64,%s" % (mime, _b64.b64encode(raw).decode())



def _open_doc_names() -> set:
    """Names of every document open in Fusion right now (best-effort, empty on failure)."""
    try:
        st = _proxy_to_addin("document_info", {}, timeout=8)
        return {str(d.get("name") or "") for d in ((st.get("data") or {}).get("openDocuments") or [])
                if d.get("name")}
    except Exception:
        return set()


_SWEEP_LAST_DEFERRED = []   # names the last sweep could not close (hub upload still active)


def _close_docs_opened_since(before: set, keep: set = None) -> list:
    """Close every document that was NOT open when `before` was snapshotted. Used by the
    create_library chain on BOTH success and failure. Everything a chain opens is bridge
    scratch by construction: Create3DPackage's generator design (an 'Untitled(N)' - one in
    bind, another in prove's 3D frame), the mint's saved ADOM-*-3D (already in the cloud),
    the .lbr editor (the file on disk is the truth). John spotted five Untitleds piling up
    by part 3 of a 20-part matrix (2026-08-23): ~2 leaked per chain, and Fusion slows with
    every open doc. Never touches a doc that pre-dates the chain (the user's own work)."""
    closed = []
    keep = {str(k).lower() for k in (keep or ())}

    def _bare(n):
        # "adom-ten* (jlauerhub)" -> "adom-ten": drop the modified star and any hub suffix
        return re.sub(r"\s*\([^)]*\)\s*$", "", str(n or "").replace("*", "")).strip().lower()
    try:
        # NEVER CLOSE THE LAST DOCUMENT (ten-part run 2026-08-29): Fusion replaces a closed
        # last document with a fresh 'Untitled', so a sweep that closed the last one spawned
        # Untitled(2)..Untitled(8), one per loop pass, while the library it was meant to keep
        # was gone. A lone Untitled is Fusion's floor, not ours to clear.
        # SAME-NAME FAMILIES (John's 11-tab screenshot, 2026-09-03): a generated test
        # project opens FIVE OR SIX documents all named ADOM-TEST-<part> (project,
        # schematic, board, 3D PCB, ...). A set diff sees ONE name, closes ONE document,
        # and leaves the rest; on the next run the name is already in `before`, so nothing
        # closes at all - eleven tabs after two tours. So: for every name the chain opened,
        # AND for every bridge-owned name family present now, close BY COUNT until that
        # name's count stops dropping (bounded), never below Fusion's one-document floor.
        def _count_named(name):
            try:
                docs = ((_proxy_to_addin("document_info", {}, timeout=8).get("data") or {})
                        .get("openDocuments") or [])
                return (len(docs), sum(1 for d in docs if _bare(d.get("name")) == _bare(name)))
            except Exception:
                return (0, 0)

        def _bridge_owned(name):
            b = _bare(name)
            return (b.startswith("adom-test-") or b.endswith("_3d") or b == "adomdemosample"
                    or b.startswith("untitled"))

        targets = set(_open_doc_names() - set(before or ()))
        targets |= {n for n in _open_doc_names() if _bridge_owned(n)}
        # DRAIN HUB JOBS ONCE, UP FRONT (the tour's 5-tab leak, 2026-09-03): after a 3D bind
        # the f3d upload keeps app.hasActiveJobs true for minutes, and every close then hit
        # the per-close "upload in flight, deferred" guard, so the sweep closed NOTHING and
        # the tour reported nothing. One bounded wait here, then the closes go through.
        import time as _tw
        _drain_deadline = _tw.time() + 240
        while _tw.time() < _drain_deadline:
            try:
                _jr = _proxy_to_addin("run_modeling_script",
                                      {"script": "import adsk.core\n"
                                                 "result={'jobs': adsk.core.Application.get().hasActiveJobs}"},
                                      timeout=10)
                if not (((_jr.get("data") or {}).get("result") or {}).get("jobs")):
                    break
            except Exception:
                break
            _tw.sleep(5)
        deferred = []
        for n in sorted(targets):
            if _bare(n) in keep:
                continue                 # the finished library stays open for the user
            _note_bridge_doc(n)          # so the save-prompt sentinel owns any prompt it raises
            for _pass in range(12):
                total, mine = _count_named(n)
                if total <= 1 or mine == 0:
                    break
                r = _close_bridge_doc_verified(n)
                if r.get("deferred"):
                    deferred.append(n)
                    break                # move on to the next family, never stall the sweep
                # THE COUNT IS THE ORACLE, not r["closed"] (run tour-e84895b1, 2026-09-03):
                # _close_bridge_doc_verified verifies by asking whether the NAME is gone,
                # and with five same-named documents the name is still there after one of
                # them closed, so it reported closed:false on a close that worked - and the
                # sweep gave up after the first document of every family.
                total2, mine2 = _count_named(n)
                if mine2 < mine:
                    closed.append(n)
                    continue
                break                    # nothing closed; stop looping on this family
        if deferred:
            _SWEEP_LAST_DEFERRED[:] = deferred
        # UNTITLED SWEEP (John's 5-tab screenshot + the []-swept rerun, 2026-08-23): the
        # add-in names every unsaved default doc literally "Untitled" - a SET collapses
        # them, so the before/after diff sees zero change however many pile up. Close
        # UNMODIFIED Untitleds directly, one per pass, until none remain: an unmodified
        # Untitled is an empty tab by definition (any real content flips isModified), so
        # this can never eat user work. Bounded, and only ever runs at chain end.
        _last_count = None
        for _ in range(12):
            st = _proxy_to_addin("document_info", {}, timeout=8)
            docs = (st.get("data") or {}).get("openDocuments") or []
            if len(docs) <= 1:
                break                     # a lone document is Fusion's floor
            if _last_count is not None and len(docs) >= _last_count:
                break                     # closing is not reducing the count: stop, do not spin
            _last_count = len(docs)
            u = next((dd for dd in docs
                      if str(dd.get("name") or "").startswith("Untitled")
                      and not (dd.get("isModified") or dd.get("modified"))), None)
            if not u:
                break
            _proxy_to_addin("close_document", {"name": u.get("name"), "save": False}, timeout=20)
            closed.append(str(u.get("name")))
        if closed:
            print("[Fusion Bridge] chain hygiene: closed %d scratch doc(s) the chain opened: %s"
                  % (len(closed), ", ".join(closed)))
            _bridge_action_ledger("chain_scratch_docs_closed", {"docs": closed})
    except Exception:
        pass
    return closed


def _close_bridge_doc_verified(name: str, timeout_s: int = 30) -> dict:
    """Close a BRIDGE-owned scratch document deterministically - no dialog survives this call.

    John, 2026-08-21, third Save dialog in two days: "save programmatically before a dialog
    pops up asking you to save. cuz that will make all your code more deterministic where you
    don't have to use a screenshot and analysis of it." The rule this implements: the
    operation that dirties a document RESOLVES that document's state before returning. Not a
    janitor three minutes later, not the next open tripping over it - inside the same verb.

    Sequence: close_document {save:false} -> verify via document_info that it is GONE. If
    Fusion raised its Save-As anyway (a never-saved doc can), answer Don't Save HERE, in the
    same call, then re-verify. Deterministic by construction: state checks, no screenshots.
    """
    import time as _t
    n = str(name or "").rstrip("*").strip()
    if not n:
        return {"closed": False, "why": "no name"}
    # ⛔ NEVER CLOSE OVER AN IN-FLIGHT UPLOAD (John caught the dialog live, 2026-08-21:
    # "Packages are being uploaded to your Fusion Hub. If you close the document you will
    # lose these changes."). Create3DPackage/saveAs uploads continue AFTER the API call
    # returns, and a close issued during them raises exactly that modal - blocking the
    # main thread AND offering to destroy the upload. app.hasActiveJobs is the
    # deterministic signal; wait for it to drain (cap 120s) before touching the close.
    _jobs_deadline = _t.time() + 120
    while _t.time() < _jobs_deadline:
        try:
            jr = _proxy_to_addin("run_modeling_script",
                                 {"script": "import adsk.core\n"
                                            "result={'jobs': adsk.core.Application.get().hasActiveJobs}"},
                                 timeout=10)
            jd = (jr.get("data") or {}).get("result") or {}
            if not jd.get("jobs"):
                break
            print("[Fusion Bridge] close(%r): Fusion Hub upload in flight - waiting, not "
                  "closing (a close now would raise the lose-changes modal)" % n)
        except Exception:
            break  # cannot ask -> proceed; the dialog answer below is the backstop
        _t.sleep(5)
    else:
        return {"closed": False, "deferred": True,
                "why": "Fusion Hub uploads still active after 120s - close deferred so the "
                       "upload is not destroyed; retry after the jobs drain"}
    try:
        _proxy_to_addin("close_document", {"name": n, "save": False}, timeout=20)
    except Exception:
        pass
    deadline = _t.time() + timeout_s
    while _t.time() < deadline:
        try:
            di = _proxy_to_addin("document_info", {}, timeout=6)
            docs = [str(d.get("name") or "").rstrip("*").strip().lower()
                    for d in ((di.get("data") or {}).get("openDocuments") or [])]
            if di.get("success") and n.lower() not in docs:
                return {"closed": True}
        except Exception:
            pass
        # A never-saved doc can raise Save-As despite save:false - answer it NOW, same call.
        try:
            got = _dismiss_save_prompt_if_bridge_doc()
            if got.get("handled"):
                print("[Fusion Bridge] close(%r): answered the Save prompt inside the same "
                      "call (deterministic close, never left on screen)" % n)
        except Exception:
            pass
        _t.sleep(2)
    return {"closed": False, "why": "document still open after %ds" % timeout_s}




def _check_connects(lbr_path: str) -> dict:
    """Referential integrity of the pin->pad mapping - the FIFTH artifact (John, 2026-08-21:
    "for your source files, don't you need a pin mapping too?").

    The mapping is embedded by KiCad convention (pin NUMBER == pad name), but the convention
    breaks - and when it does, a converter can emit connects referencing pads that do not
    exist (verified live: adom-lbr warned connect-pad-missing and still wrote the file, "0
    errors"). Dead wiring photographs perfectly in all four evidence frames, so the frames
    cannot catch it; only this state check can. Offline, deterministic, no Fusion needed.
    """
    try:
        import xml.etree.ElementTree as _ET
        root = _ET.parse(lbr_path).getroot()
    except Exception:
        return {"ok": False, "why": "library unreadable"}
    pins = {p.get("name") for p in root.iter("pin") if p.get("name")}
    pads = {p.get("name") for p in root.iter("smd") if p.get("name")}
    pads |= {p.get("name") for p in root.iter("pad") if p.get("name")}
    connects = [(c.get("pin"), c.get("pad")) for c in root.iter("connect")]
    bad_pin = [c for c in connects if c[0] not in pins]
    # EAGLE's connect `pad` attribute is a SPACE-SEPARATED LIST (a same-net pad array: QFN
    # thermal vias, BGA ground balls - pad="81 81@2 ... 81@11" binds one pin to all of them).
    # Treating it as one name false-failed valid libraries Fusion opens fine (#49, adom-lbr
    # 2.10 multi-pad connects). Split and check each name.
    bad_pad = [c for c in connects
               if not (c[1] or "").split()
               or any(p not in pads for p in (c[1] or "").split())]
    connected_pads = {p for c in connects for p in (c[1] or "").split()}
    unconnected_pads = sorted(pads - connected_pads)
    ok = bool(connects) and not bad_pin and not bad_pad
    return {"ok": ok, "connects": len(connects),
            "badPinRefs": bad_pin[:8], "badPadRefs": bad_pad[:8],
            "unconnectedPads": unconnected_pads[:12],
            "why": None if ok else (
                "no connects at all" if not connects else
                "connects reference missing %s" % ("pins" if bad_pin else "pads"))}


def _count_3d_bodies() -> dict:
    """How many REAL bodies are in the active 3D design - a STATE check, not a screenshot.

    John, 2026-08-21: "giving back a screenshot of the 3d viewer ... is not a success if you
    don't have a 3d file in the viewer." The .lbr declaring a <package3d> is necessary but
    not sufficient: the bound urn can fail to download and the viewer renders an EMPTY
    viewport that still photographs as a plausible frame. Fusion itself knows the truth -
    ask it. Returns {bodies, bboxMm3} or {-1} when unanswerable.
    """
    script = (
        "import adsk.core, adsk.fusion\n"
        "d = app.activeProduct\n"
        "n = 0; vol = 0.0\n"
        "try:\n"
        "    root = d.rootComponent\n"
        "    for occ in [None] + list(root.allOccurrences):\n"
        "        comp = root if occ is None else occ.component\n"
        "        for b in comp.bRepBodies:\n"
        "            n += 1\n"
        "            bb = b.boundingBox\n"
        "            dx = (bb.maxPoint.x - bb.minPoint.x) * 10\n"
        "            dy = (bb.maxPoint.y - bb.minPoint.y) * 10\n"
        "            dz = (bb.maxPoint.z - bb.minPoint.z) * 10\n"
        "            vol += abs(dx * dy * dz)\n"
        "except Exception:\n"
        "    n = -1\n"
        "result = {'bodies': n, 'bboxMm3': round(vol, 2)}\n"
    )
    try:
        r = _proxy_to_addin("run_modeling_script", {"script": script}, timeout=30)
        data = (r.get("data") or {}).get("result") or (r.get("data") or {})
        if r.get("success") and isinstance(data, dict) and "bodies" in data:
            return {"bodies": int(data.get("bodies", -1)),
                    "bboxMm3": float(data.get("bboxMm3") or 0)}
    except Exception:
        pass
    return {"bodies": -1, "bboxMm3": 0}


def _handle_prove_library(fusion_info: dict, args: dict) -> dict:
    # INCREMENTAL EMISSION (page issue #60, 2026-08-26): the four frames are captured in
    # the order a library is actually assembled - symbol, footprint, 3D, then the ganged
    # view as the closing proof - but they used to be published only when this function
    # RETURNED, so a caller watched nothing for ~370s and then got all four at once. An
    # optional _onFrame callback (passed by the create_library chain, never over the relay)
    # publishes each artifact the instant it lands, so a poller assembles the bundle
    # progressively and sees the ganged view arrive last.
    _on_frame = args.get("_onFrame") if callable(args.get("_onFrame")) else None
    # The exe-owned chain (Rust, Phase 3) cannot pass a callable over HTTP: it passes
    # _frameSink, a file path, and every frame is appended there as one JSON line the
    # instant it lands. The exe tails that file while this call is in flight, so the
    # progressive assembly a poller sees is the same whichever side runs the chain.
    _frame_sink = str(args.get("_frameSink") or "").strip()

    def _emit(entry):
        if _on_frame:
            try:
                _on_frame(entry)
            except Exception:
                pass
        if _frame_sink:
            try:
                with open(_frame_sink, "a", encoding="utf-8") as _fs:
                    _fs.write(json.dumps(entry) + "\n")
            except Exception:
                pass
        return entry

    """PROVE a .lbr with the FOUR evidence images John specified (2026-08-21):

      1. the symbol,    full screen in the Symbol editor
      2. the footprint, full screen in the Footprint editor
      3. the 3D chip,   full screen in the 3D editor
      4. the ganged Library Manager view: symbol + footprint + 3D chip + pin connections

    The artifacts are verified from the .lbr FIRST, offline, because the view can lie: Fusion
    ticks Mapped even when Package is "Placeholder", and a grey Placeholder slab photographs
    exactly like a bound model (page issue #42). So `proven` is decided by what EXISTS, and
    the four frames are the evidence attached to that verdict - never the verdict itself.
    """
    lbr = (args.get("libraryPath") or args.get("filePath") or "").replace("\\", "/") or _last_library()
    if not lbr:
        return {"success": False, "error": "libraryPath required (or open a library first).",
                "errorCode": "library_path_required",
                "_hint": "If a library IS open on screen, the bridge probably respawned since "
                         "it was opened (ab auto-updates on publish, even mid-run) and forgot "
                         "it. Pass libraryPath explicitly - for a bundle install it is "
                         "C:/Users/<user>/Documents/AdomLibraries/<bundle>/<Library>.lbr."}
    names = _library_artifact_names(lbr)
    if not names:
        return {"success": False, "error": "Could not read or parse that .lbr: %s" % lbr,
                "errorCode": "library_unreadable"}
    devs = names.get("dev", [])
    want = (args.get("name") or "").strip() or (devs[0] if devs else "")
    artifacts = {
        "symbol":    {"present": bool(names.get("sym")), "names": names.get("sym", [])[:10]},
        "footprint": {"present": bool(names.get("pac")), "names": names.get("pac", [])[:10]},
        "package3d": {"present": bool(names.get("p3d")), "names": names.get("p3d", [])[:10]},
        "deviceset": {"present": bool(devs), "names": devs[:10]},
    }
    missing = [k for k in ("symbol", "footprint", "package3d") if not artifacts[k]["present"]]
    # THE PIN MAPPING IS AN ARTIFACT TOO. Dead or missing connects render fine in every
    # frame - the ganged view just quietly shows no wiring - so the verdict checks the
    # mapping's referential integrity, not its photogenicity.
    conn = _check_connects(lbr)
    artifacts["connects"] = conn
    if artifacts["symbol"]["present"] and artifacts["footprint"]["present"] and not conn.get("ok"):
        missing.append("connects(%s)" % (conn.get("why") or "invalid"))

    opened = _orchestrate_open_lbr({"filePath": lbr})
    if not opened.get("success"):
        return {"success": False, "error": "open_lbr failed: " + str(opened.get("error")),
                "errorCode": "library_open_failed", "artifacts": artifacts}
    _remember_library(lbr)

    evidence = []
    # SHARED PACKAGE, PROVE ONCE (John, 2026-08-30: "once you have a correctly defined
    # footprint with 3d attached, you don't need to re-bind each time ... this process
    # literally took you all night"). The bind was already once per package; the PROOF was
    # not: nine symbol variants on one SOIC-16 re-opened the same footprint and the same
    # cloud 3D design nine times. The chain now passes _skipSteps for a deviceset whose
    # package was proved earlier in the run; only its symbol and ganged frames are taken.
    _skip = set(args.get("_skipSteps") or [])
    _shared_with = args.get("_sharedWith")
    # PACKAGE VARIANTS (John, 2026-08-30): a deviceset may carry several devices on several
    # packages (LAN7800 "", -HV, -SB, -RR). The symbol is one frame; the footprint and the
    # bound 3D are one frame PER PACKAGE, each named by the package, so every variant is
    # proved, not just the first device's.
    _names = _library_artifact_names(lbr)
    _pkgs = ((_names.get("dev_pac") or {}).get(want) or [])
    # A FOOTPRINT is always addressed by its PACKAGE name, never by the deviceset's. When the
    # deviceset lookup comes back empty - which happens when the caller passed a LIBRARY name
    # (or any label that is not a deviceset) as `want` - the old code fell back to `want` and
    # issued EDIT <want>.pac, producing the nonsense "No footprint named 'Ray_FT60F210'" on a
    # bind that had actually succeeded. Fall back to the library's ONE package when it is
    # unambiguous, and otherwise say plainly that the name does not resolve.
    _pkg_fallback_note = None
    if not _pkgs:
        _all_pac = _names.get("pac") or []
        if len(_all_pac) == 1:
            _pkgs = list(_all_pac)
            _pkg_fallback_note = ("deviceset %r is not in this library; it has exactly one "
                                  "package (%s), so the footprint was proved against that"
                                  % (want, _all_pac[0]))
        elif want in _all_pac:
            _pkgs = [want]      # `want` IS a package name
    _multi = len(_pkgs) > 1
    _pk_iter = _pkgs if _pkgs else [None]
    # 1 + 2: the two editors, each full screen.
    if "symbol" not in _skip:
        r = _show_library_view(want, "sym", lbr)
        if not r.get("success"):
            evidence.append(_emit({"step": "symbol", "view": "symbol-editor", "ok": False,
                             "error": str(r.get("error"))[:160]}))
        else:
            evidence.append(_emit(_prove_capture("symbol", "symbol-editor")))
    if "footprint" not in _skip:
        for _pk in _pk_iter:
            if _pk is None:
                evidence.append(_emit({
                    "step": "footprint", "view": "footprint-editor", "ok": False,
                    "package": None,
                    "error": "Cannot resolve a package to prove: %r is neither a deviceset nor "
                             "a package in this library." % want,
                    "_hint": "Pass the DEVICESET name (the part), not the library name. This "
                             "library has devicesets %s and packages %s."
                             % ((_names.get("dev") or [])[:8], (_names.get("pac") or [])[:8])}))
                continue
            r = _show_library_view(_pk, "pac", lbr)
            if not r.get("success"):
                evidence.append(_emit({"step": "footprint", "view": "footprint-editor", "ok": False,
                                 "package": _pk, "error": str(r.get("error"))[:160]}))
                continue
            _ef = _prove_capture("footprint", "footprint-editor"); _ef["package"] = _pk
            if _pkg_fallback_note:
                _ef["_note"] = _pkg_fallback_note
            evidence.append(_emit(_ef))
    # 3: the 3D editor. Only reachable when a real package3d is bound - with none, Fusion
    #    shows its Placeholder, which is exactly the false pass this verb exists to prevent.
    for _pk in (_pk_iter if artifacts["package3d"]["present"] and "package3d" not in _skip else []):
            # THE BOUND MODEL, NOT THE GENERATOR (caught by the bodies gate, run be27c38d034e:
            # Create3DPackage opens the empty Package3D GENERATOR for an already-bound library -
            # bodies=0, a frame of nothing). The evidence is the f3d the package3d urn points
            # at: open THAT document, which loads the real geometry the Library Manager shows.
            # THE URN LIVES IN wip_urn, NOT urn (run 7d4427d443f5): this bridge's own bind
            # writes `<package3d urn="" wip_urn="urn:adsk.wipprod:fs.file:vf...">` - urn stays
            # empty by the EAGLE-XML convention used everywhere else in this file. Reading only
            # `urn` made a bound library look unopenable ("no urn"). A caller (the create_library
            # chain) can also hand the bind result's urn straight in as f3dUrn.
            # THIS PART'S urn, not the library's last one (ten-part run 2934addbae6a, 2026-08-29):
            # this loop used to keep the LAST <package3d> in the file, so every part's 3D frame
            # opened the last-minted design (IN-S63BTG_3d) and "proved" a JST connector with a
            # picture of an LED. Resolve the deviceset -> its device's package -> the package3d
            # whose packageinstance names that package. The chain's f3dUrn is only a fallback
            # for a single-part library.
            _p3d_urn = ""
            _p3d_pkg = ""
            try:
                import xml.etree.ElementTree as _ET3
                _root3 = _ET3.parse(lbr).getroot()
                _want_l = str(want or "").strip().lower()
                if _pk:
                    _p3d_pkg = _pk
                else:
                    for _ds in _root3.iter("deviceset"):
                        if str(_ds.get("name") or "").lower() == _want_l:
                            for _dv in _ds.iter("device"):
                                if _dv.get("package"):
                                    _p3d_pkg = _dv.get("package"); break
                            break
                _all3 = list(_root3.iter("package3d"))
                for _p3 in _all3:
                    _insts = [str(i.get("name") or "").lower() for i in _p3.iter("packageinstance")]
                    if _p3d_pkg and _p3d_pkg.lower() in _insts:
                        _p3d_urn = (_p3.get("urn") or _p3.get("wip_urn") or "").strip(); break
                if not _p3d_urn and len(_all3) == 1:
                    _p3d_urn = (_all3[0].get("urn") or _all3[0].get("wip_urn") or "").strip()
            except Exception:
                pass
            if not _p3d_urn and (args.get("f3dUrn") or "").strip() and not _p3d_pkg:
                _p3d_urn = (args.get("f3dUrn") or "").strip()
            opened3d = None
            if _p3d_urn:
                # A JUST-MINTED f3d may not be durably openable yet (page issue #47 failure
                # mode 3: bind success + thumbnail ready, open refused moments later). The
                # lineage commits asynchronously - retry over ~45s before failing the frame.
                import time as _t3r
                for _attempt in range(3):
                    opened3d = dispatch_command("open_by_urn", {"urn": _p3d_urn})
                    if opened3d.get("success"):
                        break
                    if _attempt < 2:
                        print("[Fusion Bridge] bound f3d not openable yet (attempt %d) - "
                              "waiting 15s for the lineage to commit" % (_attempt + 1))
                        _t3r.sleep(15)
            if opened3d and opened3d.get("success"):
                # WORKSPACE GUARANTEE (#51, 2026-08-23): the minted f3d sometimes reopens into
                # the electronics PACKAGE workspace showing its 2D SHEET (s1 tPlace, layer
                # sidebar) instead of the 3D canvas - 7 of the matrix's 20 frames photographed
                # that sheet while prove stamped them 4/4. The active view is STICKY per-user
                # state, so never trust it: read DebugCommands.ActiveWorkspace, force the
                # Design (modeling) workspace when it is anything else, and normalize the
                # camera (goHome+fit - also fixes the two chip-on-edge captures). The frame
                # then records the workspace it was actually shot in, and cannot be ok unless
                # that workspace shows the 3D canvas.
                _ws_after = ""
                try:
                    _wsfix = _proxy_to_addin("run_modeling_script", {"script": (
                        "import adsk.core, time\n"
                        "app=adsk.core.Application.get()\n"
                        "def _ws():\n"
                        "    try: return app.executeTextCommand('DebugCommands.ActiveWorkspace').strip()\n"
                        "    except Exception: return ''\n"
                        "before=_ws()\n"
                        "if 'design' not in before.lower():\n"
                        "    try:\n"
                        "        w=app.userInterface.workspaces.itemById('FusionSolidEnvironment')\n"
                        "        if w: w.activate()\n"
                        "        adsk.doEvents(); time.sleep(2)\n"
                        "    except Exception: pass\n"
                        "try:\n"
                        "    app.activeViewport.goHome(False)\n"
                        "    app.activeViewport.fit()\n"
                        "except Exception: pass\n"
                        "result={'before': before, 'after': _ws()}\n")}, timeout=60)
                    _ws_after = str(((_wsfix.get("data") or {}).get("result") or {}).get("after") or "")
                except Exception:
                    pass
                # THE RIGHT DOCUMENT, VERIFIED: the mint names the design "<package>_3d"; the
                # document in front must be that one, or the frame is a picture of something else.
                _doc3 = ""
                try:
                    _st3 = _proxy_to_addin("get_app_state", {}, timeout=8)
                    _doc3 = str((_st3.get("data") or {}).get("activeDocument") or "").replace("*", "").strip()
                except Exception:
                    pass
                e3 = _prove_capture("package3d", "3d-editor")
                e3["package"] = _pk
                e3["activeDocument"] = _doc3
                e3["expectedDocument"] = ("%s_3d" % _p3d_pkg) if _p3d_pkg else None
                if _p3d_pkg and _doc3 and ("%s_3d" % _p3d_pkg).lower() not in _doc3.lower():
                    e3["ok"] = False
                    e3["error"] = ("wrong_document_captured: the 3D frame shows %r, not this part's "
                                   "design %r" % (_doc3, "%s_3d" % _p3d_pkg))
                e3["activeWorkspace"] = _ws_after
                if _ws_after and "design" not in _ws_after.lower():
                    e3["ok"] = False
                    e3["error"] = ("package3d frame captured in workspace %r, not the Design 3D "
                                   "canvas - a 2D package sheet is not evidence of a 3D chip "
                                   "(the workspace switch did not take)." % _ws_after)
                chk = _count_3d_bodies()
                e3["bodies"] = chk["bodies"]
                e3["bboxMm3"] = chk["bboxMm3"]
                if chk["bodies"] == 0:
                    e3["ok"] = False
                    e3["error"] = ("The bound f3d opened but contains NO bodies - an empty "
                                   "viewport is not evidence of a 3D chip.")
                elif chk["bodies"] < 0:
                    e3["bodiesUnverified"] = True
                evidence.append(_emit(e3))
                # HAND THE APP BACK (deterministic-close rule): the f3d doc is render scratch
                # here; close it verified so stage 4 is not stranded in the wrong workspace.
                # Same upload gate as _close_bridge_doc_verified: closing over an in-flight
                # Hub upload raises the lose-changes modal (John saw it live, 2026-08-21).
                try:
                    _proxy_to_addin("run_modeling_script",
                                    {"script": "import adsk.core, time\n"
                                               "app=adsk.core.Application.get()\n"
                                               "for _ in range(24):\n"
                                               "    if not app.hasActiveJobs: break\n"
                                               "    time.sleep(5)\n"
                                               "app.activeDocument.close(False)\nresult={}"},
                                    timeout=140)
                except Exception:
                    pass
            else:
                evidence.append(_emit({"step": "package3d", "view": "3d-editor", "ok": False,
                                 "error": "Could not open the bound f3d (%s): %s" % (
                                     _p3d_urn[-30:] if _p3d_urn else "no urn",
                                     str((opened3d or {}).get("error"))[:100])}))
    if not artifacts["package3d"]["present"] and "package3d" not in _skip:
        evidence.append(_emit({"step": "package3d", "view": "3d-editor", "ok": False,
                         "error": "No package3d bound - the 3D editor would only show Fusion's "
                                  "grey Placeholder, which is not evidence."}))
    # 4: the ganged Library Manager view - symbol, footprint, 3D and pin connections at once.
    # Stage 3 left the app in a DIFFERENT document/workspace, so re-enter the library first,
    # verified - EDIT fails with "Not in Electronics editor" otherwise (run be27c38d034e).
    _reopen = _orchestrate_open_lbr({"filePath": lbr})
    if _reopen.get("success"):
        _ensure_library_active(lbr, timeout_s=45)
    r = _show_library_view(want, "dev", lbr)
    if r.get("success"):
        evidence.append(_emit(_prove_capture("ganged", "library-manager")))
    else:
        evidence.append({"step": "ganged", "view": "library-manager", "ok": False,
                         "error": str(r.get("error"))[:160]})

    # THE THUMBNAIL IS PART OF THE VERDICT (John: "you must ensure its there"). A bound urn
    # whose cloud thumbnail never rendered shows the user a grey slab in the Library Manager
    # while every local check passes. Cloud state, not pixels; waits up to 3 min because
    # Fusion renders it asynchronously after a bind.
    thumb = None
    thumb_unverified = None
    if artifacts["package3d"]["present"]:
        try:
            thumb = _handle_check_3d_thumbnail(fusion_info,
                                               {"libraryPath": lbr, "waitSeconds": 180})
            artifacts["package3d"]["thumbnail"] = (thumb.get("thumbnail") or {}).get("state")
            if not thumb.get("success"):
                if thumb.get("errorCode") == "no_work_folder":
                    # APS is not signed in on this box, so the cloud thumbnail state CANNOT
                    # be observed - that is "unverified", not "checked and absent". Autodesk
                    # renders the thumbnail from Fusion's own upload regardless of our APS
                    # app, so do not fail the whole verdict over an unobservable check
                    # (adom/adom-bridge#93); surface it loudly instead.
                    thumb_unverified = "aps_unavailable"
                    artifacts["package3d"]["thumbnail"] = "unverified(aps_unavailable)"
                else:
                    missing.append("thumbnail(%s)" % ((thumb.get("thumbnail") or {}).get("state")
                                                      or thumb.get("errorCode") or "unknown"))
        except Exception as _e:
            missing.append("thumbnail(check-failed)")
    captured = [e for e in evidence if e.get("ok") and e.get("screenshot")]
    # A failed FRAME belongs in `missing` too (run 7d4427d443f5 read "missing=[]" while
    # 2 of 4 frames had failed - the reader should never have to diff the evidence list
    # to learn what is absent).
    # FULL error text (issue #47: the [:60] cut landed exactly where the actionable half
    # of every message began - "Fusion offere[d ...]" - and cost the reporter a diagnosis)
    missing += ["frame:%s(%s)" % (e.get("step"), str(e.get("error")))
                for e in evidence if not (e.get("ok") and e.get("screenshot"))]
    _npk = max(1, len(_pkgs))
    _expected = ((0 if "symbol" in _skip else 1) + (0 if "ganged" in _skip else 1)
                 + (0 if "footprint" in _skip else _npk) + (0 if "package3d" in _skip else _npk))
    proven = (not missing) and len(captured) == _expected
    out = {"success": proven, "proven": proven, "libraryPath": lbr, "deviceset": want,
           "artifacts": artifacts, "missing": missing,
           "stepsSkipped": sorted(_skip), "sharedPackageProof": _shared_with,
           "evidence": evidence, "imagesCaptured": len(captured), "imagesRequired": 4}
    if thumb_unverified:
        out["thumbnailUnverified"] = thumb_unverified
        out["_thumbnailNote"] = ("The cloud thumbnail could not be observed (APS not signed in "
                                 "on this box) - the 4-frame local evidence stands, but verify "
                                 "the Library Manager preview by eye or sign in APS and run "
                                 "fusion_check_3d_thumbnail.")
    # RESOLVE OUR OWN DOCUMENT STATE BEFORE RETURNING (deterministic-close rule): the four
    # views dirtied the library document; closing it here, verified, means no Save dialog
    # can ever be raised at the user for this run's scratch work. The .lbr ON DISK is the
    # artifact; the Fusion document was only ever a rendering surface.
    try:
        _stem = lbr.rsplit("/", 1)[-1].rsplit(".", 1)[0]
        _note_bridge_doc(_stem)
        out["scratchDocClosed"] = _close_bridge_doc_verified(_stem).get("closed", False)
    except Exception:
        pass
    try:
        _PROVE_LAST[0] = dict(out)
        import tempfile as _tf
        open(os.path.join(_tf.gettempdir(), "fusion-prove-last.json"), "w",
             encoding="utf-8").write(json.dumps(out))
    except Exception:
        pass
    if proven:
        out["_hint"] = ("PROVEN: all three artifacts exist and all FOUR evidence frames were "
                        "captured app-sized - symbol editor, footprint editor, 3D editor, and "
                        "the ganged Library Manager view with pin connections. Attach "
                        "evidence[].screenshot in that order.")
    else:
        why = []
        if missing:
            why.append("missing artifacts: " + ", ".join(missing))
        if len(captured) != 4:
            why.append("only %d of 4 evidence frames captured" % len(captured))
        out["error"] = "NOT proven - " + "; ".join(why)
        out["errorCode"] = "library_not_proven"
        out["_hint"] = ("Do NOT present these frames as proof. A missing package3d renders as a "
                        "grey Placeholder that photographs like a real chip, and a frame that "
                        "failed its size check is not evidence of anything. Bind the 3D with "
                        "fusion_show_3d_package {stepPath} and re-run.")
    return out


COMMAND_HANDLERS["prove_library"] = _handle_prove_library



def _preflight_lbr(path: str) -> dict:
    """PRE-ANALYZE a .lbr: will Fusion open it? Every check is offline and named.

    John, 2026-08-21: "what if you just pre-analyze a library to make sure its going to
    open ok?" - this is that, as one reusable gate. It encodes every rejection Fusion has
    actually produced on this bridge: a truncated binary file (77-byte ads1220irvar, the
    blocking Error modal), empty <packages> (Fusion's silent "No Components"), dead
    <connects> (renders fine, wires nothing), and absent devicesets (EDIT prompts).
    Returns {ok, checks: [{check, ok, detail}], errors: [...]} - callers show the list.
    """
    import xml.etree.ElementTree as _ET
    checks = []
    def _c(name, ok, detail=""):
        checks.append({"check": name, "ok": bool(ok), "detail": detail})
        return ok
    p = (path or "").replace("\\", "/")
    try:
        sz = os.path.getsize(p)
    except Exception as e:
        _c("file-readable", False, str(e)[:120])
        return {"ok": False, "checks": checks,
                "errors": [c["check"] + ": " + c["detail"] for c in checks if not c["ok"]]}
    _c("file-readable", True, "%d bytes" % sz)
    if not _c("size-sane", sz >= 200,
              "%d bytes - a real EAGLE library is never this small (truncated transfer?)" % sz):
        pass
    root = None
    if sz >= 200:
        try:
            root = _ET.parse(p).getroot()
            _c("xml-parses", True)
        except Exception as e:
            _c("xml-parses", False, str(e)[:160])
    if root is not None:
        _c("root-is-eagle", root.tag == "eagle", "root is <%s>" % root.tag)
        pkgs = [x.get("name") for x in root.iter("package")]
        syms = [x.get("name") for x in root.iter("symbol")]
        devs = [x.get("name") for x in root.iter("deviceset")]
        _c("packages-nonempty", bool(pkgs),
           "%d package(s)" % len(pkgs) if pkgs else
           "EMPTY <packages> - Fusion opens this to a silent 'No Components' library")
        _c("symbols-present", bool(syms), "%d symbol(s)" % len(syms))
        # EAGLE's align vocabulary; anything else makes Fusion pop one warning PER text on
        # open (John, 2026-08-30: ten 'invalid value right for attribute align' lines).
        # Reported, never blocking: the library still opens.
        _ALIGNS = {"bottom-left", "bottom-center", "bottom-right", "center-left", "center",
                   "center-right", "top-left", "top-center", "top-right"}
        _bad_align = [t.get("align") for t in root.iter("text") if t.get("align") and t.get("align") not in _ALIGNS]
        _c("text-align-valid", True,
           ("%d <text> element(s) carry a non-EAGLE align value (%s); Fusion will list a warning "
            "for each on open. Fix upstream (adom-lbr group captions)." % (len(_bad_align), sorted(set(_bad_align))))
           if _bad_align else "all <text> align values are EAGLE values")
        _c("devicesets-present", bool(devs),
           "%d deviceset(s)" % len(devs) if devs else
           "no <deviceset> - EDIT <name>.dev will raise Fusion's Create-new prompt")
        if pkgs and syms:
            conn = _check_connects(p)
            _c("connects-resolve", bool(conn.get("ok")),
               ("%d connect(s), all resolving" % (conn.get("connects") or 0))
               if conn.get("ok") else str(conn.get("why") or "invalid")[:160])
        # DUPLICATE PAD NAMES sink the WHOLE library in Fusion's EAGLE importer while
        # passing every XML-level check (adom-bridge#93: ads1220irvar shipped SIX pads
        # named "17" - thermal pad + stitching vias - and Fusion refused the file with
        # its generic 'has errors' modal). Mirror of the wiki's LBR_DUPLICATE_PADS lint.
        _dups = []
        for pkg_el in root.iter("package"):
            seen_pads = {}
            for pad_el in list(pkg_el.iter("smd")) + list(pkg_el.iter("pad")):
                nm = pad_el.get("name")
                if nm:
                    seen_pads[nm] = seen_pads.get(nm, 0) + 1
            _dups += ["%s/%s x%d" % (pkg_el.get("name"), n, c)
                      for n, c in seen_pads.items() if c > 1]
        _c("pads-unique", not _dups,
           ("duplicate pad names - Fusion's importer refuses the ENTIRE library over "
            "these: %s. Rename to e.g. 17, 17@2, 17@3..." % ", ".join(_dups[:6]))
           if _dups else "all pad names unique per package")
    ok = all(c["ok"] for c in checks)
    # PART METADATA IS REPORTED SEPARATELY, ON PURPOSE. `ok` must keep meaning "this file
    # will open in Fusion" - a geometry-only library opens perfectly well. Whether it is
    # fit to ORDER from is a different question and just as important to an EE, so it gets
    # its own key rather than being folded into a pass/fail that gates opening.
    res = {"ok": ok, "checks": checks,
           "errors": [c["check"] + ": " + c["detail"] for c in checks if not c["ok"]]}
    if root is not None:
        try:
            _lint = _lint_part_metadata(p)
            res["metadataOk"] = _lint["ok"]
            res["metadataWarnings"] = _lint["warnings"]
            res["partTotals"] = _lint.get("totals")
            if not _lint["ok"]:
                res["_metadataHint"] = (
                    "The library will open, but %d part-metadata warning(s) mean an EE "
                    "cannot order from it: MPN / MANUFACTURER / a distributor PN "
                    "(MOUSER_PN, DIGIKEY_PN, LCSC_PN) are what this bridge's own BOM export "
                    "reads, and variants that share geometry are told apart ONLY by these. "
                    "See fusion_library_parts for the per-variant breakdown."
                    % len(_lint["warnings"]))
        except Exception:
            pass
    return res


# ── Part metadata + variants: the fields an EE actually orders from ──────────────
# John, 2026-08-28: "one chip could be the same symbol, footprint, and 3d chip, but there
# might be an automotive high temp version vs a cheaper baseline variant." That distinction
# is invisible in geometry - it lives ENTIRELY in the metadata - so a library that drops it
# is worse than useless: it looks complete and orders the wrong part.
#
# EAGLE expresses this with three nested names, and the full orderable part is all three:
#     <deviceset name>  +  <device name>  +  <technology name>
# deviceset = the logical part, device = the PACKAGE variant, technology = the GRADE/temp
# variant. Attributes hang off <technology>, which is why two technologies can share one
# symbol, one footprint and one 3D body and still be different products.
#
# These names are not arbitrary - they are what this bridge's OWN BOM exporter reads
# (addin/AdomBridge/commands/manufacturing.py): MPN, MANUFACTURER, MOUSER_PN, DIGIKEY_PN,
# LCSC_PN. Emitting anything else means the BOM comes out blank.
PART_ATTRS = ("MPN", "MANUFACTURER", "MOUSER_PN", "DIGIKEY_PN", "LCSC_PN")
_DISTRIBUTOR_ATTRS = ("MOUSER_PN", "DIGIKEY_PN", "LCSC_PN")


def _library_part_metadata(path: str) -> dict:
    """Every orderable variant in a .lbr, with its attributes. Offline, no Fusion.

    Returns {ok, devicesets:[{name, description, variants:[{device, technology, fullName,
    attributes{}, missing[]}]}], totals{...}}.
    """
    import xml.etree.ElementTree as _ET
    try:
        root = _ET.parse((path or "").replace("\\", "/")).getroot()
    except Exception as e:
        return {"ok": False, "error": str(e)[:160], "devicesets": []}
    out = []
    for ds in root.iter("deviceset"):
        ds_name = ds.get("name") or ""
        desc_el = ds.find("description")
        ds_desc = (desc_el.text or "").strip() if desc_el is not None else ""
        variants = []
        for dev in ds.iter("device"):
            dev_name = dev.get("name") or ""
            techs = list(dev.iter("technology")) or [None]
            for tech in techs:
                tname = (tech.get("name") or "") if tech is not None else ""
                attrs = {}
                if tech is not None:
                    for a in tech.iter("attribute"):
                        n = (a.get("name") or "").upper()
                        v = (a.get("value") or "").strip()
                        if n and v:
                            attrs[n] = v
                # EAGLE's orderable identity is the three names concatenated.
                full = "%s%s%s" % (ds_name, dev_name, tname)
                missing = [k for k in ("MPN", "MANUFACTURER") if not attrs.get(k)]
                if not any(attrs.get(d) for d in _DISTRIBUTOR_ATTRS):
                    missing.append("a distributor PN (MOUSER_PN/DIGIKEY_PN/LCSC_PN)")
                variants.append({"device": dev_name, "technology": tname,
                                 "fullName": full, "package": dev.get("package") or "",
                                 "attributes": attrs, "missing": missing})
        out.append({"name": ds_name, "description": ds_desc, "variants": variants})
    n_var = sum(len(d["variants"]) for d in out)
    return {"ok": True, "devicesets": out,
            "totals": {"devicesets": len(out), "variants": n_var,
                       "withMpn": sum(1 for d in out for v in d["variants"]
                                      if v["attributes"].get("MPN")),
                       "withDistributor": sum(1 for d in out for v in d["variants"]
                                              if any(v["attributes"].get(x)
                                                     for x in _DISTRIBUTOR_ATTRS))}}


def _lint_part_metadata(path: str) -> dict:
    """EE-grade warnings about a library's part metadata. Never blocks opening.

    Deliberately SEPARATE from _preflight_lbr's hard checks: a geometry-only library opens
    in Fusion perfectly well, so `ok` there must keep meaning "will open". These warnings
    mean something different and just as important - "is this fit to ORDER from".
    """
    md = _library_part_metadata(path)
    if not md.get("ok"):
        return {"ok": False, "warnings": [], "error": md.get("error")}
    warns = []
    for ds in md["devicesets"]:
        vs = ds["variants"]
        if not ds["description"]:
            warns.append({"code": "deviceset_no_description", "deviceset": ds["name"],
                          "detail": "No <description>. Fusion's Content Manager shows this "
                                    "to whoever places the part; without it they are picking "
                                    "on a symbol outline alone."})
        for v in vs:
            if v["missing"]:
                warns.append({"code": "variant_missing_part_numbers",
                              "deviceset": ds["name"], "variant": v["fullName"],
                              "missing": v["missing"],
                              "detail": "Missing %s. This bridge's own BOM export reads "
                                        "these exact attribute names, so the BOM comes out "
                                        "blank for this part." % ", ".join(v["missing"])})
        # ⚠️ THE VARIANT TRAP (John's automotive-vs-baseline case). Two variants that share
        # geometry are distinguishable ONLY by their metadata. If their MPNs match, or are
        # both absent, nobody downstream can tell an AEC-Q100 part from the cheap one.
        if len(vs) > 1:
            mpns = [v["attributes"].get("MPN") for v in vs]
            named = [m for m in mpns if m]
            if len(named) < len(vs):
                warns.append({"code": "variants_indistinguishable", "deviceset": ds["name"],
                              "detail": "%d variants but only %d carry an MPN. Variants that "
                                        "share a symbol, footprint and 3D body are "
                                        "distinguishable ONLY by metadata - an automotive "
                                        "grade part and a baseline one look identical here."
                                        % (len(vs), len(named))})
            elif len(set(named)) < len(named):
                warns.append({"code": "duplicate_variant_mpn", "deviceset": ds["name"],
                              "detail": "Two or more variants share the same MPN (%s), so "
                                        "they are not actually different parts - or one MPN "
                                        "is wrong." % ", ".join(sorted(set(
                                            m for m in named if named.count(m) > 1))[:3])})
    # GEOMETRY (John, 2026-08-29, the JST pad that would short and the crystal's stacked GND
    # pins): things an EE would reject on sight, reported before Fusion ever draws them.
    try:
        warns.extend(_lint_library_geometry(path))
    except Exception:
        pass
    return {"ok": not warns, "warnings": warns, "totals": md["totals"]}


def _lint_library_geometry(path: str) -> list:
    """Shape-aware geometry checks on a .lbr: pins stacked at one point, pad copper that
    overlaps or comes within 0.2 mm of a neighbour, generic class names/descriptions.
    Returns warning dicts in the same shape as the metadata lint."""
    import xml.etree.ElementTree as _ET
    import re as _re
    out = []
    root = _ET.parse((path or "").replace("\\", "/")).getroot()
    lib = root.find(".//library")
    if lib is None:
        return out
    # 1. symbol pins at identical coordinates
    for sym in lib.findall("./symbols/symbol"):
        seen = {}
        for pin in sym.findall("pin"):
            key = (round(float(pin.get("x") or 0), 3), round(float(pin.get("y") or 0), 3))
            if key in seen:
                out.append({"code": "symbol_pins_stacked", "symbol": sym.get("name"),
                            "detail": "pins %r and %r sit at the same point %s; one hides the other"
                                      % (seen[key], pin.get("name"), key)})
            seen[key] = pin.get("name")
        if not sym.findall("wire") and not sym.findall("rectangle") and not sym.findall("circle") \
                and not sym.findall("polygon") and len(sym.findall("pin")) <= 4:
            out.append({"code": "symbol_no_body", "symbol": sym.get("name"),
                        "detail": "symbol has pins but no body graphics (the converter dropped them)"})
    # 2. pad copper overlap / clearance, shape-aware
    def _box(el):
        x, y = float(el.get("x") or 0), float(el.get("y") or 0)
        if el.tag == "smd":
            dx, dy = float(el.get("dx") or 0), float(el.get("dy") or 0)
        else:
            dia = float(el.get("diameter") or 0) or (float(el.get("drill") or 0) * 1.5)
            shape = (el.get("shape") or "round").lower()
            dx, dy = (dia * 2, dia) if shape == "long" else (dia, dia)
        rot = str(el.get("rot") or "")
        if any(r in rot for r in ("R90", "R270")):
            dx, dy = dy, dx
        return (x - dx / 2, y - dy / 2, x + dx / 2, y + dy / 2)
    for pkg in lib.findall("./packages/package"):
        pads = pkg.findall("smd") + pkg.findall("pad")
        boxes = [(p.get("name"), _box(p)) for p in pads]
        for i in range(len(boxes)):
            for j in range(i + 1, len(boxes)):
                (na, a), (nb, b) = boxes[i], boxes[j]
                gap_x = max(a[0] - b[2], b[0] - a[2])
                gap_y = max(a[1] - b[3], b[1] - a[3])
                gap = max(gap_x, gap_y)
                if gap < 0:
                    out.append({"code": "pad_overlap", "package": pkg.get("name"),
                                "detail": "pads %r and %r overlap (copper intersects): a short on the board"
                                          % (na, nb)})
                elif gap < 0.2:
                    out.append({"code": "pad_clearance", "package": pkg.get("name"),
                                "detail": "pads %r and %r are only %.2f mm apart" % (na, nb, gap)})
    # 3. generic names / descriptions that tell an EE nothing
    _gen_name = _re.compile(r"^(L|R|C|D|LED|Q|Y|X|F|FB|J|Conn_\d+x\d+.*|Crystal(_\w+)?|Resistor|Capacitor|Inductor)$", _re.I)
    _gen_desc = _re.compile(r"^(inductor|resistor|capacitor|unpolarized capacitor|polarized capacitor|light emitting diode|led|diode|generic connector.*|four pin crystal.*|crystal)$", _re.I)
    for ds in lib.findall("./devicesets/deviceset"):
        n = ds.get("name") or ""
        d = (ds.findtext("description") or "").strip()
        if _gen_name.match(n):
            out.append({"code": "generic_deviceset_name", "deviceset": n,
                        "detail": "deviceset is named %r, a class name, not a part; the MPN belongs here" % n})
        if d and _gen_desc.match(d):
            out.append({"code": "generic_description", "deviceset": n,
                        "detail": "description %r says the class, not the part (value, package, maker)" % d})
    return out


def _part_metadata_report(path: str) -> dict:
    """A SHORT, user-facing metadata report an import can carry all the way up the chain.

    John, 2026-08-28: importing must NOT be blocked by missing part numbers, but the gap has
    to "bubble up all the way to the top, like in the wiki quick actions UI or via web
    control so the user sees 'oh, 2 of my 5 components had no mouser part number' so they
    can go fix it". The bridge is the end of the chain and cannot invent the data - but it
    is the only thing that has actually read the file, so it is the only thing that can say
    what is missing. Silence here means the user finds out at ordering time.

    Counts are per COMPONENT (deviceset), because that is the unit a user thinks in, with
    per-variant detail underneath for the automotive-vs-baseline case.
    """
    md = _library_part_metadata(path)
    if not md.get("ok"):
        return {"ok": None, "unavailable": md.get("error", "could not read the library")}
    lint = _lint_part_metadata(path)
    # TWO SEVERITIES, and conflating them made a GOOD library look broken (caught by my own
    # test): a part needs an MPN and a manufacturer, and it needs AT LEAST ONE distributor -
    # not all three. A library with DigiKey on one variant and Mouser on another is
    # perfectly orderable, and reporting "no LCSC part number" as a deficiency there is
    # noise that trains people to ignore the whole report.
    #   missing            -> actionable: cannot order this component
    #   absentDistributors -> informational: which catalogues lack a number, John's
    #                         "2 of my 5 components had no mouser part number"
    dist = ("MOUSER_PN", "DIGIKEY_PN", "LCSC_PN")
    fields = ("MPN", "MANUFACTURER") + dist
    comps, missing_counts = [], {f: 0 for f in fields}
    n_no_distributor = 0
    for ds in md["devicesets"]:
        vs = ds["variants"] or []
        # A component is missing a field when NO variant of it carries that field.
        absent = [f for f in fields if not any(v["attributes"].get(f) for v in vs)]
        for f in absent:
            missing_counts[f] += 1
        blocking = [f for f in ("MPN", "MANUFACTURER") if f in absent]
        if all(d in absent for d in dist):
            blocking.append("a distributor part number")
            n_no_distributor += 1
        if blocking or [d for d in dist if d in absent]:
            comps.append({"component": ds["name"], "variants": len(vs),
                          "missing": blocking,
                          "absentDistributors": [d for d in dist if d in absent],
                          "variantNames": [v["fullName"] for v in vs]})
    total = len(md["devicesets"])
    label = {"MPN": "no manufacturer part number", "MANUFACTURER": "no manufacturer",
             "MOUSER_PN": "no Mouser part number", "DIGIKEY_PN": "no DigiKey part number",
             "LCSC_PN": "no LCSC part number"}
    # DESCRIPTION (John, 2026-08-29, the Content Manager's empty Description column): a
    # deviceset with no description is as unusable as one with no part number; report it
    # in the same breath. The page fills it from the wiki brief when the library lacks it.
    n_no_desc = sum(1 for ds in md["devicesets"] if not (ds.get("description") or "").strip())
    missing_counts["DESCRIPTION"] = n_no_desc
    for ds in md["devicesets"]:
        if not (ds.get("description") or "").strip():
            hit = next((c for c in comps if c["component"] == ds["name"]), None)
            if hit:
                hit["missing"].append("a description")
            else:
                comps.append({"component": ds["name"], "variants": len(ds["variants"] or []),
                              "missing": ["a description"], "absentDistributors": [],
                              "variantNames": [v["fullName"] for v in (ds["variants"] or [])]})

    def _n(n, what):
        return "%d of %d component%s ha%s %s" % (n, total, "" if total == 1 else "s",
                                                 "s" if n == 1 else "ve", what)
    bits = [_n(missing_counts[f], label[f]) for f in ("MPN", "MANUFACTURER")
            if missing_counts[f]]
    if n_no_desc:
        bits.append(_n(n_no_desc, "no description"))
    if n_no_distributor:
        bits.append(_n(n_no_distributor, "no distributor part number at all"))
    return {
        "ok": lint["ok"],
        "componentsTotal": total,
        "variantsTotal": md["totals"]["variants"],
        "missingCounts": missing_counts,
        "componentsWithoutAnyDistributor": n_no_distributor,
        "distributorCoverage": {d: total - missing_counts[d] for d in dist},
        "components": comps[:40],
        "warnings": lint["warnings"],
        "summary": ("; ".join(bits) + ". The library imported fine - this is about ordering, "
                    "not geometry." ) if bits else
                   "All %d component(s) carry a manufacturer part number and at least one "
                   "distributor part number." % total,
        "distributorNote": ("; ".join(_n(missing_counts[d], label[d]) for d in dist
                                      if missing_counts[d]) or None),
        "userAction": ("Add the missing part numbers where the component metadata lives "
                       "(upstream of this bridge), then re-export." if bits else None),
    }


def _handle_library_parts(fusion_info: dict, args: dict) -> dict:
    """Every ORDERABLE variant in a .lbr with its part numbers. Offline, no Fusion needed.

    For an EE this is the point of the library: two variants can share one symbol, one
    footprint and one 3D body and still be different products (an AEC-Q100 automotive part
    versus the baseline one). That difference lives entirely in the metadata.
    """
    path = (args.get("libraryPath") or args.get("lbrPath") or args.get("filePath")
            or args.get("path") or "").replace("\\", "/")
    if not path:
        return {"success": False, "errorCode": "no_path_argument",
                "error": "No library path given; the file was never examined.",
                "acceptedArgs": ["libraryPath", "lbrPath", "filePath", "path"]}
    md = _library_part_metadata(path)
    if not md.get("ok"):
        return {"success": False, "errorCode": "library_unreadable",
                "error": md.get("error"), "libraryPath": path}
    lint = _lint_part_metadata(path)
    t = md["totals"]
    return {
        "success": True, "libraryPath": path,
        "devicesets": md["devicesets"], "totals": t,
        "metadataOk": lint["ok"], "warnings": lint["warnings"],
        "attributeNames": list(PART_ATTRS),
        "_hint": ("%d deviceset(s), %d orderable variant(s); %d carry an MPN and %d a "
                  "distributor PN. %s The full orderable identity is deviceset + device + "
                  "technology (e.g. TJA1050 + T + -AUTO), because EAGLE puts package "
                  "variants on <device> and grade/temperature variants on <technology> - "
                  "which is how two variants share geometry and remain different parts."
                  % (t["devicesets"], t["variants"], t["withMpn"], t["withDistributor"],
                     ("No metadata warnings." if lint["ok"] else
                      "%d warning(s) in `warnings` - these do NOT stop the library opening, "
                      "but an EE cannot order from it as-is." % len(lint["warnings"])))),
    }


def _handle_preflight_lbr(fusion_info: dict, args: dict) -> dict:
    """Verb wrapper: will this .lbr open cleanly in Fusion? Offline, instant, no Fusion."""
    p = (args.get("libraryPath") or args.get("filePath") or "").replace("\\", "/")
    if not p:
        return {"success": False, "error": "libraryPath required."}
    pf = _preflight_lbr(p)
    return {"success": pf["ok"], "preflight": pf, "libraryPath": p,
            "errorCode": None if pf["ok"] else "lbr_preflight_failed",
            "_hint": ("Every check green - Fusion will open this library cleanly."
                      if pf["ok"] else
                      "Fix the named check(s) BEFORE any Fusion verb touches this file - "
                      "each one maps to a real Fusion failure (blocking Error modal, silent "
                      "No Components, Create-new prompt, or dead wiring). Re-stage and "
                      "hash-compare if size-sane or xml-parses failed: transfer corruption "
                      "is the usual cause.")}


COMMAND_HANDLERS["preflight_lbr"] = _handle_preflight_lbr


def _kicad_ingest_unavailable_error() -> dict:
    """The shape-B refusal, shared by import_kicad and the create_library fast-fail.

    Platform rule (John, 2026-08-21): a bridge runs on Windows or Mac; ALL Adom binaries
    are Linux-container-only. So KiCad conversion can never happen on this box, and the
    contract is authoring-time: convert in a container, publish the validated .lbr on the
    part page, hand the bridge shape A (.lbr + .step)."""
    return {"success": False, "errorCode": "adom_lbr_unavailable",
            "kicadIngest": False,
            "error": "adom-lbr (the canonical KiCad->EAGLE converter) does not run on this "
                     "platform - Adom binaries are Linux-container-only by design - and this "
                     "bridge deliberately does NOT convert KiCad formats itself.",
            "_hint": ("Convert at AUTHORING time, where adom-lbr genuinely runs: in a "
                      "container, `adom-lbr generate --sym <.kicad_sym> --fp <.kicad_mod> "
                      "-o <out.lbr>` (auto-linted), publish the .lbr on the part page, then "
                      "call fusion_create_library shape A {libraryPath, stepPath}. "
                      "fusion_readiness reports kicadIngest so this is knowable before "
                      "staging files or launching Fusion.")}


def _find_adom_lbr() -> str:
    """Locate adom-lbr on THIS box (the canonical KiCad->EAGLE converter, adom/adom-lbr)."""
    import shutil as _sh
    hit = _sh.which("adom-lbr") or _sh.which("adom-lbr.exe")
    if hit:
        return hit
    la = os.environ.get("LOCALAPPDATA", "")
    for c in ([os.path.join(la, "Programs", "adom-lbr", "adom-lbr.exe"),
               os.path.join(la, "adom-lbr", "adom-lbr.exe")] if la else []):
        if os.path.exists(c):
            return c
    return ""



# _install_adom_lbr_windows was DELETED (2026-08-21). The platform rule it violated:
# a bridge runs on Windows or Mac, and ALL Adom binaries are Linux-container-only by
# deliberate decision - there will never be a windows adom-lbr asset for it to fetch
# (adom-lbr#6 withdrawn, adom-lbr#8 retracted by its own filer). Conversion happens at
# AUTHORING time in a container; the published .lbr is the runtime contract (page
# issue #43, adom-bridge#92).


def _apply_pin_map(lbr_path: str, pin_map: dict) -> dict:
    """Rewrite <connects> from an EXPLICIT {pinName: padName} map, then re-verify.

    The identity join (pin number == pad name) is convention, and when it breaks no
    converter can know the truth - it is datasheet knowledge. The caller supplies it here;
    a deterministic XML rewrite applies it; _check_connects validates the result. Refuses
    (and changes nothing) if the map references pins or pads that do not exist.
    """
    try:
        text = open(lbr_path, "r", encoding="utf-8", errors="replace").read()
    except Exception as e:
        return {"ok": False, "why": "unreadable: %s" % str(e)[:80]}
    import re as _re
    import xml.etree.ElementTree as _ET
    try:
        root = _ET.fromstring(text)
    except Exception:
        return {"ok": False, "why": "library is not valid XML"}
    pins = {p.get("name") for p in root.iter("pin") if p.get("name")}
    pads = {p.get("name") for p in root.iter("smd") if p.get("name")}
    pads |= {p.get("name") for p in root.iter("pad") if p.get("name")}
    bad = [(k, v) for k, v in pin_map.items() if k not in pins or v not in pads]
    if bad:
        return {"ok": False, "why": "map references missing pins/pads",
                "bad": bad[:8], "pins": sorted(pins)[:20], "pads": sorted(pads)[:20]}
    gate = ( _re.findall(r'<gate name="([^"]+)"', text) or ["G$1"])[0]
    conns = "\n".join('<connect gate="%s" pin="%s" pad="%s"/>' % (gate, k, v)
                      for k, v in pin_map.items())
    joined = "<connects>\n%s\n</connects>" % conns
    if _re.search(r"<connects>.*?</connects>", text, _re.S):
        text = _re.sub(r"<connects>.*?</connects>", joined, text, count=1, flags=_re.S)
    elif "<connects/>" in text:
        text = text.replace("<connects/>", joined, 1)
    else:
        return {"ok": False, "why": "no <connects> element to rewrite"}
    open(lbr_path, "w", encoding="utf-8").write(text)
    chk = _check_connects(lbr_path)
    return {"ok": bool(chk.get("ok")), "applied": len(pin_map), "verify": chk}


def _handle_import_kicad(fusion_info: dict, args: dict) -> dict:
    """Ingest KiCad symbol/footprint files and produce a valid .lbr (issue #43) - by
    DELEGATING to adom-lbr, the ecosystem's canonical converter.

    John, 2026-08-21: "just use adom-lbr as your converter" - instead of this bridge
    growing its own KiCad parser. adom-lbr owns the conversion, ships a linter whose
    checks name the exact failure this issue was filed about (empty-packages /
    empty-connects -> Fusion "No Components"), and round-trips KiCad, EAGLE and Altium.
    One converter, one set of bugs, one place to fix them.

    args: kicadSymbolPath, kicadFootprintPath (Windows paths), libraryPath (output,
    optional), name (optional override).
    """
    import subprocess as _sp
    sym_p = (args.get("kicadSymbolPath") or "").replace("\\", "/")
    mod_p = (args.get("kicadFootprintPath") or "").replace("\\", "/")
    lbr_p = (args.get("libraryPath") or "").replace("\\", "/")
    if not sym_p and not mod_p:
        return {"success": False, "errorCode": "kicad_source_required",
                "error": "Pass kicadSymbolPath and/or kicadFootprintPath."}
    tool = _find_adom_lbr()
    if not tool:
        return _kicad_ingest_unavailable_error()
    if not lbr_p:
        base = (sym_p or mod_p).rsplit("/", 1)
        stem = base[-1].rsplit(".", 1)[0]
        lbr_p = (base[0] + "/" if len(base) > 1 else "") + stem + ".lbr"
    cmd = [tool, "generate"]
    if sym_p:
        cmd += ["--sym", sym_p]
    if mod_p:
        cmd += ["--fp", mod_p]
    if args.get("name"):
        cmd += ["--name", str(args["name"])]
    cmd += ["-o", lbr_p]
    try:
        r = _sp.run(cmd, capture_output=True, text=True, timeout=120)
    except Exception as e:
        return {"success": False, "errorCode": "adom_lbr_failed",
                "error": "adom-lbr did not run: %s" % str(e)[:150]}
    if r.returncode != 0:
        return {"success": False, "errorCode": "adom_lbr_failed",
                "error": "adom-lbr generate failed (exit %d): %s"
                         % (r.returncode, (r.stderr or r.stdout)[-300:]),
                "_hint": "adom-lbr auto-lints its output and exits 2 on lint failure - the "
                         "message above names the exact check that failed."}
    names = _library_artifact_names(lbr_p)
    pin_map = args.get("pinMap")
    pm_result = None
    if isinstance(pin_map, dict) and pin_map:
        pm_result = _apply_pin_map(lbr_p, {str(k): str(v) for k, v in pin_map.items()})
        if not pm_result.get("ok"):
            return {"success": False, "errorCode": "pin_map_invalid",
                    "error": "The explicit pinMap could not be applied: %s" % pm_result.get("why"),
                    "pinMapDetail": pm_result,
                    "_hint": "pinMap must be {pinName: padName} using names that exist - the "
                             "detail lists the symbol's pins and the package's pads."}
    return {"success": True, "libraryPath": lbr_p, "converter": "adom-lbr",
            "pinMapApplied": (pm_result or {}).get("applied"),
            "symbols": names.get("sym", []), "packages": names.get("pac", []),
            "devicesets": names.get("dev", []), "has3d": bool(names.get("p3d")),
            "toolOutput": (r.stdout or "").strip()[-300:],
            "_hint": ("Generated by adom-lbr (lint-clean by construction). fusion_show_symbol / "
                      "show_footprint consume it directly; bind the 3D with "
                      "fusion_bind_library_3d {libraryPath, stepPath}, then "
                      "fusion_prove_library captures the four evidence frames.")}


COMMAND_HANDLERS["import_kicad"] = _handle_import_kicad



_PROVE_LAST = [None]


def _mark_cancelled(state: dict, run_id: str, at_stage: str, detail: str):
    """Terminal state for a cancelled run: honest about what finished before the stop."""
    _req = _cancel_requested(run_id) or {}
    state.update(stage="cancelled", success=False, verdict="cancelled",
                 errorCode="cancelled_by_user", cancelRequested=True,
                 cancelledAt=at_stage, cancelledBy=_req.get("by") or "user",
                 error="Cancelled by %s while %s. %s" % (_req.get("by") or "the user", at_stage, detail),
                 stageLabel="cancelled while %s" % at_stage)
    try:
        _banner("update", title="Import cancelled", detail=detail[:120], percent=None,
                eta_sec=0, tone="failed")
        import threading as _bt
        _bt.Timer(20.0, lambda: _banner("hide")).start()
    except Exception:
        pass


def _handle_cancel_run(fusion_info: dict, args: dict) -> dict:
    """Stop a library import at its next part boundary (John, 2026-08-29).

    Cancel is cooperative, not a kill: the bind loop finishes the part it is on (a STEP
    import cannot be interrupted mid-command without leaving a half-made design), keeps
    every urn already minted, binds them into the library, closes the scratch documents and
    leaves the library open. The run ends stage:cancelled / verdict:cancelled with
    cancelledAt telling which stage and how many parts were done. Idempotent.
    """
    import tempfile as _tf
    import time as _ct
    _rid = str(args.get("runId") or "").strip()
    _runs = {}
    try:
        _runs = json.loads(open(os.path.join(_tf.gettempdir(), "fusion-prove-runs.json"),
                                encoding="utf-8").read())
    except Exception:
        _runs = {}
    _terminal = ("done", "failed", "cancelled")
    if not _rid:
        # the latest run still in flight (single-caller convenience, same rule as prove_result)
        _live = [r for r in _runs.values() if r.get("stage") not in _terminal]
        if _live:
            _rid = sorted(_live, key=lambda r: r.get("startedAt") or 0)[-1].get("runId") or ""
    if not _rid:
        return {"success": False, "errorCode": "no_run_in_flight",
                "error": "No import is running on this box, so there is nothing to cancel.",
                "_hint": "fusion_prove_result {} shows the latest run and its stage."}
    _st = _runs.get(_rid) or (dict(_PROVE_LAST[0]) if _PROVE_LAST[0] and _PROVE_LAST[0].get("runId") == _rid else None)
    if _st is None:
        return {"success": False, "errorCode": "unknown_run_id",
                "error": "no run with runId %r on this box" % _rid}
    if _st.get("stage") in _terminal:
        return {"success": True, "runId": _rid, "alreadyFinished": True, "stage": _st.get("stage"),
                "_hint": "That run had already ended (%s); nothing to cancel." % _st.get("stage")}
    _by = str(((args.get("caller") or {}).get("aiThread")) or args.get("by") or "user")[:60]
    _CANCEL_REQUESTS[_rid] = {"at": _ct.time(), "by": _by, "stage": _st.get("stage")}
    # let pollers see the request on their very next tick, before the chain reaches a boundary
    try:
        _st["cancelRequested"] = True
        _st["stageLabel"] = "stopping at the next part boundary (%s)" % (_st.get("stageLabel") or _st.get("stage") or "")
        _runs[_rid] = _st
        open(os.path.join(_tf.gettempdir(), "fusion-prove-runs.json"), "w",
             encoding="utf-8").write(json.dumps(_runs))
        if _PROVE_LAST[0] and _PROVE_LAST[0].get("runId") == _rid:
            _PROVE_LAST[0]["cancelRequested"] = True
    except Exception:
        pass
    try:
        _banner("update", title="Stopping the import", detail="Finishing the current part, then stopping",
                percent=None, eta_sec=None, tone="working")
    except Exception:
        pass
    return {"success": True, "runId": _rid, "cancelRequested": True, "stage": _st.get("stage"),
            "statusVerb": "fusion_prove_result",
            "_hint": "The run stops at its next part boundary (a STEP import in progress finishes "
                     "first, up to ~60s). Poll fusion_prove_result {runId}: stage becomes 'cancelled' "
                     "with cancelledAt and what was completed. Parts already bound keep their 3D; "
                     "the library stays open in Fusion, unsaved to the cloud."}


def _handle_prove_result(fusion_info: dict, args: dict) -> dict:
    """Fetch the LAST completed fusion_prove_library run.

    prove_library drives four editor views with settle loops and routinely outlives ab's
    relay window - and since it returned its evidence IN the response, a timeout LOST the
    result while the work had succeeded (caught live on the CR0603 chain, 2026-08-21). The
    run now persists its outcome, and this verb hands it back: fire prove_library, and on a
    relay timeout poll THIS until it reports the run you started.
    """
    # RESULT IDENTITY (#52 item 6, 2026-08-24): with several threads driving one Fusion,
    # "the last run globally" is the WRONG run for anyone who passed a runId - the matrix
    # runner picked up another thread's TLV9001 chain as its own result. A given runId is
    # now authoritative: looked up in the per-run store, and an unknown id is an ERROR,
    # never silently the latest.
    _rid = str(args.get("runId") or "").strip()
    res = None
    if _rid:
        try:
            import tempfile as _tf
            _runs = json.loads(open(os.path.join(_tf.gettempdir(), "fusion-prove-runs.json"),
                                    encoding="utf-8").read())
            res = _runs.get(_rid)
        except Exception:
            res = None
        if res is None:
            _pl = _PROVE_LAST[0]
            if _pl and _pl.get("runId") == _rid:
                res = _pl
        if res is None:
            return {"success": False, "errorCode": "unknown_run_id",
                    "error": "no prove run with runId %r on this box" % _rid,
                    "_hint": "runId comes from fusion_create_library's response. Omit runId "
                             "for the latest completed run (single-caller convenience only)."}
    if res is None:
        res = _PROVE_LAST[0]
    if res is None:
        try:
            import tempfile as _tf
            res = json.loads(open(os.path.join(_tf.gettempdir(), "fusion-prove-last.json"),
                                  encoding="utf-8").read())
        except Exception:
            res = None
    if res is None:
        return {"success": False, "errorCode": "no_prove_run",
                "error": "No fusion_prove_library run has completed on this box yet.",
                "_hint": "Run fusion_prove_library {libraryPath}; if the relay times out, the "
                         "bridge keeps working - poll this verb until the result appears."}
    # INLINE FRAMES ARE OPT-IN (issue #47's web-control ask): each evidence entry carries
    # thumbnailDataUrl, but four base64 frames on EVERY 15s poll would bloat the relay
    # body. Pass {inlineFrames:true} on the FINAL fetch to get them; plain polls return
    # paths + metadata only.
    if not args.get("inlineFrames"):
        try:
            res = json.loads(json.dumps(res))
            for e in ((res.get("prove") or {}).get("evidence") or []) + (res.get("evidence") or []):
                e.pop("thumbnailDataUrl", None)
        except Exception:
            pass
    # LIVE PROGRESS FOR WEB-CONTROL (John, 2026-08-21: "as accurate of progress bars as we
    # can ... you should know how long stuff takes by measuring it"). Computed AT READ TIME
    # from the persisted stage plan + this machine's measured stage history, so every poll
    # gets fresh numbers with no ticker thread: percent = (sum of completed stages'
    # estimates + elapsed of the current stage, capped at 95% of its estimate) / total.
    # The estimates are the rolling per-machine p50s that update after EVERY run, so the
    # bar gets more accurate the more this user uses it - per box, automatically.
    try:
        if res.get("stage") not in (None, "done", "failed") and res.get("plan"):
            import time as _tp
            plan = res.get("plan") or []
            total = sum(float(p.get("estimatedSec") or 0) for p in plan) or 1.0
            done_names = {d.get("stage") for d in (res.get("stagesDone") or [])}
            done_est = sum(float(p.get("estimatedSec") or 0) for p in plan
                           if p.get("stage") in done_names)
            cur = next((p for p in plan if p.get("stage") not in done_names), None)
            cur_elapsed = max(0.0, _tp.time() - float(res.get("stageStartedAt") or _tp.time()))
            cur_est = float((cur or {}).get("estimatedSec") or 1)
            cur_credit = min(cur_elapsed, cur_est * 0.95)
            pct = int(100 * (done_est + cur_credit) / total)
            eta = max(0, int(total - done_est - cur_credit))
            confs = [p.get("confidence") for p in plan]
            # ALSO top-level (found on the first live 2-part run, 2026-08-28): every hint,
            # the fusion-web-control skill and the prompt handed to the ab thread all say
            # "percent, etaSec" as if top-level, while they only ever lived under
            # res["progress"]. A caller following our own documentation read None the
            # whole run. Both places now, so either reading works.
            res["percent"] = min(95, pct)
            res["etaSec"] = eta
            res["progress"] = {
                "percent": min(95, pct), "etaSec": eta,
                "elapsedSec": int(cur_elapsed), "stepLabel": res.get("stage"),
                "estimatedSec": int(total),
                "confidence": ("measured" if all(c == "measured" for c in confs)
                               else "unknown" if all(c == "unknown" for c in confs)
                               else "typical"),
                "overdue": cur_elapsed > cur_est,
            }
    except Exception:
        pass
    want = str(args.get("runId") or "")
    if want and str(res.get("runId") or "") != want:
        # PER-RUN store first (issue #47: concurrent chains used to overwrite the single
        # latest slot, so a valid runId could match NOTHING and the caller's poll hung).
        try:
            import tempfile as _tfr
            _runs = json.loads(open(os.path.join(_tfr.gettempdir(),
                                                 "fusion-prove-runs.json"),
                                    encoding="utf-8").read())
        except Exception:
            _runs = {}
        if want in _runs:
            _r2 = _runs[want]
            if not args.get("inlineFrames"):
                try:
                    _r2 = json.loads(json.dumps(_r2))
                    for e in ((_r2.get("prove") or {}).get("evidence") or []) + (_r2.get("evidence") or []):
                        e.pop("thumbnailDataUrl", None)
                except Exception:
                    pass
            return {"success": True, "result": _r2,
                    "_hint": "Your run, from the per-run store (concurrent chains no "
                             "longer overwrite each other)."}
        return {"success": False, "errorCode": "run_not_found",
                "error": "No stored result for run %s (latest is %s; %d run(s) in the "
                         "per-run store)." % (want, res.get("runId"), len(_runs)),
                "knownRunIds": list(_runs)[-8:],
                "_hint": "If your chain JUST started, keep polling - its first state lands "
                         "within seconds. A runId absent after ~30s means the chain never "
                         "started (check the create_library response for an immediate "
                         "refusal like lbr_preflight_failed) - treat that as TERMINAL "
                         "rather than polling out your budget."}
    return {"success": True, "result": res,
            "_hint": "The last completed prove run (survives relay timeouts and respawns)."}


COMMAND_HANDLERS["prove_result"] = _handle_prove_result
COMMAND_HANDLERS["cancel_run"] = _handle_cancel_run




def _publish_library_to_cloud(lbr_path: str, name: str, folder: str = "Adom Libraries",
                              replace_existing: bool = False) -> dict:
    """Save the bound library into the user's cloud project so Fusion's Library Manager lists it.

    Folder is created if missing (project root/<folder>). A same-named library already there is
    reported (Fusion allows duplicate names; this saves a NEW document and says so, update-in-
    place is a follow-up). Returns {saved, name, project, folder, fileId, versionId,
    duplicatesBefore, error}.
    """
    out = {"saved": False, "name": name, "folder": folder}
    stem = lbr_path.replace("\\", "/").rsplit("/", 1)[-1].rsplit(".", 1)[0]
    name = (name or stem).strip()
    out["name"] = name
    # NAME THE PROJECT (run 93ccf05278ac, 2026-08-29): the add-in's default is
    # app.data.activeProject, which raises InternalValidationError on this install. Resolve
    # the same project the mint uploads to (the Adom default, else 'Main', else the first
    # healthy one) by NAME, and hand it to every cloud call below.
    project_name = ""
    try:
        pr = _proxy_to_addin("list_cloud_projects", {}, timeout=60)
        _projects = []
        for hub in ((pr.get("data") or {}).get("hubs") or []):
            for pj in (hub.get("projects") or []):
                if isinstance(pj, dict) and pj.get("name"):
                    _projects.append(pj)
        for pj in _projects:
            if pj.get("id") == _DEFAULT_UPLOAD_PROJECT:
                project_name = pj["name"]; break
        if not project_name:
            project_name = next((pj["name"] for pj in _projects if pj.get("name") == "Main"),
                                _projects[0]["name"] if _projects else "")
    except Exception as e:
        out["projectNote"] = str(e)[:120]
    out["project"] = project_name or None
    _pn = {"projectName": project_name} if project_name else {}
    try:
        r = _proxy_to_addin("create_cloud_folder", dict(_pn, folderName=folder), timeout=60)
        out["folderCreated"] = bool(r.get("success")) and not bool((r.get("data") or {}).get("existed"))
    except Exception as e:
        out["folderNote"] = str(e)[:120]
    _older = []
    try:
        lst = _proxy_to_addin("list_cloud_files", dict(_pn, folderPath=folder), timeout=60)
        for f in ((lst.get("data") or {}).get("files") or []):
            if str(f.get("name") or "").lower() == name.lower():
                _older.append({"id": f.get("id"), "versionNumber": f.get("versionNumber")})
        out["duplicatesBefore"] = len(_older)
        out["olderSameName"] = [f.get("id") for f in _older]
    except Exception:
        out["duplicatesBefore"] = None
    # Make the library the active document ourselves (the generic helper also demands a
    # library workspace and gave up after the prove left a 3D document in front, 2026-08-29).
    import time as _tpa
    # The prove stage closes the library behind itself, so it is usually NOT open here; open it
    # from disk (the bound file is the truth) before trying to activate it.
    try:
        _names = _open_doc_names()
        _bare_names = {re.sub(r"\s*\([^)]*\)\s*$", "", n.replace("*", "")).strip().lower() for n in _names}
        # Only reopen when the document list is KNOWN and the library is not in it; an empty
        # list under a busy main thread means "unknown", and reopening then stacks a copy.
        if _names and stem.lower() not in _bare_names:
            _op = _orchestrate_open_lbr({"filePath": lbr_path})
            out["reopened"] = bool(_op.get("success"))
            if not _op.get("success"):
                out["error"] = "could not reopen the library to save it: " + str(_op.get("error"))[:160]
                return out
    except Exception as e:
        out["reopenNote"] = str(e)[:120]
    _deadline = _tpa.time() + 120
    _seen = None
    _active_ok = False
    while _tpa.time() < _deadline:
        try:
            _proxy_to_addin("activate_document", {"name": stem}, timeout=10)
            st = _proxy_to_addin("get_app_state", {}, timeout=8)
            _seen = (st.get("data") or {})
            if stem.lower() in str(_seen.get("activeDocument") or "").lower():
                _active_ok = True
                break
        except Exception as e:
            _seen = {"error": str(e)[:80]}
        _tpa.sleep(2)
    if not _active_ok:
        out["error"] = ("library document %r could not be made active (last seen: %s), so it was not "
                        "saved" % (stem, json.dumps(_seen)[:160]))
        return out
    r = _proxy_to_addin("save_to_cloud", dict(_pn, name=name, folderPath=folder,
                                              description="Imported by Adom (bound 3D packages)"),
                        timeout=180)
    d = r.get("data") or {}
    if not r.get("success"):
        out["error"] = str(r.get("error") or "save_to_cloud failed")[:200]
        return out
    out.update(saved=True, project=d.get("project"), folder=d.get("folder") or folder,
               fileId=d.get("fileId"), versionId=d.get("versionId"), versionNumber=d.get("versionNumber"))
    # NO ASTERISK LEFT BEHIND (John, 2026-08-29, the "XL7026E1 *" in the Content Manager):
    # a library the user is left with must be CLEAN. saveAs should leave it saved; verify
    # from the document itself, and if anything re-dirtied it, save once more in place.
    try:
        _clean = False
        for _try in range(2):
            di = _proxy_to_addin("document_info", {}, timeout=10)
            for dd in ((di.get("data") or {}).get("openDocuments") or []):
                if str(dd.get("name") or "").rstrip("*").strip().lower() == stem.lower():
                    _clean = bool(dd.get("isSaved")) and not (dd.get("isModified") or str(dd.get("name") or "").endswith("*"))
            if _clean:
                break
            _proxy_to_addin("run_modeling_script",
                            {"script": "import adsk.core\napp=adsk.core.Application.get()\n"
                                       "d=app.activeDocument\nok=d.save('Imported by Adom') if d and not d.isSaved else True\n"
                                       "result={'saved': bool(ok), 'isSaved': bool(d.isSaved) if d else None}"},
                            timeout=120)
        out["savedClean"] = _clean
    except Exception as e:
        out["savedCleanNote"] = str(e)[:120]
    # UPDATE IN PLACE (John, 2026-08-29: a re-import must update THE library, not mint a second).
    # Fusion's saveAs always creates a new document, so one-library-per-name means retiring the
    # older same-named documents in OUR folder once the new save has landed. That removes cloud
    # documents, so it runs only when the caller asks (replaceExisting:true), only for files in
    # <project>/<folder> with this exact name, by id, and never the one just saved. Without the
    # flag the older copies are left alone and named in olderSameName so the caller can decide.
    replaced = []
    if replace_existing and d.get("fileId") and _older:
        for f in _older:
            fid = f.get("id")
            if not fid or fid == d.get("fileId"):
                continue
            try:
                rr = _proxy_to_addin("delete_cloud_file", dict(_pn, fileId=fid, folderPath=folder), timeout=60)
                if rr.get("success"):
                    replaced.append(fid)
            except Exception:
                pass
    out["replaced"] = replaced
    out["note"] = (("replaced %d older library document(s) of the same name in %s; a board that "
                    "referenced an old document by id must re-link to this one" % (len(replaced), folder))
                   if replaced else
                   ("%d older library document(s) of the same name remain in %s (pass replaceExisting:true "
                    "to retire them)" % (len(_older), folder) if _older else "first library of this name in %s" % folder))
    _bridge_action_ledger("library_saved_to_cloud", {"name": name, "folder": folder,
                                                     "fileId": d.get("fileId"), "replaced": replaced})
    return out

def _handle_create_library(fusion_info: dict, args: dict) -> dict:
    """ONE verb for the whole "create a Fusion library" task - one call, one consent.

    John, 2026-08-21: web-control needs to hand the bridge EITHER a ready .lbr + .step, OR
    KiCad sources (+ optional pinMap) - two entry shapes, one contract. And ab measured what
    multi-verb flows cost a browser caller: every cross-origin call is a human consent
    (#30's nine-prompt incident). So this verb accepts both shapes and runs the whole chain
    itself: [convert via adom-lbr] -> [bind the STEP] -> [prove: four frames + verdict].

    Entry shapes:
      A: {libraryPath, stepPath?}                                - .lbr given, bind + prove
      B: {kicadSymbolPath?, kicadFootprintPath?, stepPath?,
          pinMap?, libraryPath?}                                 - convert, then bind + prove

    The chain outlives ab's relay window by design, so it returns IMMEDIATELY with
    statusVerb fusion_prove_result (the #32 pollable pattern): fire once, poll one verb,
    both results (progress and final) come from state the bridge persists.
    """
    import threading as _th
    import tempfile as _tf
    lbr_p = (args.get("libraryPath") or "").replace("\\", "/")
    sym_p = (args.get("kicadSymbolPath") or "").replace("\\", "/")
    mod_p = (args.get("kicadFootprintPath") or "").replace("\\", "/")
    step_p = (args.get("stepPath") or "").replace("\\", "/")
    # ⛔ EVERY LIBRARY IS MULTI-PART. ONE PART IS N=1 (John, 2026-08-28: "any library
    # probably should just be considered multi-part and if it then just so happens to have
    # one part, so be it").
    # This used to be a whole SINGULAR family (create_library{stepPath},
    # bind_library_3d{stepPath}, attach_3d_package{modelPath}) beside a PLURAL one
    # (build_library_3d{parts[]}), which forced a caller to know their part count before
    # they could even pick a verb. Part count is DATA, not a different operation. `parts`
    # is now the canonical input and `stepPath` is sugar that normalises into it, so both
    # shapes run the identical code path and a 1-part library is not a special case.
    parts_in = args.get("parts") or []
    # ⛔ SINGLE-FLIGHT (John, 2026-08-28). Two Create Fusion Library presses seconds apart
    # started TWO chains (cea6c079 and ba243a1c); both entered their launch stage, both
    # restarted Fusion, both then bound into one Fusion at once. There is exactly one
    # Fusion and one main thread; a second import must wait for or attach to the first,
    # never race it. Return the running run's id so the caller can poll it instead.
    try:
        import tempfile as _tfs, json as _jss, time as _tts
        _rp = os.path.join(_tfs.gettempdir(), "fusion-prove-runs.json")
        _runs = _jss.loads(open(_rp, encoding="utf-8").read()) if os.path.exists(_rp) else {}
        _live = [(k, v) for k, v in _runs.items()
                 if v.get("verb", "create_library") in ("create_library", None)
                 and v.get("stage") not in ("done", "failed", "cancelled", None)
                 and v.get("startedAt")
                 and _tts.time() - float(v.get("startedAt")) < 1800]
        if _live:
            _rid, _rv = _live[-1]
            return {"success": True, "started": False, "attached": True,
                    "runId": _rid, "statusVerb": "fusion_prove_result",
                    "stage": _rv.get("stage"),
                    "_hint": "An import is ALREADY running (%s, stage: %s). Not starting a "
                             "second one - two chains racing one Fusion is how it got "
                             "restarted under a user today. Poll this runId." % (_rid, _rv.get("stage"))}
    except Exception:
        pass
    # HYGIENE AFTER A RESPAWN (2026-08-29): a chain killed by a bridge restart cannot close its
    # own scratch documents (its list died with the process; five Untitled tabs were left). Any
    # run the reconcile marked bridge_restarted_mid_run and nobody swept gets swept here, before
    # a new chain adds to the pile: never-modified Untitled docs and *_3d mint designs only.
    try:
        _sweep_orphan_scratch_docs()
    except Exception:
        pass
    if not lbr_p and not (sym_p or mod_p):
        return {"success": False, "errorCode": "no_input",
                "error": "Give me EITHER libraryPath (a ready .lbr) OR kicadSymbolPath / "
                         "kicadFootprintPath (KiCad sources).",
                "_hint": "Shape A: {libraryPath, stepPath}. Shape B: {kicadSymbolPath, "
                         "kicadFootprintPath, stepPath, pinMap?}. stepPath is required for "
                         "a PROVEN library - without it there is no real 3D body.",
                "_hintParts": "`parts:[...]` is a SUPPLEMENT, not an input on its own: it says "
                              "which STEP binds to which package INSIDE the library you also "
                              "pass as libraryPath. Passing only parts[] lands here.",
                "_hintLibraryPath": "libraryPath is the INPUT .lbr that must already exist. It "
                                    "is NOT an output path - naming a file you have not written "
                                    "yet fails preflight with lbr_preflight_failed "
                                    "('file-readable: cannot find the file'). To write the bound "
                                    "library somewhere else, use outLbrPath.",
                "_hintWholeLibrary": "Importing a whole wiki Cloud Library? Do not assemble "
                                     "parts[] by hand - `adom-wiki library export <ref> --format "
                                     "fusion` gives a zip, and fusion_install_library_bundle "
                                     "{zipPath} pairs every deviceset to its STEP for you."}
    # SHAPE B FAILS AT THE FRONT DOOR, never three stages in (#43 follow-up, 2026-08-21):
    # the old flow staged files, launched Fusion (a minute of the user's time), and THEN
    # discovered adom-lbr is absent - which on Windows/Mac it always is, by platform rule.
    # Knowable up front, so refused up front; fusion_readiness carries the same flag.
    if (sym_p or mod_p) and not _find_adom_lbr():
        return _kicad_ingest_unavailable_error()
    # SHAPE A PRE-ANALYSIS at the same front door (John, 2026-08-21: "pre-analyze a
    # library to make sure its going to open ok"): a bad .lbr must never cost the caller
    # a Fusion LAUNCH to discover. Same named checks as fusion_preflight_lbr.
    if lbr_p and not (sym_p or mod_p):
        # MID-WRITE TOLERANCE (#49 "start-failed / no runId", 2026-08-23): a caller that
        # STAGES the .lbr and calls this in the same breath can race the file's final flush -
        # preflight's own size-sane / xml-parses checks (built to catch a TRUNCATED file)
        # then fire on a file that is merely half-written, the call returns lbr_preflight_failed
        # with NO runId, and the very next attempt (file now complete) succeeds. That is a
        # transient the code can absorb: re-run preflight a few times with a short settle so a
        # still-flushing file gets its beat. A GENUINELY bad .lbr fails every try and still
        # returns fast. Only the truncation-class checks are worth retrying; a structural error
        # (root-not-eagle, no devicesets) will not fix itself, but re-checking it is cheap.
        import time as _pf_t
        _pf = _preflight_lbr(lbr_p)
        for _pf_try in range(4):
            if _pf["ok"]:
                break
            _pf_t.sleep(0.6)
            _pf = _preflight_lbr(lbr_p)
        if not _pf["ok"]:
            return {"success": False, "errorCode": "lbr_preflight_failed",
                    "preflight": _pf, "runId": None, "preflightRetries": _pf_try,
                    "error": "Pre-analysis says Fusion will NOT open this library (re-checked "
                             "%d times over ~%.1fs in case it was mid-write): "
                             % (_pf_try + 1, _pf_try * 0.6) +
                             "; ".join(_pf["errors"])[:360],
                    "_hint": "This is NOT 'Fusion failed to start' - there is no runId because "
                             "the .lbr did not pass pre-analysis (errorCode lbr_preflight_failed; "
                             "see preflight.errors). If your harness stages the file, ensure the "
                             "write is FLUSHED/closed before calling. fusion_preflight_lbr re-runs "
                             "these checks in isolation."}

    import uuid as _uuid
    run_id = _uuid.uuid4().hex[:12]

    def _chain():
        import time as _tst
        state = {"stage": "starting", "success": None, "libraryPath": lbr_p or None,
                 "runId": run_id, "startedAt": _tst.time(),
                 "verb": "create_library"}
        # Register with the hang gate: while this chain runs (and is under the 900s
        # ceiling) a non-pumping main thread is WORK, not a hang (#49 mid-mint kills).
        _LONG_OP["name"] = "create_library:" + run_id
        _banner_watchdog_start(run_id)   # keep the in-Fusion palette present for the whole run
        # 'show' (not 'update') at the start of a run: it re-opens a palette the user closed
        # with its X during an earlier run; every later update respects a close.
        _banner("show", title="%s: starting" % _lib_label(lbr_p), detail="checking the bundle and Fusion")
        _LONG_OP["since"] = __import__("time").time()
        _docs_before = _open_doc_names()   # chain hygiene: everything opened after this is ours
        # OWNERSHIP (John, 2026-08-30): every document this chain opens is the BRIDGE's, so a
        # later restart never asks the user about it. The library itself, plus the per-part
        # generator/3d documents, are registered here; the snapshot above is what the user had.
        try:
            if lbr_p:
                _note_bridge_doc(lbr_p.replace("\\", "/").rsplit("/", 1)[-1].rsplit(".", 1)[0])
            for _n in (_docs_before or []):
                _SESSION_DOCS_AT_START.add(str(_n).lower())
        except Exception:
            pass
        def _save():
            # PER-RUN persistence (issue #47 follow-up, 2026-08-21): a single latest-run
            # slot meant two concurrent chains OVERWROTE each other - the matrix's poll
            # hung 18 minutes on a runId whose state my verification chain had clobbered,
            # and a no-runId poll returned someone else's run. Every run keeps its own
            # entry (capped at 20, insertion order); latest stays for no-runId polls.
            try:
                _PROVE_LAST[0] = dict(state)
                open(os.path.join(_tf.gettempdir(), "fusion-prove-last.json"), "w",
                     encoding="utf-8").write(json.dumps(state))
                _runs_p = os.path.join(_tf.gettempdir(), "fusion-prove-runs.json")
                try:
                    _runs = json.loads(open(_runs_p, encoding="utf-8").read())
                except Exception:
                    _runs = {}
                _runs[run_id] = dict(state)
                while len(_runs) > 20:
                    _runs.pop(next(iter(_runs)))
                open(_runs_p, "w", encoding="utf-8").write(json.dumps(_runs))
            except Exception:
                pass
        import timings as _tmc
        import time as _tc
        # Stage estimates are MEASURED per machine and self-improving: each completed stage
        # records its actual into the rolling store, so the very next run's bar is paced by
        # what THIS box just did. Seeds (used only until real samples exist): launch 60s,
        # convert 5s, bind 60s (measured 45s on AdomLapper), prove 300s (four frames +
        # thumbnail wait).
        #
        # DOCTRINE (run 8bb786b174ee, 2026-08-21): the chain once failed with "Fusion 360 is
        # installed but not running" - an error the code can fix. Fusion not being up is a
        # STAGE of this chain, never a refusal: probe once, and when the add-in is not
        # answering, launch Fusion ourselves and wait for the round trip.
        _needs_launch = not _addin_round_trip_ready(tries=1)
        # ⛔ THE BIND ESTIMATE MUST SCALE WITH THE PART COUNT (John, 2026-08-27).
        # It was a flat 60s no matter how many parts the wiki sent. A multi-part library
        # imports ONE STEP PER PART - the slowest thing this chain does - so a 7-part
        # bundle showed a bar that filled in a minute and then sat at 100% for many more.
        # A progress bar that lies is worse than none: the user concludes it has hung.
        # Per-part seconds are MEASURED per machine under their own key, so the bar is
        # paced by what THIS box actually does, and only the seed is a literal.
        _n_parts = max(1, len(args.get("parts") or []) or 1)
        _per_part, _pp_conf = _tmc.estimate("stage:bind_per_part", 60.0)
        _bind_est = _per_part * _n_parts
        _STAGES = [("launch", 60.0, _needs_launch),
                   ("convert", 5.0, bool(sym_p or mod_p)),
                   ("bind", _bind_est, bool(step_p or parts_in)),
                   ("prove", 300.0, True)]
        state["plan"] = [{"stage": n, "estimatedSec": round(_tmc.estimate("stage:" + n, f)[0] or f, 1),
                          "confidence": _tmc.estimate("stage:" + n, f)[1]}
                         for n, f, active in _STAGES if active]
        state["stagesDone"] = []
        _est_by_stage = {p["stage"]: float(p.get("estimatedSec") or 1.0)
                         for p in state["plan"]}
        _est_total = sum(_est_by_stage.values()) or 1.0
        _est_done = [0.0]
        def _enter(stage_name):
            state["stage"] = stage_name
            state["stageStartedAt"] = _tc.time()
            _save()
            # honest taskbar progress instead of orange attention-flashes (2026-08-23)
            _pct = 100.0 * _est_done[0] / _est_total
            _taskbar_progress(_pct)
            # ...and the SAME story inside Fusion's own window (2026-08-28)
            # THE PALETTE MUST SAY WHAT IS HAPPENING (John, 2026-08-30: "whenever you're driving
            # fusion, you MUST show better info than this"): which library, which stage, how many
            # parts, and what the stage does. "Working / 0%" is not information.
            _n = int(state.get("partsTotal") or 1)
            _lib = _lib_label(state.get("libraryPath") or lbr_p)
            _stage_key = stage_name.split(" (")[0].lower()
            _what = {
                "starting": "Unpacking the bundle and pairing every deviceset with its STEP",
                "launch": "Starting Fusion and waiting for the Adom add-in",
                "convert": "Converting KiCad sources into an EAGLE library",
                "binding 3d": ("Minting one 3D package per footprint (%d STEP import%s), then binding the "
                               "urns into the library" % (_n, "" if _n == 1 else "s")),
                "proving": ("Opening every deviceset and capturing its symbol, footprint, 3D and "
                            "component views (%d part%s)" % (_n, "" if _n == 1 else "s")),
            }
            _desc = next((v for k, v in _what.items() if _stage_key.startswith(k)), stage_name)
            _banner("update", title="%s: %s" % (_lib, stage_name.split(" (")[0]),
                    detail=_desc, percent=_pct, eta_sec=max(0, _est_total - _est_done[0]))
        def _leave(stage_key):
            took = _tc.time() - float(state.get("stageStartedAt") or _tc.time())
            _tmc.log_run("stage:" + stage_key, took, ok=True)
            # the estimate for the NEXT run reads "stage:<key>|any"; log_run only fed
            # "verb:stage:<key>", so stages never graduated from 'typical' to 'measured'
            _tmc.record("stage:" + stage_key, took)
            state["stagesDone"] = state.get("stagesDone", []) + [
                {"stage": stage_key, "seconds": round(took, 1)}]
            _est_done[0] += _est_by_stage.get(stage_key, 0.0)
            _taskbar_progress(100.0 * _est_done[0] / _est_total)
        try:
            if _needs_launch:
                # SAY WHAT IS ACTUALLY HAPPENING (John, 2026-08-28: "i don't get why colby's
                # interface is saying 'launching fusion' when its already launched"). This
                # stage name flowed straight up the chain to the wiki's progress line. When
                # Fusion is already running and only the add-in is silent, "launching" is a
                # lie about the reason, and the user cannot reconcile it with the Fusion
                # window in front of them. Name the real condition; the wiki renders `stage`.
                if _is_fusion_running():
                    # ⛔ ASK, NEVER JUST RESTART (John, 2026-08-28: "why do you not give me a
                    # choice?", and again 2026-08-29 when the restart-ask test showed this
                    # stage fired no toast at all and _handle_launch killed Fusion after a
                    # 30s gate). The two-button notice is the user's decision point; this
                    # chain records the payload for the page, fires the toast, and WAITS.
                    # ASK ONLY WHEN THERE IS SOMETHING OF THE USER'S TO LOSE (John, 2026-08-30:
                    # "i NEVER opened any docs in fusion, ONLY YOU DID so asking me is just
                    # slowing you down"). The bridge registers every document it opens
                    # (_BRIDGE_DOCS) and snapshots what was open when it started; an unsaved
                    # document that the bridge did not open is the only reason to ask. With
                    # none, the bridge restarts Fusion itself, saving what can be saved.
                    _ri0 = _addin_restart_info(fire_toast=False)
                    _mine = _ri0.get("userUnsaved")
                    if _mine is None:
                        _mine = _user_unsaved_docs(_ri0.get("unsavedDocumentNames") or [])
                    if not _mine:
                        _enter("Fusion is open but its Adom add-in is not answering - "
                               "restarting Fusion (only bridge documents are open)")
                        state["stageReason"] = "addin_not_answering"
                        state["restartChoice"] = "auto"
                        state["restartReason"] = ("no unsaved user documents; open: %s"
                                                  % (_ri0.get("openDocuments") or []))
                        _save()
                        try:
                            _rr = _handle_restart_for_addin(fusion_info, {"save": True, "auto": True})
                            state["restartResult"] = {k: _rr.get(k) for k in ("success", "error", "errorCode")}
                        except Exception as _e:
                            state["restartResult"] = {"success": False, "error": str(_e)[:160]}
                        _save()
                        _chosen = "restarted"
                    else:
                        _chosen = None
                    if _chosen != "restarted":
                     _enter("Fusion is open but its Adom add-in is not answering - "
                           "asking you whether to save and restart it (%d unsaved document(s) "
                           "look like the user's: %s)" % (len(_mine), ", ".join(str(n) for n in _mine)[:70]))
                     state["stageReason"] = "addin_not_answering"
                     _ri = _addin_restart_info(fire_toast=True)
                     state.update(errorCode="addin_restart_required",
                                  userMessage=_ri.get("userMessage"),
                                  documentsKnown=_ri.get("documentsKnown"),
                                  openDocuments=_ri.get("openDocuments"),
                                  unsavedDocuments=_ri.get("unsavedDocuments"),
                                  remedyVerb="fusion_restart_for_addin",
                                  notificationId=(_ri.get("toast") or {}).get("id"))
                     _save()
                     _nid = (_ri.get("toast") or {}).get("id")
                     if _nid:
                         _watch_restart_choice(_nid)
                     _ask_deadline = _tc.time() + 1800
                     _chosen = None
                     while _tc.time() < _ask_deadline:
                         if _addin_round_trip_ready(tries=1):
                             _chosen = "restarted"; break        # restarted (by them or by the remedy)
                         if _user_restart_hold_active():
                             _chosen = "hold"; break
                         _tc.sleep(4)
                     if _chosen == "hold":
                         state.update(stage="failed", success=False, errorCode="user_owns_restart",
                                      error="You chose 'I'll handle it' on the restart notice, so "
                                            "this import stopped without touching Fusion. Restart "
                                            "Fusion yourself, then start the import again.")
                         _save(); return
                     if _chosen != "restarted":
                         state.update(stage="failed", success=False, errorCode="awaiting_user_restart",
                                      error="Fusion's add-in stayed silent for 30 minutes and nobody "
                                            "answered the restart notice. Nothing was restarted. "
                                            "Save and restart Fusion, then start the import again.")
                         _save(); return
                    state.pop("errorCode", None); state.setdefault("restartChoice", "restart")
                    _save()
                else:
                    _enter("launching Fusion (add-in loads at start)")
                    state["stageReason"] = "fusion_not_running"
                    _save()
                    dispatch_command("start", {})
                _deadline = _tc.time() + 240
                while _tc.time() < _deadline and not _addin_round_trip_ready(tries=1):
                    _tc.sleep(5)
                if not _addin_round_trip_ready(tries=1):
                    state["launch"] = {"success": False,
                                       "error": "Fusion did not become ready within 240s "
                                                "of the chain launching it."}
                    state.update(stage="failed", success=False); _save(); return
                _leave("launch")
            target_lbr = lbr_p
            if sym_p or mod_p:
                _enter("converting (adom-lbr)")
                conv = _handle_import_kicad(fusion_info, {
                    "kicadSymbolPath": sym_p, "kicadFootprintPath": mod_p,
                    "libraryPath": lbr_p, "pinMap": args.get("pinMap"),
                    "name": args.get("name")})
                state["convert"] = {k: conv.get(k) for k in
                                    ("success", "errorCode", "error", "libraryPath",
                                     "packages", "symbols", "pinMapApplied")}
                if not conv.get("success"):
                    state.update(stage="failed", success=False); _save(); return
                _leave("convert")
                target_lbr = conv.get("libraryPath")
            state["libraryPath"] = target_lbr
            # Normalise to the plural shape HERE, once the target .lbr is known, because a
            # single stepPath needs the library's own package name to become a part entry.
            _parts = list(parts_in)
            if not _parts and step_p:
                try:
                    _pkgs = _library_artifact_names(target_lbr).get("pac") or []
                except Exception:
                    _pkgs = []
                _parts = [{"package": _pkgs[0] if _pkgs else "",
                           "lbrPath": target_lbr, "modelPath": step_p}]
            state["partsTotal"] = len(_parts)
            if _parts:
                _enter("binding 3D (STEP -> f3d -> package3d)")
                # ONE binder for one part or seven. Run synchronously: this chain is
                # already detached, so a second background thread would only hide the
                # per-part progress the caller is polling for.
                # THE BUSY GATE MUST NOT KILL THE CHAIN (run b1b1451e3298, 2026-08-29): a poller's
                # 0.1s get_app_state made the dispatcher refuse build_library_3d with
                # main_thread_busy and the whole import failed at 0%. A momentary busy answer is
                # a wait, not a verdict: retry for up to two minutes before giving up.
                bind = {}
                # DO NOT START A MINT UNDER A BUSY ADD-IN (run 445659e5c108, 2026-08-30): a
                # fusion_open_lbr issued seconds earlier was still running inside Fusion, the
                # first mint's saveAs came back empty with "the add-in is BUSY", and the whole
                # run failed at 0%. Wait up to two minutes for the add-in to report idle first.
                for _idle_try in range(60):
                    _st_idle = _check_addin_status(timeout=3.0) or {}
                    # Our own palette update is not "a previous command" (run 16c2e13149f4,
                    # 2026-08-30: the watchdog beat every 10 s kept this loop waiting on
                    # busyCommand=activity_banner for the full two minutes).
                    if not _st_idle.get("busy") or _st_idle.get("busyCommand") in ("activity_banner", "get_app_state"):
                        break
                    state["stageLabel"] = "waiting for Fusion to finish a previous command before binding"
                    _save()
                    _tc.sleep(2)
                for _bt_try in range(40):
                    bind = dispatch_command("build_library_3d",
                                            {"lbrPath": target_lbr, "parts": _parts,
                                             "outLbrPath": target_lbr, "async": False,
                                             "_cancelKey": run_id})
                    _berr = str(bind.get("error") or "") + str(bind.get("errorCode") or "")
                    if bind.get("success") or "main thread busy" not in _berr.lower() \
                            and "main_thread_busy" not in _berr:
                        break
                    _tc.sleep(3)
                state["bind"] = {k: bind.get(k) for k in
                                 ("success", "error", "wip_urn", "reopened",
                                  "reopenError")}
                state["bind"]["parts"] = bind.get("results") or bind.get("parts")
                state["bind"]["partsBound"] = bind.get("partsBound")
                if bind.get("cancelled") or _cancel_requested(run_id):
                    _mark_cancelled(state, run_id, "binding 3D",
                                    "%s of %s parts had their 3D bound before the stop"
                                    % (bind.get("partsBound") or 0, len(_parts)))
                    _save(); return
                if not bind.get("success"):
                    # NEVER surface a failure with error:null (#49, John 2026-08-23). If the
                    # bind handler somehow returns no reason, synthesize one from whatever it
                    # did carry so the caller always sees WHY, never a silent rejection next
                    # to a real urn.
                    if not state["bind"].get("error"):
                        _u = state["bind"].get("wip_urn")
                        state["bind"]["error"] = (
                            ("bind reported failure but gave no reason; a urn WAS minted (%s) "
                             "- likely a post-mint step (library re-open / verify) failed "
                             "silently" % _u) if _u else
                            "bind failed and returned no reason and no urn")
                    # The top-level error is what every poller renders; a bind failure buried
                    # in state["bind"]["error"] read as error:null on the Quick Actions page
                    # (run 445659e5c108, 2026-08-30).
                    state.update(stage="failed", success=False, verdict="not_proven",
                                 errorCode=(bind.get("errorCode") or "bind_failed"),
                                 error="3D bind failed: " + str(state["bind"].get("error"))[:300])
                    _save(); return
                _leave("bind")
                # Per-part mint libraries were scaffolding (one deviceset each so the generator
                # loads the right footprint); nothing of them may linger on the user's disk.
                for _cd in (args.get("_cleanupAfterBind") or []):
                    try:
                        import tempfile as _tfc
                        if str(_cd).startswith(_tfc.gettempdir()):
                            shutil.rmtree(_cd, ignore_errors=True)
                    except Exception:
                        pass
            _enter("proving (four evidence frames)")
            # #60: publish each artifact THE MOMENT it lands, in assembly order
            # (symbol -> footprint -> 3d -> ganged), so a poller watches the bundle build
            # instead of waiting ~370s for all four. The frame carries its own label, so a
            # renderer can show what just arrived and which phase is running.
            def _publish_frame(entry):
                ev = state.setdefault("evidence", [])
                ev.append(entry)
                state["evidenceCount"] = len(ev)
                state["lastArtifact"] = entry.get("step")
                # Total is parts x 4, not a literal 4: on the first real 2-part run this
                # read "captured symbol (5 of 4)" - a narration line the front end renders
                # under the progress bar, reading nonsense from part two onward.
                _tot = int(state.get("framesExpected") or 0) or 4 * max(1, int(state.get("partsToProve") or state.get("partsTotal") or 1))
                state["stageLabel"] = ("part %s of %s: captured %s (frame %d of %d)"
                                       % (state.get("partIndex") or "?", state.get("partsToProve") or "?",
                                          entry.get("step"), len(ev), _tot))
                _banner("update", title="%s: proving part %s of %s" % (_lib_label(target_lbr), state.get("partIndex") or "?",
                                                                        state.get("partsToProve") or "?"),
                        detail="%s. Captured %s (frame %d of %d)" % (_named(entry.get("part") or "library", _descs_p),
                                                                     entry.get("step"), len(ev), _tot),
                        percent=100.0 * (_est_done[0] + _est_by_stage.get("prove", 0.0)
                                         * len(ev) / _tot) / _est_total,
                        eta_sec=max(0, _est_total - _est_done[0]
                                    - _est_by_stage.get("prove", 0.0) * len(ev) / _tot))
                _save()

            # ⛔ PROVE EVERY PART, NOT JUST THE FIRST (John, 2026-08-28).
            # _handle_prove_library takes a singular `name` and defaults to devs[0], so a
            # ten-component library used to stream FOUR frames and call itself proven -
            # evidence for one part presented as evidence for the library. Same false-pass
            # family as #58 and #62, just at library scale. Now every deviceset is proved,
            # so a 10-part library streams 40 frames, four per component, each tagged with
            # the part it belongs to so a front end can group them as they arrive.
            _want = (args.get("name") or "").strip()
            try:
                _all_devs = _library_artifact_names(target_lbr).get("dev") or []
            except Exception:
                _all_devs = []
            _targets = [_want] if _want else (_all_devs or [""])
            _descs_p = _library_descriptions(target_lbr)
            state["partsToProve"] = len(_targets)
            state["provedParts"] = []
            _prove_results, _all_ok = [], True
            _last_beat = [_tc.time()]
            # ONE FOOTPRINT + 3D PROOF PER PACKAGE (John, 2026-08-30). Devicesets that share a
            # package (symbol variants, value clones) get only their symbol and ganged frames;
            # the footprint and the cloud 3D design were proved with the first deviceset on
            # that package. Nine CH340G variants: 20 frames instead of 36, and the slow 3D
            # open happens once instead of nine times.
            try:
                _dev_pac = _library_artifact_names(target_lbr).get("dev_pac") or {}
            except Exception:
                _dev_pac = {}
            _pkg_proved_by = {}
            _uniq_pkgs = len({tuple(v) for v in _dev_pac.values()}) or len(_targets)
            state["framesExpected"] = 4 * min(_uniq_pkgs, len(_targets)) + 2 * max(0, len(_targets) - _uniq_pkgs)
            for _pi, _dev in enumerate(_targets, 1):
                # A RUN MUST NOT OUTLIVE A SLEEP (John, 2026-08-30: "it seems odd to me that
                # its the next morning and you're still running this job"). The chain lives
                # on the laptop; when the machine sleeps mid-run it simply carries on at wake,
                # hours later, driving Fusion in the user's face. A gap of more than 15 minutes
                # between two parts is a sleep, not a slow part: stop, keep what is done.
                # The beat is the END of the previous part (a slow part is not a sleep: run
                # 17c9908274fd tripped this on a 17-minute part because the beat was taken at
                # the START of the part).
                _gap = _tc.time() - _last_beat[0]
                if _gap > 900:
                    state["provedParts"] = _prove_results
                    _mark_cancelled(state, run_id, "proving",
                                    "the machine appears to have slept for %d minutes between two parts; "
                                    "%d of %d parts proved before it, the 3D is bound for every part. Start "
                                    "the import again for a full proof." % (int(_gap // 60), _pi - 1, len(_targets)))
                    state["errorCode"] = "interrupted_by_sleep"
                    state["cancelledBy"] = "sleep"
                    state["error"] = state["error"].replace("Cancelled by user while", "Stopped while")
                    _save(); return
                if _cancel_requested(run_id):
                    state["provedParts"] = _prove_results
                    _mark_cancelled(state, run_id, "proving",
                                    "%d of %d parts proved before the stop; the 3D is bound "
                                    "for every part" % (_pi - 1, len(_targets)))
                    _save(); return
                state["stageLabel"] = ("proving %s (part %d of %d)"
                                       % (_dev or "library", _pi, len(_targets)))
                state["partIndex"] = _pi
                _save()
                # The palette must say WHERE we are before the first frame lands (John,
                # 2026-08-29: "10 parts" with no position "leaves me, the human, a bit lost").
                _banner("update", title="%s: proving part %d of %d" % (_lib_label(target_lbr), _pi, len(_targets)),
                        detail="%s. Opening its symbol, footprint, 3D package and ganged view"
                               % _named(_dev or "library", _descs_p),
                        percent=100.0 * (_est_done[0] + _est_by_stage.get("prove", 0.0)
                                         * (_pi - 1) / max(1, len(_targets))) / _est_total,
                        eta_sec=max(0, _est_total - _est_done[0]
                                    - _est_by_stage.get("prove", 0.0) * (_pi - 1) / max(1, len(_targets))))

                def _frame_for(entry, _d=_dev):
                    entry = dict(entry)
                    entry["part"] = _d          # tag it so 40 frames stay groupable
                    entry["kind"] = "proof"     # vs "main"/"popup" from fusion_live_feed
                    return _publish_frame(entry)

                _pk_key = tuple(_dev_pac.get(_dev) or [])
                _shared = _pkg_proved_by.get(_pk_key) if _pk_key else None
                _r = _handle_prove_library(fusion_info, {
                    "libraryPath": target_lbr, "name": _dev,
                    # the bind just told us exactly which f3d holds the geometry - hand it
                    # on so the 3D frame never depends on re-parsing the lbr for it
                    "f3dUrn": ((state.get("bind") or {}).get("wip_urn") or ""),
                    "_skipSteps": (["footprint", "package3d"] if _shared else []),
                    "_sharedWith": _shared,
                    "_onFrame": _frame_for})
                _prove_results.append({"part": _dev, "proven": bool(_r.get("proven")),
                                       "error": _r.get("error"),
                                       "sharedPackageProof": _shared})
                if _r.get("proven") and _pk_key and not _shared:
                    _pkg_proved_by[_pk_key] = _dev
                state["provedParts"] = _prove_results
                _last_beat[0] = _tc.time()
                _all_ok = _all_ok and bool(_r.get("proven"))
                # Close what THIS deviceset's frames opened (the 3D frame's design, any
                # Untitled) before moving to the next one; the library itself stays.
                try:
                    _lib_stem = (target_lbr or "").replace("\\", "/").rsplit("/", 1)[-1].rsplit(".", 1)[0].lower()
                    _prove_results[-1]["scratchClosed"] = _close_docs_opened_since(
                        _docs_before, keep={_lib_stem})
                except Exception:
                    pass
                _save()
            # SECOND PASS FOR THE FIRST-PART RACE (run 23cd0f587c13, 2026-08-29): right after the
            # bind reopens the library, Fusion reported NO active document for ~90s while its cloud
            # save settled, so part 1 of 10 scored not_proven on library_not_active and the other
            # nine passed. That is a timing miss, not a bad part: re-prove exactly those parts once,
            # after the rest, when the library has long been active.
            # Also retried: add-in timeouts and "not responding" (run 6aeb6f853c17, 2026-08-29:
            # CH340G and IN-S63BTG missed on addin_command_timeout while Fusion was sluggish and
            # passed nothing else wrong). Those are timing misses too; a real geometry failure
            # says so in its error and is NOT retried.
            _transient = ("ACTIVE document", "addin_command_timeout", "not responding",
                          "main thread busy", "main_thread_busy")
            _retry = [r for r in _prove_results if not r["proven"]
                      and any(k in str(r.get("error") or "") for k in _transient)]
            for _rr in _retry:
                if _cancel_requested(run_id):
                    break
                _dev = _rr["part"]
                state["stageLabel"] = "re-proving %s (its first pass hit a timing miss, not a part fault)" % _dev
                _save()
                def _frame_for_retry(entry, _d=_dev):
                    entry = dict(entry); entry["part"] = _d; entry["kind"] = "proof"; entry["retry"] = True
                    return _publish_frame(entry)
                _r2 = _handle_prove_library(fusion_info, {
                    "libraryPath": target_lbr, "name": _dev,
                    "f3dUrn": ((state.get("bind") or {}).get("wip_urn") or ""),
                    "_onFrame": _frame_for_retry})
                _rr.update(proven=bool(_r2.get("proven")), error=_r2.get("error"), retried=True)
                try:
                    _lib_stem = (target_lbr or "").replace("\\", "/").rsplit("/", 1)[-1].rsplit(".", 1)[0].lower()
                    _rr["scratchClosed"] = _close_docs_opened_since(_docs_before, keep={_lib_stem})
                except Exception:
                    pass
                state["provedParts"] = _prove_results
                _save()
            _all_ok = all(r["proven"] for r in _prove_results)
            # The library is proven only if EVERY part is. One good part does not vouch
            # for nine unchecked ones.
            prove = {"proven": _all_ok, "parts": _prove_results,
                     "partsProven": sum(1 for r in _prove_results if r["proven"]),
                     "partsTotal": len(_prove_results)}
            _leave("prove")
            # ⛔ "done" WITH ZERO FRAMES IS A LIE (page issue #65, Colby, 2026-08-27).
            # A caller polling the documented contract watched this walk launching ->
            # proving -> done with NO evidence frames and NO errorCode, while Fusion's
            # "<file> has errors and cannot be opened" dialog sat on the user's screen.
            # stage is what a poller branches on, so a chain that captured nothing must
            # never reach the terminal stage that means it worked. Same false-pass class
            # as #58 and #62: reporting a non-result as success.
            _frames = len(state.get("evidence") or [])
            _dlg_text = ""
            if not _frames:
                # An open-error dialog is the usual cause, and its text is the single most
                # useful thing we can hand back - so read it rather than guessing.
                try:
                    from handlers.dialog_classify import _dialog_text as _dtxt
                    for _d in (classify_blocking_dialogs() or []):
                        _t = (_dtxt(_d.get("hwnd")) or _d.get("title") or "")
                        if _t:
                            _dlg_text = str(_t)[:400]
                            break
                except Exception:
                    pass
            if not _frames:
                state.update(
                    stage="failed", success=False, verdict="not_proven",
                    errorCode=("blocked_by_dialog" if _dlg_text else "no_evidence_captured"),
                    error=(("Fusion is showing a blocking dialog and the chain captured no "
                            "evidence: %s" % _dlg_text) if _dlg_text else
                           "The prove step completed but captured ZERO evidence frames, so "
                           "there is nothing proving this library imported. Reporting failed "
                           "rather than done."),
                    dialogText=(_dlg_text or None),
                    evidenceCount=0, prove=prove,
                    _hint=("stage is 'failed', not 'done', because zero of the four evidence "
                           "frames were captured. %s Re-run after clearing the condition; if "
                           "the library genuinely imported, this is a bridge defect worth "
                           "filing." % ("The dialog text above is what Fusion is showing the "
                                        "user right now." if _dlg_text else
                                        "No blocking dialog was detected, so the import "
                                        "likely never produced a document to photograph.")))
            else:
                state.update(stage="done", success=bool(prove.get("proven")),
                             verdict=("proven" if prove.get("proven") else "not_proven"),
                             evidenceCount=_frames, prove=prove)
            # THE LIBRARY MUST END UP IN THE USER'S LIBRARY MANAGER (John, 2026-08-29: "the user
            # is just after a multipart library"). Until now the chain bound, proved, and closed
            # the local .lbr with save:false, leaving a proven FILE and no library in Fusion.
            _pub = args.get("_publishToCloud")
            if _pub and (state.get("bind") or {}).get("success"):
                try:
                    state["stageLabel"] = "saving the library to your cloud Library Manager"
                    _save()
                    state["cloudLibrary"] = _publish_library_to_cloud(
                        target_lbr, _pub.get("name") or "", _pub.get("folder") or "Adom Libraries",
                        replace_existing=bool(_pub.get("replaceExisting")))
                except Exception as _pe:
                    state["cloudLibrary"] = {"saved": False, "error": str(_pe)[:200]}
                _save()
            # Resolve the in-Fusion banner to match the verdict, then let it linger briefly
            # so the human sees the outcome, and hide it - a stale "working" banner over an
            # idle Fusion would be the same lie as a stuck progress bar.
            try:
                _ok = state.get("stage") == "done" and state.get("success")
                _banner("update",
                        title=("Library imported" if _ok else "Import did not complete"),
                        detail=(("%d of %d parts proven" % (
                                    (state.get("prove") or {}).get("partsProven", 0),
                                    (state.get("prove") or {}).get("partsTotal", 0)))
                                if _ok else str(state.get("error") or state.get("errorCode") or "")[:120]),
                        percent=100 if _ok else None,
                        eta_sec=0, tone=("done" if _ok else "failed"))
                import threading as _bt
                _bt.Timer(20.0, lambda: _banner("hide")).start()
            except Exception:
                pass
            # The part-number gap rides the RUN STATE, so web control sees it on the same
            # poll it already makes rather than needing a second call (John, 2026-08-28).
            # Attached on the failure path too: a chain that could not prove the geometry
            # still read the file, and the user still wants to know what is missing.
            try:
                _lbr_for_meta = state.get("libraryPath") or target_lbr
                if _lbr_for_meta:
                    state["partMetadata"] = _part_metadata_report(_lbr_for_meta)
            except Exception:
                pass
            _save()
        except Exception as e:
            state.update(stage="failed", success=False, verdict="not_proven",
                         error=str(e)[:400]); _save()
            _banner("update", title="Import did not complete", detail=str(e)[:120],
                    tone="failed")
            try:
                import threading as _bt
                _bt.Timer(20.0, lambda: _banner("hide")).start()
            except Exception:
                pass
        finally:
            _banner_watchdog_stop(run_id)
            if _LONG_OP.get("name") == "create_library:" + run_id:
                _LONG_OP["name"] = None
                _LONG_OP["since"] = 0.0
            _taskbar_progress(None)  # drop the bar + kill any lingering orange flash
            # A BATCH MUST NOT NEED THE CALLER TO SWEEP BETWEEN PARTS (page issue #47,
            # failure mode 5: unsaved library docs accumulated until the add-in stopped
            # responding and one part's mess failed every part after it). Close THIS
            # chain's library document on success AND failure - prove's own close is one
            # path here, this is the guarantee. Idempotent when already closed.
            # LEAVE THE LIBRARY OPEN FOR THE USER (John, 2026-08-29: "otherwise they then have
            # to go find it"). A bundle import ends with the saved library in front of the
            # user and only the scratch closed. Other callers keep the old close-everything
            # contract (batch matrices sweep between parts).
            # Bundle imports leave the library open on success AND on failure (John, 2026-08-29,
            # after a failed rerun left him an empty Untitled): if the library document exists,
            # it stays; only scratch is closed.
            _keep_open = bool(args.get("_publishToCloud"))
            _lbr_done = (state.get("libraryPath") or lbr_p or "")
            _stem_done = _lbr_done.replace("\\", "/").rsplit("/", 1)[-1].rsplit(".", 1)[0] if _lbr_done else ""
            try:
                if _lbr_done and not _keep_open:
                    _closed = _close_bridge_doc_verified(_stem_done)
                    state["libraryDocClosed"] = bool(_closed.get("closed"))
                    _save()
            except Exception:
                pass
            # CHAIN HYGIENE: close EVERY doc this chain opened (the Create3DPackage
            # 'Untitled' generators, the saved 3D, anything a frame left behind), not just
            # the library. Pre-existing docs are untouched.
            try:
                state["scratchDocsClosed"] = _close_docs_opened_since(
                    _docs_before, keep={_stem_done.lower()} if _keep_open else set())
                _save()
            except Exception:
                pass
            if _keep_open and _stem_done:
                try:
                    _proxy_to_addin("activate_document", {"name": _stem_done}, timeout=10)
                    state["libraryDocClosed"] = False
                    state["libraryLeftOpen"] = True
                    _save()
                except Exception:
                    pass

    # THE RUN MUST BE ON FILE BEFORE THE RUNID LEAVES THIS FUNCTION (2026-08-28, cold-path
    # run bc3141915abf): the page polled 0.4s after receiving the runId and got
    # unknown_run_id, because the chain thread writes its first state after planning.
    # A pollable contract that can answer "no such run" for its own fresh runId is a lie.
    try:
        import tempfile as _tf0, json as _js0, time as _t0
        _rp0 = os.path.join(_tf0.gettempdir(), "fusion-prove-runs.json")
        _runs0 = _js0.loads(open(_rp0, encoding="utf-8").read()) if os.path.exists(_rp0) else {}
        _runs0[run_id] = {"runId": run_id, "verb": "create_library", "stage": "starting",
                          "stageLabel": "accepted, chain starting", "startedAt": _t0.time(),
                          "libraryPath": lbr_p or None, "percent": 0}
        open(_rp0, "w", encoding="utf-8").write(_js0.dumps(_runs0))
    except Exception:
        pass
    _th.Thread(target=_chain, daemon=True).start()
    return {"success": True, "started": True, "runId": run_id,
            "statusVerb": "fusion_prove_result",
            "shape": "A (.lbr direct)" if (lbr_p and not (sym_p or mod_p)) else "B (KiCad sources)",
            "_hint": ("The chain runs in the background (it outlives the relay window by "
                      "design). Poll fusion_prove_result every ~15s: `stage` narrates "
                      "(converting -> binding 3D -> proving), and on stage:'done' the "
                      "`prove` block carries the four evidence frames and the verdict. "
                      "ONE call, ONE consent, ONE poll verb.")}


COMMAND_HANDLERS["create_library"] = _handle_create_library


def _handle_install_library_bundle(fusion_info: dict, args: dict) -> dict:
    """COMPOSITE (#68, Colby's routing call): Cloud Library bundle -> installed, bound, proven
    library in ONE call, the Fusion twin of kicad_install_library_bundle. The wiki CLI's
    `library install --format fusion` and the Export button both read this one contract, so
    the import recipe has exactly one owner (this bridge) instead of living in webcontrol.js.

    Shape (settled on #68, 2026-08-28):
      in : {zipPath | lbrPath (staged on THIS machine), libraryName?, capture? (default true)}
      out: {installed, libraryPath, partsTotal, partsToBind, unbound[], partMetadata,
            fusion{...}, runId, statusVerb:"fusion_prove_result"}  - returned IMMEDIATELY.
    ASYNC by contract: a STEP bind is 3-15 minutes and cannot fit the 135s web-control
    envelope, so this returns the runId and the caller polls fusion_prove_result (measured
    progress, streamed proof frames, per-part verdicts, addin_restart_required bubbled up).
    PARTIAL SHIPS: a deviceset with no model in the bundle is a NAMED skip in `unbound[]`,
    never a silent one, and the rest still bind; partsBound < partsTotal is the signal.
    """
    import tempfile as _tf
    import library_bundle as _lb
    zip_p = (args.get("zipPath") or "").replace("\\", "/")
    lbr_in = (args.get("lbrPath") or args.get("libraryPath") or "").replace("\\", "/")
    if not zip_p and not lbr_in:
        return {"success": False, "errorCode": "no_input",
                "error": "Give me zipPath (a staged Cloud Library bundle) or lbrPath (a staged .lbr "
                         "with its STEP models beside it).",
                "_hint": "Stage with write_file / write_file_chunk first; the path must exist on "
                         "THIS machine. Then poll fusion_prove_result with the runId."}
    src = zip_p or lbr_in
    if not os.path.exists(src):
        return {"success": False, "errorCode": "not_found", "error": "Not on this machine: " + src,
                "_hint": "Stage it with write_file / write_file_chunk first, then pass that path."}
    lib_name = (args.get("libraryName") or "").strip()
    work = None
    try:
        if zip_p:
            if os.path.getsize(zip_p) > _lb.MAX_ZIP_BYTES:
                return {"success": False, "errorCode": "too_large",
                        "error": "Bundle zip is larger than the 256 MB cap"}
            work = _tf.mkdtemp(prefix="adom-lib-bundle-")
            try:
                n = _lb.safe_extract(zip_p, work)
            except Exception as e:
                return {"success": False, "errorCode": "bad_bundle",
                        "error": "Could not unpack the bundle: %s" % str(e)[:160]}
            if not n:
                return {"success": False, "errorCode": "bad_bundle", "error": "The bundle zip is empty"}
            lbr_found, models, all_lbrs = _lb.find_bundle_files(work)
            if not lbr_found:
                return {"success": False, "errorCode": "no_lbr",
                        "error": "No .lbr in the bundle",
                        "_hint": "Expected an Adom Cloud Library bundle for Fusion: <Lib>.lbr plus "
                                 "<Part>.step models (beside it or under 3dmodels/)."}
            lib_name = lib_name or os.path.splitext(os.path.basename(zip_p))[0]
        else:
            lbr_found = lbr_in
            _d = os.path.dirname(lbr_in) or "."
            _, models, all_lbrs = _lb.find_bundle_files(_d)
            lib_name = lib_name or os.path.splitext(os.path.basename(lbr_in))[0]
        if not _lb.library_name_ok(lib_name):
            return {"success": False, "errorCode": "bad_name",
                    "error": "Library name not usable: %r" % lib_name,
                    "_hint": "Letters, digits, space, dot, dash, underscore; up to 64 chars."}
        # The same offline gate every import runs: a corpse .lbr fails HERE, not after a launch.
        pf = _preflight_lbr(lbr_found) if "_preflight_lbr" in globals() else {"ok": True}
        if isinstance(pf, dict) and pf.get("ok") is False:
            return {"success": False, "errorCode": "lbr_preflight_failed",
                    "error": "The .lbr will not open in Fusion: " + "; ".join(pf.get("errors") or [])[:300],
                    "preflight": pf}
        meta = _library_part_metadata(lbr_found)
        pairing = _lb.pair_parts(lbr_found, models, meta)
        # Stable install folder: the library must outlive the temp dir the chain reads from.
        # NO SPACES anywhere in it (first live run, 2026-08-29: "Adom Libraries/<name>/part.lbr"
        # failed inside Fusion with "Filename not found" - executeTextCommand is space-
        # delimited and the add-in's quote + 8.3 fallbacks did not save it). The user's home
        # may still carry a space ("C:/Users/John Doe"); that is the add-in's problem to
        # solve, this folder must never add one.
        home = os.path.expanduser("~")
        dest = os.path.join(home, "Documents", "AdomLibraries", re.sub(r"\s+", "_", lib_name))
        # PRESERVE the previous install's 3D bindings (2026-08-31, seen live: a re-install
        # overwrites the fully-bound .lbr at run start, so cancelling mid-bind left the
        # library with Placeholder packages). Harvest the old copy's package3d urns before
        # the overwrite and re-inject them; the merge-aware injector lets the new run's
        # binds win as they land, while unrebound parts keep the 3D they already had.
        _old_bindings = []
        try:
            _dest_lbr = os.path.join(dest, os.path.basename(lbr_found))
            if os.path.isfile(_dest_lbr):
                _old_txt = open(_dest_lbr, "r", encoding="utf-8", errors="replace").read()
                for _m in re.finditer(r'<package3d name="([^"]+)"[^>]*wip_urn="([^"]+)"', _old_txt):
                    _old_bindings.append({"package": _m.group(1), "wip_urn": _m.group(2)})
        except Exception:
            _old_bindings = []
        placed = _lb.place_library(lbr_found, models, dest)
        lbr_path = placed["lbrPath"]
        if _old_bindings:
            try:
                _txt = open(lbr_path, "r", encoding="utf-8", errors="replace").read()
                _txt = _inject_package3d_bindings(_txt, _old_bindings)
                open(lbr_path, "w", encoding="utf-8").write(_txt)
                print("[Fusion Bridge] carried %d prior 3D binding(s) into the fresh install"
                      % len(_old_bindings))
            except Exception as _e:
                print("[Fusion Bridge] could not carry prior bindings: %s" % str(_e)[:120])
        # ONE LIBRARY PER PART FOR THE MINT (John, 2026-08-29). Fusion's Package3D generator
        # loads the FIRST package of the open library, so minting every part against the
        # merged file put chips two and three on part one's footprint. Each part is minted
        # on its own single-deviceset library; the urns are bound back into the merged file.
        # orient:false - the wiki publishes STEPs with the correct origin and orientation;
        # no bounding-box guess (rotation or translation) is ever right for a lopsided chip.
        _split_dir = _tf.mkdtemp(prefix="adom-lib-parts-")
        _split = _lb.split_per_part(lbr_path, _split_dir)
        parts = []
        for p in pairing["parts"]:
            _ds = p.get("deviceset") or p["package"]
            # per-variant key first (deviceset::package), then the deviceset's primary file
            _sp = (_split.get(_ds + "::" + (p.get("package") or "")) if p.get("package") else None) \
                  or _split.get(_ds) or {}
            if not _sp.get("lbrPath") or not _sp.get("package"):
                pairing["unbound"].append({"package": _ds, "why": "could not split this deviceset "
                                           "into its own library (no package on its devices?)"})
                continue
            parts.append({"deviceset": _ds, "package": _sp["package"], "lbrPath": _sp["lbrPath"],
                          "modelPath": placed["models"].get(p["modelPath"], p["modelPath"]).replace("\\", "/"),
                          "orient": False})
        report = _part_metadata_report(lbr_path)
        plan = _handle_plan_library_import(fusion_info, {"parts": parts})
        out = {"success": True, "installed": True, "libraryName": lib_name, "libraryPath": lbr_path,
               "installDir": dest.replace("\\", "/"),
               "partsTotal": len(meta.get("devicesets") or []),
               "partsToBind": len(parts),
               "parts": [{"deviceset": p["deviceset"], "package": p["package"], "modelPath": p["modelPath"],
                          "mintedOn": p["lbrPath"]} for p in parts],
               "mint": "each part minted on its own single-deviceset library so the generator loads "
                       "that part's footprint; STEP placed as published (orient:false, origin trusted)",
               "unbound": pairing["unbound"], "modelsUnused": [os.path.basename(m) for m in pairing["modelsUnused"]],
               # Devicesets that SHARE an already paired package (symbol variants, value clones)
               # are not unbound: the package3d is per package and the bind pass writes the urn
               # into every device on it. Reported so a caller can tell "4 variants on one
               # footprint" from "4 parts with no model" (2026-08-29, CH340G-LR/SQ/LRC/CHIP).
               "shared": pairing.get("shared") or [],
               "partMetadata": report, "fusion": plan.get("fusion"), "plan": plan.get("plan"),
               "estimatedSec": plan.get("estimatedSec"),
               "otherLbrsInBundle": [os.path.basename(x) for x in all_lbrs[1:]]}
        if args.get("capture") is False:
            out.update(started=False, statusVerb=None,
                       _hint="capture:false - the library is placed at libraryPath and nothing was "
                             "opened in Fusion. Call fusion_create_library {parts} later to bind and prove.")
            return out
        if not parts:
            out.update(started=False, errorCode="nothing_to_bind",
                       _hint="Installed, but no deviceset could be paired with a STEP, so there is "
                             "nothing to bind or prove. unbound[] names each one. Add models named "
                             "like the deviceset or its package and call again.")
            return out
        # NO `name` here (second live run, 2026-08-29): create_library treats `name` as
        # "prove ONLY this deviceset", so passing the library name made prove hunt for a
        # symbol called ray-dac80504 and score DAC80504's real frames not_proven. The chain
        # proves EVERY deviceset when name is absent, which is what a bundle wants.
        chain = _handle_create_library(fusion_info, {"libraryPath": lbr_path, "parts": parts,
                                                     # scaffolding only: deleted once the bind is done
                                                     "_cleanupAfterBind": [_split_dir],
                                                     # the end state the user is after: ONE library in
                                                     # Fusion's Library Manager, not a file on disk
                                                     "_publishToCloud": {"name": lib_name,
                                                                         "folder": "Adom Libraries",
                                                                         # retire older same-named
                                                                         # copies only when asked
                                                                         "replaceExisting": bool(args.get("replaceExisting"))}})
        out.update({k: chain.get(k) for k in ("started", "attached", "runId", "statusVerb", "stage")})
        if not chain.get("success"):
            out.update(success=False, error=chain.get("error"), errorCode=chain.get("errorCode"))
        out["_hint"] = ("Installed to libraryPath. The bind+prove chain is running in the background "
                        "(async by contract - a STEP bind cannot fit a relay window). Poll "
                        "fusion_prove_result {runId} every ~15s: stage narrates, percent is measured, "
                        "evidence[] streams four proof frames per part, provedParts[] is per-part, and "
                        "an addin_restart_required payload bubbles up if Fusion must restart (the "
                        "caller owns that ask). partsBound < partsTotal means named skips in unbound[].")
        return out
    finally:
        try:
            if work:
                shutil.rmtree(work, ignore_errors=True)
        except Exception:
            pass


COMMAND_HANDLERS["install_library_bundle"] = _handle_install_library_bundle
COMMAND_HANDLERS["demo_control"] = lambda info, args: _handle_demo_control(args)



def _ledger_entries_since(t0: float, limit: int = 12) -> list:
    """The background-action ledger entries with epoch >= t0 (newest last, capped)."""
    import pathlib as _pl
    p = _pl.Path(os.path.expanduser("~")) / ".adom" / "fusion-bridge" / "actions.jsonl"
    if not p.exists():
        return []
    out = []
    try:
        lines = p.read_text(encoding="utf-8", errors="replace").splitlines()[-400:]
    except Exception:
        return []
    for line in lines:
        try:
            e = json.loads(line)
        except Exception:
            continue
        if float(e.get("epoch") or 0) >= t0 - 1:
            out.append(e)
    return out[-limit:]

def _handle_action_ledger(args: dict) -> dict:
    """READ-ONLY view of the background-action ledger (#52 item 1: 'a caller on a relay or
    in a container is blind to it'). Answers "who touched my host app" without filesystem
    access. args: {limit (default 50, max 500), sinceSeconds, action (substring filter)}."""
    import pathlib as _pl
    p = _pl.Path(os.path.expanduser("~")) / ".adom" / "fusion-bridge" / "actions.jsonl"
    if not p.exists():
        return {"success": True, "entries": [], "count": 0,
                "_hint": "Ledger empty: the bridge has taken no background actions on this "
                         "box since 1.9.177 (or none since the file was last cleared)."}
    limit = max(1, min(int(args.get("limit") or 50), 500))
    since = args.get("sinceSeconds")
    want = str(args.get("action") or "").lower()
    import time as _t
    cutoff = (_t.time() - float(since)) if since else None
    out = []
    try:
        for line in p.read_text(encoding="utf-8", errors="replace").splitlines():
            try:
                e = json.loads(line)
            except Exception:
                continue
            if cutoff and float(e.get("epoch") or 0) < cutoff:
                continue
            if want and want not in str(e.get("action") or "").lower():
                continue
            out.append(e)
    except Exception as e:
        return {"success": False, "error": "ledger unreadable: %s" % e}
    return {"success": True, "entries": out[-limit:], "count": len(out),
            "_hint": ("Every background action the bridge takes on the host app - update "
                      "clicks and deferrals, seat takeovers, dialog dismissals, doc closes, "
                      "hang-gate kills, WM_CLOSEs (with window class+size) - appended live, "
                      "survives respawns. Filter: {action:'wm_close', sinceSeconds:3600}.")}


COMMAND_HANDLERS["action_ledger"] = lambda info, args: _handle_action_ledger(args or {})



# ── Design rules by fab (roadmap Part 2 item 4) ─────────────────────────────────────────
_DESIGN_RULES_PAGE = "https://wiki.adom.inc/api/v1/pages/adom/pcb-design-rules/files"


def _design_rules_index(max_age_s: int = 3600) -> dict:
    """The wiki's rules/index.json (vendor, process, layerCount -> files), cached an hour in
    the bridge state dir so a rule change is a wiki publish, never a bridge release."""
    import time as _t, urllib.request as _ur
    cache = os.path.join(os.path.expanduser("~"), ".adom", "fusion-bridge", "design-rules-index.json")
    try:
        if os.path.isfile(cache) and _t.time() - os.path.getmtime(cache) < max_age_s:
            with open(cache, "r", encoding="utf-8") as fh:
                return json.load(fh)
    except Exception:
        pass
    req = _ur.Request(_DESIGN_RULES_PAGE + "/rules/index.json", headers={"User-Agent": "adom-fusion-bridge/" + str(BRIDGE_VERSION)})
    with _ur.urlopen(req, timeout=20) as r:
        idx = json.loads(r.read().decode("utf-8"))
    try:
        os.makedirs(os.path.dirname(cache), exist_ok=True)
        with open(cache, "w", encoding="utf-8") as fh:
            json.dump(idx, fh)
    except Exception:
        pass
    return idx


def _design_rules_pick(idx: dict, fab: str, process: str, layers: int):
    """The index row for (fab, process, layers). fab matches vendorSlug or a loose alias
    (jlcpcb, pcbway, 3rdpartyfab, oshpark...); process defaults to the vendor's standard tier."""
    want = re.sub(r"[^a-z0-9]", "", (fab or "").lower())
    rows = [v for v in idx.get("vendors") or [] if re.sub(r"[^a-z0-9]", "", v.get("vendorSlug", "").lower()) == want
            or want and want in re.sub(r"[^a-z0-9]", "", v.get("vendorSlug", "").lower())]
    if not rows:
        return None, sorted({v.get("vendorSlug") for v in idx.get("vendors") or []})
    procs = sorted({v.get("process") for v in rows})
    proc = (process or "").lower() or ("standard" if "standard" in procs else procs[0])
    hit = [v for v in rows if v.get("process") == proc and int(v.get("layerCount") or 0) == int(layers)]
    if not hit:
        return None, {"processes": procs, "layerCounts": sorted({int(v.get("layerCount") or 0) for v in rows if v.get("process") == proc})}
    return hit[0], None


def _handle_set_design_rules(fusion_info: dict, args: dict) -> dict:
    """fusion_set_design_rules {fab, process?, layers, druPath?, action}: load a FAB's design
    rules into the open board. The rule packs live on the wiki page adom/pcb-design-rules
    (one JSON record per vendor/process/layer count, generated .edru per row, every number
    citing the vendor's own capabilities page); the bridge fetches the .edru, stages it on
    the box, and the add-in runs `drc load <path>`. No fab (or fab:"3rdpartyfab" with the page
    unreachable) keeps the bundled 3rd party fab behaviour. action:"list" answers with the
    vendors, processes and layer counts on the page so the AI can offer a real choice.

    AI hint: the pack is per (vendor, process, layers). "jlcpcb" alone means its standard
    tier; a user on JLCPCB's advanced/HDI service needs process:"advanced" where the page
    has it. Always confirm afterwards with fusion_alerts (an empty list) - a bad .edru shows
    as an in-canvas toast, never a dialog.
    """
    a = dict(args or {})
    action = (a.get("action") or "apply").lower()
    fab = (a.get("fab") or "").strip()
    dru_path = (a.get("druPath") or "").replace("\\", "/")
    if action == "list":
        try:
            idx = _design_rules_index()
        except Exception as e:
            return {"success": False, "errorCode": "design_rules_page_unreachable", "error": str(e)[:200],
                    "_hint": "The wiki page adom/pcb-design-rules did not answer; the bundled 3rd party fab rules still apply with no fab argument."}
        rows = [{"fab": v.get("vendorSlug"), "vendor": v.get("vendor"), "process": v.get("process"),
                 "layers": v.get("layerCount"), "available": bool(v.get("filesGenerated"))} for v in idx.get("vendors") or []]
        return {"success": True, "rules": rows, "count": len(rows), "page": "https://wiki.adom.inc/adom/pcb-design-rules",
                "_hint": "Pick a row and call fusion_set_design_rules {fab, process, layers}. Layers auto-detects when omitted (2 or 4 from the open board)."}
    if action in ("show", "export") or (not fab and not dru_path):
        return _proxy_to_addin("set_design_rules", a, timeout=90)   # the bundled 3rd party fab path, unchanged
    if not dru_path:
        layers = str(a.get("layers") or "auto")
        if layers == "auto":
            det = _proxy_to_addin("detect_layers", {}, timeout=90) or {}
            layers = str(((det.get("data") or {}).get("layerCount")) or det.get("layerCount") or 2)
        try:
            idx = _design_rules_index()
        except Exception as e:
            return {"success": False, "errorCode": "design_rules_page_unreachable", "error": str(e)[:200],
                    "_hint": "Could not fetch the rule packs from wiki.adom.inc; pass druPath to a local .edru, or omit fab for the bundled 3rd party fab rules."}
        row, alt = _design_rules_pick(idx, fab, a.get("process") or "", int(layers))
        if not row:
            return {"success": False, "errorCode": "design_rules_not_found", "fab": fab, "layers": int(layers),
                    "available": alt, "_hint": "No pack for that fab/process/layer count; `available` lists what the page has. fusion_set_design_rules {action:'list'} shows every row."}
        if not row.get("filesGenerated"):
            return {"success": False, "errorCode": "design_rules_not_generated", "fab": fab, "process": row.get("process"), "layers": int(layers),
                    "_hint": "The page has this vendor's record but no generated .edru yet (the vendor publishes no usable limits for it). Pick another process or fab."}
        rel = (row.get("files") or {}).get("fusionEdru")
        import urllib.request as _ur
        try:
            req = _ur.Request(_DESIGN_RULES_PAGE + "/" + rel, headers={"User-Agent": "adom-fusion-bridge/" + str(BRIDGE_VERSION)})
            with _ur.urlopen(req, timeout=30) as r:
                edru = r.read()
        except Exception as e:
            return {"success": False, "errorCode": "design_rules_download_failed", "error": str(e)[:200], "file": rel}
        if b"<designrules" not in edru:
            return {"success": False, "errorCode": "design_rules_invalid", "file": rel,
                    "error": "The downloaded file is not an EAGLE design rules document."}
        stage = os.path.join(os.environ.get("TEMP") or os.path.expanduser("~"), "adom-fusion", "design-rules")
        os.makedirs(stage, exist_ok=True)
        dru_path = os.path.join(stage, os.path.basename(rel)).replace("\\", "/")
        with open(dru_path, "wb") as fh:
            fh.write(edru)
        a["_pack"] = {"fab": row.get("vendorSlug"), "vendor": row.get("vendor"), "process": row.get("process"),
                      "layers": row.get("layerCount"), "file": rel, "record": (row.get("files") or {}).get("record")}
    a["druPath"] = dru_path
    a["action"] = "apply"
    res = _proxy_to_addin("set_design_rules", a, timeout=90) or {}
    if isinstance(res, dict):
        if a.get("_pack"):
            res.setdefault("data", {})
            if isinstance(res["data"], dict):
                res["data"]["pack"] = a["_pack"]
                res["data"]["source"] = "https://wiki.adom.inc/adom/pcb-design-rules"
        res["_hint"] = ((res.get("_hint") or "") + " Confirm with fusion_alerts {sinceSec:60}: an empty list means Fusion accepted the rules; a toast means it refused the file.").strip()
    return res


COMMAND_HANDLERS["set_design_rules"] = _handle_set_design_rules


# ── Readiness executors (Phase 3 of the Rust port) ───────────────────────────────────────
# The Rust exe computes readiness itself and asks THIS process to perform the two side
# effects that still live here (UIA + screenshot verification against Fusion's dialogs).
def _handle_resolve_seat_dialog(fusion_info: dict, args: dict) -> dict:
    """Detect + resolve the seat/licensing modal in the background (max 3 clicks per call,
    screenshot-verified). Idempotent: nothing up -> {clicks: 0}."""
    try:
        r = _resolve_seat_dialog(max_clicks=int(args.get("maxClicks") or 3)) or {}
    except Exception as e:
        return {"success": False, "error": str(e)[:200]}
    still = False
    try:
        still = find_licensing_dialog() is not None
    except Exception:
        pass
    return {"success": True, "clicks": int(r.get("clicks", 0) or 0), "resolved": not still, "detail": r,
            "_hint": "Readiness executor: the seat dialog is clicked Continue (Suspend pre-selected) and verified gone by screenshot; poll fusion_readiness."}


def _handle_apply_fusion_update(fusion_info: dict, args: dict) -> dict:
    """Click Fusion's update offer in the background when auto-update is on (never the
    exit-class buttons while a host op is active). Returns {applied, button, why}."""
    try:
        r = _apply_fusion_update_silently() or {}
    except Exception as e:
        return {"success": False, "error": str(e)[:200]}
    return {"success": True, **r, "offered": bool(_LAST_UPDATE_OFFER.get("offered")),
            "autoUpdateEnabled": _auto_update_enabled()}


COMMAND_HANDLERS["resolve_seat_dialog"] = _handle_resolve_seat_dialog
COMMAND_HANDLERS["apply_fusion_update"] = _handle_apply_fusion_update


# ── Chain executors (Phase 3 of the Rust port): the exe orchestrates create_library /
# install_library_bundle itself and asks THIS process only for the routines that still
# carry Fusion-driving lessons (document hygiene, the restart gate, the cloud publish).
def _handle_chain_docs_open(fusion_info: dict, args: dict) -> dict:
    """The names of every document open in Fusion right now (the chain's `before` snapshot)."""
    return {"success": True, "documents": sorted(_open_doc_names())}


def _handle_chain_close_docs_since(fusion_info: dict, args: dict) -> dict:
    """Close every document that was NOT in `before` (bridge scratch by construction), keeping
    `keep`. Returns the names closed; never touches a document that pre-dates the chain."""
    before = set(str(n) for n in (args.get("before") or []))
    keep = set(str(k).lower() for k in (args.get("keep") or []))
    try:
        closed = _close_docs_opened_since(before, keep=keep)
    except Exception as e:
        return {"success": False, "error": str(e)[:200], "closed": []}
    return {"success": True, "closed": closed, "deferred": list(_SWEEP_LAST_DEFERRED)}


def _handle_chain_close_bridge_doc(fusion_info: dict, args: dict) -> dict:
    """Close a BRIDGE-owned document deterministically (no Save dialog survives), verified."""
    name = str(args.get("name") or "").strip()
    if not name:
        return {"success": False, "error": "name required"}
    _note_bridge_doc(name)
    r = _close_bridge_doc_verified(name, timeout_s=int(args.get("timeoutSec") or 30)) or {}
    return {"success": True, **r}


def _handle_chain_sweep_orphans(fusion_info: dict, args: dict) -> dict:
    """Close scratch documents a chain that died with a bridge respawn left open (ledgered)."""
    try:
        return {"success": True, **(_sweep_orphan_scratch_docs() or {})}
    except Exception as e:
        return {"success": False, "error": str(e)[:200]}


def _handle_chain_addin_restart_info(fusion_info: dict, args: dict) -> dict:
    """The restart-gate facts for a running Fusion whose add-in is silent: which unsaved
    documents are the USER'S, whether an automatic restart is allowed, and (when asked) the
    ab toast that asks the human. The exe runs the wait loop itself."""
    try:
        info = _addin_restart_info(fire_toast=bool(args.get("fireToast")))
    except Exception as e:
        return {"success": False, "error": str(e)[:200]}
    mine = info.get("userUnsaved")
    if mine is None:
        mine = _user_unsaved_docs(info.get("unsavedDocumentNames") or [])
    info["userUnsaved"] = mine
    info["holdActive"] = bool(_user_restart_hold_active())
    return {"success": True, **info}


def _handle_chain_publish_library(fusion_info: dict, args: dict) -> dict:
    """Save the bound library into the user's cloud Library Manager folder."""
    p = (args.get("libraryPath") or "").replace("\\", "/")
    if not p:
        return {"success": False, "error": "libraryPath required"}
    try:
        r = _publish_library_to_cloud(p, args.get("name") or "", args.get("folder") or "Adom Libraries",
                                      replace_existing=bool(args.get("replaceExisting")))
    except Exception as e:
        return {"success": False, "saved": False, "error": str(e)[:200]}
    return {"success": True, **(r or {})}


def _handle_chain_taskbar_progress(fusion_info: dict, args: dict) -> dict:
    """The honest progress bar on Fusion's taskbar button (pct null clears it)."""
    pct = args.get("pct")
    try:
        _taskbar_progress(None if pct is None else float(pct))
    except Exception:
        pass
    return {"success": True}


def _handle_chain_part_metadata_report(fusion_info: dict, args: dict) -> dict:
    """The short, user-facing part-metadata report an import carries up the chain."""
    p = (args.get("libraryPath") or "").replace("\\", "/")
    if not p:
        return {"success": False, "error": "libraryPath required"}
    return {"success": True, "report": _part_metadata_report(p)}


for _name, _fn in (("chain_docs_open", _handle_chain_docs_open),
                   ("chain_close_docs_since", _handle_chain_close_docs_since),
                   ("chain_close_bridge_doc", _handle_chain_close_bridge_doc),
                   ("chain_sweep_orphans", _handle_chain_sweep_orphans),
                   ("chain_addin_restart_info", _handle_chain_addin_restart_info),
                   ("chain_publish_library", _handle_chain_publish_library),
                   ("chain_taskbar_progress", _handle_chain_taskbar_progress),
                   ("chain_part_metadata_report", _handle_chain_part_metadata_report)):
    COMMAND_HANDLERS[_name] = _fn


def _handle_chain_addin_gate(fusion_info: dict, args: dict) -> dict:
    """Chain EXECUTOR (Rust port): the add-in preamble for one command; ok:true means send it."""
    cmd = str(args.get("command") or "")
    pre = _addin_preamble(cmd, dict(args.get("args") or {}))
    if pre is None:
        return {"success": True, "ok": True}
    return {"success": True, "ok": False, "refusal": pre}


def _handle_chain_diagnose_addin_timeout(fusion_info: dict, args: dict) -> dict:
    """Chain EXECUTOR (Rust port): _diagnose_addin_timeout for a command the exe sent itself."""
    cmd = str(args.get("command") or "")
    orig = dict(args.get("original") or {})
    return {"success": True, "diagnosis": _diagnose_addin_timeout(cmd, orig)}


def _handle_chain_dialog_array(fusion_info: dict, args: dict) -> dict:
    """Chain EXECUTOR (Rust port): the post-op dialog array a mutating verb carries (null when none)."""
    try:
        arr = _capture_dialog_array()
    except Exception:
        arr = None
    return {"success": True, "dialogArray": arr}


def _handle_chain_verb_indicator(fusion_info: dict, args: dict) -> dict:
    """Chain EXECUTOR (Rust port): the taskbar activity indicator around one verb (start / stop)."""
    import timings as _tm
    cmd = str(args.get("command") or "")
    action = str(args.get("action") or "start")
    if action == "stop":
        try:
            from handlers.taskbar_overlay import end_activity_progress
            end_activity_progress(cmd, failed=bool(args.get("failed")))
        except Exception:
            pass
        return {"success": True}
    mode = _activity_mode()
    est, conf = _tm.verb_estimate(cmd)
    if mode in ("progress", "both"):
        try:
            from handlers.taskbar_overlay import start_activity_progress
            start_activity_progress(cmd, est, conf)
        except Exception:
            pass
    if mode in ("flash", "both"):
        try:
            _ab_call("desktop_taskbar", {"titleContains": "Autodesk Fusion", "flash": "until_focused"}, timeout=10)
        except Exception:
            pass
    return {"success": True, "mode": mode, "estimatedSec": est, "confidence": conf}


for _name, _fn in (("chain_addin_gate", _handle_chain_addin_gate),
                   ("chain_diagnose_addin_timeout", _handle_chain_diagnose_addin_timeout),
                   ("chain_dialog_array", _handle_chain_dialog_array),
                   ("chain_verb_indicator", _handle_chain_verb_indicator)):
    COMMAND_HANDLERS[_name] = _fn


_SENTINELS = {}


def _handle_chain_pre_verb(fusion_info: dict, args: dict) -> dict:
    """Chain EXECUTOR (Rust port): everything the Python dispatcher does BEFORE an add-in verb
    runs: the open-document snapshot, the window etiquette (foreground reason or background
    envelope), and for a long-running command the pre-dismissal of blocking dialogs."""
    cmd = str(args.get("command") or "")
    a = dict(args.get("args") or {})
    docs_before = sorted(_snapshot_open_docs()) if cmd in _WINDOW_RAISING else None
    a2, ran_bg, refusal = _window_etiquette_pre(cmd, a)
    if cmd in LONG_RUNNING_COMMANDS:
        try:
            info = get_fusion_window_info()
            for d in (info.get("dialogs") or []):
                try:
                    close_window(d.get("hwnd"))
                except Exception:
                    pass
            if info.get("dialogs"):
                import time as _t
                _t.sleep(0.5)
        except Exception:
            pass
    return {"success": True, "args": a2, "docsBefore": docs_before, "ranBackgrounded": bool(ran_bg),
            "refusal": refusal, "fg": bool(a.get("foreground"))}


def _handle_chain_post_verb(fusion_info: dict, args: dict) -> dict:
    """Chain EXECUTOR (Rust port): everything the Python dispatcher does AFTER an add-in verb
    ran (documents opened, bridge docs, surface shot, chokepoint sweep, etiquette note, the
    recent-actions ledger on a failed slow verb)."""
    cmd = str(args.get("command") or "")
    a = dict(args.get("args") or {})
    res = args.get("result")
    if not isinstance(res, dict):
        res = {"success": False, "error": "no result"}
    docs_before = args.get("docsBefore")
    docs_before = set(docs_before) if isinstance(docs_before, list) else None
    since = args.get("since")
    if isinstance(since, (int, float)) and res.get("success") is False and (cmd in LONG_RUNNING_COMMANDS or cmd in _WINDOW_RAISING):
        try:
            acts = _ledger_entries_since(float(since))
            if acts:
                res.setdefault("recentActions", acts)
                res.setdefault("_ledgerHint", "recentActions = every background action the bridge took on the host app while this verb ran (newest last); fusion_action_ledger {sinceSeconds} for more.")
        except Exception:
            pass
    res = _window_etiquette_post(cmd, a, res, docs_before, bool(args.get("fg")))
    return {"success": True, "result": res}


def _handle_chain_sentinel(fusion_info: dict, args: dict) -> dict:
    """Chain EXECUTOR (Rust port): the save-prompt sentinel around a doc-switching add-in call
    the exe sends itself: action start {seconds} -> id; action stop {id} -> soft-stop (12s grace)."""
    import uuid as _u
    action = str(args.get("action") or "start")
    if action == "stop":
        sid = str(args.get("id") or "")
        st = _SENTINELS.pop(sid, None)
        if st is not None:
            try:
                st.soft_stop(12.0)
            except Exception:
                pass
        return {"success": True, "stopped": st is not None}
    try:
        st = _SavePromptSentinel(float(args.get("seconds") or 40) + 10)
        st.__enter__()
    except Exception as e:
        return {"success": False, "error": str(e)}
    sid = _u.uuid4().hex[:10]
    _SENTINELS[sid] = st
    return {"success": True, "id": sid}


for _name, _fn in (("chain_pre_verb", _handle_chain_pre_verb),
                   ("chain_post_verb", _handle_chain_post_verb),
                   ("chain_sentinel", _handle_chain_sentinel)):
    COMMAND_HANDLERS[_name] = _fn


def _handle_chain_dismiss_recovery(fusion_info: dict, args: dict) -> dict:
    """Chain EXECUTOR (Rust port): relocate crash-recovery files and dismiss the recovery dialogs."""
    from handlers.dismiss_recovery import relocate_recovery_files, dismiss_recovery_dialog
    reloc = {"moved": 0, "dest": None}
    if args.get("relocate", True):
        try:
            reloc = relocate_recovery_files()
        except Exception:
            pass
    dismissed = False
    if args.get("dismiss", True):
        try:
            dismissed = bool(dismiss_recovery_dialog(max_wait=float(args.get("maxWait") or 5.0)))
        except Exception:
            dismissed = False
    return {"success": True, "dismissed": dismissed, "relocated": {"moved": reloc.get("moved", 0), "dest": reloc.get("dest")}}


def _handle_chain_hung_check(fusion_info: dict, args: dict) -> dict:
    """Chain EXECUTOR (Rust port): the kill-grade hang verdict (_confirmed_hung), with the exe's
    own long-op marker honoured the way the Python one is."""
    import time as _t
    since = args.get("longOpSince")
    if isinstance(since, (int, float)) and since > 0 and not _LONG_OP.get("since"):
        _LONG_OP["since"] = float(since)
        _LONG_OP["name"] = str(args.get("longOpName") or "exe long op")
        try:
            return {"success": True, "verdict": _confirmed_hung()}
        finally:
            _LONG_OP["since"] = 0.0
            _LONG_OP["name"] = None
    return {"success": True, "verdict": _confirmed_hung()}


def _handle_chain_clean_incomplete_webdeploy(fusion_info: dict, args: dict) -> dict:
    """Chain EXECUTOR (Rust port): remove partial webdeploy dirs after a failed streamed launch."""
    from fusion_detect import _clean_incomplete_webdeploy
    try:
        removed = _clean_incomplete_webdeploy()
    except Exception as e:
        return {"success": False, "error": str(e)[:200]}
    return {"success": True, "removed": removed}


for _name, _fn in (("chain_dismiss_recovery", _handle_chain_dismiss_recovery),
                   ("chain_hung_check", _handle_chain_hung_check),
                   ("chain_clean_incomplete_webdeploy", _handle_chain_clean_incomplete_webdeploy)):
    COMMAND_HANDLERS[_name] = _fn

def _handle_reset_workspace(args: dict) -> dict:
    """fusion_reset_workspace - bring Fusion back to a clean slate between runs (#59).

    Closes named documents AND the Untitled scratch the library/bind flow leaves behind,
    which close_all_documents alone could not do. Unmodified scratch always; MODIFIED
    Untitled docs only with force:true, because those can hold real unsaved work.
    """
    force = bool(args.get("force", False))
    r = dispatch_command("close_all_documents",
                         {"save": False, "closeUntitled": True, "force": force})
    swept = _sweep_untitled_scratch()
    left = []
    try:
        st = _proxy_to_addin("document_info", {}, timeout=8)
        left = [d.get("name") for d in ((st.get("data") or {}).get("openDocuments") or [])]
    except Exception:
        pass
    return {"success": True, "closedByAddin": (r.get("data") or {}).get("closed"),
            "skipped": (r.get("data") or {}).get("skipped"),
            "untitledScratchClosed": len(swept), "remaining": left,
            "_hint": ("Fusion always keeps ONE blank document alive, so a single Untitled "
                      "remaining is correct, not a failure. A modified Untitled is left "
                      "untouched unless you pass force:true - it may hold unsaved work.")}


COMMAND_HANDLERS["reset_workspace"] = lambda info, args: _handle_reset_workspace(args or {})



def _handle_check_3d_thumbnail(fusion_info: dict, args: dict) -> dict:
    """Is the Library Manager's 3D THUMBNAIL actually rendered? Deterministic, no pixels.

    The thumbnail is rendered by Autodesk's cloud against the bound f3d and exposed as a
    relationship on the file's version - a free /data/v1 read. Fusion takes its time after a
    bind, so `waitSeconds` polls (default 0 = single check). This is the check John named as
    "an area you always screw up": a bound urn with NO rendered thumbnail shows the user a
    grey slab in the Library Manager while everything else reports success.

    args: {libraryPath | urn, fileName?, projectId?, folderId?, waitSeconds?}
    """
    import time as _t
    import aps as _aps
    urn = (args.get("urn") or "").strip()
    lbr = (args.get("libraryPath") or "").replace("\\", "/")
    file_name = args.get("fileName") or ""
    if not urn and lbr:
        try:
            import xml.etree.ElementTree as _ET
            root = _ET.parse(lbr).getroot()
            for p3 in root.iter("package3d"):
                # wip_urn, not urn: this bridge writes <package3d urn="" wip_urn="...">
                # (the same one-attribute miss that broke prove's 3D frame, issue #46 -
                # and unfixed HERE it made tja1051t/type-c report thumbnail not_found
                # with the file sitting right there, issue #47 failure mode 4)
                urn = (p3.get("urn") or p3.get("wip_urn") or "").strip() or urn
                file_name = file_name or (p3.get("name") or "")
        except Exception:
            pass
    if not urn and not file_name:
        return {"success": False, "errorCode": "no_3d_reference",
                "error": "No urn/fileName given and the .lbr carries no package3d urn.",
                "_hint": "Bind first (fusion_bind_library_3d), then check the thumbnail."}
    project_id, folder_id = _discover_upload_target(args)
    if not folder_id:
        return {"success": False, "errorCode": "no_work_folder",
                "error": "Cannot resolve the upload folder to look in (APS signed in?)."}
    deadline = _t.time() + float(args.get("waitSeconds") or 0)
    while True:
        st = _aps.thumbnail_state(project_id, folder_id, wip_urn=urn, file_name=file_name)
        if st.get("state") == "ready" or _t.time() >= deadline:
            break
        _t.sleep(10)
    ready = st.get("state") == "ready"
    return {"success": ready, "thumbnail": st, "urn": urn or None,
            "_hint": ("Thumbnail is RENDERED in the cloud - the Library Manager will show the "
                      "real chip." if ready else
                      "Not rendered yet (state=%s). Fusion uploads + renders asynchronously "
                      "after a bind - re-call with waitSeconds:180 to poll, and treat a "
                      "still-pending thumbnail as NOT proven." % st.get("state"))}


COMMAND_HANDLERS["check_3d_thumbnail"] = _handle_check_3d_thumbnail


def _window_etiquette_post(command: str, args: dict, res, _docs_before, _fg: bool):
    """The window etiquette AFTER a verb ran: register the documents it opened, note bridge
    docs, the surface shot, the save-prompt / dialog chokepoint sweep, and the etiquette
    annotation. Shared by dispatch_command and the exe's chain_post_verb executor."""
    if _docs_before is not None:
        _register_docs_we_opened(_docs_before)
    # ── capture:true - ONE call = ONE consent = surface opened AND proven (issue #30) ──
    # ab measured NINE human consent prompts for a single Symbol button under an
    # "Allow ONCE" web-control grant: the show verb, then eight state-verb polls while the
    # page guessed at when the surface had settled. Page-side settle polling is
    # structurally wrong when every cross-origin call costs a consent, and the bridge is
    # the only party that actually KNOWS when the surface is ready - it opened it. So the
    # bridge owns settle + proof, and returns the frame inline.
    try:
        _cmd = command[7:] if command.startswith("fusion_") else command
        # REGISTER WHAT WE OPEN, OR WE LEAVE OUR MESS ON THE USER'S SCREEN (John, live
        # 2026-08-20: a Save prompt for our own staged demo part sat on his desktop). The
        # save-prompt sentinel only auto-discards documents it knows the BRIDGE created,
        # which is the right instinct - we must never discard the user's work - but the
        # show verbs stage a part and never registered it, so the sentinel correctly
        # refused to touch a dialog that was 100% our doing. Registering the target of
        # every show verb closes that gap without widening what we are willing to discard.
        if _cmd in _SHOW_SURFACE_VERBS and isinstance(args, dict):
            for _k in ("name", "deviceset", "libraryPath", "fileName", "query"):
                if args.get(_k):
                    _note_bridge_doc(str(args[_k]))
        if (_cmd in _SHOW_SURFACE_VERBS and bool((args or {}).get("capture"))
                and isinstance(res, dict) and res.get("success") is not False):
            res["shot"] = _capture_surface_shot(_cmd, args or {})
    except Exception as _e:
        try:
            res["shot"] = {"title": command, "image": None, "error": str(_e)[:160]}
        except Exception:
            pass
    try:
        # FULL-PASS GUARANTEE (John, 2026-08-21: a Save-As sat on screen again and blocked
        # every web-control call behind it). Instead of auditing each of ~20 paths and
        # missing the 21st, ONE structural rule at the dispatch chokepoint - the same move
        # that fixed Create3DPackage: after ANY window-raising verb, if a Save/Save-As
        # prompt is standing and attributable to a bridge document, it is answered HERE,
        # before the verb returns. A modal we caused must never outlive the call.
        _cmd_sw = command[7:] if command.startswith("fusion_") else command
        if _cmd_sw in _WINDOW_RAISING:
            try:
                _sw = _dismiss_save_prompt_if_bridge_doc()
                if _sw.get("handled") and isinstance(res, dict):
                    res["savePromptCleared"] = _sw.get("why")
            except Exception:
                pass
            # Same chokepoint, the other standing modals (John caught each on screen,
            # 2026-08-21). All are answered with their work-preserving choice - a raw
            # electron_run EDIT from ANY caller can raise the Create prompt, and no
            # verb-level pre-check reaches those, so the answer lives HERE:
            #   upload_in_progress -> No  (cancels the close; the upload survives)
            #   create_confirm     -> No  (never fabricate an empty artifact)
            #   lbr open Error     -> OK  (dismiss; its orphan Untitled design gets
            #                              closed by the doc janitor)
            try:
                for _d in (classify_blocking_dialogs() or []):
                    _cat = _d.get("category")
                    if _cat == "upload_in_progress":
                        _unwrap(_ab_call("desktop_ui_click",
                                         {"hwnd": _d.get("hwnd"), "name": "No"}, timeout=12))
                        if isinstance(res, dict):
                            res["uploadCloseDeclined"] = True
                        print("[Fusion Bridge] answered the Hub upload-close confirm with "
                              "No (work-preserving; close retried after jobs drain)")
                    elif _cat == "create_confirm":
                        _unwrap(_ab_call("desktop_ui_click",
                                         {"hwnd": _d.get("hwnd"), "name": "No"}, timeout=12))
                        if isinstance(res, dict):
                            res["createPromptDeclined"] = True
                        print("[Fusion Bridge] declined a standing Create-new prompt at the "
                              "chokepoint (raised by a raw EDIT; Yes would fabricate an "
                              "empty artifact)")
                    elif _cat in ("launch_error", "lbr_open_error"):
                        _unwrap(_ab_call("desktop_ui_click",
                                         {"hwnd": _d.get("hwnd"), "name": "OK"}, timeout=12))
                        print("[Fusion Bridge] dismissed a standing %s dialog at the "
                              "chokepoint" % _cat)
            except Exception:
                pass
    except Exception:
        pass
    try:
        cmd = command[7:] if command.startswith("fusion_") else command
        # windowEtiquette is an ENUM, not a boolean in disguise (SDK canonicalization,
        # page issue #34): a caller branches on it, so BOTH outcomes must be stated. We
        # only ever annotated the background case, which left a runner unable to tell
        # "came forward, with a reason" from "this verb does not report etiquette".
        if cmd in _WINDOW_RAISING and _fg and isinstance(res, dict):
            res.setdefault("windowEtiquette", "foreground")
        if cmd in _WINDOW_RAISING and not _fg and isinstance(res, dict):
            res.setdefault("windowEtiquette", "background")
            note = ("Window opened/changed in the BACKGROUND - the user's focus was NOT touched; "
                    "do NOT foreground anything to compensate. Render results via fusion_state. "
                    "If the USER asks to watch, re-call with {\"foreground\": true, "
                    "\"foregroundReason\": \"<why, shown to the user as a 3s caption>\"}.")
            h = res.get("_hint")
            if not (isinstance(h, str) and "focus was NOT touched" in h):
                res["_hint"] = (h + " " + note) if isinstance(h, str) else note
    except Exception:
        pass
    return res


def dispatch_command(command: str, args: dict, caller_identity: dict = None) -> dict:
    """Thin wrapper: run the verb, then guarantee the windowEtiquette annotation lands on
    EVERY backgrounded window-raising result - the inner dispatcher's closure only covered
    direct-handler and add-in paths, so orchestrated verbs (show_*, make_test_project, the
    tour, ...) returned unannotated (the calling AI could not know no focus was taken).
    Idempotent with the inner annotation (the note is only appended once)."""
    _fg = bool((args or {}).get("foreground"))
    # WHAT WE OPEN IS OURS TO CLEAN UP (see _register_docs_we_opened). Snapshot before,
    # diff after, so every document our verbs create is attributable to us - and only ours.
    _cmd_short = command[7:] if command.startswith("fusion_") else command
    _docs_before = _snapshot_open_docs() if _cmd_short in _WINDOW_RAISING else None
    res = _time_and_indicate(_cmd_short, args,
                             lambda: _dispatch_command_inner(command, args, caller_identity))
    res = _window_etiquette_post(command, args, res, _docs_before, _fg)
    return res


def _addin_preamble(command: str, args: dict):
    """The gate every add-in-dependent verb passes BEFORE its command is sent: Fusion
    installed / running (auto-start unless noAutoStart), the hang gate, and the add-in
    restart contract. Returns the refusal dict, or None when the command may proceed.
    Extracted verbatim from _dispatch_command_inner so the Rust exe can call it as the
    chain_addin_gate executor while it owns the add-in pass-through (Phase 4)."""
    import time as _time
    # Issue #326: Fusion may be installed under a different Windows user profile than the current
    # one, so file-path detection fails even though Fusion is running. Check if it's actually
    # running (via tasklist) as a reliable fallback. If running, it's installed somewhere.
    fusion_running = _is_fusion_running()
    if not fusion_info.get("installed") and not fusion_running:
        return {
            "success": False,
            "error": "Fusion 360 is not installed on this machine.",
            "errorCode": "fusion_not_installed",
            "_hint": "Fusion 360 isn't installed. Do NOT tell the user to install it themselves - OFFER to install it FOR them and do it on a yes: the fusion-onboarding skill silent-installs Fusion + drives the Autodesk sign-in.",
        }
    if not fusion_running:
        # SECOND OPINION BEFORE A LAUNCH (2026-08-29): a launch on a false "not running"
        # starts a SECOND Fusion ("Multiple instances are not supported" in the user's
        # face, mid-import). A probe can miss while Fusion pegs the CPU; the add-in
        # answering on its port, or a Fusion main window, proves it is up. Re-ask twice
        # over ~3s and only launch when every signal still says closed.
        import time as _sop
        from fusion_detect import _fusion_main_hwnd_quick as _hwnd_quick
        for _ in range(2):
            _sop.sleep(1.5)
            if _is_fusion_running() or _probe_addin(timeout=1.5) or _hwnd_quick():
                fusion_running = True
                break
    if not fusion_running:
        # AUTO-OPEN, DO NOT REFUSE (page issue #64, 2026-08-26).
        # CLAUDE.md's automation doctrine: "If Fusion is closed, launch it ... Never
        # return an error the code could have fixed." fusion_create_library already
        # treats launch as a STAGE of the chain, but every INDIVIDUAL verb still
        # refused - so a user who ran a library import with Fusion merely closed
        # watched four steps fail in a row ("3D bind did NOT report a binding: Fusion
        # 360 is installed but not running", then symbol, footprint and 3D the same).
        # They cannot tell "the app was shut" from "this product is broken". Fusion
        # not being up is a STEP, never a refusal.
        _auto = None
        if not args.get("noAutoStart"):
            import time as _as
            _t0 = _as.time()
            print("[Fusion Bridge] %s needs Fusion; it is closed - launching it "
                  "(page issue #64)" % command)
            try:
                # Same handler fusion_start uses (server.py:676), with waitForAddin so
                # we do not hand back a Fusion whose add-in is not answering yet.
                _auto = _handle_launch(fusion_info, {"waitForAddin": True}) or {}
            except Exception as _e:
                _auto = {"success": False, "error": "%s: %s" % (type(_e).__name__, _e)}
            _took = round(_as.time() - _t0, 1)
            _bridge_action_ledger("fusion_auto_started",
                                  {"forCommand": command, "ok": bool(_auto.get("success")),
                                   "tookSec": _took})
            if _auto.get("success") and _is_fusion_running():
                fusion_running = True
                print("[Fusion Bridge] Fusion up in %ss - continuing %s" % (_took, command))
            else:
                return {
                    "success": False,
                    "errorCode": "fusion_autostart_failed",
                    "error": "Fusion was closed so the bridge tried to open it for you, "
                             "and the launch did not come up: %s"
                             % str(_auto.get("error") or "no error reported")[:300],
                    "autoStartAttempted": True, "autoStartSec": _took,
                    "_hint": "This is a launch failure, NOT a bad request - the command "
                             "itself was never run. Check fusion_readiness (needsSignin "
                             "true means Autodesk sign-in is blocking the launch, which "
                             "is a human step). Pass noAutoStart:true to get the old "
                             "refuse-immediately behaviour.",
                }
    if not fusion_running:
        return {
            "success": False,
            "error": "Fusion 360 is installed but not running.",
            "errorCode": "fusion_not_running",
            "_hint": "Call fusion_start to start Fusion 360, then wait for the add-in to become ready (fusion_start blocks until ready). Then retry this command.",
        }
    # Fusion is running — check if add-in is responsive (short timeout).
    # But first, if the add-in is busy with a long command, skip the
    # responsiveness check and fall through to the busy gate below.
    addin_busy = _check_addin_status(timeout=3.0)
    if addin_busy and addin_busy.get("busy"):
        pass  # Fall through to busy gate
    elif not wait_for_addin(timeout=5):
        # Hung HOST vs down ADD-IN (page issue #38) - opposite remedies, so say which.
        # KILL-GRADE evidence only (_confirmed_hung): a busy main thread starves these
        # very probes and stops pumping messages, so one failed snapshot looks exactly
        # like a hang. Three healthy Fusions were taskkilled mid-work by this gate
        # before the persistence rule (2026-08-21).
        _h = _confirmed_hung()
        if _h.get("hung"):
            _ok = False
            _bridge_action_ledger("hang_gate_kill",
                                  {"heldSeconds": _h.get("heldSeconds"),
                                   "signals": _h.get("signals")})
            try:
                dispatch_command("kill", {})
                _time.sleep(6)
                dispatch_command("start", {})
                _ok = True
            except Exception:
                pass
            return {
                "success": False,
                "error": "Fusion 360 itself is NOT RESPONDING (hung host process, held "
                         "%ss with the add-in refusing a 20s probe); the add-in cannot "
                         "answer while it is." % _h.get("heldSeconds"),
                "errorCode": "fusion_host_hung",
                "hungSignals": _h.get("signals"),
                "restartAttempted": _ok,
                "_hint": ("Do NOT hunt for a blocking dialog - there is not one. Nothing "
                          "reaches Fusion over IPC while the host is hung. "
                          + ("The bridge force-restarted it; poll fusion_readiness."
                             if _ok else "Use fusion_kill then fusion_start.")),
            }
        if _h.get("phase") in ("suspect", "busy-not-hung"):
            return {
                "success": False,
                "error": "Fusion's main thread is not answering right now - most likely "
                         "WORKING on something heavy (a STEP import or save starves the "
                         "add-in's probes), not hung.",
                "errorCode": "fusion_addin_busy",
                "hostResponding": _h.get("phase") == "busy-not-hung",
                "hungHeldSeconds": _h.get("heldSeconds"),
                "_hint": ("WAIT and re-poll; do not restart anything. The bridge only "
                          "declares a real hang after 180s of continuous silence "
                          "including a 20s probe - if that happens it restarts Fusion "
                          "itself and says so."),
            }
        # TELL THE HUMAN WHY, AND OFFER THE FIX (the Adonis incident, John 2026-08-27).
        # This used to return a bare "add-in not responding" and a hint aimed at an AI
        # ("fix it YOURSELF - never ask the user"). That advice is right for an agent
        # driving headlessly and WRONG when a person is sitting in front of Fusion with
        # documents open: a brand-new user drove this from the wiki, every verb failed,
        # and nothing ever told him the product was fine and just needed a relaunch.
        # The payload now carries what a web-control caller needs to ASK him, and a
        # toast fires so he is told even if the caller drops it on the floor.
        _ri = _addin_restart_info()
        if _ri.get("autoRestart"):
            _started = _begin_auto_addin_restart()
            return {
                "success": False,
                "errorCode": "addin_restarting",
                "error": ("Fusion is running without the Adom add-in and no unsaved "
                          "document belongs to the user, so the bridge is restarting "
                          "Fusion itself (documents are saved first). No toast, no ask."),
                "restartStarted": _started,
                "userUnsaved": _ri.get("userUnsaved") or [],
                "_hint": "Poll fusion_readiness; ready flips true in ~60-90s, then retry this verb.",
            }
        return {
            "success": False,
            "error": ("Fusion 360 is running but the AdomBridge add-in is not responding "
                      "- it was installed or updated while Fusion was already open, and "
                      "Fusion only loads add-ins at launch."),
            "errorCode": "addin_restart_required",
            "hostResponding": True,
            "restartRequired": True,
            "userMessage": _ri["userMessage"],
            "openDocuments": _ri.get("openDocuments"),
            "unsavedDocuments": _ri.get("unsavedDocuments"),
            "documentsKnown": _ri.get("documentsKnown"),
            "remedyVerb": "fusion_restart_for_addin",
            "toast": _ri.get("toast"),
            "_hint": ("ASK THE USER, do not silently restart their Fusion: show "
                      "userMessage, and if documentsKnown say how many documents are "
                      "open. On yes, call fusion_restart_for_addin {save:true} - it "
                      "saves open documents, restarts Fusion, waits for the add-in and "
                      "reports back. An AI driving with no human present may call it "
                      "directly. (Legacy callers: this was errorCode "
                      "fusion_addin_not_responding.)"),
        }
    return None


def _window_etiquette_pre(command: str, args: dict):
    """The window etiquette BEFORE a verb runs (foreground:true needs a reason and a 3s
    heads-up; otherwise the background envelope). Returns (args, ran_backgrounded, refusal).
    Shared by _dispatch_command_inner and the exe's chain_pre_verb executor (Phase 4)."""
    import time as _time
    _ran_backgrounded = False
    if command in _WINDOW_RAISING:
        if (args or {}).get("foreground"):
            # Foreground is allowed but never free (John, 2026-08-16): the CALLER must say
            # WHY, and the user gets that reason as an ab caption for ~3s BEFORE the steal,
            # so they know what is happening and can yield (or object). No reason = refuse
            # with a teaching hint - same pattern as every reason-gated Adom verb.
            fg_reason = str(args.get("foregroundReason") or "").strip()
            if not fg_reason:
                _note_focus_event(command, "refused")
                return args, False, {"success": False, "errorCode": "foreground_reason_required",
                        "error": "foreground:true needs foregroundReason (a short user-facing sentence).",
                        "_hint": ("Fusion verbs ALWAYS run in the background so the user is never "
                                  "interrupted mid-typing. If the user genuinely wants to WATCH, re-call "
                                  "with foreground:true AND foregroundReason:\"<why, in words the user "
                                  "will read>\" - it is shown as an ab caption for 3 seconds before the "
                                  "window comes forward, so they can yield.")}
            _note_focus_event(command, "foreground", fg_reason)
            try:
                ab_client.notify("Fusion is coming to the foreground",
                                 fg_reason + " (Requested by your AI - Fusion verbs otherwise always "
                                 "run in the background.)", level="info", reach_user=True)
                _time.sleep(3.0)   # the user's 3-second heads-up BEFORE any focus change
            except Exception:
                pass
            args = {k: v for k, v in args.items() if k not in ("foreground", "foregroundReason")}
        else:
            _sec = ADDIN_COMMAND_TIMEOUTS.get(command, 60) + 20
            begin_background_envelope(float(_sec), minimize=(command == "start"))
            _ran_backgrounded = True
    return args, _ran_backgrounded, None


def _dispatch_command_inner(command: str, args: dict, caller_identity: dict = None) -> dict:
    import time as _time  # MUST be first: a later local `import time as _time` makes the
    # name local to this whole scope, so three earlier uses raised UnboundLocalError -
    # found by tools/shadow_check.py minutes after the same class shipped as the 1.9.128
    # open_lbr regression (issue #47). The gate now runs at every release.
    """Dispatch a command to the appropriate handler.

    caller_identity: {'thread','container','reason'} from the X-Adom-Caller-* headers ab stamped on
    this request. Bound to THIS thread (issue #348) so any ab callback we make while carrying out
    the verb is attributed to the AI thread that asked for it. Pass None to leave the binding alone.
    """
    if caller_identity is not None:
        _set_caller_identity(caller_identity)
    # Strip the "fusion_" prefix if present — handlers are registered without it (e.g. "aps_signin"
    # not "fusion_aps_signin"), but the relay passes them with the prefix for CLI clarity. Issue #327.
    if command.startswith("fusion_"):
        command = command[7:]

    # Driving-badge activity clock (John, 2026-08-17): every non-passive verb marks Fusion
    # as actively driven; the janitor sheds the badge after _BADGE_IDLE_SECONDS of quiet.
    try:
        _note_driving_activity(command)
    except Exception:
        pass

    # Document guard (issue #289 items 1 AND 5, Oliver): a verb carrying `expectDocument` must not
    # run against a document the caller didn't intend. Originally only mutating verbs were guarded;
    # Oliver's BOM session proved READS need it too - a passive tab switch silently returned a
    # different assembly (68/268 vs 94/424 instances) with nothing signalling the retarget. So the
    # assertion now fires for ANY verb that passes the arg: a caller who states an expectation wants
    # it checked. Asserted BEFORE dispatch, then stripped so handlers never see the key.
    if isinstance(args, dict) and args.get("expectDocument"):
        _guard = _assert_active_document(str(args["expectDocument"]))
        if _guard is not None:
            return _guard
        args = {k: v for k, v in args.items() if k != "expectDocument"}

    # Background envelope (John 2026-08-14): ANY verb that can raise a Fusion window runs guarded,
    # so a cloud open / view switch / import NEVER steals the user's foreground mid-typing. The
    # deliberate-foreground verbs (prefs_open, mcp_enable, demo - which announce themselves with
    # captions) are excluded on purpose. Refcounted, so overlapping verbs nest cleanly.
    args, _ran_backgrounded, _refusal = _window_etiquette_pre(command, args)
    if _refusal is not None:
        return _refusal

    def _annotate_background(res):
        # Tell the CALLING AI what just happened window-wise (John, 2026-08-16): the verb
        # ran under the background envelope - no focus was taken, nothing to apologize for,
        # and the AI should not "helpfully" foreground anything afterward.
        if _ran_backgrounded and isinstance(res, dict):
            res.setdefault("windowEtiquette", "background")
            note = ("Window opened/changed in the BACKGROUND - the user's focus was NOT touched; "
                    "do NOT foreground anything to compensate. Render results via fusion_state. "
                    "If the USER asks to watch, re-call with {\"foreground\": true, "
                    "\"foregroundReason\": \"<why, shown to the user as a 3s caption>\"}.")
            h = res.get("_hint")
            res["_hint"] = (h + " " + note) if isinstance(h, str) and note not in h else (h or note)
        return res

    # Direct handlers (don't need the add-in)
    handler = COMMAND_HANDLERS.get(command)
    if handler is not None:
        return _annotate_background(handler(fusion_info, args))

    # Check if Fusion is installed and running before any add-in-dependent command.
    # We do NOT auto-launch — that causes 60s+ hangs when Fusion isn't running.
    # The user should launch Fusion themselves; we just report the status.
    # Commands that need Fusion running (add-in commands + orchestrated commands)
    _OPEN_WITH_SCREENSHOT = {"open_schematic", "open_board", "show_3d_board", "show_2d_board", "show_schematic", "import_electronics"}
    if command in ADDIN_COMMANDS or command in ("open_lbr", "save_lbr", "attach_3d_package", "make_3d_package", "build_library_3d", "capture_library_views", "cleanup_cloud_files", "generate_package", "export_optimized_glb", "board_stackup", "show_symbol", "show_footprint", "show_3d_package", "make_test_project", "bind_library_3d", "show_library", "show_project") or command in _OPEN_WITH_SCREENSHOT:
        _pre = _addin_preamble(command, args)
        if _pre is not None:
            return _pre

    # ── Busy gate: reject add-in commands immediately if a long command is running ──
    # Bridge-level commands (COMMAND_HANDLERS) already returned above — they use
    # Win32 APIs and don't touch the add-in, so they always work during a walk.
    # But add-in commands (and orchestrated commands that proxy to the add-in)
    # would pile up behind _main_thread_lock for 300s and crash the host.
    busy = _get_long_command()
    if busy and command not in LONG_RUNNING_COMMANDS and command not in BUSY_EXEMPT_COMMANDS:
        # This command would block behind the long-running one — reject immediately
        progress = _get_busy_progress()
        elapsed = round(_time.time() - busy["startedAt"], 1)
        progress_pct = None
        if progress and progress.get("foldersVisited") and progress.get("queueSize") is not None:
            total = progress["foldersVisited"] + progress["queueSize"]
            if total > 0:
                progress_pct = round(100 * progress["foldersVisited"] / total)
        return {
            "success": False,
            "error": (
                f"Fusion main thread busy — {busy['command']} has been running for {elapsed}s. "
                f"Your command '{command}' cannot execute until it finishes."
            ),
            "errorCode": "main_thread_busy",
            "busyCommand": busy["command"],
            "elapsedSeconds": elapsed,
            "progress": progress,
            "_hint": (
                f"A cloud search ({busy['command']}) is in progress"
                + (f" (~{progress_pct}% done)" if progress_pct is not None else "")
                + f", running for {elapsed}s. "
                "Do NOT retry add-in commands (get_app_state, document_info, etc.) — they will "
                "all be rejected until the search finishes. Commands that still work right now: "
                "fusion_window_info, fusion_screenshot_fusion, fusion_click_fusion, "
                "fusion_send_key, fusion_close_window. Wait for the search to complete, then retry."
            ),
        }

    # Cross-bridge case: this bridge's _long_command is None but the add-in
    # might be busy from another bridge/session. Quick non-blocking check.
    if not busy and command not in LONG_RUNNING_COMMANDS and command not in BUSY_EXEMPT_COMMANDS:
        addin_status = _check_addin_status(timeout=3.0)
        if addin_status and addin_status.get("busy"):
            elapsed = addin_status.get("elapsedSeconds", 0)
            walk = addin_status.get("walkProgress")
            busy_cmd = addin_status.get("busyCommand", "unknown")
            resp = {
                "success": False,
                "error": (
                    f"Fusion main thread busy — {busy_cmd} running for {elapsed}s "
                    f"(from another bridge/session). Your command '{command}' cannot "
                    f"execute until it finishes."
                ),
                "errorCode": "main_thread_busy",
                "busyCommand": busy_cmd,
                "elapsedSeconds": elapsed,
                "_hint": (
                    f"A long-running command ({busy_cmd}) is in progress from another session. "
                    "Do NOT retry add-in commands — they will all be rejected until it finishes. "
                    "Do NOT press Escape — the add-in is working, not stuck on a dialog. "
                    "Commands that still work: fusion_window_info, fusion_screenshot_fusion, "
                    "fusion_click_fusion, fusion_send_key, fusion_close_window."
                ),
            }
            if walk:
                resp["progress"] = walk
            return resp

    # Orchestrated multi-step commands (handled at bridge level)
    if command in _OPEN_WITH_SCREENSHOT:
        # Test-project layout (make_test_project): the "views" are two DOCUMENTS, not
        # one design's internal views. Activate the right document first, then the
        # normal flow proceeds (for 2D the board editor already IS the 2D view).
        if _TEST_PROJECT and command in ("show_schematic", "show_2d_board", "show_3d_board"):
            try:
                st = _proxy_to_addin("get_app_state", {}, timeout=10)
                active = str((st.get("data") or {}).get("activeDocument", "")).lower()
            except Exception:
                active = ""
            if _TEST_PROJECT:
                kind = "sch" if command == "show_schematic" else "brd"
                _test_project_activate(kind)
                if command in ("show_schematic", "show_2d_board"):
                    shot = _handle_screenshot_fusion({}, {})
                    return {"success": True, "view": command.replace("show_", ""),
                            "document": _TEST_PROJECT["docBase"],
                            "screenshot": shot.get("savedTo"),
                            "_hint": "Test-project layout: the %s DOCUMENT is now active (its editor IS this view)." % kind}
                # 3D board: with the board doc active, fall through to the normal
                # show_3d_board machinery which switches the board editor to 3D.
        return _handle_open_with_screenshot(fusion_info, args, command)
    if command == "open_lbr":
        # DIALOG-GUARDED LIKE EVERY NEIGHBOUR (Colby, PR #58 / page issue #65).
        # This was the ONE dispatch here returning raw while show_symbol,
        # show_footprint, show_3d_package, make_test_project and bind_library_3d all
        # ride _merge_dialog_array. So a malformed .lbr reported SUCCESS: Fusion opens
        # an empty design named like the file and puts the real failure in an OWNED
        # Error popup that a plain response never sees. Same false-pass class as #65.
        return _merge_dialog_array(_orchestrate_open_lbr(args))
    if command == "show_symbol":
        return _merge_dialog_array(_orchestrate_show_symbol(args))
    if command == "show_footprint":
        return _merge_dialog_array(_orchestrate_show_footprint(args))
    if command == "show_3d_package":
        return _merge_dialog_array(_orchestrate_show_3d_package(args))
    if command == "make_test_project":
        return _merge_dialog_array(_orchestrate_make_test_project(args))
    if command == "bind_library_3d":
        return _merge_dialog_array(_orchestrate_bind_library_3d(args))
    if command == "show_library":
        return _merge_dialog_array(_orchestrate_show_library(args))
    if command == "show_project":
        return _merge_dialog_array(_orchestrate_show_project(args))
    if command == "save_lbr":
        return _merge_dialog_array(_orchestrate_save_lbr(args))
    if command == "attach_3d_package":
        return _merge_dialog_array(_orchestrate_attach_3d_package(args))
    if command == "make_3d_package":
        return _apply_failure_dialogs(_orchestrate_make_3d_package(args))
    if command == "build_library_3d":
        return _apply_failure_dialogs(_orchestrate_build_library_3d(args))
    if command == "capture_library_views":
        return _apply_failure_dialogs(_orchestrate_capture_library_views(args))
    if command == "cleanup_cloud_files":
        return _orchestrate_cleanup_cloud_files(args)
    if command == "generate_package":
        return _apply_failure_dialogs(_orchestrate_generate_package(args))
    if command == "export_optimized_glb":
        if (args or {}).get("async"):
            return _start_glb_export_async(args)
        return _orchestrate_export_optimized_glb(args)
    if command == "fetch_optimized_glb":
        if (args or {}).get("exportId"):
            return _glb_export_async_status(args)
        return _orchestrate_fetch_optimized_glb(args)
    if command == "demo":
        if (args or {}).get("mode") == "tour":
            return _orchestrate_demo_tour(args)
        return _orchestrate_demo(args)
    if command == "signin":
        return _orchestrate_signin(args)
    if command == "signin_2fa":
        return _orchestrate_signin_2fa(args)
    if command == "board_stackup":
        return _orchestrate_board_stackup(args)

    # Add-in proxy commands
    if command in ("routing_state", "route_net", "routing_drc", "plan_route"):
        import pathlib
        source = "\n".join((pathlib.Path(__file__).parent / "rust/bridge/src" / name).read_text() for name in ("routing_script.py", "routing_planner.py"))
        script = "import json\nexec(%r)\nresult=handle(app,json.loads(%r),%r)\n" % (source, json.dumps(args), command)
        reply = _proxy_to_addin("run_modeling_script", {"script":script, "expectDocument":args.get("expectDocument")}, timeout=120)
        return reply.get("data", {}).get("result", reply)

    if command in ADDIN_COMMANDS:
        # ⛔ RAW close_document GATE (John caught the modal standing twice, 2026-08-21):
        # web control calls this verb directly, bypassing _close_bridge_doc_verified's
        # upload gate - and a close over an in-flight Hub upload raises the lose-changes
        # confirm. Same rule at this door: wait for app.hasActiveJobs (cap 45s inside
        # the relay window), then refuse honestly rather than raise a data-loss modal.
        if command == "close_document":
            import time as _tcg
            _cg_deadline = _tcg.time() + 45
            while _tcg.time() < _cg_deadline:
                try:
                    _jr = _proxy_to_addin("run_modeling_script",
                                          {"script": "import adsk.core\n"
                                                     "result={'jobs': adsk.core.Application.get().hasActiveJobs}"},
                                          timeout=10)
                    if not ((_jr.get("data") or {}).get("result") or {}).get("jobs"):
                        break
                except Exception:
                    break
                _tcg.sleep(5)
            else:
                return {"success": False, "errorCode": "upload_in_flight_close_deferred",
                        "error": "Fusion is still uploading packages to the Hub - closing now "
                                 "would raise a lose-changes confirm and risk destroying the "
                                 "upload. The close was NOT issued.",
                        "_hint": "Re-call fusion_close_document in ~30s; the bridge closes the "
                                 "moment app.hasActiveJobs drains. Nothing was lost and no "
                                 "dialog was raised."}
        addin_cmd = ADDIN_COMMAND_MAP.get(command, command)
        proxy_timeout = ADDIN_COMMAND_TIMEOUTS.get(command, 30)
        # Wrap long-running commands in set/clear so the gate knows they're active.
        # Pre-dismiss blocking dialogs: modal dialogs steal Fusion's event loop,
        # preventing fireCustomEvent from being processed. Without this, the
        # walk appears "busy" but never actually starts — _walk_progress stays
        # None indefinitely while the command sits in the event queue.
        if command in LONG_RUNNING_COMMANDS:
            try:
                info = get_fusion_window_info()
                if info.get("dialogs"):
                    for d in info["dialogs"]:
                        try:
                            # Use WM_CLOSE via PostMessage — doesn't steal foreground
                            # (unlike send_key which uses SendInput + SetForegroundWindow).
                            # WM_CLOSE is also more reliable than Escape for Qt dialogs.
                            close_window(d.get("hwnd"))
                        except Exception:
                            pass
                    import time as _time
                    _time.sleep(0.5)  # give Fusion a moment to process the dismiss
            except Exception:
                pass
            _set_long_command(command)
            try:
                return _annotate_background(_proxy_to_addin(addin_cmd, args, timeout=proxy_timeout))
            finally:
                _clear_long_command()
        result = _proxy_to_addin(addin_cmd, args, timeout=proxy_timeout)
        # After a state-changing op, surface any dialog/owned-popup the AI must analyze
        # (the Hub upload-close confirm, a save prompt, recovery, etc.) so it can't fly
        # blind. Read-only verbs are excluded to avoid per-call screenshot latency.
        if command in MUTATING_COMMANDS:
            result = _merge_dialog_array(result)
        return _annotate_background(result)

    # Unknown command — check installation/running status for helpful errors
    if not fusion_info.get("installed"):
        return {
            "success": False,
            "error": f"Fusion 360 is not installed on this machine. (command: {command})",
            "errorCode": "fusion_not_installed",
            "_hint": "Fusion 360 isn't installed. Do NOT tell the user to install it themselves - OFFER to install it FOR them and do it on a yes: the fusion-onboarding skill silent-installs Fusion + drives the Autodesk sign-in.",
        }
    if not _is_fusion_running():
        return {
            "success": False,
            "error": f"Fusion 360 is installed but not running. (command: {command})",
            "errorCode": "fusion_not_running",
            "_hint": "Call fusion_start to start Fusion 360 and wait for the add-in to be ready, then retry this command.",
        }
    return {
        "success": False,
        "error": f"Unknown command: {command}",
        "_hint": "Run `adom-bridge help` or check cli/src/commands.rs to see the list of available fusion_* commands. This command name may be misspelled or not yet implemented.",
    }


_STATUS_CACHE = {"at": 0.0, "running": False, "addin": None, "refreshing": False}


def _status_probe_cached(installed: bool, ttl: float = 5.0):
    """(running, addin) for /status without ever blocking the request thread.

    Stale by at most `ttl` seconds; a refresh runs on a helper thread when the cache
    is older than that, and never more than one at a time. While a long operation is
    declared (a library import mid-mint) the cache is served without refreshing: the
    add-in cannot answer while Fusion imports a STEP, and a stalled poll here is what
    got the bridge reaped mid-run."""
    import time as _st
    now = _st.time()
    c = _STATUS_CACHE
    if not installed:
        return False, None
    long_op = bool(_LONG_OP.get("name"))
    if (now - c["at"] > ttl) and not c["refreshing"] and not (long_op and c["at"]):
        c["refreshing"] = True

        def _refresh():
            try:
                r = _is_fusion_running()
                a = _probe_addin() if r else None
                c.update(running=r, addin=a, at=_st.time())
            except Exception:
                pass
            finally:
                c["refreshing"] = False
        threading.Thread(target=_refresh, daemon=True, name="status-probe").start()
        if not c["at"]:
            # first ever call: give the probe up to ~1.5s so the very first /status is real
            for _ in range(15):
                _st.sleep(0.1)
                if c["at"]:
                    break
    return bool(c["running"]), c["addin"]


def _build_status() -> dict:
    """Build the /status payload — the endpoint ab declares as healthEndpoint.

    MUST return HTTP 2xx whenever the server is up: ab's health check polls this
    path and a 404 (the classic manifest-vs-server mismatch) makes ab wait the
    full startup grace then report a generic "not reachable". We also self-report
    the GUI chip fields {led, summary, tooltip} — ab renders them verbatim (the
    bridge owns its color; ab owns only the unreachable→gray state).
    """
    import platform

    info = fusion_info or {}
    installed = bool(info.get("installed"))
    # NEVER BLOCK THE HEALTH POLL (2026-08-29, two ten-part imports killed by
    # bridge_restarted_mid_run, both while Fusion was importing the 3.9 MB S10B STEP
    # with the CPU pegged). The crash log showed no traceback and no clean exit for
    # either death: the process was killed from outside, i.e. ab reaped a bridge whose
    # health poll stalled. This handler used to run `tasklist` (seconds under load) and
    # an add-in HTTP probe on EVERY poll. Now the probes run at most every 5s on a
    # helper thread and the handler answers from the cache in microseconds; during a
    # declared long operation the last known state is reported as-is with busy:true.
    running, addin = _status_probe_cached(installed)
    addin_ok = bool(addin)

    if platform.system() not in ("Windows", "Darwin"):
        led, summary = "yellow", "Unsupported OS"
        tooltip = ("Bridge running, but this host is neither Windows nor macOS. Fusion 360 verbs "
                   "need a Windows or macOS host with Fusion 360 installed.")
    elif not installed:
        led, summary = "yellow", "Fusion not installed"
        tooltip = ("Bridge running, but Fusion 360 is not installed. The AI can install it "
                   "for the user (fusion-onboarding), then fusion_start.")
    elif not running:
        led, summary = "yellow", "Fusion not running"
        tooltip = "Fusion 360 is installed but not running. Call fusion_start to launch it."
    elif not addin_ok:
        led, summary = "yellow", "Add-in not connected"
        tooltip = ("Fusion 360 is running but the AdomBridge add-in isn't responding yet "
                   "(still loading; if it persists the AI restarts Fusion via fusion_stop/start).")
    else:
        led, summary = "green", "Fusion ready"
        tooltip = "Fusion 360 running, AdomBridge add-in connected. All fusion_* verbs available."

    return {
        "status": "ok",
        "busy": bool(_LONG_OP.get("name")),
        "longOp": _LONG_OP.get("name"),
        "led": led,
        "summary": summary,
        "tooltip": tooltip,
        "bridgeVersion": BRIDGE_VERSION,
        "fusion": {**info, "running": running},
        "addin": addin,
    }


class FusionBridgeHandler(BaseHTTPRequestHandler):
    """HTTP request handler for the Fusion 360 bridge server."""

    def do_GET(self):
        # /status is the manifest's healthEndpoint; /health is kept as a
        # back-compat alias (older callers + the add-in-probe code path). Both
        # return the same 2xx payload — new chip fields are purely additive.
        if self.path in ("/status", "/health"):
            self._respond(200, _build_status())
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

        # Who is asking (issue #348). ab stamps these on every request it dispatches to a bridge.
        caller_identity = {
            "thread": self.headers.get("X-Adom-Caller-Thread", ""),
            "container": self.headers.get("X-Adom-Caller-Container", ""),
            "reason": self.headers.get("X-Adom-Caller-Reason", ""),
        }

        print(f"[Fusion Bridge] Command: {command} | Args: {json.dumps(args)}")

        try:
            result = dispatch_command(command, args, caller_identity=caller_identity)
            self._respond(200, result)
        except Exception as e:
            print(f"[Fusion Bridge] ERROR: {e}")
            traceback.print_exc()
            self._respond(500, {
                "success": False,
                "error": f"Internal error: {e}",
            })
        finally:
            # Unbind before this thread is reused or retired. Without this, a later call made on
            # our OWN behalf could inherit this AI thread's name and misattribute it.
            _set_caller_identity(None)

    def _respond(self, status: int, data: dict):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(data).encode("utf-8"))

    def log_message(self, format, *args):
        print(f"[Fusion Bridge] {args[0]} {args[1]} {args[2]}")


def _prune_bridge_logs(max_mb: int = 8, keep_tail_mb: int = 2) -> dict:
    """LOG JANITOR (John, 2026-07-22: "do you have a janitor to clean up your log?").

    ab captures this bridge's stdout/stderr into ~/.adom/bridge-logs/fusion360.log. Nothing
    rotated it, so it grew unbounded - fine today (it is small), a slow leak on a long-lived box.
    On every bridge start we truncate our OWN logs to the most recent keep_tail_mb once they pass
    max_mb, keeping the tail because that is where a crash traceback lives.

    Only touches files this bridge owns (fusion360*). Never raises.
    """
    import glob
    out = {"checked": 0, "pruned": []}
    try:
        d = os.path.join(os.path.expanduser("~"), ".adom", "bridge-logs")
        for path in glob.glob(os.path.join(d, "fusion360*.log")):
            out["checked"] += 1
            try:
                sz = os.path.getsize(path)
                if sz <= max_mb * 1024 * 1024:
                    continue
                with open(path, "rb") as f:
                    f.seek(-keep_tail_mb * 1024 * 1024, os.SEEK_END)
                    tail = f.read()
                with open(path, "wb") as f:
                    f.write(b"[adom-bridge log janitor] truncated %d MB -> tail %d MB\n"
                            % (sz // (1024 * 1024), keep_tail_mb))
                    f.write(tail)
                out["pruned"].append({"file": os.path.basename(path), "wasMB": sz // (1024 * 1024)})
            except Exception:
                continue
        if out["pruned"]:
            print("[Fusion Bridge] log janitor: %s" % out["pruned"])
    except Exception:
        pass
    return out


def main():
    global fusion_info

    # Line-buffer stdout/stderr. ab spawns the bridge console-less and captures
    # our stdout+stderr into ~/.adom/bridge-logs/fusion360.log. Python BLOCK-buffers
    # stdout when it's not a TTY, so without this the buffer never flushes while
    # serve_forever() runs → the log stays 0 bytes (and a spawn-crash traceback
    # would be lost). Line buffering flushes every print()/traceback on newline.
    try:
        sys.stdout.reconfigure(line_buffering=True)
        sys.stderr.reconfigure(line_buffering=True)
    except Exception:
        pass

    _prune_bridge_logs()
    _start_signin_janitor()

    port = DEFAULT_PORT
    if "--port" in sys.argv:
        idx = sys.argv.index("--port")
        if idx + 1 < len(sys.argv):
            port = int(sys.argv[idx + 1])

    # Early banner BEFORE detection — so the log proves we started even if
    # detect_fusion() is slow or wedges. (KiCad bridge prints an equivalent line.)
    print(f"[Fusion Bridge] starting - version {BRIDGE_VERSION}, port {port}, "
          f"healthEndpoint /status, pid {os.getpid()}", flush=True)

    fusion_info = detect_fusion()

    print(f"[Fusion Bridge] Fusion 360 detection result:")
    print(f"  Installed: {fusion_info.get('installed', False)}")
    if fusion_info.get("installed"):
        print(f"  Exe path:  {fusion_info.get('exe_path')}")
        print(f"  AddIns:    {fusion_info.get('addins_dir')}")
        print(f"  Add-in:    {'installed' if fusion_info.get('addin_installed') else 'not installed'}")
        print(f"  Running:   {fusion_info.get('running')}")

        # ALWAYS sync the add-in on startup - it is idempotent (_sync_directory copies
        # only CHANGED files). The old `if not addin_installed` guard meant a STALE
        # add-in was never UPDATED: an add-in fix (e.g. get_parameters, caught live
        # 2026-07-06) never reached users who already had ANY copy, because the bridge
        # only deployed when it was entirely MISSING. Always-sync fixes that. It is safe
        # while Fusion is up (locked add-in files simply skip via the per-target OSError
        # catch); the update lands on the next bridge respawn with Fusion closed
        # (fusion_stop -> bridge_install -> fusion_start).
        _n = _reconcile_orphaned_runs()
        if _n:
            print(f"[Fusion Bridge] {_n} run(s) were mid-flight when the previous bridge died - marked failed (bridge_restarted_mid_run)")
        print(f"[Fusion Bridge] Syncing AdomBridge add-in (idempotent; updates a stale copy)...")
        try:
            install_addin()
            fusion_info = detect_fusion()  # Re-detect after sync
            print(f"  Add-in:    {'installed' if fusion_info.get('addin_installed') else 'FAILED'}")
        except Exception as e:
            print(f"  Add-in sync failed: {e}")
    else:
        print(f"  WARNING: Fusion 360 not found. Some commands will fail.")
        # Detection can miss a Fusion that is nonetheless installed (another profile, an odd
        # webdeploy root). If ANY AddIns dir exists the add-in is in use there, so sync it
        # anyway: idempotent, and the only way a stale copy ever gets repaired (issue #75).
        try:
            from install_addin import TARGET_CANDIDATES as _tc
            if any(c.exists() for c in _tc):
                print(f"[Fusion Bridge] An AddIns dir exists though Fusion was not detected: syncing the add-in anyway")
                install_addin()
        except Exception as e:
            print(f"  Add-in sync (undetected Fusion) failed: {e}")

    addin_health = _probe_addin()
    if addin_health:
        print(f"  Add-in server: running on port {ADDIN_PORT}")
    else:
        print(f"  Add-in server: not running (port {ADDIN_PORT})")

    all_commands = list(COMMAND_HANDLERS.keys()) + sorted(ADDIN_COMMANDS)
    print(f"[Fusion Bridge] Available commands: {', '.join(all_commands)}")

    # ab (>=1.9.63) passes ADOM_BIND_HOST (always 127.0.0.1) to every bridge it
    # spawns. Honor it and NEVER bind 0.0.0.0/'' by default - a public bind pops a
    # Windows Firewall "allow access?" dialog, and ab's guarantee to users is no
    # firewall prompts. Default to loopback if the var is absent (e.g. local dev).
    bind_host = os.environ.get("ADOM_BIND_HOST", "127.0.0.1")
    server = ThreadingHTTPServer((bind_host, port), FusionBridgeHandler)
    server.daemon_threads = True
    print(f"[Fusion Bridge] Listening on http://{bind_host}:{port}")
    print(f"[Fusion Bridge] Health check: http://{bind_host}:{port}/status (alias /health)")
    print(f"[Fusion Bridge] Press Ctrl+C to stop.")

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\n[Fusion Bridge] Shutting down.")
        server.server_close()


# ── Geometry read-back (issue #289 item 2, Oliver) ───────────────────────────────────────────
# There was no way to ask what actually got BUILT, so every caller hand-wrote body enumeration
# inside a modeling script and parsed it back out. That read-back caught both real modelling bugs
# in Oliver's session and NEITHER was visible in a screenshot (a body at Z -167.6 instead of -83.8
# from a double-applied offset; a handle buried inside the plate it should hang below). This work
# is often matte-black parts on a dark canvas, so pixels are a weak verification channel and an AI
# driving the bridge has no other way to check its own output.
#
# Implemented BRIDGE-side on the existing run_modeling_script surface (the add-in already captures
# a script's `result`), so it needs no add-in redeploy and no Fusion restart.
#
# Units are MILLIMETRES, deliberately. Fusion's internals are centimetres, which is a standing
# source of off-by-ten errors when an AI reads these numbers back.
_INSPECT_BODIES_SCRIPT = r'''
import adsk.core, adsk.fusion

_des = adsk.fusion.Design.cast(app.activeProduct)
_WORLD = __WORLD__
_OCC = __OCC__

def _mm(v):      return round(v * 10.0, 4)      # cm  -> mm
def _mm3(v):     return round(v * 1000.0, 4)    # cm3 -> mm3
def _mm2(v):     return round(v * 100.0, 4)     # cm2 -> mm2

def _describe(body, owner):
    bb = body.boundingBox
    mn, mx = bb.minPoint, bb.maxPoint
    cyl = 0
    try:
        for f in body.faces:
            g = f.geometry
            if g and g.surfaceType == adsk.core.SurfaceTypes.CylinderSurfaceType:
                cyl += 1
    except Exception:
        cyl = -1
    def _nm(o):
        try:    return o.name
        except Exception: return None
    return {
        "name": body.name,
        "owner": owner,
        "bbox": {"x": [_mm(mn.x), _mm(mx.x)],
                 "y": [_mm(mn.y), _mm(mx.y)],
                 "z": [_mm(mn.z), _mm(mx.z)]},
        "sizeMm": [_mm(mx.x - mn.x), _mm(mx.y - mn.y), _mm(mx.z - mn.z)],
        "volumeMm3": _mm3(body.volume),
        "areaMm2": _mm2(body.area),
        "faceCount": body.faces.count,
        "cylindricalFaceCount": cyl,
        "appearance": _nm(body.appearance),
        "material": _nm(body.material),
        "isVisible": body.isVisible,
    }

rows = []
root = _des.rootComponent
for b in root.bRepBodies:
    rows.append(_describe(b, root.name))

if _WORLD:
    # Bodies fetched THROUGH an occurrence are proxies whose boundingBox is already resolved
    # into assembly space, so occurrence placement is verified without any matrix maths here.
    for occ in root.allOccurrences:
        if _OCC and _OCC.lower() not in occ.name.lower():
            continue
        try:
            for b in occ.bRepBodies:
                rows.append(_describe(b, occ.name))
        except Exception:
            pass

result = {"units": "mm", "worldSpace": bool(_WORLD),
          "documentName": app.activeDocument.name if app.activeDocument else None,
          "bodyCount": len(rows), "bodies": rows}
'''


def _handle_plan_library_import(fusion_info: dict, args: dict) -> dict:
    """PRE-FLIGHT: what a library import will do, how long, and what state Fusion is in.

    John, 2026-08-27: when the wiki asks to import a library, does the bridge pre-check
    that Fusion is installed, whether it is running or must be launched, hand back honest
    progress-bar data with informational states, and scale the estimate to how much library
    the wiki is actually sending? The facts existed (fusion_readiness) and the chain had a
    measured plan, but nothing REPORTED them before work began, so the wiki could not show
    the user anything until the import was already under way.

    This does NO work and changes nothing: it answers so the wiki can render the summary
    and the user can decide. The same stage names come back from fusion_prove_result while
    the run executes, so one renderer draws the plan and then the bar.
    """
    import time as _t
    import timings as _tmc

    parts = args.get("parts") or []
    n_parts = max(1, len(parts) or 1)
    step_paths = [p.get("modelPath") for p in parts if p.get("modelPath")] or (
        [args.get("stepPath")] if args.get("stepPath") else [])

    inst = bool(fusion_info.get("installed")) or _is_fusion_running()
    running = _is_fusion_running()
    addin_ok = _addin_round_trip_ready(tries=1) if running else False

    # The informational line John asked for, in the app's own voice.
    if not inst:
        status, note = "not_installed", ("Fusion does not look installed on this machine. "
                                         "I can install it for you - it is a silent install "
                                         "plus an Autodesk sign-in.")
    elif not running:
        status, note = "installed_not_running", ("Fusion is installed but not running. "
                                                 "I can launch it, which usually takes about "
                                                 "a minute, and then carry on.")
    elif not addin_ok:
        status, note = "addin_restart_required", ("Fusion is running but the Adom add-in is "
                                                  "not loaded - it needs a quick restart to "
                                                  "finish setup. I can save your documents "
                                                  "and do it.")
    else:
        status, note = "ready", "Fusion is running and ready - I can start straight away."

    def _est(key, fallback):
        v, c = _tmc.estimate(key, fallback)
        return round(float(v), 1), c

    steps = []
    if not inst:
        # DO NOT PROMISE AN INSTALL AND THEN LEAVE IT OUT OF THE ESTIMATE. My own test of
        # this verb caught it: the summary offered to install Fusion while the total quoted
        # only the import stages, so a first-run user would have been told ~12 minutes for
        # something that starts with a multi-gigabyte download. Fusion's own installer
        # streams the app and takes 10-30 minutes on a real connection (fusion-onboarding),
        # so it is seeded wide and flagged low-confidence rather than quietly omitted.
        v, c = _est("stage:install_fusion", 1200.0)
        steps.append({"stage": "install", "label": "install Fusion (streams the app)",
                      "estimatedSec": v, "confidence": c, "needsUserConsent": True,
                      "note": "10-30 minutes depending on connection, plus an Autodesk sign-in"})
    if inst and not running:
        v, c = _est("stage:launch", 60.0)
        steps.append({"stage": "launch", "label": "open Fusion",
                      "estimatedSec": v, "confidence": c})
    if status == "addin_restart_required":
        v, c = _est("stage:launch", 60.0)
        steps.append({"stage": "restart", "label": "restart Fusion to load the add-in",
                      "estimatedSec": v, "confidence": c, "needsUserConsent": True})
    if step_paths:
        per, c = _est("stage:bind_per_part", 60.0)
        steps.append({"stage": "bind",
                      "label": "attach 3D to %d part%s" % (n_parts, "" if n_parts == 1 else "s"),
                      "estimatedSec": round(per * n_parts, 1), "confidence": c,
                      "parts": n_parts, "perPartSec": per,
                      "note": "one STEP import per part - the slowest stage"})
    v, c = _est("stage:prove", 300.0)
    steps.append({"stage": "prove", "label": "capture 4 evidence views",
                  "estimatedSec": v, "confidence": c})

    total = round(sum(x["estimatedSec"] for x in steps), 1)
    ready_total = round(sum(x["estimatedSec"] for x in steps
                            if x["stage"] not in ("launch", "restart")), 1)
    return {
        "success": True,
        "fusion": {"installed": inst, "running": running, "addinReady": addin_ok,
                   "status": status, "message": note},
        "plan": steps,
        "partsTotal": n_parts,
        "estimatedSec": total,
        "estimatedSecIfAlreadyOpen": ready_total,
        "confidence": ("measured" if all(x["confidence"] == "measured" for x in steps)
                       else "typical"),
        "summary": ("%s This will take about %s. %s"
                    % (note, _human_secs(total),
                       ("Had Fusion already been open, about %s." % _human_secs(ready_total))
                       if ready_total != total else "")).strip(),
        "_hint": ("Pre-flight only - NOTHING was launched, imported or changed. Render "
                  "`summary` plus `plan[]` as the progress bar's steps, then call "
                  "fusion_create_library (or open_lbr + build_library_3d) and poll "
                  "fusion_prove_result, which reports the SAME stage names so one renderer "
                  "draws both. Estimates marked confidence:'measured' come from this "
                  "machine's own recorded runs; 'typical' means seeded and not yet observed "
                  "here."),
    }


def _human_secs(v: float) -> str:
    v = int(round(float(v or 0)))
    if v < 60:
        return "%ds" % v
    m, sec = divmod(v, 60)
    return "%dm %02ds" % (m, sec) if sec else "%dm" % m


_LIVE_FEED = {"thread": None, "stop": False, "tick": 0, "frames": [], "startedAt": None,
              # HONESTY (2026-08-28): tick counts ATTEMPTS. A feed that reports running:true
              # with tick climbing and buffered:0 for ten minutes was silently failing every
              # capture (except: pass). captured/noHwnd/lastError make that visible.
              "captured": 0, "noHwnd": 0, "lastError": None, "lastErrorAt": None, "lastCaptureAt": None}
_LIVE_FEED_MAX = 600      # ring buffer: ~10 min at 1 fps



def _live_feed_health(out: dict) -> dict:
    """Attach capture health to a live-feed reply. running:true + tick climbing is NOT proof
    that frames exist (2026-08-28: ten minutes of running:true, tick 199, buffered 0, every
    capture failing silently). captured/lastError let a caller tell 'live' from 'spinning'."""
    import time as _t
    f = _LIVE_FEED
    out.update(captured=f.get("captured", 0), noHwnd=f.get("noHwnd", 0),
               lastShotKeys=f.get("lastShotKeys"), lastShotDataKeys=f.get("lastShotDataKeys"),
               lastError=f.get("lastError"), lastErrorAt=f.get("lastErrorAt"),
               lastCaptureAt=f.get("lastCaptureAt"))
    tick = f.get("tick") or 0
    if tick >= 5 and not f.get("captured"):
        out["healthy"] = False
        out["_honesty"] = ("%d capture attempts, ZERO frames. This feed is spinning, not live. "
                           "lastError=%s noHwnd=%d. Do not present it as a video." %
                           (tick, f.get("lastError"), f.get("noHwnd", 0)))
    elif f.get("lastCaptureAt") and _t.time() - f["lastCaptureAt"] > 15 and out.get("running"):
        out["healthy"] = False
        out["_honesty"] = ("no frame for %.0fs although running. lastError=%s" %
                           (_t.time() - f["lastCaptureAt"], f.get("lastError")))
    else:
        out["healthy"] = bool(f.get("captured"))
    return out

def _handle_live_feed(fusion_info: dict, args: dict) -> dict:
    """A ~1 fps look INTO Fusion while the bridge drives it, as a box-side ring buffer.

    John, 2026-08-28: "i literally should be getting screenshots like every 5 seconds
    during boot up ... or maybe each second, so like a 1 fps video" - after a chain
    restarted Fusion under him and nothing ever showed him what was in it. And: a Browser
    panel popup, the main window, and a proof shot must never be confused.

    DESIGN, and why it is shaped this way. The first version (1.9.233) ran here on the
    Windows bridge and shelled out to `adom-shotlog` while reading `localSafePath` - a
    container tool and a container-side key. It reported running:true, frames:0 forever.
    The bridge can only ever produce BOX paths, so that is what it produces: each tick
    records the main-window capture and every owned popup (dialogs, Browser/tool panels,
    the Adom banner - Fusion draws these as separate windows a main capture omits) into a
    ring buffer, tagged. Something on the container side drains it with
    {action:"frames", since:<tick>}, read_file's the paths, and publishes them wherever a
    human is looking (shotlog, or the wiki if it chooses to render 1 fps). The bridge owns
    the feed; the presenter is pluggable.

    kind=main   the Fusion main window, one per tick (the video)
    kind=popup  each owned popup over it, tied to its tick
    kind=proof  NOT produced here: the import chain's evidence frames carry kind:"proof"
                and a step label, so the three are distinguishable at the source.

    args: {action: start|stop|status|frames, intervalSec? (default 1.0), maxSec? (600),
           since? (tick; frames newer than this)}
    """
    import threading as _th, time as _t
    action = str(args.get("action") or "status").lower()
    alive = bool(_LIVE_FEED["thread"] and _LIVE_FEED["thread"].is_alive())
    if action == "status":
        return _live_feed_health({"success": True, "running": alive, "tick": _LIVE_FEED["tick"],
                "buffered": len(_LIVE_FEED["frames"]), "startedAt": _LIVE_FEED["startedAt"]})
    if action == "frames":
        since = int(args.get("since") or 0)
        fr = [f for f in _LIVE_FEED["frames"] if f["tick"] > since]
        return _live_feed_health({"success": True, "running": alive, "tick": _LIVE_FEED["tick"],
                "frames": fr[-200:], "count": len(fr),
                "_hint": ("Each frame is a BOX path (this machine); read_file it from the "
                          "container and publish it. kind is main|popup; proof frames come "
                          "from fusion_prove_result evidence[] with kind:'proof'. Pass "
                          "since:<tick> to get only what you have not seen.")})
    if action == "stop":
        _LIVE_FEED["stop"] = True
        return {"success": True, "stopped": True, "tick": _LIVE_FEED["tick"]}
    if alive:
        return {"success": True, "alreadyRunning": True, "tick": _LIVE_FEED["tick"]}
    interval = max(0.5, float(args.get("intervalSec") or 1.0))
    max_sec = min(3600.0, float(args.get("maxSec") or 600.0))
    _LIVE_FEED.update(stop=False, tick=0, frames=[], startedAt=_t.time(), captured=0, noHwnd=0,
                      lastError=None, lastErrorAt=None, lastCaptureAt=None)

    def _run():
        t0 = _t.time()
        while not _LIVE_FEED["stop"] and _t.time() - t0 < max_sec:
            _LIVE_FEED["tick"] += 1
            tick = _LIVE_FEED["tick"]
            try:
                info = get_fusion_window_info() or {}
                hwnd = info.get("hwnd") or (info.get("mainWindow") or {}).get("hwnd")
                if not hwnd:
                    _LIVE_FEED["noHwnd"] += 1
                if hwnd:
                    # The in-process ab_client returns a bare image ({data:{image,format,...}}) and
                    # NO owned popups; the loopback direct API returns the full capture with
                    # screenshots[] (banner, panels, dialogs) and box paths. Prefer it here.
                    # PROBED 2026-08-29 (fusion_live_feed {action:"probe"}): the in-process
                    # ab_client returns the FULL capture (fullPath, fullWidth, ownedPopupCount,
                    # screenshots[] with every owned popup); the loopback direct API returns a
                    # bare inline image with no popups. _ab_call would unwrap ab_client's `output`
                    # string and throw the outer shape away, so call the client directly here.
                    shot = None
                    try:
                        if ab_client.available():
                            _r = ab_client.call("desktop_screenshot_window", {"hwnd": int(hwnd)}, timeout=20)
                            if isinstance(_r, dict) and (_r.get("fullPath") or _r.get("screenshots") is not None):
                                shot = _r
                    except Exception as _ce:
                        _LIVE_FEED["lastError"] = ("ab_client: %s" % _ce)[:200]
                    if not isinstance(shot, dict):
                        shot = _ab_call("desktop_screenshot_window", {"hwnd": int(hwnd)}, timeout=20) or {}
                    # The ab reply nests the capture under `data` ({data:{fullPath,...}, message}).
                    # Reading the top level gave "screenshot returned no path: ['data','message']"
                    # on every tick all session (found by the honesty fields, 2026-08-29).
                    # Owned popups (banner, panels, dialogs) may sit at the OUTER level while the
                    # image sits under `data`; keep them before unwrapping. Record the shape once
                    # so a missing popup list is diagnosable from status, not guessed.
                    _outer_pops = shot.get("screenshots")
                    _LIVE_FEED["lastShotKeys"] = sorted(list(shot.keys()))[:12]
                    if isinstance(shot.get("data"), dict) and not shot.get("fullPath"):
                        shot = shot["data"]
                        _LIVE_FEED["lastShotDataKeys"] = sorted(list(shot.keys()))[:12]
                    if _outer_pops and not shot.get("screenshots"):
                        shot = dict(shot, screenshots=_outer_pops)
                    # From inside the bridge the capture arrives INLINE ({image, format,
                    # encoding, sizeBytes}); there is no path to hand out. Write the frame to
                    # disk so the feed's contract (box paths, read_file-able) holds.
                    if shot.get("image") and not (shot.get("fullPath") or shot.get("fullPathHost")):
                        try:
                            import base64 as _b64, tempfile as _tfl
                            _dir = os.path.join(_tfl.gettempdir(), "adom-live-feed")
                            os.makedirs(_dir, exist_ok=True)
                            _ext = str(shot.get("format") or "png").lower().replace("jpeg", "jpg")
                            _fp = os.path.join(_dir, "main-%d.%s" % (tick, _ext)).replace("\\", "/")
                            _raw = shot["image"]
                            if isinstance(_raw, str):
                                _raw = _b64.b64decode(_raw.split(",")[-1])
                            open(_fp, "wb").write(_raw)
                            shot = dict(shot, fullPath=_fp)
                            # keep the ring from eating the disk: drop frames older than the buffer
                            _old = os.path.join(_dir, "main-%d.%s" % (tick - _LIVE_FEED_MAX - 5, _ext))
                            if os.path.exists(_old):
                                os.remove(_old)
                        except Exception as _we:
                            _LIVE_FEED["lastError"] = ("frame write failed: %s" % _we)[:200]
                    # BOX paths: fullPath (main) and fullPathHost / localFullPath per popup.
                    main = shot.get("fullPath") or shot.get("fullPathHost") or shot.get("path")
                    title = str(shot.get("title") or info.get("title") or "Fusion")[:60]
                    now = _t.time()
                    if not main:
                        _LIVE_FEED["lastError"] = ("screenshot returned no path: " +
                                                   str(shot.get("error") or shot.get("errorCode") or
                                                       list(shot)[:6]))[:200]
                        _LIVE_FEED["lastErrorAt"] = now
                    # WHAT THE USER SEES, NOT JUST THE MAIN BITMAP (John, 2026-08-29: "I never
                    # see the banner in the 3D document"). Fusion draws the Adom banner, its
                    # Browser and comment panels, and every dialog as OWNED POPUP windows; a
                    # main-window capture omits all of them, so the live view showed Fusion being
                    # driven with no banner although it was on screen the whole time. Each frame
                    # now carries the main window's screen rect and every popup with its rect and
                    # image, so a viewer can composite them exactly where they sit.
                    _wrect = info.get("rect") or (info.get("mainWindow") or {}).get("rect") or {}
                    if main:
                        _LIVE_FEED["captured"] += 1
                        _LIVE_FEED["lastCaptureAt"] = now
                        _LIVE_FEED["frames"].append({"tick": tick, "kind": "main", "at": now,
                                                     "path": main, "title": title, "rect": _wrect,
                                                     "popups": len(shot.get("screenshots") or [])})
                    for k, sp_ in enumerate(shot.get("screenshots") or [], 1):
                        pth = sp_.get("fullPathHost") or sp_.get("fullPath") or sp_.get("path")
                        if not pth and sp_.get("image"):
                            try:
                                import base64 as _b64p, tempfile as _tfp
                                _pd = os.path.join(_tfp.gettempdir(), "adom-live-feed")
                                os.makedirs(_pd, exist_ok=True)
                                _pext = str(sp_.get("format") or "png").lower().replace("jpeg", "jpg")
                                pth = os.path.join(_pd, "popup-%d-%d.%s" % (tick, k, _pext)).replace("\\", "/")
                                _praw = sp_["image"]
                                if isinstance(_praw, str):
                                    _praw = _b64p.b64decode(_praw.split(",")[-1])
                                open(pth, "wb").write(_praw)
                            except Exception:
                                pth = None
                        if pth:
                            _LIVE_FEED["frames"].append({"tick": tick, "kind": "popup", "at": now,
                                                         "index": k, "path": pth,
                                                         "rect": sp_.get("rect") or sp_.get("bounds") or {},
                                                         "title": str(sp_.get("title") or "dialog")[:50]})
                    if len(_LIVE_FEED["frames"]) > _LIVE_FEED_MAX:
                        del _LIVE_FEED["frames"][: len(_LIVE_FEED["frames"]) - _LIVE_FEED_MAX]
            except Exception as _e:
                _LIVE_FEED["lastError"] = ("%s: %s" % (type(_e).__name__, _e))[:200]
                _LIVE_FEED["lastErrorAt"] = _t.time()
            _t.sleep(interval)
        _LIVE_FEED["thread"] = None
    _LIVE_FEED["thread"] = _th.Thread(target=_run, daemon=True); _LIVE_FEED["thread"].start()
    return {"success": True, "started": True, "intervalSec": interval, "maxSec": max_sec,
            "_hint": ("Capturing into a box-side ring buffer. Drain it from the container with "
                      "fusion_live_feed {action:'frames', since:<lastTick>}, read_file each "
                      "path, and publish (e.g. adom-shotlog inject -c fusion-live -d "
                      "'[main #n] ...'). tools/live_feed_drain.py does exactly that.")}


# ── library.io managed libraries (John, 2026-09-01) ──────────────────────────
# Fusion's LIBRARY.IO commands live in the EAGLE-hosted library editor: they are NOT Fusion
# CommandDefinitions (all 3100 were enumerated; none match) and the ribbon is invisible to UIA,
# so the only drive is coordinates. Everything here is therefore SELF-VERIFYING: each step
# confirms the real dialog window it expects (those ARE top-level and UIA-reachable) and fails
# with the step that broke rather than clicking on blindly. See dev-skills/fusion-cloud-libraries.
# FRACTIONS of the Fusion window rect, not DIP: they need neither the DPI nor a screenshot
# coordMap (two failed attempts at reading the scale clicked at (283,51) instead of (592,129)),
# and they hold on any resolution as long as the window is MAXIMIZED. Measured on ConfRoomROG.
# Image-space anchors, in the 1400-px-wide capture ab returns: ab converts these to screen px
# itself via the capture's coordMap (the zero-math path). Re-measure only if Fusion's ribbon
# layout changes, never to "fix" a DPI problem - ab already handles DPI.
_RIBBON_IMG = {
    "manage_tab": (216, 47),
    "libraryio_group": (332, 92),
    "push_item": (340, 108),
}
_RIBBON_FRAC = {
    "manage_tab": (216 / 1400.0, 47 / 788.0),
    "libraryio_group": (332 / 1400.0, 92 / 788.0),
    "push_item": (340 / 1400.0, 108 / 788.0),
}


def _window_dpi_scale(hwnd) -> float:
    """Physical px per DIP for a window, straight from Win32 (GetDpiForWindow / 96).

    This is the piece that makes ribbon driving portable: the SAME control measured 296 DIP on
    ConfRoomROG (4K, scale 2.0 -> 592 px) and on AdomLapper (scale 1.5 -> 444 px). Reading the
    scale from a screenshot coordMap failed inside the bridge (the shape differs from the CLI's),
    and window-rect fractions do NOT port. Ask Windows instead."""
    try:
        import ctypes
        u = ctypes.windll.user32
        try:
            u.GetDpiForWindow.restype = ctypes.c_uint
            u.GetDpiForWindow.argtypes = [ctypes.wintypes.HWND]
            dpi = int(u.GetDpiForWindow(ctypes.wintypes.HWND(int(hwnd))))
            if dpi > 0:
                return dpi / 96.0
        except Exception:
            pass
        # Windows 8.1 fallback: per-monitor DPI
        try:
            sh = ctypes.windll.shcore
            mon = u.MonitorFromWindow(ctypes.wintypes.HWND(int(hwnd)), 2)  # NEAREST
            x = ctypes.c_uint(); y = ctypes.c_uint()
            if sh.GetDpiForMonitor(mon, 0, ctypes.byref(x), ctypes.byref(y)) == 0 and x.value:
                return x.value / 96.0
        except Exception:
            pass
    except Exception:
        pass
    return 1.0


def _find_key(obj, key, depth=0):
    """First value for `key` anywhere in a nested ab response (the shape varies by verb)."""
    if depth > 6:
        return None
    if isinstance(obj, dict):
        if key in obj:
            return obj[key]
        for v in obj.values():
            r = _find_key(v, key, depth + 1)
            if r is not None:
                return r
    elif isinstance(obj, list):
        for v in obj[:20]:
            r = _find_key(v, key, depth + 1)
            if r is not None:
                return r
    elif isinstance(obj, str) and obj.strip().startswith("{") and key in obj:
        try:
            return _find_key(json.loads(obj), key, depth + 1)
        except Exception:
            return None
    return None


def _dpi_scale_of(win_row: dict) -> float:
    """Physical px per DIP for the Fusion window. Read from a screenshot's coordMap (its shape
    moves around, hence the recursive lookup); falls back to the screen/……ratio, then 1.0.
    A wrong scale is why the first managed-library run clicked at (283,51) instead of (592,129)."""
    try:
        sc = _ab_call("desktop_screenshot_screen", {}, timeout=40) or {}
        v = _find_key(sc, "dpiScale")
        if v:
            return float(v)
        img = _find_key(sc, "image") or {}
        rect = _find_key(sc, "screenRect") or {}
        if img.get("w") and rect.get("w"):
            # not the DPI itself, but the capture's own downscale - better than assuming 1.0
            return max(1.0, round(float(rect["w"]) / float(img["w"]), 3))
    except Exception:
        pass
    return 1.0


def _frac_to_screen(frac_xy, info):
    """Window-rect fraction -> physical screen px."""
    rect = info.get("rect") or {}
    left = float(rect.get("left", 0)); top = float(rect.get("top", 0))
    w = float(rect.get("width") or (rect.get("right", 0) - left) or 0)
    h = float(rect.get("height") or (rect.get("bottom", 0) - top) or 0)
    return (int(left + frac_xy[0] * w), int(top + frac_xy[1] * h))


def _fusion_main_window():
    """{hwnd, rect, dpiScale} for Fusion's main window, or None."""
    try:
        r = _ab_call("desktop_list_windows", {}, timeout=20) or {}
        out = r.get("output")
        data = json.loads(out) if isinstance(out, str) else (r.get("data") or {})
        for w in ((data.get("data") or data).get("windows") or []):
            t = str(w.get("title") or "")
            if "Autodesk Fusion" in t and w.get("kind") == "normal":
                rect = w.get("rect") or {}
                dpi = _dpi_scale_of(w)
                return {"hwnd": w.get("hwnd"), "rect": rect, "dpiScale": dpi, "title": t}
    except Exception:
        pass
    return None


def _wait_for_dialog(title_substr: str, timeout_s: float = 20.0):
    """Wait for a REAL top-level Fusion dialog whose title contains title_substr."""
    import time as _t
    deadline = _t.time() + timeout_s
    while _t.time() < deadline:
        try:
            d = _capture_dialog_array(settle=0.2) or {}
            for x in d.get("dialogs") or []:
                if title_substr.lower() in str(x.get("title") or "").lower():
                    return x
        except Exception:
            pass
        _t.sleep(1.0)
    return None


# ── Signing the user into Autodesk's WEB surfaces (John, 2026-09-01) ─────────
# Fusion's desktop app is signed in, but library.io, the hub portal (adom.autodesk360.com) and
# Autodesk Account are WEB properties with their own session, and pup starts with none. Every
# one of them offers "Continue with Google", and Adom already owns the user's Google identity,
# so ONE stored Google credential unlocks all of them for that user - no per-site passwords.
# The password itself never passes through this bridge or the AI: the user stores it once with
# ab's credential_set (OS keychain), and pup fills it in the background.
_AUTODESK_SIGNIN_HOST = "signin.autodesk.com"


# The LIBRARY.IO ribbon in DIP, measured once on ConfRoomROG and portable by construction
# (see _window_dpi_scale). The menu items sit under the group in the order Fusion lists them.
# DIP measured FROM THE CLIENT ORIGIN (ClientToScreen), which is what _ribbon_click adds to.
# Derived from coordinates proven by hand on ConfRoomROG (scale 2.0, client origin y=45):
# MANAGE 592,129 -> (296,42); group 910,252 -> (455,103); menu items step 41 px = 20.5 DIP.
_LIBIO_DIP = {
    "manage_tab": (296, 42),
    "group": (455, 103),
    "open_icon": (352, 80),   # OPEN icon - its default action is Open Library Manager
}

# The LIBRARY.IO dropdown, read off the screen 2026-09-01. It has these FOUR items in this
# order, and the order does NOT change with link state (an unlinked library still shows Pull
# and Unlink, greyed):
#     0 Push changes to managed library on library.io
#     1 Pull changes from managed library on library.io
#     2 Link to managed library on library.io
#     3 Unlink from managed library on library.io
_LIBIO_MENU_ITEMS = ["push", "pull", "link", "unlink"]


def _popup_menu_rect(timeout_s=5.0):
    """Locate the open LIBRARY.IO dropdown and return its rect.

    The menu is NOT part of Fusion's main window - it is a separate always-on-top Qt window
    (className Qt683QWindow, title "Fusion360"), which is why a window capture of Fusion shows
    no menu and a capture of the popup itself comes back black. So we never try to READ it; we
    MEASURE it with desktop_list_windows and derive each item's centre from the rect. That is
    what replaced the hardcoded per-item DIP offsets, which silently pointed at the DESIGN
    ribbon whenever a library was not actually in front.

    Fusion's docked panels share the title "Fusion360", so filter on the exact class and a
    menu-sized rect, and take the topmost (lowest z).
    """
    import time as _t
    deadline = _t.time() + timeout_s
    while _t.time() < deadline:
        try:
            resp = _ab_call("desktop_list_windows", {}, timeout=25)
            wins = _find_key(resp, "windows") or []
            cands = []
            for w in wins:
                r = w.get("rect") or {}
                if str(w.get("className") or "") != "Qt683QWindow":
                    continue
                if str(w.get("title") or "") != "Fusion360":
                    continue
                if not (200 <= int(r.get("width") or 0) <= 1400):
                    continue
                if not (60 <= int(r.get("height") or 0) <= 700):
                    continue
                cands.append((int(w.get("z") or 99), w, r))
            if cands:
                cands.sort(key=lambda c: c[0])
                _z, w, r = cands[0]
                return {"hwnd": w.get("hwnd"), "left": int(r["left"]), "top": int(r["top"]),
                        "width": int(r["width"]), "height": int(r["height"])}
        except Exception:
            pass
        _t.sleep(0.4)
    return None


def _wait_for_top_window(title_substr, timeout_s=25.0):
    """Wait for a top-level window whose title contains title_substr, via desktop_list_windows.
    Fusion's own dialogs (Select Managed Library, Library Manager) are REAL windows and are
    fully UIA-readable, unlike its ribbon and its menus."""
    import time as _t
    want = title_substr.lower()
    deadline = _t.time() + timeout_s
    while _t.time() < deadline:
        try:
            resp = _ab_call("desktop_list_windows", {}, timeout=25)
            for w in (_find_key(resp, "windows") or []):
                if want in str(w.get("title") or "").lower():
                    return w
        except Exception:
            pass
        _t.sleep(0.8)
    return None


def _wait_for_top_window_gone(title_substr, timeout_s=15.0):
    """True once no top-level window matching title_substr remains.

    Not the inverse of _wait_for_top_window: that one returns the instant it SEES a match, so
    using it to prove a dialog closed reports a stale hit as "still open". The window list lags
    a beat behind a dialog dismissal, so poll until the entry is actually absent."""
    import time as _t
    want = title_substr.lower()
    deadline = _t.time() + timeout_s
    while _t.time() < deadline:
        try:
            resp = _ab_call("desktop_list_windows", {}, timeout=25)
            wins = _find_key(resp, "windows")
            if wins is not None and not any(want in str(w.get("title") or "").lower()
                                            for w in wins):
                return True
        except Exception:
            pass
        _t.sleep(1.0)
    return False


_PICKER_COLUMNS = ["name", "type", "version", "managedFolder", "description"]


def _picker_rows(hwnd):
    """Read the Select Managed Library table into records.

    Its rows are UIA TreeItems under one QTreeWidget and stream in row-major order, five cells
    per row. Only REALIZED rows are exposed (Qt virtualizes the list), so this returns what is
    on screen, not necessarily the whole catalogue."""
    try:
        resp = _ab_call("desktop_ui_tree", {"hwnd": hwnd, "maxDepth": 14}, timeout=60)
    except Exception as e:
        return [], str(e)
    cells = []

    def walk(n):
        if isinstance(n, dict):
            if n.get("role") == "TreeItem" and n.get("name"):
                r = n.get("rect") or {}
                cells.append({"text": str(n["name"]), "x": r.get("centerX"), "y": r.get("centerY")})
            for v in n.values():
                if isinstance(v, (dict, list)):
                    walk(v)
        elif isinstance(n, list):
            for i in n:
                walk(i)

    walk(resp)
    rows = []
    for i in range(0, len(cells) - (len(_PICKER_COLUMNS) - 1), len(_PICKER_COLUMNS)):
        chunk = cells[i:i + len(_PICKER_COLUMNS)]
        row = {k: c["text"] for k, c in zip(_PICKER_COLUMNS, chunk)}
        row["_at"] = [chunk[0]["x"], chunk[0]["y"]]
        rows.append(row)
    return rows, None


def _fusion_hwnd_rect():
    """(hwnd, rect, scale) for Fusion's main window, or None."""
    win = _fusion_main_window()
    if not win or not win.get("hwnd"):
        return None
    win["dpiScale"] = _window_dpi_scale(win["hwnd"])
    return win


def _client_origin(hwnd):
    """Screen px of the window's CLIENT (0,0). Fusion draws its own title bar inside the client
    area, and a MAXIMIZED window's rect overhangs the screen by the border width (left/top were
    -13 here), so using rect.left/top put every ribbon click 13 px off. ClientToScreen is the
    honest origin."""
    try:
        import ctypes
        class _PT(ctypes.Structure):
            _fields_ = [("x", ctypes.c_long), ("y", ctypes.c_long)]
        pt = _PT(0, 0)
        if ctypes.windll.user32.ClientToScreen(ctypes.wintypes.HWND(int(hwnd)), ctypes.byref(pt)):
            return (int(pt.x), int(pt.y))
    except Exception:
        pass
    return (0, 0)


def _ribbon_click(win, key, why, popup=False):
    """Click a LIBRARY.IO ribbon target. Re-foregrounds Fusion in the SAME call (John,
    2026-09-01: "you always have to re-foreground the window the moment you are going to click
    anything cuz the user is constantly alt-tabbing away"), except for a popup item, where
    refocusing would dismiss the menu - that one goes through the raw mouse verb."""
    scale = float(win.get("dpiScale") or 1.0)
    ox, oy = _client_origin(win.get("hwnd"))
    x = int(ox + _LIBIO_DIP[key][0] * scale)
    y = int(oy + _LIBIO_DIP[key][1] * scale)
    if popup:
        _ab_call("desktop_mouse_click", {"x": x, "y": y, "reason": why}, timeout=25)
    else:
        _ab_call("desktop_click", {"hwnd": win["hwnd"], "x": x, "y": y, "cursor": False,
                                   "reason": why}, timeout=25)
    return {"step": key, "at": [x, y], "scale": scale}


def _libio_menu(win, item, why, steps):
    """MANAGE tab -> LIBRARY.IO group -> one menu item, by NAME not by guessed offset.

    Returns None on success, or a typed error dict when the dropdown never opened - which is
    the honest answer for "a library was not really in front", the failure that used to show up
    as a mystery `picker_missing` after the clicks had quietly driven the Design ribbon.
    """
    import time as _t
    if item not in _LIBIO_MENU_ITEMS:
        return {"success": False, "errorCode": "bad_menu_item",
                "error": "%r is not a LIBRARY.IO menu item; expected one of %s"
                         % (item, ", ".join(_LIBIO_MENU_ITEMS))}
    steps.append(_ribbon_click(win, "manage_tab", "open the library editor's MANAGE tab"))
    _t.sleep(1.4)
    # The group button TOGGLES. If a previous verb left the dropdown open, the first click
    # closes it and nothing appears, so try a second time before calling it missing.
    rect = None
    for attempt in (1, 2):
        steps.append(_ribbon_click(win, "group", "open the LIBRARY.IO command group"))
        _t.sleep(1.0)
        rect = _popup_menu_rect(6.0)
        if rect:
            break
        steps.append({"step": "group_retry", "attempt": attempt})
    if not rect:
        return {"success": False, "errorCode": "libio_menu_missing", "steps": steps,
                "error": "The LIBRARY.IO dropdown did not open.",
                "_hint": "Almost always means a LIBRARY is not really in front - Fusion's API "
                         "can report a library as active while an empty Untitled design holds "
                         "the visible tab. Re-open the library, confirm the ribbon reads "
                         "COMPONENT/DOCUMENT/MANAGE (not SOLID/SURFACE), then retry."}
    idx = _LIBIO_MENU_ITEMS.index(item)
    n = len(_LIBIO_MENU_ITEMS)
    x = int(rect["left"] + rect["width"] / 2)
    y = int(rect["top"] + (idx + 0.5) * rect["height"] / n)
    # A popup item goes through the raw mouse verb: re-focusing Fusion here would dismiss it.
    _ab_call("desktop_mouse_click", {"x": x, "y": y, "reason": why}, timeout=25)
    steps.append({"step": item, "at": [x, y], "menuRect": rect})
    _t.sleep(2.0)
    return None


def _library_in_front(expect_library=True):
    """(ok, doc, workspace) for the document Fusion actually has in front."""
    st = _proxy_to_addin("get_app_state", {}, timeout=10) or {}
    d = st.get("data") or {}
    doc = str(d.get("activeDocument") or ""); ws = str(d.get("activeWorkspace") or "")
    ok = ("librar" in ws.lower()) if expect_library else True
    return ok, doc, ws


def _handle_web_signin(fusion_info: dict, args: dict) -> dict:
    """Sign a pup window into an Autodesk web surface, via Google SSO by default.

    args: {url (required), sessionId (default "autodesk-web"), provider ("google"|"password"),
           username (optional, for the Google chooser)}
    Returns where it got to, and NEVER guesses a credential: a missing password or a 2FA
    challenge is reported as a typed stop with the exact one-time command the USER runs.
    """
    import time as _t
    url = (args.get("url") or "").strip()
    if not url:
        return {"success": False, "error": "url is required (e.g. https://library.io/ or "
                                           "https://<hub>.autodesk360.com/)"}
    sess = args.get("sessionId") or "autodesk-web"
    provider = (args.get("provider") or "google").lower()
    user = args.get("username") or ""
    steps = []

    def _pup(verb, a, timeout=90):
        return _ab_call("pup_" + verb, a, timeout=timeout) or {}

    def _state():
        r = _pup("eval", {"sessionId": sess, "expression":
                          "({url:location.href,body:document.body.innerText"
                          ".replace(/\\s+/g,' ').slice(0,400)})"}, timeout=60)
        return (r.get("result") or {}) if isinstance(r, dict) else {}

    _pup("open_window", {"sessionId": sess, "url": url,
                         "reason": args.get("reason") or "sign in to an Autodesk web surface"},
         timeout=120)
    _t.sleep(8)
    st = _state(); steps.append({"step": "open", "url": (st.get("url") or "")[:120]})
    body = str(st.get("body") or "")
    if _AUTODESK_SIGNIN_HOST not in str(st.get("url") or "") and "Sign in to Autodesk" not in body:
        return {"success": True, "alreadySignedIn": True, "sessionId": sess, "steps": steps,
                "_hint": "That surface was already authenticated in this pup profile."}

    if provider == "google":
        r = _pup("eval", {"sessionId": sess, "expression":
                          "(()=>{const b=[...document.querySelectorAll('button,a,div[role=button]')]"
                          ".find(e=>/Continue with Google/i.test(e.textContent));"
                          "if(!b)return {found:false};b.click();return {clicked:true};})()"}, timeout=60)
        steps.append({"step": "google-sso", "result": (r.get("result") if isinstance(r, dict) else None)})
        _t.sleep(9)
        st = _state()
        if "accounts.google.com" not in str(st.get("url") or ""):
            return {"success": False, "errorCode": "google_sso_unavailable", "steps": steps,
                    "state": (st.get("body") or "")[:200],
                    "_hint": "The page did not hand off to Google. Retry with provider:'password'."}

    # Let pup fill from its vault. It never hands the secret to this bridge or to the AI.
    lg = _pup("login", {"sessionId": sess,
                        "url": "https://accounts.google.com/" if provider == "google" else url,
                        "username": user or None,
                        "reason": "sign the user in so Adom can drive their Autodesk web surfaces"},
              timeout=120)
    steps.append({"step": "vault-login", "used": lg.get("used"), "ok": lg.get("success")})
    _t.sleep(8)
    st = _state()
    at = str(st.get("url") or "")
    body = str(st.get("body") or "")
    if "accounts.google.com" in at or _AUTODESK_SIGNIN_HOST in at:
        needs_2fa = any(k in body.lower() for k in ("verify it", "2-step", "verification",
                                                    "authenticator", "code was sent"))
        return {"success": False,
                "errorCode": "human_credential_required" if not needs_2fa else "second_factor_required",
                "sessionId": sess, "steps": steps, "at": at[:120], "page": body[:200],
                "error": ("Stopped at the sign-in page: pup has no stored credential for this "
                          "user" if not needs_2fa else
                          "Google is asking for the second factor, which only the user can supply."),
                "_hint": ("ONE-TIME setup, run by the USER so no password ever reaches the AI:\n"
                          "  adom-bridge credential_set '{\"host\":\"accounts.google.com\",\"username\":"
                          "\"<you>@<domain>\",\"password\":\"<your Google password>\"}'\n"
                          "then re-run this verb. For 2FA, approve the prompt on your phone; the "
                          "session then persists in pup's profile and later runs are silent.")}
    return {"success": True, "signedIn": True, "sessionId": sess, "steps": steps,
            "at": at[:120],
            "_hint": "Signed in. The session persists in pup's profile, so library.io, the hub "
                     "portal and Autodesk Account are now drivable for this user."}


def _managed_precheck(require_hub=True):
    """Shared precondition for every managed-library verb: a LIBRARY is in front, and (when the
    action needs it) that library lives in a Fusion Hub. Returns (err, ctx)."""
    ok, doc, ws = _library_in_front()
    if not ok:
        return ({"success": False, "errorCode": "not_a_library",
                 "error": "The document in front is %r in the %s workspace, not a library."
                          % (doc, ws or "unknown"),
                 "_hint": "fusion_open_lbr {lbrPath} first - it lands the library editor in front "
                          "and verifies it, which activate_document alone does NOT."}, None)
    win = _fusion_hwnd_rect()
    if not win:
        return ({"success": False, "errorCode": "no_fusion_window",
                 "error": "Could not locate Fusion's main window."}, None)
    ctx = {"doc": doc, "ws": ws, "win": win}
    if require_hub:
        want = doc.replace("*", "").strip()
        hub = _proxy_to_addin("run_modeling_script", {"script": (
            "import adsk.core\n"
            "app=adsk.core.Application.get()\n"
            "want=%r\n"
            "found=None\n"
            "for i in range(app.documents.count):\n"
            "    dd=app.documents.item(i)\n"
            "    if (dd.name or '').replace('*','').strip()==want: found=dd; break\n"
            "df=None\n"
            "if found is not None:\n"
            "    try: df=found.dataFile\n"
            "    except Exception: df=None\n"
            "result={'name':(found.name if found else None),'inHub': df is not None}\n" % want)},
            timeout=30) or {}
        res = ((hub.get("data") or {}).get("result")) or {}
        ctx["inHub"] = bool(res.get("inHub"))
        if not res.get("inHub"):
            return ({"success": False, "errorCode": "not_in_hub",
                     "error": "Library %r is not saved in a Fusion Hub." % (res.get("name") or doc),
                     "remedyVerb": "fusion_save_to_cloud",
                     "_hint": "fusion_save_to_cloud {name, projectName, folderPath} first; a real "
                              "library save returns a fileId ending in .flbr."}, None)
    return (None, ctx)


def _click_cef_button(win, x_frac, y_frac, why):
    """Click a button in a CEF-drawn Fusion dialog (Update, Create New Version).

    These expose NOTHING to UIA - desktop_ui_tree returns zero nodes - so there is no name to
    click, only a position. Use the dialog's OWN rect from desktop_list_windows: those rects are
    already in the same physical-px space the input verbs take, so a fraction of the rect needs
    no DPI math and no screenshot. (Going via a capture's coordMap does not work from in here -
    the shotId lives in the screenshot's sidecar, not in the response the bridge can read, which
    is why the first version of this silently clicked nothing.)

    Pass hwnd so the click focuses the dialog atomically: it is a real modal window, not a Qt
    popup, so focusing is correct here.
    """
    r = (win or {}).get("rect") or {}
    left, top = r.get("left"), r.get("top")
    w, h = int(r.get("width") or 0), int(r.get("height") or 0)
    if left is None or top is None or not (w and h):
        return False
    _ab_call("desktop_click", {"x": int(left + w * x_frac), "y": int(top + h * y_frac),
                               "hwnd": win.get("hwnd"), "cursor": False, "reason": why},
             timeout=25)
    return True


def _dismiss_ok_dialog(title_substr, timeout_s=10.0):
    """Dismiss a single-OK CEF result dialog (e.g. "Up to Date": "Your library is up-to-date.").

    These are the RESULT of an action rather than a question, but they still block the add-in
    until answered, so a verb that ignores one reports success while Fusion sits waiting. The
    lone OK sits at the same button row as every other Fusion CEF dialog, rightmost."""
    import time as _t
    win = _wait_for_top_window(title_substr, timeout_s)
    if not win:
        return None
    clicked = _click_cef_button(win, 0.903, 0.824, "dismiss Fusion's %s dialog" % title_substr)
    _t.sleep(1.5)
    gone = _wait_for_top_window_gone(title_substr, 20)
    return {"dialog": title_substr, "clicked": clicked, "dismissed": bool(gone)}


def _confirm_create_version(choice="create", timeout_s=20.0):
    """Answer Fusion's "Create New Version" confirmation, which is what actually performs a push.

    Clicking the Push menu item only OPENS this dialog: "A new version of your library will be
    created." [Create] [Cancel]. Leaving it unanswered blocks the add-in, and reporting success
    at that point is a false pass - the push has not happened yet.
    """
    import time as _t
    win = _wait_for_top_window("Create New Version", timeout_s)
    if not win:
        return None
    frac = 0.565 if choice == "create" else 0.818
    clicked = _click_cef_button(win, frac, 0.824,
                                "answer Fusion's Create New Version prompt with %s" % choice)
    _t.sleep(2)
    gone = _wait_for_top_window_gone("Create New Version", 20)
    return {"prompt": "Create New Version", "choice": choice, "clicked": clicked,
            "answered": bool(gone), "dialogClosed": gone}


_UPDATE_CHOICES = {"merge": "Merge Latest Version", "replace": "Replace Local Library",
                   "cancel": "Cancel"}


def _handle_update_prompt(choice="cancel", timeout_s=12.0):
    """Answer Fusion's "Would you like to force update your library to the latest version?"

    Linking a library to a managed library makes Fusion offer to pull that library's contents
    over yours, with buttons [Merge Latest Version] [Replace Local Library] [Cancel]. REPLACE
    DESTROYS the local library, so this defaults to Cancel: linking is a relationship, and
    overwriting a user's parts is never an implied consequence of asking for one. Callers opt
    in explicitly with onUpdatePrompt:"merge"|"replace".

    The dialog is CEF-drawn: UIA returns nothing for it, so it is answered by capturing the
    window and clicking through ab's image space (zero coordinate math on our side).
    """
    import time as _t
    label = _UPDATE_CHOICES.get(choice, "Cancel")
    win = _wait_for_top_window("Update", timeout_s)
    if not win:
        return None
    # The three buttons sit on one row near the bottom right, in the order merge/replace/cancel.
    frac = {"Merge Latest Version": 0.396, "Replace Local Library": 0.689, "Cancel": 0.903}[label]
    if not _click_cef_button(win, frac, 0.824,
                             "answer Fusion's library Update prompt with %s" % label):
        return {"prompt": "Update", "answered": False, "choice": label,
                "error": "Could not measure the Update dialog to answer it."}
    _t.sleep(2)
    gone = _wait_for_top_window_gone("Update", 20)
    return {"prompt": "Update", "choice": label, "answered": bool(gone), "dialogClosed": gone}


def _handle_managed_library_push(fusion_info: dict, args: dict) -> dict:
    """Push a NEW VERSION of an already-linked managed library to library.io."""
    err, ctx = _managed_precheck()
    if err:
        return err
    steps = []
    mark = _alert_watermark()
    menu_err = _libio_menu(ctx["win"], "push", "Push changes to library.io", steps)
    if menu_err:
        return menu_err
    # An UNLINKED library answers Push with Create Managed Library instead.
    unlinked = _wait_for_top_window("Create Managed Library", 6)
    if unlinked:
        return {"success": False, "errorCode": "not_linked_yet", "steps": steps,
                "dialog": "Create Managed Library",
                "_hint": "This library is not linked yet - Fusion offered Create Managed Library. "
                         "Call fusion_managed_library_create instead (it completes that dialog), "
                         "or fusion_managed_library_link to attach it to an existing one. The "
                         "dialog is still open; answer or close it before the next verb."}
    # A LINKED library asks to confirm. Clicking Push alone does NOT push - the confirmation is
    # what creates the version, and leaving it open blocks the add-in.
    confirm = _confirm_create_version(args.get("onConfirm") or "create")
    if confirm:
        steps.append(confirm)
    alerts = _alerts_since(mark)
    if not confirm:
        return {"success": False, "errorCode": "confirm_missing", "steps": steps, "alerts": alerts,
                "library": ctx["doc"],
                "error": "Push did not raise the Create New Version confirmation.",
                "_hint": "Nothing was pushed. Check that this library is linked to a managed "
                         "library (fusion_managed_library_link) and read alerts[]."}
    ok = bool(confirm.get("dialogClosed")) and not alerts
    return {"success": ok, "library": ctx["doc"], "steps": steps, "alerts": alerts,
            "confirmed": confirm,
            "_hint": "A new version was pushed to the managed library. Library Manager still "
                     "shows the IMPORTED version number until you pull, so the row gains an "
                     "update-available badge rather than incrementing." if ok else
                     "The confirmation did not close or Fusion alerted - treat this as NOT "
                     "pushed and read alerts[]."}


def _handle_managed_library_pull(fusion_info: dict, args: dict) -> dict:
    """Pull the newest version FROM library.io into this linked library."""
    err, ctx = _managed_precheck(require_hub=False)
    if err:
        return err
    steps = []
    mark = _alert_watermark()
    menu_err = _libio_menu(ctx["win"], "pull",
                           "Pull changes from the managed library on library.io", steps)
    if menu_err:
        return menu_err
    # A pull ends in ONE of two dialogs, and BOTH block the add-in until answered:
    # content differs -> the force-update prompt; already current -> "Up to Date" with one OK.
    prompt = _handle_update_prompt(args.get("onUpdatePrompt") or "merge")
    if prompt:
        steps.append(prompt)
    up_to_date = None
    if not prompt:
        up_to_date = _dismiss_ok_dialog("Up to Date")
        if up_to_date:
            steps.append(up_to_date)
    alerts = _alerts_since(mark)
    answered = bool(prompt) or bool(up_to_date)
    return {"success": answered and not alerts, "library": ctx["doc"], "steps": steps,
            "alerts": alerts, "upToDate": bool(up_to_date), "updatePrompt": prompt,
            "_hint": ("Already current - Fusion answered with Up to Date." if up_to_date else
                      "Pulled; the force-update prompt was answered %r."
                      % ((prompt or {}).get("choice"))) if answered and not alerts else
                     ("Fusion raised an alert - read alerts[]." if alerts else
                      "Neither the update prompt nor Up to Date appeared, so NOTHING was pulled. "
                      "Fusion greys Pull out on a library that was never linked - link it first.")}


def _handle_managed_library_unlink(fusion_info: dict, args: dict) -> dict:
    """Unlink this library from its managed library on library.io."""
    err, ctx = _managed_precheck(require_hub=False)
    if err:
        return err
    steps = []
    mark = _alert_watermark()
    menu_err = _libio_menu(ctx["win"], "unlink",
                           "Unlink from the managed library on library.io", steps)
    if menu_err:
        return menu_err
    alerts = _alerts_since(mark)
    return {"success": not alerts, "library": ctx["doc"], "steps": steps, "alerts": alerts,
            "_hint": "Unlink requested. Fusion greys this item out on a library that was never "
                     "linked, so a silent no-op there is expected." if not alerts
                     else "Fusion raised an alert - read alerts[] and treat this as failed."}


def _handle_managed_library_link(fusion_info: dict, args: dict) -> dict:
    """Link this library to an EXISTING managed library on library.io.

    With no `name`, returns the picker's contents (name, type, version, managed folder,
    description) and cancels, so a caller can choose. With `name`, selects that row and
    confirms.

    Both the menu and the picker are handled by measurement, not by guesswork: the LIBRARY.IO
    dropdown is located as a window and its items derived from its rect, and the picker is a
    REAL dialog whose rows are readable over UIA.
    """
    err, ctx = _managed_precheck(require_hub=False)
    if err:
        return err
    import time as _t
    steps = []
    mark = _alert_watermark()
    menu_err = _libio_menu(ctx["win"], "link", "Link to a managed library on library.io", steps)
    if menu_err:
        return menu_err
    dlg = _wait_for_top_window("Select Managed Library", 25)
    if not dlg:
        return {"success": False, "errorCode": "picker_missing", "steps": steps,
                "alerts": _fusion_alerts(120),
                "error": "The LIBRARY.IO menu opened but Select Managed Library never appeared.",
                "_hint": "Fusion greys out Link on a library that is already linked - call "
                         "fusion_managed_library_unlink first, or check alerts[]."}
    hwnd = dlg.get("hwnd")
    rows, read_err = _picker_rows(hwnd)
    want = (args.get("name") or "").strip()

    def _close(why):
        _ab_call("desktop_ui_click", {"hwnd": hwnd, "name": "Cancel", "reason": why}, timeout=30)

    if not want:
        _close("close the Select Managed Library picker after listing it read-only")
        return {"success": True, "listedOnly": True, "steps": steps,
                "managedLibraries": rows, "count": len(rows), "readError": read_err,
                "_hint": "Pass name:<one of these> to link. This is what THIS Autodesk account "
                         "can reach: library.io's public managed folders plus any managed folder "
                         "this account belongs to. A managed library owned by another account "
                         "does NOT appear until its managed folder is shared with this one. Qt "
                         "virtualizes the list, so only realized rows are returned - scroll or "
                         "filter in the dialog if the one you want is missing."}
    hit = next((r for r in rows if r["name"].lower() == want.lower()), None)
    if not hit or not (hit.get("_at") or [None])[0]:
        _close("close the picker; the requested managed library was not listed")
        return {"success": False, "errorCode": "not_listed", "steps": steps,
                "managedLibraries": rows, "count": len(rows),
                "error": "%r is not in this account's managed-library list." % want,
                "_hint": "Names are exact. If you expect a library shared from another Autodesk "
                         "account, it only appears once its MANAGED FOLDER is shared with this "
                         "account on library.io - being in the same Fusion hub is not enough."}
    _ab_call("desktop_click", {"x": int(hit["_at"][0]), "y": int(hit["_at"][1]), "cursor": False,
                               "reason": "select %s in the Select Managed Library picker" % want},
             timeout=25)
    _t.sleep(1)
    ok = _ab_call("desktop_ui_click", {"hwnd": hwnd, "name": "OK",
                                       "reason": "confirm the managed-library link"}, timeout=30) or {}
    steps.append({"step": "confirm", "row": hit["name"], "clicked": bool(_find_key(ok, "success"))})
    _t.sleep(2)
    gone = _wait_for_top_window_gone("Select Managed Library", 15)
    # Fusion follows a link by offering to overwrite this library with the managed one.
    # Default to Cancel: the caller asked to LINK, not to have their parts replaced.
    prompt = _handle_update_prompt(args.get("onUpdatePrompt") or "cancel")
    if prompt:
        steps.append(prompt)
    alerts = _alerts_since(mark)
    return {"success": bool(gone), "linkedTo": hit["name"], "library": ctx["doc"],
            "managedFolder": hit.get("managedFolder"), "version": hit.get("version"),
            "steps": steps, "alerts": alerts, "dialogClosed": gone, "updatePrompt": prompt,
            "_hint": "Linked. Fusion offered to force-update this library from the managed one "
                     "and that prompt was answered %r - pass onUpdatePrompt:'merge' to take the "
                     "managed library's parts, or 'replace' to overwrite yours entirely."
                     % ((prompt or {}).get("choice") or "not shown")}


def _handle_library_manager(fusion_info: dict, args: dict) -> dict:
    """Open Library Manager and read one of its three tabs.

    tab: "hub" | "public" | "private" (default private - the library.io managed list).
    Returns the rows UIA exposes, which is how a caller SEES what this account can reach.
    """
    import time as _t
    ok, doc, ws = _library_in_front()
    if not ok:
        return {"success": False, "errorCode": "not_a_library",
                "error": "A library must be in front; the front document is %r (%s)." % (doc, ws),
                "_hint": "fusion_open_lbr {lbrPath} first."}
    win = _fusion_hwnd_rect()
    steps = [_ribbon_click(win, "manage_tab", "open the MANAGE tab")]
    _t.sleep(1.2)
    steps.append(_ribbon_click(win, "open_icon", "open Library Manager (the OPEN icon's default action)"))
    _t.sleep(9)
    dlg = _wait_for_dialog("Library Manager", 30)
    if not dlg:
        return {"success": False, "errorCode": "library_manager_missing", "steps": steps,
                "alerts": _fusion_alerts(120)}
    tab = (args.get("tab") or "private").lower()
    label = {"hub": "Hub Libraries", "public": "Public Libraries",
             "private": "Private Libraries"}.get(tab, "Private Libraries")
    click = _ab_call("desktop_ui_click", {"hwnd": dlg.get("hwnd"), "name": label,
                                          "reason": "switch Library Manager to %s" % label},
                     timeout=40) or {}
    steps.append({"step": "tab", "label": label, "ok": bool(click.get("success"))})
    _t.sleep(12)   # library.io fetches are slow; Library Manager can even show Not Responding
    tree = _ab_call("desktop_ui_tree", {"hwnd": dlg.get("hwnd"), "maxDepth": 10,
                                        "maxNodes": 600}, timeout=120) or {}
    rows = []

    def _walk(n, d=0):
        if d > 10 or not isinstance(n, dict):
            return
        nm = (n.get("name") or "").strip()
        if nm and 1 < len(nm) < 80:
            rows.append(nm)
        for c in (n.get("children") or []):
            _walk(c, d + 1)
    _walk(tree.get("root") or {})
    # Be honest about what came back. Library Manager's TABLE (Library / Folder Name / Version /
    # Imported / Active) is CEF-drawn and exposes nothing to UIA - only the dialog chrome and the
    # tab names do - so `rows` is navigation, NOT a library listing. Select Managed Library, by
    # contrast, is a real Qt table and IS readable, which is why link returns records and this
    # does not. Callers that need the actual list should read a screenshot.
    chrome = {"Library Manager", "System", "Close", "Electronics Desktop Widget",
              "Hub Libraries", "Public Libraries", "Private Libraries",
              "Select an item to see details here"}
    entries = [r for r in rows if r not in chrome]
    return {"success": True, "tab": label, "hwnd": dlg.get("hwnd"), "steps": steps,
            "rows": rows[:200], "libraries": entries[:200], "tableReadable": bool(entries),
            "nodeCount": tree.get("nodeCount"),
            "screenshotVerb": "desktop_screenshot_window {hwnd: %s}" % dlg.get("hwnd"),
            "_hint": ("Hub Libraries = libraries in the Fusion hub (what our bind chain produces, "
                      "and what Autodesk recommends for sharing). Private Libraries = library.io "
                      "managed libraries in folders THIS account belongs to. Public Libraries = "
                      "library.io's public catalogue. NOTE: the library TABLE is CEF-drawn and "
                      "invisible to UIA, so libraries[] is normally EMPTY and that is not a "
                      "failure - screenshot the hwnd to read the rows. Close it with "
                      "fusion_close_window when done.")}


def _handle_managed_library_create(fusion_info: dict, args: dict) -> dict:
    """Publish the library that is IN FRONT as a library.io managed library.

    Preconditions Fusion enforces (both refused here rather than prompting the user):
      1. a LIBRARY document is in front (not a design, not a board)
      2. that document is saved in a Fusion HUB - a library opened from a local .lbr is not,
         and Fusion answers "Cannot create a managed library from a document not saved in a
         Fusion Hub" as an in-canvas CEF error no window enumeration can see (fusion_alerts).
    """
    import time as _t
    st = _proxy_to_addin("get_app_state", {}, timeout=10) or {}
    data = st.get("data") or {}
    doc = str(data.get("activeDocument") or "")
    ws = str(data.get("activeWorkspace") or "")
    if "librar" not in ws.lower():
        return {"success": False, "errorCode": "not_a_library",
                "error": "The document in front is %r in the %s workspace, not a library."
                         % (doc, ws or "unknown"),
                "_hint": "fusion_open_lbr {lbrPath} first - it lands the library editor in front "
                         "and verifies it, which activate_document alone does NOT."}
    # Look the library up BY NAME, never via app.activeDocument: with a stray Untitled open,
    # get_app_state and a modeling script disagreed about which document was active on the very
    # first run of this verb (2026-09-01), so the hub check answered for the wrong document.
    _want = doc.replace("*", "").strip()
    hub = _proxy_to_addin("run_modeling_script", {"script": (
        "import adsk.core\n"
        "app=adsk.core.Application.get()\n"
        "want=%r\n"
        "found=None\n"
        "for i in range(app.documents.count):\n"
        "    dd=app.documents.item(i)\n"
        "    if (dd.name or '').replace('*','').strip()==want:\n"
        "        found=dd; break\n"
        "df=None\n"
        "if found is not None:\n"
        "    try: df=found.dataFile\n"
        "    except Exception: df=None\n"
        "result={'name': (found.name if found else None), 'matched': found is not None,"
        " 'inHub': df is not None, 'id': (df.id if df else None),"
        " 'open': [app.documents.item(i).name for i in range(app.documents.count)][:8]}\n"
        % _want)}, timeout=30) or {}
    hub_res = ((hub.get("data") or {}).get("result")) or {}
    if not hub_res.get("inHub"):
        return {"success": False, "errorCode": "not_in_hub",
                "openDocuments": hub_res.get("open"),
                "error": "Library %r is not saved in a Fusion Hub, so Fusion cannot make it a "
                         "managed library." % (hub_res.get("name") or doc),
                "remedyVerb": "fusion_save_to_cloud",
                "_hint": "fusion_save_to_cloud {name, projectName, folderPath} on this library "
                         "first; a real library save returns a fileId ending in .flbr (a .f3d "
                         "means the WRONG document was in front)."}
    win = _fusion_main_window()
    if not win:
        return {"success": False, "errorCode": "no_fusion_window",
                "error": "Could not locate Fusion's main window to drive the ribbon."}
    steps = []
    # USE AB'S ZERO-MATH PATH, do not compute coordinates here (John, 2026-09-01: "are you sure
    # you're not using enough of ab's verbs and rolling your own?"). desktop_click
    # {space:"image", shotId} lets ab convert through the capture's own coordMap: measured
    # exact (933,296 vs the 932,296 that works), where hand-rolled DIP math gave (283,51) and
    # window-fraction math (925,273). cursor:false matters too - the glowing cursor overlay
    # dismisses Fusion's Qt popup menus.
    win["dpiScale"] = _window_dpi_scale(win.get("hwnd"))
    menu_err = _libio_menu(win, "push", "Push changes to library.io", steps)
    if menu_err:
        return menu_err
    dlg = _wait_for_top_window("Create Managed Library", 25)
    if not dlg:
        return {"success": False, "errorCode": "create_dialog_missing", "steps": steps,
                "alerts": _fusion_alerts(120),
                "error": "The LIBRARY.IO menu opened but Create Managed Library never appeared.",
                "windowRect": win.get("rect"),
                "_hint": "Push offers Create only for a library that is NOT yet linked. If this "
                         "library is already linked, fusion_managed_library_push pushes a new "
                         "version instead - check fusion_alerts."}
    ok = _ab_call("desktop_ui_click", {"hwnd": dlg.get("hwnd"), "name": "Create",
                                       "reason": "create the managed library on library.io"},
                  timeout=30) or {}
    steps.append({"step": "create", "ok": bool(ok.get("success"))})
    conf = _wait_for_dialog("Create Managed Library", 40)   # the confirmation reuses the title
    if conf:
        _ab_call("desktop_ui_click", {"hwnd": conf.get("hwnd"), "name": "OK",
                                      "reason": "acknowledge the managed-library confirmation"},
                 timeout=30)
        steps.append({"step": "ok"})
    alerts = _fusion_alerts(120)
    return {"success": not alerts, "library": hub_res.get("name") or doc, "steps": steps,
            "alerts": alerts,
            "_hint": ("Managed library created; it lives under the account's managed FOLDER on "
                      "library.io (not public - a public search will not find it). Share it by "
                      "managing that folder on library.io." if not alerts else
                      "Fusion raised an alert during the push - read alerts[] and treat this as "
                      "failed.")}


def _handle_restart_for_addin(fusion_info: dict, args: dict) -> dict:
    """Save open documents, restart Fusion, wait for the add-in, report back.

    The remedy half of the Adonis incident (John, 2026-08-27): telling a user "restart
    Fusion" is still making them do our job. This does it, and keeps the promise the
    message makes - "we will save your open documents first" - because a restart that
    quietly discards someone's work would be far worse than the original silence.

    save:false skips saving (an AI on an unattended box that knows the docs are scratch).
    """
    if args.get("decline"):
        # The page's "I'll handle it" button: record the hold exactly as the toast's button
        # would, so no automatic path restarts Fusion until the user does.
        _set_user_restart_hold(args.get("notificationId") or "page")
        _bridge_action_ledger("restart_choice", {"choice": "declined-from-page"})
        return {"success": True, "declined": True, "hold": True,
                "_hint": "Recorded: the user owns the restart. Any chain waiting on the "
                         "notice stops with user_owns_restart; nothing restarts Fusion."}
    # ASYNC BY DEFAULT (Ray, adom/hydrogen#42): a real Fusion restart takes longer than the
    # relay's 60s budget, so the synchronous form ALWAYS reported "timeout" even when it
    # worked, and a caller could not tell "restarting, wait" from "failed". The verb now
    # returns immediately and does the work on a thread; poll fusion_readiness until
    # ready:true with the expected addinVersion. Pass wait:true for the old blocking form
    # (in-process callers use that).
    if not args.get("wait") and not args.get("_sync"):
        import threading as _rth, json as _rjs
        def _bg():
            try:
                r = _handle_restart_for_addin(fusion_info, dict(args, wait=True))
                print("[Fusion Bridge] async restart_for_addin finished: %s"
                      % _rjs.dumps({k: r.get(k) for k in ("success", "errorCode", "error")})[:200])
                _set_bridge_pref("lastRestartResult", {k: r.get(k) for k in ("success", "errorCode", "error")})
            except Exception as e:
                print("[Fusion Bridge] async restart_for_addin crashed: %s" % str(e)[:200])
        _rth.Thread(target=_bg, daemon=True).start()
        return {"success": True, "started": True, "async": True,
                "statusVerb": "fusion_readiness",
                "_hint": ("Restart under way in the background (a Fusion restart takes 60-120s, "
                          "longer than the relay budget, so this returns immediately). Poll "
                          "fusion_readiness until ready:true and addinVersion matches; the "
                          "outcome is also stored in the lastRestartResult preference. A timeout "
                          "on this verb no longer exists and never meant failure anyway.")}
    import time as _t
    save = args.get("save", True)
    t0 = _t.time()
    steps = []

    if save:
        # Save FIRST, and treat a failure as a STOP, not a warning: the whole point is
        # that nobody loses work to our restart.
        try:
            sv = _proxy_to_addin("close_all_documents", {"save": True}, timeout=180) or {}
            steps.append({"step": "save", "ok": bool(sv.get("success")),
                          "closed": (sv.get("data") or {}).get("closed"),
                          "wouldPrompt": (sv.get("data") or {}).get("wouldPrompt")})
            _wp = (sv.get("data") or {}).get("wouldPrompt")
            # BRIDGE-OWNED SCRATCH IS DISPOSABLE (John, 2026-08-30, three "Unsaved changes"
            # prompts in a row): a never-saved test board the bridge itself generated must
            # not stop the restart, and a graceful close would make Fusion ASK for it. When
            # every such document is the bridge's own, end Fusion hard (no close prompts,
            # nothing of the user's on the line) instead of refusing or asking.
            _hard = False
            if _wp and not _user_unsaved_docs(_wp):
                steps.append({"step": "discard-bridge-scratch", "docs": list(_wp)})
                _hard = True
                _wp = None
            if _wp:
                return {"success": False, "errorCode": "unsaved_named_document",
                        "error": ("Cannot restart safely: %s has never been saved, so Fusion "
                                  "would raise its Save box and block the restart."
                                  % ", ".join(repr(n) for n in _wp)),
                        "steps": steps,
                        "_hint": ("Ask the user where to save it (fusion_save_to_cloud "
                                  "{name, projectName}), then retry. Nothing was closed and "
                                  "no dialog was raised.")}
        except Exception as e:
            return {"success": False, "errorCode": "save_failed", "steps": steps,
                    "error": "Refusing to restart because saving failed: %s" % str(e)[:200],
                    "_hint": "Nothing was restarted; the user's documents are untouched."}

    try:
        # handle_fusion_stop, imported at module top from handlers.close_fusion. My first
        # draft called a name that does not exist (_handle_fusion_stop) behind a globals()
        # check that would have silently taken a fallback path instead of failing loudly.
        if (save and locals().get("_hard")) or not save:
            # no graceful close: it would prompt for every unsaved document; with only bridge
            # scratch (save:true path) or an explicit save:false, a hard stop is the dialog-free way
            st = handle_fusion_kill(detect_fusion(), {"reason": "restart for add-in; only bridge scratch unsaved"})
            steps.append({"step": "kill", "ok": bool((st or {}).get("success", True))})
        else:
            st = handle_fusion_stop(detect_fusion(), {})
            steps.append({"step": "stop", "ok": bool((st or {}).get("success", True))})
    except Exception as e:
        steps.append({"step": "stop", "ok": False, "error": str(e)[:200]})

    _t.sleep(3)
    try:
        lr = _handle_launch(detect_fusion(), {"waitForAddin": True}) or {}
        steps.append({"step": "start", "ok": bool(lr.get("success"))})
    except Exception as e:
        return {"success": False, "errorCode": "restart_failed", "steps": steps,
                "error": "Fusion was stopped but did not come back: %s" % str(e)[:200],
                "_hint": "Launch it with fusion_start and check fusion_readiness "
                         "(needsSignin true means Autodesk sign-in is blocking)."}

    ready = _addin_round_trip_ready(tries=6, gap=5.0)
    return {
        "success": bool(ready),
        "addinReady": bool(ready),
        "tookSec": round(_t.time() - t0, 1),
        "steps": steps,
        "errorCode": None if ready else "addin_still_not_responding",
        "_hint": ("Fusion restarted and the add-in answered - re-run the verb that failed."
                  if ready else
                  "Fusion restarted but the add-in still is not answering. That is a real "
                  "defect, not a user problem: check fusion_addin_status and file it."),
    }


def _handle_inspect_bodies(fusion_info: dict, args: dict) -> dict:
    """fusion_inspect_bodies - what actually got built, in millimetres."""
    world = bool(args.get("worldSpace"))
    occ = args.get("occurrence") or ""
    script = (_INSPECT_BODIES_SCRIPT
              .replace("__WORLD__", "True" if world else "False")
              .replace("__OCC__", repr(str(occ)) if occ else "None"))
    res = _proxy_to_addin("run_modeling_script", {"script": script}, timeout=120)
    if not res.get("success"):
        return res
    data = res.get("data") or {}
    payload = data.get("result")
    if not isinstance(payload, dict):
        return {"success": False, "error": "inspect_bodies returned no structured result",
                "errorCode": "inspect_failed", "data": data,
                "_hint": "The read-back script ran but produced no result dict. Usually means no "
                         "active Design; open a document and retry."}
    n = payload.get("bodyCount", 0)
    payload["success"] = True
    payload["_hint"] = (
        "Bounding boxes, volumes and areas are in MILLIMETRES (Fusion's API is centimetres; "
        "converted here so you do not have to). Check bbox Z against where the part SHOULD sit - "
        "a wrong offset renders perfectly and is invisible in a screenshot. cylindricalFaceCount "
        "is the cheap way to confirm holes were actually cut: a symmetric cut that reaches nothing "
        "fails SILENTLY with no exception, and the count is what surfaces it. "
        + ("Pass worldSpace:true to resolve bodies through their occurrence transforms and verify "
           "assembly placement." if not world else
           "worldSpace:true - occurrence bodies are proxies already resolved into assembly space.")
    ) if n else (
        "No bodies found. If you expected some, the active document may not be the one you built "
        "into (pass expectDocument on mutating verbs to catch that), or the design is empty."
    )
    return payload


COMMAND_HANDLERS["inspect_bodies"] = _handle_inspect_bodies
COMMAND_HANDLERS["restart_for_addin"] = _handle_restart_for_addin
COMMAND_HANDLERS["live_feed"] = _handle_live_feed
COMMAND_HANDLERS["plan_library_import"] = _handle_plan_library_import
COMMAND_HANDLERS["library_parts"] = _handle_library_parts



# ── Uninstall cascade (ab 2.0.35) ────────────────────────────────────────────────────────────
# ab's bridge_uninstall calls this verb FIRST, while our files still exist, so we can remove the
# artifacts we planted OUTSIDE our own cache dir. That matters more for this bridge than for most:
# we install a Fusion ADD-IN into Autodesk's own directories, and a stale AdomBridge add-in keeps
# loading on every Fusion launch, binding port 8774, and reaching for a bridge that no longer
# exists. We also hold two user-data stores (~/.adom/fusion-bridge prefs, ~/.adom/fusion-aps
# OAuth tokens). Uninstall means all of it goes.
#
# Pass {"dryRun": true} to see exactly what WOULD be removed without touching anything.
def _handle_uninstall(fusion_info: dict, args: dict) -> dict:
    """fusion_uninstall - remove this bridge's out-of-cache artifacts (add-in, prefs, APS tokens)."""
    import shutil as _sh
    import pathlib as _pl
    args = args or {}
    dry = bool(args.get("dryRun"))
    home = _pl.Path(os.path.expanduser("~"))

    targets = []
    try:
        from install_addin import TARGET_CANDIDATES as _TC
        targets += [_pl.Path(str(t)) / "AdomBridge" for t in _TC]
    except Exception:
        targets += [home / "AppData" / "Roaming" / "Autodesk" / "FusionAddins" / "AdomBridge",
                    home / "AppData" / "Roaming" / "Autodesk" / "Autodesk Fusion" / "API" / "AddIns" / "AdomBridge",
                    home / "AppData" / "Roaming" / "Autodesk" / "Autodesk Fusion 360" / "API" / "AddIns" / "AdomBridge"]
    addin_targets = list(targets)
    data_targets = [home / ".adom" / "fusion-bridge", home / ".adom" / "fusion-aps"]

    removed, failed, skipped = [], [], []
    for p in addin_targets + data_targets:
        kind = "addin" if p in addin_targets else "userdata"
        if not p.exists():
            skipped.append({"path": str(p), "kind": kind, "why": "not present"})
            continue
        if dry:
            removed.append({"path": str(p), "kind": kind, "dryRun": True})
            continue
        try:
            _sh.rmtree(p)
            removed.append({"path": str(p), "kind": kind})
        except Exception as e:
            failed.append({"path": str(p), "kind": kind, "error": str(e)})

    running = False
    try:
        running = bool(_is_fusion_running())
    except Exception:
        pass

    out = {"success": not failed, "dryRun": dry, "cascade": bool(args.get("cascade")),
           "fusionRunning": running, "removed": removed, "failed": failed, "skipped": skipped}
    if failed and running:
        out["_hint"] = ("Fusion is RUNNING and holds the add-in files open, so %d path(s) could not be "
                        "removed. Close Fusion (fusion_stop) and re-run fusion_uninstall, or delete "
                        "those paths by hand - otherwise the stale add-in keeps loading inside Fusion "
                        "on every launch and binding its port." % len(failed))
    elif failed:
        out["_hint"] = ("%d path(s) could not be removed (permissions?). They are listed in `failed`; "
                        "delete them by hand so no stale add-in is left inside Fusion." % len(failed))
    elif dry:
        out["_hint"] = ("DRY RUN - nothing was touched. These are the paths a real uninstall removes: "
                        "the Fusion add-in in Autodesk's AddIns dirs, plus this bridge's prefs and "
                        "APS OAuth tokens under ~/.adom.")
    else:
        out["_hint"] = ("Cleaned up: the Fusion add-in is gone from Autodesk's directories, and this "
                        "bridge's prefs + APS OAuth tokens are removed. Fusion itself is untouched. "
                        "Re-installing the bridge redeploys the add-in on its next start.")
    return out


COMMAND_HANDLERS["uninstall"] = _handle_uninstall
COMMAND_HANDLERS["demo_progress"] = lambda _info, args: _handle_demo_progress(args or {})


def _arm_crash_log():
    """A death record that SURVIVES the respawn (2026-08-29: a ten-part import died at 39%
    with `bridge_restarted_mid_run`, and nothing explained why: ab truncates
    fusion360.log on every spawn, so the previous process's last words were gone, and
    fusion360.ad.log only lists spawns). This bridge now appends to its OWN
    ~/.adom/bridge-logs/fusion360.crash.log: a start line per process, every uncaught
    exception (main thread and worker threads), native faults via faulthandler, and a
    clean-exit line. A start line with neither a traceback nor a clean-exit line after it
    means the process was killed from outside (an ab reap, a kill, a logoff)."""
    try:
        import atexit
        import faulthandler
        d = os.path.join(os.path.expanduser("~"), ".adom", "bridge-logs")
        os.makedirs(d, exist_ok=True)
        path = os.path.join(d, "fusion360.crash.log")
        try:
            if os.path.getsize(path) > 2 * 1024 * 1024:
                with open(path, "rb") as f:
                    f.seek(-512 * 1024, os.SEEK_END)
                    tail = f.read()
                with open(path, "wb") as f:
                    f.write(tail)
        except Exception:
            pass
        fh = open(path, "a", buffering=1, encoding="utf-8", errors="replace")
        stamp = lambda: _t2.strftime("%Y-%m-%d %H:%M:%S")
        fh.write("[%s] START pid %s version %s argv %s\n" % (stamp(), os.getpid(), BRIDGE_VERSION, sys.argv[1:]))
        faulthandler.enable(file=fh, all_threads=True)

        def _hook(exc_type, exc, tb):
            fh.write("[%s] UNCAUGHT in main thread (pid %s):\n" % (stamp(), os.getpid()))
            traceback.print_exception(exc_type, exc, tb, file=fh)
            sys.__excepthook__(exc_type, exc, tb)
        sys.excepthook = _hook

        def _thook(args):
            fh.write("[%s] UNCAUGHT in thread %r (pid %s):\n" % (stamp(), getattr(args.thread, "name", "?"), os.getpid()))
            traceback.print_exception(args.exc_type, args.exc_value, args.exc_traceback, file=fh)
            try:
                traceback.print_exception(args.exc_type, args.exc_value, args.exc_traceback)
            except Exception:
                pass
        threading.excepthook = _thook
        atexit.register(lambda: fh.write("[%s] CLEAN EXIT pid %s\n" % (stamp(), os.getpid())))
    except Exception:
        traceback.print_exc()


if __name__ == "__main__":
    # Surface any fatal startup error to stderr (which ab captures into
    # ~/.adom/bridge-logs/fusion360.log) so a spawn-crash is debuggable, then
    # re-raise for a non-zero exit. The crash log (fusion360.crash.log) is the record
    # that survives ab's per-spawn truncation of fusion360.log.
    _arm_crash_log()
    try:
        main()
    except Exception:
        print("[Fusion Bridge] FATAL: bridge failed to start", flush=True)
        traceback.print_exc()
        sys.stderr.flush()
        raise
