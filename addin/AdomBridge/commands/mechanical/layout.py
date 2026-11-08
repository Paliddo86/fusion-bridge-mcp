"""What an assembly planner reads from the design: the parts of a scope with their boxes, the joints and
rigid groups between them, and batches of rays. Both operations change nothing.

assembly_layout lists the visible occurrences directly under a scope occurrence (or under the root), each
with its box in root space, and the scope component's joints and rigid groups between them. It is
journaled: the first guarded read after the add-in loads registers every entity of a big assembly (about
45 s on a 4,579-occurrence one) and can outlive the relay's 60 s, so the caller follows it with
fusion_mech_operation_status.

ray_cast casts rays, each against the few occurrences named for it, in each occurrence's own component
coordinates (the inverse of its root-space transform): a component holds a handful of bodies where a whole
scope can hold thousands (a ray against a 4,579-occurrence assembly's panel component took about 0.25 s,
against one part's component about a millisecond). It is not journaled: a plan casts thousands of rays,
and the journal keeps every request and result with no eviction (10,000 records, 64 MiB), so journaled ray
batches would fill it within tens of plans. It stops at budgetSeconds instead, and before its reply passes
1 MiB, and says how many rays it answered; the caller sends the rest.
"""
from collections import deque
import json
import time

import adsk.core
import adsk.fusion

from .core import active, current_target, occurrence_at, read_target, require, target

DEFAULT_BODIES = 200          # bodies read per part for its box (a body proxy's box costs about 26 ms on a big assembly)
MAX_UNITS = 4096              # visible parts in one scope
MAX_LAYOUT_BYTES = 2 * 1024 * 1024
MAX_REPLY_BYTES = 1024 * 1024
DEFAULT_BUDGET = 30.0         # seconds of ray casting per call, inside the relay's 60 s
_clock = time.perf_counter


def part_box(occurrence, cap):
    """The root-space box (mm) of an occurrence's bodies and its children's, read breadth first from at
    most cap bodies. Returns (box or None when it has no bodies, whether the cap stopped the walk)."""
    lo = hi = None
    read, pending, stopped = 0, deque([occurrence]), False
    while pending and read < cap:
        node = pending.popleft()
        bodies = node.bRepBodies
        for i in range(bodies.count):
            if read >= cap:
                stopped = True
                break
            box = bodies.item(i).boundingBox
            read += 1
            low = [box.minPoint.x * 10, box.minPoint.y * 10, box.minPoint.z * 10]
            high = [box.maxPoint.x * 10, box.maxPoint.y * 10, box.maxPoint.z * 10]
            lo = low if lo is None else [min(u, v) for u, v in zip(lo, low)]
            hi = high if hi is None else [max(u, v) for u, v in zip(hi, high)]
        kids = node.childOccurrences
        pending.extend(kids.item(i) for i in range(kids.count))
    return ([lo, hi] if lo else None), stopped or (read >= cap and bool(pending))


def part_path(occurrence, scope, names):
    """The scope part an occurrence belongs to, "" for the scope component's own geometry. A joint's
    occurrences are in the scope component's context, so their paths start with the part's own name;
    names (every child's, longest first) keep a name that holds "+" whole."""
    if occurrence is None:
        return ""
    path = occurrence.fullPathName
    head = next((n for n in names if path == n or path.startswith(n + "+")), path.split("+")[0])
    return scope + "+" + head if scope else head


def assembly_layout(app, args):
    # the guard and the reply are inspect's: an optional target, and the current target returned
    if args.get("target"):
        doc, design, value = target(app, args)
    else:
        doc, design, _ = active(app)
        value = current_target(app)
    root = design.rootComponent
    scope = args.get("scope") or ""
    cap = args.get("maxBodies", DEFAULT_BODIES)
    start = None
    if scope:
        start = occurrence_at(design, scope)
        require(start is not None and start.fullPathName == scope, "occurrence_missing",
                "No occurrence at %r: pass the full path of the subassembly (\"Panel:1\", \"Cell:1+Panel:1\")." % scope[:200])
    component = start.component if start else root
    children = start.childOccurrences if start else root.occurrences
    require(children.count <= 20000, "inspection_budget", "The scope has more than 20000 occurrences directly under it.")
    kids = [children.item(i) for i in range(children.count)]
    names = sorted((k.name for k in kids), key=len, reverse=True)
    units, hidden = [], 0
    for occurrence in kids:
        if not occurrence.isLightBulbOn:
            hidden += 1
            continue
        require(len(units) < MAX_UNITS, "layout_budget", "The scope has more than %d visible parts; pass the path of a subassembly as scope." % MAX_UNITS)
        box, capped = part_box(occurrence, cap)
        unit = {"path": occurrence.fullPathName, "name": occurrence.name, "component": occurrence.component.name,
                "grounded": occurrence.isGrounded, "box": box, "children": occurrence.childOccurrences.count}
        if capped:
            unit["boxCapped"] = True
        units.append(unit)
    joints, groups, unreadable = [], [], 0
    for collection, label in ((component.joints, "joint"), (component.asBuiltJoints, "asBuilt")):
        for i in range(collection.count):
            joint = collection.item(i)
            try:
                if joint.isSuppressed:
                    continue
                joints.append([part_path(joint.occurrenceOne, scope, names), part_path(joint.occurrenceTwo, scope, names), label])
            except Exception:
                unreadable += 1  # a joint to geometry Fusion cannot name here; it constrains nothing the plan reads
    for i in range(component.rigidGroups.count):
        group = component.rigidGroups.item(i)
        try:
            members = group.occurrences
            groups.append([part_path(members.item(k), scope, names) for k in range(members.count)])
        except Exception:
            unreadable += 1
    result = {"target": value, "document": doc.name, "scope": scope,
              "scopeName": start.component.name if start else doc.name,
              "units": units, "joints": joints, "rigidGroups": groups, "frameBodies": component.bRepBodies.count,
              "hiddenParts": hidden, "unreadableLinks": unreadable, "maxBodies": cap,
              "unit": "mm", "space": "root", "boxes": "axis-aligned in root space, from each part's bodies and its children's"}
    require(len(json.dumps(result)) <= MAX_LAYOUT_BYTES, "layout_budget", "The layout exceeds its 2 MiB bound; pass the path of a subassembly as scope.")
    return result


def ray_cast(app, args):
    started = _clock()
    _, design, value = read_target(app, args)
    paths, rays = args.get("occurrences") or [], args.get("rays") or []
    budget = float(args.get("budgetSeconds", DEFAULT_BUDGET))
    for n, ray in enumerate(rays):
        require(all(0 <= i < len(paths) for i in ray["against"]), "invalid_input",
                "rays[%d].against names an index past the %d occurrences." % (n, len(paths)))
        require(any(v != 0 for v in ray["direction"]), "invalid_input", "rays[%d].direction must be nonzero." % n)
    parts, missing = {}, []

    def part(index):
        """The occurrence's component and the inverse of its root-space transform, once per call."""
        if index not in parts:
            occurrence = occurrence_at(design, paths[index])
            if occurrence is None or occurrence.fullPathName != paths[index]:
                parts[index] = None
                missing.append(index)
            else:
                inverse = occurrence.transform2.copy()
                inverse.invert()
                parts[index] = (occurrence.component, inverse)
        return parts[index]

    hits, failed, casts, size, stopped = [], 0, 0, 2, None
    for ray in rays:
        # every call answers at least one ray, so a caller sending the rest always gets further
        if hits and _clock() - started > budget:
            stopped = "budget"
            break
        x, y, z = ray["origin"]
        dx, dy, dz = ray["direction"]
        longest = ray["length"]
        row = []
        for index in ray["against"]:
            found_part = part(index)
            if found_part is None:
                continue
            component, inverse = found_part
            try:
                origin = adsk.core.Point3D.create(x / 10, y / 10, z / 10)
                direction = adsk.core.Vector3D.create(dx, dy, dz)
                origin.transformBy(inverse)
                direction.transformBy(inverse)
                points = adsk.core.ObjectCollection.create()
                # a component's own bodies report invisible (their occurrence is what shows): do not filter on it
                found = component.findBRepUsingRay(origin, direction, adsk.fusion.BRepEntityTypes.BRepBodyEntityType, -1.0, False, points)
                casts += 1
                near = [origin.distanceTo(points.item(k)) * 10 for k in range(found.count)]
                near = [d for d in near if d <= longest]
                if near:
                    row.append([index, round(min(near), 2)])
            except Exception:
                failed += 1
        row.sort(key=lambda hit: hit[1])
        row_bytes = len(json.dumps(row)) + 1
        if hits and size + row_bytes > MAX_REPLY_BYTES:
            stopped = "reply_bytes"
            break
        size += row_bytes
        hits.append(row)
    result = {"target": value, "unit": "mm", "space": "root", "hits": hits, "cast": len(hits), "complete": len(hits) == len(rays),
              "missing": sorted(missing), "failed": failed, "casts": casts, "seconds": round(_clock() - started, 3)}
    if stopped:
        result["stopped"] = stopped
        result["hint"] = ("Answered the first %d of %d rays (%s); send the rest in another call."
                          % (len(hits), len(rays), "budgetSeconds reached" if stopped == "budget" else "the reply reached 1 MiB"))
    return result


OPERATIONS = {"assembly_layout": assembly_layout, "ray_cast": ray_cast}
WRITES = {"assembly_layout"}  # journaled, model read-only: a big assembly's first layout outlives the relay's 60 s
