# R10: retained origin-plane sketch creation and inspection

## Lineage and scope

Created from r9 freeze1 product/config/source, without targets, live receipts or root verification directories. `BASELINE-R9-SHA256.txt` records the 76 copied files before r10 changes (the unchanged hidden `.cargo/config.toml` was explicitly added after copy). `BASELINE-R9-PRODUCT-FREEZE1.txt` preserves r9's 79-entry frozen manifest, SHA256 `ae4730b08fa12c9bad86e34888145dcbeffee41378e5a48e75425287a1232af7`. Later concrete r9 fixes require explicit rebasing; they are not automatically inherited. The vendor symlink points to the unchanged r7 offline vendor.

Copied `rust-next-ports/sketch-plan-core` has reviewed source manifest SHA256 `e4b8c3bce1c5c4be0fa6b75635a1a659e52446397dd50ee69c20dd221ea776a6`. Its Rust source and public tests are byte-identical; only removal of the standalone `[workspace]` stanza joins it to this workspace. The already-qualified 24 Linux/24 actual Windows core tests were not rerun merely because of this copy. One integration smoke confirms pure preparation does not execute the SDK. `BASELINE-SKETCH-CORE-SHA256.txt` records its copied input; the expected Cargo-only difference is documented in the local report.

The bounded implementation is **creation and summary inspection**, not the eight-primitive insertion port. `sketch_geometry` is refused before durable admission. Existing `native-parameters-v1`, `native-lifecycle-v1`, and `native-bom-v1` retain their earlier operation gates. Use `X-Adom-Mechanical-Contract: native-sketch-v1` for the two sketch operations; this header also permits the older supported operations.

## Public requests and observations

All examples use the current authenticated native runtime and exact document ID from its document snapshot; an opaque ID cannot be copied between runtime epochs.

1. `{"op":"native_sketch_inspect","target":{"documentId":"<exact document ID>"}}` returns `data.target`, with a `sketch-root-projection-v1:` revision. `data.root` contains active/parametric/hybrid/configuration state, root SDK revision, root sketch count, timeline count and current position. The same full projection is read twice. This is a creation precondition, not complete existing-sketch geometry inspection.
2. `{"op":"sketch_create","sessionId":"<epoch>","operationId":"<new ID>","target":<data.target>,"plane":"xy"}` creates once, after the durable journal executing transition. Plane is `xy`, `xz` or `yz`; omission means `xy`. Supplied `name`, `component`, and reference-object planes are refused before admission, including an empty name. There is no silent name substitution. Creation requires active parametric hybrid/unconfigured design, timeline at end, and observed `SelectCommand` on the owner custom-event thread. The command guard does not prove absence of all Autodesk internal transaction states.
3. A successful create returns `data.sketchId`, `data.target` with `sketch-projection-v1:`, `data.sketch`, `created.sketchId`, numeric `created.plane`, and `rootTarget` observed after creation. `legacyParity:false`, `rollbackAttempted:false`, `complete:false`, and `geometryEnumerated:false` make scope explicit. `data.sketch` exposes `name`, `sdkRevision`, `plane`, 16-value native `transform`, counts `curves/points/profiles/dimensions/constraints`, `health`, and `deferred`. Plane codes are 1=XY, 2=XZ, 3=YZ, 0=another observed plane; creation success requires the requested origin plane. `transform` is the SDK parent-component-to-sketch-space matrix, with Fusion database length units (cm), not a claimed world/occurrence transform. Creation verifies zero curves/profiles/dimensions/constraints, healthy/nondeferred state, and root sketch count increasing by exactly one. Native points are reported, not assumed zero: Autodesk creates an origin point.
4. `{"op":"native_sketch_inspect","target":{"documentId":"<same document ID>"},"sketchId":"<returned ID>"}` reads that exact retained SDK sketch. A matching `target.revision` may be supplied to reject changed sketches. Without `sketchId`, the optional revision instead guards the root projection. No name, entity token, collection position, active sketch or list difference substitutes for identity.

Both read projections state `complete:false`. Sketch projection hashes the opaque identity, exact document ID, SDK revision and observed summary. The SDK revision is a content-change witness; names and counters alone are never treated as the full sketch revision. Neither revision is claimed to be a geometry-wide document revision or a transactional lock.

Stable refusals include `native_sketch_contract_required`, `native_sketch_exact_target_required`, `native_sketch_root_default_name_only`, `native_sketch_origin_plane_required`, `native_sketch_root_revision_required`, `native_sketch_revision_required`, `native_sketch_context_refused`, `target_revision_mismatch`, `native_sketch_capacity`, `native_sketch_id_unknown`, and `native_sketch_id_retired`. SDK read failures are explicit errors. An observed invalidity, membership/read/projection failure permanently retires that sketch ID, even if a later SDK call would appear valid. Uncertain creation faults further sketch work in the session and remains an operation-journal unknown; do not replay it with a new operation ID.

## Ownership and failure boundaries

Rust reserves a lifetime slot and sets acquisition uncertainty before calling create. The C++ holder is published before `Sketches::add`; it independently retains exact `Ptr<Document>`, `Ptr<Component>` and the returned `Ptr<Sketch>`. It never borrows a `DocumentReference` that document enumeration could release. The returned holder is adopted before any metadata/readback operation. A null return, SDK error or missing acknowledgment after invocation is unknown, not proof that no effect happened. A panic or impossible missing holder keeps teardown fenced.

Each sketch read checks thread, exact supplied document and root equality, valid sketch, parent component equality, and membership by comparing the retained SDK object against root collection members. The collection index is only enumeration, never a reference selector. Two complete sketch summaries must agree. A bounded recheck after create observes actual state; there is no automatic rollback, delete, retry or name-based recovery.

Slots are capped at 64 for the runtime lifetime and never evicted/reused, including failed creates. Root membership enumeration is capped at 512 sketches; timeline at 4096; each summary count at 16384; name/revision strings at 4096 bytes and serialized projection at 32768 bytes. IDs include runtime document identity plus a monotonically assigned book serial, and are not persistent references across reload.

Before parent document references are released, `Context::prepare_release` closes the sketch book explicitly on the owner thread. Each holder release is attempted once. A reported release error or panic leaves the book uncertain; another close refuses without retry, and `Context::close` independently refuses while sketch refs remain. Existing native cleanup retains callback/module on that failure. No destructor invokes sketch SDK cleanup. Successful stop releases references only; it does **not** delete sketches or close documents.

The inherited owned-scratch discard remains **parameter-only** and rejects sketches. Root's geometry fixture owns test cleanup; the product does not broaden discard to make a test pass. Actual Fusion test failure requires preserving receipts and exact retained fixture ownership, with no blind second create or arbitrary cleanup.

## Next eight-primitive insertion slice

Use the copied immutable planner only after validating the whole request, current exact sketch/document/root, supported 2D mode, every point's plane compatibility, budgets and fresh sketch revision. Never silently project 3D input into a 2D sketch. Reserve bounded result/ownership capacity and journal start before the first entity effect. Revalidate the full plan at the SDK hop. Preserve primitive intent, especially rectangle constraints and spline closure; four lines do not establish legacy rectangle-helper parity.

| Prepared primitive | Installed typed SDK call candidates | Required native observation before success |
| --- | --- | --- |
| Point | `SketchPoints::add` | Exact returned point; sketch-space coordinates, validity and parent. |
| Line | `SketchLines::addByTwoPoints` | Exact start/end, geometry type, construction flag, parent. |
| Rectangle | `SketchLines::addTwoPointRectangle` or explicitly versioned four-line behavior | All four exact returned lines, closure, constraints introduced by the helper; no false helper parity. |
| Circle | `SketchCircles::addByCenterRadius` | Center/radius, construction and parent; independent profile observation. |
| Arc | `SketchArcs::addByCenterStartSweep` | Center, start/end, signed sweep/orientation and construction in actual sketch coordinates. |
| Polygon | Prepared ordered lines | All actual segments, shared endpoints/closure and construction; applied prefix if interrupted. |
| Slot | Two lines/two signed semicircle arcs | Actual widths, tangency/endpoints/closure, orientation and construction; no fabricated slot constraint. |
| Fitted spline | `SketchFittedSplines::add`, qualified closure setter | Actual fit points/closed state/curve geometry, solver health, construction; planner points are not an observed spline. |

A later primitive or setter failure must retain actual observed prefix/unknown result and forbid automatic replay; do not invent atomic rollback. Coordinate-only readback does not prove constraints or profile solvability. Tests need signed arcs, non-axis slots, transformed XY/XZ/YZ frames, construction flags, coincident endpoints, rectangle helper effects, spline closure/degeneracy and solver refusals, deletion/undo/retirement, plan-wide refusal before effects, and late-result/durable-commit failures. Root should qualify them in an exact-owned native fixture with independent geometry oracles, not merely by comparing against the planner's own predicted numbers.

## SDK evidence and qualification limits

Installed Autodesk C++ headers were inspected directly. Autodesk documents [Sketch.revisionId](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_Sketch_revisionId.htm) as changing with sketch changes, [Sketches.add](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_Sketches_add.htm) as returning the created object or null, and [Sketch.originPoint](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_Sketch_originPoint.htm) as automatically projected. [Sketch.transform](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_Sketch_transform.htm) defines parent-component-to-sketch coordinates; its text restricts parametric position changes. This slice only reads transform; actual parametric getter behavior and origin-plane matrices still require live qualification.

Author tests and compile checks do not establish actual Fusion solver, API callback, GUI transaction, unload, frame or geometry behavior. Windows MSVC SDK build, exact-source independent review, normal live guarded creation/inspection/replay/conflict/cleanup and teardown gates remain root-owned. No public release or full legacy parity is claimed.
