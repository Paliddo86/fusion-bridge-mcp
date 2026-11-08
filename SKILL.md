---
name: fusion-bridge
public: true
description: "Launch and drive Fusion 360 on the user's laptop from this container via the adom-bridge CLI. Includes FAST cloud-file search via APS (Autodesk Platform Services) — search/browse/open the team's Fusion cloud designs in seconds (set up once per company, each user signs in once, never charged). Plus open .f3d/.f3z designs, open cloud files, close documents, dismiss recovery dialogs, capture Fusion window screenshots, send keys + click, run the add-in (STEP/GLB/.lbr import-export, BOM/parameters). Trigger words: fusion, Fusion 360, fast cloud search, fusion_aps_search, search fusion files, find a design, browse cloud files, fusion_aps_open, aps, open fusion, launch fusion, open design, f3d, f3z, import STEP, export STEP, export GLB, export lbr, Fusion electronics, Fusion PCB, BOM from Fusion, Fusion parameters, Fusion API, dismiss recovery, Fusion screenshot, fusion_click, fusion_send_key, fusion bridge, close fusion."
---

# adom-bridge — Fusion 360 bridge

> **The bridge is open, and the wiki page is the repo.** Hit a bug, a missing verb, a wrong hint
> or a dialog it cannot answer? File it with the reply JSON (`adom-wiki issue create
> adom/fusion-bridge`) or clone the source (`adom-wiki repo clone adom/fusion-bridge`), fix it,
> try it on your box behind a dev pin and send a PR (`adom-wiki pr create`). The
> **fusion-contributing** skill has every step. Every failing reply carries `_contribute`
> with the same pointer.

> **Cloud search not set up on a box?** Your org's APS app ID lives on the members-only `<org>/org-settings`
> wiki page (`adom-wiki settings get <org> autodesk-aps`). The **fusion-aps-org-setup** skill shows the two calls a new box needs, and the one-time
> admin setup for an org that has none. Never copy it from another machine's config file.

All commands dispatch to the Fusion Python bridge. From this container:

## Open or create a schematic (one verb)

A schematic is a VIEW of an electronics design, not a file. So the front door is one verb:

```bash
adom-bridge --target <box> --ai-thread <you> fusion_schematic '{"action":"open","fileName":"BQ25792 Assembly","projectName":"Main"}'   # cloud design -> schematic view
adom-bridge --target <box> --ai-thread <you> fusion_schematic '{"action":"open","urn":"urn:adsk.wipprod:dm.lineage:..."}'          # by lineage
adom-bridge --target <box> --ai-thread <you> fusion_schematic '{"action":"open","schPath":"C:/work/my.sch"}'                        # local EAGLE .sch (+ its .brd if present)
adom-bridge --target <box> --ai-thread <you> fusion_schematic '{"action":"create","name":"my-new-design"}'                          # blank schematic, new local design, no dialog
adom-bridge --target <box> --ai-thread <you> fusion_schematic '{"action":"show","capture":true}'                                    # switch the open design to its schematic
```

To put a library's parts on a NEW schematic and board (show off parts, a demo board, "make a board with these three sensors"):

```bash
adom-bridge --target <box> --ai-thread <you> fusion_make_showcase_project '{"libraryPath":"C:/Users/<user>/Downloads/Adom_ToF.lbr","devicesets":["VL53L5CX","VL53L7CX","VL53L8CX"]}'
```

It generates the EAGLE pair on the box with the parts placed (and their bound 3D bodies) and opens both documents dialog-free; then `fusion_show_3d_board {capture:true}` shows them. `fusion_schematic {action:"create"}` is the blank-sheet variant. Circuitry (nets, placement) is authored as EAGLE source and brought in with `fusion_new_electronics_from_eagle`.

Only `fusion_new_electronics_from_eagle` (a .sch plus its .brd, parts instantiated) drives Fusion's own Open dialogs, because Autodesk offers no API for it; it now refuses before touching a dialog when a file is missing and cancels the dialog itself if it does not take the file.

```bash
adom-bridge fusion_<action> '<json_args>'
```

The leading `fusion_` routes to the Fusion plugin; `<action>` is the plugin-side name.

## FIRST-TIME / COLD-START — read before you panic

A fresh laptop may not have Fusion running (or may run a stale add-in). That is EXPECTED, not defeat.
Branch on the stable `errorCode`, never the prose:

| response (`errorCode`) | what it means | what you do |
|---|---|---|
| `ready:true` (fusion_readiness) | Fusion up + add-in current | drive |
| `running:false` / `fusion_not_running` | installed, not launched | `fusion_start` (returns when the add-in is ready, or `stillLaunching` on a slow first launch) |
| `stillLaunching:true` (fusion_start) | first launch, add-in not up yet (can take 2-4 min) | do NOT re-call `fusion_start`; POLL `fusion_readiness` every ~10s until `ready:true` |
| `installed:false` / `host_app_not_installed` (ab >=1.9.79, + a `hostApp` field) | no Fusion host app | OFFER to install, then **`fusion_install_fusion`** (no shell approval; elevation-aware, streams the free trial) -> poll `fusion_readiness` (`installing:true`) until `installed:true` -> `fusion_start`. Details: [fusion-onboarding](skills/fusion-onboarding/SKILL.md) |
| `needsSignin:true` (`fusion_start` **and** `fusion_readiness`) | Fusion is running but stuck at the first-run Autodesk sign-in (main UI blocked, add-in can't serve, so `ready:false`) | AUTOMATE it (proven sequence, see [fusion-onboarding](skills/fusion-onboarding/SKILL.md)): (1) clear STALE `signin.autodesk.com` tabs (a dead tab = expired code = "Sign-in request expired"); (2) `desktop_ui_click` "Retry browser sign-in" / "Sign In" for ONE fresh request; (3) click "Open Product" with **`desktop_click` (SendInput=isTrusted)** — `desktop_ui_click` (UIA) does NOT fire the protocol; (4) on the in-page "open Autodesk Identity Manager?" overlay (NOT an owned popup) tick "Always allow" + "Open" via `desktop_click`, FAST; (5) poll `fusion_readiness`. `nbrowser` (CDP) can do 3-4 headlessly if it's on the SAME browser window. NEVER auto-enter password/2FA. To ask the user, `notify_user` works directly (ab ≥1.9.84); add a cross-ab `target` if you're on a VM |
| `addinStale:true` / `addin_stale` | Fusion runs an OLD add-in that never re-synced (silent-failure risk) | `fusion_stop` then `fusion_start` — the bridge re-syncs the add-in from cache on start (Fusion must be CLOSED) |
| `bridge_restarting` + `stillRunning:true` (ab ≥1.9.77) | the bridge is respawning after a `bridge_install` | NEVER a not-installed/fatal signal: poll `fusion_readiness` until `ready:true`, then retry. Read-only verbs (`fusion_readiness`/`fusion_describe`/`fusion_get_app_state`) forward straight through even mid-respawn — no sleep-and-recheck hacks needed |
| `blockingDialogs:[…]` | a modal is blocking the add-in | dismiss in the BACKGROUND, then the op auto-resumes |

**Runtime note:** ab **provisions Python itself** (a portable no-UAC 3.12.13 runtime if none is on PATH — the bridge never bootstraps its own; check ab's `runtimes` verb). You never install Python.

## The 8 conventions (from the Bridge SDK)

1. **Poll readiness, don't launch blindly.** `fusion_readiness` (or ab's `bridge_readiness`) is read-only — it reports `installed`/`running`/`ready`/`addinStale` WITHOUT launching anything. Loop: readiness → act on the row above → drive.
2. **`fusion_readiness` is the standardized cold-start probe.** (Fusion is a detected HOST APP with no heavy download, so there's no `prewarm` step — readiness is the whole gate.)
3. **Core verbs live in the table below; `fusion_describe '{}'` is the full machine-readable catalog.** Read the table to act fast; hit `describe` for completeness — don't guess a verb.
4. **Background by default — automate first, NOTIFY only at a true wall.** This bridge drives the AI's workspace, not the user's screen: dismiss dialogs in the BACKGROUND (`fusion_close_window`, never `fusion_send_key`/Escape, which steals focus). AUTOMATE sign-in / dialogs yourself (see the proven sign-in sequence in [fusion-onboarding](skills/fusion-onboarding/SKILL.md)); reserve an ab **`notify_user`** for something only the user can do — a **UAC/elevation** prompt, a license accept, a **2FA/password** step you must not auto-fill, or a licensing **DECISION** that risks their other machine. **`notify_user` works directly over the bridge's direct API (ab ≥1.9.84) — call it, don't relay a payload for the AI to send.** When this bridge runs UNATTENDED (e.g. on a VM) and the user is elsewhere, add a cross-ab **`target`** (a peer clientName from the `targets` verb, or `"all"`) so the toast lands where they are. Only foreground Fusion to SHOW the user something. See [fusion-driving](skills/fusion-driving/SKILL.md) and [fusion-onboarding](skills/fusion-onboarding/SKILL.md).
5. **`ok:true` is not enough — verify.** A long op that "times out" at ~60s is usually STILL RUNNING server-side (the relay caps the request, not the work). Verify by artifacts (the saved file / a screenshot / `fusion_get_app_state`), don't blindly retry.
6. **Reuse the running Fusion — and when a restart IS needed, YOU do it, never the user.** Fusion is a singleton: `fusion_readiness` before `fusion_start`; if `ready`, just drive. But a stale/missing add-in or a wedged session needs a restart, and that is YOUR job — `fusion_stop` then `fusion_start`. Restarting Fusion and enabling the add-in are the bridge's job, not something to hand back to the user — doing it for them is the entire point of this bridge. (Add-in installs land in ALL Fusion add-in dirs — 2025+ Fusion scans `%APPDATA%\Autodesk\FusionAddins`, older builds use `...\Autodesk Fusion[ 360]\API\AddIns` — so a restart always picks it up.)
7. **Surface `_hint` verbatim + branch on `errorCode`.** Every verb returns an actionable `_hint`; relay it to the user unchanged and key your logic off the stable `errorCode`, not the prose.
8. **Never write files to a shared cloud ROOT.** Uploads default to an `Adom AI Workspace` folder; see [fusion-cloud-hygiene](skills/fusion-cloud-hygiene/SKILL.md).

## [!] Electronics PCBs: ALWAYS open the PROJECT file (never the schematic / board / 3D)

A Fusion electronics design is NOT one file. It is a parent/child chain of FOUR files, each a
different `productType`:

```
PROJECT        EcadDesignProductType     <-- OPEN THIS ONE, ALWAYS
  |- schematic   SchematicProductType
  |- board/.brd  BoardProductType
  |- 3D PCB      DesignProductType        (generated from the .brd - the LEAF; opening it gives NO editable board)
```

- **Open the PROJECT** (`EcadDesignProductType`) with `fusion_open_cloud_file` / `fusion_open_by_urn`.
  Fusion then brings up its schematic + board as children; move between them with the schematic view,
  `fusion_show_2d_board`, and `fusion_show_3d_board`.
- **NEVER open the schematic, the .brd, or the 3D file directly** to "get the board." You get an
  isolated/empty view and waste time trying to derive a 2D board from the 3D. The 3D is the LEAF,
  generated FROM the .brd - it is not the board, and you cannot pull a board out of it.
- Same-named copies in OTHER projects (e.g. a drone assembly) are usually 3D-model derivatives
  (`DesignProductType`), NOT the project. Don't open those.
- `fusion_open_cloud_file` / `fusion_open_by_urn` REPRIMAND you in the response `_hint` when you open
  a child file instead of the project - read it and re-open the `EcadDesignProductType` project.

## ⚡ Fast cloud search (APS) — READ THIS FIRST for any "find/search/open a cloud design" task

To search the team's Fusion cloud files, use **`fusion_aps_search`**, NOT the legacy
`fusion_search_cloud_files` (which walks folders one-by-one and takes 30+ MINUTES). APS hits
Autodesk's **server-side index** → results in **seconds**, and works with Fusion closed.

| Verb | Does |
|---|---|
| `fusion_aps_search {query, projectId?, limit?}` | Fast name search across the whole team hub; newest-first; shows each file's project. |
| `fusion_aps_browse {} / {projectId} / {projectId,folderId}` | Browse hubs→projects→folders→files (cloud explorer, no Fusion). |
| `fusion_aps_open {query}` | Search + open the best match in Fusion (opens by exact URN — works for nested files). Returns immediately; the open finishes in the background — poll `fusion_get_app_state` (or pass `{wait:true}`). |
| `fusion_aps_recent {limit?}` | Most-recently-modified designs across the hub, newest-first. |
| `fusion_aps_file_info {query}` | Version history for a file — count, who/when last modified, size. |
| `fusion_aps_status {}` | configured? signedIn? token live? + never-charge usage meter. **Call this first** — its `_hint` tells you the next step. |
| `fusion_aps_signin {}` | One-time per user: opens Autodesk consent, captures the token. |
| `fusion_aps_set_client_id {clientId}` | Set the org's APS Client ID. |

**Auth model (important): set up ONCE per company, each user signs in once.** An admin registers a free APS app for the org one time (the AI drives the whole setup — see the `fusion-aps-signin` skill) → a **Client ID** (a public PKCE id, no secret, safe to share within the org). Then every teammate just runs `fusion_aps_signin` once with their own Autodesk login — **no plan, no card, no setup** for them. It's **per-organization** (each company runs its own setup; don't ship one global Client ID). **Nobody is ever charged** — the card is identity-verification only, and the bridge is free-endpoint-only + usage-capped.

If `fusion_aps_status` says not configured/signed in, follow its `_hint`. Full why + step-by-step setup: the bundled **`fusion-aps-search`** and **`fusion-aps-signin`** skills.

## ⛔ "Expired subscription — Read Only" / trial / personal Fusion — what it does NOT block

**An expired / "Read Only" / personal-use Fusion can OPEN, VIEW, and BROWSE files just fine.**
Autodesk keeps the account in **Basic Access for 365 days** after a subscription lapses. So in
read-only Fusion you CAN: **open cloud designs, view/inspect them, browse the Data Panel, and run
APS search** — everything this bridge does for *reading* works.

Read-only ONLY blocks **save, export, modify, and creating new designs** (the Save/Export buttons
are dimmed). So:

- ⛔ **Do not attribute an `open` / `view` / `search` failure to "the Fusion subscription
  is expired / read-only."** That is FALSE and confuses users (it's been said wrongly many times).
  If `fusion_aps_open` / `fusion_open_cloud_file` hangs or fails, it is a **code / approach
  problem in the open path** (URN resolution, the Electronics design picker, a slow assembly
  download, a stuck main thread) — debug/fix THAT, not the license. Try harder; don't blame the
  subscription.
- ✅ The ONLY place read-only is a legitimate explanation is **save / export** — e.g.
  `fusion_export_*` may be blocked (Save/Export dimmed). There, "renew the subscription" is valid.

(Source: Autodesk — "Term has Expired or Usage Limits Exceeded"; data stays in Basic Access ~365 days.)

## Commands (core bridge)

| CLI form | Action | Purpose | Key args |
|---|---|---|---|
| `fusion_start` / `fusion_launch` | `start` / `launch` | Launch Fusion 360 (handles webdeploy glob, waits for bridge, dismisses startup picker) | — |
| `fusion_open_design` | `open_design` | Open a local `.f3d` / `.f3z` design file | `path` |
| `fusion_open_cloud_file` | `open_cloud_file` | Open a design from the user's Fusion cloud | `project`, `path` |
| `fusion_close_document` | `close_document` | Close the active document | — |
| `fusion_close_window` | `close_window` | Close a specific Fusion window by HWND | `hwnd` |
| `fusion_close` | `close` | Close Fusion 360 entirely | — |
| `fusion_dismiss_recovery` | `dismiss_recovery` | Dismiss the "recover unsaved work" dialog | — |
| `fusion_relocate_recovery` | `relocate_recovery` | Move recovery dialog offscreen (for recording) | — |
| `fusion_state` | `state` | ONE read of everything on screen: dialogs (title, text, buttons), focus events, the window capture, and for a Design the frame Fusion itself renders (`nativeViewport`), a body count (`geometry`) and a blank-capture check (`windowPixels`). `{screenshot:false}` for the fast dialog check | - |
| `fusion_screenshot_fusion` | `screenshot_fusion` | Screenshot the main Fusion window | — |
| `fusion_check_dialogs` | `check_dialogs` | Every open dialog as data: title, text, buttons, hwnd (UI Automation, no screenshot) | — |
| (retired) `fusion_click_fusion` | | Answers `verb_retired`. Use ab `desktop_ui_click {hwnd, name}` or `desktop_screenshot_window` then `desktop_click {hwnd, space:"image", shotId, x, y}` | |
| (retired) `fusion_send_key` | | Answers `verb_retired`. Dialogs are read and expired by the bridge itself; a real key is ab `desktop_press_key {hwnd, key}` | |
| `fusion_addin_status` | `addin_status` | Report whether the in-process Adom add-in is loaded + healthy | — |
| `fusion_generate_package` | `generate_package` | Generate an IPC package via Fusion's built-in EPG (~50 families) + raised-WHITE marking (longest-axis, measured 10% margin, multi-line MPN+variant) + colored STEP + sidecar manifest - in ONE call | `type`, `params` (mm), `etch`, `etchStyle`, `outputStep` |
| `fusion_set_preference` | `set_preference` | Set appearance/navigation prefs LIVE (no restart): `theme` (light/darkblue/darkgray/classic/device/dark), `invertScrollZoom`, `orbitScheme` (fusion360/alias/inventor/solidworks/tinkercad/powermill), `modelingOrientation` (yup/zup), `gestureNav`, `cameraPivot`, `lengthUnit` (mm/cm/m/in/ft). Per-key result in `applied`; some themes aren't in every build (falls back) | any of the keys |
| `fusion_get_preferences` | `get_preferences` | Read current appearance/navigation prefs as the friendly values `set_preference` accepts | — |

## Extended commands via the in-process add-in

If the Adom add-in is installed (`cd ~/project/adom-desktop/plugins/fusion360 && python install_addin.py` on the laptop), Fusion 360 exposes an additional API over its own ports. Typical extra commands proxied through the bridge:

- `fusion_import_step` — import a STEP file into the active document
- `fusion_export_step` — export active body/occurrence to STEP
- `fusion_export_optimized_glb` — export to GLB (used by the 3D viewer)
- `fusion_export_lbr` — export an Eagle library (used by the Eagle workflow)
- `fusion_open_lbr` / `fusion_show_symbol` — open symbol libraries
- `fusion_command` — generic `{"command": "<fusion_api_name>", "args": {...}}` pass-through for any Fusion API call
- `fusion_list_cloud_projects`, `fusion_list_cloud_files`, `fusion_walk_cloud_tree`, `fusion_search_cloud_files`, `fusion_open_cloud_file`, `fusion_export_cloud_file` — Autodesk cloud Data API operations

**Opening multi-design electronics files — pass `fileExtension`.** A Fusion electronics data file that links to multiple designs (common for boards with variants) makes `fusion_open_cloud_file` pop Fusion's native **"Select Electronics Design File" picker** — a CEF/Chromium modal whose list is **not keyboard- or Win32-navigable** (and screen-coord clicks are unreliable on multi-monitor/HiDPI), so it **can't be resolved headlessly**. Avoid it by opening the specific file directly: `fusion_open_cloud_file {"fileName":"<name>","fileExtension":"fbrd", ...}` (the `.fbrd` board lands straight in PCB Editor — live-confirmed). Opening the bare name or the `.fprj` project triggers the picker. If a picker still appears, `_diagnose_addin_timeout`/`blockingDialogs` will classify it (see below).

### Blocking-dialog awareness

Several Fusion modals (the multi-link "Select Electronics Design File" picker, "Fusion needs to update", Document Recovery, "Save changes?") block the add-in's main thread. The bridge now **classifies them by title** and, on a blocked command, returns `blockingDialogs: [{hwnd, title, category, resolution}]` with the precise cause + fix — instead of a misleading "add-in not responding / may have crashed". So a blocked open tells you *which* modal is up and how to clear it.

Resolution notes:
- **Multi-link picker** is a **CEF/web modal whose list is NOT keyboard- or Win32-navigable** (SendInput reaches only the native Cancel/OK; screen-coord clicks are unreliable on multi-monitor/HiDPI). It currently **must be resolved on the desktop**. *Headless fix tracked separately:* have the add-in open the specific linked design via the Fusion API, avoiding the picker entirely.
- **Update nag** is reported, not auto-dismissed (auto-confirming could start an update mid-run) — clear it on the desktop; doing so also commonly resolves recurring add-in crashes (host/add-in version drift).
- **Recovery** prompts: `fusion_dismiss_recovery` / `fusion_relocate_recovery`.

Check `fusion_addin_status` first — if the add-in isn't loaded, these will fail with a `_hint` pointing at the install script.

## Cloud search — defensive contract (add-in v1.0.2+)

`fusion_search_cloud_files` is the most-used long-running verb. Its v1.0.2 behavior is engineered specifically so the AI never misreports a failed search as "file not found":

| You read in the response | Mean |
|---|---|
| `searchComplete: true` + `totalFound: 0` | **Confirmed negative.** No match in the searched scope. |
| `searchComplete: true` + `totalFound: N` | All matches returned. |
| `searchComplete: false` (any reason) | **Search stopped early.** May or may not be more matches. Do NOT say "not found" to the user. Re-run with broader caps OR narrower `folderPath`. |

`searchComplete` is `true` ONLY when every one of these is clean: not `folderLimitReached`, not result-`truncated`, not `searchTimedOut`, `foldersSkipped == 0`, `filesSkipped == 0`. Five conditions, all five must pass.

### Performance + the slow-search etiquette

`fusion_search_cloud_files` runs at ~1 folder/sec because Autodesk's free Fusion 360 Python API has no indexed search endpoint — it walks the cloud Data API one folder at a time (30+ min on a real hub). **⚠️ Prefer `fusion_aps_search` instead** (see the APS section at the top) — it uses Autodesk Platform Services' server-side index and returns in **seconds**. This legacy walk verb remains only as a fallback for when APS isn't set up; if a user is hitting slow searches, set up APS (`fusion_aps_status` → its `_hint`) rather than tuning the walk.

**Before firing a broad search**, ask the user which subfolder is likely. Naming a folder like "Molecules" cuts 5-40 min searches down to ~10s. The verb's response includes a `costAnalysis` block (`foldersPerSecond`, `estimatedSecondsPer100Folders`) so the AI can budget realistically.

### Streaming progress

For interactive sessions, wrap long searches in `watch` so the AI gets one JSON-line progress event per `interval` seconds:

```bash
adom-bridge watch '{
  "command": "fusion_search_cloud_files",
  "args": {
    "query": "cosmiic",
    "projectName": "Main",
    "folderPath": "Molecules",
    "recursive": true,
    "maxFolders": 300,
    "maxDepth": 10,
    "searchTimeout": 340,
    "timeout": 400
  },
  "interval": 3
}'
```

Output:
```
{"event":"started", ...}
{"event":"progress", "foldersVisited": 12, "filesFound": 1, "elapsedSeconds": 15.1, "currentFolder": "Molecules/COSMIIC", ...}
{"event":"progress", "foldersVisited": 173, "filesFound": 3, "elapsedSeconds": 168.3, ...}
{"event":"complete", "result": {... full response with searchComplete:true ...}}
```

The `watch` wrapper composes with the `Monitor` tool — each stdout line becomes a notification.

### Args (no hard upper caps in v1.0.2+)

| Arg | Default | Notes |
|---|---|---|
| `query` | (required) | Substring of file name. **Case-insensitive both directions.** |
| `projectName` | active | Cloud project name. |
| `folderPath` | project root | Starting subfolder. **NARROW HERE — biggest perf lever.** |
| `recursive` | `false` | Descend into subfolders. |
| `maxDepth` | 2 | Recursion depth cap. |
| `maxFolders` | 10 | Folder-visit cap. |
| `maxResults` | 20 | Result cap. |
| `folderTimeout` | 30s | Per-folder cloud-API timeout. Bridge skips slow folders. |
| `searchTimeout` | 120s | Overall wall-clock timeout. Set 300+ for big walks. |
| `timeout` | 620s | HTTP envelope timeout (CLI+direct API). Bump if `searchTimeout` > 180. |

Real-world live test: `Main/Molecules` recursive depth-10 cosmiic search hit 173 folders in 168s with `searchComplete:true, totalFound:3, foldersSkipped:0, filesSkipped:0`. Fusion stayed `main_thread: responsive` throughout.

## Quick examples

```bash
# Launch Fusion
adom-bridge fusion_start '{}'

# Open a local design
adom-bridge fusion_open_design '{"path":"C:/designs/board.f3d"}'

# Everything on screen in one read: dialogs as text, plus the window capture
adom-bridge fusion_state '{}'

# Run a Fusion API command (add-in required)
adom-bridge fusion_command '{
  "command":"export_step",
  "args":{"occurrence":"body1","output":"C:/tmp/part.step"}
}'

# Check add-in health
adom-bridge fusion_addin_status '{}'
```

## Error shape

Every failing response includes a `_hint` field. Surface it verbatim when a Fusion command fails.

## Bridge architecture (v1.8.31+) — dynamic ports

Earlier versions used hardcoded port `8773` for the bridge and `8774` for the in-process add-in. **v1.8.31 made the bridge use OS-assigned ephemeral ports.** The add-in port is still 8774 (it's inside Fusion's process and the manifest pins it), but that's an internal detail.

- The CLI (`adom-bridge fusion_*`) routes through adom-desktop's direct API → the bridge's runtime port. No callers need port numbers.
- The add-in v1.0.2 ships a stale-lock watchdog in `http_server.py`: if a command holds the main-thread lock longer than `per_command_timeout + 30s` (e.g. Fusion's native cloud SDK wedges on a slow API call), the watchdog force-releases so subsequent commands work without restarting Fusion. The wedged command is orphaned but no longer blocks everything else.
- The `searchComplete` contract (see above) prevents false-negative reporting on capped/skipped searches. Always check `data.searchComplete` before telling the user "file not found".

## How this bridge is packaged + delivered (3 artifacts)

This bridge is ONE wiki page (`wiki.adom.inc/adom/fusion-bridge`) that ships **three things**:

1. **The page git repo** — the ONE source of truth (all source, `skills/`, the docs, `page.json`, and the
   bridge manifest). Edit + push here.
2. **The Release zip + the bridge manifest** (`adom-bridge-fusion-manifest.json`) — what **adom-desktop**
   consumes on `bridge_install`: the manifest tells adom-bridge who this bridge is (name, version, the
   Release-zip URL, sha256, size, verb prefixes, health), and it streams that zip into its bridge cache.
3. **The adom-wiki skills pkg** (`adom-wiki pkg install adom/fusion-bridge`) — **skills only**.
   It deploys these skills into a container's `~/.claude/skills/` so the AI knows how to drive this bridge
   and Fusion.

**Install handshake:** when an AI has adom-bridge install this bridge, adom-bridge installs the Release
AND tells the AI to `adom-wiki pkg install` this pkg — so the container gets all these skills. An AI can
also install the pkg itself; then the skills **auto-update** via the pkg mechanism (the goal). Full detail
+ the publish ritual + the hero-relink reminder: the **`fusion-bridge-dev`** skill.

## Related

- **Building a whole board from a netlist (library, live schematic, fab rules, aiflow placement and routing, fields refinement, Gerbers and BOM):** **`fusion-board-from-netlist`**. Proven on the 12 V to 5 V buck molecule.
- **Building component libraries (symbol + footprint + RENDERING 3D):** **`fusion-libraries`** (one part) and **`fusion-multipart-libraries`** (many parts in one `.lbr`, each with real 3D) — the latter's SOLVED recipe is `fusion_make_3d_package` (STEP → orient → f3d → upload → `wip_urn`, hand-written into the library, **no GUI dialogs**). Full tutorial: the **`fusion-multipart-libraries`** skill's `CREATING_BASIC_PARTS_LIBRARIES.md`. Operate Fusion safely with **`fusion-driving`** + **`fusion-cloud-save`**.
- **`fusion-aps-search`** + **`fusion-aps-signin`** — APS fast-search reasoning + the AI-driven one-time setup (bundled in this repo's `skills/`).
- `adom-desktop-kicad` — sibling bridge for KiCad.
- `adom-desktop-direct-api` — the direct API contract that callers route through.
- **Canonical repo:** the wiki page `https://wiki.adom.inc/adom/fusion-bridge` (+ GitHub backup `adom-inc/fusion-bridge`). The old `adom-inc/adom-desktop/plugins/fusion360/` is a stale seed.

## Quick demo on first install (sample library included)

The repo ships a ready sample so you can prove the bridge end to end right after installing:
`examples/ADS8588SIPM.fusion.lbr` (a real 4-sided LQFP-64 library) + `examples/ADS8588SIPM.step` (its 3D).

1. Download both from this page's Files tab (or `adom-wiki repo clone`), then stage them onto Windows
   (serve + `Invoke-WebRequest`, since there is no container->Windows push verb).
2. **Import demo:** `fusion_open_lbr {"filePath":"C:/.../ADS8588SIPM.fusion.lbr","verify":true}` -> the
   symbol + 4-sided footprint render in the Electronics Library editor.
3. **3D-attach demo:** `fusion_attach_3d_package {"filePath":"C:/.../ADS8588SIPM.fusion.lbr",
   "modelPath":"C:/.../ADS8588SIPM.step","packageName":"ADS8588SIPM"}` -> click the Save dialog per the
   returned hint -> the real chip binds onto the footprint (Content Manager shows the 3D under the package).

See the **`fusion-libraries`** skill's `MAKING_LIBRARIES.md` for the full walkthrough + screenshots.
