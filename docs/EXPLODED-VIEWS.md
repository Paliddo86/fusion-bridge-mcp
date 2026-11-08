# Exploded-view plans and planned presentation backends

The `exploded_view_plan` source candidate provides read-only translation plans,
ordered assembly/disassembly instructions, component-linked instance labels and
finite numeric pose samples. Native module-function qualification passed on
Fusion 2705.1.30; installed public routing remains untested. See the
[checkpoint evidence](build-2026-10-06-exploded-plans.md). It does not create
an exploded viewport, graphics, trails, a storyboard or video. Those presentation
backends remain a core animation requirement, and must preserve the source
assembly's joints, grounding, positions and configuration.

A separate [finite preview controller candidate](EXPLODED-PREVIEWS.md) implements the next source slice; native graphics and installed routing are still unqualified. It does not change the numeric planner’s accepted behavior.

## Current planning contract

Call `fusion_mechanical` with this operation, the current `sessionId`, an exact
`target` from inspect, explicit `unit: "mm"`, and one or more ordered `steps`.
Each step supplies `durationSeconds` and `moves`; optional `title` (256 characters)
and `instruction` (1024 characters) describe the assembly action.

```json
{
  "op": "exploded_view_plan",
  "sessionId": "<current session>",
  "target": {"documentId": "<current document>", "revision": "<current revision>"},
  "unit": "mm",
  "steps": [
    {
      "title": "Lift the cover", "instruction": "Lift evenly along its home local Z axis.",
      "durationSeconds": 2,
      "moves": [{"occurrence": "<replace with exact inspect reference object>",
                 "direction": [0, 0, 1], "distance": 20, "frame": "local"}]
    }
  ],
  "sampleTimes": [0.5, 1, 1.5],
  "playback": "explode"
}
```

The reference placeholder must be replaced by the whole guarded occurrence object;
component names or part numbers cannot select an instance. Directions are normalized
and must be nonzero. Distance is positive; reverse direction with a negative vector
component. Supported frames are `world`, `parent` and `local`, all fixed to the
immutable home orientation of the exact instance. A top-level instance's parent
frame is world. Rotated repeated subassemblies therefore give different world
translation directions for their otherwise identical local parts.

Moves in one step are simultaneous; ordered steps accumulate their translations
with linear interpolation. Descendants inherit each ancestor's translation exactly
once and add any explicitly selected child translation. The returned root matrix is
the saved home matrix with those world translations added. It never multiplies an
already root-space child transform by its parent a second time. Rotation interpolation,
automatic radial layouts and collision-free disassembly are unsupported.

`plan.instances` freezes each root-context occurrence's guarded reference, exact
full path, component identity, parent instance ID, part number and row-major home
matrix. Matrix translations are mm at indices 3, 7 and 11. Instance IDs bind session,
document, path and component identity; source references must also resolve by native
API equality. Repeated instances share component labels but have distinct instance
labels and poses. Duplicate part numbers do not merge distinct components. Labels
are presentation identifiers linked to the component grouping used by BOM readback;
they are not authoritative engineering item numbers. Engineering/purchasing quantities
are neither recalculated nor changed. Root-owned direct bodies are returned as a
fixed root group and cannot be selected as occurrences in this slice.

### Designs over 256 occurrences

A design of up to 256 occurrences gets every root-context occurrence in
`plan.instances` (`instanceScope: "all"`). A bigger design gets the occurrences the
steps move and all their parents (`instanceScope: "moved"`); `designInstances` gives
the design's occurrence count and `scopeNote` the rule. Every other occurrence keeps
its home pose plus the translation of its nearest listed parent, so the children of a
moved subassembly follow it. Each listed occurrence is found by name, level by level
from the root (`itemByName`), so the plan's own reads never walk the whole tree; the
revision check that every guarded operation runs still reads each occurrence once. A
parent still comes from the native assembly context, never from the name. Listed rows
carry the same facts the full inventory gives: the same instance IDs, paths, home
matrices, part numbers and sampled poses.

A component inserted by reference from another saved design is planned like any
other (read-only here, as the revision digest treats it). Part numbers are read once
per component revision for the session, because each is a cloud read of about 0.5 s
on a cloud assembly; a `bom` run's reads are reused. An exploded preview copies every
instance, so it refuses a moved-scope plan; use `exploded_view_render` or
`exploded_view_stills` for a big assembly.

Measured on scratch designs of positioned subassemblies (Fusion 2705): at 1,088
occurrences a plan of 3 moves took 0.6 s and one of 128 moves (136 listed
occurrences) 1.0 s; at 4,590 occurrences, 3.0 s and 4.5 s. The plan's own reads took
about 0.1 s at either size; the two revision checks were most of the rest. Before
this, both designs refused, and lifting the 256 cap alone spent 17 s capturing the
1,088-occurrence tree twice before refusing a reply over 2 MiB.

`planId` hashes the returned immutable plan; each sample includes its time, forward
plan time, per-step progress and instance matrices. `playback: "assemble"` samples
the same plan backwards. Both endpoints are always included; the last assembly
sample returns the exact saved home matrices. Pure sampling does not modify the
home arrays. The plan is bound to one session/model revision and a complete supported
configuration witness; incomplete configurations refuse, and no old-session native
reference replay is enabled. No plan file is written automatically.

Limits are 256 occurrences for the full inventory (past it, the moved scope), 4,096
tracked occurrences in a moved scope (the moved ones with their parents), 32
hierarchy levels, 32 ordered steps, 128 moves in total, 3600 seconds total duration,
62 requested sample times plus the endpoints, 4096 instance poses (listed instances
times sample times) and 2 MiB output. The native reads have a 40 s budget counted
from the start of the operation, inside the relay's 60 s for a read. Each bound
refuses with `plan_budget`, the numbers involved and a `hint`. The pose and tracked
bounds are checked before any per-instance read and say how to split the steps or
sample less. Past the time budget the reply says how many instances were read and how
long the revision check took; running the plan again goes further, because the part
numbers already read are kept. Individual moves are limited to 1e6 mm and home
translations to 1e9 mm. Scaled, reflected, nonfinite or malformed native matrices,
ambiguous instances and incomplete parent relationships refuse. Main-thread, session,
document and revision guards precede native reads. A second instance/configuration
snapshot, document flag comparison and target check must remain unchanged; a conflict
refuses without attempting to restore user state. Native acceptance is separate from
offline frame-math and read-only-handler tests.

## Future presentation backend

Use a separate owned presentation scene with copied geometry for the initial
automated preview. Move its display nodes rather than source occurrences; do not
suppress joints or add source-design position snapshots to manufacture an exploded
view. Every repeated or nested physical occurrence needs its own display identity
and an explicit home transform. Parent and child offsets must be applied once in
the declared frame. Purchased-unit policy and displayed-piece counts remain
separate from engineering BOM quantities.

The presentation plan should bind exact source session/document/revision,
configuration provenance, occurrence identities, home matrices, geometry hashes
and a stable BOM-item map. Each ordered step declares selected instances,
translation/rotation/pivot, frame and units, timing, visibility, labels and trails.
Explicit directions and distances come first; automatic radial layouts must be
labeled as heuristics rather than collision-free physical disassembly proofs.

Fusion's documented Animation UI supports explode/restore, timed component actions
and trails. The inspected public API exposes animation managers and storyboard
lifecycle/playback, but does not expose a documented component-action/keyframe or
explode-authoring collection. Native storyboard authoring therefore remains a
separate backend requiring an independently verified command path. A custom
graphics preview must not be described as a native editable storyboard, drawing
or video. Relevant sources: [AnimationManager](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_AnimationManager.htm),
[Storyboard](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_Storyboard.htm),
[Animation transform tools](https://help.autodesk.com/cloudhelp/ENU/Fusion-Animate/files/ANI-TRANSFORM.htm),
[independent graphics copies](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_CustomGraphicsGroup_addBRepBody.htm).

The finite acceptance gate must exercise rotated/repeated/nested instances,
root bodies, purchased subassemblies and a grounded/jointed source. Independently
check each displayed matrix, labels/trails, unchanged BOM quantities and the full
source assembly graph. Reverse sequencing must restore presentation home poses.
Cleanup may remove only exact owned presentation objects; an uncertain call or a
user change retains evidence rather than invoking global Undo or overwriting the
user's newer state. Persisted presentation data and native archive reopening need
their own proof; graphics availability alone does not establish persistence.

Still rendering, finite requested video export and durable asynchronous job
lifecycle remain separate gates. Neither the numeric planning operation nor this
backend proposal enables rendering, video, capture loops, continuous feeds or
Shotlog uploads. Follow the
[capture policy](mechanical-verification/CAPTURE-POLICY.md).


Planning reads follow the documented [root occurrence inventory](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_Component_allOccurrences.htm),
[context-preserving child occurrences](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_Occurrence_childOccurrences.htm),
[assembly context](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_Occurrence_assemblyContext.htm)
and [transform2](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_Occurrence_transform2.htm).
