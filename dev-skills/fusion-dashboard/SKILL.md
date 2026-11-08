---
name: fusion-dashboard
description: THE Dashboard - the fusion bridge's web surface (Launch Page) that opens from the Adom Dock Bar / Web Control / Hydrogen webview / pup, showing LED status for every bridge facet, a first-run Setup flow, a Settings panel, copy-paste AI prompts, demo launchers, and wiki part/molecule import demos. Served by a RUST BINARY CLI installed into the Linux container via the wiki pkg tarball - the FIRST bridge to ship a container-side binary (the new three-artifact pattern the Bridge SDK adopts and kicad mimics). Read BEFORE touching dashboard/ or the pkg layout.
---

# fusion-dashboard - the bridge's Launch Page

## The gold (John, 2026-08-16, the founding spec - do not lose ANY of this)

> "we need you to create a new surface which i'll call your Dashboard ... something that
> should and can open in a browser like the built-in webview in hydrogen or a pup window
> or some other browser surface and its sort of like your Launch Page that gives the adom
> user a summary of what's going on with you."

- **Trigger**: Adom is implementing a **Dock Bar** (documented on the wiki). Clicking the
  dock bar entry, or triggering from Web Control, opens this Dashboard.
- **Status LEDs** - a full high-level summary of all Fusion features:
  - Fusion running or not
  - APS configured AND working or not (cloud search capability, explicitly)
  - MCP server into Fusion configured / running or not
  - Other APIs (Data Hub) configured or not
  - Pending Fusion update sitting waiting for install or not
  - (Pattern: every facet the bridge knows about gets an LED; readiness is the source)
- **Setup button** for first-time users - walks through EVERYTHING configurable in the
  bridge (sign-in, APS, MCP, auto-update, demo samples).
- **Settings panel** - "most of our apps have a Settings panel" - toggles for key items
  with today's defaults (auto-update ON/sticky-opt-out, etc.).
- **Sample AI prompts** - copy/paste prompts so the user LEARNS how to drive the bridge.
- **Demo tour** - the web-control tour equally exists here (start + progress bar +
  gallery via fusion_demo {mode:"tour"} / fusion_demo_progress).
- **Sample components and boards** to show off the demos.
- **Wiki import demos**: (a) import an adom basic part from the wiki and open in Fusion;
  (b) open a MOLECULE from the wiki into Fusion.
- **AI hints galore in the code** - every API response teaches the calling AI.
- Built with the house **ui-design + app-creator** skill patterns (dark default,
  AI-responsive, responsive layout).

## The NEW three-artifact pattern (this bridge blazes the trail; kicad mimics)

A bridge's wiki page repo now ships THREE installable artifacts:

| Artifact | Installs into | Contents |
|---|---|---|
| Release zip + manifest | ab on Windows (`bridge_install`) | server.py, add-in, resources |
| pkg tarball (adompkg) | the Linux container | skills AND NOW `bin/fusion-dashboard` |
| (unchanged) page media/docs | the wiki page itself | README, heroes, proofs |

The pkg previously carried ONLY skills. The dashboard adds the first **Rust binary CLI**
to the container payload: `fusion-dashboard` - it serves and manages the Dashboard.

### Why a Rust CLI in the container (not a Windows-side page)

The user's AI lives in the container; the container talks to every ab box via the relay.
A container-served dashboard can aggregate MULTIPLE machines, reuse the `adom-bridge`
CLI for all status (no new protocol), and install/update through the exact pkg flow
users already have. Ports proxy to the user's browser through Hydrogen's port mapping.

### fusion-dashboard CLI contract

```
fusion-dashboard serve [--port 8930] [--target <box>]   # serve UI + status API
fusion-dashboard open [--pup|--webview]                 # open it on a surface
fusion-dashboard status                                 # one-shot JSON (the LED payload)
```

- Status is gathered by SHELLING `adom-bridge --target <box> fusion_readiness` (+
  aps_status, mcp_status) - the CLI is a thin aggregator, the bridge stays the truth.
- The web app polls `/api/status`; actions POST to `/api/action` which shells the
  matching verb (fusion_start, fusion_set_auto_update, fusion_demo tour, ...).
- EVERY API response carries `_hint` fields teaching the AI what it can automate -
  the dashboard is a surface for humans AND a discovery surface for AIs.

## Serving surfaces

- Hydrogen webview: `adom-cli hydrogen workspace add-tab` on the proxied port.
- pup: `pup_open_window` at the proxied URL (background per etiquette).
- Dock Bar: registers the dashboard URL as this bridge's dock entry (see the wiki Dock
  Bar docs; the dock click = open the URL on the user's preferred surface).

## Build + release ritual (PROVEN live, v1.9.42, 2026-08-16 - follow exactly)

1. `cd dashboard && cargo build --release` (tiny_http + serde_json only; opt-level="z",
   lto, strip -> ~640KB). The UI is `include_str!`d, so the binary is the whole app.
2. **The binary is a RELEASE ASSET, never tarball content** (the wiki linter enforces
   PKG_VS_RELEASE): `adom-wiki release upload adom/fusion-bridge <ver> bin/fusion-dashboard
   --platform linux --arch x64`. Download URL:
   `https://wiki.adom.inc/download/adom/fusion-bridge/<ver>/fusion-dashboard`.
3. `install.sh` downloads that URL to /usr/local/bin (fallback ~/.local/bin) + chmod +x.
   Bump `DASH_VERSION` in install.sh each release (it pins the asset version).
4. Publish the pkg with **`adompkg publish --org adom --yes`** from the repo root.
   ⚠ package.json `slug` must be `fusion-bridge` (the PAGE slug) - it was
   `adom-desktop-fusion-bridge` for ages and adompkg treated the publish as a NEW page
   (and demanded a Hero Studio hero). `adom-wiki pkg publish` keys off the same file.
5. The BRIDGE zip is unchanged by dashboard work - do not bloat it (release-files.txt
   does not list dashboard/).
6. Version the dashboard with the bridge (one BRIDGE_VERSION everywhere: Cargo.toml
   `version`, install.sh DASH_VERSION, page.json/package.json version).
7. Verify the circle end-to-end: `adom-wiki pkg install adom/fusion-bridge` in a container
   -> `fusion-dashboard health` -> `serve` -> `curl /api/status` returns live LEDs.

## The automation doctrine (John, 2026-08-18) - the Dashboard NEVER hands the user a chore

> "even if something is off, you should be detecting that and then auto launching all the needed
> pieces and features, and then continuing on with the symbol ... never make the adom user do
> anything manually."

- Opening the Dashboard = an intent to work: auto-green launches Fusion, drives sign-in, enables
  MCP, signs in to APS (toggle-able, sticky).
- EVERY action that needs Fusion calls `ensure_ready()` first, which joins an in-flight
  auto-green rather than duplicating it, and SELF-HEALS while waiting: clears blocking dialogs,
  and restarts Fusion once if it is running but the add-in never came up (waiting longer never
  fixes a missing add-in - it only loads at launch).
- Surface clicks that fail get a remediation pass (dismiss dialogs, re-ready, re-stage the part)
  and ONE retry before any error reaches the user.
- The old behavior - returning "Fusion is not ready, press Setup" - is the anti-pattern this
  section exists to forbid.

## Hard-won build lessons (v1.9.44, 2026-08-16 - John called out the misses)

1. **The house contract is ALL mandatory, not a menu.** v1.9.42 shipped without a favicon,
   without POST /ui/toast, without SSE, without CLI ui verbs - John: "they sure as fuck
   tell you to make a favicon." Before calling a dashboard done, check off: served
   favicon + <link rel=icon>, SSE /events, /ui/toast, /ui/click, CLI ui verbs,
   /state + /console, brand-hover app-info popover, clamped tooltips, 24h idle reaper,
   --ai-thread identity. THEN ralph-test each one live.
2. **tiny_http cannot stream SSE** - it buffers the entire response until the reader EOFs
   (verified: zero bytes reached a connected client, not even headers). The dashboard
   hand-rolls HTTP/1.1 on TcpListener with an explicit flush() per event. ~120 lines,
   smaller binary, full control.
3. **The container proxy buffers event-streams too.** Direct 127.0.0.1 streamed, the
   proxied URL delivered NOTHING. Fix (all three): `Cache-Control: no-cache,
   no-transform`, `X-Accel-Buffering: no`, and a ~2KB SSE comment padding written at
   connect. Without the padding the compression buffer sits on your first events.
4. **`*{margin:0}` resets kill `<dialog>` centering** - native centering is margin:auto;
   re-add `dialog{margin:auto}` or every showModal() pins top-left.
5. **`curl -I` sends HEAD** - a GET-only route 404s under it and you chase a phantom bug.
   Test GETs with `-o /dev/null -w "%{http_code}"`.
6. **Favicon-in-tab proof needs an OS window capture** (desktop_screenshot_window on the
   pup hwnd - pup window titles end "- Google Chrome for Testing"); a page screenshot
   never shows the tab bar.

## SDK evangelism

ab's Bridge SDK gets this pattern as "the container binary facet" - filed as adom/adom-bridge#81
(⚠ NOT adom-bridge-sdk: that repo is a dead letterbox, the SDK lives in the main adom/adom-bridge
repo now) so kicad-bridge can mimic. Keep the contract doc in sync when the CLI grows.
