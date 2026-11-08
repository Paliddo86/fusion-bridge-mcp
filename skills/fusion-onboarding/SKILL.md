---
name: fusion-onboarding
user-invocable: true
description: Onboard a brand-new Adom user who wants Fusion 360 features but doesn't have Fusion installed — the AI downloads the Fusion installer, silent-installs it, drives the Autodesk login (free/trial is enough), and auto-installs the AdomBridge add-in so the bridge can inject commands. "Complete the circuit" so fusion_* verbs work end to end. Trigger words — install fusion, no fusion, fusion not installed, set up fusion, download fusion, onboard fusion, get fusion, fusion trial, install the bridge add-in, fusion addin not running, set up the fusion bridge from scratch.
---

# Fusion 360 onboarding — from zero to a working bridge

Goal: a new Adom user with **no Fusion 360** should be able to use the Fusion bridge after a
hands-off, AI-driven setup. The AI installs Fusion, signs them in, and wires up the add-in so
`fusion_*` commands work. **The free/trial tier is plenty** for viewing files and a lot of real
work — nobody has to pay to get value.

All laptop steps run via `adom-bridge --target <laptop> shell_execute` / `run_script` /
`send_files`. Drive the user's own machine; don't ask them to do steps you can do.

## Step 0 — Check readiness (fast, never launches Fusion)
Call **`fusion_readiness`** (or ab's **`bridge_readiness`**) FIRST - it reports `installed` / `running`
/ `ready` without launching anything:
- `installed:false` → onboard (the steps below).
- `installed:true, running:false` → **no onboarding needed**, just `fusion_start`, then drive.
- `ready:true` → drive Fusion directly.

NOTE (ab >=1.9.63): ab **DETECTS** the Fusion host app (via the bridge.json `detect` block - the
`%LOCALAPPDATA%\Autodesk\webdeploy\production\*\FusionLauncher.exe` path on Windows, or
`/Applications/Autodesk Fusion.app` on macOS) but **NEVER installs it** -
Fusion is the user's host app. ab also **provisions Python itself** - a system install if one is on
PATH, else a portable, no-UAC python-build-standalone copy (pinned 3.12.13) under
`~/.adom/adom-runtimes/` - and spawns the bridge from it by absolute path. So **never** download or
bootstrap Python here; the user sees no UAC prompt for a runtime (check state with ab's `runtimes`
verb). Onboarding is only about Fusion itself + the Autodesk login + deploying the add-in.

## Step 1 — Download the Fusion installer
The installer is the small "Fusion Client Downloader" (it streams the full app during install).
The Windows `.exe` URL below is the Windows flow; on macOS fetch the macOS installer (`.dmg`) from
Autodesk's Fusion download page and mount it.
```
URL: https://dl.appstreaming.autodesk.com/production/installers/Fusion Client Downloader.exe
```
Download it to a no-space path on the laptop (e.g. %TEMP%\ on Windows, `$TMPDIR` on macOS):
```
shell_execute {"command":"curl -L -o C:\\tmp\\FusionDL.exe \"https://dl.appstreaming.autodesk.com/production/installers/Fusion%20Client%20Downloader.exe\""}
# macOS equivalent: curl -L -o "$TMPDIR/Fusion.dmg" <dmg-url>
```
(PowerShell `Invoke-WebRequest` works too; curl ships on modern Windows. macOS has no PowerShell
requirement - `curl` is native.)

## Step 2 — Silent install (elevated)
```
"%TEMP%\FusionDL.exe" --globalinstall --quiet      # system-wide, no UI
# macOS: no --globalinstall flag - open the .dmg, drag Fusion to /Applications (or use the
# installer's own UI); `sudo` is needed only if installing for all users.
```
- Needs **admin/elevation** — run it so it elevates (UAC). On the user's own box this is one
  consent. It then **streams ~several GB**, so it can take 10-30 min — **monitor, don't block**
  (poll for the FusionLauncher under `%LOCALAPPDATA%\Autodesk\webdeploy\production\<hash>\` on Windows;
  on macOS poll for `/Applications/Autodesk Fusion.app`).
- Uninstall (if ever needed): `--globalinstall --process uninstall --quiet`.
- When done, `fusion_readiness` flips to `installed:true` (the bridge finds the newest webdeploy
  launcher by mtime).

## Step 3 — First launch + Autodesk login (free account / trial)
`fusion_start` launches it; the first run needs an Autodesk login. If they have no Autodesk account,
the sign-in page has **Create account** (free) — and Fusion offers a **free trial / personal-use**
tier that's enough for viewing and a lot of work. **Don't auto-enter their password/2FA; do drive
everything else.**

> **Use the dedicated `fusion-autodesk-signin` skill for the full drive** — it has the complete,
> battle-tested sequence and every pitfall (blank webview → restart Fusion, invisible-Edge / CLIXML
> noise, warm Google/Apple/MS SSO, the Identity Manager protocol overlay, fast-expiring codes, and
> `fusion_notify_owner` escalation). The summary below is the mechanism; that skill is the how-to.

### The mechanism (why naive clicking fails)
Fusion's "Sign In" opens the **default browser** to an Autodesk OAuth page. When auth completes the
browser lands on `signin.autodesk.com/idmgr/callback#code=…`, whose JS posts the code to Fusion's
**localhost** listener via an `adskidmgr://` protocol handoff. Three traps make this fail if you
click naively:
- **Codes expire fast.** A stale sign-in tab left open holds a dead code → "Sign-in request
  expired". Every retry that races an old tab loses.
- **`desktop_ui_click` (UIA Invoke) does NOT fire the protocol.** Browsers block protocol launches
  from untrusted (synthetic-invoke) events. You need an **isTrusted** click: **`desktop_click`**
  (SendInput coordinate click) or an `nbrowser` CDP click.
- **An in-page "This site is trying to open Autodesk Identity Manager?" overlay** gates the launch.
  It is NOT an owned popup, so `desktop_screenshot_window` sees it but popup-capture (`ownedPopupCount`)
  never will. It must be clicked "Open".

### The PROVEN sequence (validated live 2026-07-05, zero password entry by the AI)
1. **Warm the session / clear stale tabs.** If the user already authenticated Autodesk in the
   browser, the session is warm. FIRST close any dead `signin.autodesk.com` / "Sign in - Autodesk"
   tabs (foreground the Edge window, `desktop_press_key {"key":"ctrl+w"}` per tab) — they only hold
   expired codes and cause false "expired" screens.
2. **One fresh request.** `desktop_ui_click` **"Retry browser sign-in"** (or "Sign In") on the
   Fusion window to mint exactly one fresh code + open one live auth tab.
3. **Open Product, isTrusted.** On the "You're signed in" page, click **"Open Product"** with
   **`desktop_click`** (SendInput), NOT `desktop_ui_click`.
4. **Clear the protocol overlay, FAST.** When the "open Autodesk Identity Manager?" overlay appears,
   `desktop_click` the **"Always allow"** radio (so it never blocks again) then **"Open"** — before
   the code expires.
5. **Confirm.** Poll `fusion_readiness`; the Fusion window title flips **"Signing in - Autodesk
   Fusion" → "Home - Autodesk Fusion"** and `ready` goes true (`fusion_readiness` reports
   `needsSignin:true` the whole time it's blocked, so branch on that).

**Headless variant:** if `nbrowser` (the Adom browser extension, `nbrowser_readiness`) is driving
the **same** browser window Fusion opened, do steps 3-4 via CDP (`nbrowser_eval` to click the DOM
button) — CDP clicks are isTrusted and need no window focus, beating the expiry race. Caveat: the
auth tab must be in nbrowser's window; if Fusion opened a different Edge window, use the SendInput
path above.

> **NOTIFY only when you truly can't proceed.** The sign-in is something you should AUTOMATE, not
> punt. Reserve an ab **`notify_user`** toast for a genuine wall (a UAC/elevation prompt, a
> `--globalinstall` consent, license acceptance, or a 2FA/password step you must not auto-fill).
> As of **ab 1.9.84** `notify_user` works directly over the bridge's direct API — you don't relay a
> payload to the AI anymore. **If the bridge runs UNATTENDED (e.g. on a VM) and the user is on
> another machine, add a cross-ab `target`** (a peer clientName from the `targets` verb, or `"all"`)
> so the toast lands where they actually are. The bridge already fires this itself for a seat/
> licensing DECISION dialog (a true wall) — see `notifyDelivered` in the launch response.

## Step 4 — Complete the circuit: auto-install the AdomBridge add-in
This is what lets the bridge inject commands into Fusion after launch.
- **The bridge does this for you:** on start, `server.py` auto-runs `install_addin`, which lands the
  add-in in **EVERY known Fusion add-in dir** — `%APPDATA%\Autodesk\FusionAddins\` (what 2025+
  Fusion actually scans) AND the legacy `Autodesk Fusion[ 360]\API\AddIns\` dirs. On macOS the same
  dirs are `~/Library/Application Support/Autodesk/FusionAddins/` and
  `~/Library/Application Support/Autodesk/Autodesk Fusion 360/API/AddIns/`. ⚠ Fusion MOVED
  this directory across versions and silently ignores the others (an add-in only in the legacy dir
  never loads on current Fusion — issue #63, proven live on a fresh install). Never assume one path.
- **⛔ NEVER make the user restart Fusion or enable the add-in BY HAND.** After installing, YOU do
  it: `fusion_stop` then `fusion_start` — the add-in auto-loads (`runOnStartup:true`, verified live:
  port 8774 up 20s after restart, zero manual enabling). Handing them the chore defeats the bridge.
- **⚠️ BUT ASK FIRST WHEN A HUMAN IS SITTING THERE WITH DOCUMENTS OPEN** (the Adonis incident,
  2026-08-27, and John's "that was really rude" before it). "Never make them do it" is not "restart
  their Fusion out from under them". The two rules resolve as: **get consent, then do the work
  yourself.** During ONBOARDING, on a machine with nothing open, just restart. When a verb returns
  `addin_restart_required`, its payload carries `userMessage`, `openDocuments` and
  `unsavedDocuments` precisely so you can ask — then call `fusion_restart_for_addin {save:true}`,
  which saves first and refuses with `unsaved_named_document` rather than discarding a never-saved
  document. An AI on an unattended box may call it directly.
- **It auto-loads every launch:** `AdomBridge.manifest` has **`"runOnStartup": true`**, so Fusion
  runs the add-in (and its localhost:8774 HTTP server) automatically on startup — **no manual
  "enable" needed**. (Verified: after a Fusion restart the add-in comes back `addinReady:true`
  with no UI interaction.)
- **If the add-in was just installed while Fusion was running**, restart Fusion (`fusion_close`
  then `fusion_start`) so it picks up + runs the add-in.

## Step 5 — Verify the circuit
```
fusion_addin_status {}          # add-in loaded + healthy?
fusion_get_app_state {}         # round-trips through the add-in to Fusion
```
When `fusion_start` returns `addinReady:true` and a `fusion_*` verb round-trips, the circuit is
complete — the bridge can drive Fusion (open/export/search/etc.).

## Then: cloud search (optional, recommended)
If they'll use cloud-file search, continue with the **`fusion-aps-search`** /
**`fusion-aps-signin`** skills (one-time-per-company APS setup; each user signs in once; never
charged).

## Notes / gotchas
- The big install is the slow part — **monitor** it (poll for the webdeploy launcher), don't sit
  on a blocking call.
- Admin elevation is required for `--globalinstall`; expect one UAC consent on the user's box.
- After a major Fusion self-update (Fusion web-deploys updates ~weekly), the add-in survives in
  `API/AddIns`, but re-verify with `fusion_addin_status`; re-run `install_addin` if it drifted.
- Verify the current installer URL / flags if Autodesk changes them (Sources: silentinstallhq,
  Autodesk deploy docs).

## Step 5.5 — Set Z-UP (Adom standard) on a NEW account

A FRESH Autodesk account comes up with **Y-up** as the Default Modeling Orientation (Fusion's
factory default), but **Adom is a Z-up environment** (John, 2026-08-23: jlauer12's new account
defaulted to Y-up while his other account was Z-up). Check and fix it as part of onboarding:

- `fusion_readiness` now reports **`modelingOrientation`** ("yup"/"zup") and flags Y-up in its
  `_hint` (read straight off Fusion's local options file - no add-in, never disturbs a running
  session, so you can check it even mid-driving).
- If it is `yup`, set it: **`fusion_set_preference {"modelingOrientation":"zup"}`** - a live
  preference change, applies immediately, no restart. (Do NOT hand-edit the options XML while
  Fusion is RUNNING - it can overwrite on exit; the verb goes through Fusion's own API.)
- Do this for every new user by default: a Z-up world where a part imports Y-up is the kind of
  silent-wrong that wastes an EE's afternoon.
