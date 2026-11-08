"""Plain-data durable operation history. No Fusion API, replay or path arguments.

SQLite transactions use FULL synchronous commits and the rollback journal. The
database is bounded; the temporary rollback journal can require one additional
database-sized allocation. Unsupported/corrupt storage is never reset or migrated.
"""
import copy
import hashlib
import json
import math
import os
import re
from pathlib import Path
import stat
import sys
import threading
import time
import uuid

STORAGE_VERSION = 1
APPLICATION_ID = 0x41444D4D
MAX_RECORDS = 10000
MAX_DATABASE_BYTES = 64 * 1024 * 1024
MAX_RECORD_BYTES = 4 * 1024 * 1024
MAX_REQUEST_BYTES = 1024 * 1024
TERMINAL = frozenset(('succeeded', 'failed', 'not_executed'))
PHASES = frozenset(('admitted', 'dispatched', 'executing')) | TERMINAL


class JournalError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


class AdmissionConflict(JournalError):
    """Proof that this submitted digest was rejected before admission."""
    def __init__(self, session_id, operation_id, submitted_digest, existing_digest):
        super().__init__('operation_conflict', 'operationId was already used with different arguments.')
        self.session_id, self.operation_id = session_id, operation_id
        self.submitted_digest, self.existing_digest = submitted_digest, existing_digest


def require(condition, code, message):
    if not condition:
        raise JournalError(code, message)


def canonical(value, maximum=MAX_RECORD_BYTES):
    try:
        data = json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False).encode('utf-8')
    except (TypeError, ValueError, RecursionError) as error:
        raise JournalError('operation_record_invalid', 'Operation record is not canonical JSON: ' + str(error)) from error
    require(len(data) <= maximum, 'operation_record_too_large', 'Operation record exceeds its durable byte limit.')
    return data


def fingerprint(request):
    return hashlib.sha256(canonical(request, MAX_REQUEST_BYTES)).hexdigest()


def storage_root():
    if os.name == 'nt':
        base = os.environ.get('LOCALAPPDATA')
        require(bool(base), 'operation_storage_unavailable', 'LOCALAPPDATA is unavailable.')
        return Path(base) / 'Adom' / 'FusionBridge' / 'mechanical'
    if sys.platform == 'darwin':
        return Path.home() / 'Library' / 'Application Support' / 'Adom' / 'FusionBridge' / 'mechanical'
    return Path.home() / '.local' / 'state' / 'adom' / 'fusion-bridge' / 'mechanical'


class Journal:
    def __init__(self, directory=None, *, max_records=MAX_RECORDS, max_bytes=MAX_DATABASE_BYTES,
                 max_record_bytes=MAX_RECORD_BYTES):
        # Constructor injection is for offline tests; no public verb accepts it.
        self.directory = Path(directory) if directory is not None else storage_root()
        self.path = self.directory / 'operations-v1.sqlite3'
        self.max_records, self.max_bytes, self.max_record_bytes = max_records, max_bytes, max_record_bytes
        self.lock = threading.RLock()

    def _initialization_link(self, info):
        """Account for both links during exclusive initialization publication.

        A normal external hardlink is still refused: exactly two links must be
        accounted for by the public file and one regular, same-inode staging
        file inside this validated app-owned directory. No open staging writer
        remains when initialization publishes it.
        """
        if info.st_nlink != 2:
            return False
        matches = []
        for staged in self.directory.glob('.operations-v1-*.tmp'):
            if not re.fullmatch(r'\.operations-v1-[0-9a-f]{32}\.tmp', staged.name):
                continue
            try:
                other = staged.lstat()
            except FileNotFoundError:
                continue
            if (stat.S_ISREG(other.st_mode) and not (getattr(other, 'st_file_attributes', 0) & 0x400)
                    and (other.st_dev, other.st_ino) == (info.st_dev, info.st_ino)):
                matches.append(staged)
        # If the publisher has already unlinked, verify the final file's fresh
        # single-link state instead of rejecting the completed transition.
        if not matches:
            fresh = self.path.lstat()
            return (fresh.st_dev, fresh.st_ino) == (info.st_dev, info.st_ino) and fresh.st_nlink == 1
        return len(matches) == 1

    def _paths(self, create):
        require(self.directory.is_absolute() and '..' not in self.directory.parts,
                'operation_storage_invalid_path', 'Operation storage must be an absolute app-owned path.')
        for item in reversed((self.directory, *self.directory.parents)):
            if not item.exists() and not item.is_symlink():
                continue
            info = item.lstat()
            require(stat.S_ISDIR(info.st_mode) and not stat.S_ISLNK(info.st_mode)
                    and not (getattr(info, 'st_file_attributes', 0) & 0x400),
                    'operation_storage_invalid_path', 'Operation storage cannot traverse links, junctions or non-directories.')
        if create:
            self.directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        if self.directory.exists() and hasattr(os, 'getuid'):
            info = self.directory.stat()
            require(info.st_uid == os.getuid() and not (info.st_mode & 0o022),
                    'operation_storage_invalid_path', 'Operation storage must be owned by this user and not writable by others.')
        for suffix in ('', '-journal', '-wal', '-shm'):
            item = Path(str(self.path) + suffix)
            if not item.exists() and not item.is_symlink():
                continue
            info = item.lstat()
            links_safe = info.st_nlink == 1 or (not suffix and self._initialization_link(info))
            require(stat.S_ISREG(info.st_mode) and not stat.S_ISLNK(info.st_mode) and links_safe
                    and not (getattr(info, 'st_file_attributes', 0) & 0x400),
                    'operation_storage_invalid_path', 'Operation storage files cannot be links or special files.')

    def _connect(self, create):
        try:
            import sqlite3
        except ImportError as error:
            raise JournalError('operation_storage_unavailable', 'This Python runtime lacks SQLite support.') from error
        self._paths(create)
        exists = self.path.exists()
        if not exists and not create:
            return None
        if not exists:
            self._initialize(sqlite3)
        connection = sqlite3.connect(str(self.path) if create else self.path.as_uri() + '?mode=rw',
                                     uri=not create, timeout=2, isolation_level=None)
        try:
            # Existing-history reads may recover SQLite's hot rollback journal
            # after a process crash. mode=rw never creates missing storage.
            connection.execute('PRAGMA synchronous=FULL')
            app_id = connection.execute('PRAGMA application_id').fetchone()[0]
            version = connection.execute('PRAGMA user_version').fetchone()[0]
            require(app_id == APPLICATION_ID, 'operation_storage_corrupt', 'Storage is not an operation journal; it will not be reset.')
            require(version == STORAGE_VERSION, 'operation_storage_version', 'Unsupported operation journal version; automatic migration is disabled.')
            columns = connection.execute('PRAGMA table_info(operations)').fetchall()
            require([row[1] for row in columns] == ['session_id', 'operation_id', 'record_json', 'record_digest'],
                    'operation_storage_corrupt', 'Operation journal schema is invalid.')
            require(self.path.stat().st_size <= self.max_bytes, 'operation_storage_full', 'Operation journal exceeds its database byte limit.')
            if create:
                page_size = connection.execute('PRAGMA page_size').fetchone()[0]
                connection.execute('PRAGMA max_page_count=%d' % max(1, self.max_bytes // page_size))
                require(connection.execute('PRAGMA quick_check').fetchone()[0] == 'ok',
                        'operation_storage_corrupt', 'Operation database integrity check failed.')
            return connection
        except BaseException:
            connection.close()
            raise

    def _initialize(self, sqlite3):
        # Publish a fully initialized database exclusively. Concurrent first
        # callers must never observe a zero-byte file as corrupt or reset one.
        pending = list(self.directory.glob('.operations-v1-*.tmp'))
        require(len(pending) < 32, 'operation_storage_unavailable', 'Too many unfinished journal initialization files; retained for inspection.')
        staged = self.directory / ('.operations-v1-' + uuid.uuid4().hex + '.tmp')
        descriptor = os.open(staged, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        os.close(descriptor)
        connection = None
        try:
            connection = sqlite3.connect(str(staged), isolation_level=None)
            connection.execute('PRAGMA synchronous=FULL')
            connection.execute('BEGIN IMMEDIATE')
            connection.execute('CREATE TABLE operations (session_id TEXT NOT NULL, operation_id TEXT NOT NULL, record_json BLOB NOT NULL, record_digest TEXT NOT NULL, PRIMARY KEY(session_id,operation_id))')
            connection.execute('PRAGMA application_id=%d' % APPLICATION_ID)
            connection.execute('PRAGMA user_version=%d' % STORAGE_VERSION)
            connection.execute('COMMIT')
            connection.close(); connection = None
            # Windows os.fsync uses _commit, which requires a writable handle.
            # Reopen without truncation after SQLite has committed and closed;
            # a failed flush must still prevent publication and admission.
            with staged.open('r+b') as stream:
                os.fsync(stream.fileno())
            try:
                os.link(staged, self.path)
            except FileExistsError:
                pass  # A different initializer published its complete database.
            staged.unlink()
            if os.name != 'nt':
                descriptor = os.open(self.directory, os.O_RDONLY)
                try: os.fsync(descriptor)
                finally: os.close(descriptor)
        finally:
            if connection is not None:
                connection.close()
            if staged.exists():
                staged.unlink()

    def _decode(self, row, session_id, operation_id):
        if row is None:
            return None
        raw = bytes(row[0])
        require(len(raw) <= self.max_record_bytes and hashlib.sha256(raw).hexdigest() == row[1],
                'operation_storage_corrupt', 'Operation record digest or size is invalid.')
        try:
            record = json.loads(raw)
            valid = (record['storageVersion'] == STORAGE_VERSION and record['sessionId'] == session_id
                     and record['operationId'] == operation_id and record['phase'] in PHASES
                     and isinstance(record['requestDigest'], str) and len(record['requestDigest']) == 64
                     and isinstance(record['events'], list) and 1 <= len(record['events']) <= 6
                     and math.isfinite(record['createdAt']) and math.isfinite(record['updatedAt']))
            if record['phase'] in TERMINAL:
                valid = valid and isinstance(record['result'], dict) and type(record['result'].get('success')) is bool
                valid = valid and (record['result']['success'] == (record['phase'] == 'succeeded'))
            else:
                valid = valid and record['result'] is None
        except (ValueError, TypeError, KeyError) as error:
            raise JournalError('operation_storage_corrupt', 'Operation record is malformed.') from error
        require(valid and canonical(record, self.max_record_bytes) == raw,
                'operation_storage_corrupt', 'Operation record identity, phase or canonical encoding is invalid.')
        return record

    def _read(self, connection, session_id, operation_id):
        return self._decode(connection.execute('SELECT record_json,record_digest FROM operations WHERE session_id=? AND operation_id=?',
                                              (session_id, operation_id)).fetchone(), session_id, operation_id)

    def _save(self, connection, record):
        raw = canonical(record, self.max_record_bytes)
        record_digest = hashlib.sha256(raw).hexdigest()
        self._decode((raw, record_digest), record['sessionId'], record['operationId'])
        connection.execute('INSERT OR REPLACE INTO operations VALUES (?,?,?,?)',
                           (record['sessionId'], record['operationId'], raw, record_digest))

    def _run(self, create, action):
        with self.lock:
            connection = None
            try:
                connection = self._connect(create)
                if connection is None:
                    return None
                if create:
                    connection.execute('BEGIN IMMEDIATE')
                result = action(connection)
                if create:
                    connection.execute('COMMIT')
                return copy.deepcopy(result)
            except JournalError:
                raise
            except Exception as error:
                # Do not reset corrupt databases, retry a possibly committed
                # operation, or claim native effects were rolled back here.
                raise JournalError('operation_storage_failed', type(error).__name__ + ': ' + str(error)) from error
            finally:
                if connection is not None:
                    connection.close()

    def lookup(self, session_id, operation_id):
        return self._run(False, lambda connection: self._read(connection, session_id, operation_id))

    def admit(self, request, source_version):
        raw_request = canonical(request, MAX_REQUEST_BYTES)
        request = json.loads(raw_request)
        digest = hashlib.sha256(raw_request).hexdigest()
        require(isinstance(request, dict), 'operation_record_invalid', 'Operation request must be an object.')
        session_id, operation_id = request.get('sessionId'), request.get('operationId')
        require(all(isinstance(value, str) and 0 < len(value) <= 128 for value in (session_id, operation_id)),
                'operation_record_invalid', 'Session and operation IDs must be bounded strings.')
        require(isinstance(request.get('op'), str) and 0 < len(request['op']) <= 128
                and isinstance(source_version, str) and 0 < len(source_version) <= 128,
                'operation_record_invalid', 'Operation name and source version must be bounded strings.')
        def action(connection):
            for sid, oid, payload, checksum in connection.execute('SELECT session_id,operation_id,record_json,record_digest FROM operations'):
                self._decode((payload, checksum), sid, oid)
            existing = self._read(connection, session_id, operation_id)
            if existing:
                if existing['requestDigest'] != digest:
                    raise AdmissionConflict(session_id, operation_id, digest, existing['requestDigest'])
                return {'created': False, 'record': existing}
            count = connection.execute('SELECT COUNT(*) FROM operations').fetchone()[0]
            require(count < self.max_records, 'operation_capacity', 'Durable operation history is full; no records are evicted.')
            now = time.time()
            record = {'storageVersion': STORAGE_VERSION, 'sessionId': session_id, 'operationId': operation_id,
                      'operation': request['op'], 'sourceVersion': source_version, 'requestDigest': digest,
                      'requestedTarget': copy.deepcopy(request.get('target')), 'observedDocument': None,
                      'phase': 'admitted', 'createdAt': now, 'updatedAt': now,
                      'events': [{'phase': 'admitted', 'at': now}], 'dispatchId': None, 'result': None}
            self._save(connection, record)
            return {'created': True, 'record': record}
        return self._run(True, action)

    def transition(self, session_id, operation_id, digest, phase, *, dispatch_id=None, document=None, result=None):
        def action(connection):
            record = self._read(connection, session_id, operation_id)
            require(record is not None and record['requestDigest'] == digest, 'operation_conflict', 'Operation claim does not match its durable admission.')
            old = record['phase']
            allowed = {'dispatched': {'admitted'}, 'executing': {'dispatched'},
                       'not_executed': {'admitted', 'dispatched'}, 'succeeded': {'executing'}, 'failed': {'executing'}}
            require(phase in allowed, 'operation_record_invalid', 'Unsupported operation phase transition.')
            if old not in allowed[phase]:
                return {'claimed': False, 'record': record}
            if old == 'dispatched' or old == 'executing':
                require(dispatch_id == record['dispatchId'] and bool(dispatch_id), 'operation_conflict', 'Dispatch generation does not match the durable record.')
            if phase == 'dispatched':
                require(isinstance(dispatch_id, str) and 0 < len(dispatch_id) <= 128, 'operation_record_invalid', 'Dispatch ID is required.')
                record['dispatchId'] = dispatch_id
            if phase == 'executing':
                record['observedDocument'] = copy.deepcopy(document)
            if phase in TERMINAL:
                require(isinstance(result, dict) and type(result.get('success')) is bool and result['success'] == (phase == 'succeeded'),
                        'operation_record_invalid', 'Terminal phase contradicts its result.')
                record['result'] = copy.deepcopy(result)
            now = time.time(); record['phase'] = phase; record['updatedAt'] = now
            record['events'].append({'phase': phase, 'at': now})
            self._save(connection, record)
            return {'claimed': True, 'record': record}
        return self._run(True, action)
