"""Sketch-space curve edits and signed, parameter-driven native offsets."""
import json
import math

from .core import items, parameter_data, ref, require, resolve, same, target
from .readback import curve_data, sketch_data
from .sketches import get_sketch, point, quantity
from .transactions import equivalent


CURVES = ("SketchLine", "SketchArc", "SketchCircle", "SketchEllipse", "SketchEllipticalArc",
          "SketchFittedSpline", "SketchControlPointSpline", "SketchFixedSpline")


def curve_shapes(sketch_result):
    # Constraint/flag changes or collection reordering alone do not prove that
    # trim/extend/split/break changed curve geometry.
    return sorted(({key: data for key, data in curve.items() if key not in ("fixed", "construction", "ref")}
                   for curve in sketch_result["curves"]), key=lambda row: json.dumps(row, sort_keys=True))


def editable_curve(design, value, reference, sketch):
    curve = resolve(design, value, reference, CURVES)
    require(getattr(curve, "assemblyContext", None) is None and same(curve.parentSketch, sketch),
            "wrong_sketch", "Curve edits require native references in this sketch definition.")
    require(not curve.isLinked and not curve.isReference, "linked_geometry", "Linked/reference geometry must be explicitly unlinked before curve editing.")
    return curve


def sketch_curve_edit(design, value, args):
    require("allowDelete" not in args or args["action"] == "trim", "invalid_input", "allowDelete applies only to trim.")
    sketch = get_sketch(design, value, args)
    require(not sketch.isDerived and not sketch.isComputeDeferred, "sketch_not_editable", "Derived or deferred-compute sketches cannot be edited.")
    curve = editable_curve(design, value, args["curve"], sketch)
    before = sketch_data(sketch)
    selected_before = curve_shapes({"curves": [curve_data(curve)]})
    pick = point(args["pick"])
    if args["action"] == "split":
        ok, parameter = curve.geometry.evaluator.getParameterAtPoint(pick)
        require(ok, "pick_off_curve", "Split point must lie on the selected curve in sketch coordinates.")
        ok, actual = curve.geometry.evaluator.getPointAtParameter(parameter)
        require(ok and actual.distanceTo(pick) <= 1e-7, "pick_off_curve", "Split point must lie on the curve.")
        if hasattr(curve, "startSketchPoint") and hasattr(curve, "endSketchPoint"):
            require(min(pick.distanceTo(curve.startSketchPoint.geometry), pick.distanceTo(curve.endSketchPoint.geometry)) > 1e-7,
                    "invalid_pick", "Split point must be inside the curve, not an endpoint.")
    if args["action"] == "extend" and hasattr(curve, "startSketchPoint") and hasattr(curve, "endSketchPoint"):
        require(abs(pick.distanceTo(curve.startSketchPoint.geometry) - pick.distanceTo(curve.endSketchPoint.geometry)) > 1e-7,
                "ambiguous_pick", "Extension pick is equally near both endpoints.")
    method = "breakCurve" if args["action"] == "break" else args["action"]
    returned = getattr(curve, method)(pick, args.get("createConstraints", True))
    require(returned is not None, "curve_edit_failed", "Fusion returned no curve edit result.")
    replacements = items(returned)
    deleted = not curve.isValid
    # Native extend can mutate the original in place and return an empty
    # collection. The post-compute selected-geometry checks below must still
    # prove the edit; this merely supplies its fresh reference on success.
    if args["action"] == "extend" and not replacements and not deleted:
        replacements = [curve]
    require(replacements or args["action"] == "trim" and deleted and args.get("allowDelete", False),
            "curve_edit_empty", "Edit returned no replacement curves; full trim deletion requires allowDelete:true.")
    def result(fresh):
        require(not equivalent(curve_shapes(before), curve_shapes(sketch_data(sketch))), "curve_edit_unchanged", "Curve edit made no persistent curve geometry change.")
        require((not curve.isValid) == deleted, "curve_edit_failed", "Original curve deletion state changed during recompute.")
        require(deleted or not equivalent(selected_before, curve_shapes({"curves": [curve_data(curve)]})),
                "curve_edit_unchanged", "Selected curve did not change; unrelated sketch changes cannot prove this edit.")
        require(all(item.isValid and same(item.parentSketch, sketch) for item in replacements),
                "curve_edit_failed", "Replacement curve disappeared after recompute.")
        return {"action": args["action"], "space": "sketch", "originalDeleted": deleted,
                "curves": [curve_data(item, fresh) for item in replacements], "sketch": sketch_data(sketch, fresh)}
    return result


def ordered_chain(curves):
    """Validate supplied order; never reorder and silently change offset sign."""
    require(all(curve.is2D for curve in curves), "unsupported_geometry", "Offset requires curves in the sketch XY plane.")
    for index, curve in enumerate(curves):
        require(not any(same(curve, prior) for prior in curves[:index]), "duplicate_entity", "Offset input repeats a curve.")
    if len(curves) == 1: return
    ends = []
    for curve in curves:
        require(hasattr(curve, "startSketchPoint") and hasattr(curve, "endSketchPoint"),
                "disconnected_chain", "Closed standalone curves cannot be mixed into an offset chain.")
        ends.append([curve.startSketchPoint.geometry, curve.endSketchPoint.geometry])
    close = lambda a, b: a.distanceTo(b) <= 1e-7
    shared = [(i, j) for i in (0, 1) for j in (0, 1) if close(ends[0][i], ends[1][j])]
    require(len(shared) == 1, "ambiguous_chain", "First two curves must share exactly one endpoint.")
    previous = ends[1][1-shared[0][1]]
    for pair in ends[2:]:
        match = [index for index, endpoint in enumerate(pair) if close(previous, endpoint)]
        require(len(match) == 1, "disconnected_chain", "Curves must be supplied in connected flow order.")
        previous = pair[1-match[0]]


def sketch_offset(design, value, args):
    sketch = get_sketch(design, value, args)
    require(not sketch.isDerived and not sketch.isComputeDeferred, "sketch_not_editable", "Derived or deferred-compute sketches cannot be edited.")
    curves = [editable_curve(design, value, item, sketch) for item in args["curves"]]
    ordered_chain(curves)
    offset = quantity(design, args["distance"])
    expression = offset.stringValue
    expected = design.unitsManager.evaluateExpression(expression, "mm")
    require(abs(expected) > 1e-8, "invalid_quantity", "Offset distance must be nonzero.")
    constraints = sketch.geometricConstraints
    require(hasattr(constraints, "createOffsetInput") and hasattr(constraints, "addOffset2"),
            "unsupported_capability", "This Fusion runtime lacks the modern signed offset API.")
    inp = constraints.createOffsetInput(curves, offset)
    require(inp is not None, "offset_failed", "Fusion rejected the offset inputs.")
    matched = args.get("topologyMatched", True)
    inp.isTopologyMatched = matched
    constraint = constraints.addOffset2(inp)
    require(constraint is not None and constraint.isValid, "offset_failed", "Fusion did not create an offset constraint.")
    def result(fresh):
        require(constraint.isValid and constraint.dimension is not None, "offset_failed", "Offset or its controlling dimension disappeared.")
        children = list(constraint.childCurves)
        require(children and all(child.isValid for child in children), "offset_failed", "Offset has no valid child curves.")
        require(constraint.isTopologyMatched == matched, "offset_failed", "Native offset topology setting differs from the request.")
        require(math.isclose(constraint.distance, expected, abs_tol=1e-8, rel_tol=1e-8),
                "offset_failed", "Native signed offset distance differs from the requested value.")
        parents = list(constraint.parentCurves)
        require(len(parents) == len(curves) and all(any(same(parent, curve) for parent in parents) for curve in curves),
                "offset_failed", "Native offset parents differ from the selected chain.")
        return {"space": "sketch", "offset": {"ref": ref(constraint, fresh), "distance": constraint.distance * 10,
                "unit": "mm", "requestedExpression": expression, "topologyMatched": constraint.isTopologyMatched,
                "parameter": parameter_data(design, constraint.dimension.parameter),
                "parentRefs": [ref(curve, fresh) for curve in curves],
                "curves": [curve_data(child, fresh) for child in children]}, "sketch": sketch_data(sketch, fresh)}
    return result


def sketch_diagnostics(app, args):
    _, design, value = target(app, args)
    sketch = resolve(design, value, args["sketch"], ("Sketch",))
    entities = items(sketch.sketchCurves) + items(sketch.sketchPoints)
    return {"target": value, "sketch": ref(sketch, value), "fullyConstrained": sketch.isFullyConstrained,
            "healthState": int(sketch.healthState), "message": sketch.errorOrWarningMessage,
            "computeDeferred": sketch.isComputeDeferred,
            "entities": [{"ref": ref(entity, value), "fullyConstrained": entity.isFullyConstrained,
                          "fixed": entity.isFixed, "linked": entity.isLinked, "reference": entity.isReference}
                         for entity in entities],
            "constraints": [{"ref": ref(item, value), "type": item.objectType, "valid": item.isValid}
                            for item in items(sketch.geometricConstraints)],
            "dimensions": [{"name": item.parameter.name, "expression": item.parameter.expression,
                            "driving": item.isDriving} for item in items(sketch.sketchDimensions)],
            "diagnosticScope": "Native health and constraint flags only; no solver degrees-of-freedom or conflict-set explanation."}


OPERATIONS = {"sketch_curve_edit": sketch_curve_edit, "sketch_offset": sketch_offset}
READS = {"sketch_diagnostics": sketch_diagnostics}
