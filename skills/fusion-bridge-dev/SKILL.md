---
name: fusion-bridge-dev
description: How to develop, version, and PUBLISH the Adom Desktop Fusion 360 bridge — where the source canonically lives, and the rule to ALWAYS publish to the wiki AND push the GitHub backup on every change. Read before editing/shipping the fusion bridge. Trigger words — fusion bridge dev, publish fusion bridge, fusion bridge repo, where does the fusion bridge live, ship fusion bridge, bump fusion bridge, adom-desktop-fusion-bridge.
---

# Fusion bridge — dev + publish workflow

## ★ THE CORE GOAL: the AI runs EVERYTHING so the user never has to (why this bridge exists)

The entire reason adom-desktop and its bridges exist is so that **the AI drives the whole flow
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

## You OWN this bridge (do NOT file your own bugs as adom-desktop requests)
AD core only HOSTS/LOADS this bridge: the `adom-desktop` CLI, the relay/passthrough, bridge lifecycle
+ streaming this bridge into `…\bridges-cache\fusion360`, and the core `desktop_*` verbs. **Everything
`fusion_*` is yours** - the verbs + their `_hint`s, the APS integration (auth/config/quota; port 8910
is AD's vestigial native APS, this bridge uses 8917), the add-in code AND its deployment into Fusion,
the SKILL.md, the publish loop. A `fusion_*` bug is a bug in THIS repo: fix it here and republish,
never as an adom-desktop discussion ask. (See the repo `CLAUDE.md`.)

## Your wiki structure: ONE page, THREE artifacts (know this cold)

You are a single wiki page (`wiki.adom.inc/adom/adom-desktop-fusion-bridge`) that ships **three distinct
things**, each with a different consumer:

1. **The page git repo — the ONE source of truth.** Everything lives here: `server.py`, `aps.py`,
   `addin/`, `handlers/`, `skills/`, the `*.md` docs, **`page.json`** (the skills-pkg manifest), and
   **`adom-bridge-fusion-manifest.json`** (the bridge manifest). Edit here, push here (+ GitHub backup).

2. **The Release (the zip) + the bridge manifest — consumed by ADOM-DESKTOP on `bridge_install`.**
   - The **Release zip** (`adom-bridge-fusion-v<ver>.zip`, uploaded via `adom-wiki release upload`) is the
     BUILT bridge (server + add-in). Binaries/zips are **Releases, NEVER git** (`*.zip` is gitignored and
     `repo push` silently skips it — see PUBLISHING.md).
   - The **bridge manifest** (`adom-bridge-fusion-manifest.json`, text, in git) tells adom-desktop **who you
     are**: name, version, the `url` pointing at the Release zip, `sha256`, `size`, verb prefixes, health
     endpoint. adom-desktop streams that zip by URL into its bridge cache and hash-verifies it.

3. **The adom-wiki pkg (`adom-wiki pkg install adom/adom-desktop-fusion-bridge`) — NOTHING BUT SKILLS.**
   It gives a user's container the skills to know how to talk to YOU and to Fusion. `page.json` is its
   manifest; `install.sh` copies the top-level `SKILL.md` + every `skills/*/` into `~/.claude/skills/`.
   **No binaries belong in this pkg** — that's what the Release is for.

### The install handshake (how a container gets your skills)
- An AI asks adom-desktop to install you → adom-desktop installs the **Release** (via the manifest) AND
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
**`https://wiki.adom.inc/adom/adom-desktop-fusion-bridge`** (owner `adom`, public). A wiki
page IS a git repo; this is THE main repo for the bridge. AD installs the bridge from here
(its manifest + zip). Every version ships here.

- Local working clone: `~/project/adom-desktop-fusion-bridge`.
- Files on the page: `bridge.json`, `BRIDGE_VERSION`, `README.md`, `SKILL.md`, `aps.py`,
  `server.py`, handlers/addin, the `adom-bridge-fusion-manifest.json` + version zips
  (`adom-bridge-fusion-v<ver>.zip`), `adom-bridge-fusion-source.tar.gz`, and
  `adom-bridge-fusion.bundle` (portable full git history).

## GitHub backup — ALWAYS push it too
`github.com/adom-inc/adom-desktop-fusion-bridge` (renamed from the old `adom-bridge-fusion`
to match the wiki slug). It is a **backup mirror**. **On EVERY change you publish to the
wiki, also `git push` to this GitHub repo.** Never let them drift again (they were 6 commits
out of sync once — that's the confusion we're preventing). Wiki = canonical, GitHub = backup,
keep them in lockstep.

## The publish ritual (every version)
1. Edit in `~/project/adom-desktop-fusion-bridge`. Bump `BRIDGE_VERSION` + `bridge.json`
   `version` together.
2. `git commit` locally.
3. **Push to GitHub backup:** `git push origin main`.
4. Publish to the wiki via the raw `/files` API (the wiki CLIs are dead) — token
   `${ADOM_WIKI_TOKEN:-$(cat /var/run/adom/api-key)}`:
   `curl -X POST -H "Authorization: Bearer $TOK" -F "file=@<f>" \
    https://wiki.adom.inc/api/v1/pages/adom-desktop-fusion-bridge/files`
   Upload: the new `adom-bridge-fusion-v<ver>.zip`, the `adom-bridge-fusion-manifest.json`
   (`{manifest_version,name,version,url,sha256,size,released_at}`, url = absolute `/files/`
   path), `bridge.json`, `BRIDGE_VERSION`, a fresh `adom-bridge-fusion-source.tar.gz`
   (`git archive`) and `adom-bridge-fusion.bundle` (`git bundle create … --all`).
5. **ANON-VERIFY**: re-download the zip + sha256-match, fetch the manifest, confirm version.
6. Install on a Fusion box: `adom-desktop --target <laptop> bridge_install
   '{"manifestUrl":".../adom-bridge-fusion-manifest.json"}'`, then reap the old instance
   (`bridge_kill` on AD ≥1.8.187, else taskkill the PID on its runtimePort) so the new code
   loads, and verify via `bridge_log_read` / a `fusion_*` verb.

## The stale seed — do NOT publish from it
`adom-desktop` (private GitHub) → `plugins/fusion360/` was the bridge's ORIGINAL home and is
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

The skills pkg (`adom-wiki pkg install adom/adom-desktop-fusion-bridge`) is SEPARATE from the Release.
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
   "hero": {"type":"billboard","headline":"Adom Desktop - Fusion 360 Bridge",
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
   adom-wiki repo push adom/adom-desktop-fusion-bridge --files page.json -m "restore full-bleed hero"
   ```
   The pkg stays published (registry snapshot is immutable); only the page display is restored.
5. **VERIFY**: `adom-wiki pkg install adom/adom-desktop-fusion-bridge` deploys skills to
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
> bridge runtime itself is loaded by Adom Desktop from the release zip."

It matters beyond tidiness: users and AIs assume `pkg install` installs the BRIDGE. It does not - it
installs SKILLS into the container. The runtime is the release zip that AD streams via
`bridge_install`. Say that in the Install card, once, plainly.

Filed as SDK-wide asks on adom/adom-desktop: **#20** (every bridge page must state the pkg-vs-runtime
split) and **#21** (document + lint keeping the release zip and pkg tarball free of media bloat).
