# Finite exploded-preview controller candidate

This source candidate adds `exploded_preview_create`, `exploded_preview_sample`, `exploded_preview_show`, `exploded_preview_dispose` and `exploded_preview_inspect` under `fusion_mechanical` (61 typed operations, still 179 public verbs). Its revised native backend passed on candidate-initialized owned fixtures in Fusion 2705.1.30: 11 graphics observations, 66 instance poses, 55 copied-body geometry observations, eight refusals and two exact disposals. The source assembly, joints, BOM and document flags were preserved; two F3D archive roundtrips and four exact document closures passed. The **installed public controller route and visual output remain unqualified**. The original two backend failures and their separate reconciliation evidence remain preserved. This no-image test qualifies native module behavior, not installed journal/creation-history routing or rendered output. See [qualification evidence](mechanical-verification/exploded-preview/README.md).

The backend copies solid BRep geometry into a separate blank presentation Design. Its flat graphics groups use the planner's root/world transforms, converted from mm to cm once. Source occurrences, bodies, joints, grounding and visibility are never written. The prototype's exact native lease/copy contract is documented in [EXPLODED-PREVIEW.md](prototypes/EXPLODED-PREVIEW.md).

Because it copies every instance, a preview needs a plan of the whole inventory (`instanceScope: "all"`, which `exploded_view_plan` gives a design of up to 256 occurrences; the backend itself copies at most 64 instances and 128 bodies). A plan of a bigger design lists only the moved occurrences and their parents, and the preview refuses it with `preview_budget` before any copy. `exploded_view_render` and `exploded_view_stills` handle big assemblies.

The shared revision reader initializes every bounded component mesh collection before capturing any native component revision ID. Native diagnosis on modified fixtures found that a first child collection acquisition can also advance ancestor IDs. All original revision fields and exact stale-target checks remain; old targets and committed creation receipts are never rewritten. This affects ordinary mechanical target issuance as well as preview preparation.

The reader records the exact parent document's saved/modified flags before initialization, after it and after hashing. Invalid ownership, collection/read failures or changed flags refuse with bounded observations and `rollbackClaimed: false`; errors also attempt flag readback and preserve unknown observations. A changed dirty flag is never reset or hidden. A separate native function gate passed a fresh clean unsaved blank and a cold, already-modified local F3D control, with unchanged flags and exact cleanup. Saved/cloud-clean multi-component preservation remains unqualified. That gate applies to the shared revision reader, not this revised graphics lifecycle.

## Caller workflow

1. Inspect the source, retain its exact target and occurrence references, and call `exploded_view_plan` with explicit steps, frames and finite sample times. Retain the original planner inputs and returned plan ID.
2. Explicitly create a blank presentation with current-session typed `new_design`, without a box. Keep its operation ID and returned target. The new blank document must remain active, unsaved, unedited and free of graphics.
3. Call `exploded_preview_create` with those exact identities. The server verifies the authoritative committed creation result, reserves an exclusive session lease, activates the exact source inside Fusion, regenerates the plan, and creates presentation copies at home. Successful creation leaves the source active.
4. Apply finite stored sample indices with fresh operation IDs/current generation. `exploded_preview_show` explicitly activates the owned presentation. It refuses to switch away from an unrelated active document. Sampling itself leaves the active document unchanged.
5. Explicitly dispose the owned graphics. The empty presentation document remains open for the caller. No automatic save, close, global Undo or cleanup by document name is provided.

```json
{
  "op": "exploded_preview_create",
  "sessionId": "<current session>",
  "operationId": "<new create intent ID>",
  "target": {"documentId": "<blank presentation ID>", "revision": "<blank revision>"},
  "creationOperationId": "<successful new_design operation ID>",
  "sourceTarget": {"documentId": "<source ID>", "revision": "<unchanged source revision>"},
  "expectedPlanId": "<plan ID>",
  "planner": {
    "unit": "mm",
    "steps": [{"durationSeconds": 2, "moves": [{
      "occurrence": "<replace with full guarded occurrence reference>",
      "direction": [0, 0, 1], "distance": 20, "frame": "world"
    }]}],
    "sampleTimes": [1],
    "playback": "explode"
  }
}
```

The occurrence placeholder must be a complete guarded object, not its name or a string. `planner` contains the same unit/steps/sampleTimes/playback inputs used for planning; session and source target come from the guarded outer request. No client matrices or replacement sample arrays are accepted. Initial creation is the home pose, including when the stored samples use reverse playback.

Sample/show/dispose requests use the stored presentation target, `previewId`, `expectedGeneration`, current session and a fresh operation ID. Sampling also requires a strict integer `sampleIndex` into the server's stored samples. For example:

```json
{
  "op": "exploded_preview_sample",
  "sessionId": "<current session>", "operationId": "<new sample intent ID>",
  "target": {"documentId": "<presentation ID>", "revision": "<original new_design revision>"},
  "previewId": "<returned controller capability>", "expectedGeneration": 1,
  "sampleIndex": 1
}
```

A successful new effect advances the generation. Concurrent or stale intents with an old generation refuse. Repeating an index with a fresh ID/current generation applies that absolute sample; repeating the same operation ID and identical arguments retrieves its immutable completion receipt without executing again.

## Ownership and status

The server requires a coherent, committed current-session `new_design` result and its exact returned document ID, native creation ID and original revision. That proof is combined with the still-open native object in the session identity registry, exact blank state, and an exclusive in-memory lease. A client success receipt, document name, label attribute or creation ID alone cannot establish ownership. The journal's execution-observed document for `new_design` is the document active before creation; it is not the newly created result document.

The request's presentation target remains the **immutable creation anchor** throughout this lease and any authorized reuse. `presentationTargetScope: immutable_creation_anchor` makes that distinction explicit. `lastVerifiedPresentationRevision` and `presentationVerificationSequence` describe the separate private presentation verification chain. Native observation showed that each owned graphics sample and deletion can advance only the root Component's opaque revision ID, including reapplying an unchanged sample. The candidate brackets these effects on the main thread: it first requires the exact previous private revision, then verifies the exact owned tree (or acknowledged deletion, empty inventory and invalidated retained root), unchanged flags and unchanged non-graphics witness before advancing its private expectation. It never changes the original creation result, source target, shared revision algorithm or operation history.

The bounded presentation witness reconstructs the exact empty-model core aggregate and checks its digest against the unchanged global reader before and after observation. It excludes only the measured opaque root revision slot from its private semantic comparison. Configuration, model/history inventories, metadata, all-group document/design/root attributes, folder visibility and the seven origin entities' identities, geometry, visibility and attributes remain bound. This is a narrowly specified blank-model witness, not a complete application/view/render-settings witness. Unrelated revision changes outside an owned effect still refuse even when that semantic witness is identical.

A successful disposal returns `observedEmptyTarget` as its completion-time empty-model observation. Reuse still submits the immutable creation anchor and requires the same live document/root and controller epoch, a coherent committed predecessor disposal/release, no unresolved prior operation, and exact equality with the predecessor's retained final revision/witness. Merely observing an empty document or matching a historical textual ID cannot authorize reuse. The disposed entry remains a tombstone; observation does not recreate resource rights.

`exploded_preview_inspect` takes the current session and preview ID. It performs main-thread live reads, reports the current generation, graphics/source observations and the last operation's coherent terminal-history status. It is not a busy-safe/offline endpoint. `mech_operation_status` remains the direct observational route for durable status while native work is busy or unavailable. Never recover a timeout by replaying a mutation with a new ID.

Operation results use `receiptScope: operation_completion`; live inspection uses `live_observation`. Replaying successful create after disposal returns the old completion receipt and never recreates graphics. The live capability is separate from historical success. A failed partial sample reports `pose: unverified` and null current sample index; its last verified index is identified separately. Disposed entries return an explicitly registry-only tombstone: native observation is not performed, current document disposition is unknown, and no live resource rights are asserted even if that document has since closed or been reused.

## Failures and bounded lifetime

Creation can reserve at most four live/failed previews, 128 total session entries, eight MiB of retained serialized plan/sample JSON and 512 acknowledged body copies. Per-preview native topology/visibility bounds still apply. Capacity failures never evict retained resources. Every retained serialized string is counted, including separate sample copies. Disposed tombstones are bounded; a full session refuses further allocation.

A failure before the graphics factory is ever invoked may release a reservation only after exact owned blank/zero-graphics readback. Any activation already performed stays recorded. Unknown factory effects or unidentified descendants retain the lease and exact handles. Failure does not imply a transaction rollback.

Sample/show require an unchanged source. An explicit first disposal can clean up exact-owned graphics after source closure, a partial sample, or failed terminal-history persistence. For the persistence exception, its own new admission must be durable and the prior synchronous native handler must have actually returned with no work still in flight, the same epoch/generation, and complete exact root/descendant ownership. The previous unknown operation history stays unchanged. A returned partial sample may establish a separately recorded cleanup-only revision after exact tree and non-graphics verification; it never becomes a ready pose or authorizes more sampling. An unreadable/changed witness or unknown descendant retains the failure without adopting the latest revision.

A false/throwing delete or failed absence readback latches an uncertain deletion. A fresh operation ID or generation cannot authorize a second delete. The document/resources remain for explicit reconciliation. If deletion succeeds but its terminal persistence fails, the observed deletion remains distinct from a durable completion; no automatic replay, replacement create or close follows.

Session restart loses native lease capabilities. Old-session journal records remain readable but cannot authorize adopting, sampling or deleting surviving graphics by a textual ID. No startup sweep or automatic document closure occurs.

## Usability and coverage limits

`show` only activates the document. Its default camera may not frame a large or widely exploded assembly. Automatic framing, camera changes, fit/refresh behavior and visual acceptance are unqualified; they require a separate explicit action and purposeful visual gate. Numeric planner matrices remain available, but this controller does not yet return qualified world-space graphics bounds. Activation is not evidence that every copy is visible on screen.

No material/texture fidelity, full-scene witness, graphics F3D persistence, native storyboard, rendered still/video, trails or collision-free disassembly is claimed. Hidden/suppressed instances or bodies, meshes and nonsolid BRep are refused by the backend rather than silently omitted. The no-image native gate established copied geometry, transforms and exact cleanup; installed public routing and a purposeful visual checkpoint are separate remaining gates.

Offline checks:

```sh
python3 -m unittest discover -s tests -p test_exploded_preview_controller.py -v
python3 -m unittest discover -s tests -p test_exploded_preview_prototype.py -v
```
