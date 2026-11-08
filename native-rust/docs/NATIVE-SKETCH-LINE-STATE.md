# Exact isolated line-state prerequisite

POST `/v1/mechanical` with `X-Adom-Mechanical-Contract: native-sketch-line-state-v1`:

```json
{"op":"native_sketch_line_state","sessionId":"<current epoch>","target":{"documentId":"<exact current document>","revision":"<optional sketch-line-state-projection-v1 SHA256>"},"sketchId":"<retained native sketch>","entityId":"<already published exact line ID>"}
```

Obtain the entity ID through `native_sketch_entities` after one fully confirmed
`sketch_geometry` containing exactly one ordinary line. Native root-origin sketch
creation, successful geometry and exact entity publication must all belong to the
current runtime/document. No name/token/coordinate/collection-index adoption or
implicit active-document target is accepted. Shape/session/header refusals occur
before dispatch; unpublished, foreign and guessed line selectors are rejected
before line-backend access.

Finite scope requires an active parametric hybrid unconfigured design, exactly
one root sketch and its single final timeline item, one nonconstruction native
line, exactly two distinct nonorigin endpoints plus the exact origin point, no
profiles, text, attributes, constraints or dimensions. Root bodies/features/
occurrences/meshes must be zero and the design must contain only its root component.
Each endpoint's connectedEntities list must contain exactly the retained line;
the origin must have no connections. Line/endpoints must not be referenced or
have a referenced entity. All four entities' parent, validity, native context,
relationship/dimension lists and attributes are inspected. The origin's special
reference/fixed state is observed rather than fabricated.

Fixed/fully-constrained flags are returned: `unfixedLineAndEndpoints:false` is a
useful observation, not permission to force/unfix anything. Units are centimeters.
Coordinates must be finite, within10000cm and lie on sketch Z=0; endpoints must
remain distinct from origin and each other with at least1e-5cm separation. The
complete actual points collection is matched by SDK object equality, not position.
Only three fixed-capacity endpoint/origin references can be retained beneath the
existing sketch holder. Repeated reads must match them and never replace them;
release uses the existing child-before-parent uncertainty fence.

Two complete captures agree, including the root/sketch summaries, actual line
geometry, all bounded flags and points; additional summaries bracket each capture.
A SHA256 `sketch-line-state-projection-v1` binds exact document/sketch/entity ID and
observed state. Graph-only/fixed-state changes alter this revision even if an SDK
revision is unchanged. A stale expected revision refuses without retiring a valid
identity. Failed/changed captures retire published entity access rather than
silently rebound to another endpoint with equal coordinates.

Response includes exact target/sketch/entity IDs, root and sketch summaries,
`lineState`, `isolatedInventoryComplete:true`, `constraintMutationSupported:false`,
`legacyParity:false` and the scope string. Completeness applies only to this finite
zero-relationship inventory, not arbitrary sketches or a general solver graph.
No endpoint IDs are published. Existing partial `native_sketch_entities` semantics
remain unchanged.

SDK read failure/panic/unstable capture remains an unavailable/unknown read (503),
with numeric stage/error and bounded diagnostic text when captured by the new
adapter. No journal record or CAD write is issued. Shared inherited sketch guards
can fail before the new diagnostic getter sequence; their original error remains,
but full per-getter diagnostics for those inherited guards are not claimed.
No tolerance, automatic retry, API-error reset or success substitution is added.

Installed headers checked: SketchEntity parentSketch/isFixed/isReference/
referencedEntity/isFullyConstrained/is2D/geometricConstraints/sketchDimensions;
SketchPoint connectedEntities; SketchLine startSketchPoint/endSketchPoint;
GeometricConstraints addHorizontal/addVertical; HorizontalConstraint/VerticalConstraint
line. Mutation getters are documented for the continuation only and are not called.
