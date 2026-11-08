"""Solid feature operations; selection always resolves against the guarded model."""
import math

import adsk.core
import adsk.fusion

from .core import items, parameter_data, ref, require, resolve, same
from .readback import feature_data, topology_data
from .sketches import axis, collection, component, named, plane, point, quantity


def owner(entity):
    if hasattr(entity, "body"):
        return entity.body.parentComponent
    if hasattr(entity, "parentComponent"):
        return entity.parentComponent
    if hasattr(entity, "parentSketch"):
        return entity.parentSketch.parentComponent
    return None


def selected(design, value, args, name, types, comp):
    raw = args.get(name, [])
    require(isinstance(raw, list) and raw, "invalid_selection", name + " requires at least one reference.")
    result = [resolve(design, value, r, types) for r in raw]
    for entity in result:
        require(getattr(entity, "assemblyContext", None) is None, "definition_reference_required", "Feature authoring requires native definition references.")
        require(same(owner(entity), comp), "wrong_component", "Selected entity belongs to another component; pass its component reference.")
    return result


def operation(value):
    return getattr(adsk.fusion.FeatureOperations, {"new_body": "NewBodyFeatureOperation", "join": "JoinFeatureOperation",
                   "cut": "CutFeatureOperation", "intersect": "IntersectFeatureOperation"}[value])


def factory(features, name):
    result = getattr(features, name, None)
    require(result is not None, "unsupported_capability", name + " is unavailable in this Fusion build.")
    return result


PATTERN_INPUTS = ("bodies", "features", "faces", "occurrences")
# Fusion's pattern compute options: identical repeats the first result (fast), optimized is fastest but
# can drop instances that touch other geometry, adjust recomputes every instance (slowest)
COMPUTE_OPTIONS = {"identical": "IdenticalPatternCompute", "optimized": "OptimizedPatternCompute", "adjust": "AdjustPatternCompute"}


def occurrence_parent(design, occurrence):
    """(the component that owns an occurrence, the occurrence as that component sees it). A root-context
    proxy belongs to its context's component; otherwise it is the root's own child or Fusion names its source."""
    context = getattr(occurrence, "assemblyContext", None)
    if context is not None:
        return context.component, occurrence.nativeObject
    root = design.rootComponent
    if any(same(child, occurrence) for child in items(root.occurrences)):
        return root, occurrence
    source = getattr(occurrence, "sourceComponent", None)
    require(source is not None, "unsupported_capability", "Fusion did not name the component that owns this occurrence.")
    return source, occurrence


def pattern_inputs(design, value, args, comp):
    """What a pattern repeats, (kind, entities, component to build it in): exactly one of bodies, features,
    faces or occurrences. Occurrences are patterned in their shared parent (Fusion's pattern components),
    so the part stays one definition; the rest pattern inside the target component as before."""
    given = [key for key in PATTERN_INPUTS if key in args]
    require(len(given) <= 1, "mixed_pattern_inputs", "A pattern repeats one kind of entity; pass only one of " + ", ".join(given) + ".")
    require(given, "invalid_selection", "A pattern needs bodies, features, faces or occurrences.")
    key = given[0]
    if key == "occurrences":
        raw = args["occurrences"]
        require(isinstance(raw, list) and raw, "invalid_selection", "occurrences requires at least one reference.")
        owned = [occurrence_parent(design, resolve(design, value, r, ("Occurrence",))) for r in raw]
        parent = owned[0][0]
        require(all(same(owner_comp, parent) for owner_comp, _ in owned), "mixed_parents",
                "Patterned occurrences must share one parent component.")
        require(not args.get("component") or same(comp, parent), "wrong_component",
                "Occurrences are patterned in their parent component; omit component or pass that parent.")
        return key, [occurrence for _, occurrence in owned], parent
    entities = selected(design, value, args, key, {"bodies": ("BRepBody",), "faces": ("BRepFace",), "features": ()}[key], comp)
    if key == "features":
        require(all(entity.objectType.endswith("Feature") and getattr(entity, "timelineObject", None) is not None for entity in entities),
                "wrong_entity_type", "Patterned features must be timeline features (an extrude, a hole, a fillet).")
    return key, entities, comp


def compute_option(inp, args, key):
    """Set the compute option on a body, face or feature pattern (identical unless asked); component
    patterns have none. Returns the option set, or None."""
    options = getattr(adsk.fusion, "PatternComputeOptions", None)
    if key == "occurrences" or options is None or not hasattr(inp, "patternComputeOption"):
        return None
    name = args.get("computeOption", "identical")
    inp.patternComputeOption = getattr(options, COMPUTE_OPTIONS[name])
    return name


def compute_name(feature, requested):
    """The compute option Fusion reports on the created pattern, by name (the requested one when unreadable)."""
    options = getattr(adsk.fusion, "PatternComputeOptions", None)
    try:
        actual = getattr(feature, "patternComputeOption", None)
    except RuntimeError:
        actual = None
    if options is None or actual is None:
        return requested
    return next((name for name, attr in COMPUTE_OPTIONS.items() if getattr(options, attr, None) == actual), requested)


def pattern_input(design, value, args, comp, kind, q):
    """(feature group, input, what is patterned, compute option) for a rectangular or circular pattern."""
    key, entities, comp = pattern_inputs(design, value, args, comp)
    features = comp.features
    if kind == "rectangular_pattern":
        if any(name in args for name in ("axis2", "count2", "spacing2")):
            require("count2" in args and "spacing2" in args, "invalid_input", "Second pattern direction requires count2 and spacing2; axis2 defaults to y.")
        group = factory(features, "rectangularPatternFeatures")
        inp = group.createInput(collection(entities), axis(design, value, comp, args["axis"]),
                                q("count", ""), q("spacing"), adsk.fusion.PatternDistanceType.SpacingPatternDistanceType)
        inp.quantityTwo = quantity(design, 1, "", True)
        inp.isSymmetricInDirectionOne = False
        inp.isSymmetricInDirectionTwo = False
        if "count2" in args:
            require(inp.setDirectionTwo(axis(design, value, comp, args.get("axis2", "y")), q("count2", ""), q("spacing2")), "pattern_failed", "Second pattern direction was refused.")
    else:
        group = factory(features, "circularPatternFeatures")
        inp = group.createInput(collection(entities), axis(design, value, comp, args["axis"]))
        inp.quantity = q("count", "")
        inp.totalAngle = quantity(design, args.get("angle", 360), "deg", True)
        inp.isSymmetric = False
    return group, inp, key, compute_option(inp, args, key)


def feature_create(design, value, args):
    comp = component(design, value, args)
    features, kind = comp.features, args["type"]
    q = lambda key, unit="mm", positive=True: quantity(design, args[key], unit, positive)
    select = lambda key, types: selected(design, value, args, key, types, comp)
    operation_name = args.get("operation", "new_body")
    if operation_name == "join" and kind in ("extrude", "revolve", "loft", "sweep"):
        require(comp.bRepBodies.count == 1, "ambiguous_join", "Join requires one existing body in the component; create a new body then use combine for explicit multi-body selection.")
    if kind == "extrude":
        profiles = select("profiles", ("Profile",))
        group = factory(features, "extrudeFeatures")
        inp = group.createInput(collection(profiles), operation(operation_name))
        if args.get("direction", "positive") == "symmetric":
            require(inp.setSymmetricExtent(q("distance"), True, quantity(design, args.get("taper", 0), "deg")), "invalid_extent", "Symmetric extrusion extent was refused.")
        else:
            extent = adsk.fusion.DistanceExtentDefinition.create(q("distance"))
            direction = getattr(adsk.fusion.ExtentDirections, "NegativeExtentDirection" if args.get("direction") == "negative" else "PositiveExtentDirection")
            require(inp.setOneSideExtent(extent, direction, quantity(design, args.get("taper", 0), "deg")), "invalid_extent", "Extrusion extent was refused.")
        if operation_name in ("cut", "intersect"):
            inp.participantBodies = select("bodies", ("BRepBody",))
    elif kind == "revolve":
        profiles = select("profiles", ("Profile",))
        group = factory(features, "revolveFeatures")
        inp = group.createInput(collection(profiles), axis(design, value, comp, args["axis"]), operation(operation_name))
        require(inp.setAngleExtent(False, q("angle", "deg")), "invalid_extent", "Revolve angle was refused.")
        if operation_name in ("cut", "intersect"):
            inp.participantBodies = select("bodies", ("BRepBody",))
    elif kind == "fillet":
        group = factory(features, "filletFeatures")
        inp = group.createInput()
        inp.edgeSetInputs.addConstantRadiusEdgeSet(collection(select("edges", ("BRepEdge",))), q("radius"), args.get("tangentChain", False))
    elif kind == "chamfer":
        group = factory(features, "chamferFeatures")
        require(hasattr(group, "createInput2"), "unsupported_capability", "Modern chamfer input API unavailable.")
        inp = group.createInput2()
        inp.chamferEdgeSets.addEqualDistanceChamferEdgeSet(collection(select("edges", ("BRepEdge",))), q("distance"), args.get("tangentChain", False))
    elif kind == "shell":
        group = factory(features, "shellFeatures")
        inp = group.createInput(collection(select("faces", ("BRepFace",))), args.get("tangentChain", False))
        inp.insideThickness = q("thickness")
    elif kind == "hole":
        group = factory(features, "holeFeatures")
        inp = group.createSimpleInput(q("diameter"))
        require(inp.setPositionBySketchPoints(collection(select("points", ("SketchPoint",)))), "hole_position_failed", "Hole positions were refused.")
        require(inp.setDistanceExtent(q("depth")), "invalid_extent", "Hole depth was refused.")
        inp.participantBodies = select("bodies", ("BRepBody",))
    elif kind == "loft":
        group = factory(features, "loftFeatures")
        inp = group.createInput(operation(operation_name))
        profiles = select("profiles", ("Profile",))
        require(len(profiles) >= 2, "invalid_selection", "Loft requires at least two profiles.")
        for profile in profiles:
            inp.loftSections.add(profile)
        inp.isSolid = True
        if operation_name in ("cut", "intersect"):
            inp.participantBodies = select("bodies", ("BRepBody",))
    elif kind == "sweep":
        group = factory(features, "sweepFeatures")
        profiles = select("profiles", ("Profile",))
        require(len(profiles) == 1, "invalid_selection", "Sweep requires exactly one profile.")
        path = adsk.fusion.Path.create(collection(select("path", ("SketchLine", "SketchArc", "SketchFittedSpline", "BRepEdge"))), adsk.fusion.ChainedCurveOptions.noChainedCurves)
        require(path is not None, "invalid_path", "Sweep path must form a connected curve chain.")
        inp = group.createInput(profiles[0], path, operation(operation_name))
        if operation_name in ("cut", "intersect"):
            inp.participantBodies = select("bodies", ("BRepBody",))
    elif kind == "combine":
        group = factory(features, "combineFeatures")
        target_body = resolve(design, value, args["body"], ("BRepBody",))
        require(getattr(target_body, "assemblyContext", None) is None, "definition_reference_required", "Combine requires a native target body reference.")
        require(same(owner(target_body), comp), "wrong_component", "Target body is in another component.")
        tools = select("tools", ("BRepBody",))
        require(not any(same(target_body, tool) for tool in tools), "invalid_selection", "Target cannot also be a tool body.")
        inp = group.createInput(target_body, collection(tools))
        require(operation_name in ("join", "cut", "intersect"), "invalid_input", "Combine operation must be join, cut or intersect.")
        inp.operation = operation(operation_name)
        inp.isKeepToolBodies = args.get("keepTools", True)
    elif kind == "split":
        group = factory(features, "splitBodyFeatures")
        bodies = select("bodies", ("BRepBody",))
        require(len(bodies) == 1, "invalid_selection", "Split requires one body.")
        tool = plane(design, value, comp, args["plane"])
        inp = group.createInput(bodies[0], tool, args.get("extend", True))
    elif kind == "move":
        group = factory(features, "moveFeatures")
        require(hasattr(group, "createInput2"), "unsupported_capability", "Modern move input API unavailable.")
        inp = group.createInput2(collection(select("bodies", ("BRepBody",))))
        matrix = adsk.core.Matrix3D.create()
        if args.get("angle", 0):
            vector = args.get("axis", [0, 0, 1])
            rotation_axis = adsk.core.Vector3D.create(*vector)
            require(rotation_axis.length > 1e-9, "invalid_input", "Rotation axis must be nonzero.")
            angle_value = design.unitsManager.evaluateExpression(str(args["angle"]) + " deg", "deg")
            require(matrix.setToRotation(angle_value, rotation_axis, point(args.get("origin", [0, 0, 0]))), "invalid_transform", "Rotation matrix creation failed.")
        translation = point(args.get("translation", [0, 0, 0]))
        existing = matrix.translation
        matrix.translation = adsk.core.Vector3D.create(existing.x + translation.x, existing.y + translation.y, existing.z + translation.z)
        require(inp.defineAsFreeMove(matrix), "invalid_transform", "Move transform was refused.")
    elif kind == "mirror":
        group = factory(features, "mirrorFeatures")
        inp = group.createInput(collection(select("bodies", ("BRepBody",))), plane(design, value, comp, args["plane"]))
        inp.isCombine = False
    elif kind in ("rectangular_pattern", "circular_pattern"):
        group, inp, patterned, requested = pattern_input(design, value, args, comp, kind, q)
    elif kind == "draft":
        group = factory(features, "draftFeatures")
        inp = group.createInput(select("faces", ("BRepFace",)), plane(design, value, comp, args["plane"]), args.get("tangentChain", False))
        require(inp.setSingleAngle(False, q("angle", "deg", False)), "draft_failed", "Draft angle was refused.")
    elif kind == "thread":
        group = factory(features, "threadFeatures")
        create_info = getattr(getattr(adsk.fusion, "ThreadInfo", None), "create", None)
        require(callable(create_info), "unsupported_capability", "ThreadInfo.create is unavailable.")
        info = create_info(False, args["internal"], args["threadType"], args["designation"], args["threadClass"], True)
        require(info is not None, "invalid_thread", "Requested thread standard/designation/class is unavailable.")
        inp = group.createInput(collection(select("faces", ("BRepFace",))), info)
        inp.isModeled = args.get("modeled", False)
        inp.isFullLength = True
    else:
        require(False, "unsupported_feature", "This feature type is not supported.")
    feature = named(group.add(inp), args)
    if kind in ("rectangular_pattern", "circular_pattern"):
        elements = getattr(feature, "patternElements", None)
        pattern = {"patterned": patterned, "computeOption": compute_name(feature, requested),
                   "instanceCount": elements.count if elements is not None else None}
        return lambda fresh: {"feature": feature_data(feature, fresh), "pattern": pattern,
                              "bodies": [topology_data(body, fresh) for body in items(feature.bodies)]}
    return lambda fresh: {"feature": feature_data(feature, fresh),
                           "bodies": [topology_data(body, fresh) for body in items(feature.bodies)]}


def feature_edit(design, value, args):
    require(any(key in args for key in ("name", "suppressed", "parameters")), "invalid_input", "Specify a feature property to edit.")
    feature = resolve(design, value, args["feature"])
    timeline = getattr(feature, "timelineObject", None)
    require(timeline is not None, "wrong_entity_type", "Feature must have a timeline object.")
    if "name" in args:
        feature.name = args["name"]
    if "suppressed" in args:
        timeline.isSuppressed = args["suppressed"]
    for name, expression in args.get("parameters", {}).items():
        parameter = design.allParameters.itemByName(name)
        require(parameter is not None and same(getattr(parameter, "createdBy", None), feature), "wrong_parameter", "Parameter is not owned by this feature: " + name)
        require(design.unitsManager.isValidExpression(expression, parameter.unit), "invalid_expression", "Invalid parameter expression.")
        parameter.expression = expression
    return lambda fresh: {"feature": feature_data(feature, fresh)}


def entity_delete(design, value, args):
    entity = resolve(design, value, args["entity"])
    kind = entity.objectType.split("::")[-1]
    require(kind in ("Sketch", "SketchLine", "SketchCircle", "SketchArc", "SketchPoint", "SketchFittedSpline", "ConstructionPlane") or kind.endswith("Feature"),
            "unsupported_delete", "Delete supports sketches, sketch geometry, construction planes and timeline features.")
    description = {"name": getattr(entity, "name", None), "type": entity.objectType}
    timeline = getattr(entity, "timelineObject", None)
    if timeline:
        require(timeline.isDeletable, "delete_refused", "Fusion reports this timeline entity cannot be deleted independently.")
        require(timeline.deleteObject(), "delete_failed", "Feature deletion failed.")
    else:
        require(entity.deleteMe(), "delete_failed", "Entity deletion failed.")
    return {"deleted": description}


def parameter_batch(design, value, args):
    changed = []
    for change in args["changes"]:
        name, action = change["name"], change["action"]
        parameter = design.allParameters.itemByName(name)
        if action == "create":
            require(parameter is None, "parameter_exists", "Parameter exists: " + name)
            parameter = design.userParameters.add(name, quantity(design, change["expression"], change["unit"]), change["unit"], change.get("comment", ""))
            require(parameter is not None, "parameter_create_failed", "Parameter creation failed.")
        else:
            require(parameter is not None, "parameter_missing", "Parameter not found: " + name)
            if action == "delete":
                user = design.userParameters.itemByName(name)
                require(user is not None, "delete_refused", "Only user parameters can be deleted.")
                require(user.deleteMe(), "delete_refused", "Parameter is in use or cannot be deleted.")
                changed.append({"name": name, "deleted": True})
                continue
            expression = change["expression"]
            require(design.unitsManager.isValidExpression(expression, parameter.unit), "invalid_expression", "Invalid expression for " + name)
            parameter.expression = expression
        changed.append(parameter_data(design, parameter))
    return {"parameters": changed}


OPERATIONS = {"feature_create": feature_create, "feature_edit": feature_edit,
              "entity_delete": entity_delete, "parameter_batch": parameter_batch}
