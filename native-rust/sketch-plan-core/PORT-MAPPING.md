# Legacy sketch mapping

Read-only references, never executed:

- `../../exploded-preview-preparation/runtime-dev9-build-r1/source/addin/AdomBridge/commands/mechanical/sketches.py`, SHA256 `1117e1e18d12ce3b4a92a20da43e4283d717f041ec4b05afdc64b5bb0984fe0c`.
- Adjacent `schema.json`, SHA256 `6f704e05de031fa8ef329dc7be0b02d3a2b167eeb2fbc44ec6c2d933af4f7b48`; `sketch_geometry` schema has eight variants, 1–128 items, 3–128 polygon sides, 2–128 spline points, mm coordinates and degree angles.
- Adjacent `core.py::number` accepts finite non-Boolean numeric values and optional positivity. `test_mechanical_transactions.py` in the published-readback tests includes Boolean-radius schema refusal. No legacy Python test was run.

| Legacy behavior | Rust pure preparation | Remaining difference/gate |
| --- | --- | --- |
| `point` | Two/three-number `PointMm`; mm/10, zero Z only for 2D | New numeric range/underflow checks; no Point3D SDK allocation. |
| Point/line/circle branches | Typed point/line/center-radius instructions | Native creation, validity and readback absent; extra degeneracy/envelope checks. |
| Rectangle branch | Four ordered corners/lines, original `PrimitiveKind::Rectangle` retained | Native `addTwoPointRectangle` ordering and helper constraints are not qualified by these coordinates. Explicit equal-Z guard. |
| Arc branch | Signed radians plus analytic Z-rotation endpoint | Native plane/projection semantics and measured endpoint absent; equal Z, finite envelope and noncollapsed chord required. |
| Polygon branch | Same circumradius, positive angular order and closed line chain | Angle bounded/reduced before trig; floating-point bits need not match Python; minimum edge length is stricter. |
| Slot branch | Left-perpendicular half-width, line/negative-half-turn/line/negative-half-turn order | Same Z tolerance and center distance threshold; stricter radius/envelope/representability rules; exact shared predicted endpoints. |
| Fitted spline branch | Preserve all ordered fit points and `closed` flag | No solved spline is produced; stricter closed minimum and repeated-endpoint refusal. |
| Construction assignment | Curve `Option<bool>` preserved per generated entity | No property setter; invalid point flag rejected at typed JSON boundary. |
| Complete geometry batch | All validation/expansion/output checks before returning a plan | Pure preparation is not legacy transaction grouping or rollback. |
| `quantity` | Numeric lengths used by these primitives only | Unit-bearing expressions, dimensional validation and evaluated ValueInput require Fusion UnitsManager; not approximated here. |
| `sketch_create`, `component`, `plane`, `axis`, `get_sketch`, `named` | No native equivalent | Exact SDK identity/ownership/validity and current revision checks are integration work. |
| Constraint/dimension/project/edit/plane handlers | Not implemented | Depend on selected native entities, current geometry, solver or units; this crate does not supply their SDK effects or claim complete sketch porting. |

The prepared JSON deliberately has no session, document, revision, guarded native selector, transaction or durable-effect fields. It is not a compatible replacement for a legacy request or result. Native integration and public exposure are later gates.
