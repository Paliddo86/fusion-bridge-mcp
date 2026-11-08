---
name: fusion-background-etiquette
description: The invariant every code change must preserve - Fusion windows NEVER steal the user's foreground; every window-raising verb runs inside the refcounted background envelope, and the daily verb tests assert focus invariance
---

# fusion-background-etiquette - Fusion never steals the foreground

**The doctrine (John, 2026-08-15, verbatim):**

> all of your verbs should do their work in the background if they can, meaning its rude
> to force foreground a fusion window cuz a user using your bridge is doing other ai
> thread prompts and typing while you do your work. so make sure all verbs try to work in
> the background when they open windows. i know sometimes you have to foreground (fg) to
> do ui stuff that can only be done in the fg, but its rude. so you have to try super
> hard to figure out how to do it in the background, and then in the worst cases ONLY
> then do it in the foreground. but of course always let the user choose to fg a window
> if they want to cuz they may want to look at what you're doing.

**The invariant (John, 2026-08-14):** a user is typing in their AI threads while this
bridge drives Fusion. Any window this bridge causes to appear - the app itself, a
document, a picker, a recovery dialog - must arrive in the BACKGROUND. Screenshots,
exports, view switches, and builds all work on background windows (PrintWindow renders
occluded windows fine), so there is no functional excuse for a focus steal. A stolen
foreground mid-keystroke is the single most user-hostile thing this bridge can do.

## The foreground CONTRACT (John, 2026-08-16, after ab's demo run stole his focus)

> you can allow the ai to foreground fusion, but you have to make it give you a reason
> and then you should show the reason for 3 seconds as an ab caption so the user knows
> why its being foregrounded and then can yeild. you should be returning hints to the
> caller, i.e. the ai, that you are opening these fusion windows in the background so
> it knows.

Implemented in dispatch (v1.9.51), and every future window-raising verb MUST keep it:

1. **`foreground:true` alone is REFUSED** (`errorCode: foreground_reason_required`).
   The caller must pass `foregroundReason:"<user-facing sentence>"`.
2. **The reason is shown as an ab caption for ~3 seconds BEFORE any focus change**
   (ad_client.notify + sleep(3)), so the user knows why and can yield.
3. **Background runs annotate the RESPONSE**: `windowEtiquette:"background"` plus a hint
   telling the calling AI no focus was taken, to render via fusion_state, and to never
   "helpfully" foreground afterward. The AI learns the etiquette from the verb itself.

## The envelope-expiry trap (the bug behind John's 2026-08-16 report)

The tour armed ONE envelope sized `estimate+60s` at start - but real tours ran 485-890s
(cold boots, dialog stalls), so the envelope EXPIRED mid-run and every later doc open
stole the foreground while the user typed in other threads. Rules:

- **Long orchestrations RE-ARM the envelope per step** (`begin_background_envelope(dur*3+240)`
  each loop iteration - nesting is refcounted, re-arming is free). Never size one envelope
  off an estimate that a slow cold boot can blow through.
- **Direct orchestrator calls bypass dispatch's envelope** (_orchestrate_*, _show_library_view,
  _test_project_activate called from inside another verb) - the CALLING orchestration owns
  the envelope for them.

## The escalation ladder (exhaust each rung before the next)

1. **API / text command** - the add-in API, EAGLE text commands, and modeling scripts
   need no window at all. Most "UI" work is really this.
2. **Background window driving** - hwnd-targeted PrintWindow capture, UI Automation
   (find_control / ui_click / ui_set), and PostMessage/SendMessage input all work on
   windows at the BOTTOM of the z-order. A thing being "in the UI" does not make it a
   foreground thing. The KiCad bridge proved the strong form (John, 2026-08-15): it
   reads the app's REAL pulldown menus and drives them with PostCommand/WM_COMMAND
   instead of foreground+SendInput, "and now kicad works better." Our equivalents:
   `send_key_background` (PostMessage WM_KEYDOWN/UP, the DEFAULT for fusion_send_key
   since 1.9.19) and `_dismiss_dialogs_bg` (WM_CLOSE). SendInput is the explicit,
   captioned exception (`method: "sendinput"`), never the default.
3. **Background + demotion envelope** - if the operation makes Fusion raise windows or
   dialogs, run it inside the envelope so every grab is denied/demoted (incl. the
   drain sweep below).
4. **Foreground, the documented last resort** - ONLY when input genuinely requires
   focus (real keystrokes into a focused field that UIA cannot set). Every such verb:
   (a) is a conscious, documented exception here, (b) announces itself with an
   on-screen caption first, (c) HANDS THE FOREGROUND BACK after. Rule (c) is not
   optional plumbing: `_snapshot_foreground()` before the take,
   `_handback_foreground(prev, taken_hwnd)` after the input lands (handlers/
   fusion_ui.py) - taking focus for an ESC and never restoring it is as rude as the
   steal (John caught send_key doing exactly this live, 2026-08-15).

**The user can ALWAYS choose foreground:** every window-raising verb accepts
`foreground: true` - the dispatcher then skips the envelope entirely so the user can
watch. Their choice is the only non-rude foreground.

**Standing audit item:** the legacy deliberate-foreground exceptions (prefs_open,
mcp_enable, demo sign-in leg) predate rung 2 - re-audit each against background UIA
before trusting its exception status.

## The mechanism: the background envelope

`fusion_detect.begin_background_envelope(seconds, minimize=False)` - refcounted, so
overlapping verbs nest and only the last drain restores the user's settings. It does
three things for its duration:

1. DENIES grabs up front: raises the foreground-lock timeout AND calls
   LockSetForegroundWindow for the envelope's duration (Fusion self-activates on every
   document/workspace switch; before the lock, each one FLASHED for the instant before
   the watcher demoted it - John counted three in one proof run, 2026-08-15). Both are
   best-effort; the watcher below is the backstop that always works.
2. Disables the minimize animation, so any demotion is an instant vanish.
3. Runs a watcher: any Autodesk-FAMILY window that still reaches the foreground is
   demoted. Two modes:
   - `minimize=True` (launch only): splash / seat / launcher windows are minimized away.
   - `minimize=False` (everything else): the window is pushed to the BOTTOM of the
     z-order WITHOUT minimizing - a minimized window renders stale in PrintWindow, and
     background captures must keep working mid-operation.

The watcher is surgical two ways: it acts only when the CURRENT foreground window
belongs to the Fusion family, AND it stands down whenever real keyboard/mouse input
happened in the last 2 seconds (GetLastInputInfo) - because a foreground change right
after user input IS the user. This second rule was added 2026-08-15 after the watcher
demoted John's own clicks and he could not raise Fusion at all; "never fights a
deliberate user" was a false claim until the input guard made it true.

**Native modal dialogs are a hard NO from automation.** ImportSCHAndBRDCmd's Open
dialogs wedged the user's Fusion (queued dialogs, disabled main window, uncancellable
by the user because the watcher buried them). Any flow that would pop a native dialog
must use the dialog-free primitive instead (Document.newDesignFromLocal covers .lbr,
.brd with placed elements, and .sch), or be a captioned, deliberate-foreground manual
verb. Recovery ritual if one ever wedges again: desktop_screenshot_window {hwnd} to see
the popups + their hwnds, then desktop_ui_click {name:"Cancel", scope:"popups"} per
queued dialog - all background.

## Where it is armed

`dispatch_command` in `server.py` holds `_WINDOW_RAISING` - the set of verbs that can
make a window appear (opens, view switches, imports, builds, exports-with-dialogs,
`start`). Every dispatch of one of those arms the envelope for the verb's timeout + 20s.

**Deliberate-foreground verbs are excluded on purpose** and must stay excluded:
`prefs_open`, `mcp_enable`, `demo` - they need real clicks on screen, and they announce
themselves with an on-screen caption first (see the fusion-driving skill's etiquette).

## The rule for every future code change

- New verb that can raise a window? **Add it to `_WINDOW_RAISING` in the same commit.**
- New verb that deliberately takes the foreground? It must (a) be a conscious exception
  documented here, (b) announce itself with a caption before touching the screen, and
  (c) take a `foregroundReason`-style justification if a caller can trigger it remotely.
- Never "fix" a focus problem by foregrounding Fusion. Fix it by making the operation
  work in the background - that is always possible with hwnd-targeted capture and UIA.

## The own-windows-only ruling (John, final, 2026-08-15 - via the kicad bridge)

The background contract got a cross-bridge design ruling: a bridge may only ever
manipulate ITS OWN app's windows. Never capture-restore-raise-demote any OTHER window,
and especially never a user's window - even "helpfully". This bridge's envelope is
compliant (it filters on the Fusion family pids), and it must STAY that way: any future
demotion/restore logic keeps the family-pid filter, no generic "whatever is foreground"
handling. The kicad bridge's guardian additionally uses WS_EX_NOACTIVATE + a
minimize-bounce (the OS returns focus naturally) - see the kicad-bridge-background
skill for that variant; worth considering here if the demote-to-bottom approach ever
proves insufficient.

Related rulings the same day: no terminal blips from shelled tools (the kicad repo's
quiet_console.py pattern: process-wide CREATE_NO_WINDOW + -NoProfile -NonInteractive +
explicit exit codes), and focus-requiring inputs hand the user's foreground BACK after
the input lands (shipped here in v1.9.16).

## The drain-sweep rule (v1.9.11, caught live 2026-08-15)

A modal dialog can pop AT the envelope's deadline and keep the foreground it grabbed
after the watcher dies - the Package3D Save dialog did exactly this during the attach
flow while John watched. Two rules came out of it:

- The watcher never walks away mid-steal: at its deadline it runs a 20s GRACE SWEEP,
  continuing to demote as long as a family window still holds the foreground.
- A multi-minute orchestrator that raises dialogs (attach_3d_package and anything like
  it) arms its OWN long envelope at entry (600s) instead of relying on the dispatch
  envelope, whose duration is sized to the verb timeout, not to when the dialogs appear.

And the answer to "do we need the foreground to drive the UI?" is NO, ever: text
commands, the add-in API, and PrintWindow captures all work on background windows. Any
foreground appearance is a bug in this envelope, not a requirement of the operation.

## How regressions get caught

The daily verb test suite (dev-skills/fusion-verb-tests) includes a focus-invariance
check: it records the foreground window before the document-flow stage and fails the
run if a Fusion-family window holds the foreground afterwards. A focus steal is a
failing test from now on, not an anecdote a user has to report.

## History, so the lesson keeps its teeth

The launch path got this treatment first (guard built for `fusion_start`). It then sat
with exactly ONE caller while every cloud open and dialog ran bare - and users kept
losing their foreground to document opens for weeks until John called it out during a
test run. The envelope exists so protection is the DEFAULT at the dispatcher, not a
per-call courtesy someone has to remember.
