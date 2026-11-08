"""Read-only access to the durable operation journal the bridge exe writes.

The exe (rust/bridge/src/mech_store.rs) is the only writer: it admits, dispatches and records every
write. Exploded previews read terminal records here as proof of earlier operations in this session.
Every read re-validates digest, identity, phase and canonical encoding, as the writer does, and
nothing here creates, migrates or resets storage.
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

STORAGE_VERSION = 1
APPLICATION_ID = 0x41444D4D
MAX_DATABASE_BYTES = 64 * 1024 * 1024
MAX_RECORD_BYTES = 4 * 1024 * 1024
MAX_REQUEST_BYTES = 1024 * 1024
TERMINAL = frozenset(('succeeded', 'failed', 'not_executed'))
PHASES = frozenset(('admitted', 'dispatched', 'executing')) | TERMINAL


class JournalError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


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
    def __init__(self, directory=None, *, max_bytes=MAX_DATABASE_BYTES, max_record_bytes=MAX_RECORD_BYTES):
        # Constructor injection is for offline tests; no public verb accepts it.
        self.directory = Path(directory) if directory is not None else storage_root()
        self.path = self.directory / 'operations-v1.sqlite3'
        self.max_bytes, self.max_record_bytes = max_bytes, max_record_bytes
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
        if not self.path.exists():
            return None
        connection = sqlite3.connect(self.path.as_uri() + '?mode=rw', uri=True, timeout=2, isolation_level=None)
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
            return connection
        except BaseException:
            connection.close()
            raise

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

    def _run(self, create, action):
        with self.lock:
            connection = None
            try:
                connection = self._connect(create)
                if connection is None:
                    return None
                return copy.deepcopy(action(connection))
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
