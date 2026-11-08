# Rust mechanical operation journal

This crate implements durable admission and lifecycle history in Rust. It does
not invoke Fusion, dispatch native work, migrate legacy history, or implement a
mechanical CAD handler. Its database is `operations-rust-v1.sqlite3`, with a new
application ID, storage format and namespaced request fingerprint. It never
opens the Python implementation's `operations-v1.sqlite3`.

## Integration contract

1. Open one `Journal` per runtime using a fresh session ID and a trusted,
   application-owned absolute directory. `Journal::open` registers that session
   durably. A previously registered ID is rejected; `Clone` shares the same
   registration, serialization lock and uncertainty latch.
2. Pass a write `fusion_mechanical_core::PreparedRequest` to
   `admit(&request, now_ms)`. Only `Admission::Created(AdmissionClaim)` continues
   toward dispatch. `Existing(Record)` is status/replay evidence and must never
   enqueue another native event. A different request using the same operation ID
   is a conflict.
3. `dispatch(&admission, dispatch_id, now_ms)` returns
   `DispatchOutcome::Claimed(DispatchClaim)` only after committing the dispatch
   claim. A duplicate returns `Existing(Record)`.
4. On the correct native main thread, after verifying the target document,
   `start(&dispatch, observed_document_object, now_ms)` must return
   `Transition { claimed: true, .. }` **before** the handler starts native work.
   Duplicate events return `claimed: false` and must not execute.
5. Persist the observed result using `finish(&dispatch, result_object, now_ms)`.
   The result requires Boolean `success`; `claimed: true` proves the terminal
   state committed. A delivery refusal before execution can use
   `not_executed`, with `success:false` and `nativeExecutionStarted:false`.
   That path cannot overwrite an executing or terminal record.
6. On any uncertain acknowledgment, reconcile using `lookup` or `status`.
   Never retry native work. The journal latches the identity against further
   execution claims for the remainder of that runtime. A terminal record read
   successfully afterward is authoritative; a missing record is **not** proof
   that native execution did not happen.

Timestamps are integer milliseconds, explicitly passed to lifecycle methods;
`now_ms()` supplies the production clock. Values must fit signed SQLite integer
range and cannot move backward within an operation. A clock error after native
execution is an uncertain terminal persistence outcome.

## Durability and bounds

- SQLite rollback journal, `synchronous=FULL`, foreign keys, immediate write
  transactions, two-second lock wait. Claims are returned after commit only.
- Complete canonical record/request and digest validation, SQLite integrity
  checking, exact schema validation and source-session association on each
  operation. Unsupported, foreign or corrupt storage is refused, never reset.
- A read-only SQLite header preflight precedes writable SQLite access. Foreign
  hot rollback journals are refused without allowing SQLite to recover or alter
  their database or journal bytes.
- Initialization writes and flushes a private staged database, then publishes
  with an exclusive hard link. Existing files are never replaced. Unix directory
  metadata and newly created parent directory entries are also flushed before
  session registration.
- Defaults: 10,000 operations, 10,000 sessions, 64 MiB database, 4 MiB record,
  1 MiB request. Configurable limits can only lower those hard bounds. The
  rollback journal can require an additional database-sized allocation.
- No pruning, deletion, eviction or automatic retention. Full storage refuses
  admission while retaining terminal and nonterminal history.
- The uncertainty ledger is bounded by the operation limit. If uncertain failed
  admissions fill it, all further runtime claims are disabled; existing
  nonterminal status reports unknown. Missing rows never become a retry permit.
- Symlinks/reparse points and unaccounted hard links are refused. Unix storage
  must be owned by the effective user and not writable by group/others.
- Historical sessions remain readable; their nonterminal records report
  `Interrupted` and `Unknown`. They cannot be dispatched by a newer journal
  runtime. Fresh sessions are not a migration of old operation identities.

## Bounded observed-result retention (r6)

The optional `retained::RetainedJournal` layer reserves a slot and its full
result-byte allowance before durable admission. Opaque non-cloneable tokens
carry admission, dispatch and execution authority. `finish` retains bounded
observations before checking its clock result or attempting terminal storage;
failed persistence does not discard the observation or authorize replay.
Status distinguishes a validated durable terminal record from a process-local
observation with unconfirmed durability. See [RETENTION.md](RETENTION.md) for the
API, limits, omission behavior and integration obligations. This layer changes
no journal schema, validation, underlying claim type or legacy-file behavior.

## Windows storage boundary (r3)

Windows storage is limited to local fixed **NTFS** volumes with persistent ACLs
and hardlink support. The check uses the opened object's volume information and
GUID volume path, not a drive-letter guess. UNC, device, alternate-stream,
remote, removable, ReFS and other filesystem paths are unsupported. NTFS's
64-bit file index is the supported identity representation; no truncated ReFS
identity is accepted. See Microsoft's
[file-identity documentation](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/ns-fileapi-by_handle_file_information)
and [handle-based volume query](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumeinformationbyhandlew).

New missing directories and staging files receive their current-process-user
owner and private protected DACL **during creation**. Directories grant that
user full control with object/container inheritance; staging files grant that
user full control. SQLite-created rollback files inherit the private leaf ACL.
The runtime refuses impersonating threads and process tokens whose default
object owner differs from the token user, because SQLite's implicit sidecar
creation must also produce user-owned files. No token, privilege, UAC, ownership
or ACL adjustment is attempted. An elevated process with an alternate default
owner is deliberately unsupported.

Before existing data content is read or SQLite can recover it, metadata-only
handles opened without following reparse points validate current-user ownership,
filesystem identity and ACLs together. The private leaf requires a protected
DACL and an effective current-user full-control ACE that inherits to files and
subdirectories. Files can use inherited DACLs. Only simple allow ACEs for the
exact current-user SID, `LOCAL SYSTEM`, and `BUILTIN\\Administrators` are
accepted; the latter two are trusted OS administrators, not an isolation target.
Other principals are rejected even for read-only or zero access, as are absent,
null or empty DACLs, deny/conditional/object ACEs, inheritance-only/no-propagate
ACEs, and unsupported flags/rights. The policy is intentionally narrower than
Windows effective-access semantics. Existing ACLs and bytes are never repaired
or rewritten on refusal.

Ancestors are traversal infrastructure: their directory types and absence of
symlinks/junctions/reparse points are checked, but their owners and ACLs are not
required to match the private leaf policy. Missing ancestors created by this
crate are private from creation; existing ancestors remain unchanged. The caller
must choose trusted ancestors, since a party allowed to rename/delete children
of an ancestor can disrupt storage. This is not protection against an active
same-user process or a trusted system administrator changing paths or ACLs
between metadata checks and SQLite's pathname opens. Existing main-file,
hardlink, staging-alias and narrowly scoped transient-sidecar disappearance
guards remain in force; no effect retry or automatic cleanup was added.

## Verification and remaining work

The r2 baseline r9 passed 39 Rust tests on Linux and 38 on Windows MSVC. One Unix-only
permission/symlink test accounts for the difference. The counts include the
child-process worker used by the crash scenarios. A separate Windows campaign
passes all 20 rounds of eight concurrent initializers (160 total).
See [the baseline evidence](../../rust-native-migration-r2/evidence/qualification-r9.json).
The suite includes concurrent
initialization and duplicate claims, file-backed reopen, immutable terminal
records, before/after-commit acknowledgment loss, lost terminal persistence,
corrupt/foreign storage refusal, size/page/session/record bounds, path refusal,
deleted/replaced sidecar validation and stale staging-alias rejection,
and real child-process exits at five lifecycle/crash boundaries. One child exits
with an uncommitted SQLite transaction and a hot rollback journal; reopen proves
recovery preserves the preceding executing record without authorizing replay.

The r3 builder ran 42 Linux journal checks successfully, and
`cargo check -p fusion-mechanical-journal --tests --target x86_64-pc-windows-gnu --offline`
passed. Windows execution and independent adversary/verifier gates are still
pending; those results must not be inferred from cross-compilation. New tests
cover the portable ACL/filesystem policy plus real Win32 descriptor parsing,
private directory/staging creation, inherited file security and path refusal.
The parent task owns Windows behavioral receipts and final qualification.

Not qualified by those results:

- Broader Windows filesystem support is intentionally refused, including ReFS
  128-bit identities and remote/removable storage. No physical power-failure
  or media/controller flush qualification is claimed.
- Cross-process tampering by the same OS user; checksums detect corruption, not
  an adversary capable of replacing a private database and its digests.
- Fusion transport/main-thread event integration, CAD effects or transactional
  document rollback. No Fusion calls were made for this crate; root ran the
  isolated suite on the named Windows host.
- A legacy-history reader and public wire response parity with Python
  `OperationHistory`. No legacy rows, digests or timestamps are reinterpreted.
- The r6 retained layer preserves bounded process-local observations, but it
  does not persist that cache across crashes, recover omitted payloads, or prove
  native CAD integration. Its tests use synthetic observed results; they are
  not Fusion execution evidence or Python wire/history parity.
- Throughput/latency at the 10,000-record bound. Validation currently scans and
  hashes the entire bounded database deliberately; main-thread cost needs a
  measured integration decision before release.
- Multi-week endurance, disk power-loss guarantees beyond SQLite/OS flush
  contracts, native failure rollback or published release suitability.

## Build notes — 2026-10-13

The checkpoint uses supported-filesystem enforcement instead of broadening file
identity to 128 bits: accepting ReFS would also require qualifying SQLite,
publication and directory durability on that filesystem. The runtime's explicit
NTFS scope is safer and testable now. The Windows APIs come from vendored
`windows-sys` 0.61.2 and `windows-link` 0.2.1. The r3 vendor tree is a separate
copy; r2 dependencies are unchanged. The default-token-owner precondition was
added because user ownership on the explicitly created leaf/staging objects
alone does not establish ownership on files implicitly created by SQLite.
