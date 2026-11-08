# R19: one driving length dimension

This checkpoint adds `native_sketch_dimensions` and a finite implementation of `sketch_dimension` under `X-Adom-Mechanical-Contract: native-sketch-dimensions-v1`. All prior operations and headers remain available. The new header also authorizes the earlier operations. No installed/public release or actual Fusion qualification is implied by source tests.

The native contract supports one exact retained isolated line on one root origin-plane sketch, already bearing the one native returned horizontal or vertical constraint. It requires an active local unconfigured parametric hybrid design, one component, no bodies/features/occurrences/meshes, exactly one sketch/timeline item, one line, its distinct start/end/origin points, and no text/attributes/foreign relations. Line and endpoints must be unfixed and not fully constrained. Existing dimension/model/user parameter collections must be empty before adding the dimension. It does not adopt existing graph objects.

Read `native_sketch_dimensions` with the current session, exact document ID, sketch ID and published line entity ID. A successful two-pass read publishes two ordered `endpointRefs` (`start`, `end`), the `sketch-dimensions-projection-v1` target revision, actual root/sketch/graph values, and zero or one exact returned dimension. Endpoint publication is a required runtime fact: guessed IDs cannot enable mutation. After dimension creation use this read, not the older zero-dimension line-state/relation projections. Those older scopes remain strict and can retire an unsupported graph's entity publication.

Example mutation structure (substitute the exact returned fields):

```json
{
  "op": "sketch_dimension",
  "sessionId": "current-runtime-epoch",
  "operationId": "fresh-operation-id",
  "target": {"documentId": "exact-document-id", "revision": "returned-dimension-projection"},
  "sketch": {"documentId": "exact-document-id", "revision": "returned-dimension-projection", "objectType": "Sketch", "sketchId": "exact-sketch-id"},
  "type": "distance",
  "entities": ["replace with returned start ref object", "replace with returned end ref object"],
  "expression": 42,
  "position": [20, 30]
}
```

Despite the legacy field name `expression`, this contract accepts only a finite numeric length in millimeters, from 0.001 through 10000 mm. Position has two or three finite millimeter coordinates bounded to ±100000 mm; Z must be zero. Only aligned distance is implemented, not horizontal/vertical dimension variants, angles, radii or diameters. `parameterName` and string expressions are refused before journal admission. Sketch and endpoint references must match the exact target revision and ordered retained endpoint identities.

The coordinator admits durably, runs the complete preflight, durably records executing, then revalidates that entire preflight on the SDK thread. The typed adapter invokes `addDistanceDimension(..., AlignedDimensionOrientation, ..., true)` once, retaining the exact returned `SketchLinearDimension` and its `ModelParameter` immediately. It then sets the driving dimension's `value` once in internal centimeters; Autodesk documents this setter as equivalent to setting its associated driving parameter value. Rust records `stages.dimensionAdd` and `stages.parameterValueSet` separately. It never sets a name or expression and never promises a unique solver placement or preserved line length before the requested solve.

Curve incidence is derived from the complete sole-line/three-point/zero-text inventory, as reviewed in the R18-fix2 propagation. The nullable `connectedEntities` getter is not called or normalized. This proof remains valid with a dimension because that API explicitly excludes constraint relationships; dimensions are separately checked through exact retained membership.

Each confirmed capture checks exact sketch/dimension/endpoint/parameter parent and collection membership, aligned/driving state, model parameter ownership/creator, and that no other parameter exists. The current dimension value, parameter value and actual endpoint distance must agree, with finite numeric tolerances (1e-8 cm plus 1e-10 relative). The final value must match the requested numeric length. Two full observations must otherwise be exactly equal; root and sketch state remain guarded. Original origin facts and H/V relation kind must remain unchanged. Readback lengths/positions are explicitly centimeters. Parameter name/expression/unit are observed bounded strings, not assigned values. Text placement is observed after the solver; no claim that its final position equals the requested initial placement.

A failure before the add call is known not executed. Any ambiguity after add invocation, including parameter retention, setter, command guard, getter panic or failed readback, remains unknown and faults further authoring. The result retains the completed/attempted stage prefix and any fully captured post-effect projection. A returned but incompletely acknowledged child remains retained in its fixed slot beneath the exact sketch; no rollback, delete, implicit replay or arbitrary retarget occurs. Lost terminal persistence and HTTP observer deadlines use the existing retained journal semantics. Normal teardown releases parameter/dimension holders before the relation/endpoints/sketch/document; existing release uncertainty fences remain in force.

This is not full legacy `sketch_dimension` parity: formulas, renaming, arbitrary graphs, other dimension types, imported/existing dimensions, repeated edits of a dimension and automatic recovery are not implemented. Source/fake tests cannot qualify Fusion solver behavior or typed wrapper equality. A future live campaign must create a fresh isolated H/V line, inspect the endpoint refs, add a driving length, independently verify the actual solver geometry, and archive/reopen before any owned task retirement.

Installed SDK headers inspected: `SketchDimensions.h`, `SketchLinearDimension.h`, `SketchDimension.h`, `ModelParameter.h`, `SketchPoint.h`, and `SketchEntity.h`. Official references: [addDistanceDimension](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_SketchDimensions_addDistanceDimension.htm), [SketchDimension.value](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_SketchDimension_value.htm). No inferred full transaction or global model revision is advertised.
