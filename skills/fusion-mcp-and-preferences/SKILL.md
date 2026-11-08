---
name: fusion-mcp-and-preferences
description: >-
  Use Autodesk's Fusion MCP server (the Autodesk + Anthropic text-to-CAD connector) from an Adom
  cloud AI, and change ANY Fusion preference including ones the Python API does not expose. The MCP
  server binds loopback on the user's machine so a cloud container cannot reach it; the fusion
  bridge proxies it. Covers turning it on for the user (no API exists for the toggle, so the bridge
  drives the Preferences dialog), the MCP session handshake, the four Autodesk tools, and when to
  prefer a native fusion_* verb instead. Trigger words - fusion mcp, fusion mcp server, autodesk
  mcp, claude connector for fusion, text to cad, 127.0.0.1:27182, enable fusion mcp, fusion
  preferences, change a fusion setting, fusion prefs dialog, api preferences, developer tools
  fusion, fusion setting not in the api.
---

# Fusion MCP server + preferences

Two capabilities that turned out to be the same trick: Fusion exposes far less to its Python API
than to its UI, and the UI is drivable.

## The MCP server

Autodesk + Anthropic ship a **local** MCP server at `http://127.0.0.1:27182/mcp` exposing Fusion's
own text-to-CAD surface.

**It binds loopback on the user's machine.** An Adom AI runs in a cloud container and cannot reach
it. The fusion bridge runs on that machine, so it proxies:

```bash
adom-bridge fusion_mcp_status '{}'          # up? serverInfo + live tool list
adom-bridge fusion_mcp_enable '{}'          # turn it ON for the user
adom-bridge fusion_mcp_tools  '{}'          # tools WITH input schemas
adom-bridge fusion_mcp_call   '{"tool":"fusion_mcp_electronics_read",
                                 "arguments":{"entity_type":"electronics.Element"}}'
adom-bridge fusion_mcp_resources '{}'       # electronics entity schemas
```

### Do not ask the user to enable it

It is OFF by default and there is **no API for the toggle**. `fusion_mcp_enable` does it for them:
opens Preferences, expands General, selects API, ticks the box, Apply, OK, then verifies the port
is listening. Tell them it is handled. Only fall back to instructions if the verb reports failure.

### The trap that costs a debugging cycle

**The setting is discarded unless Apply is clicked.** Restart or kill Fusion with the dialog open
and it reverts, while the checkbox looked correct. If the port is closed but the user says they
enabled it, this is almost certainly why. Re-run `fusion_mcp_enable`.

### The handshake is handled

Streamable-HTTP: a bare POST returns `400 Missing MCP-Session-Id`. You must `initialize`, capture
the `MCP-Session-Id` **response header**, send `notifications/initialized`, then pass that header.
The bridge does this and re-establishes dropped sessions, so `fusion_mcp_call` just works.

### Autodesk's four tools

| tool | for | the depth is in the params |
|---|---|---|
| `fusion_mcp_read` | read/search/see/introspect | `queryType`: `projects`, `document` (search/open/recent = **file search**), `apiDocumentation` (search Fusion's API docs), `screenshot` (any view-cube direction), `activeCommand` (read the live dialog) |
| `fusion_mcp_execute` | run scripts + document lifecycle | `featureType`: `script` (**arbitrary Python** = text-to-CAD), `document` (open/close/save) |
| `fusion_mcp_update` | undo / redo only | `featureType`: `undo`, `redo` |
| `fusion_mcp_electronics_read` | read the EAGLE object model | `entity_type`: **49 classes** across schematic/board/library + ERC/DRC `Error`, with `object.fields` / `object.filters` / `object.pagination` |

`fusion_mcp_resources` returns 50 resources: `electronics_entity_types` plus one
`electronics_schema_<class>` per class (properties, types, filter operators). Read the schema
before filtering. Electronics is READ-ONLY; author boards with the native `fusion_electron_run`.
Full write-up: docs/fusion-mcp-server.md.

MCP acts on the **active document**. Open one first (`fusion_aps_open`).

### MCP vs native verbs

**Prefer a native `fusion_*` verb when one exists.** They are tested, they return hints that teach
the next step, they handle the awkward parts (kit-aware BOM counting, the 2-minute sign-in clock,
GLB optimization), and they work without a Fusion subscription or MCP enabled.

**Reach for MCP** for Autodesk's text-to-CAD surface and the structured Electronics object model,
which is richer for querying a board than anything hand-rolled.

Requirements: Fusion subscription, Fusion running, loopback only.

## Any preference, via the UI

The API exposes a thin slice (`generalPreferences`: theme/orbit/units; `apiPreferences`:
`debuggingPort`, `isDeveloperToolsEnabled`). Everything else is UI-only.

```bash
adom-bridge fusion_prefs_open  '{"section":"api"}'   # opens + navigates + screenshots
# look at the screenshot, then click what you see:
adom-bridge desktop_click '{"space":"image","shotId":"<from response>","x":690,"y":476,"hwnd":<hwnd>}'
adom-bridge fusion_prefs_close '{"save":true}'       # Apply + OK
```

Sections: `general`, `api`, `design`, `manufacture`, `electronics`, `render`, `drawing`,
`material`, `graphics`, `network`, `preview features`.

### Why it returns an image instead of control names

`Commands.Start PreferencesCommand` opens the dialog, and the **top-level** section tree IS in the
UIA tree. But the children under General (API, Design, Manufacture, Electronics, Render, Drawing,
Simulation) are rendered **lazily by Qt** and never appear there, even after `desktop_ui_expand`
reports success. So child navigation is an image-space click, which takes the foreground and is
announced with a caption.

### Rules

1. **Always** finish with `fusion_prefs_close {save:true}`. Nothing persists otherwise.
2. **Verify the effect**, do not trust the click. For MCP, probe the port. For a render setting,
   observe the change.
3. Prefer `fusion_set_preference` for anything in the API-exposed slice; it is background and does
   not touch the user's screen.
4. Never leave the dialog open. A modal Preferences window blocks other Fusion verbs.

## Full write-ups

- [docs/fusion-mcp-server.md](../../docs/fusion-mcp-server.md)
- [docs/fusion-preferences.md](../../docs/fusion-preferences.md) (with screenshots of all three steps)
