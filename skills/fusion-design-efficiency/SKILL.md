---
name: fusion-design-efficiency
description: >
  The playbook for Fusion designs built through the bridge that stay fast to recompute, open and
  edit, not just correct. Measure with efficiency_report, then place parts at creation, move them
  by editing the initial position, insert instances instead of copies, pattern 3D features or
  components instead of sketch entities with an identical compute option, and split big designs
  into referenced sub-designs. Each rule has its evidence (benchmarks run through the typed
  operations, Autodesk and forum sources) and the bridge operation that applies it.
  Trigger words - efficient design, slow design, timeline too long, recompute slow, edit initial
  position, position snapshot, capture position, move feature, paste new, copy paste, instances,
  component pattern, feature pattern vs sketch pattern, compute option, sub-assembly, xref, insert
  by reference, design size, large assembly, heavy model, defeature, wires slow, efficiency report.
---

# Fusion design efficiency

A design people will reuse has to stay quick to recompute and edit. These rules come from Autodesk's
large-assembly guidance, practitioners, and benchmarks run through the bridge's typed operations on
Fusion 2705 (October 2026). Rules marked **measured** have numbers from those runs.

## Measure first

`fusion_mechanical {op: "efficiency_report", target, sessionId, operationId}` reads the timeline
make-up and health, Position snapshots, Move features, size, repeated definitions (Paste New copies),
instance-weighted faces, imported history, wires in a parametric design, big sketches, pattern
compute options, modeled threads, meshes and empty components. It returns `findings` (rule,
severity, measured, limit, fix), worst first.
- It never changes the model, and it is journaled. On a big design, follow it with
  `fusion_mech_operation_status` after the relay's 60 s.
- A 4,579-occurrence production cell took 76 s.
- `computeTiming: true` also times a full `computeAll` (slow on big designs).

Run it before and after a change, and before calling a build done.

## Placement

1. **Create each occurrence where it belongs.** Pass `transform` to `component_create` or
   `occurrence_insert`; that placement is free. **Measured:** 10 blocks placed at insert gave a
   12-item timeline and 0 snapshots.
2. **To move an occurrence, edit its initial position.** `occurrence_update` does this by default.
   It uses Fusion's Edit Initial Position (`Occurrence.initialTransform`, November 2024), so nothing
   joins the timeline.
   - Fusion edits only the last occurrence in place. For earlier ones the bridge rolls the marker to
     just after the occurrence's creation, edits it, and puts the marker back (about 20 ms each).
   - Nested, patterned or jointed occurrences fall back to a captured Position snapshot. The reply's
     `placement` says which happened.
   - **Measured:** 9 moves left 0 snapshots and the same 12 timeline items. Forcing
     `method: "capture"` gave 9 snapshots and 21 items.
3. **Ground static parts to their parent** (`groundedToParent: true`) once placed. Later position
   changes then cannot drift them.
4. **Position parts; add joints only when the human asks.** Place every occurrence with its transform
   (the pose at insert, or `occurrence_update`) and stop there. Positioned parts stay put unless
   someone drags them. Joints, rigid groups and motion links are solved again on every compute, and
   Autodesk lists joint solving as a large-assembly slowdown, so add one only for motion or a
   mechanism the human asked to drive. When one is asked for, a rigid group beats many rigid joints.
   Positioned parts don't follow when the part they sit on changes size. So if the assembly is meant to
   be driven by parameters, say so and offer joints for those parts, rather than adding them silently or
   leaving them out silently.

## Repeated parts

5. **Insert instances of one component, never copies made as new components (Paste New).**
   Use `occurrence_insert` with the component's ref (`independentCopy` stays false).
   - **Measured:** 8 identical frame members as instances built in 45 to 80 s with 1 definition, a
     10-item timeline and 0.14 s to recompute. As 8 separate components they took 143 to 314 s,
     8 definitions, 24 items and 0.40 s.
   - The BOM counts instances as one line of 8.
6. **Vendor parts come from their own design, inserted by reference.** A referenced design takes one
   timeline item and does not join this design's compute. Instance it after the first insert.
7. **Component patterns for regular arrays** (T-nuts at a pitch, a row of members).
   Use `feature_create {type: "rectangular_pattern" | "circular_pattern", occurrences: [...]}`.
   - **Measured:** 8 members built in 43 s in one operation, against 80 s for 7 inserts, with a
     4-item timeline against 10.
   - Recompute is slightly higher (0.21 against 0.14 s), and patterned occurrences cannot have their
     initial position edited. For an irregular layout, use instances.

## Patterns

8. **Pattern the 3D feature, not sketch entities.** Model one hole, then
   `feature_create {type: "rectangular_pattern", features: [hole]}`. **Measured** on a 400 × 400 mm
   plate with 100 holes:

   | approach | sketch curves | recompute |
   |---|---|---|
   | one sketch of 100 circles | 104 | 0.87 s |
   | one hole + feature pattern | 5 | 0.24 s |

   Autodesk's own test at 2,500 instances: a face pattern took about 35 s, and the same pattern with
   adjust took 38 minutes.
9. **Keep the compute option identical** (the bridge's default) or optimized. Use adjust only when
   each instance must adapt to the geometry under it. **Measured:** adjust took 2.42 s against 0.24 s
   for identical (10×). `efficiency_report` flags adjust patterns.

## Size and splitting

10. **Keep a design under about 1,000 components or occurrences.** Autodesk's large-assembly article
    puts slowdowns at around 1,000 components, and AU classes at 1,500 to 2,000. Split bigger builds
    into sub-designs inserted by reference. Each computes on its own, and the top level stays light.
11. **Keep imported and generated geometry out of the parametric timeline.** Build STEP imports,
    wires, harnesses and dense generated bodies in direct-mode designs, then insert them by
    reference.
    - A direct-mode wiring design took 0.016 s per add, against 0.17 s when parametric.
    - Capturing import history roughly triples file size.
12. **Wires in their own design.** Use few path entities (lines and tangent arcs, or splines with few
    fit points). Inserted by reference, 114 routed wires cost the main assembly nothing to edit.

## Sketches and timeline hygiene

13. **Keep sketches under about 200 curves.** Each sketch is a constraint solve. Pattern in 3D instead
    of repeating sketch entities.
14. **Zero timeline errors and warnings.** Every one is recomputed each time. Reference origin
    geometry rather than other components.
15. **Batch work into fewer operations.** Each guarded operation pays a revision check and a read of
    the timeline's health (O(model)), and every few writes a full recompute, so N small operations
    cost O(N²) on a big design. Prefer one pattern, one multi-entity sketch geometry call or one insert
    at the final pose.

## Heavy detail

16. **Cosmetic threads** (`modeled: false`, the default) **and unthreaded vendor models**
    (McMaster's "no threads" STEP). Fifty fully threaded screws can bog a system down.
17. **Defeature vendor models** before inserting them: drop small fillets, text and springs. Keep a
    simplified copy of high-face-count parts. `efficiency_report` lists definitions whose faces times
    instances pass 20,000. On the production cell that was a terminal block of 723 faces × 42.

## What the production cell showed

A 4,579-occurrence cell (1,249 components, 243 by reference) had these findings, each tied to a rule
above:
- 66 Position snapshots and one uncaptured move (rule 2);
- 165 repeated definitions (rule 5);
- 637 base features of imported geometry in a parametric design (rule 11);
- 15 timeline warnings (rule 14).

Before the guard fixes, one guarded operation on it took about 20 minutes. It now takes about 40 s;
the remaining cost is size (rule 10).

## Sources

- Autodesk, Edit Initial Position (November 2024 update) and `Occurrence.initialTransform`.
- Autodesk knowledge base, "Performance issues when working with large assemblies".
- AU 2015 CP10167, "Fusion 360 Large Model Management".
- Autodesk blog: "3D solid patterns vs sketch patterns", and "Mirror and pattern compute options" (2026).
- Autodesk forum, "Problem creating a big rectangular pattern" (Jeff Strater's 2,500-instance timings).
- Autodesk forum, "Most efficient way to crowd a design with McMaster nuts and bolts".
- Autodesk API manual, Design Intent (part, assembly and hybrid designs).
- Benchmarks: `efficiency_bench.py` (placement, frame, holes) in the certification scripts (fusion-cert).
