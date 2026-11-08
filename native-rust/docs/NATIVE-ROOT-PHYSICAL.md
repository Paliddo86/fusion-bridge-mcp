# Native root physical-property projection

R16 adds the read-only `assembly_properties` subset under
`X-Adom-Mechanical-Contract: native-root-physical-v1`. It requires current
`sessionId` and an exact published current-document ID. `target.revision` is
optional initially and may guard the returned `root-physical-projection-v1:...`.
Explicit `entity` (including null), unknown target fields and mismatched sessions
are refused before queue/backend work. The inherited typed selector gate refuses
malformed, foreign, never-published and retired IDs before this backend. Existing
R15 headers and all previous operation behaviors remain; the new header is also
accepted for those operations.

Example body (substitute actual runtime values):

```json
{"op":"assembly_properties","sessionId":"<epoch>","target":{"documentId":"<exact opaque document ID>"}}
```

Finite scope is a hybrid, unconfigured Design's exact local root component with
no occurrences, mesh bodies, non-solid/transient/proxy bodies or arbitrary entity
selection. Each typed owner-thread call retains at most128 native bodies in a
fixed local array, verifies exact parents, uniqueness, full ordered membership
and validity, counts at most20000 total faces, and only then asks the root for
HighCalculationAccuracy properties. No SDK pointer crosses the ABI or worker
boundary and no lasting SDK holder is added. Every retained body and root is
rechecked against fresh exact document/design/root membership after calculation;
counts, solid/native/parent facts and design scope are rechecked. Root revision
is sampled before all those reads and after the final check.

Rust requires stable nonempty root revisions and two equal full observations.
It validates numeric finiteness/signs and checked conversion before JSON output:
mass kg, volume cm3×1000→mm3, area cm2×100→mm2, center cm×10→mm. Average density is
observed mass divided by converted volume (kg/mm3); it is null only for observed
zero volume. The empty root is not fabricated: SDK failure/null physical results
remain failures. Nonempty roots require positive volume/area. Supplier labels or
BOM provenance do not establish actual assigned/default material density.

Returned data contains target/revision, revisionScope, mass, volume, area,
centerOfMass, averageDensity, units, bodyCount, calculationAccuracy, sdkAccuracyCode,
materialBasis, and explicit assemblyOccurrencesSupported:false,
arbitraryEntitySelectionSupported:false, legacyParity:false. This is not full
assembly-property parity. The revision is this bounded physical projection, not
a whole-document geometry/material change token. It does not qualify unsupported
features, configurations, occurrence transforms or per-entity references.

The installed SDK documents high accuracy as enum2 and +/-0.1% error; repeat
floating-point equality is a conservative consistency gate, not proof of exact
analytic geometry or repeatability on every model. No tolerance relaxation,
calculation retry, material setter, command execution or geometry edit is used.
Unknown SDK/bounds/membership/calculation errors retain unavailable/unknown read
semantics (including immediate SDK status/stage/code/bounded diagnostic text).
A known stale returned projection revision produces400. Earlier shared document
SDK helpers can report only their original status; the adapter does not invent
missing diagnostic data or reset/read errors again. No journal write/admission or
effect permission arises from this read. Existing mutation dispatch is unchanged.

Official references, cross-checked against installed C++ declarations:

- [PhysicalProperties](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_PhysicalProperties.htm) defines mass kg, volume cm3 and area cm2.
- [Component.getPhysicalProperties](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_Component_getPhysicalProperties.htm) takes an explicit calculation accuracy.
- [PhysicalProperties.centerOfMass](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_PhysicalProperties_centerOfMass.htm) returns the actual center point; SDK geometry lengths use internal centimeters.

Linux policy/socket tests and binding/syntax checks do not qualify the actual
SDK's empty-root, material-mass or two-pass behavior. Root's Windows/actual-app
qualification and independent numeric/material observer are still required.
