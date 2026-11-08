---
name: fusion-driving
description: How to SAFELY drive Fusion 360 through the Adom bridge across many steps without losing work - the discipline of reading the screenshot/owned-popup ARRAY after every mutating op and ANALYZING any dialog BEFORE the next step, never blind-dismissing, and never fullscreen-capturing (it forces foreground and disrupts the user). Includes the blocking-dialog catalog with the CORRECT response for each, and how the bridge now returns the dialog array + an analyze-this hint automatically. Read this BEFORE running any multi-step Fusion automation (attaching 3D, saving libraries, closing docs, modeling scripts). Trigger words - drive fusion, fusion dialog, blocking dialog, are you sure you want to close, packages are being uploaded, save was cancelled, ownedPopupCount, screenshot array, owned popups, fusion notification, lower right error, dismiss dialog, fusion modal, fusion_screenshot_all, fusion_state, fusion_check_dialogs, fullscreen capture fusion, analyze before proceeding, foreground vs background, does driving fusion foreground it, run fusion in the background, background screenshot, foreground fusion, focus steal, capture fusion without raising it, drive fusion while user works.
---

# Driving Fusion 360 safely (read the popup array, never fly blind)

Fusion is a desktop app you are operating headlessly. **Almost any mutating operation can pop a
modal dialog or a notification you cannot see unless you look** - and if you barrel ahead (or
blind-dismiss), you destroy work. This skill is the operating discipline that keeps that from
happening. It is the foundation under [fusion-libraries](../fusion-libraries/SKILL.md),
[fusion-multipart-libraries](../fusion-multipart-libraries/SKILL.md), and
[fusion-cloud-save](../fusion-cloud-save/SKILL.md).

## ⛔ The cautionary tale (why this skill exists, 2026-06-28)

Building a 10-part library, the loop force-closed generator docs that were **still uploading 3D
packages to the Fusion Hub**. Fusion popped *"Packages are being uploaded... if you close you will
lose these changes. Are you sure you want to close?"* The automation was sleeping through a blind
`dismiss_blocking_dialogs` loop, which clicked straight through it - **closing the library before it
ever saved. All 10 attaches were lost.** Every rule below is the fix for one beat of that failure.

## Background by default: what foregrounds Fusion, and what does not

The whole bridge is built so you can drive Fusion **while it stays in the background** and the user
keeps working in their foreground app. This is deliberate, and it works. The add-in runs *inside*
Fusion on its main thread and manipulates the document, view and workspace through the Fusion API,
which changes state **without activating or raising the window**. Captures use `PrintWindow`/WGC,
which grab the exact window even when it is occluded or minimized. So opening designs, switching to
the schematic/board/3D view, running scripts, exporting, and screenshotting are **all background**.

> ⚠️ Learned the hard way (2026-07-24): an AI refused to grab Fusion screenshots because it
> *believed* driving Fusion for a screenshot would foreground it and disrupt the user. **It does
> not.** Driving Fusion through the add-in + hwnd capture never raises the window. Do not repeat that
> mistake, and do not tell the user Fusion has to come to the front to be captured.

**Background (safe, never disturbs the user):**
- **Open files:** `fusion_aps_open`, `fusion_open_lbr`, `handle_open_cloud_file` / `_by_urn`.
- **Change view or workspace via the add-in:** schematic, 2D board, 3D PCB, Design/Manage,
  `fusion_show_2d_board`, `fusion_show_3d_board`. These switch through the API and do NOT raise the
  window.
- **Run the add-in:** `fusion_run_modeling_script`, `fusion_execute_text_command` / `electron_run`,
  every export, BOM, parameters, `fusion_attach_3d_package`, `fusion_close_document`.
- **Capture by hwnd:** `fusion_state` (dialogs as text plus the window capture), `fusion_screenshot_fusion`,
  `desktop_screenshot_window {hwnd}` (PrintWindow/WGC, occlusion-independent, works minimized).
- **Dismiss a dialog in the background:** `fusion_close_window {hwnd}` (WM_CLOSE via PostMessage, no
  focus steal).

**Foreground (raises Fusion / steals focus, use only deliberately):**
- **`fusion_start`** - launching Fusion brings it up. This is the one legitimate foregrounding action
  (it is the boot).
- **Synthetic keys and clicks.** The old `fusion_send_key` / `fusion_click_fusion` verbs are RETIRED (they
  answer `verb_retired` with the replacement): they called `SetForegroundWindow` and yanked Fusion to
  the front (caught 2026-06-29). Prefer the add-in API or `close_window`. When only a click will do, use
  ab: `desktop_ui_click {hwnd, name}` by accessible name, else `desktop_screenshot_window {hwnd}` then
  `desktop_click {hwnd, space:"image", shotId, x, y, cursor:false}`.
- **`desktop_screenshot_screen`** (whole-desktop capture) - screen-DC capture forces the target
  toward the foreground AND often grabs the wrong window. Never use it on Fusion (see Rule 2).
- Any explicit OS window activation (`SetForegroundWindow`, focus/raise verbs).

**The rule:** anything expressed through the **add-in API** or an **hwnd-targeted capture** is
background; only **synthetic input** (`send_key`/`click`), **whole-screen capture**, and the
**initial launch** foreground Fusion. Reach for the API path first; fall back to input injection only
when there is no API for it, and expect that to surface the window.

## Guard the active document with `expectDocument` (a tab-switch retargets your writes)

Every mutating verb acts on Fusion's **active document**, and a user switching tabs in Fusion
silently retargets it. A `fusion_run_modeling_script` meant for one design can land in another and
delete bodies out of it (a real near-miss, issue #289). So across a multi-step mutation, pass
**`expectDocument`** with the name you expect to be active:

```bash
adom-bridge fusion_run_modeling_script '{"script":"...","expectDocument":"Monitor arms v1"}'
```

If the active document is not that, the bridge refuses **before executing** and returns
`errorCode: "wrong_document"` with `expected`/`actual`, instead of mutating the wrong file. It is a
**name** check, so if the doc was renamed mid-session, pass the new name. Honoured on every mutating
verb: `run_modeling_script`, `execute_text_command`, `electron_run`, `close_document`,
`close_all_documents`, `import_step`, `set_parameter`, `save_to_cloud`, `delete_cloud_file`.

## Rule 1 - after EVERY mutating op, READ the popup array. Then analyze. Then proceed.

A mutating op = anything that changes Fusion's state: `fusion_run_modeling_script`,
`fusion_execute_text_command`, `fusion_close_document`, `fusion_close_all_documents`,
`fusion_save_lbr`, `fusion_attach_3d_package`, `fusion_open_lbr`, any export, any `electron_run`
that writes.

After it returns, you MUST inspect what owned popups are now up before doing the next step. Three
ways to get the array, in order of preference:

1. **The bridge does it for you (default).** Mutating `fusion_*` verbs now return
   `dialogsDetected`, a `dialogs[]` array (each with `title`, `category`, `resolution`, and a
   `screenshot` path you can Read), and a top-level `_hint`. **When `dialogsDetected > 0, STOP and
   analyze the dialogs[] before your next call.** Never ignore that hint. (See "How the bridge
   enforces this" below.)
2. **`fusion_check_dialogs`** (or `fusion_state`) - every open dialog as DATA: title, text, buttons and
   hwnd, read through UI Automation. Read the text first; a screenshot is the fallback, not the start.
3. **`desktop_screenshot_window {"hwnd": <fusion-hwnd>}`** (ab core) - returns `screenshots[]` and
   **`ownedPopupCount`**. `ownedPopupCount > 0` means dialogs are stacked on the window. Read the
   shots. Get the hwnd from `fusion_state {screenshot:false}` (its `hwnd` field) or `desktop_find_window
   {"titleContains":"Autodesk Fusion"}`.

**To actually SEE a shot - mind WHERE it lands:**
- ab core `desktop_screenshot_window {hwnd}` saves the PNG **into your container** (e.g.
  `/tmp/adom-desktop-screenshots/`) - Read it directly.
- The bridge's own captures (the auto `dialogArray[].screenshot` paths and `fusion_screenshot_fusion`)
  are saved on the **Windows box** (`%TEMP%/adom-fusion-shots/`) and, since 1.10.38, pulled to YOU automatically: read `localFullPath` / `localSafePath` in the reply. On older bridges, re-capture that
  hwnd with `desktop_screenshot_window {hwnd}` (lands in your container) or `desktop_pull_file` it over.

Either way, LOOK - do not describe it from memory.

## Rule 2 - NEVER fullscreen-capture. Always target the window by hwnd.

- ⛔ **Do NOT use `desktop_screenshot_screen`** (whole-desktop capture). It grabs whatever is in the
  foreground (often the user's browser, not Fusion), and screen-DC capture paths force the target
  toward the foreground - **which disrupts the user, who may be doing something else.** It also
  routinely captures the WRONG window.
- ✅ **Always capture a specific `hwnd`** (`fusion_state`, `fusion_screenshot_fusion`,
  `desktop_screenshot_window {hwnd}`). These use `PrintWindow` / WGC and capture the **exact window
  in the BACKGROUND** without bringing it forward. Fusion keeps running behind whatever the user is
  doing. This is the only acceptable way to look at Fusion.

## Rule 3 - NEVER blind-dismiss. Classify first, then choose the RIGHT action.

A dialog has a correct answer that depends on what it says. Clicking the default can be catastrophic
(clicking "Yes" on *"sure you want to close, you'll lose changes?"* = data loss). So:

1. Identify the dialog (the `category`/`resolution` from `dialogs[]`, or Read its screenshot).
2. Choose the action that PRESERVES work, from the catalog below.
3. Only use a generic dismiss as a LAST resort, and only for dialogs you have confirmed are safe to
   cancel/escape.

To act on a specific dialog, target its `hwnd`: ab `desktop_ui_click {"hwnd":<dlg>, "name":"Cancel", "role":"button"}`
for a named button, or `fusion_close_window {"hwnd":<dlg>}`. The bridge also expires unknown dialogs by
itself before the next add-in verb, so most of the time there is nothing for you to do.

### ⛔ Dismiss in the BACKGROUND - `close_window` (WM_CLOSE), NEVER `send_key`/Escape

When you DO dismiss a dialog, use **`fusion_close_window {"hwnd":<dlg>}`** (WM_CLOSE via PostMessage =
Cancel/No). It dismisses **without stealing focus**. ⛔ **Do NOT send Escape to expire
a dialog** - a synthetic key calls `SetForegroundWindow` and **YANKS Fusion to the foreground**, disrupting
the user (caught 2026-06-29: an Escape pulled Fusion to the front mid-task). WM_CLOSE is also more
reliable than Escape on Qt dialogs. The bridge's own auto-dismiss paths (`_dismiss_dialogs_bg`, the
long-command pre-dismiss) all use `close_window` for exactly this reason. Background screenshots +
background dismiss = the user never sees Fusion jump around.

## The blocking-dialog catalog (title substring -> what it means -> do THIS)

| Dialog (title/body contains) | Meaning | Correct response |
|---|---|---|
| **"packages are being uploaded", "lose these changes", "are you sure you want to close"** | A doc is closing while 3D packages still upload to the Hub. Closing = **lose the packages + unsaved bindings**. | Click **No**. WAIT and poll until the upload finishes (see [fusion-cloud-save](../fusion-cloud-save/SKILL.md)), THEN close/save. NEVER click Yes. |
| **"cannot be saved while packages are being uploaded", "save was cancelled"** | You tried saveAs mid-upload; Fusion refused. | Not fatal. Wait for the Hub upload to finish, then retry the saveAs (poll past it). |
| **"needs to update", "update is available", "software update"** | Fusion wants to self-update. | Do NOT auto-confirm (it can update mid-automation). Clear on the desktop or let it finish, then retry. A pending update also causes add-in/host version-drift crashes. |
| **"select electronics design", "linked to multiple"** | A file links to multiple electronics designs; CEF picker. | Its list is NOT Win32/keyboard navigable. Open the specific design by URN/API instead, or select on the desktop. See [fusion-electronics](../fusion-electronics/SKILL.md). |
| **"recovery", "recover unsaved", "document recovery"** | Document Recovery prompt. | `fusion_dismiss_recovery`, or `fusion_relocate_recovery` BEFORE `fusion_start` to prevent it. |
| **"save changes", "unsaved changes", "do you want to save"** | Unsaved-changes prompt on close. | Use `fusion_close_document` (closes without the save modal). Only Escape/Cancel if you mean to keep editing. |
| **"what do you want to design"** | Fusion's start picker (no doc open). | Nothing: `fusion_start` dismisses it, and the bridge expires it before the next add-in verb. `fusion_close_window {hwnd}` if you must. |
| **"create new symbol/footprint/device '<name>'?"** | An `EDIT <name>.sym/.pac/.dev` was run with a name Fusion can't find as that object - usually a DEVICESET name passed to `.sym`/`.pac` whose symbol/package is named by the shared combo (e.g. `R-0603`, not `R-0603-10K`). | Dismiss in the BACKGROUND (`fusion_close_window {hwnd}` = No - never create a stray). Then re-`EDIT` with the correct symbol/package name. `fusion_capture_library_views` now auto-handles this. |
| bare **"Fusion360"** modal during an open | Usually the linked-design picker. | `fusion_screenshot_fusion` to confirm, then handle as the picker. |

(The bridge classifies these in `handlers/dialog_classify.py`. When you discover a new one, add a
rule there with its correct response so every future session handles it right.)

## Rule 4 - the lower-right notification tray (errors/warnings you keep missing)

Fusion shows non-modal toasts in the **lower-right** ("1 warning(s), 1 error(s)..."). They are NOT
modal dialogs, so dialog enumeration may not flag them - but they tell you an operation half-failed.
They DO render in an hwnd-targeted main-window screenshot. So after a risky op, also glance at the
main-window shot's lower-right, not just the popup array. If you see an error count, open the
notification center and read it before continuing.

## Rule 5 - don't DESTABILIZE Fusion (it crashes more easily than you think)

Long automation runs take Fusion DOWN if you abuse it - learned the hard way over a multi-hour run
where Fusion died repeatedly. The culprits:

- **Blanket "close all documents" loops** (`while app.documents.count>0: app.documents.item(0).close(False)`)
  run before every op to "reset state". This churns Fusion hard and can crash it or leave it wedged.
  Close ONLY what you need (`fusion_close_document`, or close the one stray doc), never everything.
- **Back-to-back heavy operations** - import + saveAs + Create3DPackage in a tight loop, dozens of
  times, plus repeated `bridge_install` respawns - destabilize the session.

So: minimize document churn, do NOT reset-by-closing-everything, save your work to the cloud as you go
([fusion-cloud-save](../fusion-cloud-save/SKILL.md)) and **reopen from the cloud** after a reset (the
in-session view is often wedged). After a `bridge_install` respawn Fusion can report "not running"
even when the process is alive - a `fusion_start` re-establishes the connection. And if Fusion dies
mid-run, that was almost certainly YOU, not the user - own it, don't blame them.

## How the bridge enforces this (so you cannot miss it)

`_post_open_screenshot()` has always auto-captured the main window + every dialog after *open*
commands and told the AI to read them. That same capture now fires after **all mutating verbs** via
`_capture_dialog_array()`, which:

- cheaply enumerates dialog windows (the same read `fusion_state {screenshot:false}` gives you: title, text and buttons through UIA - no screenshot
  when nothing is up),
- suppresses false positives: a docked panel (Browser/Timeline) shares the "Fusion360" title and can
  slip past the size filter, but it does NOT disable the main window. So a generic-titled candidate is
  only surfaced when the main window is actually disabled (`mainWindowEnabled:false` - a real modal is
  up) or it matches a known dialog by title. That keeps the hint from crying wolf on every op,
- on detection, classifies each (`classify_blocking_dialogs`) and screenshots it by hwnd
  (`screenshot_hwnd`, background `PrintWindow`),
- returns `dialogsDetected` + `dialogs[]` + a top-level `_hint` that says **ANALYZE these before
  proceeding; do NOT blind-dismiss**.

You can also call **`fusion_check_dialogs`** at any time to get the current array on demand (e.g.
while polling a long upload). The point: the array and the hint come back to you automatically - your
job is to actually READ them and act, every time.

## The loop that should be muscle memory

```
do mutating op
  -> read dialogsDetected / dialogs[] / ownedPopupCount in the response (or fusion_check_dialogs)
  -> if a dialog is up: Read its screenshot, look it up in the catalog, take the work-preserving action
  -> glance at the main-window shot's lower-right for an error toast
  -> only then do the next op
```

Slow is smooth, smooth is fast. The one time you skip the look is the time you lose the library.

## Pitfall: sketch.modelToSketchSpace() does not project onto the sketch plane

(Oliver, 2026-07-23 session.) `modelToSketchSpace()` transforms coordinates; it does NOT
project the point onto the sketch plane. A wrong off-plane coordinate passes straight
through into the resulting geometry, silently, offset by exactly the plane offset. This
caused a real bug: a body landed at the wrong Z with a perfectly clean-looking render.
Verify geometry with `fusion_inspect_bodies` (bbox in mm) instead of trusting the viewport.

## Fusion not running is a STEP, never a refusal

Verbs auto-launch Fusion when it is closed, folding the launch into the ETA, so "Fusion 360
is installed but not running" should never reach a user. If a launch genuinely fails you get
`fusion_autostart_failed`, which states the command was never run and points at
`needsSignin` (Autodesk sign-in is a human step, not a bug).

The one state you must ASK about rather than fix silently: `addin_restart_required` means the
add-in was installed while Fusion was already open. Its payload carries `userMessage` plus
`openDocuments`/`unsavedDocuments` so you can ask, then `fusion_restart_for_addin {save:true}`.

For a costed plan before any of it, `fusion_plan_library_import` reports state and timings
without doing work.

## Verifying a palette: read the POPUP array, never the main frame

Hard-won on 2026-08-28. Fusion draws every palette (including its own Text Commands panel
and the Adom activity banner) as an OWNED POPUP WINDOW. A capture of the main Fusion hwnd
omits them by design, so a palette that Fusion reports as visible, docked and sized will be
absent from the main frame - and it is not lying. I spent an hour concluding "no HTML palette
renders in this build" from main-frame screenshots while the palette was painting perfectly
the whole time. The evidence was in `screenshots[]` (ownedPopupCount > 0) all along.

Rule: to verify anything a palette shows, call `desktop_screenshot_window {hwnd}` and read
`screenshots[]`. Also: the hwnd CHANGES on every Fusion restart - re-read it from
`fusion_state` or the capture returns "Window is not visible".

## A probe that rides a gated verb reports a default, not the truth

Watching a real import with two witnesses (2026-08-28): the popup capture showed the Adom
banner on screen while a `run_modeling_script` probe of Fusion's palette registry said
`exists: False` on every poll. The probe was not wrong about Fusion - it was being REFUSED by
the busy gate during bind/prove and my parser turned the refusal into a default. When two
instruments disagree, check which one can even reach the thing before believing either.
Anything that must read state DURING a long stage needs to be in BUSY_EXEMPT_COMMANDS, or
it is measuring the gate, not Fusion.

## When the window capture is blank or stale (Design documents)

A background capture of a GPU Design viewport can come back blank or a stage behind while the
model is fine. `fusion_state {}` therefore reports three separate things: `windowPixels`
(`blank:true` above 97 percent one colour), `geometry.bodies` (the API body count, independent
of any capture) and `nativeViewport.path` (the frame Fusion itself renders through
`activeViewport.saveAsImageFile`, no foreground). Trust the native frame for geometry;
`{viewport:"native"}` skips the window capture. Never foreground Fusion to fix a capture.

## When a dialog is in the way

The bridge reads every dialog through UIA, answers the ones it has a rule for, sends WM_CLOSE
(Cancel or the X, never a save or a discard) to the rest, and runs your verb. A dialog that
survives comes back as `blocked_by_dialog` with `dialogs[]` (text and button names): click the
right button with `desktop_ui_click {hwnd, name}` and re-run. `dialog_expired_retry` means the
box is gone, call again. The bridge never restarts Fusion over a dialog, and neither should you.
