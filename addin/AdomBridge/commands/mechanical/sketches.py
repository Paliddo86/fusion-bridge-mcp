"""Typed sketches, dimensions, constraints and construction planes (mm/deg)."""
import math

import adsk.core
import adsk.fusion

from .core import items, number, ref, require, resolve, same
from .readback import sketch_data


def collection(entities):
    result = adsk.core.ObjectCollection.create()
    for entity in entities:
        result.add(entity)
    return result


def point(value):
    require(isinstance(value, list) and len(value) in (2, 3), "invalid_input", "Point must be [x,y] or [x,y,z] in mm.")
    coords = [number(v, "coordinate") / 10 for v in value]
    return adsk.core.Point3D.create(*(coords + [0] if len(coords) == 2 else coords))


def quantity(design, value, unit="mm", positive=False):
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        expression = str(number(value, "quantity", positive)) + (" " + unit if unit else "")
    else:
        require(isinstance(value, str) and value.strip(), "invalid_input", "Quantity must be a number or unit-bearing expression.")
        expression = value
    require(design.unitsManager.isValidExpression(expression, unit), "invalid_expression", "Quantity does not match " + (unit or "unitless") + ".")
    evaluated = design.unitsManager.evaluateExpression(expression, unit)
    require(math.isfinite(evaluated) and (not positive or evaluated > 0), "invalid_quantity", "Quantity must be finite" + (" and positive." if positive else "."))
    return adsk.core.ValueInput.createByString(expression)


def component(design, value, args):
    if args.get("component"):
        return resolve(design, value, args["component"], ("Component",))
    return design.rootComponent


def plane(design, value, comp, selector):
    if isinstance(selector, str):
        require(selector in ("xy", "xz", "yz"), "invalid_plane", "Plane must be xy, xz, yz or a guarded reference.")
        return getattr(comp, {"xy": "xYConstructionPlane", "xz": "xZConstructionPlane", "yz": "yZConstructionPlane"}[selector])
    entity = resolve(design, value, selector, ("ConstructionPlane", "BRepFace"))
    owner = entity.body.parentComponent if hasattr(entity, "body") else entity.component
    require(getattr(entity, "assemblyContext", None) is None and same(owner, comp), "wrong_component", "Plane reference must belong to this native component definition.")
    return entity


def axis(design, value, comp, selector):
    if isinstance(selector, str):
        require(selector in ("x", "y", "z"), "invalid_axis", "Axis must be x, y, z or a guarded reference.")
        return getattr(comp, selector + "ConstructionAxis")
    entity = resolve(design, value, selector, ("ConstructionAxis", "SketchLine", "BRepEdge"))
    owner = entity.body.parentComponent if hasattr(entity, "body") else (entity.parentSketch.parentComponent if hasattr(entity, "parentSketch") else entity.component)
    require(getattr(entity, "assemblyContext", None) is None and same(owner, comp), "wrong_component", "Axis reference must belong to this native component definition.")
    return entity


def named(entity, args):
    require(entity is not None and entity.isValid, "creation_failed", "Fusion did not return a valid entity.")
    if args.get("name"):
        entity.name = args["name"]
    return entity


def get_sketch(design, value, args):
    sketch = resolve(design, value, args["sketch"], ("Sketch",))
    require(getattr(sketch, "assemblyContext", None) is None, "definition_reference_required", "Sketch edits require a native definition reference.")
    return sketch


def sketch_create(design, value, args):
    comp = component(design, value, args)
    support = plane(design, value, comp, args.get("plane", "xy"))
    sketch = named(comp.sketches.add(support), args)
    return lambda fresh: {"sketch": sketch_data(sketch, fresh)}


def sketch_geometry(design, value, args):
    sketch = get_sketch(design, value, args)
    curves = sketch.sketchCurves
    for item in args["items"]:
        kind = item["type"]
        created = []
        if kind == "point":
            created = [sketch.sketchPoints.add(point(item["position"]))]
        elif kind == "line":
            created = [curves.sketchLines.addByTwoPoints(point(item["start"]), point(item["end"]))]
        elif kind == "rectangle":
            created = items(curves.sketchLines.addTwoPointRectangle(point(item["start"]), point(item["end"])))
        elif kind == "circle":
            radius = number(item["radius"], "radius", True) / 10
            created = [curves.sketchCircles.addByCenterRadius(point(item["center"]), radius)]
        elif kind == "arc":
            sweep = math.radians(number(item["sweep"], "sweep"))
            require(0 < abs(sweep) < 2 * math.pi, "invalid_input", "Arc sweep magnitude must be between 0 and 360 deg.")
            created = [curves.sketchArcs.addByCenterStartSweep(point(item["center"]), point(item["start"]), sweep)]
        elif kind == "polygon":
            center = point(item["center"])
            count = item["sides"]
            require(isinstance(count, int) and not isinstance(count, bool) and 3 <= count <= 128, "invalid_input", "Polygon sides must be 3..128.")
            radius = number(item["radius"], "radius", True) / 10
            angle = math.radians(number(item.get("angle", 0), "angle"))
            vertices = [adsk.core.Point3D.create(center.x + radius * math.cos(angle + 2 * math.pi * i / count),
                                                center.y + radius * math.sin(angle + 2 * math.pi * i / count), center.z) for i in range(count)]
            created = [curves.sketchLines.addByTwoPoints(vertices[i], vertices[(i + 1) % count]) for i in range(count)]
        elif kind == "slot":
            start, end = point(item["start"]), point(item["end"])
            require(abs(start.z - end.z) < 1e-9, "invalid_input", "Slot center points must share their sketch-space Z coordinate.")
            radius = number(item["width"], "width", True) / 20
            dx, dy = end.x - start.x, end.y - start.y
            length = math.hypot(dx, dy)
            require(length > 1e-8, "invalid_input", "Slot center points must differ.")
            ox, oy = -dy * radius / length, dx * radius / length
            p = lambda center, sign: adsk.core.Point3D.create(center.x + sign * ox, center.y + sign * oy, center.z)
            a, b, cc, d = p(start, 1), p(end, 1), p(end, -1), p(start, -1)
            created = [curves.sketchLines.addByTwoPoints(a, b),
                       curves.sketchArcs.addByCenterStartSweep(end, b, -math.pi),
                       curves.sketchLines.addByTwoPoints(cc, d),
                       curves.sketchArcs.addByCenterStartSweep(start, d, -math.pi)]
        elif kind == "spline":
            spline = curves.sketchFittedSplines.add(collection([point(p) for p in item["points"]]))
            if item.get("closed"):
                spline.isClosed = True
            created = [spline]
        else:
            require(False, "unsupported_geometry", "Unsupported sketch primitive.")
        for entity in created:
            require(entity and entity.isValid, "creation_failed", "Sketch primitive creation failed.")
            if "construction" in item:
                require(hasattr(entity, "isConstruction"), "invalid_input", "Construction applies to curves.")
                entity.isConstruction = item["construction"]
    return lambda fresh: {"sketch": sketch_data(sketch, fresh)}


def sketch_entities(design, value, args, sketch):
    entities = [resolve(design, value, entity) for entity in args["entities"]]
    for entity in entities:
        require(hasattr(entity, "parentSketch") and same(entity.parentSketch, sketch), "wrong_sketch", "Every selected entity must belong to the specified sketch.")
    return entities


def sketch_constraint(design, value, args):
    sketch = get_sketch(design, value, args)
    entities = sketch_entities(design, value, args, sketch)
    methods = {"horizontal": ("addHorizontal", 1), "vertical": ("addVertical", 1),
               "coincident": ("addCoincident", 2), "parallel": ("addParallel", 2),
               "perpendicular": ("addPerpendicular", 2), "tangent": ("addTangent", 2),
               "equal": ("addEqual", 2), "concentric": ("addConcentric", 2),
               "collinear": ("addCollinear", 2), "midpoint": ("addMidPoint", 2),
               "symmetry": ("addSymmetry", 3)}
    if args["type"] == "fix":
        require(len(entities) == 1, "invalid_input", "fix requires one entity.")
        entities[0].isFixed = True
    else:
        method, count = methods[args["type"]]
        require(len(entities) == count, "invalid_input", "Constraint requires " + str(count) + " entities.")
        result = getattr(sketch.geometricConstraints, method)(*entities)
        require(result is not None, "constraint_failed", "Fusion did not create the constraint.")
    return lambda fresh: {"sketch": sketch_data(sketch, fresh)}


def sketch_dimension(design, value, args):
    sketch = get_sketch(design, value, args)
    entities = sketch_entities(design, value, args, sketch)
    position, kind = point(args["position"]), args["type"]
    dims = sketch.sketchDimensions
    if kind in ("distance", "horizontal", "vertical"):
        require(len(entities) == 2, "invalid_input", "Distance dimension requires two sketch points.")
        orientation = getattr(adsk.fusion.DimensionOrientations, {"distance": "AlignedDimensionOrientation", "horizontal": "HorizontalDimensionOrientation", "vertical": "VerticalDimensionOrientation"}[kind])
        dimension = dims.addDistanceDimension(entities[0], entities[1], orientation, position)
    elif kind == "angle":
        require(len(entities) == 2, "invalid_input", "Angle dimension requires two lines.")
        dimension = dims.addAngularDimension(entities[0], entities[1], position)
    else:
        require(len(entities) == 1, "invalid_input", "Radius/diameter dimension requires one circle or arc.")
        dimension = getattr(dims, "addRadialDimension" if kind == "radius" else "addDiameterDimension")(entities[0], position)
    require(dimension is not None, "dimension_failed", "Fusion did not create the dimension.")
    expression = args["expression"]
    quantity(design, expression, "deg" if kind == "angle" else "mm")
    dimension.parameter.expression = str(expression) + (" deg" if kind == "angle" else " mm") if isinstance(expression, (int, float)) else expression
    if args.get("parameterName"):
        dimension.parameter.name = args["parameterName"]
    return lambda fresh: {"sketch": sketch_data(sketch, fresh), "parameterName": dimension.parameter.name}


def sketch_project(design, value, args):
    sketch = get_sketch(design, value, args)
    projected = []
    for reference in args["entities"]:
        entity = resolve(design, value, reference, ("BRepEdge", "BRepFace", "BRepVertex", "SketchPoint", "SketchLine", "SketchCircle", "SketchArc"))
        projected.extend(items(sketch.project(entity)))
    require(projected, "projection_failed", "No sketch entities were projected.")
    return lambda fresh: {"sketch": sketch_data(sketch, fresh), "projected": [ref(e, fresh) for e in projected]}


def sketch_edit(design, value, args):
    sketch = get_sketch(design, value, args)
    entity = resolve(design, value, args["entity"])
    require(same(entity.parentSketch, sketch), "wrong_sketch", "Entity is not in this sketch.")
    changes = args["changes"]
    for key, val in changes.items():
        if key in ("construction", "fixed"):
            setattr(entity, "isConstruction" if key == "construction" else "isFixed", val)
        elif key == "radius":
            entity.radius = number(val, "radius", True) / 10
        elif key in ("position", "start", "end", "center"):
            handle = entity if key == "position" else getattr(entity, {"start": "startSketchPoint", "end": "endSketchPoint", "center": "centerSketchPoint"}[key])
            wanted, old = point(val), handle.geometry
            require(handle.move(adsk.core.Vector3D.create(wanted.x - old.x, wanted.y - old.y, wanted.z - old.z)), "edit_failed", "Sketch point move was refused.")
        else:
            require(False, "invalid_input", "Unknown sketch edit property.")
    return lambda fresh: {"sketch": sketch_data(sketch, fresh)}


def plane_create(design, value, args):
    comp = component(design, value, args)
    base = plane(design, value, comp, args.get("plane", "xy"))
    inp = comp.constructionPlanes.createInput()
    if args.get("type", "offset") == "offset":
        require(inp.setByOffset(base, quantity(design, args["offset"])), "plane_failed", "Offset plane input failed.")
    else:
        rotation = axis(design, value, comp, args["axis"])
        require(inp.setByAngle(rotation, quantity(design, args["angle"], "deg"), base), "plane_failed", "Angled plane input failed.")
    created = named(comp.constructionPlanes.add(inp), args)
    return lambda fresh: {"plane": {"ref": ref(created, fresh), "name": created.name}}


OPERATIONS = {"sketch_create": sketch_create, "sketch_geometry": sketch_geometry,
              "sketch_constraint": sketch_constraint, "sketch_dimension": sketch_dimension,
              "sketch_project": sketch_project, "sketch_edit": sketch_edit, "plane_create": plane_create}
