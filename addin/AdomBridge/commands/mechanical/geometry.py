"""Bounded geometry selection, engineering measurements and construction axes."""
import math

import adsk.core
import adsk.fusion

from .core import (MAX_ENTITIES, UNITS, in_root_context, items, number, point_mm, ref, require,
                   resolve, same, target)
from .readback import concrete_geometry, geometry_data
from .sketches import component, named, point


def native(entity):
    return getattr(entity, "nativeObject", None) or entity


def owner(entity):
    entity = native(entity)
    if hasattr(entity, "body"):
        return entity.body.parentComponent
    if hasattr(entity, "parentSketch"):
        return entity.parentSketch.parentComponent
    return getattr(entity, "parentComponent", None) or getattr(entity, "component", None)


def root_entity(design, entity):
    """Never guess which instance of a non-root definition the caller means."""
    if entity.objectType.endswith("::Occurrence"):
        require(in_root_context(design, entity),
                "root_context_required", "Occurrence must be resolved in the root assembly context.")
    else:
        context = getattr(entity, "assemblyContext", None)
        require(context is not None or same(owner(entity), design.rootComponent),
                "occurrence_required", "Root-space geometry in a child component requires an occurrence-context reference.")
        if context is not None:
            require(in_root_context(design, context),
                    "root_context_required", "Geometry context must be a root assembly occurrence.")
    return entity


def finite_point(value):
    require(value is not None, "measurement_failed", "Native measurement has no witness point.")
    result = point_mm(value)
    require(all(math.isfinite(v) for v in result), "measurement_failed", "Native witness point is not finite.")
    return result


def minimum(manager, first, second):
    result = manager.measureMinimumDistance(first, second)
    require(result is not None and result.isValid, "measurement_failed", "Native minimum-distance measurement failed.")
    distance = number(result.value, "native distance") * 10
    require(distance >= 0, "measurement_failed", "Native minimum distance is negative.")
    points = [finite_point(result.positionOne), finite_point(result.positionTwo)]
    separation = math.dist(*points)
    require(math.isclose(distance, separation, abs_tol=1e-5, rel_tol=1e-7),
            "measurement_failed", "Native distance does not agree with its witness positions.")
    return distance, points


MEASURABLE = ("Occurrence", "BRepBody", "BRepFace", "BRepEdge", "BRepVertex",
              "ConstructionPlane", "ConstructionAxis", "ConstructionPoint", "SketchPoint",
              "SketchLine", "SketchArc", "SketchCircle", "SketchEllipse", "SketchEllipticalArc",
              "SketchFittedSpline", "SketchControlPointSpline", "SketchFixedSpline")
POINT_TYPES = ("BRepVertex", "ConstructionPoint", "SketchPoint")


def angle_direction(entity):
    # Sketch geometry is in sketch coordinates; worldGeometry includes the
    # sketch transform and the resolved occurrence context.
    geometry = concrete_geometry(entity.worldGeometry if entity.objectType.endswith("::SketchLine")
                                 else entity.geometry)
    kind = geometry.objectType.split("::")[-1]
    if kind == "Plane":
        vector, meaning = geometry.normal, "normal"
    elif kind == "Line3D":
        vector, meaning = geometry.startPoint.vectorTo(geometry.endPoint), "line"
    else:
        require(kind == "InfiniteLine3D", "measurement_failed", "Cannot establish the native angle direction.")
        vector, meaning = geometry.direction, "line"
    return {"kind": meaning, "vector": unit_vector([vector.x, vector.y, vector.z])}


def angle_witness(entities, value, points):
    a = [points[0][i] - points[1][i] for i in range(3)]
    b = [points[2][i] - points[1][i] for i in range(3)]
    if math.hypot(*a) > 1e-12 and math.hypot(*b) > 1e-12:
        expected = math.degrees(math.acos(max(-1, min(1, sum(x*y for x, y in zip(unit_vector(a), unit_vector(b)))))))
        require(math.isclose(value, expected, abs_tol=1e-5), "measurement_failed", "Angle disagrees with native witness positions.")
        return {"method": "native_points", "nativePointWitnessDegenerate": False}
    # Fusion returns three zero points for valid parallel-plane angles. Keep
    # those original points, and establish parallelism from native geometry.
    # Never accept a degenerate three-point angle or synthesize witness points.
    require(len(entities) == 2 and all(e.objectType.split("::")[-1] not in POINT_TYPES for e in entities),
            "invalid_direction", "Point angle witness is degenerate.")
    require(min(abs(value), abs(value - 180)) <= 1e-5,
            "measurement_failed", "Degenerate native points do not establish this angle.")
    directions = [angle_direction(entity) for entity in entities]
    dot = sum(x*y for x, y in zip(directions[0]["vector"], directions[1]["vector"]))
    cosine = min(1, abs(dot))
    deviation = math.degrees(math.acos(cosine) if directions[0]["kind"] == directions[1]["kind"]
                             else math.asin(cosine))
    require(deviation <= 1e-5, "measurement_failed", "Degenerate angle points disagree with native geometry directions.")
    return {"method": "native_geometry_directions", "nativePointWitnessDegenerate": True,
            "directions": directions, "parallelDeviationDegrees": deviation}


def measure(app, args):
    _, design, value = target(app, args)
    entities = [root_entity(design, resolve(design, value, item, MEASURABLE)) for item in args["entities"]]
    mode = args["type"]
    if mode == "distance":
        require(len(entities) == 2, "invalid_input", "Distance requires exactly two entities.")
        result, witnesses = minimum(app.measureManager, *entities)
        validation = {"method": "native_points"}
        unit = "mm"
    else:
        kinds = [e.objectType.split("::")[-1] for e in entities]
        if any(kind in POINT_TYPES for kind in kinds):
            require(len(entities) == 3 and all(kind in POINT_TYPES for kind in kinds),
                    "invalid_input", "Point angles require exactly three points, with the second as apex.")
        else:
            require(len(entities) == 2, "invalid_input", "Line/plane angles require two entities.")
            for entity, kind in zip(entities, kinds):
                geometry = getattr(entity, "geometry", None)
                require(kind in ("ConstructionPlane", "ConstructionAxis", "SketchLine") or
                        kind == "BRepEdge" and geometry.objectType.endswith("::Line3D") or
                        kind == "BRepFace" and geometry.objectType.endswith("::Plane"),
                        "unsupported_geometry", "Angle inputs must be linear or planar, or three points.")
        measured = app.measureManager.measureAngle(*entities)
        require(measured is not None and measured.isValid, "measurement_failed", "Native angle measurement failed.")
        result = math.degrees(number(measured.value, "native angle"))
        witnesses = [finite_point(getattr(measured, name)) for name in ("positionOne", "positionTwo", "positionThree")]
        require(0 <= result <= 180 + 1e-7, "measurement_failed", "Native angle is outside 0..180 degrees.")
        validation = angle_witness(entities, result, witnesses)
        unit = "deg"
    return {"target": value, "type": mode, "value": result, "unit": unit,
            "frame": {"space": "root", "lengthUnit": "mm"}, "witnessPoints": witnesses,
            "witnessValidation": validation,
            "entities": [ref(entity, value) for entity in entities]}


def unit_vector(vector):
    length = math.sqrt(sum(number(v, "direction") ** 2 for v in vector))
    require(length > 1e-12, "invalid_direction", "Direction or angle witness is degenerate.")
    return [v / length for v in vector]


def transformed_direction(vector, context):
    xyz = [vector.x, vector.y, vector.z]
    if context is not None:
        matrix = context.transform2.asArray()
        xyz = [sum(matrix[row*4 + col] * xyz[col] for col in range(3)) for row in range(3)]
    return unit_vector(xyz)


def query_row(entity, kind, context, value):
    base = native(entity)
    geometry = getattr(base, "geometry", None)
    if geometry is not None: geometry = concrete_geometry(geometry)
    row = {"ref": ref(base, value, context), "type": geometry.objectType.split("::")[-1] if geometry else "degenerate"}
    direction = None
    if kind == "faces":
        row["area"] = base.area * 100
        if geometry.objectType.endswith("::Plane"):
            ok, direction = base.evaluator.getNormalAtPoint(base.pointOnFace)
            require(ok, "geometry_unavailable", "Could not evaluate planar face normal.")
    elif kind == "edges":
        row["length"] = base.length * 10
        if geometry and geometry.objectType.endswith("::Line3D"):
            direction = geometry.startPoint.vectorTo(geometry.endPoint)
    if geometry and hasattr(geometry, "radius"):
        row["radius"] = geometry.radius * 10
    if direction:
        row["direction"] = transformed_direction(direction, context)
    return row


def near_geometry(manager, entity, kind, pick, frame_context, design):
    if kind == "vertices":
        candidate = entity.createForAssemblyContext(frame_context) if frame_context else entity
        if frame_context is None and not same(owner(entity), design.rootComponent):
            # MeasureManager rejects native B-Rep in a child definition; vertices are transient points.
            candidate = entity.geometry
    else:
        # Faces and edges are measured on a temporary copy of the native entity. MeasureManager
        # rejects native B-Rep in a child definition, and on an occurrence-context proxy it measures
        # to the face's underlying surface or the edge's underlying curve: a coplanar face or a
        # collinear edge beside the pick read 0 mm away and passed the filter. The copy keeps the
        # trimmed geometry; root space moves it by the occurrence's transform. Nothing is added to
        # the design.
        temporary = adsk.fusion.TemporaryBRepManager.get()
        candidate = temporary.copy(native(entity))
        require(candidate is not None, "measurement_failed", "Could not copy component geometry for measurement.")
        if frame_context is not None:
            require(temporary.transform(candidate, frame_context.transform2), "measurement_failed",
                    "Could not move the copied geometry into the root frame.")
    distance, witnesses = minimum(manager, point(pick), candidate)
    # Native temporary-body measurements can reverse the two witness positions.
    # Bind the pick by coordinates instead of assuming positionTwo is on the CAD.
    matching = [i for i, witness in enumerate(witnesses) if math.dist(pick, witness) <= 1e-5]
    require(matching, "measurement_failed", "Native distance has no witness at the requested pick.")
    return distance, witnesses[1 - matching[0]]


def geometry_query(app, args):
    _, design, value = target(app, args)
    body = resolve(design, value, args["body"], ("BRepBody",))
    space, kind, filters = args["space"], args["kind"], args.get("filters", {})
    if space == "root":
        root_entity(design, body)
    context = getattr(body, "assemblyContext", None)
    frame_context = context if space == "root" else None
    source = native(body)
    selected, collection = [], getattr(source, kind)
    require(collection.count <= MAX_ENTITIES, "inspection_budget", "Body topology exceeds query budget.")
    allowed_filters = {"type", "near"} | ({"area", "radius", "direction"} if kind == "faces" else
                      {"length", "radius", "direction"} if kind == "edges" else set())
    require(not (set(filters) - allowed_filters), "invalid_input", "Filter is not supported for the selected topology kind.")
    for key in ("area", "radius", "length"):
        if key in filters:
            limits = filters[key]
            require(limits.get("min", 0) <= limits.get("max", math.inf), "invalid_input", "Filter minimum exceeds maximum.")
    direction_filter = filters.get("direction")
    desired = unit_vector(direction_filter["vector"]) if direction_filter else None
    for entity in items(collection):
        # Reference context and filter frame are separate: component-space
        # queries on an instance still return references to that exact instance.
        row = query_row(entity, kind, frame_context, value)
        row["ref"] = ref(entity, value, context)
        if "type" in filters and row["type"] != filters["type"]: continue
        if any(key in filters and (key not in row or not filters[key].get("min", 0) <= row[key] <= filters[key].get("max", math.inf))
               for key in ("area", "radius", "length")): continue
        if desired:
            if "direction" not in row: continue
            dot = sum(a*b for a, b in zip(desired, row["direction"]))
            if direction_filter.get("orientation", "same") == "parallel": dot = abs(dot)
            if dot < math.cos(math.radians(direction_filter.get("tolerance", 0.01))) - 1e-12: continue
        if "near" in filters:
            distance, nearest = near_geometry(app.measureManager, entity, kind, filters["near"]["point"], frame_context, design)
            if distance > filters["near"]["tolerance"] + 1e-7: continue
            row.update(distanceToPick=distance, nearestPoint=nearest)
        selected.append(row)
        require(len(selected) <= args.get("maxResults", 256), "query_budget", "Query result exceeds maxResults; narrow the filters.")
    if args.get("selection", "one") == "one":
        require(len(selected) == 1, "selection_empty" if not selected else "ambiguous_selection",
                "Geometry query must select exactly one entity.", matchCount=len(selected))
    return {"target": value, "matches": selected, "count": len(selected), "complete": True,
            "frame": {"space": space, "component": ref(source.parentComponent, value),
                      "occurrencePath": context.fullPathName if context else None}, "units": UNITS}


def axis_data(axis, value=None):
    result = {"name": axis.name, "geometry": geometry_data(axis.geometry), "space": "component",
              "healthState": int(axis.healthState), "message": axis.errorOrWarningMessage,
              "parametric": axis.isParametric, "lengthUnit": "mm"}
    if value: result["ref"] = ref(axis, value)
    return result


def construction_axis(design, value, args):
    comp = component(design, value, args)
    methods = {"edge": ("setByEdge", ("BRepEdge", "SketchLine", "SketchArc", "SketchCircle"), 1),
               "circular_face": ("setByCircularFace", ("BRepFace",), 1),
               "two_planes": ("setByTwoPlanes", ("BRepFace", "ConstructionPlane"), 2),
               "two_points": ("setByTwoPoints", POINT_TYPES, 2)}
    method, allowed, count = methods[args["type"]]
    require(len(args["entities"]) == count, "invalid_input", "Incorrect number of axis references.")
    entities = [resolve(design, value, item, allowed) for item in args["entities"]]
    require(count != 2 or not same(entities[0], entities[1]), "duplicate_entity", "Axis references must identify distinct entities.")
    cross_component = any(not same(owner(entity), comp) or getattr(entity, "assemblyContext", None) for entity in entities)
    inp = comp.constructionAxes.createInput()
    if cross_component:
        for entity in entities: root_entity(design, entity)
        if not same(comp, design.rootComponent):
            require(args.get("creationOccurrence"), "creation_occurrence_required", "Cross-component axes need an explicit occurrence of the target component.")
            context = root_entity(design, resolve(design, value, args["creationOccurrence"], ("Occurrence",)))
            require(same(context.component, comp), "wrong_component", "Creation occurrence does not instantiate the target component.")
            inp.creationOccurrence = context
        else:
            require(not args.get("creationOccurrence"), "invalid_input", "Root axes do not use a creation occurrence.")
    else:
        require(not args.get("creationOccurrence"), "invalid_input", "Native same-component axis references do not use a creation occurrence.")
    require(getattr(inp, method)(*entities), "axis_failed", "Fusion rejected the construction axis definition.")
    axis = named(comp.constructionAxes.add(inp), args)
    return lambda fresh: {"axis": axis_data(axis, fresh)}


READS = {"geometry_query": geometry_query, "measure": measure}
OPERATIONS = {"construction_axis": construction_axis}
