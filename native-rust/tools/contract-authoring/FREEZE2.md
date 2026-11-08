# Contract authoring freeze 2

Freeze1 (20 exact files, manifest 9b6135f5e99cff5d217aba4eedb9b9237642139cfe664eecdc9766d252303b0c)
is preserved under evidence/freeze1-source. Root review identified that historical
reproduction alone omitted the legacy freezer's changed-catalog authoring role.

Freeze2 adds explicit refreeze: sorted expanded verbs determine whether the old
contract is kept verbatim or regenerated. Changed/absent prior contracts use the
canonical metadata and supplied Gregorian authoring date. All cases synchronize
bridge.json and emit matching catalog.json in a new atomic directory. None of the
three input files or original baseline files is replaced. A missing prior is
explicitly represented by `-`, never inferred from a failed file read.

Historical strict check/freeze and the complete baseline byte comparison remain.
Date selection is intentionally explicit rather than Python's implicit local
date.today(). Typed canonical validation, duplicate rejection, size bounds and
malformed-prior refusal intentionally reject Python's permissive coercions. An
unchanged contract retains its old metadata, even if canonical metadata changes,
matching the old freezer's verbs-only change decision. See README for scope.

Author self-review checked unchanged and changed metadata/date paths, schema and
row expansion order, first-time generation, manifest preservation, update output
self-consistency and fresh exclusive publication. No new runtime dependencies or
CAD behavior. Canonical baseline data and all six original source evidence files
are unchanged. No Python code was executed.

Validation: 18 library and 3 actual CLI-process tests PASS on Linux; GNU Windows
tests compile and formatting PASS. CLI authoring tests change a name and summary,
check the new date and sorted manifest, check the emitted catalog against the new
contract, prove idempotence despite a later supplied date, and reject invalid
dates/existing output without touching inputs. Existing malformed/duplicate/
schema mismatch/concurrent publication/partial-staging tests remain passing.

No actual Windows execution, host installation, release CI integration or bridge
runtime conversion is claimed. This replaces one authoring tool's behavior; the
full Python-to-Rust runtime migration remains separate.
