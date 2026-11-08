---
name: fusion-publishing
description: DEVELOPER-only. USE WHEN publishing the fusion-bridge package, cutting a bridge release, or adding/updating a user skill - the full adom-wiki ritual and the two traps that silently strand work (skills not globbed in page.json never install for users; adompkg is DEAD). Trigger words - publish fusion bridge, cut fusion release, ship a skill, skill not installing, adompkg, adom-wiki pkg publish, release ritual, page.json files.
user-invocable: false
---

# Publishing the fusion-bridge: the adom-wiki ritual (and the two silent traps)

## ⛔ Trap 1: adompkg is DEAD. Everything is adom-wiki now.

`adompkg` and every wiki-v1/v2 slug host are deprecated and gone (John, 2026-08-23: "there is
no such thing as adompkg anymore, its all done via adom-wiki"). The old CLI may still appear
to work as a shim - do not use it. The verbs:

| Task | Command |
|---|---|
| Publish the package | `adom-wiki pkg publish` (from the repo dir) |
| Bump the manifest version | `adom-wiki pkg version` (or edit page.json+package.json in lockstep) |
| Pre-publish lint | `adom-wiki pkg lint` |
| Upload a bridge release zip | `adom-wiki release upload adom/fusion-bridge <ver> <zip> --platform windows --arch x64` |
| Push source files | `adom-wiki repo push adom/fusion-bridge --files <...> -m "<msg>"` |
| User installs | `adom-wiki pkg install adom/fusion-bridge` |

## ⛔ Trap 2: a skill in skills/ does NOT ship unless it is GLOBBED in page.json

`page.json`'s `files` array is the package content list. A user-tier skill ships ONLY if
`skills/<name>/**` appears there. This bit us live (2026-08-23): FIVE skills sat on disk and
in the wiki repo - including `fusion-autodesk-signin`, which SHIPPED skills told the agent to
load - and no user ever received them, because nobody added the globs. Writing the skill and
pushing the repo is NOT publishing it.

**The check, run it before every publish:**
```python
import json, os
fl = json.load(open('page.json'))['files']
shipped = {g.split('/')[1] for g in fl if g.startswith('skills/')}
missing = [d for d in os.listdir('skills') if d not in shipped]
print('ON DISK but NOT shipped:', missing or 'none')   # must be 'none' (or deliberate)
```

## The full release ritual (both channels, in order)

The bridge has TWO delivery channels that move together:
- **runtime zip** (server.py/add-in/handlers, listed in `tools/release-files.txt`) -> delivered
  by `bridge_install` on each box. Skills are NOT in this zip.
- **package** (SKILL.md, skills/ tier, docs, installers, per page.json `files`) -> delivered by
  `adom-wiki pkg install` / user updates. Skills ship ONLY here.

1. Edit source. THREE gates, all required - and the third is the one that catches what the
   first two cannot:
   - `python3 -c "import ast; ast.parse(open('server.py').read())"` (syntax)
   - `python3 tools/shadow_check.py server.py` (0 findings)
   - `python3 -c "import server"` **(IMPORT SMOKE TEST - MANDATORY.)** Syntax + AST pass a
     forward reference; import does not. The 1.9.145 OUTAGE (2026-08-23): a handler was
     registered in the COMMAND_HANDLERS dict literal ~1600 lines BEFORE its `def`, syntax and
     shadow-check were green, and the deployed server NameError'd on import and crash-looped -
     every fusion verb returned "Transient" until the 1.9.146 hotfix. If a handler must be
     defined after the dict, register it post-definition
     (`COMMAND_HANDLERS["x"] = _handle_x` after the def, like the mcp_* handlers).
2. Bump `BRIDGE_VERSION` + `bridge.json` (bridge ver) AND `page.json` + `package.json`
   (pkg ver) - they track separately but always move together in a release.
3. Build the zip from `tools/release-files.txt` (python zipfile; 59 files, MISSING must be none).
4. `adom-wiki release upload adom/fusion-bridge <bridgever> <zip> --platform windows --arch x64`
5. Update `adom-bridge-fusion-manifest.json`: version, url, sha256, size.
6. `git commit` (wiki is canonical, git is history).
7. `adom-wiki repo push adom/fusion-bridge --files <changed files> -m "..."`
8. Run the Trap-2 skill-glob check above.
9. `adom-wiki pkg publish`
10. `bridge_install {"manifestUrl":"https://wiki.adom.inc/api/v1/pages/fusion-bridge/files/adom-bridge-fusion-manifest.json","force":true}`
    - server.py changes respawn on the next verb (NO Fusion restart); add-in changes ALSO need
      the Roaming deploy with Fusion closed (see CLAUDE.md deploy section).
11. Verify the changed behavior live (call the verb, check the new field/hint).

## Skill tiers (wiki three-tier layout)

- `skills/` = USER tier: ships + installs for every user, IF globbed (Trap 2).
- `dev-skills/` = DEV tier: source-only, maintainers read it in the repo, never installs.
- `publish-skills/` = PUBLISH tier: source-only, publishing workflows.

When you write or materially update a USER skill: add the glob (if new), bump pkg version,
`adom-wiki pkg publish` - in the SAME sitting. A skill that is written but not published is
invisible to every Adom user, which defeats the entire point (John's standing rule: shared
learnings go to the wiki so every user's agent gets them, not into private memory).

## ⛔ Trap 3: a DASHBOARD-only ship still needs `pkg publish` (repo push is NOT enough)

The dashboard binary version is wired into `install.sh` as `DASH_VERSION`, and `install.sh`
lives in the PACKAGE PAYLOAD. `adom-wiki repo push` updates the page's git repo (the Files
tab) but does NOT touch the published install payload, and `adom-wiki pkg install` reads the
PAYLOAD. So a dashboard ship that only does `release upload` + `repo push` leaves every
`pkg install` resolving the OLD `DASH_VERSION` -> the old dashboard binary. project-1d caught
this live twice (2026-08-23: install kept giving 1.9.153/1.9.157 after 1.9.157/1.9.158 were
"shipped"). After ANY dashboard binary ship you MUST also bump the pkg version
(page.json + package.json) and `adom-wiki pkg publish --org adom`.

**Dashboard ship checklist:** edit main.rs -> bump dashboard `Cargo.toml` version -> `cargo
build --release` -> `adom-wiki release upload adom/fusion-bridge <ver> <binary> --platform
linux --arch x64` -> bump `DASH_VERSION` in install.sh -> **bump page.json + package.json pkg
version** -> commit -> `adom-wiki repo push` -> **`adom-wiki pkg publish --org adom`** ->
VERIFY with a real clean install: `cd /tmp/x && adom-wiki pkg install adom/fusion-bridge`
then `grep DASH_VERSION adom_modules/adom/fusion-bridge/install.sh` must show the new version.
Never trust the `files/install.sh` HTTP endpoint for this - it serves the git repo, not the
payload.
