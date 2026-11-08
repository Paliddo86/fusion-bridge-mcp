---
name: fusion-multiprofile-signin
description: >
  Fix Fusion 360 signing into the WRONG Autodesk account because it fires its OAuth at the OS-DEFAULT
  browser. Fusion opens sign-in in whatever browser is default (often a personal Chrome profile), but
  a user's Autodesk seat usually lives in a DIFFERENT profile (their work Google/SSO). The bridge's
  fusion_signin verb reads the full authorize URL Fusion wrote into that wrong browser's history and
  re-opens it in the RIGHT profile, where it completes back to the running Fusion. Trigger words -
  fusion wrong browser, fusion wrong account, fusion signs into wrong profile, fusion opens personal
  chrome, autodesk wrong profile, multiple chrome profiles autodesk, fusion_signin, fusion sign in
  wrong google account, fusion default browser oauth, authenticate fusion multiple profiles.
---

# Signing Fusion into the right browser profile (fixing Fusion's dumb OAuth)

## The problem (Fusion's fault, we work around it)

Fusion's desktop sign-in fires an OAuth URL at the **OS-default browser**. If your Autodesk account
lives in a *different* Chrome/Edge profile - personal Chrome is default but your seat is
`you@work.com` in your work profile - Fusion signs you into the wrong account or stalls on the wrong
Google session. Autodesk should let you pick the profile; they don't. So we do.

Verified live (John, 2026-07-21): Fusion opened his personal Chrome (`jlauer12@gmail.com`) when his
Autodesk login is `john@adom.inc` in his work profile.

## The fix: `fusion_signin`

```bash
# BEST - one server-side call does the whole chain, beats the ~2-min clock:
adom-bridge fusion_signin '{"auto":true,"profile":"chrome:you@work.com"}'
adom-bridge fusion_signin '{"profile":"chrome:you@work.com"}'   # staged/manual: fix + REMEMBER profile
adom-bridge fusion_signin '{}'                                  # staged: probe/remember/ask
```

### `auto:true` - the reliable path (do this)

The manual staged loop below needs THREE separate remote round-trips (click Sign In, capture, reopen)
and each one races Fusion's ~2-minute request expiry - on a slow or flaky relay you lose that race and
the callback shows "Sign-in request expired". `auto:true` collapses the whole chain into ONE
server-side call that runs **on the box**: it clicks Fusion's Sign In, polls the browser History
on-box (up to `waitSec`, default 32) for the fresh authorize URL, then re-opens it in the target
profile - so only ONE remote hop happens and everything else is local and fast, fitting inside the
2-min window. Pass the profile too (`{"auto":true,"profile":"chrome:you@work.com"}`) so it does not
have to stop and ask. Prefer this; use the staged calls below only to debug or when auto reports it
could not click/capture.

What it does:

1. **Reads the full authorize URL out of the wrong browser's History.** Fusion writes a complete
   `developer.api.autodesk.com/.../authorize?client_id=...&code_challenge=...&state=...&request_id=idsdk-...`
   into whatever browser it opened. The address bar rewrites within a second (the Autodesk SPA
   cleans it), but Chrome's/Edge's **History DB keeps the full URL** - so the bridge copies the
   locked History file and reads the newest Fusion (`idsdk`/`idmgr`) authorize entry. Racing the
   address bar over the relay does NOT work; the History read is the reliable capture.
2. **Re-opens that exact URL in the RIGHT profile, backgrounded** (ABE / `nbrowser_open_window`), so
   it never interrupts what the user is doing.
3. It completes back to the **running Fusion** on its own. The redirect is
   `accounts.autodesk.com/idmgr/callback` + an `autodesk://` protocol handoff matched by
   `request_id`, **not** a browser-specific localhost port - so the profile no longer has to be the
   OS default. Any profile that finishes the login hands the token to Fusion.

## Picking the right profile

- **Best:** the user tells you once (`{profile:"chrome:you@work.com"}`); the bridge **remembers** it
  (`~/.adom/fusion-signin/profile.json`) for every future sign-in.
- **Auto:** the verb probes each live profile with `nbrowser_login_state` against
  accounts.autodesk.com and prefers the one holding a real Autodesk session; failing that, a
  work/corporate identity over a consumer mailbox - and says it is guessing.
- **Unsure:** it returns `stage:"ask_profile"` - ASK which account their Autodesk login is, do not
  assume. Name profiles by identity (`Chrome - you (you@work.com)`), never "your browser".

## The staged loop

- `stage:"click_signin"` - Fusion has not opened the browser yet. Click Fusion's **Sign In** (it is a
  webview with no UIA node, so an image-space `desktop_click` on the button; screenshot the Fusion
  window first for a shotId), wait ~4s, call `fusion_signin` again.
- `stage:"opened"` - the URL is now in the right profile (`signinWhere`). Tell the user where, why
  (`signinProfileReason`), and offer: they finish it, you foreground it, or (with their OK) you drive
  "Continue with Google/Apple/Microsoft". **Never type their password/2FA** - escalate a real secret
  with `fusion_notify_owner`. Poll `fusion_readiness` until `ready:true`.
- `stage:"ask_profile"` - ask which profile, then re-call with it.

## The ~2-minute clock and the last-mile handoff (learned live 2026-07-21)

Two things bite after the profile fix works:

1. **Fusion's sign-in request EXPIRES in ~2 minutes.** If capture-to-completion takes longer (e.g.
   the user logs in by hand and dawdles), the callback page shows **"Sign-in request expired"** and
   Fusion never gets the handoff. `fusion_signin` now REFUSES a captured URL older than ~150s and
   returns `stage:"stale"` telling you to get a FRESH request: **fusion_stop -> fusion_start** (resets
   Fusion to a clean "Welcome to Fusion"), click Sign In, then call fusion_signin again promptly.
   Completion is near-instant when the target profile is ALREADY signed into Autodesk (it
   auto-consents), so pre-sign-into the profile once and future sign-ins fly.
2. **The `autodesk://` protocol handoff is the real last mile.** After login the callback page
   (`signin.autodesk.com/idmgr/callback#code=...`) says **"Click Open Identity Manager in the browser
   dialog"** with a **Retry** button. Fusion completes only when Chrome's native
   *"Open Autodesk Identity Manager?"* dialog is accepted (tick "Always allow", click "Open"). If it
   does not auto-fire: click **Retry** on the callback tab, then accept the Chrome Open dialog
   (a native UIA dialog - `desktop_find_control`/`desktop_ui_click` "Open", or a foreground click if
   UIA Invoke no-ops). This step is the same for ALL Fusion sign-ins; it is not caused by the profile
   move.

## ⭐ THE PROVEN APPROACH (John's directive, 2026-08-22) - DO NOT over-automate this

After hours of trying to fully automate the desktop sign-in, John's ruling is: **trigger the sign-in,
then put up an ab CAPTION asking the USER to complete it in the browser window that just opened, and
STOP. Do not try to automate the browser-side auth or the protocol handoff.** The exact flow:

1. **If switching accounts:** sign the current user out first (reset the shared Autodesk identity, see
   "Signing OUT" below), then `fusion_stop`/`fusion_start` so Fusion comes up `needsSignin:true`.
2. **Trigger the sign-in with UIA, not screenshots.** The Fusion "Welcome to Fusion" modal exposes a
   **"Sign In" button in the accessibility tree** (`desktop_find_control {hwnd, name:"Sign In"}` returns
   `invokable:true` with a real screen rect, e.g. center ~1279,910). Click it in the BACKGROUND with
   `desktop_ui_click {hwnd, name:"Sign In"}` - no foreground, no capture, no focus steal. (Get the hwnd
   from `Get-Process Fusion360 | MainWindowHandle` - `desktop_list_windows` sometimes misses it.) This
   makes Fusion open the OS-default browser to `signin.autodesk.com/?flowId=...`.
3. **Detect the browser auth window** (you CAN): `Get-Process chrome|msedge | Where MainWindowTitle -match
   "Sign in - Autodesk"` gives its hwnd, and `nbrowser_list_tabs {profile}` shows which profile+tab holds
   the `signin.autodesk.com` flow.
4. **Put an ab caption on that browser window asking the user to finish**, then poll and STOP automating:
   `desktop_caption {hwnd:<auth window>, text:"Please finish signing in as <email> here", subtext:"Adom
   will detect when it completes"}`. Do NOT drive the page, do NOT reopen the flow elsewhere, do NOT click
   the protocol dialog. John completes it by hand.
5. **Remind the user about "Open Product" (the commonly-missed step, John 2026-08-22).** After the web
   sign-in succeeds, the browser lands on a **"You're signed in - Open Product"** page. The user MUST
   click **"Open Product"** (and clear any "Open Autodesk Identity Manager?" native dialog) for the
   `adsk.idmgr://` handoff to reach Fusion. Fusion sits on "Signing in - Autodesk Fusion" indefinitely
   until they do. If the disk/title shows no completion ~30-60s after the web auth, **caption the auth
   window reminding them: "click Open Product to finish"**. This is the single most common reason it
   looks stuck.
6. **Detect completion via DISK** (the add-in is blocked by the modal so `fusion_*` verbs time out):
   a per-user folder appears/updates at `%LOCALAPPDATA%\Autodesk\Autodesk Fusion 360\<AUTODESK_USER_ID>\`
   (Windows) / `~/Library/Application Support/Autodesk/Autodesk Fusion 360/<AUTODESK_USER_ID>/` (macOS)
   (john=`X3G293QFLPJERKGZ`), and the window title flips off "Signing in - Autodesk Fusion".
7. **After the handoff, Fusion may show a NEW account/hub-selection screen** (seen first time on a
   personal Google-federated account, 2026-08-22) - this is where the user can pick which **hub** (e.g.
   their **personal hub**) to connect to. Not an error; capture it, show it in shotlog, and let the user
   choose. This is a great real-world test of a fresh personal-account sign-in.

### Why we do NOT automate the browser side (all tried and failed, 2026-08-22)
- Fusion opens the **OS-default browser profile** (john's `Default`), NOT the target account's profile.
  The `signin.autodesk.com/?flowId=...` flow is **locked to that browser session** - reopening the same
  flowId in another profile returns **"Unable to access this link"**, so you cannot redirect it.
- `nbrowser_*` **refuses to drive the user's own (unowned) window**, so you cannot fill that tab.
- The final `adsk.idmgr://` protocol handoff pops a **native Chrome dialog** that no screenshot can see
  and UIA exposes only intermittently. Even with the `AutoLaunchProtocolsFromOrigins` policy set (below),
  completion was unreliable. Automating past this point wastes the user's time - caption and hand off.

### Screenshot facts learned the hard way (2026-08-22)
- **`desktop_screenshot_window {hwnd}` works on a BACKGROUND window** (no foreground needed) - but the
  window must NOT be minimized (a minimized window has no surface; un-minimize with `ShowWindow(hwnd,4)`
  = SW_SHOWNOACTIVATE to bring it back to the background without stealing focus).
- **NEVER take fullscreen screenshots** (John's rule) - always `desktop_screenshot_window`.
- **The "screenshots return null" scare was a FIELD-NAME/parse bug, not a broken capture.** `desktop_
  screenshot_*` returns the image path at **top-level `localSafePath`** (and `localFullPath`), and
  ab-core auto-pulls the PNG into the container. Do NOT descend into the response's `output` sub-object
  (that holds `{data:{image:base64...}}` and has NO `localSafePath`) - reading `localSafePath` off
  `output` yields null and looks like a failed shot. The Fusion bridge's OWN `fusion_screenshot_fusion`
  uses DIFFERENT keys (`savedTo`, `dimensions.{width,height}`, `sizeKB`) and does NOT auto-pull, so a
  caller keying on `fullPathHost`/`localSafePath` gets null there too - read `savedTo`.
- Capture can transiently return null under **contention** with another thread capturing heavily - retry.
- Always show your work: `adom-shotlog inject -c <ch> -d "..." <path>` so the user sees exactly what you
  captured, and caption each foreground step ("taking window screenshot" / "done - back to background")
  so the user knows when they can alt-tab away.

## PERMANENT FIX for the protocol dialog (John, 2026-08-22) - set it BEFORE sign-in

The `autodesk://`/`adskidmgr://` "Open Autodesk Identity Manager?" dialog is the #1 blocker and it is
a NATIVE OS dialog (not DOM, not always in the UIA tree). Kill it forever with Chrome/Edge policy so
the protocol auto-launches with NO dialog, for every profile and every future user. Set it in HKCU
(no admin) BEFORE driving the sign-in, then RESTART the browser so it loads the policy:

```
# HKCU\SOFTWARE\Policies\Google\Chrome\AutoLaunchProtocolsFromOrigins   (and ...\Microsoft\Edge\...)
#   value "1" = {"protocol":"adskidmgr","allowed_origins":["*"]}
#   value "2" = {"protocol":"autodesk","allowed_origins":["*"]}
# then: taskkill chrome (NOT Chrome-for-Testing), relaunch with --restore-last-session
# macOS: pkill -f "Google Chrome" (or osascript -e 'quit app "Google Chrome"'), relaunch the same way
```

Chrome only reads this policy at startup, so a running Chrome must be restarted (tabs restore). This is
the single change that makes desktop sign-in hands-free. **This should be baked into `fusion_signin`
itself** so the bridge sets the policy automatically before every sign-in - build it into the verb, do
not rely on the AI remembering to do it.

## ⛔ THE HARD WALL: Fusion's sign-in modal is a GPU-protected, UNCAPTURABLE surface (2026-08-22)

Learned the very hard way over hours. When Fusion sits on **"Signing in - Autodesk Fusion"** waiting
for the browser, that modal blocks EVERY visual-automation technique:

- **Screen/window capture returns NULL** for it - `desktop_screenshot_window {hwnd}` AND
  `desktop_screenshot_screen` (once Fusion is foregrounded) both come back with no image (WGC/BitBlt/
  PrintWindow all fail on Fusion's hardware-accelerated overlay). So screenshot-then-click-by-coord is
  IMPOSSIBLE on this modal - do not burn time trying to capture it.
- **`desktop_list_windows` returns 0 windows** while this modal is up (enumeration wedges). Get the
  Fusion hwnd via PowerShell `Get-Process Fusion360 | MainWindowHandle` instead (macOS: no direct
  equivalent - there is no `MainWindowHandle`; Fusion reports as `Autodesk Fusion`, so AppleScript/
  `osascript` window queries are the nearest thing).
- **The modal blocks Fusion's ADD-IN** (main-thread), so `fusion_readiness`/`fusion_signin` and all
  `fusion_*` verbs TIME OUT. The bridge CORE verbs (`run_script`, `desktop_*`, `nbrowser_*`) still work.
  Check sign-in progress via DISK instead: a per-user folder appears at
  `%LOCALAPPDATA%\Autodesk\Autodesk Fusion 360\<AUTODESK_USER_ID>\` (Windows) /
  `~/Library/Application Support/Autodesk/Autodesk Fusion 360/<AUTODESK_USER_ID>/` (macOS)
  (e.g. jlauer12 = `LMFMLDP33E65`, john = `X3G293QFLPJERKGZ`).
- **Fusion launches foreground-LOCKED**, so a bare `SetForegroundWindow` is ignored and your SendInput
  clicks land nowhere. Defeat it with the ALT-key + AttachThreadInput trick (press ALT, attach to the
  current foreground thread, then SetForegroundWindow) - verified to make `GetForegroundWindow()==hwnd`.
- Even then, blind SendInput clicks and keyboard (Enter/Tab/Space) did NOT activate the modal's
  "Retry browser sign-in" button in a degraded-capture state.

**CONCLUSION / the way that actually works:** do NOT try to click this modal. Instead make the flow
never need a click: (1) set the AutoLaunchProtocols policy above, (2) pre-sign the target profile into
Autodesk (warm session) so OAuth auto-consents, (3) let Fusion open the browser (it does so on its own
when it enters sign-in, OR one human click of "Retry browser sign-in" that then completes fully
hands-free because of the policy). **Switching a personal Google-federated account onto the Fusion
DESKTOP app, on a machine whose desktop-capture subsystem is degraded, may still need ONE human click
on that modal - that is a genuine capability limit, not a skipped step. Say so honestly and stop; do
not flail.** To un-break a machine left signed-out, restore the backed-up
`Identity Services\idservices.db` + the user folder.

## Signing OUT to switch accounts (no fusion_signout verb exists)

Fusion caches the login in the SHARED `%LOCALAPPDATA%\Autodesk\Identity Services\idservices.db` (Windows) /
`~/Library/Application Support/Autodesk/Identity Services/idservices.db` (macOS), used by ALL Autodesk
desktop apps. To force a clean signed-out state: BACK IT UP first, then `fusion_stop`,
stop `IdentityService`/`IdentityUpdateService`, move `idservices.db` aside, `fusion_start` -> Fusion
comes up `needsSignin:true`. Restore = reverse it. `fusion_signin` takes a **`profile`** param
(`chrome:<email>`) to open the OAuth in a specific profile.

## Gotchas

- Read History AFTER Fusion has opened the browser, and take the NEWEST authorize entry - a stale one
  from a previous attempt carries a dead `request_id` and Fusion (waiting on a new one) never gets it.
- Do NOT close the Autodesk callback tabs Fusion opens - closing them removes Fusion's pending browser
  handoff and forces you back to the un-clickable "Retry" modal.
- If the wrong account completes in the default browser before you move it, sign Fusion out and re-run.
- Chrome keeps History locked; always COPY the file before opening it (the verb does this).
- This is also the sign-in stage `fusion_demo` uses, so a first-time demo never lands in the wrong
  Autodesk account.
