"""UNREGISTERED prototype: exact-owned, finite CustomGraphics presentation copies.

No document creation/close, source setters, timers, capture, durable replay or public
operation registration. The native qualification harness supplies an owned blank
presentation lease. All native access, including failed-resource cleanup, stays on
the main thread. Retain the Preview object if create/sample/dispose raises.
"""
import copy
import json
import math
import threading
import uuid

import adsk.core

from . import core
from .core import (SESSION_ID, concrete, digest, document_identity, require,
                   resolve, revision, same, target)
from .exploded_view import IDENTITY, exploded_view_plan

PROTOTYPE_ONLY = True
MAX_INSTANCES = 64
MAX_BODIES = 128
MAX_TOPOLOGY = 4096
MAX_VERTICES = 16384
_PREPARED = object()


def main_thread():
    require(threading.current_thread() is threading.main_thread(), 'wrong_thread',
            'Preview native access requires the main thread.')


def count(collection, limit=MAX_TOPOLOGY):
    count = collection.count
    require(type(count) is int and 0 <= count <= limit, 'preview_budget', 'Native collection exceeds the preview bound.')
    return count


def bounded(collection, limit=MAX_TOPOLOGY):
    return [concrete(collection.item(i)) for i in range(count(collection, limit))]


def finite(value):
    require(type(value) in (int, float) and math.isfinite(value) and abs(value) <= 1e12,
            'preview_geometry_invalid', 'Native geometry contains a nonfinite or excessive coordinate.')
    return value


def point(value):
    return [finite(value.x), finite(value.y), finite(value.z)]


def body_witness(body, transient):
    require(body is not None and body.isValid and body.objectType == 'adsk::fusion::BRepBody'
            and body.isTransient is transient and body.isSolid is True,
            'preview_geometry_unsupported', 'Preview requires valid solid BRep bodies with the expected native/copy provenance.')
    box = body.boundingBox
    low, high = point(box.minPoint), point(box.maxPoint)
    require(all(a <= b for a, b in zip(low, high)), 'preview_geometry_invalid', 'Invalid native body bounds.')
    volume = finite(body.volume)
    require(volume > 0, 'preview_geometry_unsupported', 'Solid BRep volume must be positive.')
    return {'volumeCm3': volume, 'boundsCm': [low, high],
            'faceCount': count(body.faces), 'edgeCount': count(body.edges),
            'verticesCm': [point(v.geometry) for v in bounded(body.vertices)]}


def equivalent(left, right):
    """Only geometry floats use tolerance; identity/counts/booleans remain exact."""
    if type(left) in (int, float) and type(right) in (int, float):
        return math.isclose(left, right, rel_tol=1e-10, abs_tol=1e-10)
    if type(left) is not type(right):
        return False
    if isinstance(left, dict):
        return left.keys() == right.keys() and all(equivalent(left[k], right[k]) for k in left)
    if isinstance(left, list):
        return len(left) == len(right) and all(equivalent(a, b) for a, b in zip(left, right))
    return left == right


def intended_native_values(values):
    require(isinstance(values, list) and len(values) == 16 and all(type(v) in (int, float) and math.isfinite(v) for v in values),
            'preview_plan_invalid', 'Stored sample matrix is invalid.')
    values = list(values)
    for index in (3, 7, 11):
        values[index] /= 10.  # Public root/world mm -> native model cm, exactly once.
    return values


def native_matrix(values):
    intended = intended_native_values(values)
    result = adsk.core.Matrix3D.create()
    require(result is not None and result.setWithArray(list(intended)) is True,
            'preview_transform_refused', 'Native matrix construction failed.')
    require(equivalent(list(result.asArray()), intended),
            'preview_transform_refused', 'Native matrix construction altered the intended stored pose.')
    return result


def matrix_values(entity):
    result = list(entity.transform.asArray())
    require(len(result) == 16 and all(type(v) in (int, float) and math.isfinite(v) for v in result),
            'preview_transform_refused', 'Native graphics transform is unavailable.')
    return result


def source_geometry(design, value, plan):
    """Proxy visibility first, native definition geometry second; flat instances."""
    # a plan of a big design lists only the moved occurrences and their parents: copying those alone
    # would drop every other part from the presentation
    require(plan.get('instanceScope') == 'all',
            'preview_budget', 'Preview copies every instance and supports at most 64; this design has %s occurrences. '
            'Use exploded_view_render or exploded_view_stills for a big assembly.' % plan.get('designInstances'))
    require(len(plan['instances']) <= MAX_INSTANCES, 'preview_budget', 'Preview supports at most 64 physical instances.')
    rows = [(None, design.rootComponent, None)]
    for row in plan['instances']:
        occurrence = resolve(design, value, row['occurrence'], ('Occurrence',))
        component = resolve(design, value, row['component'], ('Component',))
        require(same(occurrence.component, component) and occurrence.fullPathName == row['path'],
                'preview_source_changed', 'Instance/component attachment changed.')
        require(occurrence.isVisible is True, 'preview_visibility_unsupported', 'Hidden or effectively hidden occurrences are unsupported.')
        timeline = occurrence.timelineObject
        require(timeline is None or timeline.isSuppressed is False,
                'preview_visibility_unsupported', 'Suppressed occurrences are unsupported.')
        rows.append((row['instanceId'], component, occurrence))
    output, body_count, vertices = [], 0, 0
    for key, component, occurrence in rows:
        require(component is not None and component.isValid and same(component.parentDesign, design),
                'preview_source_changed', 'Component is not owned by the source Design.')
        require(count(component.meshBodies, MAX_BODIES) == 0,
                'preview_geometry_unsupported', 'Mesh bodies are unsupported; no partial geometry omission is permitted.')
        bodies, witnesses = [], []
        for native in bounded(component.bRepBodies, MAX_BODIES):
            body_count += 1
            require(body_count <= MAX_BODIES, 'preview_budget', 'Preview exceeds 128 body copies.')
            require(native.nativeObject is None and same(native.parentComponent, component),
                    'preview_geometry_unsupported', 'Copy inputs must be definition-space native bodies.')
            visible = concrete(native.createForAssemblyContext(occurrence)) if occurrence is not None else native
            require(visible is not None and visible.isValid and visible.isVisible is True,
                    'preview_visibility_unsupported', 'Hidden or effectively hidden bodies are unsupported.')
            if occurrence is not None:
                require(same(visible.nativeObject, native) and same(visible.assemblyContext, occurrence),
                        'preview_source_changed', 'Body proxy does not belong to the exact instance.')
            witness = body_witness(native, False)
            vertices += len(witness['verticesCm'])
            require(vertices <= MAX_VERTICES, 'preview_budget', 'Preview exceeds 16384 copied vertices.')
            bodies.append(native); witnesses.append(witness)
        output.append({'instanceId': key, 'bodies': bodies, 'witnesses': witnesses})
    require(body_count > 0, 'preview_geometry_unsupported', 'Source has no supported solid bodies.')
    return output


class Prepared:
    """Server-only preparation; callers receive identifiers, never writable poses."""
    def __init__(self, app, args, expected_plan_id):
        main_thread()
        args = copy.deepcopy(args)
        result = exploded_view_plan(app, args)
        require(result['planId'] == expected_plan_id, 'preview_plan_mismatch', 'Regenerated plan differs from the expected plan.')
        self.app = app
        self.document, self.design, value = target(app, args)
        self._result_json = json.dumps(result, sort_keys=True, allow_nan=False)
        self._target = copy.deepcopy(value)
        self._flags = (self.document.isSaved, self.document.isModified)
        require(all(type(v) is bool for v in self._flags), 'preview_source_changed', 'Source document flags are unavailable.')
        self._geometry = source_geometry(self.design, value, result['plan'])
        self._witness = [r['witnesses'] for r in self._geometry]
        self._seal = _PREPARED
        self.check()

    @property
    def plan_id(self):
        return json.loads(self._result_json)['planId']

    def check(self):
        main_thread()
        result = json.loads(self._result_json)
        require(result['plan']['sessionId'] == SESSION_ID, 'session_mismatch', 'Preview belongs to an earlier add-in session.')
        require(self.document.isValid and self.design.isValid and same(self.design.parentDocument, self.document)
                and sum(same(d, self.document) for d in bounded(self.app.documents, 128)) == 1,
                'preview_source_changed', 'Exact source document is no longer open.')
        require(document_identity(self.document) == self._target['documentId']
                and revision(self.design) == self._target['revision']
                and all(type(v) is bool for v in (self.document.isSaved, self.document.isModified))
                and (self.document.isSaved, self.document.isModified) == self._flags,
                'preview_source_changed', 'Source identity, revision or flags changed.')
        current = source_geometry(self.design, self._target, result['plan'])
        require([r['witnesses'] for r in current] == self._witness,
                'preview_source_changed', 'Source geometry/visibility changed.')
        # Refresh handles through the guarded refs, never use a stale body wrapper.
        self._geometry = current


def prepare(app, planner_args, expected_plan_id):
    return Prepared(app, planner_args, expected_plan_id)


def all_attributes(entity):
    rows = []
    for attribute in bounded(entity.attributes, 256):
        row = [attribute.groupName, attribute.name, attribute.value]
        require(all(isinstance(v, str) and len(v) <= 4096 for v in row),
                'preview_budget', 'Presentation attribute is unbounded.')
        rows.append(row)
    return sorted(rows)


def origin_witness(root):
    """The seven built-in origin entities are outside construction collections."""
    rows = []
    for field, kind in (('xConstructionAxis', 'ConstructionAxis'), ('yConstructionAxis', 'ConstructionAxis'),
                        ('zConstructionAxis', 'ConstructionAxis'), ('xYConstructionPlane', 'ConstructionPlane'),
                        ('xZConstructionPlane', 'ConstructionPlane'), ('yZConstructionPlane', 'ConstructionPlane'),
                        ('originConstructionPoint', 'ConstructionPoint')):
        entity = concrete(getattr(root, field))
        require(entity is not None and entity.isValid is True and entity.objectType == 'adsk::fusion::' + kind
                and same(entity.component, root) and entity.nativeObject is None and entity.isDeletable is False,
                'preview_not_blank', 'Exact built-in presentation origin is unavailable.')
        visibility = [entity.isLightBulbOn, entity.isVisible]
        require(all(type(value) is bool for value in visibility), 'preview_not_blank', 'Origin visibility is unavailable.')
        geometry = entity.geometry
        values = point(geometry) if kind == 'ConstructionPoint' else [point(geometry.origin),
                 point(geometry.direction if kind == 'ConstructionAxis' else geometry.normal)]
        rows.append([field, core.identity(entity), entity.name, visibility, values, all_attributes(entity)])
    return rows


def presentation_observation(document, design):
    main_thread()
    flags = None
    try:
        flags = [document.isSaved, document.isModified]
        return _presentation_observation(document, design, flags)
    except Exception as error:
        observations = {'beforeFlags': flags, 'afterFlags': None, 'flagReadError': None}
        try:
            observations['afterFlags'] = [document.isSaved, document.isModified]
        except Exception as flag_error:
            observations['flagReadError'] = str(flag_error)[:512]
        valid = all(isinstance(value, list) and len(value) == 2 and all(type(v) is bool for v in value)
                    for value in (flags, observations['afterFlags']))
        observations['flagEffect'] = ('unchanged' if flags == observations['afterFlags'] else 'changed') if valid else 'unknown'
        details = dict(getattr(error, 'details', {}))
        details.update(presentationObservation=observations, rollbackClaimed=False)
        raise core.MechanicalError(getattr(error, 'code', 'preview_revision_unavailable'), str(error)[:1024], **details) from error


def _presentation_observation(document, design, flags):
    """Complete bounded empty-model payload; only the root's opaque revision varies.

    Rebuild the exact current core aggregate for this one-component empty case,
    and prove its digest equals the unmodified core revision before and after.
    No global monkeypatch, target refresh, or source revision exemption.
    """
    from .assembly_state import component_state
    from .readback import construction_state
    from .configurations import snapshot
    require(document.isValid is True and design.isValid is True and same(design.parentDocument, document),
            'preview_ownership', 'Exact presentation document/Design is unavailable.')
    require(all(type(v) is bool for v in flags) and flags[0] is False,
            'preview_not_blank', 'Presentation flags must be known and unsaved.')
    before = revision(design)
    root = design.rootComponent
    require(root is not None and root.isValid is True and root.objectType == 'adsk::fusion::Component'
            and same(root.parentDesign, design), 'preview_ownership', 'Exact presentation root is unavailable.')
    components = bounded(design.allComponents, 1)
    require(len(components) == 1 and same(components[0], root), 'preview_not_blank', 'Presentation must have exactly its root.')
    empty_fields = ('bRepBodies', 'meshBodies', 'occurrences', 'allOccurrences', 'sketches', 'features',
                    'joints', 'asBuiltJoints', 'jointOrigins', 'rigidGroups', 'motionLinks',
                    'constructionAxes', 'constructionPlanes', 'constructionPoints', 'modelParameters',
                    'canvases', 'decals')
    for name in empty_fields:
        require(count(getattr(root, name), 0) == 0, 'preview_not_blank', 'Presentation contains unsupported ' + name + '.')
    require(count(design.allParameters, 0) == 0 and count(design.timeline, 0) == 0
            and type(design.timeline.markerPosition) is int and design.timeline.markerPosition == 0
            and count(design.snapshots, 0) == 0 and design.snapshots.hasPendingSnapshot is False,
            'preview_not_blank', 'Presentation contains model parameters, history or positions.')
    configuration = snapshot(design, strict=True)
    require(configuration['complete'] is True and configuration['configured'] is False
            and configuration['isConfiguration'] is False, 'preview_not_blank', 'Configured presentations are unsupported.')
    attributes = {name: all_attributes(entity) for name, entity in (('document', document), ('design', design), ('root', root))}
    require(all(isinstance(value, str) and len(value) <= 4096 for value in (root.name, root.partNumber, root.description)),
            'preview_not_blank', 'Presentation metadata is unavailable or unbounded.')
    folders = {}
    for name in ('isBodiesFolderLightBulbOn', 'isConstructionFolderLightBulbOn', 'isOriginFolderLightBulbOn',
                 'isSketchFolderLightBulbOn', 'isJointsFolderLightBulbOn', 'isJointOriginsFolderLightBulbOn',
                 'isCanvasFolderLightBulbOn', 'isDecalFolderLightBulbOn'):
        value = getattr(root, name)
        require(type(value) is bool, 'preview_not_blank', 'Presentation visibility is unavailable.')
        folders[name] = value
    native_revision = root.revisionId
    require(isinstance(native_revision, str) and 0 < len(native_revision) <= 256,
            'preview_not_blank', 'Native presentation revision is unavailable.')
    payload = [[[core.identity(root), native_revision, root.name, [], [], component_state(root), [], construction_state(root)]],
               [], [], design.timeline.count, design.timeline.markerPosition, configuration]
    require(digest(payload) == before, 'preview_revision_unavailable', 'Empty presentation payload does not match the actual aggregate.')
    stable_payload = copy.deepcopy(payload)
    stable_payload[0][0][1] = None  # The sole classified, opaque native revision slot; never a missing-data sentinel.
    witness = {'aggregate': stable_payload, 'attributes': attributes, 'folders': folders, 'origin': origin_witness(root),
               'documentName': document.name, 'creationId': document.creationId,
               'designType': int(design.designType), 'designIntent': int(design.designIntent)}
    wire = json.dumps(witness, sort_keys=True, allow_nan=False)
    require(len(wire.encode('utf-8')) <= 65536, 'preview_budget', 'Presentation witness exceeds its bound.')
    after = revision(design)
    final_flags = [document.isSaved, document.isModified]
    require(all(type(v) is bool for v in final_flags) and final_flags == flags and after == before,
            'preview_revision_unavailable', 'Presentation observation changed or could not establish a stable revision/flags.')
    return {'revision': after, 'nativeRevision': native_revision, 'witnessDigest': digest(witness), 'flags': flags,
            'rootId': core.identity(root)}


class OwnedPresentationLease:
    """Exact native doc + independent ownership receipt; no name-based selection.

    verify_owner() must return success:true, ownerId, sessionId, documentId after
    checking the existing native run registry. This prototype never creates,
    registers, archives or closes a document; the qualification harness owns that.
    """
    def __init__(self, app, document, design, owner_id, verify_owner, expected_state=None):
        main_thread()
        require(isinstance(owner_id, str) and 1 <= len(owner_id) <= 128 and callable(verify_owner),
                'preview_ownership', 'A bounded owner ID and exact ownership verifier are required.')
        self.app, self.document, self.design = app, document, design
        self.root_component = design.rootComponent
        self.owner_id, self.verify_owner = owner_id, verify_owner
        self.document_id = document_identity(document)
        self.session_id = SESSION_ID
        self.sequence = 0
        self._effect = None
        self.cleanup_state = None
        self.failure_observation = None
        observed = self.observe()
        require(expected_state is None or observed == expected_state,
                'preview_not_blank', 'Presentation changed after reservation.')
        self._accept(observed)

    def observe(self):
        main_thread()
        require(self.session_id == SESSION_ID, 'session_mismatch', 'Presentation lease belongs to another session.')
        receipt = self.verify_owner()
        require(isinstance(receipt, dict) and set(receipt) == {'success', 'ownerId', 'sessionId', 'documentId'}
                and receipt.get('success') is True and receipt.get('ownerId') == self.owner_id
                and receipt.get('sessionId') == SESSION_ID and receipt.get('documentId') == self.document_id,
                'preview_ownership', 'Independent exact-document ownership was not verified.')
        doc, design = self.document, self.design
        require(doc.isValid is True and design.isValid is True and same(design.parentDocument, doc)
                and document_identity(doc) == self.document_id and same(design.rootComponent, self.root_component)
                and sum(same(d, doc) for d in bounded(self.app.documents, 128)) == 1,
                'preview_ownership', 'Owned presentation document/root is unavailable or differs.')
        return presentation_observation(doc, design)

    def _accept(self, observed):
        self.verified_state = copy.deepcopy(observed)
        self.model_revision = observed['revision']

    def check(self, cleanup=False):
        main_thread()
        observed = self.observe()
        expected = self.cleanup_state if cleanup and self.cleanup_state is not None else self.verified_state
        require(observed == expected, 'preview_not_blank', 'Presentation revision or non-graphics model changed.')
        return observed

    def begin_effect(self, cleanup=False):
        observed = self.check(cleanup=cleanup)
        require(self._effect is None, 'preview_state', 'A presentation effect is already in flight.')
        self._effect = copy.deepcopy(observed)
        return self._effect

    def _after_effect(self, token):
        require(token is self._effect and token is not None, 'preview_state', 'Effect does not belong to the current lease.')
        observed = self.observe()
        # Full payload digest is already proven by observe(). Only the opaque
        # root Component revision slot may differ; flags and every witness field stay exact.
        require(all(observed[key] == token[key] for key in ('witnessDigest', 'flags', 'rootId')),
                'preview_not_blank', 'Owned graphics changed non-graphics presentation state.',
                presentationBefore=token, presentationAfter=observed, rollbackClaimed=False)
        return observed

    def finish_effect(self, token):
        observed = self._after_effect(token)
        self._accept(observed)
        self.sequence += 1
        self.cleanup_state = None
        self._effect = None

    def failed_effect(self, token, ownership):
        if token is None or self._effect is not token:
            return  # No effect, or the graphics boundary already completed.
        self.cleanup_state = None
        try:
            ownership()
            self.cleanup_state = self._after_effect(token)
            self.failure_observation = {'cleanupOnly': True, 'observedRevision': self.cleanup_state['revision']}
        except Exception as error:
            self.failure_observation = {'cleanupOnly': False, 'error': str(error)[:512],
                'presentationObservation': getattr(error, 'details', {}).get('presentationObservation'),
                'observedState': getattr(error, 'details', {}).get('presentationAfter')}
        finally:
            self._effect = None


class Preview:
    """Retain this object on failure: .state/.root identify partial owned resources."""
    def __init__(self, prepared, lease):
        main_thread()
        require(isinstance(prepared, Prepared) and prepared._seal is _PREPARED and isinstance(lease, OwnedPresentationLease),
                'preview_ownership', 'Use server preparation and an exact-owned presentation lease.')
        require(same(prepared.app, lease.app) and not same(prepared.document, lease.document),
                'preview_ownership', 'Source and presentation must be different documents in the same application.')
        self.prepared, self.lease = prepared, lease
        self.preview_id = str(uuid.uuid4())
        self.root = None
        self.nodes = []
        self.state = 'prepared'
        self.sample_index = None
        self._root_id_written = False
        self._delete_issued = False
        self._sample_json = json.dumps(json.loads(prepared._result_json)['samples'], sort_keys=True, allow_nan=False)
        self.samples_digest = digest(json.loads(self._sample_json))

    def _guard(self):
        main_thread()
        self.lease.check()
        self.prepared.check()

    def _ownership(self):
        require(self.root is not None and self.root.isValid is True
                and self.root.objectType == 'adsk::fusion::CustomGraphicsGroup'
                and (not self._root_id_written or self.root.id == 'AdomExplodedPreview:' + self.preview_id),
                'preview_ownership', 'Exact retained graphics root is unavailable or changed.')
        require(same(self.lease.design.rootComponent, self.lease.root_component),
                'preview_ownership', 'Exact retained presentation root Component changed.')
        roots = bounded(self.lease.root_component.customGraphicsGroups, 1)
        require(len(roots) == 1 and same(roots[0], self.root), 'preview_ownership', 'Presentation graphics inventory changed.')
        children = bounded(self.root, MAX_INSTANCES + 1)
        require(len(children) == len(self.nodes), 'preview_ownership', 'Owned child inventory changed.')
        for actual, node in zip(children, self.nodes):
            group = node['group']
            require(same(actual, group) and group.isValid and same(group.parent, self.root),
                    'preview_ownership', 'Exact owned graphics subgroup changed.')
            bodies = bounded(group, MAX_BODIES)
            require(len(bodies) == len(node['graphics']) and all(same(a, b) and a.isValid and same(a.parent, group)
                    for a, b in zip(bodies, node['graphics'])),
                    'preview_ownership', 'Exact owned graphics body inventory changed.')

    def _graphics(self):
        self._ownership()
        require(self.root.isSelectable is False and self.root.isChildrenSelectable is False
                and self.root.isVisible is True and matrix_values(self.root) == IDENTITY,
                'preview_ownership', 'Owned root graphics properties changed.')
        for node in self.nodes:
            group = node['group']
            require(group.id == node['id'] and group.isVisible is True
                    and equivalent(matrix_values(group), node['matrix']),
                    'preview_ownership', 'Owned graphics group or last applied transform changed.')
            require(len(node['graphics']) == len(node['witnesses']), 'preview_copy_mismatch', 'Incomplete geometry copying.')
            for graphic, witness in zip(node['graphics'], node['witnesses']):
                require(graphic.isVisible is True and matrix_values(graphic) == IDENTITY
                        and equivalent(body_witness(graphic.bRepBody, True), witness),
                        'preview_copy_mismatch', 'Owned copied geometry changed or copy readback differs.')

    def create(self):
        main_thread()
        require(self.state == 'prepared', 'preview_state', 'Creation cannot be retried or replayed.')
        self._guard()
        require(self.lease.design.rootComponent.customGraphicsGroups.count == 0,
                'preview_not_blank', 'Presentation already contains graphics.')
        token = self.lease.begin_effect()
        self.state = 'creating'
        try:
            self.root = self.lease.design.rootComponent.customGraphicsGroups.add()
            require(self.root is not None and self.root.isValid, 'preview_create_unknown', 'Graphics root creation could not be verified.')
            self.root.id = 'AdomExplodedPreview:' + self.preview_id
            require(self.root.id == 'AdomExplodedPreview:' + self.preview_id, 'preview_create_unknown', 'Root ownership ID setter was ignored.')
            self._root_id_written = True
            self.root.isSelectable = False
            self.root.isChildrenSelectable = False
            result = json.loads(self.prepared._result_json)
            homes = {r['instanceId']: r['homeMatrix'] for r in result['plan']['instances']}
            for row in self.prepared._geometry:
                group = self.root.addGroup()
                require(group is not None, 'preview_create_unknown', 'Graphics subgroup creation could not be verified.')
                node = {'group': group, 'id': row['instanceId'] or 'fixed-root', 'graphics': [],
                        'witnesses': row['witnesses'], 'matrix': list(IDENTITY), 'instanceId': row['instanceId']}
                self.nodes.append(node)  # Keep exact ownership before ANY native getter/setter.
                require(group.isValid, 'preview_create_unknown', 'Returned graphics subgroup is invalid.')
                group.id = node['id']
                for body in row['bodies']:
                    graphic = group.addBRepBody(body)
                    require(graphic is not None, 'preview_create_unknown', 'Independent BRep graphics creation could not be verified.')
                    node['graphics'].append(graphic)  # Retain before even isValid may throw.
                    require(graphic.isValid, 'preview_create_unknown', 'Returned graphics body is invalid.')
                pose = homes.get(row['instanceId'], IDENTITY)
                node['matrix'] = intended_native_values(pose)
                group.transform = native_matrix(pose)
            self._graphics()
            self.lease.finish_effect(token)
            self._guard()
            self.state = 'ready'
            # A new presentation normally became active in the harness. Return to
            # the exact source; never steal another intervening active document.
            current = self.lease.app.activeDocument
            if same(current, self.lease.document):
                require(self.prepared.document.activate() is True and same(self.lease.app.activeDocument, self.prepared.document),
                        'preview_activation_refused', 'Could not return to the exact source document.')
            else:
                require(same(current, self.prepared.document), 'preview_activation_refused', 'Another active document intervened; no activation performed.')
            self._guard()
            return self.receipt()
        except Exception:
            self.state = 'retained_failure'
            self.lease.failed_effect(token, self._ownership)
            raise

    def sample(self, index):
        main_thread()
        samples = json.loads(self._sample_json)
        require(type(index) is int and 0 <= index < len(samples), 'invalid_input', 'Sample requires an exact stored integer index.')
        require(self.state == 'ready', 'preview_state', 'Sampling requires a verified ready preview.')
        self._guard(); self._graphics()
        poses = {r['instanceId']: r['matrix'] for r in samples[index]['poses']}
        token = self.lease.begin_effect()
        self.state = 'sampling'
        try:
            for node in self.nodes:
                pose = poses[node['instanceId']] if node['instanceId'] is not None else IDENTITY
                node['matrix'] = intended_native_values(pose)
                node['group'].transform = native_matrix(pose)
            self._graphics(); self.lease.finish_effect(token); self._guard()
            self.sample_index = index
            self.state = 'ready'
            return self.receipt()
        except Exception:
            self.state = 'retained_failure'
            self.lease.failed_effect(token, self._ownership)
            raise

    def activate(self):
        main_thread()
        require(self.state == 'ready', 'preview_state', 'Only a verified ready preview can be activated.')
        self._guard(); self._graphics()
        require(any(same(self.lease.app.activeDocument, doc) for doc in (self.prepared.document, self.lease.document)),
                'preview_activation_refused', 'Another active document intervened; no activation performed.')
        require(self.lease.document.activate() is True and same(self.lease.app.activeDocument, self.lease.document),
                'preview_activation_refused', 'Presentation activation was not acknowledged.')
        self._guard(); self._graphics()
        # No foreground/viewport/camera settings or automatic capture.
        return self.receipt()

    def dispose(self):
        main_thread()
        self.lease.check(cleanup=True)  # Source may have closed; disposal still requires exact owned presentation.
        require(self.state in ('ready', 'retained_failure'), 'preview_state', 'Only retained owned graphics can be disposed.')
        # Failed samples may have partial transforms or a stale/closed source.
        # Disposal still requires every native descendant to be exactly owned.
        self._ownership()
        require(not self._delete_issued, 'preview_dispose_unknown', 'An earlier native deletion attempt remains unresolved.')
        token = self.lease.begin_effect(cleanup=True)
        self.state = 'disposing'
        try:
            self._delete_issued = True
            require(self.root.deleteMe() is True, 'preview_dispose_unknown', 'Owned graphics deletion was not acknowledged.')
            require(self.lease.design.rootComponent.customGraphicsGroups.count == 0,
                    'preview_dispose_unknown', 'Owned graphics absence could not be verified.')
            require(self.root.isValid is False, 'preview_dispose_unknown', 'Deleted retained root remains valid.')
            self.lease.finish_effect(token)
            self.state = 'disposed'
            return self.receipt()
        except Exception:
            self.state = 'retained_failure'
            self.lease._effect = None  # Delete outcome cannot authorize another attempt.
            self.lease.cleanup_state = None
            raise

    def receipt(self):
        main_thread()
        return {'prototypeOnly': True, 'previewId': self.preview_id, 'planId': self.prepared.plan_id,
                'samplesDigest': self.samples_digest, 'sessionId': self.lease.session_id,
                'sourceTarget': copy.deepcopy(self.prepared._target), 'presentationDocumentId': self.lease.document_id,
                'state': self.state, 'sampleIndex': self.sample_index, 'groupCount': len(self.nodes),
                'bodyCopyCount': sum(len(n['graphics']) for n in self.nodes),
                'sourceMoved': False, 'sourceVisibilityWritten': False, 'storyboardCreated': False,
                'visualQualified': False, 'archivePersistence': 'UNVERIFIED',
                'copyWitness': 'solid volume, bounds, topology counts and indexed vertex coordinates; appearances/textures unqualified',
                'documentCleanup': 'owned caller responsibility; no automatic close or global undo'}
