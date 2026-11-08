# Pure Rust sketch geometry preparation

This isolated library prepares the numeric portion of the eight legacy `sketch_geometry` primitive forms. Input coordinates and lengths are sketch-space millimeters; angles are degrees. Output is immutable supplied-data instructions in centimeters and radians. It creates no sketch, entity, constraint, transaction or document and makes no SDK validity, current identity, solvability, native readback or `modelUnchanged` claim.

The author previously served as migration verifier and authored earlier native/storage and exploded-planner work. This is explicitly an author role. The public API tests below have hand-derived arithmetic oracles, but are author-written, not an independent external verification campaign. Root and the separate reviewer perform independent follow-up. No Python was executed or added, no host/AB/Fusion/capture action was taken, and no r7/r8/r9, installed bridge or other frozen core was edited.

## API

`Request { items: Vec<Primitive> }` accepts `Point`, `Line`, `Rectangle`, `Circle`, `Arc`, `Polygon`, `Slot`, and `Spline`. `PointMm::Xy([x,y])` adds zero Z; `PointMm::Xyz([x,y,z])` retains all three coordinates. JSON uses the legacy primitive field names and tagged `type`; unknown fields, Boolean numeric values, bad coordinate dimensions, noninteger sides and null Boolean flags are rejected. The containing request is deliberately only `items`, not a substitute for the legacy operation/session/target/reference envelope.

`prepare(&request)` or `prepare_with_limits(&request, limits)` returns `PreparedPlan` only after the entire request passes numeric, count, expansion and serialized-output checks. `items()` preserves original item order/index and `PrimitiveKind`, including the distinction between a rectangle and four independent lines. Each item contains its prepared entities; `entity_count()`, `stored_point_count()` and `to_json_bytes()` expose bounded output. The plan has no mutable public fields and cannot be deserialized as a validated plan. Cloning an individual public geometry does not grant a native capability.

Construction is `Option<bool>` on curves: omission stays distinct from an explicit false flag. A point cannot accept construction. The same requested flag is carried on every generated curve. No flag is applied by the planner. An omitted flag serialized in output is `null`, meaning no assignment; the input JSON rejects an explicitly null flag, matching the legacy schema.

| Primitive | Prepared instructions |
| --- | --- |
| Point | One exact converted point, retaining signed zero. |
| Line | One ordered start/end segment; supplied 3D coordinates are preserved. |
| Rectangle | Four XY-aligned lines through `(sx,sy,z)`, `(ex,sy,z)`, `(ex,ey,z)`, `(sx,ey,z)`, sharing exact endpoint arrays. Z must match exactly after conversion. Native helper-created constraints and solver behavior are not represented or certified. |
| Circle | Center and positive radius, both converted to cm; radius is not diameter. |
| Arc | Center/start, signed degree-to-radian sweep and predicted endpoint from rotation about sketch Z. Center/start Z must match exactly. Positive is counterclockwise in the XY convention; negative is clockwise. |
| Polygon | 3–128 equal-angle vertices with circumradius and optional starting angle (default zero), then one closed line per side. Angle reduction is modulo 360 degrees. |
| Slot | Two parallel lines and two clockwise half-circle arcs. The offset is the normalized left XY perpendicular times half the width. Opposite endpoints are carried explicitly so the four segments share exact endpoints instead of a `sin(-PI)` rounding gap. |
| Spline | Ordered converted fit points and the requested closed flag (default false). No spline coefficients, knot vector, interpolated curve, closure geometry or solver result is fabricated. |

All output endpoints are numeric predictions, not observations of what Fusion created. Slot center Z tolerance remains the legacy `abs(delta Z) < 1e-9 cm`; the two supplied Z values are retained without snapping, so near-equal Z need not imply one exact plane. Lines and spline fit points can also be 3D. Future integration must establish the sketch's actual 2D/3D mode and reject incompatible data rather than silently projecting it.

## Bounds and deliberate stricter refusals

| Budget/check | Policy |
| --- | --- |
| Input items | 1–128, matching the schema. |
| Polygon sides | 3–128, matching the schema. |
| Spline fit points | 2–128 for open; 3–128 for closed. Closed minimum is deliberately stricter than the legacy 2-point schema. |
| Coordinates and size inputs | Finite, absolute value ≤ `1e9 mm`; nonzero mm underflowing to zero cm is refused. |
| Generated coordinates | Finite, absolute value ≤ `1e8 cm`. |
| Minimum span | Every line, polygon edge, radius, slot half-width/center separation and adjacent spline span exceeds `1e-8 cm`; arc endpoint chord also exceeds this threshold. This is a conservative planning threshold, not an SDK tolerance guarantee. |
| Circle envelopes | The full XY circle must fit coordinate bounds, including for arcs, polygons and slot ends; each axial radius must be representable above the span threshold. A partial arc which fits but whose full envelope does not is conservatively refused. |
| Arc sweep | Finite `0 < abs(degrees) < 360`, nonzero representable radians below a full turn. Tiny/near-full sweeps that collapse the endpoint chord are refused. |
| Polygon angle | Finite magnitude ≤ `1e9 degrees`, reduced before trigonometry. Legacy allowed any finite angle. |
| Expanded entities | ≤ 4096, counted before generating geometry. |
| Stored point operands | ≤ 16384. Counts point=1, line=2, circle=1, arc=3, fitted spline=fit-point count; generated curves count individually. |
| Serialized output | ≤ 2 MiB, measured with the actual serializer and a bounded counting writer before success. |

`Limits` may reduce the three aggregate budgets, never expand them. Oversized or invalid work returns an error, without a successful truncated plan or side effects. Limits cover validated expansion/output, not arbitrary caller-owned input allocation, deserialization, allocation failure or process memory. Input admission remains the caller's responsibility.

Compared with the legacy handler's SDK-deferred cases, this core additionally refuses zero/tiny lines or rectangle sides, ambiguous off-plane rectangles/arcs, insufficient closed-spline fit points, adjacent coincident fit points, and a repeated closing fit point. Nonadjacent repeated fit points, self-intersection, zero-area or collinear closed fits, tangent continuity, constraint compatibility and final spline solvability are not generally decided here. Their native validation remains mandatory. The pure preparation result is not permission to apply an unqualified plan.

## Verification and integration gaps

24 public API integration tests pass on Linux. Their oracles include a triangle starting at 30° (`sqrt(3)` coordinates), a circumradius square, a 3–4–5 diagonal slot, horizontal/reversed vertical slots, signed quarter/half arcs, exact shared endpoints, 3D and signed-zero conversion, all eight strict JSON forms, invalid numeric/degenerate/plane inputs, 128/4096/16384 inclusive budgets, and exact serializer byte limits. The Windows GNU target is compile-only qualification when recorded; no Windows or Fusion execution is implied. Raw evidence and hashes are listed in [evidence/RESULTS.md](evidence/RESULTS.md) and `SOURCE-CHECKSUMS.sha256`.

Still absent: native sketch/object reference resolution, current document/revision/membership checks, component and plane ownership, unit expression evaluation, native geometry creation, rectangle helper constraints, spline interpolation, solver/constraint/dimension/project/edit/plane behavior, main-thread dispatch, transactions and recovery, exact SDK readback, HTTP registration and live integration. Native integration must retain the original primitive intent, establish these guards, apply an authorized effect once, and observe its actual result. No complete legacy sketch parity or public release is claimed.
