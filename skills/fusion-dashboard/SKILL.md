---
name: fusion-dashboard
user-invocable: true
description: The Fusion bridge's Dashboard (Launch Page) - a web app showing LED status for every bridge facet (Fusion running, sign-in, APS cloud search, MCP server, Data Hub, pending update), a first-run Setup flow, a Settings panel, copy-paste AI prompts, the demo tour, and a wiki part import demo. Served by the fusion-dashboard CLI installed in this container. Use when the user asks for the fusion dashboard, fusion status page, fusion launch page, "what's going on with fusion", or clicks the Fusion dock entry.
---

Parent skill: adom-desktop-fusion-bridge (the fusion-bridge page skill).

# fusion-dashboard - the bridge's Launch Page

A Rust CLI in THIS container serving the Dashboard web app. Status comes from shelling
`adom-bridge` (the bridge stays the single source of truth); every API response carries
`_hint` fields telling you what to automate next.

## CLI

```
fusion-dashboard serve [--port 8930] [--target <box>] [--print-url]   # serve the app
fusion-dashboard status [--target <box>]                              # one-shot LED JSON
fusion-dashboard ui toast "msg" [--kind ok|err|info]                  # narrate OVER the running app
fusion-dashboard ui click <setup|start|stop|tour|import-part|settings|refresh>
fusion-dashboard ui state | ui console                                # observe the running app
fusion-dashboard open [--pup (default) | --webview] [--target <box>]  # put it on a surface
fusion-dashboard health                                               # liveness
```

- `serve --print-url` prints the local URL first - the Hydrogen dock's `await:"url"` step.
- `open --pup` opens a BACKGROUND pup window on the user's desktop (taskbar flashes; never
  steals focus). `open --webview` prints the proxy URL plus the webview-skill instruction -
  panel placement stays the AI's decision.
- No `--target` = first box from `adom-bridge targets`. Global `--ai-thread <name>`
  attributes every bridge call in the user's Activity Log.
- HTTP: `GET /api/status` (the LED payload), `POST /api/action {action, args}` (start,
  stop, signin, tour, tour_progress, set_auto_update, mcp_enable, aps_signin,
  import_basic_part), `GET /events` (SSE live channel: state/toast/click events),
  `POST /ui/toast {message,kind}`, `POST /ui/click {target}`, `GET /state`,
  `GET /console`, `GET /health` (includes sseClients - live viewer count).

## Driving it as an AI (the ralph loop)

The page holds an SSE connection; `ui click` makes THE PAGE click its own control (same
path a user takes) and `ui toast` narrates on top of the app while you work. Toast/click
responses return `delivered` = live viewers; 0 means nobody has the page open. Screenshot
via pup after each drive step and fix what you see.

## Displaying this app (REQUIRED - read before showing anything)

Reach it from outside the container ONLY via the proxy URL:
`https://<slug>.adom.cloud/proxy/8930/` (a webview/pup renders OUTSIDE the container, so
`127.0.0.1:8930` is unreachable there).

- **NEVER open this app in VS Code's "Simple Browser"** - not a webview in the Adom
  ecosystem: no live channel, no console read-back, no AI drivability. Using it is a bug.
- Surfaces: **wv** - Hydrogen webview panel (`webview` skill), **pup** (`pup` skill,
  background etiquette: never steal foreground), the user's **real signed-in browser**
  (Adom browser extension), and the **Dock Bar** (dock.json in this repo launches it).
- The `/api/status` LEDs each carry a `_hint` naming the verb that fixes a red LED - act
  on those instead of guessing.
