"""Classify Fusion 360 blocking dialogs by title, with resolution hints.

Why this exists: several Fusion modals (the multi-linked "Select Electronics
Design File" picker, the "Fusion needs to update" nag, Document Recovery, "Save
changes?", "What do you want to design?") block Fusion's main thread. When that
happens during an add-in command, the add-in's HTTP server can't respond and the
bridge reports a misleading "add-in not responding / may have crashed" — which
sends callers into a pointless restart loop.

Qt dialog *titles* are enumerable over Win32 even while the add-in is blocked
(unlike the dialogs' CEF/Qt child controls, which are not). So we can cheaply
identify WHICH modal is up and return an actionable resolution instead of a fake
crash. This is bridge-wide: it helps every command that can be blocked by a
modal, not just exports.
"""

from handlers.fusion_ui import get_fusion_window_info


# (category, resolution-hint) keyed by lowercase title substrings, checked in order.
_RULES = [
    # Fusion Electronics' own router assertion (pusher.cpp inLocator), seen on ConfRoomROG
    # 2026-09-07 whenever a LINE completed a net's last airwire. OK is informational; the
    # copper already landed. Checkpoint, then verify with fusion_routing_state.
    (("unexpected condition", "pusher.cpp"),
     "fusion_unexpected_condition",
     "Fusion Electronics asserted inside its own router. OK is the only button and is "
     "informational; the copper already landed. Click OK, CHECKPOINT the board "
     "(fusion_export_eagle_source), then verify the copper with fusion_routing_state."),

    # Fusion refused to open a broken .lbr and left an empty design behind (John caught it
    # live 2026-08-21, a 77-byte truncated file). OK is the only button; the orphan
    # Untitled design is bridge-attributable scratch, closed without saving by the janitor.
    (("has errors and cannot be opened", "close the empty design file without saving"),
     "lbr_open_error",
     "Fusion could not parse the library file. Click OK, close the leftover empty Untitled "
     "design WITHOUT saving, and fix the FILE - as of 1.9.128 fusion_open_lbr refuses "
     "unparseable libraries up front (lbr_invalid) so this modal should never appear for a "
     "bridge-issued open; a raw caller-staged file can still trip it."),

    # ⚠️ DATA-LOSS dialog. Closing a doc while its 3D packages still upload to the Hub.
    # Title is often the bare "Fusion", so this also gets caught by the generic fallback
    # below; the rule here fires when the build surfaces descriptive text. The CORRECT
    # action is NEVER "dismiss" - it is "No, then wait for the upload to drain".
    (("uploaded to your fusion hub", "packages are being uploaded", "being uploaded",
      "lose these changes"),
     "upload_in_progress",
     "DATA LOSS RISK. Fusion is still uploading 3D package(s) to the Hub. Do NOT click Yes / do "
     "NOT close - you will LOSE the uploaded packages and any unsaved bindings. Click No, then WAIT "
     "and poll until the upload finishes before closing or saveAs. See the fusion-cloud-save skill."),

    (("cannot be saved while packages", "save was cancelled", "save cancelled"),
     "save_blocked_by_upload",
     "saveAs was refused because 3D packages are still uploading to the Hub (this also throws "
     "InternalValidationError). Not fatal: wait for the upload to drain, then retry the saveAs and "
     "poll past it. See the fusion-cloud-save skill."),

    (("needs to update", "update is available", "software update", "new update"),
     "update_nag",
     "Fusion is prompting to update itself. Do NOT auto-confirm (it can start an "
     "update mid-automation). Dismiss it on the desktop or let Fusion update, then "
     "retry. A pending update is also a common cause of AdomBridge add-in crashes "
     "(add-in/host version drift), so clearing it often fixes those too."),

    (("select electronics design", "multiple electronics design", "linked to multiple"),
     "linked_design_picker",
     "This file links to multiple Electronics designs. The chooser is a CEF/web "
     "modal whose list is NOT keyboard- or Win32-navigable (SendInput reaches only "
     "the native Cancel/OK), so it can't be resolved headlessly today — select the "
     "design on the desktop. (Headless fix tracked: have the add-in open the specific "
     "linked design via the Fusion API, avoiding the picker entirely.)"),

    (("recovery", "recover unsaved", "document recovery"),
     "recovery",
     "Document Recovery prompt. Call fusion_dismiss_recovery, or fusion_relocate_recovery "
     "BEFORE fusion_start to prevent it."),

    # "save" alone: ConfRoomROG's Fusion build titles the unsaved-doc prompt with the bare
    # word (2026-08-16 - it classified "unknown" and blocked the main thread invisibly).
    # Substring "save" is safe HERE because the more specific save rules above run first.
    (("save changes", "save document", "do you want to save", "unsaved changes", "save"),
     "save_changes",
     "Unsaved-changes prompt. Use fusion_close_document (closes without the save modal), "
     "or fusion_dismiss_blocking_dialogs, which answers No."),

    (("what do you want to design", "what to design"),
     "what_to_design",
     "Fusion's start picker. fusion_dismiss_blocking_dialogs closes it."),

    # ── Launch / licensing / setup dialogs (owned by AdskIdentityManager /
    #    FusionLauncher, so classify_launch_dialogs enumerates ALL top-level
    #    windows to catch them - get_fusion_window_info would miss them). ──

    # "Create new footprint 'X'?" - Fusion offering to CREATE something we asked to open.
    # The bridge should never see this (we verify the name exists first, see
    # _resolve_artifact_name), but a library opened by another route can still raise it, and
    # it BLOCKS THE MAIN THREAD until answered - which reads downstream as "add-in not
    # responding" rather than "a modal is waiting". NOTE the window TITLE is just "Fusion":
    # this is only identifiable from its BODY TEXT, which is why title-only classification
    # missed it entirely (John, 2026-08-21).
    (("create new footprint", "create new symbol", "create new device",
      "create new package", "create new library"),
     "create_confirm",
     "Fusion is asking whether to CREATE an artifact we asked to open, which means the name "
     "does not exist in the library. Answering 'Yes' invents an empty artifact and hides the "
     "real problem, so the bridge answers NO and reports the missing name. Fix the NAME "
     "(fusion_show_* refuses up front with `available` listing what the .lbr actually holds)."),

    # Fusion CRASHED and is offering to send a report. Benign to close (the X == Close,
    # the report still uploads its basic data), and CRITICALLY it is the tell that Fusion
    # is gone even though a Fusion360.exe still shows in tasklist - which is exactly how
    # ConfRoomROG spent 10+ minutes reporting "running but not ready" with nothing to
    # explain it (2026-08-19). Classified so readiness can clear it AND know to relaunch.
    (("caused fusion to close unexpectedly", "fusion error report",
      "a software problem has caused", "close unexpectedly"),
     "crash_report",
     "Fusion CRASHED. The bridge closes this and relaunches Fusion - the add-in only "
     "loads at launch, so a relaunch is the only way back to drivable."),

    # Autodesk SEAT conflict — a DECISION dialog. The options suspend / shut down
    # Fusion on the user's OTHER machine (risking its unsaved work), so NEVER
    # auto-pick: notify + ASK the user which option they want.
    # "Session Suspended" - the OTHER side of the seat conflict: another box took the seat
    # and THIS box's session is suspended. The remedy is 'Check again' (regrab), never
    # 'Shut Down' (signs this box out). John, 2026-08-21: "always regrab the session."
    (("session suspended", "suspended this session", "suspended by:"),
     "session_suspended_regrab",
     "This box's Autodesk session was SUSPENDED by another machine taking the seat. The "
     "bridge clicks 'Check again' to regrab it (background UIA). Never click 'Shut Down' - "
     "that signs this box out."),

    # Electronics-editor SETTINGS dialogs that block automation and carry NO data (John saw
    # the Grid dialog full-screen-blocking during a matrix run, 2026-08-23). Nothing we send
    # opens GRID on purpose - it is a stray/inadvertent raise - but a settings dialog must
    # never block the queue: it is safe to Cancel unconditionally (nothing is lost).
    (("grid", "display style dots lines", "finest", "altdistance"),
     "settings_dialog_cancel",
     "A Fusion electronics SETTINGS dialog (e.g. Grid) is open and blocking. It carries no "
     "data - Cancel it (WM_CLOSE / Cancel). Nothing we drive opens these deliberately; if it "
     "keeps recurring, something is injecting a stray GRID/settings command or keystroke."),

    (("active sessions exceeded", "more active sessions", "more sessions running than are allowed",
      "sessions running than are allowed"),
     "session_conflict_decision",
     "DECISION DIALOG - do NOT auto-pick. This Autodesk seat is already active on another machine; "
     "the options SUSPEND or SHUT DOWN Fusion THERE (risking that machine's unsaved work). fire a "
     "notify_user and ASK the user which option they want, then click it. (Suspend = pausable/"
     "resumable; Shut down = saves the other machine's work to a recovery file.)"),

    (("suspend remote session",),
     "session_suspend_confirm",
     "Confirmation of a chosen 'suspend the OTHER machine's Fusion'. Only click Continue if the user "
     "ALREADY chose suspend; otherwise Go Back and ASK. Never auto-confirm a seat action."),

    # A second Fusion launch while one is already running - the correct action is
    # CANCEL (Fusion is a singleton; a 2nd instance is never wanted by automation).
    (("fusion is already open", "multiple instances are not supported",
      "launch another fusion instance"),
     "already_open",
     "Fusion is ALREADY running - a second launch was attempted. Click CANCEL (never Launch): "
     "Fusion is a singleton and the running instance is the one to drive. This usually means a "
     "prior fusion_stop/kill did not fully clear the old process before relaunch."),

    # First-run Autodesk sign-in (the 'Welcome to Fusion / Sign In' window, and the
    # browser callback 'You're signed in ... Open Product'). NOT auto-dismissable -
    # it needs the user's login; drive the buttons (Sign In / Open Product) via UIA
    # but NEVER auto-fill credentials.
    (("welcome to fusion", "signing in - autodesk", "sign in - autodesk"),
     "signin_required",
     "Autodesk sign-in needed (first run). NOTIFY the user, drive the 'Sign In' button (desktop_ui_"
     "click), fetch any email OTP via adom-google, and after the browser shows \"You're signed in\" "
     "click its 'Open Product' button - but NEVER auto-enter the password/2FA. See fusion-aps-signin."),

    # webdeploy launch failure - an INCOMPLETE/stale production folder (missing
    # FusionLauncher.exe.ini). Benign to dismiss; means you launched the wrong hash.
    (("error launching streamed application", "missing or incomplete", "please re-install",
      "re-install the application"),
     "launch_error",
     "Launch error from an INCOMPLETE webdeploy folder (missing FusionLauncher.exe.ini). Safe to "
     "dismiss (OK/close) - it is only an acknowledgement. Launch the COMPLETE production folder (the "
     "hash dir that HAS FusionLauncher.exe.ini); detect_fusion now prefers it."),

    # ⚠️ OPERATION FAILED. A bare "Error"-titled owned popup means the LAST command did
    # NOT succeed even if the API call returned ok - e.g. an .lbr that "has errors and
    # cannot be opened" (Fusion opened an EMPTY design instead), a failed import/export,
    # a bad STEP. This is NOT a decision and NOT a benign launch ack: the bridge flips the
    # command's success to False when it sees this (see server._apply_failure_dialogs), so
    # a malformed library can never again be reported as "opened". Keep this rule LAST so
    # the specific "error launching..."/seat rules above win first. It is a pure OK
    # acknowledgement, so it is safe to close (WM_CLOSE) after the failure is surfaced.
    (("error", "cannot be opened", "has errors", "failed to", "could not", "invalid"),
     "operation_error",
     "The last operation FAILED - Fusion is showing an Error dialog (the API call may still have "
     "returned ok because it opened an EMPTY design). READ the dialog screenshot for the exact "
     "message. Common case: '<file>.lbr has errors and cannot be opened' = the library is malformed "
     "(re-lint/regenerate; a frequent cause is a single UNNAMED symbol pin - EAGLE <pin name=''> + "
     "<connect pin=''> - name the pin from the KiCad pin NUMBER). The bridge already flipped success "
     "to False and dismissed the empty design; FIX the source file and retry. Do NOT report success."),
]

# Categories that mean the OPERATION FAILED. The bridge downgrades a command's
# success to False when a popup of one of these categories is up after it runs, so
# a failure (e.g. a malformed .lbr) can never be reported as success. These are pure
# OK-acknowledgements, so the bridge may close them after surfacing the failure.
FAILURE_CATEGORIES = {"operation_error"}
# Categories that require a USER decision — the launch code must NOT auto-dismiss
# these; it surfaces them + fires a notify_user + asks.
DECISION_CATEGORIES = {"session_conflict_decision", "session_suspend_confirm"}
# Categories the launch code MAY auto-dismiss (a benign ack; WM_CLOSE == Cancel/OK).
# Everything else (decisions, sign-in) is SURFACED, never auto-closed.
AUTO_DISMISS_CATEGORIES = {"launch_error", "already_open", "crash_report", "create_confirm", "settings_dialog_cancel"}
# A crash report means the app is GONE, not merely dialog-blocked: clearing it is not
# enough, the bridge must relaunch. Kept separate from AUTO_DISMISS so a caller can tell
# "I closed a nuisance" from "I closed a nuisance and must now restart the app".
RELAUNCH_CATEGORIES = {"crash_report"}
# Launch/setup categories enumerated across ALL top-level windows (not just Fusion).
_LAUNCH_CATEGORIES = DECISION_CATEGORIES | {"launch_error", "already_open", "signin_required"}


def _category_for_title(title: str):
    t = (title or "").strip().lower()
    if not t:
        return None
    for substrings, category, resolution in _RULES:
        if any(s in t for s in substrings):
            return category, resolution
    # A bare "Fusion"/"Fusion360"-titled modal is generic - its body text (the part
    # that tells you what it actually IS) lives in CEF/Qt child controls that are NOT
    # Win32-enumerable, so we cannot classify it by title alone. The two it most often
    # is, depending on context: during a file OPEN, the "Select Electronics Design File"
    # chooser; during a CLOSE or SAVE, the Hub "packages are being uploaded... sure you
    # want to close?" data-loss confirm. So the only safe instruction is: READ the
    # attached screenshot and ANALYZE before acting. NEVER tell the caller to blindly
    # dismiss - clicking Yes on the upload confirm destroys the packages.
    if t in ("fusion", "fusion360", "autodesk fusion 360", "fusion 360"):
        return ("generic_modal_read_screenshot",
                "Generic Fusion modal - its body is not Win32-readable, so a screenshot "
                "is attached: READ it and ANALYZE before acting. Do NOT blind-dismiss. "
                "If it mentions uploading / 'lose these changes' / 'sure you want to "
                "close' -> click No and wait for the Hub upload to drain (fusion-cloud-save). "
                "If it is the 'Select Electronics Design File' chooser during an open, its "
                "list is not keyboard/Win32-navigable; open the specific design by URN or "
                "select on the desktop.")
    return ("unknown",
            "Unrecognized modal - a screenshot is attached: READ it and ANALYZE before "
            "acting. Do NOT blindly dismiss (some modals lose work if confirmed wrong). "
            "Identify it from the screenshot, then take the work-preserving action.")


def _enumerate_all_top_level():
    """(hwnd, title) for every visible, titled top-level window.

    Unlike get_fusion_window_info (scoped to the Fusion process), this catches
    dialogs owned by the Autodesk FAMILY - AdskIdentityManager (sign-in / seat
    conflict) and FusionLauncher (streamed-app launch errors) - which are the
    ones that block a first launch. Best-effort; never raises.
    """
    import ctypes
    results = []
    user32 = ctypes.windll.user32 if hasattr(ctypes, "windll") else None
    if not user32:
        return results
    WNDENUMPROC = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)

    def _cb(hwnd, _lparam):
        try:
            if user32.IsWindowVisible(hwnd):
                n = user32.GetWindowTextLengthW(hwnd)
                if n:
                    buf = ctypes.create_unicode_buffer(n + 1)
                    user32.GetWindowTextW(hwnd, buf, n + 1)
                    if buf.value.strip():
                        results.append((hwnd, buf.value))
        except Exception:
            pass
        return True

    try:
        user32.EnumWindows(WNDENUMPROC(_cb), 0)
    except Exception:
        pass
    return results


def classify_launch_dialogs() -> list:
    """Classify LAUNCH / licensing / setup dialogs across ALL top-level windows.

    Each entry: {hwnd, title, category, resolution, decision}. `decision:true`
    means a USER choice is required (seat conflict) - the launch path must NOT
    auto-dismiss it; it notifies + asks. Benign `launch_error` dialogs (decision:
    false) can be closed. Empty list if none. Never raises.
    """
    out = []
    for hwnd, title in _enumerate_all_top_level():
        classified = _category_for_title(title)
        if not classified:
            continue
        category, resolution = classified
        if category not in _LAUNCH_CATEGORIES:
            continue
        out.append({
            "hwnd": hwnd,
            "title": title,
            "category": category,
            "resolution": resolution,
            "matchedOn": "title",
            "decision": category in DECISION_CATEGORIES,
        })
    return out


def close_dialog_bg(hwnd) -> bool:
    """WM_CLOSE a window in the BACKGROUND (PostMessage = no focus steal). Used to
    auto-dismiss benign launch-error acks. Never raises; returns True if posted."""
    import ctypes
    user32 = ctypes.windll.user32 if hasattr(ctypes, "windll") else None
    if not user32 or not hwnd:
        return False
    try:
        return bool(user32.PostMessageW(ctypes.c_void_p(int(hwnd)), 0x0010, 0, 0))  # WM_CLOSE
    except Exception:
        return False


def _dialog_text(hwnd) -> str:
    """The dialog's BODY TEXT, read via UIA.

    Fusion titles several distinct modals just "Fusion", so a title-only classifier cannot
    tell "Create new footprint 'X'?" from any other one (John, 2026-08-21). ab exposes the
    a11y tree through desktop_find_controls, and Qt publishes the prompt as Text controls,
    so the identity we need is right there. Read-only and background: no focus is taken.
    Returns "" on any failure - text we could not read must never become a misclassification.
    """
    if not hwnd:
        return ""
    try:
        import ab_client
        r = ab_client.call("desktop_find_controls", {"hwnd": int(hwnd)}, timeout=10) or {}
        data = r.get("data") if isinstance(r.get("data"), dict) else r
        ctrls = (data or {}).get("controls") or (data or {}).get("matches") or []
        return " ".join(str(c.get("name") or "") for c in ctrls if c.get("name"))[:400]
    except Exception:
        return ""


def classify_blocking_dialogs() -> list:
    """Return classified blocking dialogs currently open in Fusion.

    Each entry: {hwnd, title, category, resolution}. Empty list if none.
    Never raises — best-effort enumeration.
    """
    out = []
    try:
        info = get_fusion_window_info() or {}
    except Exception:
        return out
    for dlg in info.get("dialogs", []) or []:
        title = dlg.get("title", "")
        classified = _category_for_title(title)
        # A title-only match of "Fusion" tells us nothing. Fall back to the BODY TEXT, which
        # is where the identity actually lives for Fusion's generic-titled modals.
        if (not classified) or classified[0] in ("generic_modal_read_screenshot", "unknown"):
            body = _dialog_text(dlg.get("hwnd"))
            if body:
                by_text = _category_for_title(body)
                if by_text:
                    classified = by_text
                    dlg["matchedOn"] = "bodyText"
        if not classified:
            continue
        category, resolution = classified
        out.append({
            "hwnd": dlg.get("hwnd"),
            "title": title,
            "category": category,
            "resolution": resolution,
        })
    return out
