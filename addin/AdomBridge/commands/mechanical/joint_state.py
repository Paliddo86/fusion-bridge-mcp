"""Semantic attachment graph and frames for regular joints, origins and links.

Entity relationships use native API equality through session identities. Transient
JointGeometry wrappers and token strings never serve as graph identity.
"""
import math
import adsk.fusion

from .core import MAX_ENTITIES, concrete, identity, items, number, point_mm, ref, require, same

_source_entities = []


def source_identity(entity):
    """Re-resolve token locators before comparing API identity after recompute.

    Fusion can invalidate an old BRep wrapper while preserving its model entity.
    A token is only a lookup locator; fresh native equality is the identity proof.
    """
    from .geometry import owner
    require(entity is not None and entity.isValid, 'lost_joint_geometry', 'Attachment native source is invalid.')
    component = owner(entity)
    if component is None:
        return identity(entity)
    design = component.parentDesign
    token = entity.entityToken
    for saved in _source_entities:
        if saved['type'] != entity.objectType or not saved['owner'].isValid or not same(saved['owner'], component):
            continue
        candidates = [concrete(candidate) for candidate in design.findEntityByToken(saved['token'])]
        require(len(candidates) <= MAX_ENTITIES, 'inspection_budget', 'Attachment token lookup exceeds inspection budget.')
        matches = [candidate for candidate in candidates if candidate.isValid and same(candidate, entity)]
        if matches:
            require(len(candidates) == 1 and candidates[0].objectType == entity.objectType,
                    'ambiguous_reference', 'Attachment identity token no longer resolves uniquely.', matchCount=len(candidates))
            return saved['id']
    require(len(_source_entities) < MAX_ENTITIES, 'identity_budget', 'Session attachment identity budget exhausted.')
    result = identity(entity)
    _source_entities.append({'owner': component, 'type': entity.objectType, 'token': token, 'id': result})
    return result


def entity_link(entity, value=None):
    entity = concrete(entity)
    if entity is None:
        return None
    require(entity.isValid, "lost_joint_geometry", "Joint attachment is no longer valid.")
    native = getattr(entity, "nativeObject", None) or entity
    context = getattr(entity, "assemblyContext", None)
    result = {"entityId": source_identity(native), "objectType": native.objectType,
              "occurrenceId": identity(context) if context is not None else None}
    if value is not None:
        result["ref"] = ref(entity, value)
    return result


def vector_data(vector):
    require(vector is not None, "joint_frame_unavailable", "Joint direction is unavailable.")
    return [number(getattr(vector, axis), "joint direction") for axis in "xyz"]


def matrix_data(matrix):
    require(matrix is not None, "joint_frame_unavailable", "Joint frame is unavailable.")
    result = [number(v, "joint transform") for v in matrix.asArray()]
    require(len(result) == 16, "joint_frame_unavailable", "Joint frame must be a 4 by 4 matrix.")
    for i in (3, 7, 11):
        result[i] *= 10
    return result


def parameter_state(parameter, unit):
    require(parameter is not None, "unsupported_inferred_joint", "Joint parameter is unavailable (inferred joints are not editable).")
    scale = 180 / math.pi if unit == "deg" else 10
    return {"expression": parameter.expression, "value": number(parameter.value, "joint parameter") * scale, "unit": unit}


def geometry_state(geometry, value=None):
    geometry = concrete(geometry)
    require(geometry is not None and geometry.isValid, "lost_joint_geometry", "Joint geometry is unavailable.")
    if geometry.objectType.endswith("::JointOrigin"):
        return {"origin": entity_link(geometry, value)}
    require(geometry.objectType.endswith("::JointGeometry"), "unsupported_joint_geometry", "Unsupported joint attachment wrapper.")
    result = {"geometryType": int(geometry.geometryType), "keyPointType": int(geometry.keyPointType),
              "frameSpace": "native_joint_geometry_api", "originUnit": "mm",
              "entityOne": entity_link(geometry.entityOne, value), "entityTwo": entity_link(geometry.entityTwo, value),
              "origin": point_mm(geometry.origin),
              "primaryAxis": vector_data(geometry.primaryAxisVector),
              "secondaryAxis": vector_data(geometry.secondaryAxisVector),
              "thirdAxis": vector_data(geometry.thirdAxisVector)}
    # These parameters distinguish analytical face and tangent attachment sites.
    kinds = adsk.fusion.JointGeometryTypes
    if geometry.geometryType in (kinds.JointTangentBRepFaceGeometry, kinds.JointTangentBRepFaceEdgeGeometry):
        for name in ("tangentFaceType", "tangentFaceParamOne", "tangentFaceParamTwo"):
            result[name] = number(getattr(geometry, name), name)
    for name in ("planeOne", "planeTwo"):
        plane = getattr(geometry, name)
        result[name] = entity_link(plane, value)
    return result


def regular_state(joint, value=None):
    return {"geometryOne": geometry_state(joint.geometryOrOriginOne, value),
            "geometryTwo": geometry_state(joint.geometryOrOriginTwo, value),
            "geometryOneTransform": matrix_data(joint.geometryOneTransform),
            "geometryTwoTransform": matrix_data(joint.geometryTwoTransform),
            "angle": parameter_state(joint.angle, "deg"),
            "offset": parameter_state(joint.offset, "mm"),
            "offsetX": parameter_state(joint.offsetX, "mm"),
            "offsetY": parameter_state(joint.offsetY, "mm"),
            "flipped": bool(joint.isFlipped), "locked": bool(joint.isLocked),
            "ownerId": identity(joint.parentComponent),
            "frameSpace": "native_joint_api", "transformTranslationUnit": "mm"}


def origin_data(origin, value=None):
    origin = concrete(origin)
    result = {"name": origin.name, "geometry": geometry_state(origin.geometry, value),
              "flipped": bool(origin.isFlipped), "transform": matrix_data(origin.transform),
              "xAxisEntity": entity_link(origin.xAxisEntity, value),
              "zAxisEntity": entity_link(origin.zAxisEntity, value),
              "primaryAxis": vector_data(origin.primaryAxisVector),
              "secondaryAxis": vector_data(origin.secondaryAxisVector),
              "thirdAxis": vector_data(origin.thirdAxisVector),
              "ownerId": identity(origin.parentComponent),
              "space": "root" if getattr(origin, "assemblyContext", None) else "component",
              "units": {"length": "mm", "angle": "deg", "transformTranslation": "mm"}}
    for name in ("angle", "offsetX", "offsetY", "offsetZ"):
        result[name] = parameter_state(getattr(origin, name), "deg" if name == "angle" else "mm")
    if value is not None:
        result["ref"] = ref(origin, value)
    return result


# Public motion names remain independent of Autodesk enum integer values.
LINK_MOTIONS = {
    "RevoluteJointMotion": {"rotation": "RevoluteJointRotateMotionType"},
    "SliderJointMotion": {"slide": "SliderJointSlideMotionType"},
    "CylindricalJointMotion": {"rotation": "CylindricalJointRotateMotionType", "slide": "CylindricalJointSlideMotionType"},
    "PinSlotJointMotion": {"rotation": "PinSlotJointRotateMotionType", "slide": "PinSlotJointSlideMotionType"},
    "PlanarJointMotion": {"rotation": "PlanarJointRotateMotionType", "primarySlide": "PlanarJointSlideOneMotionType", "secondarySlide": "PlanarJointSlideTwoMotionType"},
    "BallJointMotion": {"pitch": "BallJointPitchMotionType", "roll": "BallJointRollMotionType", "yaw": "BallJointYawMotionType"}}


def link_motion_name(joint, enum):
    import adsk.fusion
    from .assembly_state import joint_motion
    names = LINK_MOTIONS.get(joint_motion(joint).objectType.split("::")[-1], {})
    matches = [name for name, member in names.items() if getattr(adsk.fusion.JointMotionTypes, member) == enum]
    require(len(matches) == 1, "unsupported_link_motion", "Motion link has an unsupported joint degree of freedom.")
    return matches[0]


def link_data(link, value=None):
    link = concrete(link)
    first, second = concrete(link.jointOne), concrete(link.jointTwo)
    names = [link_motion_name(joint, enum) for joint, enum in
             ((first, link.motionOne), (second or first, link.motionTwo))]
    result = {"name": link.name, "suppressed": bool(link.isSuppressed), "reversed": bool(link.isReversed),
              "jointOne": entity_link(first, value), "jointTwo": entity_link(second, value),
              "sameJoint": second is None or same(first, second),
              "motionOne": names[0], "motionTwo": names[1], "healthState": int(link.healthState),
              "message": link.errorOrWarningMessage}
    for suffix, name in zip(("One", "Two"), names):
        result["value" + suffix] = parameter_state(getattr(link, "value" + suffix), "mm" if "lide" in name else "deg")
    if value is not None:
        result["ref"] = ref(link, value)
    return result


def component_joint_state(comp):
    return {"jointOrigins": [origin_data(o) for o in items(comp.jointOrigins)] if hasattr(comp, "jointOrigins") else [],
            "motionLinks": [link_data(o) for o in items(comp.motionLinks)] if hasattr(comp, "motionLinks") else []}


MOTION_AXES = {
    "RigidJointMotion": (),
    "RevoluteJointMotion": (("rotationAxis", "customRotationAxisEntity"),),
    "CylindricalJointMotion": (("rotationAxis", "customRotationAxisEntity"),),
    "SliderJointMotion": (("slideDirection", "customSlideDirectionEntity"),),
    "PinSlotJointMotion": (("rotationAxis", "customRotationAxisEntity"), ("slideDirection", "customSlideDirectionEntity")),
    "PlanarJointMotion": (("normalDirection", "customNormalDirectionEntity"), ("primarySlideDirection", "customPrimarySlideDirectionEntity")),
    "BallJointMotion": (("pitchDirection", "customPitchDirectionEntity"), ("yawDirection", "customYawDirectionEntity"))}


def motion_axes_state(motion, value=None):
    kind = motion.objectType.split("::")[-1]
    require(kind in MOTION_AXES, "unsupported_joint_motion", "Unknown joint direction contract.")
    result = {}
    for direction, source in MOTION_AXES[kind]:
        enum = getattr(motion, direction)
        custom = getattr(motion, source) if enum == adsk.fusion.JointDirections.CustomJointDirection else None
        require(enum != adsk.fusion.JointDirections.CustomJointDirection or custom is not None,
                "lost_joint_geometry", "Custom joint axis has no source entity.")
        result[direction] = int(enum)
        result[source] = entity_link(custom, value)
    return result
