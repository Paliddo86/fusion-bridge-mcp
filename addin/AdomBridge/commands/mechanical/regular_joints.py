"""Geometry-solving joints and reusable origins; all writes use PTransaction."""
from contextlib import contextmanager
import math

import adsk.core
import adsk.fusion

from . import compute
from .core import concrete, identity, items, ref, require, resolve, same
from .geometry import native, owner, root_entity
from .joint_state import (geometry_state, matrix_data, origin_data,
                          regular_state)
from .sketches import component, named, quantity
from .assemblies import configure_joint, joint_result

GEOMETRY_TYPES = ("BRepFace", "BRepEdge", "BRepVertex", "SketchPoint", "ConstructionPoint",
                  "SketchLine", "SketchArc", "SketchCircle", "SketchEllipse", "SketchEllipticalArc")
MOTION_METHODS = {"rigid": "Rigid", "revolute": "Revolute", "slider": "Slider", "cylindrical": "Cylindrical",
                  "pin_slot": "PinSlot", "planar": "Planar", "ball": "Ball"}


@contextmanager
def before_timeline(design, entity):
    """Restore the exact caller marker, including on native setter exceptions."""
    position = design.timeline.markerPosition
    timeline = entity.timelineObject
    require(timeline is not None and timeline.isValid, "timeline_unavailable", "This edit requires a valid parametric timeline item.")
    try:
        require(timeline.rollTo(True), "timeline_roll_failed", "Could not roll immediately before the joint item.")
        yield
    finally:
        design.timeline.markerPosition = position
        require(design.timeline.markerPosition == position, "timeline_restore_failed", "Could not restore the original timeline marker.")


def geometry_from_entity(entity, key_name=None, edge=None):
    entity, edge = concrete(entity), concrete(edge)
    kind = entity.objectType.split("::")[-1]
    geometry = getattr(entity, "geometry", None)
    shape = geometry.objectType.split("::")[-1] if geometry is not None else ""
    default = "middle" if kind == "BRepFace" and shape in ("Cylinder", "Cone") else "center"
    key_name = key_name or default
    key = getattr(adsk.fusion.JointKeyPointTypes, {"center": "CenterKeyPoint", "start": "StartKeyPoint",
                 "middle": "MiddleKeyPoint", "end": "EndKeyPoint"}[key_name])
    factory = adsk.fusion.JointGeometry
    if kind in ("BRepVertex", "SketchPoint", "ConstructionPoint"):
        require(key_name == "center" and edge is None, "invalid_joint_geometry", "Point attachments have no edge/key-point choice.")
        result = factory.createByPoint(entity)
    elif kind == "BRepEdge" or kind.startswith("Sketch"):
        require(edge is None, "invalid_joint_geometry", "An edge qualifier belongs only to a planar face.")
        require(shape in ("Line3D", "Arc3D", "Circle3D", "Ellipse3D", "EllipticalArc3D"),
                "unsupported_joint_geometry", "Use a line or analytical arc/circle/ellipse attachment.")
        require(shape not in ("Circle3D", "Ellipse3D") or key_name == "center", "invalid_joint_geometry", "Closed circles/ellipses require center.")
        result = factory.createByCurve(entity, key)
    elif kind == "BRepFace" and shape == "Plane":
        require(edge is not None or key_name == "center", "invalid_joint_geometry", "A planar face requires center or one of its explicit edges.")
        if edge is not None:
            require(any(same(native(item), native(edge)) for item in items(entity.edges)), "invalid_joint_geometry", "Selected edge does not belong to this face.")
        result = factory.createByPlanarFace(entity, edge, key)
    else:
        require(kind == "BRepFace" and shape in ("Cylinder", "Cone", "Sphere", "Torus") and edge is None,
                "unsupported_joint_geometry", "Joint face attachment requires a plane, cylinder, cone, sphere or torus.")
        result = factory.createByNonPlanarFace(entity, key)
    require(result is not None and result.isValid, "invalid_joint_geometry", "Fusion did not create joint geometry.")
    return result


def selector(design, value, raw, definition=None):
    if "origin" in raw:
        require(definition is None, "invalid_joint_geometry", "A joint origin must be attached to geometry, not another origin.")
        origin = resolve(design, value, raw["origin"], ("JointOrigin",))
        root_entity(design, origin)
        return origin
    entity = resolve(design, value, raw["entity"], GEOMETRY_TYPES)
    edge = resolve(design, value, raw["edge"], ("BRepEdge",)) if "edge" in raw else None
    for item in (entity, edge):
        if item is None:
            continue
        if definition is None:
            root_entity(design, item)
        else:
            require(getattr(item, "assemblyContext", None) is None and same(owner(item), definition),
                    "wrong_component", "Joint-origin inputs require native geometry in the selected component definition.")
    if edge is not None:
        require(same(getattr(entity, "assemblyContext", None), getattr(edge, "assemblyContext", None)),
                "invalid_context", "Face and edge must refer to the same occurrence instance.")
    return geometry_from_entity(entity, raw.get("keyPoint"), edge)


def attachment_context(geometry):
    geometry = concrete(geometry)
    entity = concrete(geometry if geometry.objectType.endswith("::JointOrigin") else geometry.entityOne)
    require(entity is not None and entity.isValid, "lost_joint_geometry", "Attachment has no live source entity.")
    return getattr(entity, "assemblyContext", None)


def check_pair(design, first, second, contexts=None):
    contexts = list(contexts) if contexts is not None else [attachment_context(g) for g in (first, second)]
    require(not same(*contexts), "invalid_joint", "Joint attachments must belong to distinct root/component instances.")
    for context in contexts:
        if context is not None:
            root_entity(design, context)
    if all(context is not None for context in contexts):
        a, b = [context.fullPathName for context in contexts]
        require(not a.startswith(b + "+") and not b.startswith(a + "+"), "invalid_joint", "Cannot joint an occurrence to its own descendant.")
    require(not all(context is None or context.isGrounded for context in contexts),
            "grounded_conflict", "Both joint sides are grounded; unground the intended movable instance explicitly.")
    return contexts


def set_motion(subject, args):
    kind = args["type"]
    require(not (kind in ("rigid", "ball") and ("axis" in args or "slideAxis" in args)), "invalid_joint_axes", "This joint motion has no configurable axis choice.")
    require(kind == "pin_slot" or "slideAxis" not in args, "invalid_joint_axes", "slideAxis is specific to pin-slot joints.")
    method = getattr(subject, "setAs" + MOTION_METHODS[kind] + "JointMotion")
    directions = adsk.fusion.JointDirections
    axis = getattr(directions, args.get("axis", "z").upper() + "AxisJointDirection")
    if kind == "rigid":
        ok = method()
    elif kind == "ball":
        ok = method(directions.ZAxisJointDirection, directions.XAxisJointDirection)
    elif kind == "pin_slot":
        require(args.get("axis", "z") != args.get("slideAxis", "x"), "invalid_joint_axes", "Pin-slot rotation and slide axes must differ.")
        ok = method(axis, getattr(directions, args.get("slideAxis", "x").upper() + "AxisJointDirection"))
    else:
        ok = method(axis)
    require(ok, "joint_failed", "Fusion refused the requested motion definition.")


def lock_arguments(args, joint=None):
    """Rigid motion has no lock setter; its implicit native lock flag is false."""
    kind = args.get("type")
    if kind is None:
        motion = joint.jointMotion
        require(motion is not None, "unsupported_inferred_joint", "Cannot determine the joint's native motion definition.")
        rigid = motion.objectType == "adsk::fusion::RigidJointMotion"
    else:
        rigid = kind == "rigid"
    if rigid:
        require(not args.get("locked", False), "unsupported_joint_lock", "Rigid joints cannot be locked; omit locked or use false.")
        # Verify the implicit flag even when a rigid redefinition omitted it.
        return {**args, "locked": False}
    return args


def apply_lock(joint, args):
    if "locked" not in args:
        return
    motion = joint.jointMotion
    require(motion is not None, "unsupported_inferred_joint", "Cannot apply a lock without a native motion definition.")
    if motion.objectType == "adsk::fusion::RigidJointMotion":
        require(not args["locked"], "joint_edit_refused", "The native joint remained rigid and cannot accept a lock.")
        # Native rigid setters throw even for false. setAsRigidJointMotion
        # clears an existing moving-joint lock; regular_result checks readback.
        return
    joint.isLocked = args["locked"]


def expected_parameters(design, args, names):
    result = {}
    for name in names:
        if name in args:
            unit = "deg" if name == "angle" else "mm"
            inp = quantity(design, args[name], unit)
            number = design.unitsManager.evaluateExpression(inp.stringValue, unit)
            result[name] = (inp, number * (180 / math.pi if unit == "deg" else 10))
    return result


def verify_parameters(actual, expected):
    for name, (_, wanted) in expected.items():
        require(math.isclose(actual[name]["value"], wanted, abs_tol=1e-6, rel_tol=1e-8),
                "joint_parameter_refused", "Joint did not retain the requested " + name + ".")


def capture_pending(design):
    if design.snapshots.hasPendingSnapshot:
        require(design.snapshots.add() is not None, "joint_capture_failed", "Could not capture pending joint state.")
        return True
    return False


def source_reference(entity, value):
    entity = concrete(entity)
    if entity is None:
        return None
    component = owner(entity)
    require(component is not None, 'unsupported_joint_geometry', 'Attachment source has no readable component owner.')
    return {'reference': ref(entity, value), 'ownerId': identity(component)}


def capture_attachment(geometry, value, context=None):
    """Freeze graph locators/scalars before solve; retain no transient geometry."""
    geometry = concrete(geometry)
    state = geometry_state(geometry)
    result = {'type': geometry.objectType, 'context': ref(context, value) if context is not None else None}
    if geometry.objectType.endswith('::JointOrigin'):
        result['origin'] = source_reference(geometry, value)
    else:
        result.update({key: state[key] for key in ('geometryType', 'keyPointType')})
        result['sources'] = {key: source_reference(getattr(geometry, key), value)
                             for key in ('entityOne', 'entityTwo', 'planeOne', 'planeTwo')}
        result['tangent'] = {key: state[key] for key in ('tangentFaceType', 'tangentFaceParamOne', 'tangentFaceParamTwo') if key in state}
    return result


def resolve_source(design, value, saved):
    if saved is None:
        return None
    entity = resolve(design, value, saved['reference'])
    require(owner(entity) is not None and identity(owner(entity)) == saved['ownerId'],
            'joint_attachment_refused', 'Attachment source changed component owner.')
    return entity


def verify_attachment(design, value, actual, saved, occurrence=None, definition=False):
    """Compare fresh native API entities, never stale wrappers or token text."""
    actual = concrete(actual)
    require(actual is not None and actual.isValid and actual.objectType == saved['type'],
            'joint_attachment_refused', 'Joint attachment type changed.')
    expected_context = resolve(design, value, saved['context'], ('Occurrence',)) if saved['context'] else None
    require(same(occurrence, expected_context), 'joint_attachment_refused', 'Joint resolved a different occurrence instance.')
    require(not definition or (occurrence is None and expected_context is None and 'origin' not in saved),
            'joint_attachment_refused', 'Definition verification is only valid for native origin geometry.')
    context = attachment_context(actual)
    require(definition or context is None or same(context, occurrence),
            'joint_attachment_refused', 'Attachment proxy disagrees with the joint occurrence.')
    if 'origin' in saved:
        require(same(native(actual), native(resolve_source(design, value, saved['origin']))),
                'joint_attachment_refused', 'Joint origin identity changed.')
        return
    state = geometry_state(actual)
    require(all(state[key] == saved[key] for key in ('geometryType', 'keyPointType'))
            and all(state.get(key) == wanted for key, wanted in saved['tangent'].items()),
            'joint_attachment_refused', 'Joint attachment definition changed.')
    for key, locator in saved['sources'].items():
        found, wanted = concrete(getattr(actual, key)), resolve_source(design, value, locator)
        require((found is None and wanted is None) or (found is not None and wanted is not None
                and found.objectType == wanted.objectType and same(native(found), native(wanted))),
                'joint_attachment_refused', 'Joint attachment source did not read back: ' + key)
        found_context = getattr(found, 'assemblyContext', None) if found is not None else None
        require(found_context is None or (definition and same(found_context.component, owner(wanted)))
                or (not definition and same(found_context, occurrence)),
                'joint_attachment_refused', 'Joint attachment source has a different occurrence context.')


def validate_frame(matrix):
    require(len(matrix) == 16 and all(math.isfinite(v) for v in matrix), "joint_frame_unavailable", "Attachment frame is incomplete.")
    axes = [[matrix[row * 4 + col] for row in range(3)] for col in range(3)]
    require(all(math.isclose(sum(v*v for v in axis), 1, abs_tol=1e-6) for axis in axes)
            and all(abs(sum(a*b for a,b in zip(axes[i],axes[k]))) < 1e-6 for i,k in ((0,1),(0,2),(1,2))),
            "joint_frame_unavailable", "Attachment frame is not orthonormal.")
    cross = [axes[0][1]*axes[1][2]-axes[0][2]*axes[1][1], axes[0][2]*axes[1][0]-axes[0][0]*axes[1][2], axes[0][0]*axes[1][1]-axes[0][1]*axes[1][0]]
    require(sum(a*b for a,b in zip(cross,axes[2])) > 1-1e-6, "joint_frame_unavailable", "Attachment frame is not right-handed.")


def attachment_frames(joint):
    # Native joint transforms already locate root-owned joint anchors in root
    # space. JointGeometry frames use another axis convention and can become
    # unreadable after compute even while isValid remains true. Do not rebuild
    # them from native faces or apply occurrence transforms a second time.
    require(getattr(joint, 'assemblyContext', None) is None
            and same(joint.parentComponent, joint.parentComponent.parentDesign.rootComponent),
            'root_joint_required', 'Frame readback requires a root-owned joint.')
    matrices = [matrix_data(joint.geometryOneTransform), matrix_data(joint.geometryTwoTransform)]
    for matrix in matrices:
        validate_frame(matrix)
    return {"space": "native_joint_api", "originSpace": "root", "translationUnit": "mm", "transformLayout": "row-major",
            "source": "Joint.geometryOneTransform / Joint.geometryTwoTransform",
            "axisConvention": "native joint transform; distinct from transient JointGeometry axes",
            "one": matrices[0], "two": matrices[1]}


def initial_alignment(joint, offset, angle, kind):
    frames = attachment_frames(joint)
    one, two = frames["one"], frames["two"]
    delta = [one[i]-two[i] for i in (3,7,11)]
    distance = math.sqrt(sum(v*v for v in delta))
    require(math.isclose(distance, abs(offset), abs_tol=1e-5, rel_tol=1e-7),
            "joint_alignment_failed", "Solved attachment origins do not match the requested offset magnitude.", actualDistance=distance, expectedDistance=abs(offset))
    if abs(offset) > 1e-7:
        axis = [two[i] for i in (2,6,10)]
        axial = sum(a*b for a,b in zip(delta,axis))
        require(math.sqrt(max(0, distance*distance-axial*axial)) < 1e-5,
                "joint_alignment_failed", "Joint offset is not along the attachment Z axis.")
    axes_checked = kind != "ball" and abs(angle) < 1e-9
    if axes_checked:
        dot = sum(one[i]*two[i] for i in (2,6,10))
        require(abs(abs(dot)-1) < 1e-6, "joint_alignment_failed", "Solved attachment Z axes are not collinear.")
    return {**frames, "originDistance": distance, "offsetMagnitudeVerified": True,
            "zeroAngleAxisCollinearityVerified": axes_checked,
            "orientationSenseVerification": "native angle/flip parameter readback; physical orientation requires independent occurrence geometry acceptance"}


def poses(design):
    return [(o, list(o.transform2.asArray()), bool(o.isGrounded), bool(getattr(o, "isGroundToParent", False)))
            for o in items(design.rootComponent.allOccurrences)]


def pose_result(design, before):
    live = items(design.rootComponent.allOccurrences)
    require(len(live) == len(before), "joint_structure_changed", "Joint solving unexpectedly changed the occurrence count.")
    # each occurrence against the live one at its path, not against every live one (that was n squared
    # native comparisons: hours on a 4,579-occurrence assembly)
    live_by_path = {o.fullPathName: o for o in live}
    changed = []
    for occurrence, old, grounded, ground_to_parent in before:
        require(occurrence.isValid and same(occurrence, live_by_path.get(occurrence.fullPathName)), "joint_structure_changed",
                "Joint solving replaced an occurrence.")
        current = list(occurrence.transform2.asArray())
        moved = any(not math.isclose(a, b, abs_tol=1e-8, rel_tol=1e-8) for a, b in zip(current, old))
        require(bool(occurrence.isGrounded) == grounded and bool(getattr(occurrence, "isGroundToParent", False)) == ground_to_parent,
                "grounding_changed", "Joint solving changed grounding state.")
        require(not grounded or not moved, "grounded_conflict", "Joint solving moved a grounded occurrence.")
        if moved:
            changed.append({"occurrenceId": identity(occurrence), "path": occurrence.fullPathName,
                            "before": [v * 10 if i in (3, 7, 11) else v for i, v in enumerate(old)],
                            "after": matrix_data(occurrence.transform2)})
    return {"space": "root", "translationUnit": "mm", "transformLayout": "row-major", "changedOccurrences": changed,
            "groundedOccurrencesPreserved": True}


def regular_result(design, joint, fresh, args, expected, attachments, before, original_value):
    result = joint_result(joint, fresh, args)
    state = result["joint"]
    verify_parameters(state, expected)
    for key, prop in (("flipped", "isFlipped"), ("locked", "isLocked")):
        require(key not in args or getattr(joint, prop) == args[key], "joint_edit_refused", "Joint did not retain " + key)
    if "type" in args:
        require(state["motionType"].endswith("::" + MOTION_METHODS[args["type"]] + "JointMotion"), "joint_edit_refused", "Joint motion type did not read back.")
        verify_motion_axes(state["axes"], args)
    contexts = (joint.occurrenceOne, joint.occurrenceTwo)
    for (prop, expected_attachment), context in zip(attachments.items(), contexts):
        verify_attachment(design, original_value, getattr(joint, prop), expected_attachment, context)
    result["placement"] = pose_result(design, before)
    result["attachmentFrames"] = attachment_frames(joint)
    return result


def verify_motion_axes(actual, args):
    kind = args["type"]
    field = {"revolute": "rotationAxis", "slider": "slideDirection", "cylindrical": "rotationAxis",
             "pin_slot": "rotationAxis", "planar": "normalDirection"}.get(kind)
    expected = {field: args.get("axis", "z")} if field else {}
    if kind == "pin_slot":
        expected["slideDirection"] = args.get("slideAxis", "x")
    if kind == "ball":
        expected.update(pitchDirection="z", yawDirection="x")
    for key, axis in expected.items():
        require(actual[key] == getattr(adsk.fusion.JointDirections, axis.upper() + "AxisJointDirection"),
                "joint_axis_refused", "Joint did not retain the requested " + key + ".")


def regular_joint_create(design, value, args):
    args = lock_arguments({"flipped": False, **args})
    require(not design.snapshots.hasPendingSnapshot, "pending_position_changes", "Capture or revert pending positions before regular-joint solving.")
    first, second = [selector(design, value, args[key]) for key in ("geometryOne", "geometryTwo")]
    expected_contexts = check_pair(design, first, second)
    attachments = {prop: capture_attachment(geometry, value, context) for prop, geometry, context in
                   zip(('geometryOrOriginOne', 'geometryOrOriginTwo'), (first, second), expected_contexts)}
    before = poses(design)
    expected = expected_parameters(design, {"angle": 0, "offset": 0, **args}, ("angle", "offset"))
    inp = design.rootComponent.joints.createInput(first, second)
    require(inp is not None, "joint_failed", "Fusion did not return JointInput.")
    for name, (quantity_input, _) in expected.items():
        setattr(inp, name, quantity_input)
    inp.isFlipped = args.get("flipped", False)
    set_motion(inp, args)
    joint = named(design.rootComponent.joints.add(inp), args)
    # Fusion solves the joint as it is added; the write's own settle decides on a full recompute. Faults
    # the model already carried are allowed, as in settle: the joint may not add or change one.
    compute.midway(design)
    alignment = initial_alignment(joint, expected["offset"][1], expected["angle"][1], args["type"])
    apply_lock(joint, args)
    configure_joint(design, joint, args)
    capture_pending(design)
    def result(fresh):
        row = regular_result(design, joint, fresh, args, expected, attachments, before, value)
        row["initialAlignment"] = alignment
        return row
    return result


def regular_joint_edit(design, value, args):
    require(not design.snapshots.hasPendingSnapshot, "pending_position_changes", "Capture or revert pending positions before regular-joint redefinition.")
    joint = resolve(design, value, args["joint"], ("Joint",))
    changed = set(args) & {"geometryOne", "geometryTwo", "angle", "offset", "offsetX", "offsetY", "flipped", "locked", "type"}
    require(changed, "invalid_input", "Supply a geometry, offset, angle, flip, lock or motion-definition change.")
    args = lock_arguments(args, joint)
    require(getattr(joint, "assemblyContext", None) is None and same(joint.parentComponent, design.rootComponent),
            "root_joint_required", "Regular-joint redefinition currently requires a root-owned joint.")
    regular_state(joint)  # Fail before rollback if attachment readback is incomplete.
    require("type" in args or not ({"axis", "slideAxis"} & set(args)), "invalid_input", "Axis changes require the motion type.")
    attachments = {prop: selector(design, value, args[key]) if key in args else getattr(joint, prop)
                   for key, prop in (("geometryOne", "geometryOrOriginOne"), ("geometryTwo", "geometryOrOriginTwo"))}
    expected_contexts = [attachment_context(attachments[prop]) if key in args else getattr(joint, occurrence)
                         for key,prop,occurrence in (("geometryOne","geometryOrOriginOne","occurrenceOne"),("geometryTwo","geometryOrOriginTwo","occurrenceTwo"))]
    check_pair(design, *attachments.values(), contexts=expected_contexts)
    saved_attachments = {prop: capture_attachment(geometry, value, context)
                         for (prop, geometry), context in zip(attachments.items(), expected_contexts)}
    expected = expected_parameters(design, args, ("angle", "offset", "offsetX", "offsetY"))
    before = poses(design)
    with before_timeline(design, joint):
        for key, prop in (("geometryOne", "geometryOrOriginOne"), ("geometryTwo", "geometryOrOriginTwo")):
            if key in args:
                setattr(joint, prop, attachments[prop])
        if "type" in args:
            set_motion(joint, args)
        if "flipped" in args:
            joint.isFlipped = args["flipped"]
        for name, (quantity_input, _) in expected.items():
            getattr(joint, name).expression = quantity_input.stringValue
    apply_lock(joint, args)
    capture_pending(design)
    return lambda fresh: regular_result(design, joint, fresh, args, expected, saved_attachments, before, value)


def origin_axes(design, value, args, comp):
    result = {}
    for key in ("xAxisEntity", "zAxisEntity"):
        if key in args:
            entity = resolve(design, value, args[key], ("ConstructionAxis", "SketchLine", "BRepEdge"))
            require(getattr(entity, "assemblyContext", None) is None and same(owner(entity), comp),
                    "wrong_component", "Orientation axes must be native geometry in the origin's component.")
            require(entity.geometry.objectType.endswith(("::Line3D", "::InfiniteLine3D")), "invalid_axis", "Orientation requires straight linear geometry.")
            result[key] = entity
    return result


def origin_result(design, origin, fresh, args, expected, geometry, axes, original_value):
    result = origin_data(origin, fresh)
    verify_parameters(result, expected)
    for key in ("name", "flipped"):
        require(key not in args or result[key] == args[key], "joint_origin_refused", "Origin did not retain " + key)
    # A native origin belongs to a definition, but Fusion can return its geometry
    # as a root-context proxy. Match native source and owner, not that incidental
    # instance. Regular joints keep the strict occurrence-context check above.
    verify_attachment(design, original_value, origin.geometry, geometry, definition=True)
    for key, locator in axes.items():
        require(same(native(concrete(getattr(origin, key))), native(resolve_source(design, original_value, locator))),
                "joint_origin_refused", "Origin axis did not read back.")
    return {"jointOrigin": result, "editScope": "component_definition"}


def joint_origin_create(design, value, args):
    args = {"flipped": False, **args}
    comp = component(design, value, args)
    geometry = selector(design, value, args["geometry"], comp)
    axes = origin_axes(design, value, args, comp)
    saved_geometry = capture_attachment(geometry, value)
    saved_axes = {key: source_reference(entity, value) for key, entity in axes.items()}
    expected = expected_parameters(design, {"angle": 0, "offsetX": 0, "offsetY": 0, "offsetZ": 0, **args}, ("angle", "offsetX", "offsetY", "offsetZ"))
    inp = comp.jointOrigins.createInput(geometry)
    require(inp is not None, "joint_origin_failed", "Fusion did not return JointOriginInput.")
    inp.isFlipped = args.get("flipped", False)
    for name, (quantity_input, _) in expected.items():
        setattr(inp, name, quantity_input)
    for key, entity in axes.items():
        setattr(inp, key, entity)
    origin = named(comp.jointOrigins.add(inp), args)
    return lambda fresh: origin_result(design, origin, fresh, args, expected, saved_geometry, saved_axes, value)


def joint_origin_edit(design, value, args):
    origin = resolve(design, value, args["origin"], ("JointOrigin",))
    require(getattr(origin, "assemblyContext", None) is None, "definition_reference_required", "Edit the native origin definition.")
    require(set(args) & {"name", "geometry", "flipped", "angle", "offsetX", "offsetY", "offsetZ", "xAxisEntity", "zAxisEntity"}, "invalid_input", "Supply an origin change.")
    geometry = selector(design, value, args["geometry"], origin.parentComponent) if "geometry" in args else origin.geometry
    axes = origin_axes(design, value, args, origin.parentComponent)
    saved_geometry = capture_attachment(geometry, value)
    saved_axes = {key: source_reference(entity, value) for key, entity in axes.items()}
    expected = expected_parameters(design, args, ("angle", "offsetX", "offsetY", "offsetZ"))
    with before_timeline(design, origin):
        if "geometry" in args:
            origin.geometry = geometry
        if "flipped" in args:
            origin.isFlipped = args["flipped"]
        for key, entity in axes.items():
            setattr(origin, key, entity)
        for name, (quantity_input, _) in expected.items():
            getattr(origin, name).expression = quantity_input.stringValue
    if "name" in args:
        origin.name = args["name"]
    return lambda fresh: origin_result(design, origin, fresh, args, expected, saved_geometry, saved_axes, value)


def joint_origin_delete(design, value, args):
    origin = resolve(design, value, args["origin"], ("JointOrigin",))
    require(getattr(origin, "assemblyContext", None) is None, "definition_reference_required", "Delete the native origin definition.")
    name = origin.name
    require(origin.deleteMe(), "delete_failed", "Fusion refused joint-origin deletion.")
    return {"deletedJointOrigin": name}


OPERATIONS = {name: globals()[name] for name in ("regular_joint_create", "regular_joint_edit", "joint_origin_create", "joint_origin_edit", "joint_origin_delete")}
