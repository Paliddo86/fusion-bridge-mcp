# r9 native BOM and CSV contract

## Requests and provenance

Use authenticated `POST /v1/mechanical` with the current Fusion epoch and `X-Adom-Mechanical-Contract: native-bom-v1`. Older parameter/lifecycle headers still authorize their existing subsets; they refuse `bom`/`bom_export` with `native_bom_contract_required` before admission.

Initial read:

```json
{"op":"bom","target":{"documentId":"<exact current document ID>"},"explodePurchased":false}
```

The target must be the active current registry member. A successful response has `success:true` and `data` containing the reviewed core BOM fields: target, configuration, configurationSuppression, parts, instances, excluded, partCount, totalPurchasingInstances, physical, units and calculationAccuracy. It adds `revisionScope`, `nativeScope` and `legacyParity:false`. `complete:true` is issued only after every supported field and collection has been observed and two full passes agree. It is not a claim of full configured/external-design support or complete geometry inspection.

The returned target revision begins `bom-projection-v1:`. It fingerprints the complete collected BOM projection, including component revisions, metadata/materials, instance paths/visibility/suppression evidence, definition bodies and occurrence-context body properties. It does not certify arbitrary geometry, camera, document state or a global Fusion transaction. A later read may supply this revision to reject a changed projection. A parameter-projection revision is not interchangeable with a BOM revision.

Component IDs group retained exact SDK component objects within one successful capture. Occurrence IDs identify exact retained context occurrences within that capture. Each capture receives a fresh nonce: IDs are intentionally not stable across separate BOM calls. Entity tokens remain opaque references and are never grouping keys. Reordering can conservatively change the projection revision. Equal names, part numbers, metadata or token strings do not merge separate definitions.

CSV export:

```json
{"op":"bom_export","sessionId":"<current epoch>","operationId":"export-1","target":{"documentId":"<exact ID>","revision":"<fresh bom-projection-v1 revision>"},"filename":"<absolute host path>.csv","mode":"flat","overwrite":false,"explodePurchased":false}
```

Modes are `flat` (default) and `structured`. The path must be absolute and end in `.csv`; overwrite defaults false. The preflight captures the exact supported snapshot and generates bounded CSV bytes. The coordinator durably records execution start before one publication on its worker. It does not hold Fusion's model locked until file commit: the output represents that immutable capture, with explicit document/revision/configuration provenance. A deadline observed before publication prevents the file call; a started publication is observed to completion even if the HTTP observer times out.

Success `data` includes `target`, `partCount`, `mode`, `configuration`, `calculationAccuracy`, `outputPath`, `fileSize`, raw-byte SHA-256 `sha256`, `published:true`, `overwrite`, `cleanupWarning`, `snapshotExport:true`, `modelMutation:false` and `powerLossDurabilityGuaranteed:false`. Existing artifact publication performs same-parent private staging and one exclusive hardlink or explicit replacement rename. Commit-call uncertainty is unknown/reconciliation-required, never automatic retry. Earlier filesystem failures can leave diagnostic staging/created parents, but do not imply a committed output. Cleanup warnings never erase an acknowledged publication.

The retained journal distinguishes durable terminal results from nondurable observation/unknown. Duplicate exact operation requests return recorded history without recollecting or republishing. If an external user later changes/deletes a published file, replay still reports the historical operation result and does not repair the file. Same operation ID with another request conflicts. No BOM operation mutates the model.

## Supported native envelope

| Evidence | Required observation |
|---|---|
| Document/design identity | Current retained document, exact parent Design, active document; repeated Design checks while view is open |
| Design configuration | `isConfiguredDesign == false`, `isConfiguration == false`, no top configuration table and empty configuration row ID; all getters successful |
| Design intent/timeline | Hybrid; timeline marker at end; at most 1024 timeline entries |
| Definitions | At most 128, fully reachable; count equals `Design.allComponents.count`; grouping by retained same-static-type SDK `Ptr` equality |
| Occurrences | At most 512 context occurrences; depth at most 32; full paths unique; no external/configured/suppressed/rolled-back occurrence |
| Bodies | Solid BRep only; no mesh bodies; at most 2048 combined definition/context body reads per pass; high-accuracy getter and returned accuracy both checked |
| Metadata | Native name/partNumber/description/material ID/name; only `AdomMechanical` attributes enter the BOM; at most 128 total native attributes per definition, 16KiB per text field and cumulative text bounds |
| Snapshot | Two full passes agree on every captured value and exact object identity; no partial result on getter failure, replacement, changed collection or inconsistent ownership |
| Serialization | Projection, BOM output and CSV each at most 512KiB; existing request/queue/journal limits also apply |

Known `AdomMechanical` values are JSON strings for supplier, supplierPartNumber, sourceUrl, sourceFile and sha256, and JSON booleans for asPurchased/excludeFromBom. Unknown names in that group remain raw strings. Other groups do not affect this BOM projection; excessive native metadata still refuses conservatively.

Flat unit mass/volume uses definition-context direct bodies unless a purchased component collapses its subtree, in which case `Component.getPhysicalProperties(High)` is required. Physical totals independently walk `Occurrence.bRepBodies` in each actual assembly context, plus root direct bodies. Visibility does not remove quantity or physical mass. Purchasing exclusions and purchased collapse do not remove physical totals. Mass is kilograms, native cubic centimeters convert to cubic millimeters, and nonfinite/negative properties refuse. The assigned/default modeled material is the physical basis; supplier metadata does not certify density.

The C++ view retains every returned component/occurrence lease for both passes, up to 4096 handles. Rust owns traversal and identity grouping. A single-use child-lease token fences parent teardown before acquisition. Explicit view close releases references once on the SDK owner thread on normal completion and Rust unwinding. A failed/panicked release permanently fences the document/context, faults the callback and prevents clean teardown or module unload; no release is retried. Registry shutdown checks this fence before releasing borrowed document references. No SDK lease crosses into the worker or a CSV plan. Object equality uses held references, never recycled raw addresses or token-string comparisons. Real nested proxy equality remains an actual-app qualification gate; a mismatch safely refuses rather than guessing equivalence.

## Official SDK source used

Installed headers under Autodesk Fusion's `API/CPP/include` supplied the typed method signatures and semantics:

- `Fusion/Fusion/Design.h`: rootComponent/allComponents, configuration flags/table/row, designIntent, timeline and parentDocument.
- `Fusion/Components/Component.h` and `BaseComponent.h`: metadata, parentDesign, revisionId, entityToken, bodies/material, High physical properties and root occurrences. `revisionId` covers a component, not a complete Design.
- `Fusion/Components/Occurrence.h`: component, context-specific childOccurrences/bRepBodies, fullPathName, timelineObject, external/configuration flags, visibility and entityToken. Its token documentation explicitly warns that token text is not object identity.
- `Fusion/BRep/BRepBody.h` and `Fusion/Fusion/PhysicalProperties.h`: solid-body status, requested/returned calculation accuracy, mass and volume units.
- `Core/Application/Attributes.h`, `Attribute.h`, `Core/Materials/Material.h`: typed metadata enumeration, group/name/value and material ID/name.
- `Core/Memory.h` and the existing reviewed retained-document shim establish held same-static-type `Ptr` comparison. Native equality is still checked in the actual fixture; no inferred cross-capture identity is claimed.

All SDK strings and getter failures remain checked. There is no active-product fallback, console/Python execution, command launch, event pumping, geometry mutation or hidden recompute in the collector.

## Remaining full-port coverage

Configured designs/table themes/custom columns/active-row suppression, inserted configurations, external/referenced components, suppressed or rolled-back timelines, mesh/surface physical scope, assemblies beyond the stated bounds, stable cross-capture entity handles and complete geometry revisions remain unsupported here. The reviewed Rust core has broader configuration logic, but that is not credited as native collector support. General STEP/STL/F3D export, model transactions/rollback, sketch/feature construction, BOM UI and public installation remain separate ports.

Builder tests use instrumented SDK observations with hand-computed nested/repeated quantities and definition-versus-context masses, plus actual SQLite/filesystem export cases. They do not establish Autodesk getter correctness or native ABI/runtime parity. The separate fixture/campaign must validate a nonempty nested assembly, copied metadata, purchased/excluded/hidden cases, both CSV modes and exact cleanup, with original documents preserved.
