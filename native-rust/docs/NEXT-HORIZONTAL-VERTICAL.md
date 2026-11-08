# Prepared finite mutation continuation

The R17 line-state read is the prerequisite, not a completed constraint port.
Implement the next slice separately without weakening R17's zero-constraint read
contract or existing headers. All solver placement is observed, never promised
unique. No downstream bodies/dimensions/foreign graph, implicit origin mutation,
projection imports, rollback or retry.

Use a new explicit native-sketch-relations-v1 contract. Accept only legacy
sketch_constraint type horizontal or vertical, one exact published SketchLine,
matching target/sketch/entity document+revision+IDs and current session/operation.
Require the current sketch-line-state projection, one unfixed/nonreference line
and unfixed endpoints, no existing relationship or dimension, complete isolated
root scope. Initially require a sloped line away from both axes/origin and a
bounded nondegenerate span; redundant/already-aligned input refuses before effect.
Prevalidate arity/type/ref fields before journal admission. Preflight obtains the
complete R17 projection; retain result capacity, durably dispatch/start through
the existing journal coordinator, then exact revalidation on the owner thread.

New SDK ABI needs one `Ptr<GeometricConstraint>` slot below AfSketchRef and a
once-only attempted latch reserved before calling addHorizontal/addVertical.
Assign the return directly into that slot before checking the acknowledgement.
A null return, exception or failed SDK status after invocation is unknown; no
retry, deletion or fake rollback. Stop/release must preserve the existing child
uncertainty fence.

The post-effect reader must be a separate relation-aware typed projection: exact
returned relationship remains valid/current, parentSketch and assemblyContext
match, and typed HorizontalConstraint.line/VerticalConstraint.line equal the
original retained line. Enumerate the complete sketch constraint collection,
requiring precisely that one object. For line/endpoint per-entity lists allow
only that exact returned relationship and reject every other object; independently
observe which endpoint lists include it (do not assume they all do). Origin must
still have zero connections/relationships. Dimensions/foreign resources remain
absent; exact start/end/origin SDK objects and complete point membership persist.

Report final coordinates and typed relationship evidence. Verify the requested
horizontal residual dy=0 or vertical residual dx=0, finite sketch-plane geometry
and nonzero span; no unchanged endpoint placement or length claim without a
separate length constraint. Origin and every out-of-scope resource remain unchanged.
Require healthy nondeferred sketch and two final agreeing full relation captures,
with root revision baseline after acknowledged solver creation, and final root
comparison last. A single successful pointer, health or fullyConstrained flag
alone is insufficient. Preserve all confirmed add/readback stages if a later
getter, clock, terminal commit or callback fails; retained unknown never authorizes
replay. Journal replay/conflict/storage-loss/deadline paths must use actual existing
coordinator tests, not only isolated policy fakes.

Expose a distinct read-only native_sketch_relations route/revision for this finite
0-or-owned-1 relationship graph. R17 line-state remains zero-relationship only,
so clients should not call that older scope after successful mutation. Keep current
line entity IDs as exact ownership references, with updated relation revision; no
new endpoints or arbitrary graph adoption. Avoid broadening old contract silently.

Concrete tests: false healthy geometry; wrong returned relationship/parent/line;
endpoint replacement with identical coordinates; unrelated origin motion; extra
constraint; solver return-null/ack error; post-add panic and final-read disagreement;
capacity before call; foreign/stale/arity/ref/current-session refusal before SDK;
late completion retained; durable replay/conflict no duplicate add; failed commit
retains observed relation without replay. Real Fusion needs fresh independently
owned sloped lines on XY/XZ/YZ for both directions and relationship-aware cleanup
fixtures. Existing geometry fixtures do not authorize constraint mutation.

R18 continuation note: this prepared plan is now implemented within the finite
scope in `docs/NATIVE-SKETCH-RELATIONS.md`. The original R17 source remains frozen.
Actual SDK/solver qualification remains a separate gate.
