"""Typed component definitions, placements, joint motion and interference."""
import math

import adsk.core
import adsk.fusion

from .core import (identity, in_root_context, items, number, point_mm, ref, require, resolve, same, target)
from .assembly_state import GROUP, MOTION_SCALES, joint_data, joint_motion, metadata, rigid_group_data
from .sketches import collection, component, named


def hybrid(design):
    enum = getattr(adsk.fusion, "DesignIntentTypes", None)
    require(enum is not None and hasattr(design, "designIntent"), "unsupported_capability", "This operation requires an inspectable Hybrid design intent.")
    require(design.designIntent == enum.HybridDesignIntentType, "hybrid_design_required", "Internal components require Hybrid design intent. Create a new Hybrid design; existing intent is never changed implicitly.")


def transform(spec=None):
    spec = spec or {}
    matrix = adsk.core.Matrix3D.create()
    angle = number(spec.get("angle", 0), "angle")
    axis = spec.get("axis", [0, 0, 1])
    vector = adsk.core.Vector3D.create(*axis)
    require(vector.length > 1e-9, "invalid_input", "Rotation axis must be nonzero.")
    require(matrix.setToRotation(math.radians(angle), vector, adsk.core.Point3D.create(0, 0, 0)), "invalid_transform", "Fusion refused rotation transform.")
    matrix.translation = adsk.core.Vector3D.create(*[number(n, "position") / 10 for n in spec.get("position", [0, 0, 0])])
    return matrix


def occurrence_data(occ, value):
    matrix = list(occ.transform2.asArray())
    for index in (3, 7, 11):
        matrix[index] *= 10
    return {"ref": ref(occ, value), "component": {"ref": ref(occ.component, value), "id": identity(occ.component), **metadata(occ.component)},
            "path": occ.fullPathName, "transform": matrix, "transformLayout": "row-major", "translationUnit": "mm",
            "space": "assembly_context", "grounded": occ.isGrounded, "groundedToParent": occ.isGroundToParent}


def require_transform(actual, expected, message):
    require_transform_values(list(actual.asArray()), list(expected.asArray()), message)


def require_transform_values(actual_values, expected_values, message):
    require(len(actual_values) == len(expected_values) == 16
            and all(math.isfinite(a) and math.isfinite(b) and abs(a-b) < 1e-8
                    for a, b in zip(actual_values, expected_values)), "placement_refused", message)


def created_occurrence_contexts(design, parent, occurrence):
    """Find placed root proxies without reading a definition-local token."""
    if same(parent, design.rootComponent):
        return [occurrence]
    native = getattr(occurrence, "nativeObject", None) or occurrence
    all_occurrences = items(design.rootComponent.allOccurrences)
    contexts = [o for o in all_occurrences if same(getattr(o, "nativeObject", None) or o, native)]
    if not contexts:
        parents = [o for o in all_occurrences if same(o.component, parent)]
        contexts = [native.createForAssemblyContext(o) for o in parents]
    require(contexts and all(o is not None and o.isValid for o in contexts), "occurrence_context_failed", "Could not obtain placed root-context instances for the new occurrence.")
    return contexts


def context_placements(design, parent, contexts, expected):
    local_values = list(expected.asArray())
    require_transform_values(local_values, local_values, "Requested transform must be a complete finite matrix.")
    if same(parent, design.rootComponent):
        return [(context, local_values) for context in contexts]
    parents_by_path = {}
    for candidate in items(design.rootComponent.allOccurrences):
        if same(candidate.component, parent):
            parents_by_path.setdefault(candidate.fullPathName, []).append(candidate)
    placements, seen_parents = [], set()
    for context in contexts:
        parent_context = context.assemblyContext
        parent_matches = parents_by_path.get(parent_context.fullPathName, []) if parent_context is not None else []
        require(parent_context is not None and same(parent_context.component, parent)
                and len(parent_matches) == 1 and same(parent_matches[0], parent_context),
                "occurrence_context_failed", "New occurrence's parent must identify an exact root-context instance.")
        require(parent_context.fullPathName not in seen_parents, "occurrence_context_failed", "New occurrence has duplicate parent contexts.")
        seen_parents.add(parent_context.fullPathName)
        parent_values = list(parent_context.transform2.asArray())
        require_transform_values(parent_values, parent_values, "Parent transform must be a complete finite matrix.")
        # Row-major matrices: parent root pose composed with requested local pose.
        world_values = [sum(parent_values[row*4+k] * local_values[k*4+column] for k in range(4))
                        for row in range(4) for column in range(4)]
        placements.append((context, world_values))
    require(seen_parents == set(parents_by_path), "occurrence_context_failed", "New occurrence must resolve in every placed parent instance.")
    return placements


def place_created_occurrence(design, parent, occurrence, expected):
    # Paste New may leave a pending pose and reused parent instances can inherit
    # the source pose. Transform overrides must use root proxies, never the local
    # occurrence. Apply every instance and capture once before shared recompute.
    contexts = created_occurrence_contexts(design, parent, occurrence)
    for context, values in context_placements(design, parent, contexts, expected):
        matrix = expected
        if not same(parent, design.rootComponent):
            matrix = adsk.core.Matrix3D.create()
            require(matrix.setWithArray(values), "placement_failed", "Could not construct the requested root-context placement.")
        context.transform2 = matrix
        require_transform_values(list(context.transform2.asArray()), values, "Fusion refused the requested occurrence transform.")
    captured = False
    if design.snapshots.hasPendingSnapshot:
        require(design.snapshots.add() is not None, "placement_failed", "Could not capture the new occurrence positions.")
        captured = True
    return captured


def created_occurrence_result(design, parent, occurrence, value, expected=None):
    """Return and verify every placed instance in root assembly coordinates.

    The local transform can retain its base pose even when captured positions
    are correct. Verify the computed root proxies, each against its exact parent.
    """
    contexts = created_occurrence_contexts(design, parent, occurrence)
    if expected is not None:
        for context, values in context_placements(design, parent, contexts, expected):
            require_transform_values(list(context.transform2.asArray()), values,
                                     "Recompute did not retain the requested placement in every parent instance.")
    rows = [occurrence_data(o, value) for o in contexts]
    return {"occurrence": rows[0], "affectedOccurrences": rows,
            "placementSpace": "parent_component", "referenceSpace": "root_assembly",
            "placementScope": "all_current_parent_instances", "editScope": "parent_component_definition"}


def parent_component(design, value, args):
    return component(design, value, {"component": args["parent"]} if "parent" in args else {})


def component_create(design, value, args):
    hybrid(design)
    parent = parent_component(design, value, args)
    expected = transform(args.get("transform"))
    occ = parent.occurrences.addNewComponent(expected)
    require(occ and occ.isValid, "creation_failed", "Component creation failed.")
    named(occ.component, args)
    if args.get("metadata"):
        apply_metadata(occ.component, args["metadata"])
    captured = place_created_occurrence(design, parent, occ, expected)
    return lambda fresh: {**created_occurrence_result(design, parent, occ, fresh, expected), "positionCaptured": captured}


def occurrence_insert(design, value, args):
    hybrid(design)
    parent = parent_component(design, value, args)
    comp = resolve(design, value, args["component"], ("Component",))
    require(not same(comp, design.rootComponent), "invalid_component", "Root component cannot be instanced.")
    # Avoid cyclic containment before asking the kernel to insert.
    descendants = [comp] + [o.component for o in items(comp.allOccurrences)]
    require(not any(same(parent, c) for c in descendants), "cyclic_assembly", "A component cannot contain itself or its ancestor.")
    method = parent.occurrences.addNewComponentCopy if args.get("independentCopy", False) else parent.occurrences.addExistingComponent
    expected = transform(args.get("transform"))
    occ = method(comp, expected)
    require(occ and occ.isValid, "creation_failed", "Occurrence insertion failed.")
    captured = place_created_occurrence(design, parent, occ, expected)
    return lambda fresh: {**created_occurrence_result(design, parent, occ, fresh, expected), "positionCaptured": captured}


def initial_position_editable(occ):
    """Can Fusion edit this occurrence's initial position? (The API spells the property isVaild...)"""
    for name in ("isVaildForEditInitialPosition", "isValidForEditInitialPosition"):
        try:
            value = getattr(occ, name, None)
        except RuntimeError:
            return False
        if value is not None:
            return bool(value)
    return False


def edit_initial_position(design, occ, matrix):
    """Set the occurrence's initial position. Fusion edits only the last occurrence in the timeline in
    place ("Cannot be edited before rolling back"), so roll the marker to just after this occurrence's
    creation, edit it there and put the marker back, as the Edit Initial Position command does."""
    try:
        occ.initialTransform = matrix
        return True
    except RuntimeError:
        pass
    try:
        item = occ.timelineObject
    except RuntimeError:
        return False
    if item is None:
        return False
    timeline = design.timeline
    marker = timeline.markerPosition
    try:
        if not item.rollTo(False):
            return False
        occ.initialTransform = matrix
        return True
    except RuntimeError:
        return False
    finally:
        timeline.markerPosition = marker


def occurrence_update(design, value, args):
    require(any(k in args for k in ("transform", "grounded", "groundedToParent")), "invalid_input", "Occurrence update requires a placement or grounding change.")
    occ = resolve(design, value, args["occurrence"], ("Occurrence",))
    if "grounded" in args:
        occ.isGrounded = args["grounded"]
    if "groundedToParent" in args:
        occ.isGroundToParent = args["groundedToParent"]
    placed = None
    if "transform" in args:
        require(not occ.isGrounded and not occ.isGroundToParent, "grounded_occurrence", "Unground the occurrence explicitly before moving it.")
        expected = transform(args["transform"])
        method = args.get("method", "auto")
        # Edit Initial Position: the placement the occurrence was created with, so no Position snapshot
        # joins the timeline. Fusion allows it for a root-level occurrence without joints or patterns
        # (the property's name is misspelled in the API).
        if method != "capture" and initial_position_editable(occ):
            if edit_initial_position(design, occ, expected):
                placed = "initial_position"
            else:
                require(method == "auto", "initial_position_refused", "Fusion refused to edit this occurrence's initial position.")
        require(placed or method != "initial_position", "initial_position_unavailable",
                "Fusion cannot edit this occurrence's initial position (nested, patterned or jointed): use method capture.")
        if placed is None:
            occ.transform2 = expected
            placed = "capture"
        actual = occ.transform2.asArray()
        require(all(abs(a-b) < 1e-8 for a,b in zip(actual, expected.asArray())), "placement_refused", "Constraints prevented the requested occurrence transform.")
        if design.snapshots.hasPendingSnapshot:
            require(design.snapshots.add() is not None, "placement_failed", "Could not capture occurrence position in the timeline.")
            placed = "capture"
    require("grounded" not in args or occ.isGrounded == args["grounded"], "grounding_refused", "Grounding did not match the request.")
    require("groundedToParent" not in args or occ.isGroundToParent == args["groundedToParent"], "grounding_refused", "Parent grounding did not match the request.")
    def result(fresh):
        if "transform" in args:
            expected = transform(args["transform"])
            require(all(abs(a-b) < 1e-8 for a,b in zip(occ.transform2.asArray(), expected.asArray())),
                    "placement_refused", "Recompute did not retain the requested occurrence transform.")
        require("grounded" not in args or occ.isGrounded == args["grounded"], "grounding_refused", "Recompute did not retain grounding.")
        require("groundedToParent" not in args or occ.isGroundToParent == args["groundedToParent"], "grounding_refused", "Recompute did not retain parent grounding.")
        return {"occurrence": occurrence_data(occ, fresh), **({"placement": placed} if placed else {})}
    return result


def occurrence_delete(design, value, args):
    occ = resolve(design, value, args["occurrence"], ("Occurrence",))
    path = occ.fullPathName
    require(occ.deleteMe(), "delete_failed", "Fusion refused occurrence deletion.")
    return {"deletedPath": path}


def apply_metadata(comp, changes):
    for key in ("name", "partNumber", "description"):
        if key in changes:
            setattr(comp, key, changes[key])
    import json
    for key in changes.keys() - {"name", "partNumber", "description"}:
        require(comp.attributes.add(GROUP, key, json.dumps(changes[key], sort_keys=True)) is not None, "metadata_failed", "Could not persist " + key)


def component_metadata(design, value, args):
    comp = component(design, value, args)
    apply_metadata(comp, args["changes"])
    return lambda fresh: {"component": {"ref": ref(comp, fresh), **metadata(comp)}}


def require_root_context(design, entity):
    context = entity if entity.objectType.endswith("::Occurrence") else getattr(entity, "assemblyContext", None)
    if context is not None:
        require(in_root_context(design, context),
                "root_context_required", "Assembly analysis and joints require a root-context occurrence reference from inspect.")
    else:
        owner = getattr(entity, "parentComponent", None) or getattr(entity, "component", None)
        if hasattr(entity, "body"):
            owner = entity.body.parentComponent
        if hasattr(entity, "parentSketch"):
            owner = entity.parentSketch.parentComponent
        require(owner is not None and same(owner, design.rootComponent), "occurrence_context_required", "Non-root geometry requires a specific occurrence-context reference.")


def joint_geometry(design, value, raw):
    entity = resolve(design, value, raw["entity"], ("SketchPoint", "ConstructionPoint", "BRepVertex", "BRepEdge", "BRepFace"))
    require_root_context(design, entity)
    kind = entity.objectType.split("::")[-1]
    default = "middle" if kind == "BRepFace" and entity.geometry.objectType.endswith(("::Cylinder", "::Cone")) else "center"
    key = getattr(adsk.fusion.JointKeyPointTypes, {"center": "CenterKeyPoint", "start": "StartKeyPoint", "middle": "MiddleKeyPoint", "end": "EndKeyPoint"}[raw.get("keyPoint", default)])
    if kind in ("SketchPoint", "ConstructionPoint", "BRepVertex"):
        result = adsk.fusion.JointGeometry.createByPoint(entity)
    elif kind == "BRepEdge":
        result = adsk.fusion.JointGeometry.createByCurve(entity, key)
    elif entity.geometry.objectType.endswith("::Plane"):
        result = adsk.fusion.JointGeometry.createByPlanarFace(entity, None, key)
    else:
        result = adsk.fusion.JointGeometry.createByNonPlanarFace(entity, key)
    require(result is not None, "invalid_joint_geometry", "Fusion could not infer the joint origin.")
    return result


def set_limits(motion, name, values, scale):
    require(hasattr(motion, name), "wrong_joint_type", "Joint does not support " + name)
    limits = getattr(motion, name)
    minimum = values.get("minimum", limits.minimumValue * scale)
    maximum = values.get("maximum", limits.maximumValue * scale)
    minimum_enabled = values.get("minimumEnabled", "minimum" in values or limits.isMinimumValueEnabled)
    maximum_enabled = values.get("maximumEnabled", "maximum" in values or limits.isMaximumValueEnabled)
    require(not (minimum_enabled and maximum_enabled) or minimum <= maximum, "invalid_limits", "Minimum joint limit must not exceed maximum.")
    rest = values.get("rest", limits.restValue * scale)
    rest_enabled = values.get("restEnabled", "rest" in values or limits.isRestValueEnabled)
    require(not rest_enabled or (not minimum_enabled or rest >= minimum) and (not maximum_enabled or rest <= maximum), "invalid_limits", "Rest position must lie within enabled limits.")
    limits.isMinimumValueEnabled = False
    limits.isMaximumValueEnabled = False
    for key in ("minimum", "maximum", "rest"):
        if key in values:
            setattr(limits, key + "Value", number(values[key], key) / scale)
    limits.isMinimumValueEnabled = minimum_enabled
    limits.isMaximumValueEnabled = maximum_enabled
    limits.isRestValueEnabled = rest_enabled


def configure_joint(design, joint, args):
    if "name" in args:
        joint.name = args["name"]
    if "suppressed" in args:
        joint.isSuppressed = args["suppressed"]
    motion = joint_motion(joint)
    for prefix, scale in MOTION_SCALES.items():
        if prefix + "Limits" in args:
            set_limits(motion, prefix + "Limits", args[prefix + "Limits"], scale)
        if prefix + "Value" in args:
            require(hasattr(motion, prefix + "Value"), "wrong_joint_type", "Joint does not support " + prefix)
            number_value = number(args[prefix + "Value"], prefix + "Value") / scale
            limits = getattr(motion, prefix + "Limits")
            require(not limits.isMinimumValueEnabled or number_value >= limits.minimumValue, "joint_limit_exceeded", "Drive value is below minimum.")
            require(not limits.isMaximumValueEnabled or number_value <= limits.maximumValue, "joint_limit_exceeded", "Drive value is above maximum.")
            setattr(motion, prefix + "Value", number_value)
    # Joint value and suppression setters alter pending assembly state. Capture
    # once, after all requested changes, so computeAll replays that state instead
    # of resetting it. Rest values and limit flags stay intact.
    captured = False
    if ("suppressed" in args or any(prefix + "Value" in args for prefix in MOTION_SCALES)) and design.snapshots.hasPendingSnapshot:
        require(design.snapshots.add() is not None, "joint_capture_failed", "Could not capture the edited assembly state.")
        captured = True
    return captured


def joint_result(joint, value, args):
    result = joint_data(joint, value)
    for field in ("name", "suppressed"):
        if field in args:
            require(result[field] == args[field], "joint_edit_refused", "Joint did not retain requested " + field + ".")
    for prefix in MOTION_SCALES:
        for suffix in ("Value", "Limits"):
            key = prefix + suffix
            if key in args:
                require(key in result, "joint_readback_failed", "Joint motion readback is unavailable.")
                if suffix == "Value":
                    require(math.isclose(result[key], args[key], rel_tol=1e-8, abs_tol=1e-6), "joint_drive_refused", "Joint did not reach the requested drive value.")
                else:
                    expected_limits = dict(args[key])
                    for field in ("minimum", "maximum", "rest"):
                        if field in expected_limits and field + "Enabled" not in expected_limits:
                            expected_limits[field + "Enabled"] = True
                    for field, expected in expected_limits.items():
                        actual = result[key][field]
                        require(actual == expected if isinstance(expected, bool) else math.isclose(actual, expected, rel_tol=1e-8, abs_tol=1e-6), "joint_limits_refused", "Joint limit did not read back: " + field)
    return {"joint": result}


def rigid_group_result(group, value, args):
    result = rigid_group_data(group, value)
    for field in ("name", "suppressed"):
        if field in args:
            require(result[field] == args[field], "rigid_group_edit_refused", "Rigid group did not retain requested " + field + ".")
    return {"rigidGroup": result}


def joint_create(design, value, args):
    first, second = [resolve(design, value, args[key], ("Occurrence",)) for key in ("occurrenceOne", "occurrenceTwo")]
    require(not same(first, second), "invalid_joint", "Joint requires two distinct occurrences.")
    for occurrence in (first, second):
        require_root_context(design, occurrence)
    geometry = joint_geometry(design, value, args["geometry"]) if args["type"] != "rigid" else None
    group = design.rootComponent.asBuiltJoints
    inp = group.createInput(first, second, geometry)
    method = getattr(inp, {"rigid": "setAsRigidJointMotion", "revolute": "setAsRevoluteJointMotion", "slider": "setAsSliderJointMotion", "cylindrical": "setAsCylindricalJointMotion", "pin_slot": "setAsPinSlotJointMotion", "planar": "setAsPlanarJointMotion", "ball": "setAsBallJointMotion"}[args["type"]])
    direction = getattr(adsk.fusion.JointDirections, args.get("axis", "z").upper() + "AxisJointDirection")
    if args["type"] == "rigid":
        ok = method()
    elif args["type"] == "ball":
        ok = method(adsk.fusion.JointDirections.ZAxisJointDirection, adsk.fusion.JointDirections.XAxisJointDirection)
    elif args["type"] == "pin_slot":
        require(args.get("axis", "z") != args.get("slideAxis", "x"), "invalid_joint_axes", "Pin-slot rotation and slide directions must differ.")
        ok = method(direction, getattr(adsk.fusion.JointDirections, args.get("slideAxis", "x").upper() + "AxisJointDirection"))
    else:
        ok = method(direction)
    require(ok, "joint_failed", "Joint motion definition failed.")
    joint = group.add(inp)
    require(joint and joint.isValid, "joint_failed", "As-built joint creation failed.")
    captured = configure_joint(design, joint, args)
    return lambda fresh: {**joint_result(joint, fresh, args), "positionCaptured": captured}


def joint_edit(design, value, args):
    require(any(k in args for k in ("name", "suppressed")) or any(prefix + suffix in args for prefix in MOTION_SCALES for suffix in ("Limits", "Value")), "invalid_input", "Joint edit requires a change.")
    joint = resolve(design, value, args["joint"], ("Joint", "AsBuiltJoint"))
    from .motion_links import coupling_snapshot, verify_coupling
    driven = any(prefix + "Value" in args for prefix in MOTION_SCALES)
    coupling = coupling_snapshot(design) if driven else []
    captured = configure_joint(design, joint, args)
    return lambda fresh: {**joint_result(joint, fresh, args), "positionCaptured": captured,
                          "motionLinksVerified": verify_coupling(coupling)}


def joint_delete(design, value, args):
    joint = resolve(design, value, args["joint"], ("Joint", "AsBuiltJoint"))
    name = joint.name
    require(joint.deleteMe(), "delete_failed", "Fusion refused joint deletion.")
    return {"deletedJoint": name}


def rigid_group_create(design, value, args):
    occurrences = [resolve(design, value, raw, ("Occurrence",)) for raw in args["occurrences"]]
    require(len({identity(o) for o in occurrences}) == len(occurrences), "duplicate_selection", "Rigid group occurrences must be distinct.")
    for occurrence in occurrences:
        require_root_context(design, occurrence)
    group = design.rootComponent.rigidGroups.add(collection(occurrences), args.get("includeChildren", True))
    named(group, args)
    return lambda fresh: rigid_group_result(group, fresh, args)


def rigid_group_edit(design, value, args):
    require("name" in args or "suppressed" in args, "invalid_input", "Rigid group edit requires a change.")
    group = resolve(design, value, args["group"], ("RigidGroup",))
    if "name" in args:
        group.name = args["name"]
    if "suppressed" in args:
        group.isSuppressed = args["suppressed"]
    captured = False
    if "suppressed" in args and design.snapshots.hasPendingSnapshot:
        require(design.snapshots.add() is not None, "rigid_group_capture_failed", "Could not capture the edited rigid group state.")
        captured = True
    return lambda fresh: {**rigid_group_result(group, fresh, args), "positionCaptured": captured}


def rigid_group_delete(design, value, args):
    group = resolve(design, value, args["group"], ("RigidGroup",))
    name = group.name
    require(group.deleteMe(), "delete_failed", "Fusion refused rigid group deletion.")
    return {"deletedRigidGroup": name}


def assembly_properties(app, args):
    _, design, value = target(app, args)
    entity = resolve(design, value, args["entity"], ("Component", "Occurrence", "BRepBody")) if args.get("entity") else design.rootComponent
    props = entity.getPhysicalProperties(adsk.fusion.CalculationAccuracy.HighCalculationAccuracy)
    volume = props.volume * 1000
    is_occurrence = entity.objectType.endswith("::Occurrence")
    return {"target": value, "entity": ref(entity, value), "mass": props.mass, "volume": volume,
            "area": props.area * 100, "centerOfMass": point_mm(props.centerOfMass),
            "averageDensity": props.mass / volume if volume else None,
            "units": {"mass": "kg", "volume": "mm3", "area": "mm2", "centerOfMass": "mm", "averageDensity": "kg/mm3"},
            "scope": "body" if entity.objectType.endswith("::BRepBody") else "component_subtree",
            "space": "assembly_context" if is_occurrence or getattr(entity, "assemblyContext", None) else "component",
            "materialBasis": "Fusion assigned/default material; supplier provenance does not certify modeled material density"}


def interference(app, args):
    _, design, value = target(app, args)
    entities = [resolve(design, value, r, ("Occurrence", "BRepBody")) for r in args["entities"]]
    selected_bodies = set()
    walked = 0
    def body_instances(entity, depth=0):
        nonlocal walked
        walked += 1
        require(depth <= 100 and walked <= 20000, "inspection_budget", "Interference assembly exceeds traversal budget.")
        if entity.objectType.endswith("::BRepBody"):
            native = getattr(entity, "nativeObject", None) or entity
            context = getattr(entity, "assemblyContext", None)
            return {(identity(native), context.fullPathName if context else "<root>")}
        result = set()
        for body in items(entity.bRepBodies):
            walked += 1
            require(walked <= 20000, "inspection_budget", "Interference assembly exceeds shared body traversal budget.")
            native = getattr(body, "nativeObject", None) or body
            result.add((identity(native), entity.fullPathName))
        for child in items(entity.childOccurrences):
            result.update(body_instances(child, depth + 1))
        require(len(result) <= 20000, "inspection_budget", "Interference assembly exceeds body budget.")
        return result
    for index, entity in enumerate(entities):
        require(not any(same(entity, earlier) for earlier in entities[:index]), "duplicate_selection", "Interference entities must be distinct.")
        require_root_context(design, entity)
        instances = body_instances(entity)
        require(not selected_bodies.intersection(instances), "overlapping_selection", "Selections include the same physical body instance more than once; choose disjoint occurrences/bodies.")
        selected_bodies.update(instances)
        require(len(selected_bodies) <= 20000, "inspection_budget", "Interference selection exceeds physical body instance budget.")
    inp = design.createInterferenceInput(collection(entities))
    inp.areCoincidentFacesIncluded = args.get("includeTouching", False)
    results = design.analyzeInterference(inp)
    require(results is not None, "interference_failed", "Fusion did not return an interference result.")
    rows = []
    for result in items(results):
        body = result.interferenceBody
        volume = body.volume * 1000 if body else 0.0
        rows.append({"entityOne": ref(result.entityOne, value), "entityTwo": ref(result.entityTwo, value),
                     "volume": volume, "classification": "overlap" if volume > args.get("volumeTolerance", 1e-6) else "touching"})
    return {"target": value, "complete": True, "results": rows, "volumeUnit": "mm3", "bodiesCreated": False}


OPERATIONS = {"component_create": component_create, "occurrence_insert": occurrence_insert,
              "occurrence_update": occurrence_update, "occurrence_delete": occurrence_delete,
              "component_metadata": component_metadata, "joint_create": joint_create, "joint_edit": joint_edit,
              "joint_delete": joint_delete, "rigid_group_create": rigid_group_create,
              "rigid_group_edit": rigid_group_edit, "rigid_group_delete": rigid_group_delete}
READS = {"interference": interference, "assembly_properties": assembly_properties}
