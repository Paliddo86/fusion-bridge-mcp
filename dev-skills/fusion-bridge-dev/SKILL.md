---
name: fusion-bridge-dev
description: How to develop, version, and PUBLISH the Adom Bridge Fusion 360 bridge — where the source canonically lives, and the rule to ALWAYS publish to the wiki AND push the GitHub backup on every change. Read before editing/shipping the fusion bridge. Trigger words — fusion bridge dev, publish fusion bridge, fusion bridge repo, where does the fusion bridge live, ship fusion bridge, bump fusion bridge, fusion-bridge.
---

# Fusion bridge — dev + publish workflow

## ★ THE CORE GOAL: the AI runs EVERYTHING so the user never has to (why this bridge exists)

The entire reason adom-bridge and its bridges exist is so that **the AI drives the whole flow
end to end and the user does nothing manual** (John, 2026-07-06). This bridge is a FAILURE any time
it punts a step back to the user. That means the code + verbs must automate, on their own:
**install → launch (background) → Autodesk sign-in → seat/licensing conflicts → every startup dialog
→ recovery/update prompts → the actual work (open/model/export).**

- **NEVER ask the user to do something manually** - not "click Continue", not "sign in yourself",
  not "close that dialog". The ONLY acceptable exceptions are a true hard wall the OS forbids us to
  cross: a **UAC/elevation** prompt, or a **password/2FA** you must not enter for them. Everything
  else you AUTOMATE. If you catch yourself about to ask the user to click/close/choose something,
  that is a bug in this bridge - fix the bridge so it does it.
- **Detect and handle state in CODE, deterministically** - do NOT rely on the AI noticing a dialog
  and following a skill (the AI is unreliable at that). Hard signals (add-in round-trip, owning
  process, window size, `family_windows()`), not titles or vibes. See the background-launch state
  machine + `find_licensing_dialog()`.
- When something can't yet be fully automated (e.g. a browser OAuth handoff), the goal is still to
  push automation as far as it goes and shrink the manual part to zero over time - not to hand it back.

Hold every change to this bar: does it move the bridge toward "the user does nothing"? If a flow
still needs the user, that's the next thing to fix, not the user's job.

## ⛔ Background-launch invariant: NEVER steal the user's foreground (HARD RULE)

The bridge runs on a machine the **user is actively working on**. Anything this bridge launches or
opens must appear in the **BACKGROUND** and must **never yank focus** off what the user is doing
(John, 2026-07-06: "make sure new windows ALWAYS only open in the background so as not to disturb my
main work"). This is not best-effort etiquette — it is enforced **in code**, and any new window-
opening path you add MUST follow it:

- **Launch minimized + not-activated.** Start GUI processes with `STARTUPINFO.wShowWindow =
  SW_SHOWMINNOACTIVE (7)` + `CREATE_NO_WINDOW`. See `_popen_fusion_background()` in `fusion_detect.py`
  — copy that, never a bare `subprocess.Popen([exe])` (which opens Fusion in the foreground).
- **Guard the startup window — match the whole Autodesk FAMILY, not just `Fusion360.exe`.**
  `STARTUPINFO` alone is not enough: Fusion re-foregrounds itself several times as it initializes, and
  the noisiest offenders are NOT `Fusion360.exe` — the splash "Loading additional modules" is
  `FusionLauncher.exe`, and the "Signing in" + **"Active Sessions Exceeded"** dialogs are
  `AdskIdentityManager.exe`. A guard scoped to only `Fusion360.exe` lets those yank focus (the exact
  bug John hit 2026-07-06). `ensure_fusion_running()` fires `_start_focus_guard()`: a daemon that, for
  ~150s, minimizes (no-activate) **only** the foreground window when it belongs to the Autodesk launch
  family (`_FUSION_FAMILY_IMAGES` = Fusion360 / FusionLauncher / AdskIdentityManager / …, matched by
  process image via `_fusion_family_pids()`). It acts ONLY on focus-theft, so it never fights a user
  who deliberately foregrounds Fusion. Any window-family you add MUST be in that set.
- **⚠️ ctypes HANDLE TRUNCATION — the silent killer (this bit us 2026-07-06).** Any Win32 call that
  RETURNS or ACCEPTS a handle (`HWND`, `HANDLE`) MUST have its `restype`/`argtypes` set, or ctypes
  defaults to `c_int` (32-bit) and **TRUNCATES the handle on 64-bit Windows**. `GetForegroundWindow()`
  with no `restype` handed back a garbage HWND, so `ShowWindowAsync` silently no-op'd and the focus
  guard minimized NOTHING for two releases (1.6.33/1.6.34) while looking correct — the user still had
  to alt-tab back themselves. Always: `user32.GetForegroundWindow.restype = wintypes.HWND`,
  `ShowWindowAsync.argtypes = [wintypes.HWND, c_int]`, etc. VERIFY by objective window state, not by
  eye or by sampling the foreground (a 0.1s-poll guard minimizes too fast to catch in a snapshot):
  `GetWindowPlacement(...).showCmd == 2` (SW_SHOWMINIMIZED) on the family window == guard is working.
- **The guard only runs on a FRESH launch.** `ensure_fusion_running()` starts it after it spawns the
  process; if Fusion is ALREADY running, `fusion_start` returns early and the guard never fires, so an
  already-foreground Fusion stays put. When you need a clean background state, `fusion_kill` first,
  confirm the process is gone, THEN launch.
- **Seat conflict ("Active Sessions Exceeded") = AUTO-RESOLVE via UIA, in the background, never ask
  (SOLVED live 2026-07-06 after a very long day).** The definitive facts, hard-won:
  - It is a **Qt** dialog (`automationId` = `QTApplication.QTSessionBlockedStartup...`), NOT CEF.
  - Its window **TITLE is literally "Fusion360"** — the seat text is body content — so NEVER detect by
    title. There are **multiple variants of different SIZES**: "Active Sessions Exceeded" (~1235x527,
    with radios) and the **"Suspend Remote Session" confirm (only 823x262)**.
  - **DETECT by the OWNED-POPUP signal, not a size window (fixed 2026-07-06 v1.6.46/1.6.47 after a
    fragile 320px height floor silently MISSED the 262px confirm and readiness lied that it was gone).**
    A seat/sign-in modal is ALWAYS an **owned popup** of the main Fusion window (`GetWindow(hwnd,
    GW_OWNER)` != 0; confirmed by `desktop_screenshot_window`'s `ownedPopupCount` on the main window).
    The splash and the maximized main app are NEVER owned popups. `find_licensing_dialog()` returns the
    owned family window that is **dialog-SHAPED: width ≥ 480** (excludes Fusion's own ~300px-wide side
    panels — the Data Panel/browser rail are owned popups too, and matching them false-blocked `ready`
    the moment a doc was open) **and height ≥ 180** (excludes toolbar strips), smaller than the maxed
    main app. ctypes `GetWindow` MUST have `restype/argtypes=HWND` set or the owner handle truncates.
  - Its native **"Continue"** button IS exposed to **UIA** (`desktop_ui_click {hwnd, name:"Continue"}`),
    and UIA **Invoke runs in the BACKGROUND — no foreground, no focus steal, no cursor move**. Suspend
    is the pre-selected radio, so Continue grabs the license. (The radios themselves are web content
    NOT in the a11y tree — do NOT depend on selecting them.)
  - **DO NOT** kill+relaunch to "free the seat at the source": the Autodesk licensing SERVER keeps the
    seat even when the other machine is OFF, so that never clears it. `_reclaim_seat_from_peers()` is
    dead code, kept only as a comment-marker — do not resurrect it.
  - **DO NOT** coordinate-click it (`desktop_click`/SendInput): that needs foreground (disturbs the
    user) AND the focus-guard fights you by minimizing the dialog.
  - **RESOLVE + SCREENSHOT-VERIFY.** The FIRST UIA Invoke on a mid-render dialog silently NO-OPs, so
    `_resolve_seat_dialog()` re-invokes "Continue" and confirms it TRULY cleared by re-reading the
    parent window's `ownedPopupCount` via `desktop_screenshot_window` (parent/child screenshotting) —
    NOT by re-running the size heuristic that once false-'resolved' it. Do the screenshot check;
    "I clicked it" is not "it's gone."
  - **AUTO-RESOLVE ANYWHERE, in CODE, not just the launch loop.** The seat dialog also appears LATER —
    minutes after sign-in, when the license server notices too many sessions — long after the launch
    loop exits. So `fusion_readiness` SELF-HEALS: whenever it detects the dialog it auto-resolves it in
    the background and re-probes. The seat is handled deterministically in code no matter when it shows
    up; the AI never has to notice or act (John: "do all this tracking from your code ... the ai never
    follows skills"). A STALE add-in port (8774 from a just-killed instance) answers while the CURRENT
    Fusion is blocked, so the seat check takes PRIORITY over the add-in probe in both paths.
- **The add-in still loads while minimized** — background operation (open/model/export) needs no
  visible window. So minimized costs you nothing.
- **Foreground is opt-in, only to SHOW the user something.** The ONLY time you may foreground/restore
  a Fusion window is when a verb's explicit job is to show the user (and even then, per the user's
  standing rule, you surface the exact window + their viewer). Never `SetForegroundWindow` as a side
  effect of automation.
- **Applies to every window surface**, not just the Fusion launcher: any child process, helper, or
  dialog you spawn opens background/no-activate by the same means. When you add one, add a focus-guard
  or launch flag — do not assume it will behave.

If you catch yourself about to `Popen` a GUI or call a foreground API, stop and route it through the
background-launch helpers. A window that steals the user's focus is a **regression**, full stop.

## You OWN this bridge (do NOT file your own bugs as adom-bridge requests)
AD core only HOSTS/LOADS this bridge: the `adom-bridge` CLI, the relay/passthrough, bridge lifecycle
+ streaming this bridge into `…\bridges-cache\fusion360`, and the core `desktop_*` verbs. **Everything
`fusion_*` is yours** - the verbs + their `_hint`s, the APS integration (auth/config/quota; port 8910
is AD's vestigial native APS, this bridge uses 8917), the add-in code AND its deployment into Fusion,
the SKILL.md, the publish loop. A `fusion_*` bug is a bug in THIS repo: fix it here and republish,
never as an adom-bridge discussion ask. (See the repo `CLAUDE.md`.)

## Your wiki structure: ONE page, THREE artifacts (know this cold)

You are a single wiki page (`wiki.adom.inc/adom/fusion-bridge`) that ships **three distinct
things**, each with a different consumer:

1. **The page git repo — the ONE source of truth.** Everything lives here: `server.py`, `aps.py`,
   `addin/`, `handlers/`, `skills/`, the `*.md` docs, **`page.json`** (the skills-pkg manifest), and
   **`adom-bridge-fusion-manifest.json`** (the bridge manifest). Edit here, push here (+ GitHub backup).

2. **The Release (the zip) + the bridge manifest — consumed by ADOM-DESKTOP on `bridge_install`.**
   - The **Release zip** (`adom-bridge-fusion-v<ver>.zip`, uploaded via `adom-wiki release upload`) is the
     BUILT bridge (server + add-in). Binaries/zips are **Releases, NEVER git** (`*.zip` is gitignored and
     `repo push` silently skips it — see PUBLISHING.md).
   - The **bridge manifest** (`adom-bridge-fusion-manifest.json`, text, in git) tells adom-bridge **who you
     are**: name, version, the `url` pointing at the Release zip, `sha256`, `size`, verb prefixes, health
     endpoint. adom-bridge streams that zip by URL into its bridge cache and hash-verifies it.

3. **The adom-wiki pkg (`adom-wiki pkg install adom/fusion-bridge`) — NOTHING BUT SKILLS.**
   It gives a user's container the skills to know how to talk to YOU and to Fusion. `page.json` is its
   manifest; `install.sh` copies the top-level `SKILL.md` + every `skills/*/` into `~/.claude/skills/`.
   **No binaries belong in this pkg** — that's what the Release is for.

### The install handshake (how a container gets your skills)
- An AI asks adom-bridge to install you → adom-bridge installs the **Release** (via the manifest) AND
  **tells the AI to `adom-wiki pkg install` your pkg** → the container gets ALL your skills.
- An AI may also `adom-wiki pkg install` you on its own. Then your skills **auto-update via the pkg
  mechanism** on every refresh. **THAT is the goal** — your skills always current, everywhere.

### The hero gets unlinked — re-check it
You have a hero we worked hard on (`screenshots/hero.png`, declared in `page.json`). **The wiki sometimes
UNLINKS it after certain updates (e.g. a `pkg publish`).** Every so often, verify `page.json` still
declares the hero and the page still shows it; **re-link it if it dropped.** TODO: file a bug to the
`adom/wiki` discussion about the hero getting unlinked on updates.

### Your STANDING JOB: document EVERY verb in skills
The skills pkg must document **absolutely every verb you support** — a **core skill** + **sub-skills** for
the extra areas. For each verb capture: what it does, **why** it exists, **when** to use it, why it's
**valuable**, and its **pitfalls**. Walk `fusion_describe` / `VERBS.md` and ensure each verb has skill
coverage. **Constantly refine and expand** — this is never "done." (Current coverage: fusion-electronics,
fusion-libraries, fusion-eagle-commands, fusion-aps-search/signin, fusion-onboarding; the full per-verb
sweep is ongoing.)

## How the add-in actually deploys (VERIFIED by shipping v1.5.1, 2026-06-28)
Fusion loads the add-in from a per-user dir Autodesk has MOVED across versions: **2025+ Fusion
scans `%APPDATA%\Roaming\Autodesk\FusionAddins\AdomBridge\`** and SILENTLY ignores the legacy
`%APPDATA%\Roaming\Autodesk\Autodesk Fusion[ 360]\API\AddIns\AdomBridge\` dirs (issue #63,
root-caused live: an add-in only in the legacy dir never loads). `install_addin.py` installs to ALL
locations (TARGET_CANDIDATES); if a future Fusion stops loading it, FIRST suspect the dir moved
again. ⛔ NEVER ask the user to restart Fusion / enable the add-in - YOU restart
(`fusion_stop`+`fusion_start`; `runOnStartup` does the rest). Legacy note (superseded):
the add-in previously lived only at `...\API\AddIns\AdomBridge\`,
NOT from this repo or the cache. `install_addin.py` (`_sync_directory`) copies `addin/AdomBridge` there.
**`bridge_install` only updates the CACHE (`method: in_place_merge`); it does NOT push the add-in into
Fusion.** And the sync canNOT overwrite the add-in while **Fusion is running** (it holds the files open),
so a bridge-server respawn alone does nothing. The exact sequence that worked (after the wiki publish):
1. `bridge_install {"manifestUrl": ".../adom-bridge-fusion-manifest.json"}` (add `"force": true` to re-merge).
   Its OWN output says: "AD reaps + respawns it from the new cache on the next call (no manual kill needed)."
   So do NOT run `bridge_kill` - AD restarts the bridge SERVER (new server.py/describe.py) automatically on
   the next verb. (I ran `bridge_kill` once here; it was pointless.)
2. **`fusion_stop`** (graceful; `fusion_kill` if wedged) - REQUIRED, releases the add-in file locks. Skipping this leaves the OLD add-in live.
4. Run the cache's `install_addin.py` (`cd <cache>\fusion360 && python install_addin.py`); it prints
   `Updated: commands\cloud_documents.py ...`.
5. **Verify by SHA256** (`Get-FileHash` on the Roaming file == `sha256sum` of that file unzipped from the
   published v<ver>.zip). NEVER use `findstr` - it false-negatives on these UTF-8 files (dash/box chars).
6. `fusion_start`, then confirm a `fusion_*` verb behaves new.
- A bumped `BRIDGE_VERSION` does NOT prove the running add-in changed - always hash-verify (step 5).
- NEVER hand-edit the cache or the deployed Roaming copy - edit this source, publish, reinstall.

## Canonical source = the wiki page (the ONE source of truth)
**`https://wiki.adom.inc/adom/fusion-bridge`** (owner `adom`, public). A wiki
page IS a git repo; this is THE main repo for the bridge. AD installs the bridge from here
(its manifest + zip). Every version ships here.

- Local working clone: `~/project/fusion-bridge`.
- Files on the page: `bridge.json`, `BRIDGE_VERSION`, `README.md`, `SKILL.md`, `aps.py`,
  `server.py`, handlers/addin, the `adom-bridge-fusion-manifest.json` + version zips
  (`adom-bridge-fusion-v<ver>.zip`), `adom-bridge-fusion-source.tar.gz`, and
  `adom-bridge-fusion.bundle` (portable full git history).

## GitHub backup — ALWAYS push it too
`github.com/adom-inc/fusion-bridge` (renamed from the old `adom-bridge-fusion`
to match the wiki slug). It is a **backup mirror**. **On EVERY change you publish to the
wiki, also `git push` to this GitHub repo.** Never let them drift again (they were 6 commits
out of sync once — that's the confusion we're preventing). Wiki = canonical, GitHub = backup,
keep them in lockstep.

## The publish ritual (every version)
1. Edit in `~/project/fusion-bridge`. Bump `BRIDGE_VERSION` + `bridge.json`
   `version` together.
2. `git commit` locally.
3. **Push to GitHub backup:** `git push origin main`.
4. Publish to the wiki via the raw `/files` API (the wiki CLIs are dead) — token
   `${ADOM_WIKI_TOKEN:-$(cat /var/run/adom/api-key)}`:
   `curl -X POST -H "Authorization: Bearer $TOK" -F "file=@<f>" \
    https://wiki.adom.inc/api/v1/pages/fusion-bridge/files`
   Upload: the new `adom-bridge-fusion-v<ver>.zip`, the `adom-bridge-fusion-manifest.json`
   (`{manifest_version,name,version,url,sha256,size,released_at}`, url = absolute `/files/`
   path), `bridge.json`, `BRIDGE_VERSION`, a fresh `adom-bridge-fusion-source.tar.gz`
   (`git archive`) and `adom-bridge-fusion.bundle` (`git bundle create … --all`).
5. **ANON-VERIFY**: re-download the zip + sha256-match, fetch the manifest, confirm version.
6. Install on a Fusion box: `adom-bridge --target <laptop> bridge_install
   '{"manifestUrl":".../adom-bridge-fusion-manifest.json"}'`, then reap the old instance
   (`bridge_kill` on AD ≥1.8.187, else taskkill the PID on its runtimePort) so the new code
   loads, and verify via `bridge_log_read` / a `fusion_*` verb.

## The stale seed — do NOT publish from it
`adom-bridge` (private GitHub) → `plugins/fusion360/` was the bridge's ORIGINAL home and is
now a **frozen first-run fallback only** (AD bundles it offline). It is STALE and NOT
canonical. `release-bridge.sh` refuses to publish it. There is a `STALE.md` in that folder
pointing here. Never treat `plugins/fusion360` as the source; never publish from it.

Related: [[project_adom_desktop_fusion_bridge]], skills `fusion-aps-search`, `fusion-aps-signin`.

## Publishing a new version (zips are RELEASES, not git files)

Full guide: PUBLISHING.md. The one rule that bites everyone: the build **`.zip` is a release asset**
(`adom-wiki release upload <ref> <ver> <zip>`), NOT a git file - `*.zip` is gitignored and
`adom-wiki repo push` silently skips binaries (reports `ok`, but the URL 404s). Only the small
**manifest JSON** goes in the git repo (`adom-wiki repo push`), and its `url` points at the release
download URL (`/download/<org>/<slug>/<ver>/...zip`) with `sha256`/`size` matching the served asset.
Then `bridge_install {manifestUrl, force:true}`. Server-only change (server.py/describe.py/handlers) =
that's it (AD respawns the bridge server). Add-in change (addin/) = ALSO `fusion_stop` ->
`install_addin.py` -> hash-verify (Get-FileHash, not findstr) -> `fusion_start`. Community: fork the
page, release to your fork, `bridge_install` your manifest to test, then PR back.

## Publishing the SKILLS pkg (`adom-wiki pkg publish`) — the maze, SOLVED (2026-06-28)

The skills pkg (`adom-wiki pkg install adom/fusion-bridge`) is SEPARATE from the Release.
Publishing it from this repo (which also holds `server.py`/`addin/`) was blocked by a maze; two fixes in
`page.json` unblock it:

1. **Make it skills-only with a `files` allowlist** — else the dep-checker scans `server.py` and flags its
   `import aps/describe/handlers/eagle_lbr/adsk/PIL` as ~20 "undeclared deps" (false positives the SERVER
   enforces, not just lint). Ship only what the pkg needs:
   ```json
   "files": ["SKILL.md","skills/**","screenshots/hero.png","README.md","VERBS.md","install.sh",
             "uninstall.sh","page.json","MAKING_LIBRARIES.md","LIBRARY_FINDINGS.md","PUBLISHING.md",
             "CREATING_BASIC_PARTS_LIBRARIES.md"]
   ```
   Use `skills/**` (a bare `skills/` matches nothing and errors).
2. **The publish DEMANDS a billboard hero** (`{type:billboard,headline,subhead,screenshot}`); it rejects
   the page's `{type:image,path}`. So set billboard ONLY to get past publish:
   ```json
   "hero": {"type":"billboard","headline":"Adom Bridge - Fusion 360 Bridge",
            "subhead":"Drive Fusion 360 from the cloud: electronics, exports, fast APS cloud search, and component libraries with real 3D",
            "screenshot":"screenshots/hero.png"}
   ```
   The image must be in the repo (curl `/files/screenshots/hero.png` if your working copy lacks it).
3. `adom-wiki pkg publish --skip-lint` (the "prompt-injection"/"no video" warnings are soft false
   positives). Bump `page.json` `version` each time; add `discovery_triggers` so AIs surface it.
4. **⛔ IMMEDIATELY REVERT THE HERO** (this is the bug John warned about). `pkg publish` overwrites the
   page's hero with the billboard form, which the PAGE renders as a generic template (your custom
   full-bleed `hero.png` shrinks to a thumbnail on the right). **Right after publishing, push the page
   hero back to the image form** — via `adom-wiki repo push` (NOT another `pkg publish`, which re-breaks it):
   ```bash
   # page.json: "hero": {"type":"image","path":"screenshots/hero.png"}
   adom-wiki repo push adom/fusion-bridge --files page.json -m "restore full-bleed hero"
   ```
   The pkg stays published (registry snapshot is immutable); only the page display is restored.
5. **VERIFY**: `adom-wiki pkg install adom/fusion-bridge` deploys skills to
   `~/.claude/skills/`, AND re-open the live page to confirm the **full-bleed custom hero renders** (not
   the thumbnail-in-template). Then `git commit` + `git push` `page.json` (image form) to GitHub for
   lockstep. NOTE: pkg version (`page.json`, 1.6.x) is a SEPARATE track from the Release/bridge version
   (`bridge.json`/`BRIDGE_VERSION`, 1.5.x). TODO: this publish↔hero conflict is a wiki bug — filed to
   `adom/wiki`.

## Bundle deep READMEs INTO each skill (so the installed AI actually reads them)

When a skill points at companion docs (a step-by-step guide, a findings log), **copy those READMEs INTO
the skill's own folder** (next to its `SKILL.md`) and link them with **plain local paths**
(`[GUIDE.md](GUIDE.md)`). Do NOT use repo-relative links (`../../GUIDE.md`) - they resolve in the repo but
**break once the skill installs** to `~/.claude/skills/<skill>/` (there is no repo root above it). And do
NOT rely only on wiki URLs - a remote fetch is a step the AI may skip; a file sitting right next to the
skill gets read. In the bundled copies, rewrite image refs to **absolute wiki URLs**
(`https://wiki.adom.inc/<org>/<slug>/files/<img>.png`) so screenshots still resolve. The repo-root copies
stay for the wiki Files tab; **re-sync the bundled copies on every publish** (see PUBLISHING.md). Net: the
installed skill carries the full content locally + always-current.

## Install-completeness detection (fusion_detect.py) — READ before touching install/readiness

Root-caused live on a fresh Hyper-V VM (John, 2026-07-14). The webdeploy layout is NOT obvious:

- `%LOCALAPPDATA%\Autodesk\webdeploy\production\<hash>\` — a COMPLETE install is ~615 files
  (**`Fusion360.exe`** ~880 KB + hundreds of DLLs + Qt/ Python/). Autodesk ALSO drops a tiny **8-file
  launcher STUB** dir next to it (icons + `FusionLauncher.exe` + a ~412-byte `FusionLauncher.exe.ini`).
  So a healthy install shows **two** hash dirs.
- **The completeness signal is `Fusion360.exe` (full size), NEVER the launcher or its `.ini`.** Two
  earlier "fixes" were WRONG: keying off `FusionLauncher.exe` existing → true mid-stream (it lands
  early) → premature `fusion_start` → *"Error Launching Streamed Application ... .ini missing or
  incomplete"*. Keying off `FusionLauncher.exe.ini` existing/size → also wrong: the **complete app dir
  has NO `.ini` at all**; only the throwaway stub has one, so a size gate would reject a healthy
  install and accept nothing.
- Helpers: `_app_dir_complete()` (Fusion360.exe ≥ 200 KB), `_any_app_complete()`, `_find_fusion_launcher()`
  (returns a launcher only when a complete app exists), `_incomplete_webdeploy_present()` (fresh-install
  "installing" signal = a launcher present but NO complete app anywhere; disk-state, because the
  streamer's tasklist presence is racy — flickered true only 3/25 polls during a real stream).
- `readiness`: gate `installed` on the app binary; `installing = _installer_running() OR
  _incomplete_webdeploy_present()`. **Poll to `installed:true`, not to `installing:false`** (the latter
  is racy). `_clean_incomplete_webdeploy()` no-ops when a complete app exists (never nuke the legit stub).

## Installer zip = RUNTIME ONLY (feedback from the AD thread, 2026-07-17)

The v1.6.76 zip was 16.7 MB - 15.3 MB of it demo MP4s, screenshots, and repo docs, streamed to
EVERY user's machine on bridge_install. Media and marketing belong on the WIKI PAGE (repo/Files),
never in the installer. Build the zip with the runtime whitelist (git ls-files MINUS
.mp4/.png/.jpg/.svg/.gif/.webm/.zip and screenshots/): server.py, aps.py, describe.py, handlers/,
addin/, resources/, skills/ (text), manifests. Result: 456 KB. If the zip is over ~1 MB,
something wrong is inside - list its contents before uploading.

## page.json `brief` vs `description` - they render in DIFFERENT places

The wiki puts these two fields in two different spots, and confusing them turns the Install card
into a README (John: "stop trying to turn that into a mine readme"):

- **`brief`** -> the **page-header one-liner** under the title. This is where the FEATURE COPY goes
  (what the bridge does: libraries, exports, APS search, modeling...).
- **`description`** -> the **Install card body**, directly above the `adom-wiki pkg install` command.
  This must say ONLY what the package install gives you. ~2 sentences. No feature list.

The canonical bridge `description`:

> "Installs this bridge's skills into your container so your AI knows how to drive the bridge. The
> bridge runtime itself is loaded by Adom Bridge from the release zip."

It matters beyond tidiness: users and AIs assume `pkg install` installs the BRIDGE. It does not - it
installs SKILLS into the container. The runtime is the release zip that AD streams via
`bridge_install`. Say that in the Install card, once, plainly.

Filed as SDK-wide asks on adom/adom-desktop: **#20** (every bridge page must state the pkg-vs-runtime
split) and **#21** (document + lint keeping the release zip and pkg tarball free of media bloat).

## Clicking without stealing focus: UIA first, SendInput last (John, 2026-07-22)

The background-launch invariant above covers windows we OPEN. This covers windows we CLICK. Same
principle, different verb set, and it is the one that bit us: `desktop_click` FOREGROUNDS.

> John, 2026-07-22: "try to NEVER bring windows to the foreground cuz its disruptive."

- `desktop_click` / `desktop_double_click` / `desktop_right_click` = **SendInput**. They
  **FOREGROUND** the target and move the real cursor. LAST RESORT only.
- `desktop_ui_click` / `desktop_ui_set` / `desktop_find_control` = **UIA Invoke/SetValue**.
  Programmatic: **no focus steal, no cursor move.** Chromium/Edge expose their a11y tree, so page
  buttons and form fields ARE reachable by accessible name. **Always try these first.**
- Use `_click_background_first()` in `server.py` - UIA first, foreground click only as fallback.
- The ONE documented exception: **Fusion's "Sign In" button is a webview control with no UIA
  node**, so only an image-space `desktop_click` reaches it. That does not license foregrounding
  anywhere else.
- Never raise a window to get attention. **Toast** (`_cli_notify_all`) or flash the taskbar.
- Restoring a MINIMIZED window before capturing it is fine (minimized windows screenshot as a
  ~237x39 title-bar sliver, so clicks land on nothing). Restore != raise-to-front.

### Human walls: toast the user AND tell the AI it can clear it itself

When only a human can clear something (2FA, credentials, a protocol dialog):

1. **Toast from CODE.** `_detect_signin_wall()` classifies the wall from window TITLES (cheap, no
   OCR, no foregrounding); `_notify_signin_wall()` fires a sticky toast. Response carries
   `notifyDelivered` so the AI knows not to nag.
2. **Still hand the AI a mechanical way to clear it.** A wall is not permission to give up - the
   Adom user wants everything automatic. `_wall_hint()` returns exact commands: for an emailed 2FA
   code, read it from Gmail via `adom-google`, then `fusion_signin_2fa {code}`, which types it via
   background UIA.
3. **If the tool that would automate it is missing, upsell it** rather than dead-ending:
   "installing adom-google would let me finish logins like this for you."

Put all of this in response **`_hint`s, not just a skill.** John, 2026-07-22: *"the ai never
follows skills well, so hints back in your cli calls is the way to get the ai to act correctly."*
Skills are reference; hints are what actually change behaviour at the callsite.

## The PROVEN Fusion sign-in: every step, FG vs BG, and why (2026-07-22)

Signing Fusion in was a mess for a long time. This is the sequence that ACTUALLY worked end to
end, live, with the account signed in and `ready:true`. **Only 2 of 10 steps touch the user's
screen.** Do not regress this.

| # | Step | FG/BG | Method that makes it background | Caption |
|---|------|-------|--------------------------------|---------|
| 1 | `fusion_start` (if not running) | **FG** | Fusion raises its own window on launch | **yes** |
| 2 | find Fusion hwnd + rect | BG | `desktop_list_windows` (also returns `rect`) | no |
| 3 | restore if minimized | **FG-ish** | `desktop_set_window_state` | **yes** |
| 4 | click **Sign In** | **BG** | `desktop_ui_click` UIA Invoke | no |
| 5 | capture authorize URL | BG | copy Chrome `History` + `-wal`/`-shm`, sqlite read | no |
| 6 | re-open URL in target profile | BG | `nbrowser_open_window {background:true}` | no |
| 7 | click "Continue with Google" | BG | CDP `nbrowser_click` | no |
| 8 | pick the account by email | BG | CDP `nbrowser_click` | no |
| 9 | click "Open Product" (`autodesk://`) | BG | CDP `nbrowser_click` | no |
| 10 | poll `fusion_readiness` | BG | HTTP | no |

### The three discoveries that made it work

1. **Fusion's "Sign In" button IS in the UIA tree and IS invokable.** I wrongly claimed for a long
   time that it was an un-automatable webview control and foregrounded the window every time.
   ```
   name "Sign In"  role Button  invokable true
   automationId QTApplication.MW1_0...Nu::QTSignInDialog.backgroundWidget.signin_button
   ```
   `desktop_ui_click {hwnd, name:"Sign In"}` clicks it with NO foreground. **Never SendInput it.**
2. **SSO beats password every time.** Click **"Continue with Google"** (or Apple/Microsoft) BEFORE
   typing any email. If the browser profile already has that identity, the whole login completes
   with **no password and no 2FA**. Typing an email first commits you to the password path.
3. **It must be a FRESH flow.** Re-navigating a `flowId` that already advanced lands on the
   PASSWORD screen with no way back to the provider buttons. If the SSO button is missing, restart
   Fusion for a fresh `request_id` rather than fighting the page.

### CDP only drives windows YOU opened

`nbrowser_*` refuses a tab in a window the user (or Fusion) opened: `not_owned`, and there is no
override. That is WHY step 6 exists - re-opening the captured URL in an **agent-owned** window is
what makes steps 7-9 background-drivable. A side effect is a second window, so close the original.

### Branches

| Condition | Detection | Action |
|---|---|---|
| wrong profile | captured dir != target dir | re-open in target |
| right profile already | dir match (work account is often `Default`!) | `already_right_profile`, do NOT open a duplicate |
| URL older than 150s | `ageSec > 150` | `stale` -> restart Fusion for a fresh request |
| no SSO button | button missing | not a fresh flow -> restart Fusion |
| 6-digit email code | window title `2-step verification` | read from Gmail via `adom-google`, submit with `fusion_signin_2fa` (background) |
| password screen | `h1` "Enter your password" | TRUE WALL - toast the user, never type it |
| `autodesk://` dialog | title has "Identity Manager" | `desktop_ui_click {name:"Open"}` |

**Chrome profile dirs lie to you.** `chrome:john@adom.inc` maps to dir **`Default`**, and the
personal account to `Profile 1`. Always compare DIRS, never assume "Default == personal".

### ONE login must cover Fusion AND APS

John, 2026-07-22: *"why should the user have to login twice to autodesk to sign in to fusion and
setup aps? why aren't those just driven from 1 login?"*

They cannot share a token (Autodesk DPAPI-encrypts Fusion's store with app entropy, on purpose),
but they CAN share the **warm browser SSO session**. So:

- `fusion_readiness` now reports APS state on EVERY launch and its `_hint` says what to do.
- Right after a successful Fusion sign-in, while the profile's Autodesk SSO session is still warm,
  the bridge starts APS consent itself - it goes through silently. Never make the user log in twice.

### Captions: only when you actually disrupt

Caption (`desktop_caption`, <= 3s) ONLY for steps that take over the screen - launching/restoring
Fusion. Do NOT caption background work; unnecessary captions are their own kind of rude. Always say
WHY, and whether you need anything from the user.

### Clean up after yourself: the sign-in window janitor

John, 2026-07-22: *"if you open a browser window for sign in, you must run a loop on a schedule to
know when to close it so you never leave the user's desktop messy."*

Any browser window WE open for OAuth is ours to close. Do not assume the flow closes it.

- `_track_signin_window(sessionId, profile, kind)` registers every window we open
  (`kind` = `fusion` | `aps`).
- `_signin_janitor_loop()` runs as a daemon thread started in `main()` and polls every 20s. It
  closes a tracked window as soon as its goal is met (`fusion_readiness.ready` for `fusion`,
  APS `signedIn` for `aps`) or it exceeds `_JANITOR_MAX_AGE` (15 min - an OAuth window older than
  that is dead weight, the request expires in ~2 min anyway).
- Closing uses `nbrowser_close_window`, which only closes OUR OWN sessions. **Never** close a
  window the user opened: `nbrowser_force_close` refuses other threads' windows by design, and
  the user's own tabs are off limits.

Same discipline applies to captions: `desktop_caption` auto-clears, but if you ever pass
`persist:true` YOU must hide it.

### Is there a CLI to sign Fusion in? (investigated 2026-07-22 - answer: effectively no)

John asked whether Fusion auth can be triggered from a command line instead of its GUI, which
would remove the whole click-the-Sign-In-button problem. Investigated on the real machine.

**What is actually running** (via `process_list {nameFilter}` - `commandLine` is populated there
even when `shell_execute`/`run_script` are dead, which is how this was found):

```
IdentityService.exe
IdentityUpdateService.exe
AdskIdentityManager.exe --process_name Autodesk.IDSDK.DefaultProcess-v2 \
                        --server_name  Autodesk.IDSDK.DefaultServer-v2
   (in <webdeploy>/production/<hash>/Autodesk Identity Manager/)
Fusion360.exe          (in <webdeploy>/production/<hash>/)
FusionLauncher.exe
```

**Conclusion: no.** `AdskIdentityManager.exe` DOES take flags, but they are **plumbing, not auth
commands**: they name the IPC channel (`--process_name` / `--server_name`) that the Identity SDK
broker listens on. It is a long-running local IPC server; Fusion is its CLIENT and asks it to do
the login over that channel. There is no `--login` / `--user` / `--token` surface, and the
resulting tokens go into a DPAPI-encrypted store (app entropy), so you cannot write one in either.

**What IS command-line reachable: the last mile.** The `autodesk://` protocol handler is what
returns the token to a waiting Fusion, and protocol handlers are invocable by URL. So the final
browser click ("Open Product") is in principle replaceable by firing the `autodesk://` callback
directly. NOT yet verified live - do not claim it works until someone tests it.

**So the practical answer stays: drive the GUI, but drive it in the BACKGROUND.** Fusion's Sign In
button is UIA-invokable, so no foreground is needed anyway (see the sign-in section above). That
already solves the annoyance a CLI would have solved.

**Bonus finding:** two `webdeploy/production/<hash>` dirs were live at once - `Fusion360.exe` from
one hash and `FusionLauncher.exe` from another. Per this repo's own notes Fusion NEVER prunes old
builds, so a dir count is NOT an update signal. Use `fusion_update_in_progress()`.

## Shell etiquette on the user's box (John, 2026-08-15)

Two hard rules for any shell/PowerShell you run via ab from this bridge's tooling:

1. **Always exit correctly.** PowerShell one-liners swallow exit codes; use `run_script`
   (base64 script) with an explicit `exit $LASTEXITCODE` appended, and propagate
   `exitCode` from the response instead of grepping stdout. `tools/adrun.py --ps` does
   this for you.
2. **Never blip a console into the foreground.** A flashing terminal window is the same
   rudeness as a foreground Fusion window. Prefer verbs that spawn NOTHING (read_file /
   write_file / pull_file / send_files cover almost every deploy step). If ab's shell
   spawn itself flashes a console on the box, that is an ab-core spawn-flags issue
   (CREATE_NO_WINDOW) - filed on adom/adom-bridge 2026-08-15; do not work around it
   with more shell.
