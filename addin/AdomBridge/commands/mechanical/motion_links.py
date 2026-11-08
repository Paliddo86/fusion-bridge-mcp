"""Typed native coupling with exact joint/degree-of-freedom readback."""
import math

import adsk.fusion

from .assembly_state import joint_motion
from .core import concrete, items, number, require, resolve, same
from .joint_state import LINK_MOTIONS, entity_link, link_data, link_motion_name
from .sketches import quantity


def root_joint(design, value, raw):
    joint = resolve(design, value, raw, ("Joint", "AsBuiltJoint"))
    require(getattr(joint, "assemblyContext", None) is None and same(joint.parentComponent, design.rootComponent),
            "root_joint_required", "Motion links currently require root-owned joints.")
    require(not joint.isSuppressed, "suppressed_joint", "Unsuppress a joint before coupling its motion.")
    return joint


def motion_enum(joint, name):
    family = joint_motion(joint).objectType.split("::")[-1]
    members = LINK_MOTIONS.get(family, {})
    require(name in members, "wrong_joint_type", "The selected joint does not have motion " + name + ".")
    enums = getattr(adsk.fusion, "JointMotionTypes", None)
    require(enums is not None and hasattr(enums, members[name]), "unsupported_capability", "This Fusion build does not expose native motion links.")
    return getattr(enums, members[name])


def inputs(design, first, second, args):
    require(not same(first, second) or args["motionOne"] != args["motionTwo"], "invalid_motion_link", "Two different joint degrees of freedom are required.")
    enums, values, expected = [], [], {}
    for suffix, joint in (("One", first), ("Two", second)):
        motion = args["motion" + suffix]
        enums.append(motion_enum(joint, motion))
        unit = "mm" if "lide" in motion else "deg"
        inp = quantity(design, args["value" + suffix], unit, positive=True)
        magnitude = design.unitsManager.evaluateExpression(inp.stringValue, unit)
        require(magnitude > 0, "invalid_motion_link", "Link magnitudes must be positive; use reversed for opposite direction.")
        values.append(inp)
        expected["value" + suffix] = magnitude * (10 if unit == "mm" else 180 / math.pi)
    return enums, values, expected


def result(link, fresh, args, first, second, expected):
    data = link_data(link, fresh)
    # A same-joint link can expose either null or the first joint again as its
    # native second member. Canonicalize only the effective member for equality;
    # link_data preserves the actual native jointTwo representation.
    actual_first, actual_second = concrete(link.jointOne), concrete(link.jointTwo)
    require(entity_link(actual_first) == entity_link(first)
            and entity_link(actual_second or actual_first) == entity_link(second or first),
            "motion_link_refused", "Linked joint identities did not read back.")
    for key in ("name", "suppressed", "reversed", "motionOne", "motionTwo"):
        require(key not in args or data[key] == args[key], "motion_link_refused", "Motion link did not retain " + key)
    for key, wanted in expected.items():
        require(math.isclose(data[key]["value"], wanted, rel_tol=1e-8, abs_tol=1e-6), "motion_link_refused", "Motion-link ratio did not read back.")
    require(link.healthState == adsk.fusion.FeatureHealthStates.HealthyFeatureHealthState, "unhealthy_motion_link", "Motion link is not healthy.")
    return {"motionLink": data}


def motion_value(joint, enum):
    name = link_motion_name(joint, enum)
    motion = joint_motion(joint)
    require(hasattr(motion, name + "Value"), "motion_link_unverifiable", "Linked motion has no readable value.")
    return number(getattr(motion, name + "Value"), "linked motion")


def coupling_snapshot(design):
    collection = getattr(design.rootComponent, "motionLinks", None)
    if collection is None:
        return []
    result = []
    for link in items(collection):
        if link.isSuppressed:
            continue
        first, second = concrete(link.jointOne), concrete(link.jointTwo or link.jointOne)
        require(not first.isSuppressed and not second.isSuppressed, "motion_link_unverifiable", "An active link references a suppressed joint.")
        result.append((link, motion_value(first, link.motionOne), motion_value(second, link.motionTwo), link_data(link)))
    return result


def verify_coupling(before):
    verified = []
    for link, one, two, state in before:
        require(link.isValid and not link.isSuppressed, "motion_link_refused", "Driving unexpectedly removed or suppressed a motion link.")
        first, second = concrete(link.jointOne), concrete(link.jointTwo or link.jointOne)
        require(link_data(link) == state, "motion_link_refused", "Driving unexpectedly changed motion-link membership or settings.")
        delta_one = motion_value(first, link.motionOne) - one
        delta_two = motion_value(second, link.motionTwo) - two
        value_one, value_two = number(link.valueOne.value, "link magnitude"), number(link.valueTwo.value, "link magnitude")
        require(abs(value_one) > 1e-12 and abs(value_two) > 1e-12, "motion_link_unverifiable", "Link magnitude cannot be zero.")
        require(math.isclose(delta_one/value_one, (-1 if link.isReversed else 1)*delta_two/value_two, abs_tol=1e-7, rel_tol=1e-7),
                "motion_link_drive_refused", "Driven motion did not propagate according to the active link ratio.")
        verified.append({"name": link.name, "jointOne": entity_link(first), "jointTwo": entity_link(second), "ratioVerified": True})
    return verified


def motion_link_create(design, value, args):
    first = root_joint(design, value, args["jointOne"])
    second = root_joint(design, value, args["jointTwo"]) if "jointTwo" in args else None
    require(second is None or not same(first, second), "invalid_motion_link", "Omit jointTwo to couple two motions of jointOne.")
    enums, values, expected = inputs(design, first, second or first, args)
    collection = getattr(design.rootComponent, "motionLinks", None)
    require(collection is not None, "unsupported_capability", "This Fusion build has no motionLinks collection.")
    inp = collection.createInput(first, second)
    require(inp is not None, "motion_link_failed", "Fusion did not return MotionLinkInput.")
    inp.motionOne, inp.motionTwo = enums
    inp.valueOne, inp.valueTwo = values
    inp.isReversed = args.get("reversed", False)
    link = collection.add(inp)
    require(link is not None and link.isValid, "motion_link_failed", "Fusion did not create a motion link.")
    if "name" in args:
        link.name = args["name"]
    return lambda fresh: result(link, fresh, {"reversed": False, **args}, first, second, expected)


def motion_link_edit(design, value, args):
    link = resolve(design, value, args["link"], ("MotionLink",))
    require(getattr(link, "assemblyContext", None) is None and same(link.parentComponent, design.rootComponent),
            "root_joint_required", "Edit the root-owned motion link.")
    require(set(args) & {"name", "suppressed", "reversed", "motionOne", "motionTwo", "valueOne", "valueTwo"}, "invalid_input", "Supply a motion-link change.")
    first, second = concrete(link.jointOne), concrete(link.jointTwo)
    coupling = set(args) & {"motionOne", "motionTwo", "valueOne", "valueTwo", "reversed"}
    expected = {}
    effective = dict(args)
    if coupling:
        old = link_data(link)
        changes = {key: args.get(key, old[key]) for key in ("motionOne", "motionTwo", "reversed")}
        changes.update({key: args.get(key, old[key]["expression"]) for key in ("valueOne", "valueTwo")})
        effective.update(changes)
        enums, values, expected = inputs(design, first, second or first, changes)
        # Unlike regular-joint redefinition, setMotionData requires the link to
        # exist in the evaluated timeline. Native rollback before this item
        # removes its component context and fails InternalValidationError.
        require(design.timeline.markerPosition == design.timeline.count, "timeline_not_at_end",
                "Motion-link coupling edits require the timeline at the end.")
        require(link.setMotionData(enums[0], values[0], enums[1], values[1], changes["reversed"]),
                "motion_link_failed", "Fusion refused updated coupling data.")
    if "name" in args:
        link.name = args["name"]
    if "suppressed" in args:
        link.isSuppressed = args["suppressed"]
        if design.snapshots.hasPendingSnapshot:
            require(design.snapshots.add() is not None, "motion_link_capture_failed", "Could not capture link suppression.")
    return lambda fresh: result(link, fresh, effective, first, second, expected)


def motion_link_delete(design, value, args):
    link = resolve(design, value, args["link"], ("MotionLink",))
    require(getattr(link, "assemblyContext", None) is None and same(link.parentComponent, design.rootComponent),
            "root_joint_required", "Delete the root-owned motion link.")
    name = link.name
    require(link.deleteMe(), "delete_failed", "Fusion refused motion-link deletion.")
    return {"deletedMotionLink": name}


OPERATIONS = {name: globals()[name] for name in ("motion_link_create", "motion_link_edit", "motion_link_delete")}
