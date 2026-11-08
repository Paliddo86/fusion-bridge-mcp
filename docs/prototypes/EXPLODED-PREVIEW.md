# Exploded presentation geometry prototype

This is an **unregistered native backend**, not an installed/public operation. It creates independent solid BRep display geometry in a separately owned blank Design, applies finite stored samples, and deletes only its exact graphics tree. It does not move source occurrences, joints or bodies. The separate staged controller exposes 61 typed operations in total; this module itself is not registered as a handler. Release versions remain unchanged. The revised backend passed native qualification on candidate-initialized owned fixtures; installed public routing and visual output remain unqualified.

## Interface and ownership

The native qualification harness creates and registers a fresh unnamed presentation Design using its existing exact-document ownership infrastructure. Preparation runs while the source is active; thereafter source checks use the retained exact source Document/Design, session identity, revision, flags, fresh entity resolution and visibility/geometry witnesses. This deliberately supports an inactive source without weakening `core.target`, which still requires an active exact target during preparation.

```python
from .exploded_preview_prototype import prepare, OwnedPresentationLease, Preview

prepared = prepare(app, original_planner_args, expected_plan_id)
lease = OwnedPresentationLease(app, owned_doc, owned_design, run_id, verify_owner)
preview = Preview(prepared, lease)  # Retain this object BEFORE create can raise.
created = preview.create()        # Native geometry copied at its home pose.
preview.sample(1)                 # Exact integer index into server-generated samples.
preview.activate()               # Explicit Fusion-document activation; no OS focus call.
preview.sample(2)                 # Finite call, no timer or playback loop.
disposed = preview.dispose()      # Deletes graphics; never closes a document.
```

`verify_owner()` must independently recheck the exact native registry object/run marker and return exactly `{success: true, ownerId, sessionId, documentId}`. Any error, false status, extra ambiguous envelope, foreign identity, saved document or nonblank model refuses. The lease must belong to the same application and a different document from the source. Source documents are never selected by name.

The harness remains responsible for document creation, registration, archive policy and exact cleanup. The prototype never creates/closes/saves a document or performs global Undo. A blank Design means one root component, no BRep/mesh bodies, occurrences, sketches, features, parameters or timeline entries. Its non-graphics model and saved/modified flags must remain unchanged. A private expected presentation revision is advanced only across a verified owned graphics effect; all unbracketed revision changes refuse. Creation also requires no existing custom graphics; subsequent calls require the exact owned graphics inventory.

The top-level ownership proof is complete unique API-equal membership of the retained root Component's `customGraphicsGroups`, plus the retained valid group type/ID. Native diagnosis found the top group's `parent` getter reporting another CustomGraphicsGroup, so no Component-parent assumption is made there. Every retained subgroup and body still requires its exact parent and collection identity.

A separate native diagnostic on the retained failed fixture measured only the root Component's opaque `revisionId` changing on each finite sample (including sample 0 repeated) and deletion. The source correction uses strict pre/post effect boundaries and a bounded blank-model witness. It rebuilds the unchanged core aggregate, proves the actual digest, and permits only that one opaque slot to differ after exact graphics readback. The witness additionally binds document/design/root attributes, folder visibility and the built-in origin entities. No source or global revision check is relaxed. Newly bracketed create/sample/dispose behavior and these added getters remain pending native qualification; original failed campaigns are not rewritten.

By default, successful creation returns to the exact source document if the presentation is still active. It refuses to activate away from an intervening foreign active document. Explicit `activate()` accepts only the source or owned presentation as the current document and verifies activation. Sampling performs no document activation. There is no viewport/camera assignment, foreground call, timer, capture, image upload or Shotlog integration.

## Copy and frame contract

Preparation regenerates the plan on the server from its original guarded arguments and compares `planId`. The object retains private JSON for that result and its finite samples. Samples have a separate digest because playback/sample times are not part of the planner's plan ID. No interface accepts client matrices, client geometry or replacement samples.

One graphics root contains **flat sibling** groups for the fixed root-owned bodies and each physical occurrence, including empty assembly nodes. A body's input is its component-definition native BRep, never an occurrence proxy. `CustomGraphicsGroup.addBRepBody` creates the independent display copy; the instance subgroup receives the planner's root/world transform once. Only translation slots 3/7/11 are converted from millimetres to native centimetres. Body and top-level root graphics transforms stay identity. This avoids applying an ancestor's transform twice to nested or repeated instances.

Copies are checked through the returned transient `CustomGraphicsBRepBody.bRepBody`: positive solid volume (cm³), local bounds (cm), face/edge counts and corresponding vertex coordinates (cm). Group transforms are read back after every finite sample. These observations supplement the native copying API contract; they do not prove appearance, material or texture fidelity, nor provide a complete shape-equivalence algorithm. Both root selection and child selection are disabled and verified.

Supported scope is at most 64 physical instances, 128 solid BRep copies, 4,096 faces/edges/vertices per body collection and 16,384 total copied vertices. The plan must list every instance (`instanceScope: "all"`); a moved-scope plan of a design over 256 occurrences refuses before any copy. All physical source occurrences and all contextual source bodies must be effectively visible. Suppression, hidden components/bodies, any instantiated/root mesh body, nonsolid BRep, invalid context or unavailable required getter refuses the entire preparation. No unsupported body is silently omitted. Empty assembly components are retained as groups. Sketches, construction geometry, decals, point clouds, existing custom graphics and other non-BRep scene objects are outside this explicitly **solid BRep presentation** scope; there is no full-scene completeness claim.

Effective visibility is checked on exact occurrence/body proxies before using the native body as copy input. Transient copies are never queried for `BRepBody.isVisible`, which the API does not support. Source geometry and revision are rechecked before/after effects. Source visibility and suppression are never changed to improve a preview.

## Failure, disposal and lifetime

The `Preview` object retains its root immediately after creation, each subgroup before further setters, and each acknowledged body copy. A failed create/sample becomes `retained_failure`; it cannot be replayed or sampled again. Explicit disposal remains available after source staleness/closure or a partially applied sample, provided every graphics descendant is still exactly one of the retained owned objects. A returned partial effect can record a cleanup-only revision only after exact descendant ownership and unchanged non-graphics/flag proof. The last verified ready revision stays distinct, and sampling remains disabled. Unknown factory effects, unknown descendants or changed/unreadable witnesses cannot be adopted merely by rereading them. This permits disposal without requiring a successful source read.

Null/throwing native creation may leave unknown geometry. If native child inventory contains an unacknowledged object, the prototype refuses automatic disposal and retains the owned presentation for independent reconciliation. It never deletes a group discovered merely by name/id. `deleteMe` must return true and the exact root collection must then be empty. A false/throwing delete or failed absence readback retains ownership evidence and permanently latches that attempt; calling dispose again cannot issue a second native deletion. An uncertain disappearance needs separate native reconciliation. Document closure is always the owned caller's separate responsibility.

The lease and samples are session-local, process-memory capabilities. There is no restart rebind, automatic recovery or durable graphics replay. Closing/changing the source invalidates sampling. No F3D persistence, native storyboard, render, animation video, trails, text labels, collision-free disassembly or visual acceptance is claimed. `visualQualified` is always false in prototype receipts. The next native gate must demonstrate actual viewport geometry separately.

## Finite qualification gate

Use the already-qualified six-instance repeated/nested/rotated planner fixture. Expected presentation: one owned root, seven flat groups, five independent BRep copies and an otherwise blank model. Check home, midpoint, final, reverse-home and out-of-order/repeated stored samples against independent matrix/geometry oracles. Check both parent and child translations are applied once, each copied body's local geometry equals its native definition, and no source API entity is moved or hidden.

Recheck the source's full model/joint/BOM graph and document flags before/after creation, sampling and disposal. Check ignored setters, ownership mismatch, invalid sample indices, source closure/staleness and partial creation/sample faults retain the appropriate ownership evidence. Archive/reopen the source normally. After verified graphics disposal, independently witness the empty presentation model before its exact-owned archive/reopen/closure. A disposed empty F3D does **not** establish graphics persistence. Any undecidable effect retains the exact document; no global Undo or blind cleanup.

The finite runner passed on Fusion 2705.1.30: 11 graphics observations, 66 instance poses, 55 copied-body geometry observations, eight refusals and two disposals. Its source/presentation F3D roundtrips, four exact closures and restoration of the original active FloorPlan document passed. The runner initialized only the owned fixtures with the candidate revision reader before issuing fresh targets; the installed public controller was not invoked. See [qualification evidence](../mechanical-verification/exploded-preview/README.md). A later explicit qualification may use one meaningful targeted visual observation; no automatic capture or feed is part of the backend.

## Primary API references

- [Independent BRep copy creation](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_CustomGraphicsGroup_addBRepBody.htm) and [custom graphics overview, including bodies from another Design](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/CustomGraphics_UM.htm).
- [Group ownership, transform and selection controls](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_CustomGraphicsGroup.htm), [transform property](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_CustomGraphicsEntity_transform.htm) and [Boolean deletion acknowledgement](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_CustomGraphicsGroup_deleteMe.htm).
- [Returned transient display body](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_CustomGraphicsBRepBody_bRepBody.htm), [body volume units](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_BRepBody_volume.htm), [contextual occurrence visibility](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_Occurrence_isVisible.htm) and [effective nontransient body visibility](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_BRepBody_isVisible.htm).
- [Opaque component revision IDs](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_Component_revisionId.htm) and [top-level custom graphics collection](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_Component_customGraphicsGroups.htm).
- [Explicit document activation](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/core_Document_activate.htm). [Document close](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/core_Document_close.htm) remains outside this module and must follow the existing ownership harness.

Offline regression command:

```sh
python3 -m unittest discover -s tests -p test_exploded_preview_prototype.py -v
```
