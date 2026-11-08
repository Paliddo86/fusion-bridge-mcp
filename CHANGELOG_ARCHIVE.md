# Changelog archive (1.6.37 and earlier)

Older history for the Adom Bridge Fusion 360 bridge. Recent versions are in the main
[CHANGELOG.md](https://wiki.adom.inc/adom/fusion-bridge). Newest-first.
## 1.6.37 - 2026-07-06

- **`fusion_readiness` no longer lies `ready:true` for a stuck/dead Fusion** (John caught it). It only
  checked for the "Signing in" window when the add-in was silent - but a STALE add-in server from a
  just-killed instance keeps port 8774 answering, so `addin_ok` went True while the CURRENT Fusion sat
  stuck on "Signing in", and readiness reported ready. Now the Fusion-process window title is checked
  WHENEVER running (authoritative: "Signing in" == not ready), and `ready` additionally requires the
  add-in to actually RESPOND (a live main thread), not just the process to exist. Added a distinct
  "running but add-in not responding yet" hint so a half-launched Fusion is never mistaken for ready.
- **Proactive foreground-lock during launch = no more flicker.** The reactive minimize-guard alone
  left a visible flash (window appears, then shrinks). Now the launch raises the Windows
  foreground-lock timeout so Fusion's splash/main/seat windows are DENIED a foreground grab outright
  (taskbar flash, not a real steal), restored to the user's original value when the launch window ends.

## 1.6.36 - 2026-07-06

- **Codified the background-launch rules in the `fusion-bridge-dev` skill** (John's request): the
  ctypes handle-truncation gotcha (the silent killer behind 1.6.33/1.6.34), how to VERIFY the guard by
  objective `GetWindowPlacement().showCmd == 2` state rather than by eye, and that the guard only fires
  on a FRESH launch (kill first for a clean background state). Verified live on the laptop: a clean
  `fusion_start` leaves `Fusion360 showCmd=2` (minimized) - the window never sits in the foreground.

## 1.6.35 - 2026-07-06

- **Fixed the focus-guard doing NOTHING due to a ctypes handle-truncation bug** (the real reason
  1.6.33/1.6.34 still let windows steal focus, caught 2026-07-06). `user32.GetForegroundWindow()` was
  called with no `restype`, so ctypes defaulted the returned HWND to `c_int` and TRUNCATED it to 32
  bits on 64-bit Windows - `ShowWindowAsync` then got a garbage handle and silently no-op'd, so the
  guard never actually minimized anything and the user had to alt-tab back themselves. Now the guard
  sets proper `restype`/`argtypes` (HWND/DWORD/BOOL) so handles survive, and polls at 0.1s. Family
  windows that steal focus are now genuinely minimized within ~100ms.

## 1.6.34 - 2026-07-06

- **Focus-guard now covers the whole Autodesk launch FAMILY** (fix for the 1.6.33 guard, caught live).
  The guard only demoted `Fusion360.exe` windows, but the splash "Loading additional modules"
  (`FusionLauncher.exe`) and the "Signing in" / **"Active Sessions Exceeded"** dialogs
  (`AdskIdentityManager.exe`) slipped past and yanked the user's foreground. Now `_fusion_family_pids()`
  matches every family process image and the guard (150s) minimizes any of them that steals focus.
- **Seat conflict ("Active Sessions Exceeded") now AUTO-RECLAIMS the license instead of asking**
  (John: "just grab the license back... no harm, the user can reclaim on the other machine"). The
  dialog is a CEF modal that ignores background SendInput, so the launch handler frees the seat at the
  SOURCE via `_reclaim_seat_from_peers()` (cross-AD `fusion_stop` to peer ADs), kills the local instance
  stuck on the dialog, and relaunches clean - no conflict dialog, no user prompt, fully background.

## 1.6.33 - 2026-07-06

- **Fusion now launches in the BACKGROUND - never steals the user's foreground** (John's directive).
  The bridge runs on a machine the user is actively working on, and a bare `Popen([launcher])` opened
  Fusion's splash + main window in the foreground, yanking focus off their work. Now `ensure_fusion_
  running()` starts the launcher MINIMIZED + NOT ACTIVATED (`STARTUPINFO SW_SHOWMINNOACTIVE`) and fires
  a ~120s **focus-guard** daemon that minimizes (no-activate) *only* a Fusion window that is *currently*
  the foreground (focus-theft detected via `GetForegroundWindow` + PID match, so it never fights a user
  who deliberately opened Fusion). The add-in still loads fully while minimized. Foreground is now
  opt-in, only when a verb's job is to SHOW the user. Codified as a HARD invariant in the
  `fusion-bridge-dev` skill so every future window-opening path follows it.

## 1.6.32 - 2026-07-06

- **Fixed a false positive in 1.6.31's `updating` detection** (caught immediately on a real laptop
  during a regression pass). Fusion NEVER prunes old `webdeploy\production` builds, so a normal
  machine accumulates many stale ones (the laptop had 5, dating back to April, Fusion not even
  running) - and the bare "2+ builds" heuristic fired permanently, falsely telling callers "Fusion
  is auto-updating, restarting every 30-60s" when Fusion was simply closed. `fusion_update_in_progress()`
  now requires ACTIVE streaming: 2+ launcher builds AND at least one build dir modified in the last
  ~20 min. Stale leftovers no longer trigger it; the genuine mid-update flap still does.

## 1.6.31 - 2026-07-05

- **`fusion_readiness` now reports a distinct `updating:true` status when Fusion is applying an
  auto-update.** New `fusion_update_in_progress()` detects the tell-tale TWO+ webdeploy `production`
  builds coexisting; while that applies, Fusion crash-restarts itself every ~30-60s, so verbs
  intermittently saw a bare "not running" that read as a crash. Readiness now flags `updating:true`
  with a hint to poll/retry (it stabilizes once the update finishes) rather than treat the flap as
  fatal. Both hardening items came straight out of the fresh-VM battery.
- **Preference verbs (`fusion_set_preference`/`fusion_get_preferences`) retry once on a transient
  add-in connection error.** The first call can hit the add-in mid doc-transition (e.g. right after
  opening a Library, when the pref script must create a Design) and come back "not running" though
  Fusion is up; a ~1.5s retry clears it (seen live on the battery - failed first call, passed on
  retry). A genuinely-down Fusion just costs one extra 1.5s attempt.

## 1.6.30 - 2026-07-05

- **`fusion_install_fusion` and `fusion_start` now live-detect too** (completing the 1.6.29 fix). Both
  still trusted the stale bridge-start `fusion_info.installed` snapshot: on the fresh VM,
  `fusion_install_fusion` REFUSED with "already installed" though disk was `NO_WEBDEPLOY`, blocking the
  reinstall, and `fusion_start` would launch into the void. Both now call `detect_fusion()` for the
  on-disk truth; `fusion_start` also reassigns from it so a launch immediately after a fresh install
  uses a current `exe_path`. Found + fixed live during the fresh-VM battery.

## 1.6.29 - 2026-07-05

- **Fixed `fusion_readiness` trusting a stale install cache.** It read the bridge-START
  `fusion_info.installed` snapshot and only re-detected when that was False - so after Fusion was
  UNINSTALLED (or a VM reset wiped it) while the bridge kept running, readiness still reported
  `installed/running/ready:true` on a machine with `NO_WEBDEPLOY` on disk (found live during a
  fresh-VM validation on ADOMBASELINE). It now ALWAYS live-detects via `detect_fusion()` (a fast
  filesystem scan), so readiness reflects reality whether Fusion was just installed OR just removed.

## 1.6.28 - 2026-07-05

- **New `fusion_set_preference` + `fusion_get_preferences` verbs** - drive Fusion's appearance and
  navigation settings programmatically, applied LIVE (no restart). Server-only orchestrators (they
  compose an `adsk` preferences script and run it via the existing add-in `run_modeling_script`, so
  no add-in redeploy). Friendly keys: `theme` (light/darkblue/darkgray/classic/device/dark),
  `invertScrollZoom` (mouse scroll/zoom direction), `orbitScheme`
  (fusion360/alias/inventor/solidworks/tinkercad/powermill), `modelingOrientation` (yup/zup),
  `gestureNav`, `cameraPivot`, `lengthUnit` (mm/cm/m/in/ft, active design). Per-key outcome is
  reported in `applied` and the resulting state in `current`. Discovered via a live introspection of
  `app.preferences.generalPreferences` on ADOMBASELINE.
- **Note on theme availability:** not every Fusion build ships every theme. On the current webdeploy
  only **LightGray, DarkBlue, Device** were available (`Dark Gray theme not available` / `Classic
  theme not available` raise on assignment), so the verb tries candidates and reports per-key
  failures; `dark` resolves to Dark Gray then falls back to Dark Blue. Dark mode was set live on the
  VM's Fusion this way (Dark Blue).

## 1.6.27 - 2026-07-05

- **Fixed a false-positive in 1.6.26's new `needsSignin` readiness detection** (caught live before
  release to any consumer). 1.6.26 derived `needsSignin` from `classify_launch_dialogs()`, which
  enumerates ALL top-level windows and so matched a stray Edge **browser tab** titled "Sign in -
  Autodesk" - reporting `needsSignin:true` while Fusion was actually signed in and drivable. Now the
  detection is scoped to the **Fusion-process window** (`get_fusion_window_info` title/`mainEnabled`)
  and is only evaluated when the **add-in isn't responding** (a responding add-in = past sign-in =
  drivable), so a leftover browser tab can never trigger it. Verified on ADOMBASELINE: signed-in
  Fusion now reports `ready:true`.

## 1.6.26 - 2026-07-05

- **Adopted AD 1.9.84's direct API (disc #78): the bridge now calls `notify_user` ITSELF, cross-AD.**
  New `ad_client.py` talks back to AD over `$ADOM_DIRECT_API_URL/command` with the
  `X-Adom-Bridge-Token` attribution header. It **capability-probes `GET /commands`** (not hardcoded)
  so it degrades gracefully on older AD, enumerates peer ADs via `targets`, and can route any verb to
  a peer with a cross-AD `target`. **Dropped the old "AD can't notify" workaround** (returning a
  `notifyUser` payload for the driving AI to relay). On a launch that surfaces a seat/licensing
  **DECISION** dialog (a true wall that can suspend/close Fusion on the user's OTHER machine), the
  bridge now fires `notify_user` directly with `reach_user=True` — fanning out to `"all"` peers when
  it may be running unattended on a VM, so the toast lands wherever the user is. Response carries
  `notifyDelivered`. Sign-in stays automate-first (NOT auto-notified).
- **Fixed `fusion_readiness` lying `ready:true` during sign-in.** Fusion can be running yet blocked by
  the first-run Autodesk sign-in modal (main UI disabled, add-in can't serve). Readiness now detects
  the `signin_required` window via the launch classifier and returns `ready:false` + `needsSignin:true`
  instead of "READY - drive Fusion" (which sent verbs into a wall). Found live on ADOMBASELINE.
- **Baked the PROVEN sign-in sequence into the `needsSignin` hint + `fusion-onboarding` skill**
  (validated live 2026-07-05): clear stale `signin.autodesk.com` tabs (expired codes = "Sign-in
  request expired") → "Retry browser sign-in" for one fresh request → click "Open Product" with
  **`desktop_click` (SendInput=isTrusted)**, NOT `desktop_ui_click` (UIA doesn't fire the protocol) →
  tick "Always allow" + "Open" on the in-page "open Autodesk Identity Manager?" overlay (not an owned
  popup) → poll readiness. Documents the `nbrowser`/CDP headless variant and the isTrusted requirement.

## 1.6.25 - 2026-07-04

- **Sign-in is now AUTOMATED-FIRST, with the path baked into the verb's `_hint` + a `notifyUser`
  payload (John's directive).** On `needsSignin`, `fusion_start` now tells the AI to: (1) check
  `nbrowser_readiness` and drive the Autodesk OAuth via the native-browser extension over CDP when
  it's on the same browser Fusion opened sign-in in (fast + headless - beats the ~2min request
  expiry that defeats background `desktop_ui_click`, the failure seen live on the VM drill); (2) else
  `desktop_ui_click` the Sign In / Open Product buttons FAST; (3) ONLY if both fail, relay the new
  structured `notifyUser:{title,body,level}` payload to `notify_user`. The goal is: automate
  everything, pull the user in (via a toast) only for a true wall - never auto-enter password/2FA.

## 1.6.24 - 2026-07-04

- **Two more launch dialogs are now classified IN CODE (caught live on the clean-VM drill):**
  "Fusion is already open / Multiple instances are not supported" (category `already_open` -
  auto-dismissed via Cancel; a singleton is never double-launched) and the first-run Autodesk
  "Welcome to Fusion / Signing in" window (category `signin_required` - SURFACED, never auto-closed,
  with a `_hint` to notify + drive Sign In / Open Product without touching credentials). Added an
  explicit `AUTO_DISMISS_CATEGORIES` set so only benign acks get WM_CLOSE'd; decisions + sign-in are
  returned as `needsUserDecision`/`needsSignin`. Fixes a latent bug where sign-in would have been
  auto-closed.

## 1.6.23 - 2026-07-04

- **NEW `fusion_install_fusion` - install Fusion for the user with ZERO approval-gated shell.** The
  clean-VM drill showed every shell_execute step (download/spawn/tasklist) hits AD's "approve 1 hr"
  gate; bridge verbs run as trusted installed code and don't. The verb downloads Autodesk's official
  Client Downloader inside the bridge (urllib), is ELEVATION-AWARE (--globalinstall when admin,
  PER-USER otherwise - a non-elevated globalinstall dies silently on UAC, learned live), spawns it
  detached, and returns promptly. `fusion_readiness` now live-redetects and reports `installing:true`
  while the streamer runs. Declared as `detect.installVerb` (AD >=1.9.79) + in hostAppOptionalVerbs
  so it forwards on a Fusion-less box - completing the contract slot left empty in 1.6.22.

## 1.6.22 - 2026-07-04

- **Adopted AD 1.9.79's two new GENERIC bridge contracts** (previously Fusion-specific AD-core code):
  (1) `detect` gains `hostAppOptionalVerbs` (readiness/describe/get_app_state/status + the APS verbs -
  all answer without Fusion running; AD now detects only INSTALLED, this bridge owns RUNNING and
  fast-fails `fusion_not_running` pre-add-in, no hang); (2) NEW `timeouts` block - per-verb HTTP
  budgets are now bridge-owned (AD's fallbacks + tuned values: build_library_3d 1800s,
  capture_library_views 600s, make_3d_package 200s, run_modeling_script 180s, generate_package 120s;
  `open*` prefix 150s). Timeouts stay NON-terminal ({stillRunning, statusVerb}). Docs note AD's
  not-installed errorCode is now generic `host_app_not_installed` (+hostApp field).

## 1.6.21 - 2026-07-04

- **Docs/skills audit (all three tiers).** USER skill: `fusion_generate_package` added to the core
  verbs table + a `bridge_restarting` cold-start row (AD >=1.9.77: poll `fusion_readiness`, never a
  not-installed signal; read-only verbs forward mid-respawn). DEV skill: deploy section rewritten for
  the FusionAddins reality (install to ALL dirs, never-ask-user-to-restart). README fully overhauled:
  11 capability sections incl. EPG package generation + markings, the architecture SVG, and the
  parent/child dialog breakthrough.

## 1.6.20 - 2026-07-04

- **The 10% marking margin is now ENFORCED by measurement, not estimated.** Fusion's text font runs
  wider than the 0.75h glyph estimate ('ADOM-A' rendered edge-to-edge - John caught it), so after
  placing the text the verb measures its REAL sketch bbox and shrink-to-fits if it busts the margin
  box (both axes; up to 2 passes). The manifest records `textWidthMeasuredMm` + `fitIterations`.

## 1.6.19 - 2026-07-04

- **Markings run along the LONGEST axis of the top face** (auto-rotated 90 deg when the long axis is
  sketch-y) - maximum room for the text (John's rule). **Multi-line markings** work: newline-joined
  text (MPN + variant) with per-line auto-fit (h = min(boxH/(1.35*lines), boxW/(0.75*maxChars))).
- **Sidecar manifest for marking refreshes:** with `outputStep`, `<step>.manifest.json` records every
  setting picked + WHY + the calculated bboxes (chip bbox, top-5% band, face extents, text box,
  height, rotation). To refresh a marking (e.g. add a variant on line 2), re-call the verb with the
  manifest's `type` + `paramsCm` (`unitsCm:true`) + the new etch text - deterministic generation
  reproduces placement exactly. Manifest also returned inline; `_hint` teaches the whole workflow.

## 1.6.18 - 2026-07-04

- **Chip markings are now READABLE: raised WHITE text by default (John's spec).** Humans need
  contrast - a dark-on-dark emboss is invisible - so `fusion_generate_package` now marks chips with a
  thin POSITIVE extrude painted white (silkscreen-style, via EPG's own rgb-appearance utility; white
  survives into the colored STEP). `etchStyle:"engraved"` keeps the sunken laser cut.
- **Top-surface finding is now the bbox-band algorithm (in CODE, not just the skill):** whole-chip
  bounding box -> only upward planar faces in the TOP 5% height band -> largest area wins -> text
  laid out with a 10% margin inside that face. The verb's `_hint` teaches all of this + per-family
  param names on every call; the fusion-build-parts-library skill gained a "creating chips" section.

## 1.6.16 - 2026-07-04

- (1.6.17) Etch auto-fit measures the face in SKETCH space (model-bbox axes can be swapped vs the
  text's baseline axis) + a 0.75h glyph width - 'LM358' now fits fully on a SOIC-8 top.
- `fusion_generate_package` etch auto-fit is now WIDTH-aware: text height is capped by box-width /
  char-count (a glyph is ~0.62h wide), so a 5-char MPN like 'LM358' fits instead of cropping
  (height-only fit rendered 'M35' on a SOIC-8 - caught on the live verify).

## 1.6.15 - 2026-07-03

- **NEW `fusion_generate_package` - parametric IPC packages from Fusion's BUILT-IN
  ElectronicsPackageGenerator, with laser-etched markings, in one headless call.** Give it a `type`
  (~50 EPG families: chip/soic/qfn/qfp/bga/sot23/dfn/melf/ecap/crystal/headers/...) + `params` (mm,
  auto-converted to EPG's cm) and it drives EPG's generator modules directly (no GUI), optionally
  **laser-etches** a marking (`etch:"103"` / an MPN) into the body top as REAL cut geometry, and
  exports STEP (`outputStep`) - the etch survives into the STEP for KiCad. Proven live on a fresh VM:
  0603 + '103' etch + valid 233KB STEP (121 faces) in ~2s. EPG is located dynamically across
  webdeploy hashes + install roots (never hardcoded); missing EPG returns `errorCode:epg_not_found`.
  Etch targets the LARGEST body's top face (terminals tie the molded body on max-Z - learned live).
  This is the royalty-free path to whole component libraries: our own generated output, not
  license-restricted vendor models.

## 1.6.14 - 2026-07-03

- **#63 ROOT-CAUSED + FIXED: Fusion MOVED its add-in directory; we now install to ALL of them.**
  2025+ Fusion scans `%APPDATA%\Autodesk\FusionAddins\` and SILENTLY ignores the legacy
  `Autodesk Fusion[ 360]\API\AddIns\` dirs (proven live on a fresh install: add-in in the legacy dir
  never loaded through a full launch; moved to FusionAddins it came up in 20s, runOnStartup honored,
  zero manual enabling). `install_addin.py` now installs/uninstalls to EVERY known location
  (TARGET_CANDIDATES, newest first), `fusion_detect` scans all of them, and the add-in-not-responding
  errors carry the FusionAddins explanation + a stable `errorCode`.
- **⛔ NEVER ask the user to restart Fusion / enable the add-in - baked into code + skills.** Every
  hint that said "restart Fusion 360" / "enable via UTILITIES > ADD-INS" now instructs the AI to DO
  IT ITSELF (`fusion_stop` + `fusion_start`); the USER skill (convention 6), fusion-onboarding, and
  CLAUDE.md all carry the rule. Doing the work for the user is the point of the bridge.

## 1.6.13 - 2026-07-03

- **`bridge.json` dependencies described honestly.** Dropped `fusion360: ">=2.0.0"` - Fusion 360 is
  the HOST APP (already declared in the `detect` block, and it uses date/webdeploy versions, not a
  ">=2.0.0" semver), not a package dependency. Corrected the stale `python: ">=3.11"` to `>=3.10`
  (the real floor - the code uses 3.10 `X | None` unions; AD provisions + pins Python 3.12.13). The
  user-facing wiki Dependencies tab stays empty, which is correct: this bridge has no external
  wiki-package deps (the add-in is bundled, the APS layer is in-repo).

## 1.6.12 - 2026-07-03

- **Launch now WATCHES for licensing/launch dialogs in CODE (not just a skill), found live on the
  ADOMBASELINE first-run test.** `_handle_launch` enumerates ALL top-level windows the instant the
  Fusion process is up and classifies the dialogs that block a first launch but are owned by the
  Autodesk FAMILY (AdskIdentityManager / FusionLauncher), so the Fusion-scoped classifier misses
  them: **"Active Sessions Exceeded"** + **"Suspend Remote Session"** (seat conflict) and **"Error
  Launching Streamed Application / missing or incomplete / re-install"** (an incomplete webdeploy
  folder). Benign launch-error acks are **auto-dismissed** (WM_CLOSE, background). A seat-conflict is
  a **DECISION** dialog: the bridge returns `needsUserDecision:true` + `setupDialogs[]` + a `_hint`
  that says fire a `notify_user` and ASK the user which option to take (the options
  suspend/shut-down Fusion on their OTHER machine) - it NEVER auto-picks. New helpers in
  `dialog_classify.py`: `classify_launch_dialogs`, `close_dialog_bg`, `DECISION_CATEGORIES`.

## 1.6.11 - 2026-07-03

- **Detect a system-wide (`--globalinstall`) Fusion, not just per-user (fixes a broken onboarding
  loop, found on the ADOMBASELINE fresh-VM test).** The fusion-onboarding skill installs Fusion with
  `--globalinstall`, which lands the launcher under `%ProgramFiles%\Autodesk\webdeploy\production\`,
  but detection (both `fusion_detect.py` and the `bridge.json` `detect` block) only checked
  `%LOCALAPPDATA%`. So an AI-installed Fusion was INVISIBLE - onboarding "succeeded" yet the bridge +
  AD `bridge_readiness` still reported `no-app`. Detection now scans `%LOCALAPPDATA%`, `%ProgramFiles%`,
  `%ProgramFiles(x86)%` (and `%ProgramW6432%`), preferring a complete install (has `.ini`) + newest.
- **Notify the user on EVERY active step.** Per the "the AI does everything, but pull the user in when
  only they can act" rule: the USER skill's convention 4 and fusion-onboarding now mandate an AD
  `notify_user` toast the instant the user must do something (Autodesk sign-in, 2FA, a UAC/elevation
  prompt, license acceptance), and a progress toast for long installs - never silently wait on a
  screen they aren't watching.

## 1.6.10 - 2026-07-03

- **"Fusion not installed" hints now honor the SDK Tier-2 cardinal rule (found on a fresh-AD test).**
  Testing a first-time run on a clean AD 1.9.74 VM (ADOMBASELINE, no Fusion) showed the bridge's
  not-installed hints said "Fusion cannot be installed programmatically... ask the user to install it
  from autodesk.com... then retry" - a breach of the rule that the AI must OFFER to install the host
  app itself and NEVER tell the user to do it manually. All 4 not-installed hints (+ the `/status`
  tooltip) now offer AI-driven install via the `fusion-onboarding` skill (silent-install + Autodesk
  sign-in). Note: the correct probe for a "what EDA tools are installed" sweep is AD's
  `bridge_readiness` (side-effect-free aggregate with `edaToolsInstalled` + per-bridge `state:no-app`);
  verified it reports Fusion cleanly on the fresh VM via this bridge's `detect` block.

## 1.6.9 - 2026-06-30

- **`fusion_start` no longer dead-ends on a slow first launch (fixes #63, Arav's blocker 1).** A first
  launch (fresh install: component downloads/updates/cloud sync) can take 2-4 min, far past AD's ~60s
  relay request cap - so `{"timeout":300}` could never win and the call looked like a hard failure.
  Now the launch confirms the Fusion process, then waits for the add-in only within a budget that
  stays UNDER the relay cap; if the add-in isn't up yet it returns `{success:true, stillLaunching:true,
  statusVerb:"fusion_readiness"}` with a `_hint` to POLL `fusion_readiness` until `ready:true` (never
  re-call `fusion_start`). Fast/normal launches still return "launched successfully" as before. The
  USER-skill cold-start table gained a `stillLaunching` row.

## 1.6.8 - 2026-06-30

- **Stale add-in is no longer a SILENT failure (fixes #55, Drew's report).** The add-in now REPORTS
  its version (`AdomBridge.manifest` → `/health` + `/status`), and the bridge compares it to the
  version it bundles. `fusion_readiness` and `fusion_addin_status` now return `addinVersion` /
  `expectedAddinVersion` / `addinStale` (+ a re-sync `_hint`), and any add-in command the running
  add-in is too old to know (e.g. `open_by_urn`, which silently broke `fusion_aps_open` for Drew) now
  comes back with `errorCode:"addin_stale"` and the exact fix (`fusion_stop` then `fusion_start` to
  re-sync from cache with Fusion closed). Add-in manifest bumped 1.0.2 → 1.0.3.
- **Bridge-SDK re-audit against the updated SDK.** (1) `bridge.json` `verbs[]` re-synced to the
  authoritative `fusion_describe` catalog (was stale: 40 → 53 verbs; added the library/3D + board +
  app-state verbs; `statusVerb` `fusion_get_app_state` is now listed; dropped the dead
  `fusion_apply_3rdpartyfab_rules`). (2) The USER skill's cold-start section now encodes all **8** SDK
  conventions (was 5), led by an `errorCode` response→meaning→action table (incl. the new
  `addin_stale` row) and the reuse-don't-relaunch + surface-`_hint`-verbatim conventions.

## 1.6.7 - 2026-06-30

- **AD 1.9.63 alignment (discussion #57).** (1) The bridge HTTP server now honors **`ADOM_BIND_HOST`**
  (`os.environ.get("ADOM_BIND_HOST", "127.0.0.1")`) instead of a hardcoded host, so it never binds a
  public interface and never trips the Windows Firewall "allow access?" prompt AD guarantees users
  won't see. (2) Docs/skills updated to the new **runtime-provisioning** reality: AD provisions Python
  itself (system if on PATH, else a portable no-UAC python-build-standalone copy pinned to 3.12.13,
  spawned by absolute path) - the bridge must **never** bootstrap/download its own Python; state is
  checkable via AD's new `runtimes` verb. No self-bootstrap code existed to remove; only the
  "auto-installs Python" wording in SKILL.md / fusion-onboarding / CLAUDE.md was corrected.

## 1.6.6 - 2026-06-30

- **Bridge-SDK self-audit pass.** bridge.json: `homepage` now points to the wiki page (was github - a
  stale default); removed the redundant top-level `healthEndpoint` (it lives inside `spawn`); added
  `spawn.persistent` + an explicit `prewarm: {assets: []}` (Fusion is a detected HOST APP + AD
  auto-installs Python, so nothing to prewarm). `discovery_triggers` rewritten to 16 everyday
  user-task phrasings (dropped verb-name/internal jargon) + a `discovery_pitch`. The USER skill now
  opens with a cold-start "read before you panic" + `fusion_readiness` pointer. `fusion-bridge-dev`
  (a dev skill) is no longer shipped in the user skills pkg (source-only).

## 1.6.5 - 2026-06-30

- **Host-app DETECTION declared (AD >=1.9.47).** bridge.json gains a `detect` block
  (`hostApp: "Fusion 360"`, Windows `%LOCALAPPDATA%\Autodesk\webdeploy\production\*\FusionLauncher.exe`
  mirroring `fusion_detect.py`, macOS `/Applications/Autodesk Fusion[ 360].app`) so AD DETECTS Fusion
  (the user's host app) instead of trying to install it. The status path fast-fails cleanly when
  Fusion isn't running.
- **NEW `fusion_readiness` verb** - a fast `{installed, running, ready, hostApp, bridgeVersion}` check
  that never launches Fusion; the `fusion-onboarding` skill now opens with it (and AD's
  `bridge_readiness`). CLAUDE.md notes AD now auto-installs Python in the background, so the deploy
  trap's spawn case is handled by AD.

## 1.6.4 - 2026-06-29

- **Folder hygiene, part 2: `make_3d_package` no longer saveAs-es to the project ROOT either.** v1.6.3
  fixed the upload target but the f3d `saveAs` still landed in the shared root - now it saves into
  `Adom AI Workspace/<task>` (found/created in the active project), so a build leaves ZERO loose files
  in root.
- **Dialogs are dismissed in the BACKGROUND now (no focus steal).** New `_dismiss_dialogs_bg` uses
  `close_window` (WM_CLOSE via PostMessage = Cancel/No), NEVER `send_key`/Escape (which calls
  SetForegroundWindow and yanks Fusion to the foreground). `fusion_capture_library_views` now
  auto-dismisses the "Create new symbol/footprint '<name>'?" prompt that pops when a DEVICESET name is
  passed to `.sym`/`.pac` in a shared-footprint library (the symbol/package is named by the combo, e.g.
  `R-0603`, not `R-0603-10K`) - and its `_hint` + the `fusion-driving` skill now teach the right names
  + the background-dismiss rule.

## 1.6.3 - 2026-06-29

- **Cloud folder hygiene - the bridge NEVER writes loose files to a project ROOT anymore.** Root cause
  of a real incident: `fusion_make_3d_package` defaulted its upload to the shared Adom project ROOT, so
  a big build dumped 100+ loose f3d files into the team's root and colleagues complained. Now uploads
  default to an AI-owned **`Adom AI Workspace`** folder with a **per-task subfolder**
  (`build_library_3d` derives the task from the .lbr name), and `fusion_aps_upload` **REFUSES** a known
  root folderId (`errorCode: refused_root_upload`). New **`fusion-cloud-hygiene`** skill.
- **NEW `fusion_cleanup_cloud_files`** - precisely delete a list of cloud files **by lineage urn** (not
  by name), so cleaning up AI clutter can't touch a teammate's file in a shared folder. The add-in
  delete gained a `fileId`/`lineageUrn` exact-match path.

## 1.6.2 - 2026-06-29

- **`fusion_capture_library_views` settle fix.** It fired the screenshot ~0.4s after `EDIT`, before
  the editor actually switched - so a shot could capture the PREVIOUS part (a mislabeled view). Now it
  waits a tunable **`settle`** (default 1.5s) after each `EDIT` before framing + capturing, so the shot
  matches its label.
- **Skill: the SHARED-FOOTPRINT pattern for whole-catalog libraries.** `fusion-multipart-libraries`
  now documents how a 138-part library is really 10 footprints + 138 devicesets: collapse the catalog
  to its unique `(type, package)` combos, `fusion_build_library_3d` the ~10 bases (10 uploads, not
  138), then clone each base deviceset per value (sharing its symbol + package + 3D binding). Proven
  by building the entire Adom basic parts library (138 parts, all Mapped + rendering).

## 1.6.1 - 2026-06-29

- **NEW `fusion_capture_library_views` - the views EEs actually trust.** Given a list of packages
  (and the .lbr open), it runs `EDIT <deviceset>.sym/.pac/.dev` + `WINDOW FIT` + a background
  screenshot and returns, per part: the **SYMBOL** (full pinout), the **FOOTPRINT** (pad array +
  Copper/Silkscreen/Mask layer stack), and the **COMPONENT** view (Content Manager: symbol + the
  package table with the footprint<->package **Mapped ✓** + pin/pad counts). The 3D before/after
  shots never showed these.
- **`_inject_package3d_bindings` is now MERGE-AWARE + idempotent.** It reads bindings already in the
  .lbr and accumulates the new ones (new wins), so re-running `fusion_build_library_3d` for just the
  parts that failed NO LONGER wipes the parts that already succeeded (the "a subset re-run dropped my
  earlier bindings" trap). `fusion_build_library_3d` now reads its own `outLbrPath` when re-run, each
  part **self-retries once** on a transient first-part failure, and its `_hint` documents the
  **relay-timeout reality** (a many-part run exceeds the ~60s relay request timeout but keeps running
  server-side - verify by the bound .lbr + the PNGs, then re-run any missing parts).

## 1.6.0 - 2026-06-29

- **NEW `fusion_build_library_3d` - build a whole RENDERING multi-part 3D library in ONE call.** Give
  it the combined `.lbr` + a `parts[]` list (`{package, lbrPath, modelPath}`); for each part it makes
  the footprint+chip f3d package (no GUI dialogs), collects the `wip_urn`, then **injects ALL bindings
  in one pass** (`_inject_package3d_bindings` - the EAGLE `<packages3d>` + per-device
  `<package3dinstances>` surgery the AI used to hand-write) and opens the finished library once.
  Returns per-part urns + a `screenshots[]` of every before/after. Each part is independent, so a
  Fusion reset mid-run just means re-running the failed parts.
- **`fusion_make_3d_package` got two upgrades:** (1) `projectId`/`folderId` are now **optional** -
  they default to the MAIN-project upload folder (`_discover_upload_target`), so the AI no longer has
  to `fusion_aps_browse` for them; (2) pass **`captureLabel`** to auto-capture a **BEFORE** (the
  footprint in the 3D viewer) and **AFTER** (the chip placed flat on its pads) hwnd-targeted
  background screenshot, returned as `before`/`after`. A viewport `fit()` frames each shot.
- Both verbs added to `fusion_describe` (the Verbs tab) and gated as Fusion-required.

## 1.5.13 - 2026-06-29

- **Rich teaching hints on `fusion_make_3d_package`:** its `_hint` now spells out NEXT step (the exact
  binding markup), what to EXPECT (check_dialogs==0, footprint+chip preview, pad alignment), the
  SPEEDUP (batch per part -> hand-merge all -> open once), and the PITFALLS (needs Fusion running; a
  raw STEP renders nothing; never FINISH the Package3D generator). Skill gained a matching
  "Speedups + what to expect" section.

## 1.5.12 - 2026-06-29

- **`fusion_make_3d_package` now produces a PROPER component package (footprint + chip).** It opens the
  lbr and runs `Electron.Create3DPackage` to load the package's FOOTPRINT into the generator, imports
  the chip ONTO it, orients flat, and `saveAs` an f3d - so the 3D model contains the footprint
  (pads + courtyard) AND the chip, merged and **aligned** (the chip's pads land on the footprint pads
  in the 3D viewer, which is the whole point of that viewer). The 1.5.11 verb imported into an empty
  Design, giving a chip with NO footprint. New required arg: **`lbrPath`**. Proven on the full 10-part
  library (`AdomBasicParts3D_v2`).
- `fusion-multipart-libraries` skill updated to the footprint-aware recipe.

## 1.5.11 - 2026-06-29

- **NEW `fusion_make_3d_package` verb - a RENDERING per-package 3D model with ZERO GUI dialogs.** It
  imports a STEP into a throwaway Design, orients it flat, saves an `.f3d`, uploads it via APS, and
  returns a `fs.file:vf` `wip_urn` to hand-write into a library's `<packages3d>`. This is the
  dialog-free replacement for the Create3DPackage generator (whose FINISH is trapped behind two
  unbeatable "Fusion360" CEF modals). **Proven on a full 10-part Adom basic-parts library
  (R/C/L/LED/nRF54L15) - every package renders its real chip, flat on its footprint.**
- **`fusion-multipart-libraries` skill rewritten as SOLVED** - the f3d recipe end to end + the key
  gotcha (a *raw STEP* upload binds but does NOT render: "Thumbnail download failed"; an *f3d* renders)
  + the GUI-generator dead end to avoid.
- New skills **`fusion-driving`** (read the dialog/owned-popup array after every mutating op; never
  fullscreen-capture) and **`fusion-cloud-save`** (Hub-upload / saveAs discipline); `fusion-libraries`
  cross-linked to both.
- README + SKILL now link the component-library docs (the skills + `CREATING_BASIC_PARTS_LIBRARIES.md`).

## 1.5.10 - 2026-06-29

- Dialog-capture false-positive fix: a docked panel (Browser/Timeline) shares the "Fusion360" title
  but does NOT disable the main window, so a generic-titled candidate is now only surfaced when the
  main window is actually disabled (a real modal is up). Stops the auto dialog-capture crying wolf.

## 1.5.9 - 2026-06-28

- **Auto dialog-array capture after every mutating `fusion_*` verb:** enumerates + screenshots any
  owned popup / modal by hwnd (background, never fullscreen) and returns it plus an "analyze this
  before proceeding" hint, so the AI cannot fly blind past a Save / "are you sure you want to close" /
  recovery dialog.
- **NEW `fusion_check_dialogs` verb** - poll the current dialog/owned-popup array on demand.
- Dialog classifier: new rules for the Hub upload-in-progress ("packages are being uploaded... lose
  these changes") and "save was cancelled / cannot be saved while packages are being uploaded" modals;
  the generic/unknown fallback no longer says "dismiss" (which could click Yes on a data-loss confirm).

## 1.5.8 - 2026-06-28

- `bridge.json`: added `updateManifestUrl` (+ `healthEndpoint`) so Adom Bridge auto-update works (it
  was null, which broke auto-update - discussion #31).

## 1.5.7 and earlier

- `fusion_attach_3d_package` (GUI Package3D generator flow - superseded for multi-part work by
  `fusion_make_3d_package`), APS fast cloud search, the Electronics workflow verbs, and exports. See
  the git history on the wiki page.
