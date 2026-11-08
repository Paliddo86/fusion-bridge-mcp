"""Detailed topology and sketch readback using resolvable guarded references."""
import json
import adsk.core
import adsk.fusion
from .core import MechanicalError, body_data, digest, identity, items, point_mm, ref, require, same


def concrete_geometry(geometry):
    """Read subclass properties even when a getter returns a base API wrapper."""
    kind = getattr(adsk.core, geometry.objectType.split("::")[-1], None)
    if kind is not None and hasattr(kind, "cast"):
        geometry = kind.cast(geometry)
        require(geometry is not None, "unavailable_geometry", "Cannot cast native geometry to its advertised type.")
    return geometry


def geometry_data(geometry):
    geometry = concrete_geometry(geometry)
    result = {"type": geometry.objectType}
    for name in ("startPoint", "endPoint", "center", "origin"):
        if hasattr(geometry, name):
            result[name] = point_mm(getattr(geometry, name))
    for name in ("normal", "direction", "majorAxis", "minorAxis"):
        if hasattr(geometry, name):
            p = getattr(geometry, name)
            result[name] = [p.x, p.y, p.z]
    for name in ("radius", "majorRadius", "minorRadius"):
        if hasattr(geometry, name):
            result[name] = getattr(geometry, name) * 10
    if geometry.objectType.endswith("::NurbsCurve3D"):
        ok, controls, degree, knots, rational, weights, periodic = geometry.getData()
        require(ok, "unavailable_geometry", "Cannot read complete spline geometry for the semantic witness.")
        result.update(controlPoints=[point_mm(p) for p in controls], knots=list(knots), weights=list(weights),
                      degree=degree, rational=rational, periodic=periodic)
    return result


def edge_geometry_data(edge):
    geometry = edge.geometry
    if geometry is not None:
        return geometry_data(geometry)
    require(edge.isDegenerate, "unavailable_geometry", "Nondegenerate BRep edge has no readable curve geometry.")
    vertices = [point_mm(vertex.geometry) for vertex in (edge.startVertex, edge.endVertex) if vertex is not None]
    require(vertices, "unavailable_geometry", "Degenerate BRep edge has no readable vertex position.")
    return {"type": "degenerate", "isDegenerate": True, "vertices": vertices}


def profile_signature(profile):
    props = profile.areaProperties()
    return {"area": props.area * 100, "centroid": point_mm(props.centroid), "loops": profile.profileLoops.count}


def profile_ref(sketch, profile, index, value):
    return {**value, "objectType": "adsk::fusion::Profile", "sketchToken": sketch.entityToken,
            "profileIndex": index, "fingerprint": digest(profile_signature(profile))}


def curve_data(curve, value=None):
    result = {"type": curve.objectType, "construction": curve.isConstruction,
              "fixed": curve.isFixed, "geometry": geometry_data(curve.geometry)}
    if value:
        result["ref"] = ref(curve, value)
    for name in ("startSketchPoint", "endSketchPoint", "centerSketchPoint"):
        if hasattr(curve, name):
            point = getattr(curve, name)
            result[name] = {"position": point_mm(point.geometry)}
            if value:
                result[name]["ref"] = ref(point, value)
    if hasattr(curve, "fitPoints"):
        result["fitPoints"] = [point_mm(p.geometry) for p in items(curve.fitPoints)]
    return result


def frame_data(matrix):
    """A Matrix3D as a frame: origin in mm and three unit directions, in component space."""
    origin, x, y, z = matrix.getAsCoordinateSystem()
    return {"origin": point_mm(origin), "xDirection": [x.x, x.y, x.z], "yDirection": [y.x, y.y, y.z],
            "normal": [z.x, z.y, z.z], "space": "component", "lengthUnit": "mm"}


def to_component(sketch, point):
    """A sketch-space point in its component's space, without changing the original."""
    moved = point.copy()
    moved.transformBy(sketch.transform)
    return point_mm(moved)


def plane_data(plane, value=None):
    """A construction plane (the three origin planes included): its frame in component space."""
    from .core import origin_plane_key
    geometry = plane.geometry
    result = {"name": plane.name, "originPlane": origin_plane_key(plane), "space": "component", "lengthUnit": "mm",
              "origin": point_mm(geometry.origin), "normal": [geometry.normal.x, geometry.normal.y, geometry.normal.z]}
    for name in ("uDirection", "vDirection"):
        if hasattr(geometry, name):
            v = getattr(geometry, name)
            result[name] = [v.x, v.y, v.z]
    if value:
        result["ref"] = ref(plane, value)
    return result


def sketch_data(sketch, value=None, strict=True):
    """A sketch's curves, points and constraint graph. strict (the default) refuses a relationship the
    graph reader cannot read completely (unsupported_constraint_graph); otherwise that relationship, or a
    curve whose geometry cannot be read, is reported as its type with the reason, and constraintGraph says
    the graph is incomplete. A fully readable sketch reads the same either way."""
    curves, points = items(sketch.sketchCurves), items(sketch.sketchPoints)
    constraints, dimensions = items(sketch.geometricConstraints), items(sketch.sketchDimensions)
    graph = SketchGraph(curves, points, constraints)
    unreadable = []

    def read(reader, fallback):
        if strict:
            return reader()
        try:
            return reader()
        except (MechanicalError, AttributeError, RuntimeError) as error:
            if isinstance(error, MechanicalError) and error.code not in ("unsupported_constraint_graph", "unavailable_geometry"):
                raise
            row = fallback()
            row["unreadable"] = str(error)[:300]
            unreadable.append({"type": row.get("type"), "reason": row["unreadable"]})
            return row

    result = {"name": sketch.name, "space": "sketch", "lengthUnit": "mm",
              "fullyConstrained": sketch.isFullyConstrained,
              "curves": [read(lambda: curve_data(curve, value), lambda: {"type": curve.objectType}) for curve in curves],
              "points": [{"position": point_mm(p.geometry), "fixed": p.isFixed, **({"ref": ref(p, value)} if value else {})}
                         for p in points],
              "curvePointLinks": [read(lambda: graph.curve_points(curve), lambda: {"type": curve.objectType}) for curve in curves],
              "constraints": [read(lambda: constraint_data(constraint, graph), lambda: {"type": constraint.objectType})
                              for constraint in constraints],
              "dimensions": [read(lambda: dimension_data(dimension, graph), lambda: dimension_summary(dimension))
                             for dimension in dimensions]}
    if unreadable:
        result["constraintGraph"] = {"complete": False, "unreadable": unreadable}
    if value:
        # where the sketch sits: its x, y and normal in component space, so a sketch-space point
        # (every curve, point and centroid above) maps to the model without guessing the plane
        result["frame"] = frame_data(sketch.transform)
        result["ref"] = ref(sketch, value)
        result["originRef"] = ref(sketch.originPoint, value)
        result["profiles"] = [{**profile_signature(profile),
                               "centroidComponent": to_component(sketch, profile.areaProperties().centroid),
                               "ref": profile_ref(sketch, profile, index, value)}
                              for index, profile in enumerate(items(sketch.profiles))]
    return result


CONSTRAINT_LINKS = {
    "CoincidentConstraint": ("point", "entity"),
    "HorizontalConstraint": ("line",), "VerticalConstraint": ("line",),
    "HorizontalPointsConstraint": ("pointOne", "pointTwo"),
    "VerticalPointsConstraint": ("pointOne", "pointTwo"),
    "ParallelConstraint": ("lineOne", "lineTwo"), "CollinearConstraint": ("lineOne", "lineTwo"),
    "PerpendicularConstraint": ("lineOne", "lineTwo"),
    "EqualConstraint": ("curveOne", "curveTwo"), "TangentConstraint": ("curveOne", "curveTwo"),
    "SmoothConstraint": ("curveOne", "curveTwo"), "ConcentricConstraint": ("entityOne", "entityTwo"),
    "MidPointConstraint": ("point", "midPointCurve"),
    "SymmetryConstraint": ("entityOne", "entityTwo", "symmetryLine"),
    "OffsetConstraint": (),
}
DIMENSION_LINKS = {
    "SketchLinearDimension": ("entityOne", "entityTwo"),
    "SketchAngularDimension": ("lineOne", "lineTwo"),
    "SketchRadialDimension": ("entity",), "SketchDiameterDimension": ("entity",),
    "SketchOffsetDimension": ("line", "entityTwo"),
    "SketchOffsetCurvesDimension": ("offsetConstraint",),
    "SketchConcentricCircleDimension": ("circleOne", "circleTwo"),
    "SketchEllipseMajorRadiusDimension": ("ellipse",), "SketchEllipseMinorRadiusDimension": ("ellipse",),
    "SketchLinearDiameterDimension": ("line", "entityTwo"),
    "SketchTangentDistanceDimension": ("entityOne", "circleOrArc"),
}


class SketchGraph:
    """Links use local inventory nodes resolved by API equality, never tokens.

    The associated geometry inventories are part of the same witness. Native
    reordering may conservatively refuse recovery; retargeting distinct nodes
    cannot disappear merely because their current positions coincide.
    """
    def __init__(self, curves, points, constraints):
        self.nodes = {"curves": curves, "points": points, "constraints": constraints}
        self.comparisons = 0
        self.direct = {}
        for name, entities in self.nodes.items():
            for index, entity in enumerate(entities):
                self.direct.setdefault(id(entity), []).append({"collection": name, "index": index})

    def node(self, entity):
        matches = self.direct.get(id(entity))
        if matches is None:
            self.comparisons += sum(len(entities) for entities in self.nodes.values())
            require(self.comparisons <= 1000000, "inspection_budget", "Native sketch graph equality lookup exceeds inspection budget.")
            matches = [{"collection": name, "index": index}
                       for name, entities in self.nodes.items() for index, item in enumerate(entities)
                       if entity is not None and same(entity, item)]
        require(len(matches) == 1, "unsupported_constraint_graph",
                "Sketch graph link is null, ambiguous or outside the readable native sketch inventory.")
        return matches[0]

    def links(self, item, fields):
        try:
            return {name: self.node(getattr(item, name)) for name in fields}
        except AttributeError as exc:
            require(False, "unsupported_constraint_graph", "Native constraint relationship is unavailable.",
                    objectType=item.objectType, detail=str(exc))

    def curve_points(self, curve):
        result = self.links(curve, [name for name in ("startSketchPoint", "endSketchPoint", "centerSketchPoint") if hasattr(curve, name)])
        if hasattr(curve, "fitPoints"):
            result["fitPoints"] = [self.node(p) for p in items(curve.fitPoints)]
        return result


def graph_item(item, supported):
    kind = item.objectType.split("::")[-1]
    require(kind in supported, "unsupported_constraint_graph",
            "No complete recovery graph reader for this native constraint/dimension type.", objectType=item.objectType)
    subtype = getattr(adsk.fusion, kind, None)
    if subtype is not None:
        item = subtype.cast(item)
        require(item is not None, "unsupported_constraint_graph", "Cannot cast native sketch relationship.")
    require(item.isValid, "unsupported_constraint_graph", "Native sketch relationship is no longer valid.")
    return item, kind


def constraint_data(constraint, graph):
    constraint, kind = graph_item(constraint, CONSTRAINT_LINKS)
    result = {"type": constraint.objectType}
    fields = CONSTRAINT_LINKS[kind]
    if kind == "PerpendicularConstraint" and hasattr(constraint, "curve"):
        # September 2026 added line/curve perpendicularity. Earlier runtimes
        # expose only the two-line relation; do not invent the missing link.
        fields = ("line", "curve")
    result["links"] = graph.links(constraint, fields)
    if kind == "OffsetConstraint":
        # Offset links and sign belong to the recovery witness, beyond the
        # number of constraints and generated curve geometry.
        result.update(distance=constraint.distance * 10, topologyMatched=constraint.isTopologyMatched,
                      parents=sorted([graph.node(curve) for curve in constraint.parentCurves], key=lambda row: row["index"]),
                      children=sorted([graph.node(curve) for curve in constraint.childCurves], key=lambda row: row["index"]))
    return result


def dimension_summary(dimension):
    """What any sketch dimension says about itself, for one whose links cannot be read."""
    row = {"type": dimension.objectType}
    try:
        parameter = dimension.parameter
        row.update(name=parameter.name if parameter else None, expression=parameter.expression if parameter else None,
                   driving=dimension.isDriving)
    except (AttributeError, RuntimeError):
        pass
    return row


def dimension_data(dimension, graph):
    dimension, kind = graph_item(dimension, DIMENSION_LINKS)
    parameter = dimension.parameter
    result = {"type": dimension.objectType, "name": parameter.name if parameter else None,
              "expression": parameter.expression if parameter else None, "driving": dimension.isDriving,
              "value": dimension.value * (180 / 3.141592653589793 if kind == "SketchAngularDimension" else 10),
              "unit": "deg" if kind == "SketchAngularDimension" else "mm",
              "links": graph.links(dimension, DIMENSION_LINKS[kind])}
    if kind == "SketchLinearDimension": result["orientation"] = int(dimension.orientation)
    return result


def construction_state(component):
    from .geometry import axis_data
    return [axis_data(axis) for axis in items(component.constructionAxes)] if hasattr(component, "constructionAxes") else []


def topology_data(body, value):
    result = body_data(body)
    result["ref"] = ref(body, value)
    result["faces"] = [{"ref": ref(face, value), "area": face.area * 100,
                         "point": point_mm(face.pointOnFace), "surface": geometry_data(face.geometry),
                         "edges": [ref(edge, value) for edge in items(face.edges)]}
                        for face in items(body.faces)]
    result["edges"] = [{"ref": ref(edge, value), "length": edge.length * 10,
                         "curve": edge_geometry_data(edge)} for edge in items(body.edges)]
    return result


# topology witnesses by body: a body's revisionId changes whenever its geometry does, and the rest of the
# witness (its name, and its mass through its material) is in the key, so a write reads face by face only
# the bodies that changed since the last witness instead of every body in the design
_WITNESSES = {}


def topology_witness(body):
    material = body.material
    key = (identity(body), str(body.revisionId), body.name, material.id if material is not None else None)
    cached = _WITNESSES.get(key)
    if cached is not None:
        return json.loads(cached)
    result = _topology_witness(body)
    if len(_WITNESSES) > 50000:
        _WITNESSES.clear()
    _WITNESSES[key] = json.dumps(result)
    return result


def _topology_witness(body):
    result = body_data(body)
    # Compare geometry rather than revision or transient topology IDs after undo.
    result["faceGeometry"] = sorted([
        {"area": f.area * 100, "point": point_mm(f.pointOnFace), "surface": geometry_data(f.geometry)}
        for f in items(body.faces)], key=lambda row: json.dumps(row, sort_keys=True))
    result["edgeGeometry"] = sorted([
        {"length": e.length * 10, "curve": edge_geometry_data(e)} for e in items(body.edges)],
        key=lambda row: json.dumps(row, sort_keys=True))
    return result


# features whose timeline position Fusion will not give (imported or grouped features raise on the
# index getter), keyed by identity and their component's revision: each refused read cost about 12 ms,
# three quarters of a 653-feature digest's index reads, for an index the digest never had
_UNINDEXED = set()


def feature_data(feature, value=None, order=None):
    result = {"name": feature.name, "type": feature.objectType}
    if order is not None:
        result["componentFeatureOrder"] = order
    if value:
        result["ref"] = ref(feature, value)
    timeline = getattr(feature, "timelineObject", None)
    if timeline:
        result["suppressed"] = timeline.isSuppressed
        owner = getattr(feature, "parentComponent", None)
        known = (identity(feature), str(owner.revisionId) if owner is not None else None)
        if known in _UNINDEXED:
            result.update(index=None, indexUnavailable="native_timeline_position_unavailable")
            return result
        try:
            result["index"] = timeline.index
        except RuntimeError as exc:
            # Imported/native grouped features can have a valid timeline object
            # whose index getter rejects its documented non-indexed position.
            # Only this known getter failure is optional; retain name, type,
            # suppression and component collection order in semantic witnesses.
            if "InternalValidationError : res >= 0" not in str(exc):
                raise
            require(timeline.isValid, "invalid_timeline", "Feature timeline object is no longer valid.")
            _UNINDEXED.add(known)
            result.update(index=None, indexUnavailable="native_timeline_position_unavailable")
    return result
