# Fusion Bridge + Codex Astra: placement to routed PCB to native 3D

<video width="100%" controls playsinline preload="metadata" poster="https://wiki.adom.inc/api/pages/adom/codex/files/docs/bq-placement-3d-poster.png" src="https://wiki.adom.inc/api/pages/adom/codex/files/videos/fusion/13-bq-astra-placement-routing-3d.mp4"></video>

**4:12, narrated.** [Meeting-ready Codex page and full results](https://wiki.adom.inc/adom/codex/files/docs/BQ-PLACEMENT-ROUTING-3D.md) · [Shared EDA methods](https://wiki.adom.inc/adom/eda-engineering)

Astra calculated an independent placement and explicit routes for Adonis's BQ25792 board. Fusion Bridge connected those calculations to the real Fusion editor: 109 elements moved off-board, new placement inside the original outline, multilayer traces and pours, native checks, and populated 3D comparison against the original. The combination is powerful because the AI can inspect, act, read back and show evidence in the same engineering application.

## What the bridge contributes

| Stage | Native control used in this run | Evidence the AI must inspect |
|---|---|---|
| Identify session and board | `fusion_describe`, `fusion_get_app_state`, `fusion_activate_document` | Exact target, document, workspace and ownership |
| Read source and rules | `fusion_export_eagle_source`, routing-state inspection, design-rule load/read-back | Actual footprint/netlist, layer stack and applied manufacturing profile |
| Place and route | `fusion_electron_run`; guarded `fusion_run_modeling_script` calling native `Electron.run` MOVE/ROTATE/LINE/VIA/POLYGON commands | Exported placement and per-net/layer copper against the accepted plan |
| Check the board | `fusion_routing_state`, native refill, `fusion_routing_drc` | Final airwires/errors/warnings after the actual replay |
| Explain progress | Existing Fusion activity palette and native net highlighting | Visible progress and highlighted geometry, not command acceptance alone |
| Native assembly review | `fusion_show_3d_board`, `fusion_view`, camera/occurrence visibility via `fusion_run_modeling_script` | Populated library models, matching-scale views, restored visibility |
| Preserve checkpoints | `fusion_save_to_cloud`, document-hygiene state | Saved task documents; no silent bulk discard |
| Record and deliver | Adom Bridge `desktop_record_window_start/stop`, `pull_file`, `send_files`, host playback | Real Fusion footage and verified requested file in the player's UI |

Discover current argument schemas before execution. The installed high-level `fusion_route_net` exposed conservative two-layer preflight; this **four-layer** example used a separate explicit planner and native Electron.run execution. The bridge is not being described as a built-in general four-layer autorouter. It supplies live access, feedback and native validation; Astra and its calculation scripts supply placement and routing decisions.

## Why native feedback matters

A copper-union verifier caught a representation issue: Fusion can merge overlapping same-net geometry without changing occupied copper. Verification was corrected to compare actual copper per net/layer with widths retained. Later, native DRC still found one LDO_3V3 airwire despite matching copper coverage. Astra calculated and visibly added an explicit junction; the final native check reported **zero airwires, errors and warnings**. Neither tool acceptance nor a mathematical overlap alone was treated as completed connectivity.

The first 3D generation outlasted the add-in response timeout. State inspection showed Fusion had created the populated 3D PCB, so the operation was not blindly repeated. Both 3D documents were saved. This is why lifecycle hints, state inspection and native views are part of useful AI control.

## What viewers learn in 3D

The matched comparison shows the real stacked connector models, then hides only those daughterboards temporarily to reveal the base PCB. It explains the moved charger and capacitors, inductor, battery transistor, input-switch grouping and programming contacts. Adonis's stronger symmetry is acknowledged; Astra's design is an alternative with more routing vias, not an unqualified claim of superior engineering.

The final board contains 109 elements, 50 nets, 879 native wire segments, 157 routing vias and 69 pour outlines. All traces follow horizontal/vertical/45° directions. Manufacturing-profile details, changed contact drills, reference DRC and unresolved electrical/mechanical/thermal checks are documented on the [Codex result page](https://wiki.adom.inc/adom/codex/files/docs/BQ-PLACEMENT-ROUTING-3D.md). Visual 3D comparison is not interference, tolerance or thermal signoff.

## Reuse the method

Use the AI-neutral EDA Skillpack skills `eda-placement-routing-review`, `eda-component-placement`, `eda-board-3d-comparison` and `eda-design-summary-video`, alongside the bridge's existing electronics, routing, cloud-save and document-lifecycle skills. Read them directly from the [skillpack wiki](https://wiki.adom.inc/adom/eda-engineering); its installable package is not yet released. Manufacturer rules and interface constraints belong to each user's board. Never copy this session's desktop, HWND or cloud document identities.

[Copy/paste workflow prompt and video](https://wiki.adom.inc/adom/codex/files/docs/BQ-PLACEMENT-ROUTING-3D.md) · [All Astra Fusion progression videos](https://wiki.adom.inc/adom/codex/files/docs/ASTRA-FUSION.md)
