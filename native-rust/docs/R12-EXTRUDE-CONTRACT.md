# Native finite extrusion contract

The `native-extrude-v1` header adds `native_sketch_profiles` and the explicitly narrowed `feature_create` extrude branch. Earlier contract headers retain their existing operations; these two operations require the new header before admission. This is not full legacy feature or transactional parity.

## Explicit profile discovery and request

Read an exact retained sketch created in this native session:

```json
{"op":"native_sketch_profiles","sessionId":"<runtime epoch>","target":{"documentId":"<exact document ID>"},"sketchId":"<returned sketch ID>"}
```

The successful result's `data.target` contains a `profile-projection-v1:` revision. `data.profiles[0].reference` contains exactly `documentId`, `revision`, `objectType:"Profile"`, `sketchId`, and `profileId`. Its sibling `observed` contains `areaCm2`, `perimeterCm`, `centroidCm`, loop/curve/root-body/root-extrude counts and on-plane/outer-only/high-accuracy flags. `data.root`, `data.sketch`, `revisionScope` and `complete:false` explain the limited projection. An optional read target revision must match that profile projection.

Pass that explicit reference and target unchanged:

```json
{"op":"feature_create","sessionId":"<runtime epoch>","operationId":"extrude-1","target":{"documentId":"<exact document ID>","revision":"<profile-projection-v1 digest>"},"type":"extrude","profiles":[{"documentId":"<exact document ID>","revision":"<same digest>","objectType":"Profile","sketchId":"<returned sketch ID>","profileId":"<published profile ID>"}],"distance":20,"operation":"new_body","direction":"positive"}
```

Distance is a finite number of millimeters in [0.0001, 100000], converted once to SDK centimeters. Direction is positive or negative; default positive. Operation defaults to new_body. Expressions, explicit taper (including zero), names, components, participant bodies, symmetry, booleans and other feature kinds refuse before admission. A profile must have been explicitly published by the current book; a guessed formatted ID does not authorize it.

## Initial model boundary

Only current exact retained root XY/XZ/YZ sketches, one on-plane closed profile with exactly one outer loop and 1–256 profile curves, healthy/nondeferred sketch, active local unconfigured parametric hybrid design, timeline at end, and empty root body/extrude collections. Area [1e-10,1e8] cm², perimeter [1e-5,1e6] cm, and finite centroid coordinates within 10000 cm are bounded. Holes, multiple profiles, arbitrary existing-sketch adoption and non-root/proxy contexts refuse. This slice permits at most one new body in the root; no new-body accumulation/general reference resolver is implied.

Two complete observations must agree. The revision includes exact session IDs, the parent root/sketch SDK revisions and bounded profile properties/counts; it is not a complete geometry revision. Exact retained SDK equality remains identity authority. Collection item(0) is used only after sole-member validation and then compared with the retained profile on every read; it never substitutes for an explicit published selection. A failed profile membership/read permanently retires that profile from authoring.

## Execution, observation and ownership

The Rust coordinator validates/preflights, durably records executing, then dispatches one owned plan. The owner-thread execution revalidates before calling `ExtrudeFeatures.add` once. Transient input preparation does not count as model execution. An invoked call with absent acknowledgment, a panic or failed postcondition is unknown, including when a body may exist. The book remains faulted and no automatic retry/rollback occurs. The existing retained journal preserves bounded observations before terminal persistence; timeout or lost persistence cannot authorize another effect.

The typed holder stores the exact returned feature before any fallible readback, then its exact sole body. Profile, feature and body references live below the existing retained sketch/parent lease. Once-only release uncertainty blocks successful parent/context/module teardown. No name, entity-token text, list difference or active-document fallback creates ownership. No implicit cleanup of CAD geometry is introduced.

Success returns `data.documentId/sketchId/profileId/featureId/bodyId`, `observed` solid properties, `volumeMm3`, `expectedPrismaticVolumeMm3`, numeric `distanceMm`, direction, `complete:false`, plus `rootTarget`. Feature/body IDs are current-session receipt identifiers backed by exact retained objects; they are not advertised as general topology references. Unknown results retain `observedSolid` when acquired, `failedStage` when available, reconciliationRequired and automaticReplayAllowed:false.

Readback checks exact root/feature/body/profile membership, healthy feature, solid body/feature, new-body one-side profile-plane start, zero taper and actual distance. High-accuracy volume must match input area×distance within 0.3% plus 1e-8 cm³. Actual center of mass is transformed by `Sketch.modelToSketchSpace`; Z must match signed distance/2 within 0.3% of distance plus 1e-6 cm, and X/Y must match profile centroid within 0.003×sqrt(profile area) plus 1e-6 cm. The last tolerance is a finite geometric placement policy, not a documented SDK centroid-error guarantee. Final parent sketch shape/frame/counts/health and profile geometry must remain unchanged (parent SDK revision may advance), root timeline grows exactly one, root sketch count stays fixed, root revision advances, and exactly one root body/extrude is observed. Independent final rereads must agree.

## SDK error diagnostics

Extrusion-specific SDK calls consume their existing immediate `getLastError(&description)` observation exactly once. Rust exposes bounded `sdkDiagnostic` with ABI status, numeric stage, original SDK error and at most 4096 diagnostic bytes. Diagnostic text alone may decode lossily; malformed/truncated optional text does not erase numeric authority. Model text remains strict. Stage 100–123 covers profile properties, 200–208 transient inputs, 209 feature add, 210–212 returned body capture, and 300–331 solid readback. Inherited shared root/sketch validation still exposes generic status and explicitly marks the original SDK error unavailable; no second error read/reset/retry fabricates it.

## SDK basis and remaining tests

Installed Autodesk C++ headers define Profile, ProfileLoops, AreaProperties, ExtrudeFeatures/Input/Feature, DistanceExtentDefinition, ProfilePlaneStartDefinition and PhysicalProperties. Official [AreaProperties.centroid](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_AreaProperties_centroid.htm) documents profile centroids relative to sketch origin; [Sketch.modelToSketchSpace](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_Sketch_modelToSketchSpace.htm) supplies the explicit model-to-sketch conversion. HighCalculationAccuracy is documented in the installed FusionTypeDefs header as ±0.1% for physical-property calculations; the paired area/volume tolerance is conservative policy. No direct-coordinate assumption substitutes for the transform.

Legacy `commands/mechanical/features.py` additionally supports multiple profiles, expressions, symmetric/tapered extents, join/cut/intersect participant selection, feature naming and other feature kinds under legacy transaction/recovery orchestration. These remain deferred. Profile identity stability after real SDK recompute, actual positive/negative extrusion on all planes, normalization, health and physical accuracy need source-bound Windows/native SDK and actual Fusion qualification. Author fake-backend tests establish control flow, fault handling and arithmetic only. The independent live geometry fixture/client, exact original roster preservation, product stop/unload and owned cleanup remain separate root-owned gates. No release is claimed here.


## Profile getter barrier (fix2)

Two actual read-only traces on Fusion2705.1.30 showed that reading ExtrudeFeature.profile advances the parent component revision while body/sketch revisions and all measured geometry remain stable. This is an observed SDK behavior; the official API describes a getter/setter but does not document this revision side effect. Evidence is preserved in ../rust-extrude-observer-r3/evidence/live1.

After an acknowledged feature add, Rust now calls a separate once-only typed profile relationship verification. The native holder consumes its attempt latch before the getter and confirms only exact retained feature/profile equality with successful immediate SDK error checks. Any mismatch/error/panic remains unknown; no later readback, retry, adoption or rollback follows. This confirmation records one observation on immutable held objects; it is not advertised as fresh relationship validation on each solid read.

Immediately after that barrier, Rust captures the root freshness baseline. All subsequent solid, profile and sketch reads remain strictly checked, including four physical solid captures. The final root comparison now runs last, guarding the entire repeated read sequence. Root revision equality, exact retained membership, physical tolerances and input/current geometry comparisons remain unchanged. No cached physical-property substitute or dropped root-revision check is introduced. Stage304 remains the profile getter diagnostic; no error reset or retry is used.

Final projection failures also retain the decisive expected/observed values or getter error without an extra diagnostic SDK read. This does not reconstruct the historical failed operation: the old journal and unknown outcome remain untouched.

M12-fix2 source checks: 361 Linux workspace tests (184 native + 2 native compile-fail docs), GNU Windows bindings/tests, adapted installed-SDK syntax and formatting passed. Independent source review precedes the frozen Windows build. Actual fresh-model qualification and clean teardown remain required.


R15 propagation: the above source correction was merged into R15 freeze2 with the newer exact-entity/material/McMaster APIs preserved. Parent authorized source integration before the changing-roster actual-app gate; no new actual-app success or retroactive unknown-operation reconciliation is claimed.
