# CLAUDE.md - fusion-bridge (READ THIS, it is YOUR project)

This repo is **the Adom Bridge Fusion 360 bridge**. If you are an Adom AI touching anything
Fusion, **you own and maintain this code.** You write it, version it, publish it, and reinstall it.
Stop treating bridge problems as someone else's. (This file exists because an AI repeatedly forgot
all of this and filed its own bridge bugs as "adom-desktop" requests. Don't.)

## You vs adom-bridge (ab) core - the boundary

**Naming (John, 2026-08-29, tightened 2026-09-01):** the host app is **Adom Bridge, "ab"**. "Adom
Desktop" and "AD" are the retired names; do not use them in prose, hints, skills or issues. The
2026-08-29 exception that kept `_ab_call`, `ab_client.py` and the `adom-desktop-fusion-bridge` folder
is REVERSED: John (2026-09-01) - "make sure there's no more remnants of ad or adom-desktop naming."
The Rust port ships the purge (census + targets in the roadmap: the Python `_ad_call`/`ad_client` identifiers, renamed to `_ab_call`/`ab_client` on 2026-09-03;
70 `adom-desktop`), and a CI grep fails the build on any remnant. Until the rename lands, do not ADD
new uses of the old identifiers; write `ab` in everything new.

- **ab core owns:** the `adom-bridge` CLI, the relay / command passthrough, bridge lifecycle
  (`bridge_install` / streaming the bridge from the wiki into `…\bridges-cache\fusion360`), and the
  core `desktop_*` verbs. That's it. ab just **hosts and loads** this bridge.
- **YOU (this bridge) own:** every `fusion_*` verb and its response `_hint`s, the **APS** integration
  (auth/config/quota - see below), the **Fusion add-in** code AND **deploying it into Fusion**, the
  `SKILL.md` + bundled `skills/`, and the edit → publish → reinstall loop.
- So: a `fusion_*` verb behaves wrong, a hint is missing, APS misbehaves, the add-in is stale → it's
  **your code**, fix it here and republish. It is NOT an adom-bridge request.

## ⛔ NEVER LET FUSION DRAW A DIALOG (John, 3 reports; read the skill)

A modal freezes the add-in's single main thread, so the bridge stops answering and every
later verb reads as a crash - and it dings the user's speaker. **Prevention, never cleanup:**
prove the command unnecessary offline (`_resolve_artifact_name` against the `.lbr`), or
pre-answer it (`electron_run {confirm: "yes"|"no"}` -> `SET CONFIRM`, so the dialog is never
drawn). A dismissal sweep is a BACKSTOP whose every firing is a defect report. If you are
writing a hint telling the caller to dismiss a dialog afterwards, you have the bug.
Full rules + the command table: `skills/fusion-no-modal-dialogs/SKILL.md`.

## THE AUTOMATION DOCTRINE (John, 2026-08-18, verbatim - this outranks tidy error messages)

> "your whole goal is to automate the life of the adom user and never slow them down or stop
> them from getting their goal done. so stupid errors that you can easily fix are supposed to
> always just work automatically. never make the adom user do anything manually."

Concretely, for every verb, button, and surface in this bridge:

- **A click is a GOAL, not a request for an audit.** If Fusion is closed, launch it. If the
  add-in is not serving, restart Fusion (the add-in only loads at launch). If a modal blocks
  the main thread, clear it. If a library fell out of the session, re-stage it. THEN do the
  thing the user asked for.
- **Never return an error the code could have fixed.** "Press Setup and try again",
  "Fusion is not running", "open the library first" are all FAILURES OF THIS BRIDGE, not
  user instructions. Remediate, retry once, and only then report - with what you tried.
- **Narrate, do not block.** Say what you are fixing (toast/hint) while you fix it, so the wait
  is legible; never make the human perform the fix.
- **The only legitimate stops** are things code genuinely cannot do: a password, a 2FA code, an
  unconfigured APS client id, a hardware/licensing decision that risks the user's other machine.
  Toast those at the exact moment they are needed, having already done everything around them.

Implemented reference: `dashboard/src/main.rs` `ensure_ready()` + the remediate-and-retry pass in
`show_surface()`.

## Where to TALK to ab (learned the hard way, 2026-08-17)

**The adom/adom-bridge-sdk repo is DEPRECATED as a discussion venue** - the SDK moved INTO the
main `adom/adom-bridge` repo (its skills ship in that pkg: `adom-bridge-sdk*`), and the ab thread
does not watch the old repo's issues. Two of this bridge's posts sat unread there. File EVERY
SDK/cross-bridge ask on **adom/adom-bridge** issues; bridge-specific dialogue stays on this
page's own issues (ab watches those).

## Map of what you manage

- `server.py` - bridge server / verb router (talks to ab's relay, proxies to the add-in). **On start
  it calls `install_addin()`** to deploy the add-in (see deploy below).
- `aps.py` - the **entire APS** implementation: PKCE OAuth, token store, server-indexed cloud search,
  never-charge cap. **All APS is yours** (the `fusion_aps_*` verbs + `fusion_search_cloud_files` /
  `fusion_walk_cloud_tree`). NOTE: port **8910 is ab's vestigial "native" APS** - NOT yours; this
  bridge uses **8917/8918/8920** and steers around it. The bare `aps_status` (8910) is that ab stub,
  not this bridge's `fusion_aps_status`.
- `addin/AdomBridge/` - the add-in that runs **inside Fusion**: `AdomBridge.py`, `http_server.py`,
  `commands/*.py` (cloud_documents, open_electronics_file, electronics, export, modeling,
  manufacturing, screenshot, document_info, parameters, …).
- `describe.py` - the `fusion_describe` self-describe catalog (ab's Verbs tab).
- `handlers/` - close / recovery / dialog-classify / UI / open_design.
- `skills/` - bundled skills you ship: `fusion-aps-search`, `fusion-aps-signin`, `fusion-bridge-dev`,
  `fusion-onboarding`. `resources/` - JLCPCB CAM jobs, Adom DRU rules, the layer-detect ULP.
- `bridge.json` + `BRIDGE_VERSION` - the manifest/version (keep in lockstep).

## How you DEPLOY when you change your code (VERIFIED by watching v1.5.1 ship, 2026-06-28)

Fusion loads the add-in from a PER-USER add-in dir that Autodesk has MOVED across versions:
**2025+ Fusion scans `%APPDATA%\Roaming\Autodesk\FusionAddins\AdomBridge\`**; older builds used
`%APPDATA%\Roaming\Autodesk\Autodesk Fusion[ 360]\API\AddIns\AdomBridge\`. An add-in in the
wrong dir is SILENTLY ignored (Fusion runs fine, port 8774 never comes up - issue #63, root-caused
live). `install_addin.py` therefore installs to ALL of them (TARGET_CANDIDATES); NEVER assume a
single path, and if a future Fusion stops loading the add-in, FIRST suspect the dir moved again
(hunt for new dirs under %APPDATA%\Autodesk yourself). NOT from this repo or the cache.
⛔ NEVER make the user restart Fusion or enable the add-in BY HAND - YOU do it (fusion_stop +
fusion_start); runOnStartup does the rest. The exact, watched-it-happen release sequence (AFTER publishing to the wiki):
  ⚠️ But ASK FIRST when a human is there with documents open (the Adonis incident): "never make them do it" is not "restart it out from under them". A verb returning addin_restart_required hands you userMessage + openDocuments/unsavedDocuments to ask with; then fusion_restart_for_addin {save:true} does it and refuses rather than discarding a never-saved document.

1. **`bridge_install {"manifestUrl": ".../adom-bridge-fusion-manifest.json"}`** - streams the new zip
   into the CACHE via `method: in_place_merge`. Updates the cache ONLY; it does NOT touch Fusion's
   AddIns dir. (Pass `"force": true` to re-merge when the version already bumped.)
   Do NOT manually `bridge_kill` - `bridge_install`'s own output says "ab reaps + respawns it from the
   new cache on the next call (no manual kill needed)," so the bridge SERVER (new server.py/describe.py)
   reloads automatically on the next verb. (Running `bridge_kill` here is pointless.)
2. **`fusion_stop`** (graceful; `fusion_kill` if it is wedged) - REQUIRED. Fusion holds the add-in files
   OPEN, so the sync canNOT overwrite them while Fusion runs. (This is why a bridge-server respawn alone
   does NOT update the add-in, and why `fusion_stop`+`fusion_start` by itself deploys nothing.)
3. Deploy the add-in into Roaming. Two ways:
   - **(intended)** Run the cache's `install_addin.py`: `shell_execute` -> `cd <cache>\fusion360 &&
     python install_addin.py`. It prints `Updated: commands\cloud_documents.py ...`.
   - **⚠️ TRAP (hit 2026-06-29): `python`/`py` are NOT on the box's shell PATH.** ab spawns the bridge
     with its OWN python, so `python install_addin.py` via `shell_execute` returns "Python was not
     found" / errors with empty output (same Windows-Store-alias stub that makes a bridge fail to
     respawn). (ab >=1.9.63 PROVISIONS Python itself - a system install if one is on PATH, else a
     portable no-UAC python-build-standalone copy under `~/.adom/adom-runtimes/python-<ver>/`, pinned
     to 3.12.13 - and spawns `server.py` by ABSOLUTE PATH from it. So the bridge-SPAWN case is fully
     handled by ab; NEVER bootstrap/download your own Python. Check runtime state anytime with ab's
     `runtimes` verb (`state ∈ absent|installing|ready|failed`). But a bare `python` you invoke
     yourself via `shell_execute` still won't resolve, so the file-copy deploy below remains the way.)
     `cmd /c copy` and `robocopy` also fight you on the spaces in `Adom Bridge` / `Autodesk
     Fusion 360` (robocopy exit 16). **The reliable deploy that needs NO shell + NO python:** copy the
     changed add-in file(s) with the BRIDGE's own file verbs - `read_file {path:<cache>/addin/AdomBridge/
     commands/<f>.py}` then `write_file {path:<Roaming>/.../AddIns/AdomBridge/commands/<f>.py, content}`
     (Fusion CLOSED so the file is not locked). Verify with another `read_file` (grep a marker from your
     change). The Roaming AddIns path is `%APPDATA%/Roaming/Autodesk/Autodesk Fusion 360/API/AddIns/AdomBridge`.
4. **Verify by SHA256, never findstr.** `(Get-FileHash <Roaming>\commands\cloud_documents.py).Hash`
   must equal `sha256sum` of that same file unzipped from the published `v<ver>.zip`. findstr gives
   FALSE NEGATIVES on these files (UTF-8 box-drawing / dash chars break it) - it told me the reprimand
   was missing when the file was byte-identical to the release. Do not trust findstr; hash or
   `Get-Content -Raw` regex only.
5. **`fusion_start`** - Fusion now loads the new add-in. Confirm a `fusion_*` verb behaves new.

Memorize:
- `bridge_install` bumping `BRIDGE_VERSION` to the new version does NOT mean the running add-in changed.
  The version marker moves independently of the Roaming sync. Always hash-verify (step 5).
- The add-in sync only takes effect with **Fusion closed** (step 3). Skipping that is the #1 way a
  "successful" install leaves the OLD add-in running.
- **Never hand-edit the cache or the Roaming copy.** They get clobbered on the next stream/sync. Edit
  THIS source, publish, then run the sequence above.

## The publish/reinstall loop

See `skills/fusion-bridge-dev/SKILL.md` for the full ritual. Short form: edit here → bump
`BRIDGE_VERSION` + `bridge.json` together → `git commit` → **push GitHub backup** + **publish to the
wiki** (`wiki.adom.inc/adom/fusion-bridge`, the canonical repo) → `bridge_install` on the
box → reap the old bridge instance + restart so the new code (and synced add-in) loads → anon-verify.
Wiki = canonical, GitHub = backup, keep in lockstep.

## Fusion electronics file hierarchy (the other thing you kept getting wrong)

A Fusion electronics design is a **parent/child chain of separate files**, each a distinct
`productType`. **ALWAYS open the PROJECT; never the schematic/.brd/3D directly.**

```
PROJECT        EcadDesignProductType    <- open THIS
  |- schematic   SchematicProductType
  |- board/.brd  BoardProductType
  |- 3D PCB      DesignProductType       (generated FROM the .brd - the LEAF; no editable board)
```

Opening a child gives an isolated/empty view (you can't pull a board out of the 3D). Same-named
copies in OTHER projects are usually 3D-model derivatives (`DesignProductType`), not the project.
The open verbs (`handle_open_cloud_file` / `handle_open_by_urn`) reprimand via `_hint` when you open
a non-`EcadDesignProductType` file - read it.
