"""Typed mechanical operations. Add new operations here behind shared guards."""
import re

import adsk.core
import adsk.fusion

from .core import (MechanicalError, components_by_occurrence, active, body_data, current_target, health,
                   identity, items, number, parameter_data, recovery_snapshot,
                   ref, require, require_healthy, resolve, target, UNITS)
from . import compute
from .exporting import export_design


def inspect(app, args):
    from .assembly_state import metadata, joint_data, rigid_group_data
    doc, design, key = active(app)
    value = target(app, args)[2] if args.get("target") else current_target(app)
    if args.get("detail") == "target":
        # just the guard a write or a BOM needs, and the size: a full inspect of a big assembly
        # (thousands of bodies) cannot answer inside the relay's budget
        return {"target": value, "document": {"name": doc.name, "creationId": doc.creationId, "saved": doc.isSaved},
                "counts": {"components": design.allComponents.count, "occurrences": design.rootComponent.allOccurrences.count,
                           "timeline": design.timeline.count}}
    bodies, components, occurrences, sketches, features, axes, planes = [], [], [], [], [], [], []
    # part numbers and descriptions are cloud reads (~0.6 s each per component on a cloud document):
    # read them for a small design, or when asked; a big one gets them from bom or inspect_entity
    native = args.get("componentProperties", design.allComponents.count <= 200)
    # body metrics (physical properties and a bounding box, about 10 ms a body) and timeline health
    # (about 22 ms a timeline item) are read for a small design or when asked: on a 4,579-occurrence
    # assembly they were 40 s of a 103 s inspect; inspect_entity and the health op read them for one thing
    metrics = args.get("bodyMetrics", design.allComponents.count <= 200)
    with_health = args.get("health", design.timeline.count <= 200)
    # allComponents.item() costs about 5 ms a component, and inspect walked the collection five times
    all_occurrences = items(design.rootComponent.allOccurrences)
    all_components = components_by_occurrence(design, all_occurrences, design.allComponents.count)
    for comp in all_components:
        components.append({"id": identity(comp), **metadata(comp, native), "ref": ref(comp, value),
                           "bodyCount": comp.bRepBodies.count, "sketchCount": comp.sketches.count})
        for body in items(comp.bRepBodies):
            row = body_data(body) if metrics else {"name": body.name, "space": "component"}
            row.update({"ref": ref(body, value), "componentId": identity(comp)})
            bodies.append(row)
        for sketch in items(comp.sketches):
            sketches.append({"ref": ref(sketch, value), "name": sketch.name, "componentId": identity(comp),
                             "profileCount": sketch.profiles.count, "fullyConstrained": sketch.isFullyConstrained})
        for index, feature in enumerate(items(comp.features)):
            from .readback import feature_data
            features.append(feature_data(feature, value, order=index))
        from .geometry import axis_data
        axes.extend(axis_data(axis, value) for axis in items(comp.constructionAxes))
        # the three origin planes and every construction plane, so a plane reference can be read
        # again after a write (each write moves the revision) and a base plane is referenceable
        from .readback import plane_data
        for plane in [comp.xYConstructionPlane, comp.xZConstructionPlane, comp.yZConstructionPlane] + items(comp.constructionPlanes):
            planes.append({**plane_data(plane, value), "componentId": identity(comp)})
    for occ in all_occurrences:
        matrix = list(occ.transform2.asArray())
        # Matrix4x4.asArray is row-major; translation lies at 3,7,11.
        for i in (3, 7, 11):
            matrix[i] *= 10
        occurrences.append({"ref": ref(occ, value), "componentId": identity(occ.component),
                            "path": occ.fullPathName, "transform": matrix,
                            "transformLayout": "row-major", "translationUnit": "mm",
                            "space": "root", "grounded": occ.isGrounded})
    from .joint_state import origin_data, link_data
    from .configurations import snapshot as configuration_snapshot
    configuration = configuration_snapshot(design)
    return {"target": value, "componentPropertiesRead": bool(native), "bodyMetricsRead": bool(metrics),
            "document": {"name": doc.name, "creationId": doc.creationId,
            "saved": doc.isSaved}, "designIntent": str(getattr(design, "designIntent", "unavailable")),
            "units": UNITS, "components": components, "bodies": bodies, "occurrences": occurrences,
            "sketches": sketches, "features": features, "constructionAxes": axes, "constructionPlanes": planes,
            "joints": [joint_data(j, value) for comp in all_components
                       for name in ("joints", "asBuiltJoints") for j in items(getattr(comp, name))],
            "rigidGroups": [rigid_group_data(g, value) for comp in all_components for g in items(comp.rigidGroups)],
            "jointOrigins": [origin_data(o, value) for comp in all_components for o in items(comp.jointOrigins)],
            "motionLinks": [link_data(o, value) for comp in all_components for o in items(comp.motionLinks)] if hasattr(design.rootComponent, "motionLinks") else [],
            "parameters": [parameter_data(design, p) for p in items(design.allParameters)],
            "health": health(design) if with_health else {"skipped": True, "timelineCount": design.timeline.count,
                                                        "hint": "Big timeline: run op health, or pass health:true."},
            "complete": configuration["complete"], "configuration": configuration}


def inspect_entity(app, args):
    _, design, value = target(app, args)
    entity = resolve(design, value, args.get("entity"))
    result = {"ref": ref(entity, value), "objectType": entity.objectType, "name": getattr(entity, "name", None)}
    if entity.objectType.endswith("::BRepBody"):
        from .readback import topology_data
        result.update(topology_data(entity, value))
        result["space"] = "assembly_context" if args["entity"].get("occurrenceToken") else "component"
        result["faceRefs"] = [ref(f, value) for f in items(entity.faces)]
        result["edgeRefs"] = [ref(e, value) for e in items(entity.edges)]
    elif entity.objectType.endswith("::Sketch"):
        from .readback import sketch_data
        # a read: a relationship the graph reader does not support is listed in constraintGraph
        result.update(sketch_data(entity, value, strict=False))
    elif entity.objectType.endswith("::Occurrence"):
        result["bodyRefs"] = [ref(b, value, entity) for b in items(entity.component.bRepBodies)]
        result["childRefs"] = [ref(o, value) for o in items(entity.childOccurrences)]
        result["jointOriginRefs"] = [ref(o, value, entity) for o in items(entity.component.jointOrigins)]
    elif entity.objectType.endswith(("::AsBuiltJoint", "::Joint")):
        from .assembly_state import joint_data
        result.update(joint_data(entity, value))
    elif entity.objectType.endswith("::Component"):
        from .assembly_state import metadata
        result.update(metadata(entity))
    elif entity.objectType.endswith("::RigidGroup"):
        from .assembly_state import rigid_group_data
        result.update(rigid_group_data(entity, value))
    elif entity.objectType.endswith("::ConstructionAxis"):
        from .geometry import axis_data
        result.update(axis_data(entity, value))
        result["space"] = "root" if getattr(entity, "assemblyContext", None) else "component"
    elif entity.objectType.endswith("::JointOrigin"):
        from .joint_state import origin_data
        result.update(origin_data(entity, value))
    elif entity.objectType.endswith("::MotionLink"):
        from .joint_state import link_data
        result.update(link_data(entity, value))
    return {"target": value, "entity": result}


def new_design(app, args):
    name = args.get("name", "Adom mechanical")
    require(isinstance(name, str) and 0 < len(name) <= 120, "invalid_input", "name must be 1..120 characters.")
    box = args.get("box")
    if box is not None:
        require(isinstance(box, dict), "invalid_input", "box must contain width, height and depth in mm.")
        dims = [number(box.get(n), n, positive=True) for n in ("width", "height", "depth")]
    previous = app.activeDocument
    doc = None
    try:
        doc = app.documents.add(adsk.core.DocumentTypes.FusionDesignDocumentType)
        require(doc is not None, "create_failed", "Fusion did not create a document.")
        design = adsk.fusion.Design.cast(doc.products.itemByProductType("DesignProductType"))
        require(design is not None, "create_failed", "Created document has no Design product.")
        design.designType = adsk.fusion.DesignTypes.ParametricDesignType
        from . import transactions
        if not transactions.capability()["verified"]:
            transactions.probe(app, {"target": current_target(app)})
        if box is not None:
            create_box(design, dims)
        require_healthy(design)
        # Naming a never-saved Fusion document makes close(False) capable of
        # raising a save dialog. Keep the scratch document unnamed and store the
        # requested label as model metadata until an explicit save operation.
        design.rootComponent.attributes.add("AdomMechanical", "requestedName", name)
        result = inspect(app, {})
        result["document"]["requestedName"] = name
        return result
    except Exception as exc:
        recovery = {"strategy": "discard_new_unsaved_document", "verified": False}
        if doc is not None:
            try:
                require(not doc.isSaved, "recovery_refused", "New document unexpectedly saved.")
                require(doc.name.lower().startswith("untitled"), "recovery_refused", "Document was renamed; refusing a close that could prompt.")
                doc.close(False)
                recovery["verified"] = not doc.isValid
                if previous and previous.isValid:
                    previous.activate()
            except Exception as close_error:
                recovery["error"] = str(close_error)
        raise MechanicalError("create_failed", str(exc), recovery=recovery)


def create_box(design, dimensions):
    root = design.rootComponent
    for name, value in zip(("width", "height", "depth"), dimensions):
        design.userParameters.add(name, adsk.core.ValueInput.createByString(str(value) + " mm"), "mm", "Adom driving dimension")
    width, height, _ = [n / 10 for n in dimensions]
    sketch = root.sketches.add(root.xYConstructionPlane)
    sketch.name = "Base rectangle"
    origin = adsk.core.Point3D.create(0, 0, 0)
    lines = sketch.sketchCurves.sketchLines.addTwoPointRectangle(origin, adsk.core.Point3D.create(width, height, 0))
    first = lines.item(0)
    second = lines.item(1)
    orient = adsk.fusion.DimensionOrientations
    sx = sketch.sketchDimensions.addDistanceDimension(first.startSketchPoint, first.endSketchPoint,
            orient.HorizontalDimensionOrientation, adsk.core.Point3D.create(width / 2, -height / 3, 0))
    sy = sketch.sketchDimensions.addDistanceDimension(second.startSketchPoint, second.endSketchPoint,
            orient.VerticalDimensionOrientation, adsk.core.Point3D.create(width * 1.3, height / 2, 0))
    sx.parameter.expression, sy.parameter.expression = "width", "height"
    # addTwoPointRectangle at the origin supplies the origin coincidence itself.
    feature_input = root.features.extrudeFeatures.createInput(sketch.profiles.item(0), adsk.fusion.FeatureOperations.NewBodyFeatureOperation)
    extent = adsk.fusion.DistanceExtentDefinition.create(adsk.core.ValueInput.createByString("depth"))
    feature_input.setOneSideExtent(extent, adsk.fusion.ExtentDirections.PositiveExtentDirection)
    feature = root.features.extrudeFeatures.add(feature_input)
    feature.name = "Base extrusion"
    feature.bodies.item(0).name = "Block"


def parameter_set(app, args):
    _, design, before_target = target(app, args, True)
    healthy = compute.health_before(design, before_target["documentId"], before_target["revision"])
    name, expression = args.get("name"), args.get("expression")
    require(isinstance(name, str) and isinstance(expression, str) and expression.strip(), "invalid_input", "name and expression are required strings.")
    parameter = design.allParameters.itemByName(name)
    require(parameter is not None, "parameter_missing", "No parameter named " + name)
    require(design.unitsManager.isValidExpression(expression, parameter.unit), "invalid_expression", "Expression is invalid for the parameter's unit.")
    before, old = recovery_snapshot(design, healthy), parameter.expression
    # a model parameter is its feature's: a fault left on that feature is this write's own result
    owner = compute.owner_of(design, name)
    try:
        parameter.expression = expression
        state, settled = compute.settle(design, before_target["documentId"], args, healthy, owner)
    except Exception as exc:
        recovery = {"strategy": "restore_parameter_expression", "verified": False,
                    "scope": "parameter expressions, body metrics, occurrence transforms, counts and health"}
        try:
            parameter.expression = old
            compute.restored(design, healthy)
            recovery["verified"] = recovery_snapshot(design) == before
        except Exception as restore_error:
            recovery["error"] = str(restore_error)
        details = {"recovery": recovery, "target": current_target(app)}
        if isinstance(exc, MechanicalError) and exc.code == "unhealthy_model":
            details.update(causeCode=exc.code, causeDetails=exc.details)
        raise MechanicalError("parameter_edit_failed", str(exc), **details)
    fresh = current_target(app)
    return {"target": fresh, "parameter": parameter_data(design, parameter), "health": state,
            "preExistingFaults": compute.summary(healthy, state),
            "compute": compute.committed(before_target["documentId"], fresh["revision"], state, settled),
            "recoveryStrategy": "restore_parameter_expression", "transactional": False}


def parameter_create(app, args):
    _, design, value = target(app, args, True)
    healthy = compute.health_before(design, value["documentId"], value["revision"])
    name, expression, unit = args.get("name"), args.get("expression"), args.get("unit")
    require(isinstance(name, str) and re.fullmatch(r"[A-Za-z][A-Za-z0-9_]{0,63}", name), "invalid_input", "Use a short parameter identifier.")
    require(unit in ("mm", "deg", "kg", ""), "invalid_input", "unit must be mm, deg, kg or empty for unitless.")
    require(isinstance(expression, str) and design.unitsManager.isValidExpression(expression, unit), "invalid_expression", "Expression must match the declared unit.")
    require(design.allParameters.itemByName(name) is None, "parameter_exists", "Parameter already exists.")
    before = recovery_snapshot(design, healthy)
    parameter = None
    try:
        parameter = design.userParameters.add(name, adsk.core.ValueInput.createByString(expression), unit, args.get("comment", ""))
        require(parameter is not None, "parameter_create_failed", "Fusion returned no parameter.")
        state, settled = compute.settle(design, value["documentId"], args, healthy)
    except Exception as exc:
        recovery = {"strategy": "delete_created_parameter", "verified": False}
        try:
            created = parameter or design.userParameters.itemByName(name)
            if created:
                created.deleteMe()
            compute.restored(design, healthy)
            recovery["verified"] = recovery_snapshot(design) == before
        except Exception as restore_error:
            recovery["error"] = str(restore_error)
        details = {"recovery": recovery}
        if isinstance(exc, MechanicalError) and exc.code == "unhealthy_model":
            details.update(causeCode=exc.code, causeDetails=exc.details)
        raise MechanicalError("parameter_create_failed", str(exc), **details)
    fresh = current_target(app)
    return {"target": fresh, "parameter": parameter_data(design, parameter), "health": state,
            "preExistingFaults": compute.summary(healthy, state),
            "compute": compute.committed(value["documentId"], fresh["revision"], state, settled), "transactional": False}


def check_health(app, args):
    _, design, value = target(app, args, args.get("recompute", False))
    if args.get("recompute", False):
        state = compute.checkpoint(design, value["documentId"], "requested")
    else:
        state = health(design, False)
    return {"target": current_target(app), "health": state, "compute": compute.status(value["documentId"])}


def export(app, args):
    _, design, value = target(app, args, True, model_write=False)
    # faults the model already carries leave with it and are reported; the recompute may not add one
    baseline = compute.health_before(design, value["documentId"], value["revision"])
    if compute.pending(value["documentId"]):
        # the design leaves Fusion: the edits since the last full recompute get one now
        compute.judge(compute.checkpoint(design, value["documentId"], "export"), baseline,
                      compute=compute.status(value["documentId"]))
    result = export_design(design, args)
    result["target"] = current_target(app)
    result["compute"] = compute.status(value["documentId"])
    result["preExistingFaults"] = compute.summary(baseline)
    return result


OPERATIONS = {"inspect": inspect, "inspect_entity": inspect_entity, "new_design": new_design,
              "parameter_set": parameter_set, "parameter_create": parameter_create,
              "health": check_health, "export": export}
WRITES = {"new_design", "parameter_set", "parameter_create", "export"}
