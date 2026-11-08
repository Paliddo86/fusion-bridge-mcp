---
name: fusion-web-export
description: >-
  Get a STEP/STL/OBJ/IGES (and then GLB) out of a Fusion cloud design WITHOUT the Fusion desktop app or add-in - the way to do it on Linux/Ubuntu, where desktop Fusion has no build. Two paths, same Autodesk cloud translation underneath: the clean API path (APS Model Derivative via the bridge's existing aps_* OAuth, headless, no browser) and the works-today browser path (drive Fusion Team in pup, grab the emailed link). Use when the user says "export a fusion step on linux/ubuntu", "get a step from fusion without opening fusion", "fusion team web export", "export fusion cloud design headless", "fusion step to glb no desktop".
---

# Fusion Web Export (no desktop app)

Get CAD out of a Fusion **cloud** design without opening (or installing) desktop Fusion. This matters most on **Linux/Ubuntu**, where desktop Fusion has no build, so the `fusion_export_*` verbs (which drive the desktop `exportManager`) cannot run.

There are **two paths, and they hit the SAME Autodesk cloud translation underneath** - the Fusion Team "Export" button and the API both queue an **APS Model Derivative** job that writes the file to OSS and hands back a signed download link.

- **API path (preferred, headless)** - call APS Model Derivative directly with the OAuth this bridge already holds. No browser, no desktop, runs in the container. **Needs one small verb that does not exist yet (see below).**
- **Browser path (works TODAY)** - drive Fusion Team in `pup` and retrieve the emailed link. Fully working now; use it until the API verb lands.

## Path A - APS Model Derivative (the clean way; verb to build)

**Status: the plumbing is here, the trigger is not.** `aps.py` in this bridge already does the hard part: 3-legged **PKCE OAuth** with scopes `data:read data:write data:create` (Model Derivative needs `data:read data:write` - already covered), plus Data Management to resolve a design's **URN** (`aps_search` / `aps_projects` / `aps_hubs`). What is missing is a verb (call it `aps_export`) that fires the translation job and downloads the result. The recipe:

1. Resolve the design **URN** (base64 of the `dm.lineage`/version id) via `aps_search` / the Data Management item, then base64url-encode it.
2. `POST https://developer.api.autodesk.com/modelderivative/v2/designdata/job` with the bearer token and body `{"input":{"urn":"<b64urn>"},"output":{"formats":[{"type":"step"}]}}` (STEP is an officially supported Model Derivative output - there is a `JobPayloadFormatSTEP`; also stl/obj/iges/fbx/dwg).
3. Poll `GET .../designdata/<b64urn>/manifest` until the `step` derivative is `success`.
4. `GET .../designdata/<b64urn>/manifest/<derivativeUrn>` (or the returned signed OSS url) to download the `.step`.
5. Hand off to `step2glb convert` for a GLB (below).

This reuses `aps.py`'s `_valid_access_token()` + `_http()` helpers; it is roughly a `_aps_post` + a poll loop. When this verb exists it SUPERSEDES the browser path (faster, headless, no login-in-a-profile, no email round-trip). Until then, use Path B. See [[fusion-aps-search]] and [[fusion-aps-signin]] for the auth/data layer this builds on.

**Caveat to test:** confirm Model Derivative actually emits STEP for the specific design type. It is reliable for 3D mechanical designs; electronics/schematic designs have no 3D body to translate (same limitation as Path B - see the routing note).

## Path B - Browser via pup (works today)

### The routing decision that wastes time if you get it wrong
- **3D mechanical designs** (a solid/assembly) export the full list incl. **STEP, IGES, SAT, SMT, STL, OBJ, DWG, DXF, FBX, SketchUp, Inventor, Fusion Archive**.
- **Electronics/PCB designs are SCHEMATICS on the web - they export SCH/BRD only, NO STEP.** The 12-format dropdowns are present in the DOM but hidden templates; do not be fooled. STEP comes from the board's **3D** design (often a separate "3D Assets" design, or the board pushed to 3D).

### Prereqs
`adom-bridge ping` connected + the pup (`browser_*`) bridge, a persistent pup profile logged into Autodesk (one-time), email access to the account that gets the export link (use **`adom-google`** - portable in any container; the claude.ai Gmail MCP connector also works if attached but is not guaranteed), and `step2glb` for the GLB.

### Steps
1. **Open Fusion Team** in a logged-in pup session at `https://<hub>.autodesk360.com` (NOT `drive.autodesk.com` - it errors; NOT `fusion.online.autodesk.com` - that is the AppStream streamed editor, pure canvas, un-automatable).
   `adom-bridge browser_open_window '{"sessionId":"fusion-web","profile":"autodesk","url":"https://<hub>.autodesk360.com"}'`
   Folder rows respond to a plain JS `.click()` via `browser_eval`; opening a design's detail page needs a TRUSTED click (`browser_input_dispatch`).
2. **Screenshot loop that works** (pup `browser_screenshot` errors os-13 on some hosts): `desktop_screenshot_window {hwnd}` (hwnd from `desktop_list_windows`, title "Google Chrome for Testing") -> read `safePathHost` from the JSON -> `pull_file {filePaths:[safePathHost], saveTo}` -> Read the PNG. Screenshot after every click.
3. **Open the 3D design**, then click the top-right download-arrow toggle by **VIEWPORT coords** from `getBoundingClientRect` (NOT screen coords; a `.trigger` selector click fails "not clickable"):
   `browser_eval` to get the visible `.download-btn.js-export-dropdown .trigger` center -> `browser_input_dispatch {type:click, x, y}`. Screenshot to confirm the menu opened (if it only shows "SCH" you are on a schematic).
4. **Click STEP**: get the visible `.export-format` STEP item's viewport center and trusted-click it. An EXPORT dialog appears ("...will send you an email with a link"). Click **OK**.

### Retrieve the emailed link (portable: adom-google)
Autodesk emails `noreply@autodesk.com`, subject **"Download file"**, to the signed-in user, with a signed direct link.
```bash
adom-google gmail search "from:autodesk.com newer_than:1h subject:(Download file)"
adom-google gmail get <MESSAGE_ID>
```
The body has a signed OSS link `https://cdn.us.oss.api.autodesk.com/oss/v2/signedresources/<uuid>?...filename*=...stp` (no auth, ~7-day expiry). `curl -sSL "<url>" -o /tmp/<Design>.stp` (sanity: starts with `ISO-10303-21;`).

## STEP -> GLB (both paths)
```bash
step2glb convert /tmp/<Design>.stp -o /tmp/<Design>.glb
```

## Gotchas
| Symptom | Fix |
|---|---|
| `drive.autodesk.com` "Something went wrong [1asss]" | Use the `*.autodesk360.com` Fusion Team hub. |
| Menu only shows SCH | Schematic/electronics design - STEP comes from a 3D design. |
| `browser_screenshot` -> `Permission denied (os error 13)` | Use `desktop_screenshot_window {hwnd}` + `pull_file`. |
| Coordinate click "ok" but nothing opens | Used SCREEN coords - use VIEWPORT coords from `getBoundingClientRect`. |
| No download in the browser | Correct - delivery is by EMAIL; retrieve via adom-google. |

## Related
- [[fusion-aps-search]] / [[fusion-aps-signin]] - the APS OAuth + Data Management layer Path A builds on (`aps.py`).
- `fusion-bridge` `fusion_export_step` - the DESKTOP path (needs Fusion open); use when Fusion is installed and running.
- `step2glb` - STEP -> GLB.
