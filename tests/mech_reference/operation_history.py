"""Plain-data admission, status and persistence boundaries for synchronous writes."""
import copy
import hashlib
import threading

from .journal import AdmissionConflict, Journal, JournalError, STORAGE_VERSION, TERMINAL, canonical, fingerprint


class OperationHistory:
    def __init__(self, session_id, source_version, journal=None):
        self.session_id, self.source_version = session_id, source_version
        self._journal = journal
        self._lock = threading.RLock()
        self._unconfirmed = {}
        self._known_requests = {}
        self._observation_bytes = 0

    def store(self):
        with self._lock:
            if self._journal is None:
                self._journal = Journal()
            return self._journal

    def failure(self, error, descriptor=None, *, observed=None, not_started=False, remember=False):
        result = {'success': False, 'sessionId': self.session_id, 'errorCode': error.code,
                  'error': str(error)[:4096], 'outcome': 'not_executed' if not_started else 'unknown',
                  'nativeExecutionStarted': False if not_started else None}
        if descriptor:
            result['operationId'] = descriptor['operationId']
            result['requestDigest'] = descriptor.get('requestDigest')
        if observed is not None:
            result.update(errorCode='operation_reconciliation_required', rollbackClaimed=False, nativeExecutionStarted=True,
                          persistenceError={'code': error.code, 'message': str(error)[:4096]})
            try:
                raw = canonical(observed)
                result['observation'] = {'sha256': hashlib.sha256(raw).hexdigest(), 'bytes': len(raw)}
                if len(raw) <= 65536:
                    result['observedResult'] = copy.deepcopy(observed)
                else:
                    result['observation']['resultOmitted'] = True
            except JournalError:
                result['observation'] = {'resultOmitted': True, 'reason': 'Observed result exceeds record limits or is not canonical JSON.'}
            remember = True
        key = (descriptor['sessionId'], descriptor['operationId']) if descriptor else None
        with self._lock:
            known = key is not None and self._known_requests.get(key) == descriptor.get('requestDigest') and key in self._known_requests
            old = self._unconfirmed.get(key, {})
            if known and old.get('observation') and observed is None:
                result = copy.deepcopy(old)
                result.pop('_payloadBytes', None)
                result['laterPersistenceFailure'] = {'code': error.code, 'message': str(error)[:4096]}
                remember = True
        if observed is not None:
            result.update(reconciliationRequired=True, durability={'terminalRecorded': False})
        if remember and known:
            result.update(reconciliationRequired=True, durability={'terminalRecorded': False})
            cached = copy.deepcopy(result)
            # Keep plain failure evidence for every admitted operation, but cap
            # retained native-result payloads across the process at four MiB.
            with self._lock:
                key = (descriptor['sessionId'], descriptor['operationId'])
                old = self._unconfirmed.get(key, {})
                if 'observation' in old and observed is None:
                    # A duplicate event or failed delivery acknowledgement may
                    # not erase stronger evidence from an already-run handler.
                    cached = copy.deepcopy(old)
                    cached.pop('_payloadBytes', None)
                    cached['laterPersistenceFailure'] = {'code': error.code, 'message': str(error)[:4096]}
                    result = copy.deepcopy(cached)
                payload_bytes = len(canonical(cached.get('observedResult')))
                self._observation_bytes -= old.get('_payloadBytes', 0)
                if self._observation_bytes + payload_bytes > 4 * 1024 * 1024:
                    cached.pop('observedResult', None)
                    if 'observation' in cached:
                        cached['observation']['resultOmitted'] = True
                    payload_bytes = 0
                cached['_payloadBytes'] = payload_bytes
                self._observation_bytes += payload_bytes
                self._unconfirmed[key] = cached
        return result

    def unconfirmed(self, key):
        with self._lock:
            result = copy.deepcopy(self._unconfirmed.get(key))
        if result:
            result.pop('_payloadBytes', None)
        return result

    def remember_record(self, record):
        if record['sessionId'] == self.session_id:
            with self._lock:
                key = (record['sessionId'], record['operationId'])
                existing = self._known_requests.get(key)
                if existing is None or existing == record['requestDigest']:
                    self._known_requests[key] = record['requestDigest']

    @staticmethod
    def metadata(record):
        return {key: copy.deepcopy(record[key]) for key in
                ('storageVersion', 'sessionId', 'operationId', 'operation', 'sourceVersion', 'requestDigest',
                 'requestedTarget', 'observedDocument', 'phase', 'createdAt', 'updatedAt', 'events')}

    def terminal_reply(self, record, replayed=False):
        result = copy.deepcopy(record['result'])
        result['durability'] = {'terminalRecorded': True, 'storageVersion': STORAGE_VERSION,
                                'recordedSessionId': record['sessionId'], 'phase': record['phase']}
        if replayed:
            result['replayed'] = True
        return result

    def pending_reply(self, record):
        if record['phase'] in TERMINAL:
            return self.terminal_reply(record, replayed=True)
        unconfirmed = self.unconfirmed((record['sessionId'], record['operationId']))
        if unconfirmed:
            return unconfirmed
        if record['phase'] == 'admitted':
            return {'success': False, 'sessionId': self.session_id, 'operationId': record['operationId'],
                    'errorCode': 'operation_reconciliation_required', 'outcome': 'unknown',
                    'reconciliationRequired': True, 'durability': {'terminalRecorded': False},
                    'error': 'Admission has no durable dispatch claim. Query status; automatic redispatch is disabled.'}
        return {'success': False, 'sessionId': self.session_id, 'operationId': record['operationId'],
                'errorCode': 'operation_in_progress', 'error': 'Operation is admitted; query its recorded status. No duplicate dispatch occurred.',
                'phase': record['phase'], 'outcome': 'pending'}

    def admit(self, request, dispatch_id):
        descriptor = {'sessionId': request['sessionId'], 'operationId': request['operationId'],
                      'requestDigest': fingerprint(request), 'dispatchId': dispatch_id}
        if request['sessionId'] != self.session_id:
            return None, {'success': False, 'sessionId': self.session_id, 'errorCode': 'session_mismatch',
                          'error': 'Old-session operation history is readable, but native execution is never replayed across sessions.'}
        accepted = None
        try:
            accepted = self.store().admit(request, self.source_version)
            self.remember_record(accepted['record'])
            if not accepted['created']:
                return None, self.pending_reply(accepted['record'])
            transition = self.store().transition(descriptor['sessionId'], descriptor['operationId'], descriptor['requestDigest'],
                                                  'dispatched', dispatch_id=dispatch_id)
            if not transition['claimed']:
                return None, self.pending_reply(transition['record'])
            return descriptor, None
        except AdmissionConflict as error:
            # This proof applies to the submitted digest, not to the existing
            # operation sharing its ID. Never replace/cache its execution state.
            if (accepted is None and error.session_id == descriptor['sessionId']
                    and error.operation_id == descriptor['operationId']
                    and error.submitted_digest == descriptor['requestDigest']
                    and error.existing_digest != error.submitted_digest):
                reply = self.failure(error, descriptor, not_started=True)
                reply['conflict'] = {'scope': 'submitted_request', 'existingRequestDigest': error.existing_digest}
                return None, reply
            return None, self.failure(error, descriptor)
        except JournalError as error:
            if accepted is None:
                # The admission COMMIT itself may have succeeded before its
                # acknowledgement was lost. Reconcile without creating rows.
                try:
                    record = self.store().lookup(descriptor['sessionId'], descriptor['operationId'])
                    if record is not None and record['requestDigest'] == descriptor['requestDigest']:
                        self.remember_record(record)
                except JournalError:
                    pass
            return None, self.failure(error, descriptor, not_started=bool(accepted and accepted['created']), remember=True)

    def start(self, descriptor, request, document):
        if (descriptor.get('sessionId') != self.session_id or descriptor.get('sessionId') != request.get('sessionId')
                or descriptor.get('operationId') != request.get('operationId') or descriptor.get('requestDigest') != fingerprint(request)):
            return {'success': False, 'sessionId': self.session_id, 'errorCode': 'operation_conflict',
                    'error': 'Dispatch identity does not match this session and canonical operation.'}
        try:
            transition = self.store().transition(descriptor['sessionId'], descriptor['operationId'], descriptor['requestDigest'],
                                                  'executing', dispatch_id=descriptor['dispatchId'], document=document)
            return None if transition['claimed'] else self.pending_reply(transition['record'])
        except JournalError as error:
            return self.failure(error, descriptor, remember=True)

    def finish(self, descriptor, result):
        try:
            transition = self.store().transition(descriptor['sessionId'], descriptor['operationId'], descriptor['requestDigest'],
                    'succeeded' if result['success'] else 'failed', dispatch_id=descriptor['dispatchId'], result=result)
            if not transition['claimed']:
                raise JournalError('operation_conflict', 'Terminal transition lost its executing claim; outcome requires reconciliation.')
            return self.terminal_reply(transition['record'])
        except JournalError as error:
            return self.failure(error, descriptor, observed=result)

    def not_executed(self, descriptor, result):
        if descriptor is None:
            return result
        if descriptor.get('sessionId') != self.session_id:
            return {'success': False, 'sessionId': self.session_id, 'errorCode': 'session_mismatch',
                    'error': 'A stale dispatch cannot update history from a previous session.', 'outcome': 'unknown'}
        result = {**result, 'sessionId': descriptor['sessionId'], 'operationId': descriptor['operationId'],
                  'nativeExecutionStarted': False}
        try:
            transition = self.store().transition(descriptor['sessionId'], descriptor['operationId'], descriptor['requestDigest'],
                                                  'not_executed', dispatch_id=descriptor['dispatchId'], result=result)
            if transition['claimed']:
                return self.terminal_reply(transition['record'])
            # A fast native handler may already have claimed/completed the event.
            # Delivery failure must never overwrite that execution evidence.
            return self.pending_reply(transition['record'])
        except JournalError as error:
            return self.failure(error, descriptor, remember=True)

    def status(self, args):
        session_id, operation_id = args.get('sessionId'), args.get('operationId')
        if not all(isinstance(value, str) and 0 < len(value) <= 128 for value in (session_id, operation_id)):
            return {'success': False, 'errorCode': 'invalid_input', 'error': 'Bounded sessionId and operationId strings are required.'}
        try:
            record = self.store().lookup(session_id, operation_id)
        except JournalError as error:
            retained = self.unconfirmed((session_id, operation_id))
            return {'success': False, 'sessionId': session_id, 'currentSessionId': self.session_id, 'operationId': operation_id,
                    'errorCode': error.code, 'error': str(error), 'outcome': 'unknown', 'reconciliationRequired': True,
                    'persistenceFailure': retained, 'durability': {'terminalRecorded': False},
                    'observedResult': retained.get('observedResult') if retained else None}
        if record is None:
            return {'success': False, 'sessionId': session_id, 'currentSessionId': self.session_id, 'operationId': operation_id,
                    'errorCode': 'operation_unknown', 'error': 'No durable record identifies this operation; no execution inference is made.', 'outcome': 'unknown'}
        historical = session_id != self.session_id
        terminal = record['phase'] in TERMINAL
        unconfirmed = self.unconfirmed((session_id, operation_id))
        state = ('succeeded' if record['phase'] == 'succeeded' else 'failed') if terminal else (
            'interrupted' if historical else 'reconciliation_required' if unconfirmed or record['phase'] == 'admitted' else 'running')
        return {'success': True, 'sessionId': session_id, 'currentSessionId': self.session_id, 'operationId': operation_id,
                'state': state, 'phase': record['phase'], 'historical': historical, 'history': self.metadata(record),
                'outcome': 'recorded' if terminal else 'unknown' if historical else unconfirmed['outcome'] if unconfirmed else 'unknown' if record['phase'] == 'admitted' else 'pending',
                'nativeExecutionStarted': unconfirmed.get('nativeExecutionStarted') if unconfirmed and not historical and not terminal else None,
                'reconciliationRequired': not terminal and (historical or unconfirmed is not None or record['phase'] == 'admitted'),
                'result': self.terminal_reply(record) if terminal else None,
                'observedResult': unconfirmed.get('observedResult') if unconfirmed and not terminal else None,
                'persistenceFailure': unconfirmed if unconfirmed and not terminal else None}
