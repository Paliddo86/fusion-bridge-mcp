---
name: fusion-routing
description: >-
  Route and review PCB copper in Autodesk Fusion through Fusion Bridge. Use for trace paths,
  vias, manufacturer design rules, power and ground connections, copper pours, thermal
  spreading and copper-ablation layouts. Inspect the actual board and verify native copper,
  connectivity and DRC. Applies to ordinary board work, independently of recording or demos.
---

# Routing and copper engineering in Fusion

Use the installed `fusion-bridge` skill for desktop discovery and application control.
Discover the live `fusion_describe` contract and check `fusion_readiness` before board calls.
If sign-in blocks readiness, resolve sign-in first. Since 1.10.1 a board call made while
Fusion is signing in returns `needs_signin` at once instead of a slow add-in restart diagnosis. Preserve pinned runtimes and existing user documents.
Work within the user's requested scope and save a recoverable checkpoint before replacing routes.

## Establish fabrication and electrical constraints

Honor a manufacturer/process already selected. Otherwise ask which manufacturer the user
wants, offering profiles returned by `fusion_set_design_rules {action:"list"}`. If undecided,
keep current rules explicitly provisional and continue inspection before dependent routing.

Load the selected fab, process and actual layer count with `fusion_set_design_rules`.
Export the loaded profile and inspect the file, including per-layer/per-object rules and
net classes. Compare rules by name and meaning, not file order or a successful command reply.
Legacy EAGLE keys such as `mdWireWire`, `msWidth` and `msDrill` do not describe every native
EDRU rule. Check native alerts for a rejected load. Keep the stricter applicable fabrication
and user/electrical constraints. A process's minimum isolation channel is not a universal
high-voltage clearance; derive insulation constraints from the actual environment and voltage.

Read [electrical priorities](references/electrical-priorities.md) for power paths, sensitive
signals, clean bends and via choices. Read [copper and thermal design](references/copper-thermal.md)
when assigning pours, retaining copper for ablation, or improving heat spreading.

## Read the board: the standard board model

`fusion_routing_state {expectDocument}` returns the EDA Bridge Standard's board model, the same shape
the Altium and KiCad bridges return: millimetres in Fusion's frame (y up), copper layers named
`F.Cu`, `In1.Cu`.., `B.Cu` (`*.Cu` for a through pad), `tracks`, `arcs`, `vias`, `pads`, `components`,
`polygons`, `outlineEdges` (arcs kept) and `unconnected` / `splitNets` computed from the copper.

- A pad's `name` is its own name (`3`) and `ref` its part; `key` is `U1.3`, the form `{pad: ...}`
  waypoints take. A custom-shape pad (`customShape: true`) is its bounding rectangle; `copperPoints`
  holds the real outline.
- Net-less copper has `net: ""`. Count it as copper; do not route through it.
- `unconnected` is from the copper. `fusionAirwires` is Fusion's ratsnest: higher when a track end
  overlaps a pad without landing on its anchor, lower when Fusion has not recomputed since an edit.
- The revision changes only with copper, parts or outline.
- Companions: `fusion_layer_stack` (copper and dielectric per layer, Dk), `fusion_pour_state` `zones`
  (the poured copper, island by island), `fusion_netlist` (live, or `schPath` with Fusion closed;
  `expect` to compare), `fusion_read_sheet_file` (parts and pin electrical types from a `.sch`),
  `fusion_bodies` (3D heights, parts with no model, models off their footprint).

## Select the supported routing path

Read `fusion_routing_state {expectDocument}` and retain its exact `boardId` and `revision`.
`fusion_route_net` requires these plus the exact net and points/paths. Coordinates and widths
are millimeters; a layer is `F.Cu`/`B.Cu` or Fusion's 1/16. Use pad references for endpoints where supported. Layer-changing waypoints
create through vias according to the live contract.

The conservative `fusion_route_net` preflight and bounded `fusion_plan_route` planner support
two-layer boards. The planner is a two-pad orthogonal A* search, not an all-net autorouter.
Do not feed it a four-layer power board or imply it enforces all electrical constraints.
For supported two-layer work, dry-run proposed geometry, inspect its electrical suitability,
then commit with the same guards. Replan after intervening changes.

For inner layers, native pours or other operations outside those verbs, use the documented
`fusion_run_modeling_script` / `fusion_electron_run` extension and the actual board stackup.
Native `LINE`, `VIA`, `POLYGON`, `CHANGE`, `RATSNEST` and `SHOW` are electronics commands,
not invented bridge verbs. These paths need their own identity checks, clearance analysis,
native readback and DRC; they do not inherit the dedicated routing verb's preflight guarantees.
Never transplant a worked example's layer numbers, dimensions or net names without inspection.

## Save the work

Save a design's first time with `fusion_save_to_cloud {name, projectName, folderPath}` (it saves the
design, linked, whichever tab is in front), and every later change in place with
`fusion_save_document {comment}`. Never Save As a design that is already saved: it forks the board.

## Commit, inspect, and recover

Inspect semantic `success`, `verified`, `mutationAttempted`, returned copper and revision.
Use the next returned revision for the next edit. Operations are not an atomic Undo group.
On timeout or mismatch, inspect the board before deciding whether any operation needs retrying.

On affected Fusion builds a completed net can raise Autodesk's unexpected-condition modal.
If the reply carries `fusionUnexpectedCondition` or a classified `fusion_unexpected_condition`,
treat it as an application fault with possibly committed copper. Verify actual connectivity
and geometry, checkpoint, and inspect remaining alerts before continuing. Do not count modal
dismissal alone as a verified route or blindly repeat the final segment.

Use the existing Adom activity palette for useful progress when requested; respect its close
state and the user's foreground preference. Recording, narration and fixture replay are optional
presentation tasks, not prerequisites for routing a board.

## Validate and save

Refill copper before measuring connectivity. Run native DRC before and after with the same
selected rules, reporting inherited and new violations separately. Since 1.10.3 each entry in
`drc.errors` carries `layer`, `x`, `y` (mm) and `drc.summary` counts them by description; use
the coordinates to find the objects (UL_ERROR has no object members). Overlap WARNINGS (code 19,
type 2) at pad entries, via joints and segment joints are same-signal overlaps raised by a
profile rule such as "Same-net track spacing" (all/all, samesignal=yes); `fusion_set_design_rules`
reports such rules in `ruleWarnings` when it loads a profile. They are rule noise, not copper
defects; a different-signal overlap is an ERROR (type 0) and still counts. `fusion_routing_drc` reads
approved errors as well; require `drc.checked`, clean applicable rules and no remaining
connections before calling the result connected and DRC-clean. A diagnostic rule profile
is not certification under the manufacturer's full profile. Check schematic parity/ERC and
power, thermal and signal-integrity requirements separately where relevant.

Inspect `documentHygiene` / `_hintDocumentHygiene`. Export a verified checkpoint with
`fusion_export_eagle_source` or save through the documented native/cloud workflow. Verify
output existence and usable contents. Exporting a file does not necessarily clear Fusion's
modified state. Close only completed task-owned scratch documents by exact name after verifying
the checkpoint; do not bulk-close or discard original user work.

Summarize actual changes, rule provenance, connectivity/DRC results, saved artifacts and remaining
engineering assumptions. Optional board-specific examples live under `demo/routing/` in the
Fusion Bridge wiki source; no example, video, Codex package or particular AI provider is required
to apply this skill.
