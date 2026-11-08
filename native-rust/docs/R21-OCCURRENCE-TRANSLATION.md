# R21 finite occurrence placement

This source-only checkpoint adds `occurrence_update` under the operation-specific
`X-Adom-Mechanical-Contract: native-root-occurrence-translation-v1` header.
The advertised primary contract and runtime mechanicalContract remain
`native-root-assembly-v1`; all earlier finite operation headers remain valid.
`assemblyTransformsSupported:false` still describes general create/insert transforms.
`occurrenceTranslationSupported:true` describes this narrower operation.

A request contains current sessionId, a new operationId, the exact published
assembly target, and an exact occurrence ref:

```json
{"op":"occurrence_update","sessionId":"<epoch>","operationId":"move-1","target":{"documentId":"<exact-document>","revision":"<root-assembly-projection-v1 digest>"},"occurrence":{"documentId":"<same-document>","revision":"<same-digest>","objectType":"Occurrence","occurrenceId":"<published-exact-occurrence>"},"transform":{"position":[25,-10,5]}}
```

Position is absolute root-assembly millimetres, finite and bounded to ±1,000,000
on each axis. Native `Matrix3D::getCell(row,col)` data are row-major, with cm
translations at indexes 3/7/11; array flattening assumptions are not used.
Orientation must remain identity. The comparison tolerance is 1e-8 cm for pose
readback/no-op detection, and the existing projection validates nontranslation
cells within 1e-10 of identity. Rotation/axis/angle, either grounding argument,
foreign or retired ownership, grounding already present, nonlocal/nested/
configured/suppressed occurrences, and pre-existing pending positions refuse.
Existing owned definitions remain empty leaves. This does not implement a
collision solver, exploded-view animation, general transforms or legacy parity.

Rust performs complete before/after checks. A plan contains owned data only;
SDK handles remain on the owner thread. The worker durably starts the journal
before dispatching execution. Execution repeats admission state, then invokes
one transform2 setter. An acknowledged setter must yield two identical complete
projections with exactly the intended pose change, pending=true, unchanged
snapshot membership, unchanged timeline and unchanged component revisions,
metadata and sibling occurrences. Only then can one `Snapshots::add` occur.
Two further strict complete projections must agree: pending=false, the new exact
retained Snapshot at the next fixed slot, exactly one new timeline row at the
old timeline count, previous snapshots unchanged, and all other assembly facts
unchanged. The idle-command guard is SelectCommand on the owning callback; it
is not a proof of all internal transaction contexts.

This path has **no read-settling allowance**. R20-fix3's separately qualified
post-creation read-settling policy remains unchanged for old assembly effects.
Every placement root bracket and repeated capture must be strictly equal.
Snapshot and assembly membership checks enumerate complete bounded collections.
Snapshot references use fixed capacity 64, reserved before add, with exact
returned-object retention before fallible acknowledgment/readback. The retained
Snapshot children release before occurrence/component/design parents. A failed
or uncertain release retains the existing context/module fence. Snapshot
TimelineObject health must currently be Healthy (0), unsuppressed, unrolled,
and exactly present in the same design timeline. Other health states refuse;
actual SDK snapshot health/position behavior remains a live qualification gate.
Timeline capacity (4096) and snapshot capacity (64) refuse before effects.

The result records stages `occurrenceTransform` and `positionSnapshot` separately.
Each attempt has invoked/acknowledged, component and occurrence. Transform IDs
are the exact retained slots; position capture is design-wide, so its component
and occurrence are null. `snapshotSlot` identifies the retained returned slot.
`placementEvidence` exposes requestedPositionMm, requestedMatrixCm,
observedMatrixCm, intermediateProjectionDigest, and before/intermediate/after
position states `{pending,timeline,timelinePosition,snapshots:[{slot,timelineIndex,health}]}`.
Success includes the unchanged occurrenceId and a refreshed occurrenceRef.

An ambiguous setter/capture, panic, ownership change or failed postcondition
retains the observed invocation prefix, reports unknown/reconciliationRequired,
faults this assembly book, and authorizes no replay. A proven pre-invocation
refusal reports nativeExecutionStarted=false. Terminal persistence loss uses the
existing bounded observed-result layer. No automatic retry, rollback, timeline
movement or grounding setter occurs. Bounded first-difference and root-bracket
diagnostics preserve the available read failure without extra SDK calls.

The implementation is Rust policy and existing Rust HTTP/journal coordination,
with only typed C++ ABI calls. It does not duplicate John's independent Rust
mechanical review CLI or claim that CLI is integrated.

## Evidence and limits

The exact 186-file R20-fix3 baseline manifest is
`9dd6f2f225a21b71cdc8cc51fe0b480b86f351dcc253939ac96b0354b7dc5208`.
Its bytes are archived under evidence/baseline-source. No earlier source,
receipt, unknown operation or live model was altered by this source task.
The pause receipt in evidence/working records the temporary overlap review.

Portable tests cover repeated absolute moves, shared-instance preservation,
all admission bounds, both invocation failures/acknowledgment losses, wrong
readback/identity/timeline/member state, capture/command panics, replay/conflict,
no-op durable refusal, terminal storage loss, header refusal and the maximum
bounded success envelope. Linux tests, GNU Rust binding compilation and adapted
installed-header syntax checks do not prove Autodesk runtime behavior. Native
MSVC build, root source review, C21 protocol qualification and independent SDK
observation remain required. C21 is root-owned and excluded from this product
freeze. No host execution, installation or publishing was performed here.

Official sources: installed Fusion/Fusion/{Snapshot,Snapshots,TimelineObject}.h,
Fusion/Components/Occurrence.h and Core/Geometry/Matrix3D.h. Public references:
[transform2](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_Occurrence_transform2.htm),
[TimelineObject healthState](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_TimelineObject_healthState.htm).
The latter documents health as the associated object's state; this checkpoint
makes no assumption that an unknown health state is a healthy capture.
