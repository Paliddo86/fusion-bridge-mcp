# Rust BOM core

This standalone crate ports the BOM business logic into Rust. It calculates flat
and structured BOMs from a typed assembly snapshot and generates CSV bytes.
There is no Python, SDK call, host operation, screenshot, runtime integration, or
file publication. **This is a tested pure computation port, not a completed
native Fusion BOM feature or evidence of SDK parity.**

`generate(&AssemblySnapshot, Options)` returns both purchasing rows and structured
instances. `csv_bytes(&Bom, CsvMode)` returns bounded UTF-8 CSV. The caller supplies
an already-allocated trusted adapter snapshot; this is not an untrusted JSON/HTTP
endpoint. A future transport adapter must enforce its wire budget before parsing.

The standalone workspace resolves dependencies offline from the existing
`../../rust-native-migration-r7/vendor` directory. No r7 source or dependency was
changed. `Cargo.lock` fixes the selected versions.

```
/home/adom/.cargo/bin/cargo test --offline
/home/adom/.cargo/bin/cargo fmt --all --check
```

The source currently has 29 integration tests. Fixtures have hand-computed
expected quantities and masses, exact CSV expectations, and adversarial
identity/provenance/budget cases. See `PORT-MAPPING.md` for preserved behavior,
intentional hardening, and the native work still required.

## Input and identity contracts

Component IDs are canonical session-local identities supplied by the native
adapter after SDK object-equality checks. Optional aliases are additional wrapper
identities already proven equivalent by that adapter. The calculator never
infers equivalence from names, part numbers, metadata, or entity tokens. Two
definitions with the same token text and part number remain separate unless an
adapter has explicitly supplied a nonambiguous alias mapping.

Occurrence nodes describe actual physical instances with unique IDs and full
paths, one containment parent, and context-specific direct-body observations.
Definitions may be reused; occurrence nodes may not. Root bodies are supplied on
the root component. Per-part unit properties use definition bodies; complete
physical totals use each occurrence's actual direct bodies. These are deliberately
different quantities and need not equal `quantity * unitMass` when context data
differs. A collapsed purchased row requires the separately observed native
component-subtree physical properties; the calculator does not invent those by
summing purchasing rows.

Every entity reference and configuration witness must match the snapshot target
and revision. `complete`, read errors, accuracy and supported configuration scope
are checked. They remain **claims supplied by the trusted adapter**, not evidence
that the pure crate contacted Fusion or verified an SDK object. Output
`complete: true` means complete computation over the supplied validated snapshot.

## Included behavior

- Definition-based flat quantities and pre-order structured instance rows,
  including containers and the root.
- Purchased collapse and `explodePurchased`, with native subtree unit properties.
- BOM exclusions prune purchasing subtrees while physical totals still include
  their unsuppressed bodies.
- Timeline suppression and effective suppression in the current ordinary
  configuration row omit physical instances. Suppression source matching honors
  exact native entity and optional occurrence-context identity.
- Visibility does not suppress parts or physical mass. Inactive/test rows do not
  change the active BOM. Non-occurrence suppression sources are reported but do
  not become occurrence filters; their actual geometry effect must already be
  present in native body observations.
- Supplier/source-file/hash/material metadata, target and current-row provenance.
  Supplier metadata does not certify modeled material density.
- High-accuracy mass in kg and volume in mm³, including a checked cm³-to-mm³
  adapter conversion helper and rejection of invalid or overflowing totals.
- Exact legacy CSV column order, structured quantity `1`, Python-compatible
  Boolean spelling, CRLF records, quoting, Unicode and formula neutralization.

## Work required before runtime acceptance

The native Rust/C++ adapter still must collect fresh data on Fusion's main thread,
resolve wrappers by SDK equality, obtain accurate body and subtree properties,
validate configuration parameter units/value types and source ownership, and
enforce document/revision guards. Full configuration snapshot collection is not
implemented here. Generated single-configuration documents, externally configured
inserted occurrences, custom/theme tables and hierarchical rows remain refused
for complete BOM generation, matching the legacy supported scope.

The `bom`/`bom_export` dispatch handlers, live fixtures, McMaster import/provenance
handoff, durable export admission, filesystem destination rules, same-filesystem
staging, atomic publication, overwrite behavior, rollback and cleanup warnings
are not integrated. CSV bytes alone do not establish export success. None of the
29 tests is a live SDK, archive, bridge transport, or public-release test.
