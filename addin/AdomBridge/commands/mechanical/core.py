"""Main-thread mechanical identity, revisions, references and health.

Session IDs are capabilities for this running add-in, not persistent CAD IDs. Tokens
are only resolver inputs: they are never compared to decide entity identity.
"""
from contextlib import contextmanager
import hashlib
import json
import math
import threading
import uuid

import adsk.core
import adsk.fusion

SESSION_ID = str(uuid.uuid4())
_documents = []
_entities = []
MAX_ENTITIES = 20000
UNITS = {"length": "mm", "angle": "deg", "mass": "kg", "area": "mm2", "volume": "mm3"}


class MechanicalError(Exception):
    def __init__(self, code, message, **details):
        super().__init__(message)
        self.code, self.details = code, details


def require(condition, code, message, **details):
    if not condition:
        raise MechanicalError(code, message, **details)


# The write in progress and the model's health before it, the baseline it is held to (compute.judge), for
# a check inside the operation. Fusion runs one write at a time on its main thread.
_write = None


@contextmanager
def write_context(baseline):
    global _write
    previous, _write = _write, {"baseline": baseline}
    try:
        yield _write
    finally:
        _write = previous


def write_baseline():
    return _write["baseline"] if _write is not None else None


def items(collection):
    require(collection.count <= MAX_ENTITIES, "inspection_budget", "Collection exceeds inspection budget.")
    return [collection.item(i) for i in range(collection.count)]


def same(a, b):
    return a is b or a == b


def concrete(entity):
    """Cast Base-returning Fusion getters before reading subclass context/state."""
    if entity is None:
        return None
    cls = getattr(adsk.fusion, entity.objectType.split("::")[-1], None)
    if cls is not None and hasattr(cls, "cast"):
        entity = cls.cast(entity)
        require(entity is not None, "unsupported_entity_wrapper", "Could not cast the advertised Fusion entity type.")
    return entity


# Lookup indexes over _entities. A lookup used to compare against every entity registered this
# session, so a 1,249-component assembly (about 10,000 identities) made the revision guard run for
# many minutes. The indexes only narrow the candidates; equality is still native object equality.
_identity_buckets = {}
_identity_tokens = {}
_identity_token_of = {}  # identity key -> the token read when the entity was registered
MAX_IDENTITIES = 100000


def _identity_bucket(entity):
    """Type plus name and owner (an occurrence's full path): same-named entities share a bucket and
    are told apart by native equality, as before."""
    kind = getattr(entity, "objectType", "")
    try:
        if kind.endswith("::Occurrence"):
            return (kind, entity.fullPathName)
        owner = getattr(entity, "parentComponent", None) or getattr(entity, "parentSketch", None) or getattr(entity, "body", None)
        return (kind, getattr(entity, "name", None), getattr(owner, "name", None))
    except Exception:
        return (kind,)


def _identity_token(entity):
    try:
        return (entity.objectType, entity.entityToken)
    except Exception:
        return None


def identity(entity):
    """Object equality, never name or token equality. Retained for this session only. The candidates
    are the entity's bucket, then (for one renamed since) its token's; an entity whose name and token
    both changed gets a new identity, which reads as a changed revision, never a false match."""
    if not _entities and (_identity_buckets or _identity_tokens):
        _identity_buckets.clear()  # the registry was reset
        _identity_tokens.clear()
        _identity_token_of.clear()
    bucket = _identity_bucket(entity)
    for obj, key in _identity_buckets.get(bucket, ()):
        if obj.isValid and same(obj, entity):
            return key
    # only a miss (new, or renamed since) reads the token: an occurrence's entityToken costs about 2 ms,
    # which was a quarter of every revision digest on a 4,579-occurrence assembly
    token = _identity_token(entity)
    for obj, key in _identity_tokens.get(token, ()) if token else ():
        if obj.isValid and same(obj, entity):
            _identity_buckets.setdefault(bucket, []).append((obj, key))
            _identity_token_of[key] = token[1]
            return key
    require(len(_entities) < MAX_IDENTITIES, "identity_budget", "Session entity budget exhausted.")
    key = str(uuid.uuid4())
    _entities.append((entity, key))
    _identity_buckets.setdefault(bucket, []).append((entity, key))
    if token:
        _identity_tokens.setdefault(token, []).append((entity, key))
        _identity_token_of[key] = token[1]
    return key


def entity_token(entity):
    """The token a reference carries. Reading an occurrence's costs about 5 ms, which was over half of
    inspect on a 4,579-occurrence assembly, so an occurrence registered this session reuses the token
    read when it was registered: Fusion resolves every token it gave an entity to that same entity."""
    if entity.objectType.endswith("::Occurrence"):
        token = _identity_token_of.get(identity(entity))
        if token:
            return token
    return entity.entityToken


def document_identity(doc):
    require(threading.current_thread() is threading.main_thread(), "wrong_thread", "Document identity requires the Fusion main thread.")
    require(doc is not None and doc.isValid, "wrong_document", "Document identity requires a live document.")
    for previous, key in _documents:
        if previous.isValid and same(previous, doc):
            return key
    key = SESSION_ID + ":" + str(uuid.uuid4())
    _documents.append((doc, key))
    return key


def active(app):
    require(threading.current_thread() is threading.main_thread(), "wrong_thread", "Mechanical API requires the Fusion main thread.")
    doc = app.activeDocument
    design = adsk.fusion.Design.cast(app.activeProduct)
    require(doc and design, "no_design", "The active product must be a Fusion Design.")
    require(same(design.parentDocument, doc), "wrong_document", "Active product and document disagree.")
    return doc, design, document_identity(doc)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()).hexdigest()


def _revision_document_observation(design, document):
    """Bounded, independently readable facts even when one native getter fails."""
    result = {"complete": True, "errors": []}
    for field, getter in (
        ("designValid", lambda: design.isValid),
        ("documentValid", lambda: document.isValid),
        ("ownerVerified", lambda: same(design.parentDocument, document)),
        ("isSaved", lambda: document.isSaved),
        ("isModified", lambda: document.isModified),
    ):
        try:
            value = getter()
            require(type(value) is bool, "revision_unavailable", "Native document observation is not boolean.")
            result[field] = value
        except Exception as error:
            result[field] = None
            result["complete"] = False
            result["errors"].append({"field": field, "error": str(error)[:512]})
    return result


def reference_version(component):
    """A component inserted by reference: its source document's identity and version, which is what
    changes when someone edits it there (the edit arrives here as a new version)."""
    try:
        data_file = component.parentDesign.parentDocument.dataFile
        return {"dataFileId": data_file.id, "version": data_file.versionNumber} if data_file else {"dataFileId": None}
    except Exception as error:
        return {"error": str(error)[:120]}


def components_by_occurrence(design, all_occurrences, size):
    """Every component of the design, found through the occurrences the digest reads anyway: Fusion keeps
    a component only while it has an occurrence (the root aside). allComponents.item() costs about 5 ms a
    component (6 s of a 1,249-component digest); occurrence.component about 0.05 ms. If the counts ever
    disagree, the slow full list is used."""
    root = design.rootComponent
    found, seen = [root], {identity(root)}
    for occurrence in all_occurrences:
        comp = occurrence.component
        key = identity(comp)
        if key not in seen:
            seen.add(key)
            found.append(comp)
    if len(found) == size:
        return found
    collection = design.allComponents
    return [collection.item(index) for index in range(size)]


def revision(design):
    """Initialize all mesh collections, then capture the unchanged revision graph.

    Fusion can advance ancestor revision IDs on the first child meshBodies read.
    All components must be initialized before ANY component revisionId is sampled.
    Native IDs still detect true equal-volume/topology changes; no target is rebased.
    """
    require(threading.current_thread() is threading.main_thread(), "wrong_thread", "Mechanical revision requires the Fusion main thread.")
    from .assembly_state import component_state, occurrence_state
    from .readback import construction_state, feature_data
    from .configurations import snapshot as configuration_snapshot
    document, observations = None, []

    def observe(stage):
        state = _revision_document_observation(design, document)
        observations.append({"stage": stage, **state})
        return state

    def checked(state, before=None):
        require(state["complete"] and all(state[key] is True for key in ("designValid", "documentValid", "ownerVerified")),
                "revision_unavailable", "Exact Design/document ownership or flags are unavailable.")
        if before is not None:
            require(all(state[key] == before[key] for key in ("isSaved", "isModified")),
                    "revision_document_changed", "Document saved/modified flags changed during revision observation.")

    try:
        document = design.parentDocument
        before = observe("before_initialization")
        checked(before)
        collection = design.allComponents
        size = collection.count
        require(type(size) is int and 0 <= size <= MAX_ENTITIES, "inspection_budget", "Component collection exceeds inspection budget.")
        all_occurrences = items(design.rootComponent.allOccurrences)
        component_handles = components_by_occurrence(design, all_occurrences, size)
        mesh_collections = []  # Retain all native collection handles through the full digest.
        # A component inserted by reference belongs to its own document's Design: it is read-only
        # here, so it enters the revision by its source version instead of its native internals.
        # Requiring every component to belong to this Design refused every assembly built from
        # referenced sub-assemblies (2026-10-28: a production cell built from 23 referenced designs).
        referenced = set()
        for index, comp in enumerate(component_handles):
            require(comp is not None and comp.isValid is True, "revision_unavailable", "Component handle is invalid.")
            if not same(comp.parentDesign, design):
                # only a non-root component from another saved document is a reference
                source = getattr(comp.parentDesign, "parentDocument", None)
                require(not same(comp, design.rootComponent) and source is not None and getattr(source, "dataFile", None) is not None,
                        "revision_unavailable", "Component does not belong to the exact Design.")
                referenced.add(index)
                continue
            meshes = comp.meshBodies
            mesh_collections.append(meshes)  # Retain before count/validation can fail.
            require(meshes is not None and meshes.isValid is True and meshes.objectType == "adsk::fusion::MeshBodies",
                    "revision_unavailable", "Native mesh collection is invalid or has an unexpected type.")
            mesh_count = meshes.count
            require(type(mesh_count) is int and 0 <= mesh_count <= MAX_ENTITIES,
                    "inspection_budget", "Mesh collection exceeds inspection budget.")
        checked(observe("after_initialization"), before)
        components = []
        for index, comp in enumerate(component_handles):
            if index in referenced:
                components.append([identity(comp), "referenced", comp.name, reference_version(comp)])
                continue
            components.append([identity(comp), str(comp.revisionId), comp.name,
                               [[identity(b), str(b.revisionId)] for b in items(comp.bRepBodies)],
                               [[identity(s), str(s.revisionId)] for s in items(comp.sketches)],
                               component_state(comp), [feature_data(f, order=i) for i, f in enumerate(items(comp.features))] if hasattr(comp, "features") else [], construction_state(comp)])
        occurrences = [[identity(o), identity(o.component), list(o.transform2.asArray()),
                        occurrence_suppression(o), occurrence_state(o)] for o in all_occurrences]
        params = [[p.name, p.expression, p.unit] for p in items(design.allParameters)]
        result = digest([components, occurrences, params, design.timeline.count, design.timeline.markerPosition,
                         configuration_snapshot(design)])
        checked(observe("after_digest"), before)
        return result
    except Exception as error:
        observe("after_failure")
        initial = observations[0] if observations and observations[0]["stage"] == "before_initialization" else None
        changed = sorted({key for state in observations for key in ("isSaved", "isModified")
                          if initial is not None and type(initial.get(key)) is bool and type(state.get(key)) is bool
                          and state[key] != initial[key]})
        effect = "changed" if changed else "unchanged" if initial is not None and all(state["complete"] for state in observations) else "unknown"
        details = dict(getattr(error, "details", {}))
        details.update(revisionObservation={"documentFlagEffect": effect, "changedFlags": changed,
                                            "observations": observations}, rollbackClaimed=False)
        raise MechanicalError(getattr(error, "code", "revision_unavailable"), str(error)[:1024], **details) from error


def inside_reference(occurrence):
    """Is this occurrence inside a design inserted by reference (it or an ancestor is the reference)?"""
    node = occurrence
    while node is not None:
        if getattr(node, "isReferencedComponent", False):
            return True
        node = getattr(node, "assemblyContext", None)
    return False


def occurrence_suppression(occurrence):
    """Suppressed in this design's timeline; None when it has no timeline object here. An occurrence
    inside a referenced design has none in this document (Fusion raises InternalValidationError, seen
    on a production cell): its suppression is decided in its source document."""
    try:
        timeline = occurrence.timelineObject
    except RuntimeError:
        if inside_reference(occurrence):
            return None
        element = pattern_element(occurrence)
        if element is not None:
            return bool(element.isSuppressed)
        raise
    return bool(timeline.isSuppressed) if timeline is not None else None


def pattern_element(occurrence):
    """The pattern element that made an occurrence, or None. A component pattern's copies have no
    timeline object of their own (Fusion raises InternalValidationError); the element carries their
    suppression. Looked up only on that error, in the owning component's patterns."""
    try:
        context = getattr(occurrence, "assemblyContext", None)
        owner = context.component if context is not None else occurrence.component.parentDesign.rootComponent
        path = occurrence.fullPathName
        groups = (owner.features.rectangularPatternFeatures, owner.features.circularPatternFeatures)
    except (AttributeError, RuntimeError):
        return None
    for group in groups:
        for i in range(group.count):
            elements = group.item(i).patternElements
            for j in range(elements.count):
                element = elements.item(j)
                if any(o.fullPathName == path for o in element.occurrences):
                    return element
    return None


def target(app, args, write=False, model_write=True):
    doc, design, key = active(app)
    wanted = args.get("target")
    require(isinstance(wanted, dict), "target_required", "Pass target from mechanical inspect.")
    require(wanted.get("documentId") == key, "wrong_document", "Target is not the active document.", activeDocumentId=key)
    if write:
        from .configurations import snapshot as configuration_snapshot
        configuration = configuration_snapshot(design, strict=True)
        if model_write:
            require(not configuration["configured"], "configuration_mutation_unverified",
                    "Configured-design model writes remain disabled until native configuration recovery and archive gates pass; inspection and current-row BOM/export remain available.")
    rev = revision(design)
    if write or "revision" in wanted:
        require(wanted.get("revision") == rev, "stale_revision", "The model changed; inspect it before a new operation.", currentRevision=rev)
    return doc, design, {"documentId": key, "revision": rev}


def read_target(app, args):
    """target() for a read that only needs the guard: the target's document must be the active one, and
    a revision, when the target carries one, must be current. The revision digest costs about 5 s on a
    4,579-occurrence assembly, so a read not asked to check one does not compute it, and its target then
    carries no revision."""
    _, design, key = active(app)
    wanted = args.get("target")
    require(isinstance(wanted, dict), "target_required", "Pass target from mechanical inspect.")
    require(wanted.get("documentId") == key, "wrong_document", "Target is not the active document.", activeDocumentId=key)
    if "revision" in wanted:
        return target(app, args)
    return app.activeDocument, design, {"documentId": key}


def current_target(app):
    doc, design, key = active(app)
    return {"documentId": key, "revision": revision(design)}


ORIGIN_PLANES = {"xy": "xYConstructionPlane", "xz": "xZConstructionPlane", "yz": "yZConstructionPlane"}


def origin_plane_key(entity):
    """'xy', 'xz' or 'yz' when entity is one of its component's three origin planes, else None."""
    if getattr(entity, "objectType", "") != "adsk::fusion::ConstructionPlane":
        return None
    comp = getattr(entity, "component", None) or getattr(entity, "parent", None)
    for key, attr in ORIGIN_PLANES.items():
        if comp is not None and same(getattr(comp, attr, None), entity):
            return key
    return None


def ref(entity, target_value, occurrence=None):
    entity = concrete(entity)
    if entity.objectType == "adsk::fusion::Profile":
        from .readback import profile_ref
        sketch = entity.parentSketch
        matches = [index for index, profile in enumerate(items(sketch.profiles)) if same(profile, entity)]
        require(len(matches) == 1, "ambiguous_reference", "Profile must belong uniquely to its sketch.")
        return profile_ref(sketch, entity, matches[0], target_value)
    is_occurrence = entity.objectType.endswith("::Occurrence")
    native = entity if is_occurrence else (getattr(entity, "nativeObject", None) or entity)
    context = None if is_occurrence else (occurrence or getattr(entity, "assemblyContext", None))
    result = {"documentId": target_value["documentId"], "revision": target_value["revision"],
            "token": entity_token(native), "objectType": native.objectType,
            "occurrenceToken": entity_token(context) if context else None,
            "occurrencePath": context.fullPathName if context else (entity.fullPathName if is_occurrence else None)}
    if native.objectType == "adsk::fusion::Component" and same(native, native.parentDesign.rootComponent):
        # Fusion can emit a root token that findEntityByToken cannot resolve.
        # Bind this explicit root reference to its live API identity instead.
        result["rootComponentId"] = identity(native)
    plane = origin_plane_key(native)
    if plane:
        # an origin plane resolves through its component, not its token, so a base plane is a
        # reference like any other plane (construction_axis two_planes, sketch_create, a split tool)
        result["originPlane"] = plane
        result["component"] = ref(native.component, target_value)
    return result


def occurrence_at(design, path):
    """The root-context occurrence at a full path, found level by level with a few reads instead of by
    reading every occurrence's path (about 0.3 s per lookup on a 4,579-occurrence assembly). A name can
    itself hold '+', so each level takes the shortest name that exists. None when nothing is there."""
    level, parts = design.rootComponent.occurrences, path.split("+") if path else []
    try:
        while parts:
            for size in range(1, len(parts) + 1):
                occurrence = level.itemByName("+".join(parts[:size]))
                if occurrence is not None:
                    break
            else:
                return None
            parts = parts[size:]
            if not parts:
                return occurrence
            level = occurrence.childOccurrences
    except (AttributeError, RuntimeError):
        return None  # callers fall back to reading every occurrence
    return None


def in_root_context(design, occurrence):
    """Whether an occurrence is one of the root context's (rootComponent.allOccurrences), by its path
    first; only a miss reads them all."""
    found = occurrence_at(design, occurrence.fullPathName)
    if found is not None and same(found, occurrence):
        return True
    return any(same(occurrence, item) for item in items(design.rootComponent.allOccurrences))


def occurrence_context(design, entity, path):
    entity = concrete(entity)
    require(entity.objectType.endswith("::Occurrence"), "invalid_context", "Context must resolve to an occurrence.")
    native = getattr(entity, "nativeObject", None) or entity
    found = occurrence_at(design, path)
    if found is not None and found.fullPathName == path and same(getattr(found, "nativeObject", None) or found, native):
        return found
    candidates = [o for o in items(design.rootComponent.allOccurrences)
                  if o.fullPathName == path and same(getattr(o, "nativeObject", None) or o, native)]
    if candidates:
        require(len(candidates) == 1, "ambiguous_context", "Occurrence path must identify one instance.")
        return candidates[0]
    # Native definition references have a path relative to their owner. Only
    # retain that context when the token and saved path both resolve exactly.
    require(entity.fullPathName == path, "invalid_context", "Occurrence path changed.")
    return entity


def resolve(design, target_value, value, allowed=()):
    require(isinstance(value, dict), "invalid_reference", "An entity reference object is required.")
    require(value.get("documentId") == target_value["documentId"] and value.get("revision") == target_value["revision"],
            "stale_reference", "Reference is from another document or revision.")
    if value.get("objectType") == "adsk::fusion::Profile":
        # Bind profile selection to a uniquely
        # resolved sketch, this exact aggregate revision and geometric witness.
        from .readback import profile_signature
        require(not allowed or "Profile" in allowed, "wrong_entity_type", "Operation does not accept profiles.")
        require(isinstance(value.get("sketchToken"), str), "invalid_reference", "Profile sketch token is required.")
        sketches = design.findEntityByToken(value["sketchToken"])
        require(len(sketches) == 1 and sketches[0].objectType.endswith("::Sketch"), "ambiguous_reference", "Profile sketch must resolve uniquely.")
        index = value.get("profileIndex")
        require(isinstance(index, int) and not isinstance(index, bool) and 0 <= index < sketches[0].profiles.count,
                "invalid_reference", "Profile index is no longer valid.")
        profile = sketches[0].profiles.item(index)
        require(digest(profile_signature(profile)) == value.get("fingerprint"), "stale_reference", "Profile geometry changed.")
        return profile
    if value.get("originPlane") is not None:
        require(value.get("objectType") == "adsk::fusion::ConstructionPlane" and value["originPlane"] in ORIGIN_PLANES,
                "invalid_reference", "An origin plane reference names xy, xz or yz.")
        require(not allowed or "ConstructionPlane" in allowed, "wrong_entity_type", "Operation does not accept this entity type.")
        comp = resolve(design, target_value, value.get("component"), ("Component",))
        entity = getattr(comp, ORIGIN_PLANES[value["originPlane"]])
        require(entity is not None and entity.isValid, "invalid_reference", "Origin plane is not available.")
        return entity
    if "rootComponentId" in value:
        entity = design.rootComponent
        require(value.get("objectType") == "adsk::fusion::Component"
                and value["rootComponentId"] == identity(entity)
                and not value.get("occurrenceToken") and not value.get("occurrencePath"),
                "invalid_reference", "Root reference does not identify this active design's root component.")
    else:
        require(isinstance(value.get("token"), str) and value["token"], "invalid_reference", "Reference token is missing.")
        matches = design.findEntityByToken(value["token"])
        require(len(matches) <= MAX_ENTITIES, "inspection_budget", "Token lookup exceeds inspection budget.")
        if len(matches) > 1 and value.get("objectType") == "adsk::fusion::Component":
            # Repeated definitions can return duplicate API wrappers. Only
            # actual Component equality makes them one entity; face splits and
            # occurrence instances retain the strict one-result rule below.
            first = matches[0]
            for candidate in matches:
                require(candidate.isValid and candidate.objectType == "adsk::fusion::Component"
                        and same(first, candidate), "ambiguous_reference",
                        "Component token resolves to distinct or invalid entities.", matchCount=len(matches))
            matches = [first]
        require(len(matches) == 1, "ambiguous_reference", "Token must resolve to exactly one live entity.", matchCount=len(matches))
        entity = concrete(matches[0])
    require(entity.isValid and entity.objectType == value.get("objectType"), "invalid_reference", "Entity type or validity changed.")
    require(not allowed or entity.objectType.split("::")[-1] in allowed, "wrong_entity_type", "Operation does not accept this entity type.")
    if entity.objectType.endswith("::Occurrence") and value.get("occurrencePath"):
        entity = occurrence_context(design, entity, value["occurrencePath"])
    if value.get("occurrenceToken"):
        contexts = design.findEntityByToken(value["occurrenceToken"])
        require(len(contexts) == 1, "ambiguous_context", "Occurrence context must resolve uniquely.")
        context = occurrence_context(design, contexts[0], value.get("occurrencePath"))
        owner = getattr(entity, "parentComponent", None)
        if owner is None and hasattr(entity, "component"):
            owner = entity.component
        if owner is None and hasattr(entity, "parentSketch"):
            owner = entity.parentSketch.parentComponent
        if owner is None and hasattr(entity, "body"):
            owner = entity.body.parentComponent
        require(owner and same(owner, context.component), "invalid_context", "Entity is not owned by this occurrence component.")
        entity = entity.createForAssemblyContext(context)
        require(entity and entity.isValid, "invalid_context", "Could not create occurrence-context proxy.")
    return entity


def health(design, recompute=False, natives=None):
    """Every timeline item's health. natives, when given, collects (row, timeline item) for each fault and
    unreadable item, so a write can tell which of them it edited."""
    computed = None
    if recompute:
        computed = bool(design.computeAll())
    faults, unknown, suppressed = [], [], []
    pending = [(str(index), item) for index, item in enumerate(items(design.timeline))]
    while pending:
        index, item = pending.pop(0)
        try:
            if getattr(item, "isGroup", False):
                group = adsk.fusion.TimelineGroup.cast(item)
                pending.extend((index + "." + str(i), child) for i, child in enumerate(items(group)))
                continue
            if getattr(item, "isSuppressed", False):
                suppressed.append({"index": index, "name": item.name})
                continue
            state = item.healthState
            if state != adsk.fusion.FeatureHealthStates.HealthyFeatureHealthState:
                faults.append({"index": index, "name": item.name, "state": int(state),
                               "message": item.errorOrWarningMessage})
                if natives is not None:
                    natives.append((faults[-1], item))
        except Exception as exc:
            row = {"index": index, "error": str(exc)}
            try:
                row["name"] = item.name
            except Exception:
                pass
            unknown.append(row)
            if natives is not None:
                natives.append((row, item))
    return {"healthy": computed is not False and not faults and not unknown,
            "computeSucceeded": computed, "complete": not unknown, "faults": faults,
            "unknown": unknown, "suppressed": suppressed, "timelineCount": design.timeline.count}


def require_healthy(design, recompute=True):
    state = health(design, recompute)
    require(state["healthy"], "unhealthy_model", "Recompute or feature health check failed.", health=state)
    return state


def point_mm(point):
    return [point.x * 10, point.y * 10, point.z * 10]


def body_data(body):
    pp = body.physicalProperties
    box = body.boundingBox
    return {"name": body.name, "volume": pp.volume * 1000, "area": pp.area * 100,
            "mass": pp.mass, "centerOfMass": point_mm(pp.centerOfMass),
            "bounds": {"min": point_mm(box.minPoint), "max": point_mm(box.maxPoint)},
            "faces": body.faces.count, "edges": body.edges.count, "isSolid": body.isSolid,
            "space": "component", "units": UNITS}


def parameter_data(design, parameter):
    unit = parameter.unit
    # Fusion numeric values always use its internal units (cm/rad/kg), regardless
    # of the display unit. Conversion occurs here, never in individual handlers.
    value, normalized = None, unit
    for canonical in ("mm", "deg", "kg", ""):
        try:
            if design.unitsManager.isValidExpression("1 " + canonical, unit):
                value = design.unitsManager.convert(parameter.value, "internalUnits", canonical)
                normalized = canonical or "unitless"
                break
        except Exception:
            continue
    row = {"name": parameter.name, "expression": parameter.expression,
           "value": value, "unit": normalized, "displayUnit": unit}
    # which feature (or sketch dimension, or joint) owns a model parameter, so feature_edit can be
    # pointed at it without diffing parameter lists; a user parameter has no owner
    owner = None
    try:
        created_by = getattr(parameter, "createdBy", None)
        if created_by is not None:
            owner = {"name": getattr(created_by, "name", None), "objectType": created_by.objectType}
            role = getattr(parameter, "role", None)
            if role:
                owner["role"] = role
    except Exception:
        owner = None
    row["kind"] = "model" if parameter.objectType.endswith("::ModelParameter") else "user"
    row["owner"] = owner
    return row


def recovery_snapshot(design, health_state=None, strict_sketches=()):
    """Semantic recovery witness; native revision IDs legitimately change on undo. health_state is the
    health just read on this same unchanged model, when the caller has it (each walk reads every timeline
    item, about 23 ms apiece on a big assembly). strict_sketches are the sketches the write edits or reads:
    their constraint graph must read completely, or the write refuses before it changes anything. Every
    other sketch enters with what can be read, and a relationship the reader does not support enters as
    its type, so an unsupported relationship elsewhere in the design does not block the write."""
    from .readback import construction_state, feature_data, sketch_data, topology_witness
    from .assembly_state import component_state, occurrence_state
    from .configurations import snapshot as configuration_snapshot
    configuration = configuration_snapshot(design, strict=True)
    all_occurrences = items(design.rootComponent.allOccurrences)
    return {"configuration": configuration,
            "parameters": [[p.name, p.expression, p.unit] for p in items(design.allParameters)],
            "components": [[identity(c), c.name, [topology_witness(b) for b in items(c.bRepBodies)],
                            [sketch_data(s, strict=any(same(s, t) for t in strict_sketches)) for s in items(c.sketches)],
                            [feature_data(f, order=i) for i, f in enumerate(items(c.features))], component_state(c), construction_state(c)]
                           for c in components_by_occurrence(design, all_occurrences, design.allComponents.count)],
            "occurrences": [[identity(o), list(o.transform2.asArray()), occurrence_state(o)] for o in all_occurrences],
            "timeline": design.timeline.count, "timelineMarker": design.timeline.markerPosition,
            "health": health_state if health_state is not None else health(design)}


def number(value, name, positive=False):
    require(isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value), "invalid_input", name + " must be a finite number.")
    require(not positive or value > 0, "invalid_input", name + " must be positive.")
    return float(value)
