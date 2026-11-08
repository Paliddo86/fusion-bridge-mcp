# Port mapping

Reference: `exploded-preview-preparation/runtime-dev9-build-r1/source/addin/AdomBridge/commands/mechanical/`.

| Legacy responsibility | Rust implementation | Qualification |
| --- | --- | --- |
| `exporting.py::publish_staged` exclusive publication or replacement | `publish_bytes` private staging, hardlink/rename and immutable input digest | Linux/Windows filesystem tests |
| `bill_of_materials.py::bom_export` absolute `.csv`, UTF-8, no implicit overwrite | `publish_csv` | Linux/Windows filesystem tests |
| Staging cleanup must not hide committed output | Success receipt with separate cleanup warning | Both platforms, injected failures and unexpected child |
| File permissions inherited from arbitrary destination parent | Hardened private staging: Unix0700, Windows current-user protected DACL | Windows shared-parent ACL regression |
| Destination operation throws after invocation | Explicit `PublicationState::Unknown` | Fault-boundary regression; no remote-server lost-ack experiment |
| Native generation, configuration snapshot and model guard | Separate BOM core plus future SDK adapter | Not integrated |
| Mechanical journal admission/retention and `bom_export` dispatch | Future handler integration | Not implemented here |
| `export_design` STEP/STL/3MF/F3D/etc. SDK factories | Future typed native adapter | Not implemented here |
| General native exporters writing their own files or bundles | Separate future staging capability | Not implemented here |

The Rust primitive deliberately accepts bytes rather than an arbitrary staging
path. It supports the BOM output path without claiming all native exporter parity.
No Python code was added or executed.

Reviewer findings resolved before qualification: Windows inherited ACL exposure,
unbounded UTF-8 work before size refusal, reserved Windows filename variants, and
uncertain commit errors. See `rust-native-migration-r7/review/ARTIFACT-PUBLICATION-REVIEW.md`.
