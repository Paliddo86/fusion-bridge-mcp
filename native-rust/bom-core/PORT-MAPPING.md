# Legacy-to-Rust BOM mapping

Legacy reference root:
`exploded-preview-preparation/runtime-dev9-build-r1/source/addin/AdomBridge/commands/mechanical/`

| Legacy source | SHA-256 |
|---|---|
| `bill_of_materials.py` | `960c010226ab1ce21515b59c18528abe34146119be5383600d7d1d6076a0d7b0` |
| `assembly_state.py` | `eb53be1b5011873719e3e9d1e382696f5e4401441962b88c4486de2d3d251d1e` |
| `configurations.py` | `f99276ad97639d5540beb642bb3d4416fb77cf24c9345594d3437e7e2ebe70d0` |

The files were read as reference text; no legacy Python was executed.

| Legacy responsibility | Rust implementation | Status and boundary |
|---|---|---|
| `generate`: `parts` keyed by `identity(comp)` | `engine::Graph` canonical-ID/alias index and `Run::purchasing` | Implemented. SDK equality and alias proof remain native adapter duties. |
| Metadata JSON attributes | `model::Metadata::from_attributes` | Implemented for supplier, supplier part number, URL, file, SHA, purchase/exclude flags, raw attributes and material. |
| Purchasing recursion and root | `Run::purchasing` | Implemented, preserves pre-order, quantities, parent paths, depths, direct body/child counts and references. |
| Purchased collapse/explode | `Run::purchasing` | Implemented. A collapsed row requires explicit native subtree properties. |
| BOM exclusions | `Run::purchasing`, independent `physical_walk` | Implemented. Excluded descendants stay out of purchasing; physical mass remains. |
| Timeline/configuration suppression | `Run::suppression` | Implemented: timeline first; active effective suppression; entity ID plus optional context match. |
| Visibility | Stored on occurrence, deliberately not an inclusion filter | Preserved: hidden unsuppressed bodies remain in both applicable inventories. |
| Physical recursion | `Run::physical_walk` | Implemented using context-specific direct bodies, including root, independent of purchase flags/exclusions. |
| Shared 20,000 visits / depth 100 / ancestry cycle guard | `Limits`, `Run::visit` | Implemented; budget is shared between both traversals. Limits can only tighten defaults. |
| High native physical properties / cm³ conversion | `PhysicalProperties` and explicit body/subtree inputs | Arithmetic and validation implemented. Native measurements remain unimplemented adapter work. |
| Strict configuration snapshot | `ConfigurationWitness::validate` | Complete/source/scope checks and table address matrix checks implemented. Native table acquisition and complete numeric/property source validation remain adapter dependencies. |
| Configuration provenance | `ConfigurationProvenance` | Implemented; document/revision and active ordinary table/row preserved in CSV. |
| CSV flat/structured formatting | `csv_bytes` | Implemented. Exact column order, row meaning, quoting and CRLF preserved. |
| `bom` native handler and `target()` | None | Not ported/integrated by this crate. |
| `bom_export` destination checks / staging / `publish_staged` | None | Not ported. The API returns bytes only and performs no filesystem operation. |

## Deliberate hardening and representation differences

1. Inputs are typed graph data, not Fusion SDK objects. References retain the
   legacy token/path fields; tokens are opaque and never used to group definitions.
2. Recognized attribute types are strict strings/Booleans. Legacy `json.loads`
   could accept a malformed non-Boolean purchase flag and then use its truthiness.
   Rust rejects those values and rejects disagreement between decoded attributes
   and duplicated typed metadata fields.
3. Finite, nonnegative mass/volume and finite totals are required. Failed source
   or configuration reads cannot masquerade as `None` suppression or zero mass.
   `timeline_suppressed: None` specifically means a successfully observed absence
   of a timeline object.
4. The input graph requires unique occurrence IDs/paths, exactly one parent,
   complete reachability, matching reference targets/paths and direct-body counts.
   Malformed graph data is refused even in branches purchasing might collapse or
   exclude. These safeguards are stricter than the legacy on-demand traversal.
5. Additional limits: 20,000 definition/alias identities and occurrence nodes,
   20,000 direct bodies per definition, 200,000 physical body instances, 32 MiB
   serialized input/output and 16 MiB CSV. Repeated metadata is charged before
   cloning output rows; output size is also checked exactly after calculation.
   Conservative row overhead can reject before the exact byte ceiling.
6. Configuration tables require a complete, nonduplicated row×column cell matrix,
   matching kinds/scopes, a consistent ordinary active row, and at most one
   reserved test row. Configured inserted occurrences and generated configuration
   documents are still explicitly unsupported, even if `complete` was mislabeled.
   Parameter expressions are retained only as address witnesses; the adapter must
   separately validate the legacy full parameter numeric/unit readback contract.
   Occurrence suppression sources additionally reconcile exact SDK type,
   component owner scope, canonical owner identity, and observed entity/context
   witnesses. Proven owner aliases and absent-context fanout remain supported.
   Relabeling an observed occurrence identity as another SDK type is refused.
   Design-owned sources must be UserParameters of the distinct Design SDK
   identity in the source witness, not merely a matching document ID.
7. CSV prefixes formula-like textual cells with an apostrophe. In addition to
   legacy leading `=`, `+`, `-`, `@`, leading whitespace/BOM bypasses and leading
   control characters are neutralized. Numeric fields remain numbers; names with
   ordinary internal punctuation are unchanged. Float spelling follows Rust's
   shortest-roundtrip serializer and may differ from Python in exponent notation.
8. No string or object in a snapshot proves live SDK validity. The library does
   not establish native parity, archive roundtrip, dispatcher authorization,
   operation-journal semantics, successful publication or release readiness.

## Independent fixture arithmetic

The main fixture has root mass 1 kg, two purchased kits each containing a 2 kg
direct body plus two 1.5 kg bolts, an excluded housing of 4 kg containing another
1.5 kg bolt, one suppressed kit, and one hidden unsuppressed 1.5 kg bolt.

Physical mass is `1 + 2×(2 + 1.5 + 1.5) + (4 + 1.5) + 1.5 = 18 kg`, with ten
direct-body instances. Collapsed purchasing has root×1, kit×2, bolt×1. Exploded
purchasing has root×1, kit×2, bolt×5, with kit unit mass changing from 5 kg subtree
to 2 kg direct bodies. Both physical results remain 18 kg. Additional fixtures
separate equal metadata/tokens from definition identity, reuse proven aliases,
override physical properties per occurrence context, and exercise source and
configuration failure boundaries.
