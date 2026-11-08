"""Session-owned public controller for finite exploded presentation graphics.

Direct synchronous WRITES use the mechanical durable journal, not PTransaction.
History receipts describe past completion; this registry describes live resources.
No document create/close, global Undo, timer, capture or cross-session adoption.
"""
import copy
import hashlib
import json
from pathlib import Path
import time
import uuid

import adsk.fusion

from . import core
from . import exploded_preview_prototype as backend
from .core import MechanicalError, SESSION_ID, require, same
from .journal import fingerprint
from .exploded_view import request_arguments

SCHEMA_VERSION = 1
BACKEND_SOURCE_DIGEST = hashlib.sha256(Path(backend.__file__).read_bytes()).hexdigest()
MAX_ACTIVE = 4
MAX_ENTRIES = 128
MAX_PLAN_BYTES = 2 * 1024 * 1024
MAX_TOTAL_PLAN_BYTES = 8 * 1024 * 1024
MAX_TOTAL_BODIES = 512


def bounded_text(value, name, maximum=128):
    require(isinstance(value, str) and 0 < len(value) <= maximum, 'invalid_input', name + ' must be a bounded nonempty string.')
    return value


def exact_target(value):
    require(isinstance(value, dict) and set(value) == {'documentId', 'revision'}, 'invalid_input', 'An exact documentId/revision target is required.')
    for name in value:
        bounded_text(value[name], name, 256)
    return copy.deepcopy(value)


def flags(doc):
    result = (doc.isSaved, doc.isModified)
    require(all(type(v) is bool for v in result), 'preview_state_unavailable', 'Document flags are unavailable.')
    return result


def known_identity(app, document_id):
    """Resolve only an already-issued session identity; no revision adoption."""
    backend.main_thread()
    matches = [doc for doc, key in core._documents if key == document_id]
    require(len(matches) == 1 and matches[0].isValid, 'preview_document_unavailable', 'Exact session document is unavailable.')
    doc = matches[0]
    require(sum(same(item, doc) for item in backend.bounded(app.documents, 128)) == 1,
            'preview_document_unavailable', 'Exact session document is not uniquely open.')
    design = adsk.fusion.Design.cast(doc.products.itemByProductType('DesignProductType'))
    require(design is not None and design.isValid and same(design.parentDocument, doc),
            'preview_document_unavailable', 'Exact document has no matching Design.')
    require(core.document_identity(doc) == document_id, 'preview_document_unavailable', 'Session document identity differs.')
    return doc, design


def known_document(app, wanted):
    wanted = exact_target(wanted)
    doc, design = known_identity(app, wanted['documentId'])
    require(core.revision(design) == wanted['revision'],
            'stale_revision', 'Exact document identity/revision changed.')
    return doc, design


def blank_model(design):
    root = design.rootComponent
    require(all(backend.count(getattr(root, field), backend.MAX_BODIES) == 0
                for field in ('bRepBodies', 'meshBodies', 'occurrences', 'sketches', 'features'))
            and backend.count(design.allComponents, 1) == 1 and same(design.allComponents.item(0), root)
            and backend.count(design.allParameters, 0) == 0 and backend.count(design.timeline, 0) == 0
            and backend.count(root.customGraphicsGroups, 0) == 0,
            'preview_not_blank', 'Presentation must be a blank Design with no graphics before reservation.')


def retained_json_bytes(*objects):
    """Count every distinct retained serialized string, including duplicate content."""
    seen = set()
    total = 0
    for obj in objects:
        for key, value in vars(obj).items():
            if key.endswith('_json') and isinstance(value, str) and id(value) not in seen:
                seen.add(id(value))
                total += len(value.encode('utf-8'))
    return total


def terminal_record(history, operation_id, operation=None):
    record = history.store().lookup(SESSION_ID, operation_id)
    require(isinstance(record, dict) and record.get('sessionId') == SESSION_ID
            and record.get('operationId') == operation_id and record.get('phase') in ('succeeded', 'failed', 'not_executed'),
            'preview_history_unresolved', 'A coherent current-session terminal operation record is required.')
    result = record.get('result')
    require(isinstance(result, dict) and result.get('sessionId') == SESSION_ID and result.get('operationId') == operation_id
            and type(result.get('success')) is bool and result['success'] == (record['phase'] == 'succeeded'),
            'preview_history_unresolved', 'Terminal record identity/result is incoherent.')
    if operation is not None:
        require(record.get('operation') == operation and result.get('operation') == operation,
                'preview_ownership', 'Document creation proof is not the required operation.')
    return record


class Controller:
    def __init__(self):
        self.epoch = str(uuid.uuid4())
        self.entries = {}
        self.claims = {}

    def guard(self, args):
        backend.main_thread()
        require(args.get('sessionId') == SESSION_ID, 'session_mismatch', 'Preview control requires the current session.')

    def entry(self, app, args):
        self.guard(args)
        key = bounded_text(args.get('previewId'), 'previewId')
        entry = self.entries.get(key)
        require(entry is not None and entry['epoch'] == self.epoch and entry['sessionId'] == SESSION_ID,
                'preview_session_expired', 'No live controller capability identifies this preview; history does not rebind native resources.')
        require(same(entry['app'], app), 'preview_ownership', 'Preview belongs to another application.')
        return entry

    def ownership(self, entry):
        backend.main_thread()
        require(entry['epoch'] == self.epoch and entry['sessionId'] == SESSION_ID
                and self.entries.get(entry['previewId']) is entry
                and self.claims.get(entry['target']['documentId']) == entry['previewId'],
                'preview_ownership', 'Exclusive live presentation lease is unavailable.')
        doc, design = known_identity(entry['app'], entry['target']['documentId'])
        require(same(doc, entry['document']) and same(design, entry['design']) and doc.isSaved is False
                and doc.creationId == entry['creationId'],
                'preview_ownership', 'Retained native presentation identity differs.')
        return {'success': True, 'ownerId': entry['previewId'], 'sessionId': SESSION_ID,
                'documentId': entry['target']['documentId']}

    def lease(self, entry, cleanup=False):
        if entry['lease'] is None:
            entry['lease'] = backend.OwnedPresentationLease(entry['app'], entry['document'], entry['design'],
                                                           entry['previewId'], lambda: self.ownership(entry), expected_state=entry['initialPresentationState'])
        entry['lease'].check(cleanup=cleanup)
        return entry['lease']

    def reserve(self, app, args, history):
        self.guard(args)
        wanted = exact_target(args.get('target'))
        source = exact_target(args.get('sourceTarget'))
        require(wanted['documentId'] != source['documentId'], 'preview_ownership', 'Source and presentation must be distinct documents.')
        creation_id = bounded_text(args.get('creationOperationId'), 'creationOperationId')
        record = terminal_record(history, creation_id, 'new_design')
        result = record['result']
        data = result.get('data')
        require(record['phase'] == 'succeeded' and isinstance(data, dict) and data.get('target') == wanted
                and isinstance(data.get('document'), dict) and data['document'].get('saved') is False,
                'preview_ownership', 'Committed new_design result does not identify this unsaved presentation target.')
        creation = bounded_text(data['document'].get('creationId'), 'creationId', 256)
        previous = [item for item in self.entries.values() if item['target']['documentId'] == wanted['documentId']]
        predecessor = previous[-1] if previous else None
        if predecessor is None:
            doc, design = known_document(app, wanted)  # Initial claim remains strict against real new_design.
        else:
            require(predecessor['state'] == 'disposed' and self.last_history(predecessor, history)['terminalRecorded']
                    and predecessor['priorUnresolved'] is None and predecessor['finalPresentationState'] is not None,
                    'preview_history_unresolved', 'Previous presentation use lacks coherent durable verified disposal/release evidence.')
            require(predecessor['creationOperationId'] == creation_id and predecessor['creationDigest'] == record['requestDigest']
                    and predecessor['target'] == wanted and predecessor['epoch'] == self.epoch,
                    'preview_ownership', 'Reuse must preserve its exact original creation proof and controller epoch.')
            doc, design = known_identity(app, wanted['documentId'])
            require(same(doc, predecessor['document']) and same(design, predecessor['design'])
                    and same(design.rootComponent, predecessor['lease'].root_component),
                    'preview_ownership', 'Disposed presentation native identity changed.')
        require(same(app.activeDocument, doc) and doc.creationId == creation and doc.isSaved is False,
                'preview_ownership', 'Claim requires the exact active unsaved document returned by new_design.')
        blank_model(design)
        initial_state = backend.presentation_observation(doc, design)
        require(initial_state['revision'] == wanted['revision'] if predecessor is None
                else initial_state == predecessor['finalPresentationState'],
                'stale_revision', 'Presentation changed outside the verified creation/disposal chain.')
        source_doc, source_design = known_document(app, source)
        source_flags = flags(source_doc)
        require(wanted['documentId'] not in self.claims, 'preview_ownership', 'Presentation already has an exclusive preview lease.')
        require(len(self.entries) < MAX_ENTRIES and len(self.claims) < MAX_ACTIVE
                and sum(e['planBytes'] for e in self.entries.values()) + MAX_PLAN_BYTES <= MAX_TOTAL_PLAN_BYTES
                and sum(e['bodyBudget'] for e in self.entries.values()) + backend.MAX_BODIES <= MAX_TOTAL_BODIES,
                'preview_capacity', 'Session preview capacity is full; retained resources are never evicted.')
        key = self.epoch + ':' + str(uuid.uuid4())
        entry = {'previewId': key, 'epoch': self.epoch, 'sessionId': SESSION_ID, 'app': app,
                 'document': doc, 'design': design, 'target': wanted, 'sourceTarget': source,
                 'sourceDocument': source_doc, 'sourceDesign': source_design, 'sourceFlags': source_flags,
                 'creationOperationId': creation_id, 'creationDigest': record['requestDigest'], 'creationId': creation,
                 'generation': 0, 'state': 'reserved', 'lease': None, 'backend': None,
                 'initialPresentationState': initial_state, 'finalPresentationState': None,
                 'predecessorPreviewId': predecessor['previewId'] if predecessor is not None else None,
                 'planBytes': MAX_PLAN_BYTES, 'bodyBudget': backend.MAX_BODIES,
                 'planId': None, 'samplesDigest': None, 'sampleCount': None, 'sampleIndex': None,
                 'activationIssued': False, 'factoryMayHaveBeenIssued': False, 'nativeWorkInFlight': False,
                 'lastOperationId': args['operationId'], 'lastOperation': args['op'], 'lastRequestDigest': fingerprint(args), 'lastReturned': False,
                 'lastExpectedResultDigest': None,
                 'lastObserved': None, 'priorUnresolved': None, 'disposition': None,
                 'disposalAttempted': False, 'disposalOutcome': 'not_attempted'}
        self.entries[key] = entry
        self.claims[wanted['documentId']] = key
        return entry

    def begin(self, entry, args):
        require(not entry['nativeWorkInFlight'], 'preview_in_progress', 'A native preview invocation is still in flight.')
        entry['generation'] += 1
        entry['lastOperationId'] = args['operationId']
        entry['lastOperation'] = args['op']
        entry['lastExpectedResultDigest'] = None
        entry['lastRequestDigest'] = fingerprint(args)
        entry['lastReturned'] = False
        entry['nativeWorkInFlight'] = True

    def last_history(self, entry, history):
        if entry['lastOperationId'] is None:
            return {'terminalRecorded': False, 'reason': 'no_effect_operation'}
        try:
            record = terminal_record(history, entry['lastOperationId'])
            require(record.get('requestDigest') == entry['lastRequestDigest']
                    and record['phase'] in ('succeeded', 'failed') and entry['lastExpectedResultDigest'] is not None
                    and core.digest(record['result']) == entry['lastExpectedResultDigest'],
                    'preview_history_unresolved', 'Last operation digest or observed terminal result differs.')
            return {'terminalRecorded': True, 'phase': record['phase'], 'operationId': entry['lastOperationId'],
                    'requestDigest': entry['lastRequestDigest']}
        except Exception as error:
            return {'terminalRecorded': False, 'operationId': entry['lastOperationId'],
                    'requestDigest': entry['lastRequestDigest'], 'errorCode': getattr(error, 'code', 'preview_history_unresolved'),
                    'error': str(error)[:512]}

    def existing(self, app, args, history, disposal=False):
        entry = self.entry(app, args)
        require(exact_target(args.get('target')) == entry['target'], 'preview_ownership', 'Presentation target differs from its lease.')
        require(type(args.get('expectedGeneration')) is int and args['expectedGeneration'] == entry['generation'],
                'preview_generation_conflict', 'Preview generation changed; inspect before another intent.',
                currentGeneration=entry['generation'])
        require(entry['state'] != 'disposed' and not entry['nativeWorkInFlight'], 'preview_state', 'Preview is disposed or in flight.')
        if disposal:
            require(not entry['disposalAttempted'], 'preview_disposal_unresolved',
                    'A prior deletion attempt remains unresolved; a new operation ID cannot authorize another delete.')
        self.lease(entry, cleanup=disposal)
        previous = self.last_history(entry, history)
        if not previous['terminalRecorded']:
            require(disposal and entry['lastReturned'] and entry['lastObserved'] is not None
                    and entry['backend'] is not None and entry['backend'].root is not None
                    and entry['backend'].state in ('ready', 'retained_failure'),
                    'preview_history_unresolved', 'Prior operation history is unresolved; automatic continuation is disabled.',
                    priorOperation=previous)
            entry['backend']._ownership()  # Explicit cleanup exception, never adoption.
            previous['observedResultDigest'] = core.digest(entry['lastObserved'])
        if not disposal:
            require(entry['state'] == 'ready', 'preview_state', 'Only a verified ready preview can be sampled or shown.')
        return entry, None if previous['terminalRecorded'] else previous

    def source_check(self, entry):
        doc, design = known_document(entry['app'], entry['sourceTarget'])
        require(same(doc, entry['sourceDocument']) and same(design, entry['sourceDesign'])
                and flags(doc) == entry['sourceFlags'], 'preview_source_changed', 'Source identity/revision/flags changed.')

    def compact(self, entry, scope='operation_completion'):
        return {'kind': 'EXPLODED_PREVIEW', 'receiptScope': scope, 'controllerSchemaVersion': SCHEMA_VERSION,
                'backendSourceDigest': BACKEND_SOURCE_DIGEST, 'previewId': entry['previewId'], 'controllerEpoch': self.epoch,
                'sessionId': SESSION_ID, 'presentationTarget': copy.deepcopy(entry['target']),
                'sourceTarget': copy.deepcopy(entry['sourceTarget']), 'creationOperationId': entry['creationOperationId'],
                'presentationTargetScope': 'immutable_creation_anchor',
                'lastVerifiedPresentationRevision': entry['lease'].model_revision if entry['lease'] is not None else entry['initialPresentationState']['revision'],
                'presentationVerificationSequence': entry['lease'].sequence if entry['lease'] is not None else 0,
                'presentationFailureObservation': copy.deepcopy(entry['lease'].failure_observation) if entry['lease'] is not None else None,
                'predecessorPreviewId': entry['predecessorPreviewId'],
                'observedEmptyTarget': {'documentId': entry['target']['documentId'], 'revision': entry['finalPresentationState']['revision']} if entry['finalPresentationState'] is not None else None,
                'planId': entry['planId'], 'samplesDigest': entry['samplesDigest'], 'sampleCount': entry['sampleCount'],
                'state': entry['state'], 'generation': entry['generation'],
                'sampleIndex': entry['sampleIndex'] if entry['state'] == 'ready' else None,
                'lastVerifiedSampleIndex': entry['sampleIndex'],
                'pose': ('sample' if entry['sampleIndex'] is not None else 'home') if entry['state'] == 'ready' else 'unverified',
                'disposalAttempted': entry['disposalAttempted'], 'disposalOutcome': entry['disposalOutcome'],
                'lastOperationId': entry['lastOperationId'], 'nativeWorkInFlight': entry['nativeWorkInFlight'],
                'documentDisposition': 'open_for_caller', 'disposition': entry['disposition'],
                'sourceMoved': False, 'sourceVisibilityWritten': False, 'rollbackClaimed': False,
                'rendered': False, 'storyboardCreated': False, 'visualQualified': False,
                'archivePersistence': 'UNVERIFIED', 'priorOperationUnresolved': entry['priorUnresolved'] is not None,
                'priorUnresolvedOperation': copy.deepcopy(entry['priorUnresolved'])}

    def release(self, entry, disposition):
        require(self.claims.get(entry['target']['documentId']) == entry['previewId'], 'preview_ownership', 'Exclusive claim changed.')
        require(entry['lease'] is not None, 'preview_ownership', 'Verified presentation lease is required for release.')
        entry['finalPresentationState'] = copy.deepcopy(entry['lease'].check())
        del self.claims[entry['target']['documentId']]
        entry.update(state='disposed', disposition=disposition, planBytes=0, bodyBudget=0)
        entry['backend'] = None  # Drop large stored samples/geometry; retain bounded tombstone/native doc proof.

    def no_graphics_release(self, entry):
        require(not entry['factoryMayHaveBeenIssued'], 'preview_reconciliation_required', 'Graphics creation may have been issued.')
        self.lease(entry)
        require(backend.count(entry['design'].rootComponent.customGraphicsGroups, 0) == 0,
                'preview_reconciliation_required', 'No-graphics absence was not verified.')
        self.release(entry, 'reservation_released_no_graphics')

    def failure(self, entry, error):
        entry['state'] = 'retained_failure'
        recovery_error = None
        if not entry['factoryMayHaveBeenIssued']:
            try:
                self.no_graphics_release(entry)
            except Exception as cleanup_error:
                recovery_error = str(cleanup_error)[:512]
        observed = {'errorCode': getattr(error, 'code', 'preview_native_failed'), 'error': str(error)[:2048],
                    'effectState': 'possible_graphics' if entry['factoryMayHaveBeenIssued'] else 'activation_only' if entry['activationIssued'] else 'none',
                    'preview': self.compact(entry), 'recoveryError': recovery_error}
        observed['preview']['nativeWorkInFlight'] = False  # Synchronous native effect already returned/threw.
        entry['lastObserved'] = observed
        entry['lastExpectedResultDigest'] = core.digest({
            'success': False, 'sessionId': SESSION_ID, 'operationId': entry['lastOperationId'],
            'errorCode': observed['errorCode'], 'error': observed['error'], 'effectState': observed['effectState'],
            'preview': observed['preview'], 'recoveryError': recovery_error, 'rollbackClaimed': False})
        raise MechanicalError(observed['errorCode'], observed['error'], effectState=observed['effectState'],
                              preview=observed['preview'], recoveryError=recovery_error, rollbackClaimed=False) from error

    def create(self, app, args, history):
        self.guard(args)
        bounded_text(args.get('operationId'), 'operationId')
        bounded_text(args.get('expectedPlanId'), 'expectedPlanId', 64)
        planner = copy.deepcopy(args['planner'])
        planner.update(op='exploded_view_plan', sessionId=SESSION_ID, target=exact_target(args['sourceTarget']))
        # Validate pure planner mathematics before reserving resources/activation.
        request_arguments(planner)
        entry = self.reserve(app, args, history)
        try:
            self.lease(entry)
            self.source_check(entry)
            self.begin(entry, args)
            entry['state'] = 'creating'
            entry['activationIssued'] = True
            require(entry['sourceDocument'].activate() is True and same(app.activeDocument, entry['sourceDocument']),
                    'preview_activation_refused', 'Exact source activation was not acknowledged.')
            self.source_check(entry)
            prepared = backend.prepare(app, planner, args['expectedPlanId'])
            preview = backend.Preview(prepared, entry['lease'])
            entry['backend'] = preview  # Retain before any subsequent failure or graphics effect.
            byte_count = retained_json_bytes(prepared, preview)
            require(byte_count <= MAX_PLAN_BYTES, 'preview_capacity', 'Total retained plan/sample JSON exceeds its reserved memory bound.')
            entry.update(planId=prepared.plan_id, samplesDigest=preview.samples_digest,
                         sampleCount=len(json.loads(preview._sample_json)), planBytes=byte_count)
            entry['factoryMayHaveBeenIssued'] = True
            preview.create()
            self.source_check(entry)
            entry.update(state='ready', bodyBudget=sum(len(n['graphics']) for n in preview.nodes))
            result = self.compact(entry)
            result['nativeWorkInFlight'] = False
            entry['lastObserved'] = copy.deepcopy(result)
            entry['lastExpectedResultDigest'] = core.digest({
                'success': True, 'sessionId': SESSION_ID, 'operationId': entry['lastOperationId'],
                'operation': entry['lastOperation'], 'data': result})
            return result
        except Exception as error:
            self.failure(entry, error)
        finally:
            entry['nativeWorkInFlight'] = False
            entry['lastReturned'] = True

    def action(self, action, app, args, history):
        self.guard(args)
        bounded_text(args.get('operationId'), 'operationId')
        if action == 'sample':
            require(type(args.get('sampleIndex')) is int and args['sampleIndex'] >= 0,
                    'invalid_input', 'Sample index must be a nonnegative integer.')
        entry, unresolved = self.existing(app, args, history, disposal=action == 'dispose')
        preview = entry['backend']
        if action == 'sample':
            require(args['sampleIndex'] < entry['sampleCount'], 'invalid_input', 'Sample index exceeds the immutable stored samples.')
        if action != 'dispose':
            self.source_check(entry)
            preview._guard(); preview._graphics()
        elif preview is not None:
            preview._ownership()
        if action == 'show':
            require(any(same(app.activeDocument, doc) for doc in (entry['sourceDocument'], entry['document'])),
                    'preview_activation_refused', 'Another active document intervened; no activation performed.')
        self.begin(entry, args)
        entry['priorUnresolved'] = unresolved
        entry['state'] = 'updating'
        try:
            if action == 'sample':
                preview.sample(args['sampleIndex'])
                entry['sampleIndex'] = args['sampleIndex']
                self.source_check(entry)
                entry['state'] = 'ready'
            elif action == 'show':
                preview.activate()
                self.source_check(entry)
                entry['state'] = 'ready'
            elif action == 'dispose':
                if preview is None:
                    self.no_graphics_release(entry)
                else:
                    entry['disposalAttempted'] = True  # Before native deletion; never retry uncertain effects.
                    entry['disposalOutcome'] = 'unknown'
                    preview.dispose()
                    entry['disposalOutcome'] = 'verified_absent'
                    self.release(entry, 'graphics_disposed')
            result = self.compact(entry)
            result['nativeWorkInFlight'] = False
            entry['lastObserved'] = copy.deepcopy(result)
            entry['lastExpectedResultDigest'] = core.digest({
                'success': True, 'sessionId': SESSION_ID, 'operationId': entry['lastOperationId'],
                'operation': entry['lastOperation'], 'data': result})
            return result
        except Exception as error:
            self.failure(entry, error)
        finally:
            entry['nativeWorkInFlight'] = False
            entry['lastReturned'] = True

    def inspect(self, app, args, history):
        entry = self.entry(app, args)
        result = self.compact(entry, 'live_observation')
        result['observedAt'] = time.time()
        result['history'] = self.last_history(entry, history)
        result['usable'] = False
        result['lastKnownDocumentDisposition'] = result['documentDisposition']
        result['documentDisposition'] = 'unknown_current'
        errors = []
        if entry['state'] == 'disposed':
            result['receiptScope'] = 'registry_tombstone'
            result['nativeResourceRights'] = False
            result['nativeObservation'] = 'NOT_PERFORMED'
            result['observationComplete'] = False
            return result
        try:
            self.lease(entry, cleanup=entry['state'] == 'retained_failure')
            result['presentationOwnershipVerified'] = True
            result['documentDisposition'] = 'verified_open_owned'
            preview = entry['backend']
            if preview is not None:
                preview._ownership()
                result['graphicsOwnershipVerified'] = True
                preview._graphics()
                result['graphicsReadbackVerified'] = True
            else:
                result['graphicsCount'] = backend.count(entry['design'].rootComponent.customGraphicsGroups, 1)
        except Exception as error:
            errors.append({'scope': 'presentation', 'code': getattr(error, 'code', 'preview_read_failed'), 'message': str(error)[:512]})
        try:
            self.source_check(entry)
            if entry['backend'] is not None:
                entry['backend'].prepared.check()
            result['sourceUnchanged'] = True
        except Exception as error:
            errors.append({'scope': 'source', 'code': getattr(error, 'code', 'preview_read_failed'), 'message': str(error)[:512]})
        result['errors'] = errors
        result['observationComplete'] = not errors
        result['usable'] = not errors and entry['state'] == 'ready' and result['history']['terminalRecorded'] and not entry['nativeWorkInFlight']
        return result


_controller = Controller()


def create(app, args, history):
    return _controller.create(app, args, history)


def sample(app, args, history):
    return _controller.action('sample', app, args, history)


def show(app, args, history):
    return _controller.action('show', app, args, history)


def dispose(app, args, history):
    return _controller.action('dispose', app, args, history)


def inspect(app, args, history):
    return _controller.inspect(app, args, history)


WRITES = {'exploded_preview_create': create, 'exploded_preview_sample': sample,
          'exploded_preview_show': show, 'exploded_preview_dispose': dispose}
READS = {'exploded_preview_inspect': inspect}
