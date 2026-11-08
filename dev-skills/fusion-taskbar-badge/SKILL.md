---
name: fusion-taskbar-badge
description: The "Adom is driving Fusion" taskbar overlay badge - the COMPLETE recipe (asset, cross-process painting via ab desktop_taskbar, activity-tracked idle shedding, proof method) written verbosely so kicad-bridge (and every future app bridge) can copy it verb-for-verb. Read BEFORE touching handlers/taskbar_overlay.py or porting the badge to another bridge.
---

# fusion-taskbar-badge - the driving badge, end to end

## What it is and why (John, 2026-08-17, verbatim intents)

> "pup places overlay icons over my windows taskbar icons to indicate lots of state. for
> you, it would seem it would make sense to apply an overlay icon of the adom favicon to
> indicate to an adom user that 'adom is driving my fusion app'. i think that would be
> super rad."

> "make that adom favicon smaller, like 30% smaller, so the adom user sees more of the
> fusion 360 windows taskbar icon and just a small version of the adom favicon so they
> know 'slightly' that we are driving it"

> "after we're done controlling fusion, u should probably shed the active adom overlay
> icon badge like the way nb does it after like 5 or 10 minutes of inactivity"

The badge is a SMALL Adom brand mark on the corner of the app's Windows taskbar button,
visible only while Adom is actively driving the app. It answers, at a glance, "why is
Fusion busy?" - and disappears once Adom has been quiet for a while, so a lingering badge
never lies about who is in control.

## ⚠ THE TRAP that costs you a day: SetOverlayIcon is in-process-only

`ITaskbarList3::SetOverlayIcon` called from YOUR process on ANOTHER process's window
**returns S_OK and the shell silently ignores it**. No error, no badge. pup paid for
this lesson first (src/server.js: "an in-bridge SetOverlayIcon is in-process-only and
fails on the browser's out-of-process window") and we re-verified it here: a full ctypes
COM implementation, HRESULT 0, nothing rendered. Do NOT reimplement COM. Delete any
attempt you find.

**The working path: AD-core's `desktop_taskbar` verb (AD >= 1.9.106).** ab paints the
overlay natively, cross-process. The bridge just hands over the art:

```jsonc
// set (icon = base64 PNG, 32x32 canvas):
desktop_taskbar {"hwnd": <app main hwnd>, "overlay": {"icon": "<b64>", "tooltip": "Adom is driving Fusion"}}
// clear:
desktop_taskbar {"hwnd": <hwnd>, "overlay": {"badge": "none"}}
```

Success check: the response's output contains "Taskbar updated". The fusion
implementation calls it through `ad_client.call("desktop_taskbar", args, timeout=6)`
(the AD direct API; from a bridge process, no target arg = this machine).

## The asset (brand compliance - John rejected an improvised glyph)

- Source of truth: **pup's canonical overlay** `src/icons/pup-overlay-32.png` on
  adom/pup-bridge - the teal rounded square with the two dark Adom leaves. That IS the
  Adom favicon look; do not draw your own mark (the first attempt here was a white "a"
  on teal and John bounced it: "that overlay is not the adom favicon").
- Sizing: render the mark at **22x22 inside a TRANSPARENT 32x32 canvas, pasted at
  (10,10)** so it hugs the overlay slot's corner. This is John's "30% smaller" ruling:
  the app's own icon stays readable and the mark only *whispers* that Adom is driving.
  Regeneration snippet (PIL, run in the container):

```python
from PIL import Image
src = Image.open("pup-overlay-32.png").convert("RGBA")
mark = src.resize((22, 22), Image.LANCZOS)
canvas = Image.new("RGBA", (32, 32), (0, 0, 0, 0))
canvas.paste(mark, (10, 10), mark)
canvas.save("resources/adom-overlay-32.png")
```

- Ship the PNG in the bridge zip (`resources/adom-overlay-32.png`, listed in
  tools/release-files.txt). The helper base64s it once and caches.

## Lifecycle: activity-tracked, idle-shed (v1.9.58)

Three moving parts, all in server.py + handlers/taskbar_overlay.py:

1. **ON - every DRIVING verb.** `dispatch_command` calls `_note_driving_activity(command)`
   right after the prefix strip. It stamps `_LAST_DRIVE_AT` and fires a daemon thread that
   badges the app's main hwnd (found by largest-family-window). The set helper is
   THROTTLED (same hwnd within 60s = no-op) so per-verb calls never spam desktop_taskbar,
   and it runs async because a desktop_taskbar call can take seconds and must never slow
   a verb.
2. **PASSIVE verbs are excluded - this is the subtle part.** `_BADGE_PASSIVE` lists pure
   reads: readiness, state, window_info, describe, demo_progress, aps/mcp status, pref
   reads, screenshots, app-state reads. WHY: the Dashboard polls readiness every 15s and
   tour pollers hit demo_progress every ~2s - if those counted as "driving," the badge
   would NEVER shed. Driving = verbs that change or command the app, not verbs that look
   at it. When you add a new read-only verb, ADD IT TO THE PASSIVE SET.
3. **OFF - idle timeout in the janitor.** The 20s janitor tick checks
   `badge_active() and now - _LAST_DRIVE_AT > _BADGE_IDLE_SECONDS` (300s; John said
   "5 or 10 minutes", five keeps the taskbar honest sooner) and clears via
   `overlay:{badge:"none"}`. The badge is NOT cleared on envelope drain anymore - that
   produced per-verb on/off flicker, which reads as noise, not state.

Also badged at background-envelope BEGIN (fusion_detect) - harmless double-set thanks to
the throttle, and it catches the window the instant a window-raising verb starts.

Observability: `fusion_state` returns `drivingBadge: {active, lastDrivingVerbAt,
idleShedSeconds}` so an AI (or the daily suite) can assert the lifecycle.

## Proof method (how to SHOW the badge working - John's requirement)

A page/window screenshot never shows the taskbar. The proof recipe:
1. `desktop_screenshot_screen {}` via ab - the **`.full.png`** artifact carries the real
   desktop including the taskbar (the `.safe.png` is downscaled; use full).
2. Crop the bottom ~56px strip, find the app's icon (color-mask scan works: Autodesk
   orange = r>200, 60<g<160, b<80, constrained to the expected x-region), draw a RED BOX
   around it, upscale 3x, shotlog it.
3. Caveat: a box with taskbar AUTO-HIDE (ConfRoomROG) shows nothing - the strip reads as
   the app's own pixels. Verify on a box with a visible taskbar.

## Porting checklist for kicad-bridge

- [ ] Copy `handlers/taskbar_overlay.py` nearly verbatim; swap tooltip text
      ("Adom is driving KiCad") and the resources path.
- [ ] Use the SAME asset recipe (pup's mark at 22px in a 32px transparent canvas) -
      users should learn ONE badge across every Adom-driven app.
- [ ] Hook `_note_driving_activity` into your dispatch; build YOUR passive set (remember
      kicad_state and any web-demo pollers - the same never-sheds trap applies).
- [ ] Idle-shed from whatever periodic loop you already run (fusion uses its 20s janitor).
- [ ] kicad already has the always-on self-raise sentinel + focusEvents - consider logging
      badge set/shed into focusEvents so quietness AND badge lifecycle are assertable.
- [ ] Multi-window apps: badge the MAIN window's button; on a grouped taskbar button
      Windows shows the most recently updated window's overlay (pup's note).
- [ ] Prove with the red-box method before claiming it works. SetOverlayIcon lies.
