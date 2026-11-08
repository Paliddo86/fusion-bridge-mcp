"""Assembly readback shared by inspection, stale guards and recovery witnesses."""
import json
import math
import adsk.fusion

from .core import concrete, identity, items, ref, require

GROUP = "AdomMechanical"
MOTION_SCALES = {"rotation": 180 / math.pi, "slide": 10, "primarySlide": 10, "secondarySlide": 10,
                 "pitch": 180 / math.pi, "roll": 180 / math.pi, "yaw": 180 / math.pi}


def attributes(entity):
    attrs = getattr(entity, "attributes", None)
    return {attr.name: attr.value for attr in items(attrs) if attr.groupName == GROUP} if attrs else {}


# Component.partNumber and .description are cloud reads on a cloud document: about 0.6 s each per
# component (measured 2026-10-28), so a 1,006-component cell took 21 minutes just
# to read them. They are cached per component and revisionId (which changes when either does).
_native_properties = {}


def native_properties(component, read=True):
    """(partNumber, description), read once per component revision this session. read=False answers
    None for a component not read yet instead of reading it."""
    from .core import identity
    key = (identity(component), str(component.revisionId))
    if key not in _native_properties:
        if not read:
            return None
        if len(_native_properties) > 50000:
            _native_properties.clear()
        _native_properties[key] = (getattr(component, "partNumber", ""), getattr(component, "description", ""))
    return _native_properties[key]


def metadata(component, native=True):
    """A component's BOM and supplier metadata. native=False skips the two slow cloud reads (part
    number and description come back None): the revision witness relies on revisionId for them."""
    raw = attributes(component)
    part_number, description = native_properties(component) if native else (None, None)
    result = {"name": component.name, "partNumber": part_number, "description": description, "attributes": raw}
    for name in ("supplier", "supplierPartNumber", "sourceUrl", "sourceFile", "sha256", "asPurchased", "excludeFromBom",
                 "category", "makeBuy", "process", "finish", "unitCost", "currency", "leadTimeDays", "stock", "notes", "revision"):
        if name in raw:
            result[name] = json.loads(raw[name])
    material = getattr(component, "material", None)
    result["material"] = {"id": material.id, "name": material.name} if material else None
    return result


def limits_data(limits, scale):
    return {"minimumEnabled": limits.isMinimumValueEnabled, "minimum": limits.minimumValue * scale,
            "maximumEnabled": limits.isMaximumValueEnabled, "maximum": limits.maximumValue * scale,
            "restEnabled": limits.isRestValueEnabled, "rest": limits.restValue * scale}


def joint_motion(joint):
    joint = concrete(joint)
    base = joint.jointMotion
    require(base is not None, "unsupported_inferred_joint", "Inferred joint motion is unavailable; redefine it in Fusion before typed edits.")
    kind = base.objectType.split("::")[-1]
    require(kind in ("RigidJointMotion", "RevoluteJointMotion", "SliderJointMotion", "CylindricalJointMotion",
                     "PinSlotJointMotion", "PlanarJointMotion", "BallJointMotion"), "unsupported_joint_motion", "Cannot inspect joint motion type: " + kind)
    cls = getattr(adsk.fusion, kind, None)
    require(cls is not None, "unsupported_joint_motion", "Fusion motion wrapper is unavailable: " + kind)
    motion = cls.cast(base)
    require(motion is not None, "unsupported_joint_motion", "Fusion could not cast joint motion: " + kind)
    return motion


def joint_data(joint, value=None):
    joint = concrete(joint)
    regular = joint.objectType.endswith("::Joint")
    motion = joint_motion(joint)
    result = {"name": joint.name, "type": joint.objectType, "motionType": motion.objectType,
              "suppressed": joint.isSuppressed,
              "occurrenceOneId": identity(joint.occurrenceOne) if joint.occurrenceOne else None,
              "occurrenceTwoId": identity(joint.occurrenceTwo) if joint.occurrenceTwo else None}
    for name, scale in MOTION_SCALES.items():
        if hasattr(motion, name + "Limits"):
            result[name + "Limits"] = limits_data(getattr(motion, name + "Limits"), scale)
        if hasattr(motion, name + "Value"):
            result[name + "Value"] = getattr(motion, name + "Value") * scale
    for name in ("rotationAxisVector", "slideDirectionVector", "normalDirectionVector", "primarySlideDirectionVector", "secondarySlideDirectionVector", "pitchDirectionVector", "yawDirectionVector", "rollDirectionVector"):
        if hasattr(motion, name):
            vector = getattr(motion, name)
            result[name] = [vector.x, vector.y, vector.z] if vector else None
    from .joint_state import motion_axes_state
    result["axes"] = motion_axes_state(motion, value)
    if hasattr(joint, "transform"):
        matrix = joint.transform
        result["transform"] = list(matrix.asArray()) if matrix else None
        if regular and matrix:
            for index in (3, 7, 11):
                result["transform"][index] *= 10
    if regular:
        from .joint_state import regular_state
        result.update(regular_state(joint, value))
    if value:
        result["ref"] = ref(joint, value)
        result["units"] = {"length": "mm", "angle": "deg", "transformTranslation": "mm" if regular else "cm"}
    return result


def rigid_group_data(group, value=None):
    result = {"name": group.name, "suppressed": group.isSuppressed,
              "occurrenceIds": [identity(o) for o in items(group.occurrences)]}
    if value:
        result["ref"] = ref(group, value)
        result["occurrenceRefs"] = [ref(o, value) for o in items(group.occurrences)]
    return result


def component_state(comp):
    # revisionId (in the witness) changes when the part number or description does: no cloud reads
    result = metadata(comp, native=False)
    result["joints"] = [joint_data(joint) for name in ("joints", "asBuiltJoints")
                        for joint in items(getattr(comp, name))] if hasattr(comp, "joints") else []
    result["rigidGroups"] = [rigid_group_data(group) for group in items(comp.rigidGroups)] if hasattr(comp, "rigidGroups") else []
    result["bodyMaterials"] = [{"id": body.material.id, "name": body.material.name} if body.material else None
                               for body in items(comp.bRepBodies)] if all(hasattr(b, "material") for b in items(comp.bRepBodies)) else []
    from .joint_state import component_joint_state
    result.update(component_joint_state(comp))
    return result


def occurrence_state(occurrence):
    return {"name": getattr(occurrence, "name", None), "grounded": occurrence.isGrounded,
            "groundedToParent": getattr(occurrence, "isGroundToParent", None),
            "attributes": attributes(occurrence)}
