# Exact returned sketch entities

R13 adds one read helper, `native_sketch_entities`, under the explicit
`native-sketch-entities-v1` header. All earlier headers/operations remain
supported; prior geometry operation responses are unchanged. No constraints or
dimensions are implemented by this slice, and it makes no complete solver graph
or legacy parity claim.

The helper requires the current document ID and retained sketch ID, after a
native geometry operation completed all of its SDK success checks. Only lease
indices from that known native result are stored. Actual SDK references remain
in the original sketch holder and retain the existing child-release fence.
Unknown prefixes cannot publish IDs. There is no collection lookup by a public
entity selector, entity-token fallback, name adoption or coordinate matching.

Each read obtains two complete bounded captures: exact parent/validity/current
membership, geometry, construction/reference flags, sketch frame/counts/health
and the complete equality matrix for these explicit returned objects. Equality
uses one typed SDK observation per capture and a bounded64-by64 matrix; Rust
validates and canonicalizes the relation. Multiple SDK leases of one actual
object share one public ID. Separate objects with equal coordinates retain
different IDs. Subsequent identity changes or failed/panicking observations
retire entity publication for that sketch; these IDs are never rebound.

The new projection includes geometry coordinates and has its own revision
prefix. Geometry edits can change that revision while preserving exact object
identity. A stale request refuses without retiring otherwise valid IDs or
publishing new IDs. The response includes a separate existing sketchTarget for
operations that require sketch-projection-v1. References are scoped to the
runtime/document/sketch and are not portable across reloads or restarts.

Bounds are64 explicit entities,512 stored points,128 points per fitted spline
and128KiB of revision projection. Repeated reads acquire no additional SDK
holders. Implicit endpoints/centers/origin, fixed/linked flags, constraints,
dimensions and full spline shape remain outside the scope (`complete:false`).
They require later typed readers before general constrained editing.

Regression coverage includes identical coordinates with distinct SDK objects,
actual aliases, alias split/merge, changing coordinates without a changed SDK
revision, stale/foreign selectors, missing members, malformed flags/nonfinite
values, getter panic, unknown creation prefixes, repeated reads, maximum entity
and point bounds, HTTP header parsing and capability/schema compatibility.
An inherited observer-deadline test exposed a150ms admission race under full
workspace load. It now claims the actual effect first, waits for the real
observer deadline, then verifies durable late completion without sleeping for
an assumed scheduling interval. This is a test synchronization correction, not
an execution behavior change.

Linux/Windows/source evidence and actual Fusion acceptance are tracked
separately. This source candidate has not been loaded, installed or published.
