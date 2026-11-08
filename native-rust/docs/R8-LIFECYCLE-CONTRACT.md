# r8 native document lifecycle contract

All requests use authenticated `POST /v1/mechanical`, the current runtime epoch header, and `X-Adom-Mechanical-Contract: native-lifecycle-v1`. The parameter-only header remains supported for `inspect`, `parameter_create` and `parameter_set`; lifecycle operations under that older header refuse with `native_lifecycle_contract_required` before admission. The manifest is `0.3.0-dev` and never startup-autoloads.

## Creation

```json
{"op":"new_design","sessionId":"<epoch>","operationId":"create-1","name":"requested label"}
```

The requested label is session/operation-receipt metadata. The native document remains unnamed (`Untitled...`); this slice does not set its name or add a root attribute. `box` and explicit `target` arguments refuse with `native_blank_design_only` before journal admission or SDK dispatch. There is no transaction probe or full legacy `new_design` parity.

A successful result has `data.target` (`documentId`, `revision`), `data.document`, `data.inspection`, `requestedName`, `requestedNameScope: "session_and_operation_receipt_metadata"`, `creationOperationId`, `ownedScratch:true`, `cleanupProfile:"parameter_only_current_session"`, and `legacyParity:false`. Revision is the existing explicit parameter projection, never a geometry-wide or complete document revision.

The SDK allocates/publishes an owned holder before invoking `Documents.add` once. Rust retains any returned exact SDK reference before fallible readback. The normal identity registry adopts it using SDK object equality; names, creation IDs, inventory order and list differences are corroboration only. A returned preexisting object is rejected before parametric setup. The created object must be active, never saved, without a data file, and a blank parametric hybrid design. Original documents must retain exact identity membership and metadata.

Any ambiguous creation, setter or readback result remains unknown. The exact reference stays retained where available, the registry faults, and no automatic retry or failure cleanup is attempted. The journal records observed unknown status; no guessed ownership is reconstructed from a name or document-set difference.

## Explicit owned scratch discard

```json
{"op":"native_scratch_discard","sessionId":"<epoch>","operationId":"discard-1","creationOperationId":"create-1","target":{"documentId":"<created ID>","revision":"<fresh inspect revision>"}}
```

Both a live runtime ownership capability and an actual current-session durable successful `new_design` journal result are required. The creation operation, canonical request digest, session and exact document ID must agree. Historical records, unknown/nondurable observations, failed records, an unrelated target or a forged client payload cannot supply this capability. After restart the old journal result remains history and cannot authorize a live close.

The exact retained document must still be active, unsaved, without a data file, and retain the native name/creation ID captured at creation. It must have a single component, no bodies, sketches, occurrences, timeline items, custom graphics, mesh bodies, features or user construction geometry; total parameters must equal user parameters and remain bounded at 512. A fresh scoped parameter revision is required. This profile is bounded SDK resource evidence, not an assertion that every possible Fusion document property is inventoried.

The private capability moves to `Closing` before one `close(false)` invocation. Success requires a true acknowledgment, invalidity of that exact object, absence of its opaque ID in a complete fresh roster, and unchanged identity membership/metadata for the remaining original documents. Success data is `closedDocumentId`, `closeReturned:true`, `createdObjectValid:false`, `originalDocumentMetadataUnchanged:true`, `activeDocumentId`, and `automaticActivationPerformed:false`. Geometry of unrelated documents is not inspected or claimed unchanged.

Unknown close outcomes permanently fault the registry and retain a spent/unconfirmed capability. There is no retry, save, close-by-name, close-all, or implicit activation. Successful replay reads the stored result and never calls close again.

## Exact activation

```json
{"op":"native_document_activate","sessionId":"<epoch>","operationId":"activate-1","target":{"documentId":"<current exact ID>"},"expectedActiveDocumentId":"<current active exact ID>"}
```

Activation requires the target to be a valid current registry member and the active document to match the caller's exact expected ID. One SDK activation is followed by complete membership/metadata and exact active-object readback. Success data is `activatedDocumentId` and `activeIdentityVerified:true`. No document is selected by name, creation ID or collection position.

## Durability, bounds and shutdown

All three operations run through the existing serialized coordinator: durable admission, dispatch claim, read-only owner-thread preflight, durable execution start, owner-thread revalidation/effect once, and retained result before terminal persistence. SQLite I/O never runs on the SDK thread. HTTP observer timeouts do not abandon the owning coordinator's late result. Exact duplicate requests return history; same operation ID with a different request conflicts. Unknown effects never become replay permission.

There are at most eight owned scratch entries during one runtime lifetime and at most 256 registry members. Closed entries are not silently evicted or recycled. Storage/serialization/capacity bounds from the existing journal and queue still apply. The lifecycle plan contains owned data only; no SDK reference leaves the registry or owning thread.

The command guard checks `SelectCommand` on the owner custom-event thread before effects. This is an observed idle-command check, not proof that every internal Fusion transaction is absent. No Command handler, command termination, event pumping, Undo or text command is used. Actual SDK callback-context behavior remains a live qualification gate.

Native stop releases SDK leases; it never closes user or owned documents. `native-stop.json` includes `documentsAutomaticallyClosed:false` and `ownedScratchAtStop` (a serialized checked-result value). That bounded table describes process-local capabilities and their states; it is not a fresh geometry inventory or restart adoption authority. A failed/unknown creation may have a retained reference without a Ready ownership entry, so a faulted table is explicitly incomplete.

## Stable refusal evidence for the external campaign

| Scenario | Result |
|---|---|
| Box creation | `native_blank_design_only`, before admission |
| Lifecycle operation with old parameter header | `native_lifecycle_contract_required`, before admission |
| Unknown creation operation | `durable_creation_receipt_required`, not executed |
| Other creation's target | `creation_target_mismatch`, not executed |
| Inactive/saved/renamed scratch | `native_lifecycle_preflight_refused`; message `owned scratch document state refused` |
| Stale parameter revision | `native_lifecycle_preflight_refused`; message `target_revision_mismatch` |
| Unexpected active document before activation | `native_lifecycle_preflight_refused`; message `active_document_mismatch` |
| Guard changes between preflight and execution | `lifecycle_baseline_changed` or `owned_scratch_guard_refused`, no invocation |
| Ambiguous close | `owned_scratch_close_unconfirmed`, unknown/reconciliation required |

Builder instrumented SDK tests cover exact identity, copied metadata, replacement, ownership, state/revision guards, preflight-to-effect changes, capacity and serial exhaustion, uncertain creation/close/activation, and reference-only shutdown. Builder coordinator tests use actual SQLite for replay/conflict, late observation, missing persistence, durable proof and unsupported preadmission rejection. They do not qualify the real Autodesk SDK. Root's external HTTP campaign, MSVC build, Windows regression suite and original-document checks are separate acceptance evidence.
