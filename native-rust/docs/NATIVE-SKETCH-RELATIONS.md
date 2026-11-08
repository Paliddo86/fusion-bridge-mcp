# Finite horizontal/vertical constraint contract

Use `X-Adom-Mechanical-Contract: native-sketch-relations-v1`. All older headers
remain accepted for their existing operations; this new header is required for
the two new capabilities. The shared legacy operation name does not imply full
legacy constraint parity. `coincident`, parallel, tangent, dimensions, fix/unfix,
multiple entities, existing constraints and arbitrary graph adoption are refused.

First create one native root-origin sketch and insert one nonconstruction sloped
line, away from the axes and origin. Publish its exact returned ID through
`native_sketch_entities`, then obtain `native_sketch_line_state`. Its complete
finite projection must show no constraints, dimensions, reference links, text,
attributes, other geometry, root features/bodies/occurrences/meshes or additional
components. The line and both endpoints must be unfixed and not fully constrained.
Only this single sketch's final timeline item may exist. XY/XZ/YZ are supported
in sketch coordinates; no model-space substitution or plane inference is used.

```json
{
  "op":"sketch_constraint", "sessionId":"<current epoch>", "operationId":"<new id>",
  "target":{"documentId":"<doc>","revision":"<line-state revision>"},
  "sketch":{"documentId":"<doc>","revision":"<same revision>","objectType":"Sketch","sketchId":"<id>"},
  "type":"horizontal",
  "entities":[{"documentId":"<doc>","revision":"<same revision>","objectType":"SketchLine","sketchId":"<same id>","entityId":"<published exact line id>"}]
}
```

Use `vertical` for the other supported direction. Revision is the exact
`sketch-line-state-projection-v1:<sha256>` value, not a sketch summary/entity
projection or SDK revision alone. Target/ref identities and revisions must agree.
Argument/type/arity/header failures occur before journal admission. Preflight
checks two complete projections and neutral command state. The serialized worker
reserves result capacity and durably records Executing before the owner-thread
execute repeats the complete projection, then invokes exactly one SDK add method.

Before invocation the existing sketch holder has fixed typed horizontal/vertical
slots and a generic relationship view; the attempt latch is set once. The exact
typed SDK return is stored before generic conversion, diagnostics or readback.
No allocation of a new registry slot follows the effect. Readback checks both
typed and generic returned relationships, exact parent sketch/native context,
exact linked retained line, complete whole-sketch constraint membership and
per-line/endpoint relationships. Endpoints and origin retain exact typed SDK
objects; origin has no relationships/connections. Each endpoint's relationship
list may contain zero or one reference to this exact returned constraint; no
other object is accepted. Repeated reads do not add holders. All these child
references remain beneath the existing sketch/document lifetime and release fence.

Postconditions require one requested relationship, healthy nondeferred sketch,
unchanged plane/frame/name/counts and out-of-scope resources, unchanged origin,
finite nondegenerate line and the requested alignment. Coordinates are bounded
by10000cm, length at least1e-5cm; alignment residual at most1e-8cm. Initial dx/dy
and each endpoint's distance from each axis are at least1e-4cm. Endpoints may move
according to the solver. Length and endpoint placement are deliberately not
specified by a single orientation constraint. No approximation is used for SDK
identity or the repeated projection equality.

Neutral-command checks and exact parent revision bracket repeated post-effect
relationship/geometry observations, with the final root comparison last. Success
returns `data.target`, `data.root`, `data.sketch`, and `data.relationState` with
`kind` 0=none,1=horizontal,2=vertical and the full `line` inventory in centimeters.
Its revision is `sketch-relations-projection-v1:<sha256>`. No public constraint ID
or endpoint ID is introduced. The old line ID still refers to its exact original
object; its observed geometry may now differ. Existing partial entity inspection
can publish the new geometry projection without adopting a new object.

Read the result again with:

```json
{"op":"native_sketch_relations","sessionId":"<epoch>","target":{"documentId":"<doc>","revision":"<optional relation revision>"},"sketchId":"<id>","entityId":"<exact line id>"}
```

This read accepts zero constraints or the one exact relationship returned by this
runtime, never an externally added equivalent relation. Failed/changed reads
retire entity access. Stale guards refuse without retirement. The older R17
line-state read remains zero-constraint only: its `constraintMutationSupported:false`
is scoped to that older contract; use the new capabilities/header to mutate.
Calling that older read after adding a relation is outside its supported inventory
and may retire its entity access. Use the relation read after mutation.

Any error/null/panic after add invocation remains unknown and poisons the sketch
book. A confirmed first observation is retained if a later read/command check
fails. If no add was invoked, the result is known not-executed. Exact duplicate
operation IDs return the existing journal result; differing requests conflict.
Terminal storage loss retains the process-local observed result without granting
replay. An HTTP deadline does not discard a claimed operation's later completion.
No automatic relation deletion, rollback, solver retry or success substitution.

Tests cover instrumented both-direction behavior, wrong/extra graph, fixed/axis/
aligned input, origin movement, changed reads, command/getter panics, unknown add,
precise selector/stale bounds, real HTTP contract gates and no journal for reads,
file-backed durable start/replay/conflict/DB disappearance and late completion.
They do not establish actual Fusion solver behavior. Real host qualification
needs fresh independently owned isolated sloped lines and a relation-aware cleanup
fixture; existing geometry fixtures do not authorize this mutation.

SDK semantics checked against installed `GeometricConstraints.h`,
`HorizontalConstraint.h`, `VerticalConstraint.h`, `SketchEntity.h`,
`GeometricConstraint.h`, `SketchPoint.h` and
[Autodesk's constraints reference](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_GeometricConstraints.htm)
and [typed horizontal line getter](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/HorizontalConstraint_line.htm).
