# Retained observed results

`retained::RetainedJournal` wraps a registered `Journal` without changing its
storage format, filesystem checks, canonical request identity, durability rules
or underlying claims. It makes no Fusion calls. The root native integration is
a read-only operation-status pilot; no read request is journaled as a CAD write.

## Lifecycle

1. Construct `RetainedJournal::new(journal, RetentionLimits)` once per registered
   journal. Clone the wrapper to share authority, reservations and observations.
2. `admit(&PreparedRequest, at_ms)` reserves one slot and the full configured
   per-result byte budget before calling durable admission. Only
   `Admission::Created(AdmissionToken)` proceeds. `Existing(Status)` is evidence
   only, even if status is unknown or storage is unavailable.
3. `dispatch(AdmissionToken, dispatch_id, at_ms)` consumes admission authority and
   returns a `DispatchToken` only after durable dispatch succeeds.
4. `start(DispatchToken, observed_document, at_ms)` returns an `ExecutionPermit`
   only after the durable executing claim. The caller still verifies the correct
   native thread, target identity and revision before performing its one effect.
   This module does not enforce SDK thread affinity or perform a transaction.
5. After that invocation, call `finish(ExecutionPermit, observed_result,
   now_ms())`. Pass the clock's `Result<u64>` directly, without `?`: the wrapper
   retains the observation before examining a failed clock or persisting it.
6. A refusal before execution uses `not_executed(DispatchToken, result,
   now_ms())`; the result requires `success:false` and
   `nativeExecutionStarted:false`. An execution permit cannot use that method.

The three tokens are opaque and non-cloneable. Their private owner identity,
operation identity, digest, dispatch generation and expected lifecycle stage
must all agree. Dropping an unfinished token preserves its reservation and
marks local authority unknown. A token passed to a foreign wrapper is consumed
and conservatively abandoned in its original wrapper; neither durable journal
advances. Callers must not mix direct underlying Journal lifecycle calls with
this wrapper or manufacture authority from status fields.

`finish`/`not_executed` return `Completion { observation, persistence,
persistence_error }`. `Persistence::Committed` requires an acknowledged new
terminal commit. Clock, validation and persistence failures normally return
`Ok(Completion)` with `Persistence::Unconfirmed`, so callers cannot accidentally
lose the observed result by handling only the error branch. Ownership/lock
failure before retention can still return `Err`; no replay claim is returned.
The cache never retries terminal writes or native effects.

## Status and authority

`status(session, operation)` returns:

| Field | Meaning |
|---|---|
| `durable` | Successfully validated underlying Journal status, if readable. |
| `observed` | Process-local observation for this exact runtime/operation/digest/dispatch, if present. |
| `storage_error` | The current journal read failed. Absence of `durable` must not be treated as proof of missing work. |
| `retention_error` | Bounded diagnostic from the lifecycle or terminal persistence attempt. |
| `authority` | `DurableTerminal`, `ObservedNondurable`, `Pending`, or `Unknown`. |
| `historical` | The requested session differs from this runtime registration. |
| `reconciliation_required` | True for observed nondurable or unknown authority. |

A validated durable terminal record always has authority. A lost commit
acknowledgment may initially produce `Unconfirmed`, then status can show
`DurableTerminal` after readback. The earlier diagnostic and observation remain
visible; neither is an excuse to rewrite the record. An observed success Boolean
without a durable terminal record is never durable success or replay permission.
Storage errors remain visible rather than becoming trusted missing-record
evidence. An identity mismatch between cached evidence and a readable durable
record refuses status.

Observations are scoped to the current process and runtime. A fresh session can
read old durable terminal records; old nonterminal rows remain interrupted and
unknown. A previous process's nondurable observation cannot be reconstructed.

Native HTTP adapters must project only explicit error codes, never raw `Error`
messages, because journal diagnostics can contain paths. `Authority`,
`Observation`, `ObservedOutcome` and `Persistence` implement Serialize; `Status`
and `Completion` need deliberate projection of their error/Journal fields.

## Bounds and omission

Defaults are 64 slots, 256 KiB per result, and 16 MiB total reserved result bytes.
Hard maxima are 256 slots, 1 MiB per result, and 16 MiB total. Configured values
can lower these bounds. Every slot reserves its entire result allowance even
before a result exists; aggregate-byte exhaustion can therefore precede the
configured slot count. Terminal entries, abandoned tokens and failed admissions
are never silently evicted or released. Full capacity refuses admission before
any new durable admission or SDK authorization.

The cache stores serialized JSON in fixed-capacity byte buffers. A nonrecursive
preflight limits container depth to 64 and node count to the result-byte bound;
serialization itself stops at the byte bound. A result too large or too deeply
nested retains an explicit observation with `payload_omitted:true`, `result:None`
and the available `ObservedOutcome`. Its completion stays Unconfirmed and its
error states the omission reason. It is not a retained full result. A small
malformed terminal result retains its exact payload with unknown outcome and
unconfirmed persistence. No diagnostic substitutes a successful CAD result.

The byte budget covers retained serialized payload capacity, not exact allocator
overhead or caller-owned Values/status projections. Per-entry identity metadata
has the Journal's bounded IDs/digest plus fixed state; retained error messages
are copied into prefixes of at most 1,024 UTF-8 bytes. The caller constructs the
input Value and later drops it; this trusted in-process API does not claim to
safely construct or dispose of arbitrarily pathological recursive Values.

A separate process-wide table allows at most 256 live Journal registrations
with retention owners. It holds only Weak references, prunes dead registrations
on construction, and refuses a second wrapper over a live Journal clone even
after the first wrapper is dropped. This prevents capacity/authority reset by
rewrapping. The table does not keep journals alive. No global/cache mutex spans
SQLite/filesystem I/O or native work; observation insertion can proceed while
another thread is blocked reading durable status.

## Verification and boundaries

Focused builder tests cover successful durable reopen; before/after-commit
acknowledgment loss; failed/backward/invalid clocks; exact retention while the
database is unavailable; immutable duplicate/conflicting results; slot and
reserved-byte exhaustion before admission; concurrent duplicate admission;
historical unknown status; token abandonment/foreign ownership; wrapper
registration reuse; preexecution refusal; malformed/oversized/deep result
handling; and insertion while a status read waits on journal I/O.

These are file-backed Rust tests with synthetic results. Private journal fault
injection distinguishes precommit loss from postcommit acknowledgment loss.
The independent public-API verifier and root's Windows/live integration gates
remain separate evidence. The builder previously authored the standalone
journal and is not an independent reviewer of this retained layer.

No CAD effect, transaction rollback, public release compatibility, legacy
history migration, cache survival after process exit, automatic repair/retry,
power-failure guarantee or full historical-list endpoint is established here.
