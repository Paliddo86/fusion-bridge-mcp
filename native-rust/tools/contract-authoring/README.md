# Rust contract authoring conversion R1

This isolated Rust CLI replaces the checked legacy `describe.py` table expansion
and runtime response construction with canonical JSON data. It also implements
checked contract freezing and bridge manifest verb synchronization. It does not
change a CAD handler, install a bridge, remove Python from an existing package,
or establish native mechanical runtime parity.

`data/catalog.json` is the ordered canonical table. It has explicit markers for
the mechanical schema, its ordered operation enum, and the `_STATUS` alias.
`data/mechanical-schema.json` is an exact copy of the 61-operation legacy schema;
its bytes and operation order are bound in the canonical table. These historical
schemas describe the legacy surface, not the smaller native implementation.

The current generated contract matches **all 665,236 bytes** of the checked
baseline: 179 sorted entries, every field, frozen metadata, key order and number
encoding. Runtime `describe` exposes 178 entries in original table order because
it filters the `_note` entry. It duplicates bridge/verbCount/verbs at the top
level and under data. The historical freezer does not filter `_note`; that
intentional legacy difference is preserved. Explicit null status stays null,
`_STATUS` expands to fusion_get_app_state, and false remains false.

## Use

Run from this directory (Cargo dependencies are already vendored locally):

```
cargo run --offline -- check data/catalog.json data/mechanical-schema.json evidence/baseline/describe.json
cargo run --offline -- describe data/catalog.json data/mechanical-schema.json
cargo run --offline -- freeze data/catalog.json data/mechanical-schema.json evidence/baseline/describe.json evidence/baseline/bridge.json /absolute/fresh-output-directory
cargo run --offline -- refreeze data/catalog.json data/mechanical-schema.json evidence/baseline/describe.json evidence/baseline/bridge.json 2026-10-16 /absolute/fresh-authoring-directory
```

`freeze` first checks complete equality against the explicitly supplied baseline,
then prepares `describe.json` and `bridge.json` in a private sibling directory.
It publishes the whole completed directory with exclusive rename, refusing even
an existing empty destination. It never edits the baseline, installed package,
user configuration or existing output. The manifest's other fields and ordering
remain intact; only its verbs list is synchronized when needed. If unchanged,
the entire original manifest bytes are retained.

`check` and `freeze` are strict historical identity modes: a divergent catalog
or different byte encoding refuses. `refreeze` is explicit authoring mode. It
compares sorted expanded verbs with the supplied prior contract. Unchanged verbs
preserve the entire old contract bytes/date/metadata, while still synchronizing
the manifest. Changed verbs regenerate the contract using canonical metadata and
the explicit supplied Gregorian date. Use `-` instead of a prior contract to
initialize one; an unreadable named prior file refuses. Both creation and update
publish a fresh atomic directory, never overwrite any input or installed file.

The refreeze bundle also includes catalog.json with its metadata synchronized to
the resulting contract, for repeatable subsequent authoring and checks. Schema
bytes remain the explicit separate input. A generated normal-format bundle passes
strict `check`; an unchanged prior contract with unusual byte layout is preserved
by refreeze but may correctly fail the strict historical encoding check.

The date must be supplied as YYYY-MM-DD (year 0001..9999, Gregorian leap rules).
There is no implicit local-clock read: callers choose the intended local authoring
date, and the date is only applied when verbs change. This is a deliberate policy
difference from Python's datetime.date.today(). Historical metadata
(`source:describe.py` and its old regeneration note) remains literal canonical
data for compatibility, not an instruction to execute Python. Canonical metadata
represents the old freezer's source literals. Editing those data is explicit
author intent; it cannot force unchanged verbs to rewrite prior metadata.
Release review and wiring into CI remain separate, unfinished integration work.

`extract LEGACY SCHEMA BASELINE NEW_DIRECTORY` is a one-time migration audit.
It accepts only the exact archived describe.py SHA, parses its bounded literal
table and the three known data expressions, and verifies the complete generated
contract before emitting catalog.json. It never imports, evaluates, executes or
launches Python. Normal check/describe/freeze commands use JSON only. Archived
`.py` files are historical evidence, not an execution dependency. Runtime logic
and freeze semantics were read independently in their actual source files.

## Provenance and checks

`evidence/BASELINE.json` records exact paths and hashes. The requested dev9
snapshot contains describe.py, contracts/describe.json, bridge.json, mechanical
schema and the old Rust contract loader. It has no tools/freeze-contract.py;
the actual freezer was read and archived separately from
exploded-preview-preparation/publication-oct7/published-readback. No claim is made
that those two snapshots share a source commit.

The first Rust encoding attempt passed whole-document semantic equality but
correctly refused exact publication because serde rendered two exponents as
`1e-6`, versus Python's `1e-06`. The rejected candidates/log remain under
`evidence/working`; the corrected encoder reproduces all current bytes. No
baseline discrepancy was normalized away. Number rendering is qualified against
this complete catalog and explicit exponent/Unicode cases, not asserted to be a
universal Python serializer replacement for arbitrary future floats.

Tests cover complete byte equality, reproducible literal extraction, every
runtime verb's order/status, note filtering, duplicated envelopes, all schema
keys/order, duplicate/malformed data, schema mismatch, bounded expansion,
manifest preservation, output collisions/concurrency and failure after staging
one file. Refreeze tests cover changed names/content, metadata/date refresh,
unchanged-byte preservation, absent prior initialization, leap dates, manifest
synchronization and self-consistent canonical output. CLI integration tests
exercise real processes, changed authoring, repeat idempotence and nonzero refusal
without output. Limits include 4MiB inputs/encoded files, bounded schema expansion,
serde's JSON recursion guard, strict duplicate rejection and no destination
replacement. Linux renameat2 and Windows MoveFileW implement exclusive directory
publication; other platforms refuse. Atomic visibility is tested on Linux; this
is not a power-loss durability or actual Windows execution claim.

Malformed-source tolerance intentionally differs from Python: canonical rows must
have exactly eight typed fields; the old freezer padded/truncated tuples, coerced
longRunning with bool(), and replaced falsy input/output/example with objects.
Rust refuses duplicate keys/verb names, invalid dates, wrong schema bytes/order,
and malformed prior contracts instead of silently coercing them. Unchanged prior
metadata must have the known four string fields. These restrictions do not alter
the checked current catalog. Arbitrary future floating-point spellings are not
claimed byte-identical to every Python json implementation; they remain finite
JSON with qualified current catalog and explicit encoding regression coverage.

Freeze1 is archived exactly at evidence/freeze1-source. Its reproduction-only
behavior was incomplete for authoring. Freeze2 adds explicit changed-catalog
refreezing; the canonical current data and original six baseline files remain
unchanged. No historical failed check/evidence was replaced.

No Python was run or newly authored. No Fusion/AB/host actions or runtime feature
additions were performed. Parent root is the independent reviewer; this author
previously built the native journal and several native ports, so this conversion
is not claimed to have fresh independent authorship.
