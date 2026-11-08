---
name: fusion-autodesk-signin
description: Drive the first-run Autodesk sign-in that blocks a freshly installed Fusion 360, from a headless/remote Adom desktop (pup/adom-desktop). Covers the whole browser OAuth handoff and every pitfall that cost hours on a fresh Hyper-V VM — blank sign-in webview, invisible Edge windows, warm Google/Apple/Microsoft SSO, the Autodesk Identity Manager protocol handoff, fast-expiring sign-in codes, and escalating the password/2FA step to the human without ever typing it. Trigger words — fusion signing in, welcome to fusion, sign in - autodesk, needsSignin, fusion stuck signing in, autodesk oauth, fusion login blocked, drive fusion sign in, fusion sign in on fresh box.
---

# Fusion 360 Autodesk sign-in — the drive-it playbook

A freshly installed Fusion launches to a first-run **Autodesk sign-in** that BLOCKS the main thread,
so the AdomBridge add-in never serves and `fusion_readiness` reports `ready:false, needsSignin:true`.
This skill gets you from that wall to `ready:true`. It is the durable record of a live drive on a
fresh Hyper-V VM (John, 2026-07-14); every step below is a thing that actually went wrong once.

**The one hard rule:** you NEVER type the user's Autodesk/Google password or 2FA. You may type their
**email address** (not a secret) and click every button. When a real secret is needed, you escalate
to the human with `fusion_notify_owner` and let THEM type it.

## Where you are (confirm first)
`fusion_readiness` → `installed:true, running:true, ready:false, needsSignin:true`. Since bridge 1.10.1
any add-in verb sent in this state answers `errorCode: needs_signin` within about a second (the gate
reads the window title before probing the add-in), so a `needs_signin` reply is the same signal:
resolve the sign-in, do not restart Fusion or diagnose the add-in. The Fusion window
title is "Signing in - Autodesk Fusion" or "Welcome to Fusion". Get its hwnd (Get-Process Fusion360
→ MainWindowHandle) and `desktop_screenshot_window {hwnd}` to see it. `desktop_screenshot_window` uses
PrintWindow, so it captures backgrounded/occluded windows and their owned child dialogs — use it, not
a screen grab.

## Step 0 — ASK the user before you drive (AskUserQuestion)
Sign-in needs a human decision, so ask up front — do NOT silently start clicking:
- **Do they have a warm browser session** (Google / Apple / Microsoft) for the account **linked to
  their Autodesk login?** If yes, "Continue with <provider>" clicks through with no password.
- **On a truly fresh box there is usually NO warm session** — the provider will demand a password.
  Confirm they're OK finishing the password/2FA step themselves (you'll toast them when it's time).
- Offer the choice: you drive the clicks and escalate only the secret step, OR they'd rather remote
  in and do it. Respect their answer.

## Step 1 — Blank white webview? RESTART Fusion.
The very first launch after install often renders the sign-in webview **blank white** (just a title
bar, no button). Do not fight it: `fusion_stop` then `fusion_start`. After a clean relaunch it renders
"Welcome to Fusion" + a real **Sign In** button. (This single restart unblocked the whole flow.)

## Step 2 — Click Fusion's "Sign In" (it opens the browser)
The sign-in webview exposes **0 accessibility controls** (it's Chromium), so click by IMAGE
coordinates: `desktop_screenshot_window {hwnd}` returns a `shotId`; then
`desktop_click {space:"image", shotId, x, y, hwnd}` — passing `hwnd` FOCUSES that window first, which
is essential (a click without it lands on whatever is foreground). Maximize the window first
(`desktop_set_window_state {hwnd, state:"maximize"}`) so the button is unambiguous. Fusion says "We
will take you to your web browser to sign in" and opens the **default browser** (Edge) to the OAuth.

## Step 3 — Find the Edge OAuth window (it IS there)
The Edge window can look invisible: `Get-Process msedge` shows background procs and your window
enumeration may print nothing. That "nothing" is almost always **PowerShell CLIXML progress noise
eating your output**, NOT a missing window. Fixes:
- Put `$ProgressPreference='SilentlyContinue'` at the TOP of the PS script.
- Enumerate `Chrome_WidgetWin` class windows (the OAuth tab is titled "Sign in - Autodesk"); a 2-tab
  window reads as "... and 1 more page".
- Then `desktop_screenshot_window {hwnd}` the Edge window — PrintWindow renders it regardless.
- The OAuth URL is handed to Edge over IPC, so it is NOT in any msedge command line — don't look there.

## Step 4 — Drive the OAuth (warm SSO path)
On the Autodesk sign-in page (`signin.autodesk.com/?flowId=...`): click **"Continue with Google"**
(or Apple/Microsoft, matching the linked account). On the provider page, click the email field and
`desktop_type` the user's **email** (e.g. john@adom.inc — an address, not a secret), click **Next**.
- If the provider recognizes a warm session → it proceeds with no password. 
- If it shows **"Enter your password"** or a 2FA challenge → **STOP**. This is the human wall.

## Step 5 — Escalate the secret step (never type it)
`fusion_notify_owner {title:"Fusion sign-in needs you", body:"On <box> Fusion is at the Google
password step for <email>. Please enter your password/2FA in the open browser; I'll finish once
you're in."}`. It toasts the user's MAIN computer, fanning out cross-ab so it reaches them even though
this bridge runs on an unattended VM. Then WAIT and poll `fusion_readiness`; do not re-toast for a few
minutes. (The user may instead remote into the box and type it — either way you take over after.)

## Step 6 — The handoff back to Fusion (Autodesk Identity Manager)
After auth, the browser lands on `signin.autodesk.com/idmgr/callback#code=...` ("You're signed in").
The code is delivered to Fusion by the **Autodesk Identity Manager** protocol, so Edge pops a
**"This site is trying to open Autodesk Identity Manager"** overlay: tick **"Always allow"** and click
**"Open"**. (Always-allow means later sign-ins skip the prompt.) If instead you see a big
**"Open Product"** button, click it.

## Step 7 — SPEED: the code expires (~2 min)
The single biggest time-sink on a slow/remote box: each screenshot+click round-trip can take 20-30s,
and the Autodesk sign-in code **expires in about 2 minutes** ("Sign-in request expired"). If you dawdle
across the tabs, it dies and you loop forever. Mitigations:
- Do the fewest possible round-trips between clicking Fusion "Sign In" and the callback.
- Once the Autodesk/browser session is warm (after the first successful auth), a fresh Fusion
  "Sign In" click completes almost instantly — so if a code expires, just click Fusion "Sign In"
  AGAIN for a fresh code; you don't redo the whole login.
- If Edge tabs get stuck on a stale expired-callback URL, clear them (`taskkill /F /IM msedge.exe` —
  browser-specific and safe; Autodesk cookies persist so it stays warm) and re-click Fusion "Sign In".

## Step 8 — Confirm
Poll `fusion_readiness` until `ready:true` (needsSignin clears, the add-in responds). Now Fusion is
drivable: `fusion_open_lbr`, `fusion_build_library_3d`, exports, GPU/graphics checks, etc.

## Escalation ladder (in order)
1. Warm-SSO click-through (no secret) — try first.
2. Type the user's EMAIL only; let the provider's warm session finish it.
3. `fusion_notify_owner` for a genuine password/2FA/UAC/physical wall — the ONLY time a human is needed.
4. Ask the user to remote in — last resort if toasting doesn't land.

## Related
- `fusion-onboarding` — the install that precedes this (streams Fusion, then hands off here).
- `fusion-bridge-dev` — how the bridge detects installed/installing/ready (Fusion360.exe completeness).

## ⭐ FIRST RULE FOR ANY BROWSER AUTOMATION: install + use ABE (nbrowser), don't coordinate-click (John, emphatic 2026-07-14)

The MOMENT a task needs you to DRIVE a browser (the Autodesk OAuth, the APS portal at
aps.autodesk.com/myapps, any web form), your FIRST move is to **recommend and install the
adom-browser-extension (ABE / nbrowser)** and drive via `nbrowser_*` (JS-level: read the DOM, click by
selector/text, reliably). Do NOT limp along with `desktop_click`/SendKeys coordinate-driving — on a VM
that Edge crashes, tabs multiply, buttons sit at the nav edge and clicks miss, and you waste enormous
time (learned the hard way here). Tell the user up front: "I need to install ABE to automate this
browser reliably." Install: `bridge_install {manifestUrl: .../adom-browser-extension/files/native-browser-bridge-manifest.json}`,
then the `installing-the-extension` flow, then `nbrowser_readiness` until `state:"ready"`. Coordinate-
driving is the fallback of last resort, never the default.


## AUTOMATED NOW: the verb self-drives this whole playbook (v1.9.32)

Everything above is what `fusion_signin` DOES ITSELF as of 1.9.32 - the manual playbook
remains as the fallback and the explanation. The contract for a calling AI:

1. Call `fusion_signin {}` (add `email` to override; the target profile's email is used
   otherwise). It SELF-STARTS Fusion if needed, clicks Sign In for the user, captures
   the OAuth URL, re-opens it in the correct profile, TYPES THE EMAIL and advances the
   page (background UIA, no focus steal), and classifies the remaining wall.
2. Walls you can get back, in `signinWall`:
   - `password` - the ONE human secret step. The user was ALREADY TOASTED with "just
     your password". Do nothing except poll `fusion_readiness`.
   - `twofactor_email_code` - fetch the code from Gmail (adom-google) and call
     `fusion_signin_2fa {code}` yourself; only fall back to the human if you cannot.
   - `protocol_handoff` - the browser asks to open Autodesk Identity Manager; the user
     was toasted to click Open.
3. NEVER re-toast, never ask the user for secrets, never re-drive an advanced page -
   the response `_hint` tells you exactly what remains.
4. The demo tour integrates this: on a signed-out box it drives sign-in to the wall,
   toasts, and fails with instructions - rerun the tour after the human's one step.

The philosophy (John, 2026-08-16): the code automates EVERYTHING automatable and
notifies the human at exactly the moment they are needed - they let this run in the
background, so the toast IS the interface. Hints teach the AI; the AI never improvises
clicks on auth pages.
