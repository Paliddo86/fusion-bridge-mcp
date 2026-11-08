# r11 native sketch geometry contract

This checkpoint ports eight prepared sketch primitives into retained root origin-plane sketches. It is a candidate implementation pending independent source review, Windows execution and actual Fusion qualification. All plan preparation, unit conversion, validation, identity policy, result verification and journaling are Rust. C++ is a typed Autodesk ABI adapter. No Python is used.

The baseline is the exact 92-file r10 product freeze2, manifest SHA256 `39f5af6c227b7ba21c2d06139dcb5442cd6514592ecc321b5b55eabd1c37f7dc`. `BASELINE-R10-PRODUCT-FREEZE2.txt` and `evidence/baseline-verification.log` record the copy. Targets, live receipts and verification clients were not copied; the immutable vendor tree is shared by symlink. Later r9/r10 SDK getter fixes require explicit source-bound propagation; this branch does not infer or relax failing getter semantics.

## Request and scope

Use `X-Adom-Mechanical-Contract: native-sketch-geometry-v1`. Earlier native parameter, lifecycle, BOM and sketch headers remain accepted for their own operations; earlier headers cannot authorize geometry. The native capability schema overrides the legacy transactional description. The legacy generic schema remains shared, but native preadmission validation imposes this narrower contract.

```json
{
  "op": "sketch_geometry",
  "sessionId": "<current epoch>",
  "operationId": "geometry-1",
  "target": {"documentId": "<exact native document ID>", "revision": "sketch-projection-v1:<digest>"},
  "sketch": {
    "documentId": "<same exact native document ID>",
    "revision": "sketch-projection-v1:<same digest>",
    "objectType": "Sketch",
    "sketchId": "<exact opaque ID returned by native sketch_create>"
  },
  "items": [{"type":"line", "start":[0,0], "end":[20,10], "construction":false}]
}
```

The sketch must be one currently retained by this runtime's `Book`, with exact parent-document/root/current collection membership. No name, entityToken, collection index, active-sketch lookup or list-difference inference adopts an object. The native opaque sketch ID is required; target and sketch document/revision witnesses must agree. It must currently contain no curves, profiles, constraints, dimensions or text; its point collection must be empty or contain only the exact SDK origin point. No prior geometry leases may exist. The root design must be active, local unconfigured parametric/hybrid with the timeline at its end and observed `SelectCommand`. The command check is not proof that every possible internal transaction is absent.

Only the XY/XZ/YZ root origin planes are supported. Coordinates are sketch-space millimeters with exactly zero Z; they are converted to centimeters before the ABI. This does not apply a model-to-sketch transform. Origin-plane/frame witnesses remain in scoped inspection; existing native geometry inspection still reports `geometryEnumerated:false` and `complete:false`.

Limits are 32 input items, 64 expanded entities, 512 stored prepared points and 64 KiB serialized prepared plan. Each fitted spline has the core's 128-point limit. Coordinates and radii are bounded to 10,000 cm; minimum relevant spans/radii are 1e-5 cm. Arcs must have a nonzero sweep strictly below a full turn (margin 1e-8 rad). The entire plan is parsed/prepared/bounded before admission and again on the owner thread before execution authorization. Invalid later items therefore cannot leave a valid earlier prefix behind.

| Input | Typed SDK effects | Actual readback |
| --- | --- | --- |
| point | `SketchPoints::add` | exact returned point membership/parent and sketch-space coordinates |
| line | `SketchLines::addByTwoPoints` | start/end coordinates, construction |
| rectangle | four prepared line additions | every edge's ordered endpoints; no rectangle-helper constraint parity |
| circle | `SketchCircles::addByCenterRadius` | center, radius, sketch-space normal, construction |
| arc | `SketchArcs::addByCenterStartSweep` | center, start/end, positive sweep and normal; negative requested sweeps require reversed actual endpoints |
| polygon | prepared ordered lines | each edge and closure endpoints; no polygon solver constraints |
| slot | two lines and two arcs | every exact edge, reversed negative arc convention, construction; no tangent constraints promised |
| spline | `SketchFittedSplines::add`, optional `isClosed(true)` | ordered fit points, actual periodic-closure flag, construction; no NURBS knots/interpolation/shape guarantee |

All scalar/frame/coordinate values must be finite. Readback tolerance is `1e-7 cm + 1e-12 * max(abs(expected),abs(observed))` for coordinates/scalars; arc sweeps use the same numerical absolute threshold in radians. Geometry checks compare each exact returned reference twice, then all entities again after the complete plan. Final sketch/root projections must remain stable around those last reads. Curve count must equal the number created; profiles and point/constraint counts are observed, not predicted. Health must be healthy, compute must not be deferred, dimensions must remain absent, and the documented SDK sketch revision must change. No implicit constraint or profile topology/area claim is made.

## Effects, failures and lifetime

The existing owned coordinator first reserves journal/observation capacity, durably admits and dispatches, obtains an owned data-only preflight plan, durably marks executing, and only then queues the SDK effect. Execution repeats exact membership, empty-sketch, root and revision checks. No mutex spans SDK calls or journal I/O. HTTP timeout does not cancel an already claimed effect or discard its late result.

Insertion is **non-atomic**. Each primitive may expand into multiple SDK additions, and construction/periodic-closure assignments are separate effects after addition. The C++ adapter reserves a fixed parent-owned slot before invoking an add and immediately retains its exact returned entity there. The holder never borrows a collection item as identity. All returned references remain beneath the retained sketch until explicit parent-before-document/context teardown. Failed or panicking release permanently blocks successful unload; no implicit Drop or retry releases uncertain SDK references.

A successful result includes `data` (new sketch projection), `rootTarget`, `geometry.itemsCompleted`, `geometry.entitiesCompleted`, and `geometry.observed`, with input index, entity index, primitive and actual centimeter/radian observation. `atomic:false`, `legacyParity:false` and `rollbackAttempted:false` are explicit.

A failure after any possible effect is unknown, not a normal transaction failure. It retains `completedEntities` and the failed attempt's input/entity index, invocation acknowledgment and stage when known. This prefix is an observation made before later effects; the final solver state may differ, and the result says so. SDK insertion panics preserve the observed prefix too. No blind retry, rollback or implicit deletion occurs. The sketch book is faulted after unknown and refuses further authoring/inspection through that book; status remains available and external exact-object reconciliation is a separate root-owned qualification step. Another request with the same operation ID replays the stored/retained status without dispatch; a different digest conflicts. Lost terminal persistence preserves nondurable observed results, never granting replay permission. Process loss follows existing journal unknown/interrupted semantics.

User-document close/cleanup remains unchanged: the product's owned scratch discard still refuses geometric content. Test fixture cleanup must independently qualify the exact owned document and all actual outcomes after the product stops/unloads. This checkpoint introduces no cleanup route.

## Unsupported and remaining qualification

No insertion into nonempty/arbitrary/sketch-proxy/component-context sketches; no text, solver constraints, dimensions, edit/delete, entity-reference publication, extrusion, solver rollback, automatic repair, batching atomicity or legacy geometry parity. Closed-spline SDK normalization, implicit coincident/solver behavior, native profiles and setter acknowledgments must be exercised in Fusion before release. Actual MSVC ABI linking and live SDK reference/module teardown are root-owned gates. The adapted MinGW SDK syntax check and Rust Windows binding compilation do not prove those gates.

Primary API evidence is the installed Autodesk C++ headers: `Fusion/Sketch/SketchArcs.h` documents counterclockwise returned arcs even for negative sweep; `SketchFittedSpline.h` documents ordered fit points and periodic `isClosed`; `Sketch.h` documents model-to-sketch transform direction and sketch revision; `SketchEntity.h` supplies exact parent and reference flags. `Core/Geometry/{Line3D,Circle3D,Arc3D,Point3D}.h` provides sketch-space transient readback getters used here. SDK calls check errors immediately; no undocumented configuration/null-getter exception is assumed.
