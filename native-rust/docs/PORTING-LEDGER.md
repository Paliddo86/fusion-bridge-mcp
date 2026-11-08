# Rust migration parity ledger

This is the r7 starting ledger, dated 2026-10-14, derived from all 61 mechanical contracts, the legacy dispatcher/handler source, the host verb catalog and every `.py` file in the supplied source snapshot. Qualification is frozen at r6; work in progress earns no implementation, Fusion or release checkmark. Regenerate with the Rust inventory tool described below.

The verifier previously authored native document identity and Windows storage hardening, but did not author r7 mechanical/HTTP effect integration. This is reused structural role separation, not fresh-context or model-diversity independence.

| Inventory | Count |
|---|---:|
| Mechanical contracts | 61 |
| Unconditional writes / reads / conditional health | 46 / 14 / 1 |
| Native mechanical handlers qualified at frozen r6 | 0 |
| Add-in dispatcher entries (includes mechanical router/status/capabilities) | 65 |
| Python host dispatcher entries | 50 |
| Full host catalog verbs | 179 |
| Python files in supplied snapshot | 60 |
| Public native replacement releases | 0 |

The default host executable is already Rust. The 60 source files break down into **47 current Python add-in files, 10 embedded runtime templates, two catalog/packaging tools, and one retired host source/reference copy**. The nine missing imports of that retired host are not nine live runtime requirements. Existing Rust replacements are credited below; their tests and actual-host behavior are not rerun by this source inventory.

## Meaning of the gates

`C` means the Rust schema exists and the frozen r6 Linux/Windows contract-inventory tests passed as a group. It does not mean an SDK handler exists or every variant was exercised. `—` means not qualified at this baseline, including absent implementation or pending evidence. `P` means the first r7 slice is in progress, with no parity credit yet. Legacy handler presence is source evidence, not a new live test. The machine-readable ledger separates Linux and Windows unit/external status for every operation.

The r6 native document list is not mechanical `inspect`. r6 qualified authenticated history delivery, read dispatch, retained document identity and bounded journal/result storage; it did not connect CAD effects. See [frozen r6 status](../../rust-native-migration-r6/RUST-STATUS.md). No old schema, test count or DLL build is credited as actual-app validation of an r7 effect.

## Mechanical operations

| Operation | Access | Legacy handler | Rust contract | Rust parity | Linux handler unit / external | Windows handler unit / external | Fusion | Public release |
|---|---|---|---|---|---|---|---|---|
| `assembly_properties` | read | `assemblies.py::assembly_properties`:395 | C | — | — / — | — / — | — | — |
| `bom` | read | `bill_of_materials.py::bom`:98 | C | — | — / — | — / — | — | — |
| `bom_export` | write | `bill_of_materials.py::bom_export`:103 | C | — | — / — | — / — | — | — |
| `cad_import` | write | `purchased.py::cad_import`:40 | C | — | — / — | — / — | — | — |
| `component_create` | write | `assemblies.py::component_create`:131 | C | — | — / — | — / — | — | — |
| `component_metadata` | write | `assemblies.py::component_metadata`:204 | C | — | — / — | — / — | — | — |
| `configuration_inspect` | read | `configurations.py::inspect`:320 | C | — | — / — | — / — | — | — |
| `construction_axis` | write | `geometry.py::construction_axis`:250 | C | — | — / — | — / — | — | — |
| `entity_delete` | write | `features.py::entity_delete`:198 | C | — | — / — | — / — | — | — |
| `exploded_preview_create` | write | `exploded_preview.py::create`:463 | C | — | — / — | — / — | — | — |
| `exploded_preview_dispose` | write | `exploded_preview.py::dispose`:475 | C | — | — / — | — / — | — | — |
| `exploded_preview_inspect` | read | `exploded_preview.py::inspect`:479 | C | — | — / — | — / — | — | — |
| `exploded_preview_sample` | write | `exploded_preview.py::sample`:467 | C | — | — / — | — / — | — | — |
| `exploded_preview_show` | write | `exploded_preview.py::show`:471 | C | — | — / — | — / — | — | — |
| `exploded_view_plan` | read | `exploded_view.py::exploded_view_plan`:184 | C | — | — / — | — / — | — | — |
| `export` | write | `operations.py::export`:226 | C | — | — / — | — / — | — | — |
| `feature_create` | write | `features.py::feature_create`:43 | C | — | — / — | — / — | — | — |
| `feature_edit` | write | `features.py::feature_edit`:181 | C | — | — / — | — / — | — | — |
| `geometry_query` | read | `geometry.py::geometry_query`:195 | C | — | — / — | — / — | — | — |
| `health` | read_or_recompute_write | `operations.py::check_health`:220 | C | — | — / — | — / — | — | — |
| `inspect` | read | `operations.py::inspect`:13 | C | P | — / — | — / — | — | — |
| `inspect_entity` | read | `operations.py::inspect_entity`:58 | C | — | — / — | — / — | — | — |
| `interference` | read | `assemblies.py::interference`:410 | C | — | — / — | — / — | — | — |
| `joint_create` | write | `assemblies.py::joint_create`:320 | C | — | — / — | — / — | — | — |
| `joint_delete` | write | `assemblies.py::joint_delete`:357 | C | — | — / — | — / — | — | — |
| `joint_edit` | write | `assemblies.py::joint_edit`:346 | C | — | — / — | — / — | — | — |
| `joint_origin_create` | write | `regular_joints.py::joint_origin_create`:438 | C | — | — / — | — / — | — | — |
| `joint_origin_delete` | write | `regular_joints.py::joint_origin_delete`:480 | C | — | — / — | — / — | — | — |
| `joint_origin_edit` | write | `regular_joints.py::joint_origin_edit`:457 | C | — | — / — | — / — | — | — |
| `material_assign` | write | `purchased.py::material_assign`:116 | C | — | — / — | — / — | — | — |
| `material_list` | read | `purchased.py::material_list`:101 | C | — | — / — | — / — | — | — |
| `mcmaster_discover` | read | `purchased.py::mcmaster_discover`:81 | C | — | — / — | — / — | — | — |
| `mcmaster_open` | write | `purchased.py::mcmaster_open`:90 | C | — | — / — | — / — | — | — |
| `measure` | read | `geometry.py::measure`:107 | C | — | — / — | — / — | — | — |
| `motion_link_create` | write | `motion_links.py::motion_link_create`:98 | C | — | — / — | — / — | — | — |
| `motion_link_delete` | write | `motion_links.py::motion_link_delete`:148 | C | — | — / — | — / — | — | — |
| `motion_link_edit` | write | `motion_links.py::motion_link_edit`:117 | C | — | — / — | — / — | — | — |
| `new_design` | write | `operations.py::new_design`:97 | C | P | — / — | — / — | — | — |
| `occurrence_delete` | write | `assemblies.py::occurrence_delete`:188 | C | — | — / — | — / — | — | — |
| `occurrence_insert` | write | `assemblies.py::occurrence_insert`:144 | C | — | — / — | — / — | — | — |
| `occurrence_update` | write | `assemblies.py::occurrence_update`:160 | C | — | — / — | — / — | — | — |
| `parameter_batch` | write | `features.py::parameter_batch`:213 | C | P | — / — | — / — | — | — |
| `parameter_create` | write | `operations.py::parameter_create`:192 | C | P | — / — | — / — | — | — |
| `parameter_set` | write | `operations.py::parameter_set`:166 | C | P | — / — | — / — | — | — |
| `plane_create` | write | `sketches.py::plane_create`:219 | C | — | — / — | — / — | — | — |
| `regular_joint_create` | write | `regular_joints.py::regular_joint_create`:349 | C | — | — / — | — / — | — | — |
| `regular_joint_edit` | write | `regular_joints.py::regular_joint_edit`:377 | C | — | — / — | — / — | — | — |
| `render_scene_inspect` | read | `render_scene.py::render_scene_inspect`:318 | C | — | — / — | — / — | — | — |
| `rigid_group_create` | write | `assemblies.py::rigid_group_create`:364 | C | — | — / — | — / — | — | — |
| `rigid_group_delete` | write | `assemblies.py::rigid_group_delete`:388 | C | — | — / — | — / — | — | — |
| `rigid_group_edit` | write | `assemblies.py::rigid_group_edit`:374 | C | — | — / — | — / — | — | — |
| `sketch_constraint` | write | `sketches.py::sketch_constraint`:146 | C | — | — / — | — / — | — | — |
| `sketch_create` | write | `sketches.py::sketch_create`:75 | C | — | — / — | — / — | — | — |
| `sketch_curve_edit` | write | `sketch_edits.py::sketch_curve_edit`:30 | C | — | — / — | — / — | — | — |
| `sketch_diagnostics` | read | `sketch_edits.py::sketch_diagnostics`:130 | C | — | — / — | — / — | — | — |
| `sketch_dimension` | write | `sketches.py::sketch_dimension`:166 | C | — | — / — | — / — | — | — |
| `sketch_edit` | write | `sketches.py::sketch_edit`:200 | C | — | — / — | — / — | — | — |
| `sketch_geometry` | write | `sketches.py::sketch_geometry`:82 | C | — | — / — | — / — | — | — |
| `sketch_offset` | write | `sketch_edits.py::sketch_offset`:94 | C | — | — / — | — / — | — | — |
| `sketch_project` | write | `sketches.py::sketch_project`:190 | C | — | — / — | — / — | — | — |
| `transaction_probe` | write | `transactions.py::probe`:41 | C | — | — / — | — / — | — | — |

### Source behavior and limits

- **assembly_properties**: Assembly physical properties and native units/readback.
- **bom**: Definition identity, nested instance quantities, configuration suppression and purchased collapse; physical totals remain independent of purchasing exclusions.
- **bom_export**: Flat or structured CSV, configuration provenance, spreadsheet formula neutralization and staged publication; file effect is not model rollback.
- **cad_import**: Bounded local STEP/SAT import into a wrapper component, hash/provenance validation, body and document-set readback; no download or cloud upload.
- **component_create**: Create component occurrence with metadata and verified placement/context.
- **component_metadata**: Edit component definition purchasing/identity metadata with readback.
- **configuration_inspect**: Read configuration table, active row, typed cells, source links and explicit completeness; no configuration activation or editing.
- **construction_axis**: Create supported construction axis with guarded source geometry and readback.
- **entity_delete**: Delete an allowed sketch/curve/point/plane/feature reference with transaction handling; not arbitrary object deletion.
- **exploded_preview_create**: Reserve finite session-owned custom graphics in an owned blank presentation design from a source-bound plan and durable creation proof.
- **exploded_preview_dispose**: Dispose only the controller's owned graphics/lease; history does not authorize adopting live resources after restart.
- **exploded_preview_inspect**: Inspect the current controller capability, ownership and live resources; not reconstructed from terminal history.
- **exploded_preview_sample**: Apply a finite sampled pose to owned presentation graphics; source model is not moved.
- **exploded_preview_show**: Show owned presentation resources with controller ownership checks; no unbounded playback timer.
- **exploded_view_plan**: Read-only bounded translation plan with exact instance hierarchy, local/parent/world frames and source/configuration binding; no native object movement.
- **export**: Guarded model export delegated to exporting.py with staged file publication and explicit format support.
- **feature_create**: Extrude, revolve, fillet, chamfer, shell, hole, loft, sweep, combine, split, move, mirror, rectangular/circular pattern, draft and thread variants.
- **feature_edit**: Edit supported existing feature parameters/properties and verify health/readback.
- **geometry_query**: Query faces/edges/vertices using typed filters and explicit coordinate contexts; no guessed name targeting.
- **health**: Read feature health; recompute:true is a journaled write and requires current target/session/operation identity.
- **inspect**: Inspect active Design entities, parameters, transforms, health, joints and configuration; r6 document-list inventory is not this operation.
- **inspect_entity**: Exact guarded entity resolution with type-specific readback.
- **interference**: Native assembly interference query with validated entity context.
- **joint_create**: Create supported as-built joint, motion/limits and attachment readback.
- **joint_delete**: Delete exact joint reference with transaction handling.
- **joint_edit**: Edit as-built joint motion/limits and verify resulting state.
- **joint_origin_create**: Create joint origin with guarded attachment/frame selectors and parameter readback.
- **joint_origin_delete**: Delete exact joint-origin reference.
- **joint_origin_edit**: Edit joint-origin attachment/axes/parameters and verify.
- **material_assign**: Assign exact material ID from selected library/design to native component/body definition; verify ID, not display name.
- **material_list**: Bounded material/library discovery with query, matching count and completeness.
- **mcmaster_discover**: Discover Fusion's InsertMcMasterCarrComponentCommand and requirements; not supplier search or purchasing.
- **mcmaster_open**: Launch native interactive McMaster command in a guarded hybrid design; launch acknowledgment is not insertion completion, transaction or unattended download.
- **measure**: Native distance/angle measurement with supported geometry and witness points/contexts.
- **motion_link_create**: Create native motion link between guarded supported joint motions.
- **motion_link_delete**: Delete exact motion-link reference.
- **motion_link_edit**: Edit motion-link parameters and verify exact native readback.
- **new_design**: Create unnamed unsaved parametric hybrid Design, optional parameter-driven box; requested name is metadata. Failure cleanup discards only the newly created unsaved document.
- **occurrence_delete**: Delete an exact occurrence without conflating component definitions or sibling instances.
- **occurrence_insert**: Insert another instance of an existing component with verified transform/context.
- **occurrence_update**: Update occurrence placement/visibility/grounding with state and transform checks.
- **parameter_batch**: Grouped parameter create/set/delete variants with transaction/recompute/readback; deletion is limited to deletable user parameters. No generic atomic-batch claim for all operations.
- **parameter_create**: Create a typed user parameter, recompute, verify health and compensate failure with a recovery witness.
- **parameter_set**: Set parameter expression, recompute, verify health and restore old expression on failure with semantic recovery readback.
- **plane_create**: Create supported construction plane from guarded selector and offset.
- **regular_joint_create**: Create regular joint from exact attachments, alignment/motion/lock settings; verify frames and placements.
- **regular_joint_edit**: Edit regular joint parameters/motion/lock with attachment and pose validation.
- **render_scene_inspect**: Bounded scene/environment observation and detached camera preparation; never starts render, writes scene settings or proves complete revision-to-pixels identity.
- **rigid_group_create**: Create rigid group over exact occurrences with native membership readback.
- **rigid_group_delete**: Delete an exact rigid-group reference.
- **rigid_group_edit**: Edit rigid group membership/settings with native readback.
- **sketch_constraint**: Create supported geometric constraint on exact sketch entities.
- **sketch_create**: Create sketch on guarded plane/component selector.
- **sketch_curve_edit**: Guarded trim/extend/break/split support with native topology/readback checks; full trim deletion needs explicit permission.
- **sketch_diagnostics**: Inspect sketch constraint/degree-of-freedom and profile diagnostics without mutation.
- **sketch_dimension**: Create distance/horizontal/vertical/angle/radius/diameter dimensions with expression/unit validation.
- **sketch_edit**: Edit supported sketch state/geometry selection with transaction/recovery behavior.
- **sketch_geometry**: Point, line, rectangle, circle, arc, polygon, slot and fitted spline primitives with input/unit checks.
- **sketch_offset**: Offset guarded sketch curves using native operation and verify resulting entities.
- **sketch_project**: Project exact supported source entities into a guarded sketch and return references.
- **transaction_probe**: Qualify available native transaction behavior; does not promise generic batch atomicity.

The JSON contains each schema's required fields, property names and enum variants. Each variant remains a separate qualification obligation. Root reports the first `parameter_set` implementation initially lacks legacy compensation/recovery; a successful set/readback cannot establish that missing parity. `new_design` with a blank fixture also cannot qualify its optional parameter-driven box.

## Remaining Python: runtime, embedded code and tools

All paths below are relative to `exploded-preview-preparation/runtime-dev9-build-r1/source`. The JSON binds each file's SHA-256, size and line count. No `.py` file is excluded as a test or build artifact. No standalone Python test files were found in this snapshot.

| Category | Files | Required migration |
|---|---:|---|
| `addin_runtime_lifecycle` | 2 | Complete native bootstrap/transport/event/lifecycle replacement; r6 infrastructure is a qualified subset, not installed replacement. |
| `contract_authoring_tool` | 1 | Runtime catalog is already embedded JSON in Rust. Port catalog generation/validation tooling and preserve the contract; no interpreter is needed by the existing runtime catalog path. |
| `development_packaging` | 1 | Replace package assembly, embedded-contract validation and manifest hashing with Rust tooling; separately qualify installer/signing/release. |
| `dynamic_python_execution` | 1 | Define a typed Rust operation/script replacement or explicitly approved compatibility break; arbitrary Python execution cannot silently remain in a Python-free product. |
| `embedded_runtime_python` | 10 | Replace emitted/executed Python with typed Rust SDK/host operations; moving string generation into Rust is not elimination. |
| `mechanical_runtime` | 23 | Port handler/helper policy and thin SDK bindings, preserving guards, units, ownership, readback and truthful recovery; qualify operation variants independently. |
| `mechanical_runtime_backend` | 1 | Port active custom-graphics ownership/backend; despite prototype filename it is imported by the public preview controller. |
| `nonmechanical_addin_runtime` | 20 | Port or explicitly retire the mapped nonmechanical command under documented compatibility policy; use exact ownership and independent readback. |
| `retired_host_reference_optional_fallback` | 1 | Default host is already Rust. Preserve behavior evidence, retire this development/reference copy and optional rollback branch; do not recreate its missing historical Python imports. |

| Python file | Category | r7 parity |
|---|---|---|
| `addin/AdomBridge/AdomBridge.py` | `addin_runtime_lifecycle` | — |
| `addin/AdomBridge/commands/__init__.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/activity_palette.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/app_state.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/assembly_bom.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/close_document.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/cloud_documents.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/design_info.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/document_hygiene.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/document_info.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/electron_view.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/electronics.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/export.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/import_file.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/manufacturing.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/mechanical/__init__.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/assemblies.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/assembly_state.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/bill_of_materials.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/configurations.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/core.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/exploded_preview.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/exploded_preview_prototype.py` | `mechanical_runtime_backend` | — |
| `addin/AdomBridge/commands/mechanical/exploded_view.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/exporting.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/features.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/geometry.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/joint_state.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/journal.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/motion_links.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/operation_history.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/operations.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/purchased.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/readback.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/regular_joints.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/render_scene.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/sketch_edits.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/sketches.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/mechanical/transactions.py` | `mechanical_runtime` | — |
| `addin/AdomBridge/commands/modeling.py` | `dynamic_python_execution` | — |
| `addin/AdomBridge/commands/open_electronics_file.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/open_lbr.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/parameters.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/physical_properties.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/screenshot.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/commands/silkscreen_capture.py` | `nonmechanical_addin_runtime` | — |
| `addin/AdomBridge/http_server.py` | `addin_runtime_lifecycle` | — |
| `describe.py` | `contract_authoring_tool` | — |
| `rust/bridge/src/attach_orient_block.py` | `embedded_runtime_python` | — |
| `rust/bridge/src/attach_template.py` | `embedded_runtime_python` | — |
| `rust/bridge/src/epg_template.py` | `embedded_runtime_python` | — |
| `rust/bridge/src/inspect_bodies_script.py` | `embedded_runtime_python` | — |
| `rust/bridge/src/make3d_orient_block.py` | `embedded_runtime_python` | — |
| `rust/bridge/src/make3d_placement_block.py` | `embedded_runtime_python` | — |
| `rust/bridge/src/make3d_template.py` | `embedded_runtime_python` | — |
| `rust/bridge/src/pref_script.py` | `embedded_runtime_python` | — |
| `rust/bridge/src/routing_planner.py` | `embedded_runtime_python` | — |
| `rust/bridge/src/routing_script.py` | `embedded_runtime_python` | — |
| `server.py` | `retired_host_reference_optional_fallback` | — |
| `tools/package-mechanical-dev.py` | `development_packaging` | — |

### Dependency closure and semantic decisions

- `bridge.json; rust/bridge/src/main.rs`: The default host is already Rust and the manifest says the Python host left the production package. Optional sidecar provisioning/spawn remains behind explicit rollback preferences/environment and a local server.py; retire this compatibility branch without recreating missing Python modules.
- `rust/bridge/src/{mint,routing,epg,orchestrations,fusion_prefs}.rs`: Ten .py files are included as runtime script/templates. Audit inline script strings and run_modeling_script transport too; extension counts alone cannot establish Python elimination.
- `addin/AdomBridge/commands/mechanical/exploded_preview.py`: Imports exploded_preview_prototype.py as backend. The prototype filename does not make this an unused test file.
- `addin/AdomBridge/commands/modeling.py`: Arbitrary Python execution is an explicit legacy capability. Replacement needs typed Rust capability design or an explicit compatibility decision, not silent omission.
- `server.py; rust/bridge/src/main.rs`: Historical search_cloud_files/walk_cloud_tree and retired UI aliases intentionally refuse/redirect in the Rust host; do not revive retired behavior merely for nominal parity.
- `tools/package-mechanical-dev.py`: Development packaging still copies retired server.py/describe.py as well as the active add-in. Its artifact must be distinguished from the default production host. No standalone Python tests were found in this snapshot; the skill names external test/generation tools whose sources still need locating.

### Retired host imports and existing Rust replacements

The [fusion-contributing skill](/home/adom/.codex/skills/fusion-contributing/SKILL.md) explicitly identifies `handlers/dialog_classify.py` as retired and points to `rust/core/src/dialog_rules.json`. The manifest, runtime owner selection and current source support the broader Rust-host boundary. Missing files below are historical imports of `server.py`; they are **not default-host Python dependencies to restore or newly port**. Each replacement anchor and file hash is bound in JSON. Presence of source is credited as existing implementation, not as a fresh platform test.

| Historical Python import | Existing Rust replacement | Source evidence |
|---|---|---|
| `fusion_detect.py` | Rust host process/path discovery, readiness and launch | `rust/os-win/src/lib.rs`:205; `rust/os-mac/src/lib.rs`:10; `rust/core/src/windows_facts.rs`:3; `rust/bridge/src/launch.rs`:122 |
| `handlers/open_design.py` | Rust design opening orchestration | `rust/bridge/src/orchestrations.rs`:995; `rust/bridge/src/main.rs`:446 |
| `handlers/close_fusion.py` | Rust lifecycle stop/kill | `rust/bridge/src/lifecycle.rs`:112; `rust/bridge/src/lifecycle.rs`:133 |
| `handlers/dismiss_recovery.py` | Rust recovery handling | `rust/bridge/src/readiness_exec.rs`:106; `rust/bridge/src/readiness_exec.rs`:56 |
| `handlers/fusion_ui.py` | Rust host window/dialog operations and explicit retired-verb redirects | `rust/bridge/src/small_verbs.rs`:283; `rust/bridge/src/small_verbs.rs`:304; `rust/bridge/src/main.rs`:612 |
| `handlers/dialog_classify.py` | Rust dialog rule table and classifier | `rust/core/src/dialogs.rs`:8; `rust/core/src/dialog_rules.json`:2; `rust/core/tests/dialogs_golden.rs`:4 |
| `install_addin.py` | Rust add-in installer/synchronization | `rust/bridge/src/addin_install.rs`:64 |
| `aps.py` | Rust APS storage/client, host routes and sign-in | `rust/core/src/aps.rs`:4; `rust/bridge/src/main.rs`:949; `rust/bridge/src/aps_signin.rs`:185 |
| `ab_client.py` | Rust authenticated host client | `rust/core/src/ab.rs`:36 |

### Host source evidence and remaining coverage

- `bridge.json`:11 — The manifest launches adom-fusion-bridge.exe and explicitly says the Python host bridge left the package in Phase 4. Its older languages/macOS descriptive fields are not executable runtime selection.
- `rust/bridge/src/main.rs`:1251 — Readiness, chains and add-in orchestration default to the Rust executable unless an owner preference explicitly requests sidecar.
- `rust/bridge/src/main.rs`:1256 — Python sidecar is an explicit compatibility/rollback branch, enabled by owner preference or ADOM_FUSION_SIDECAR=1; spawn also requires server.py beside the executable.
- `rust/core/src/contract.rs`:15 — The Rust host embeds contracts/describe.json. describe.py is catalog authoring/legacy reference, not required by this runtime catalog path.
- `rust/bridge/src/addin.rs`:149 — Native Rust host transport still invokes the separate Python add-in at 127.0.0.1:8774. Host retirement does not retire in-app Python.
- `tools/package-mechanical-dev.py`:25 — This development packaging helper still copies retired server.py and describe.py. Distinguish this development artifact from the manifest's default production host architecture; remove or explicitly exclude compatibility artifacts in the final Python-free package.

Do not confuse retired imports with the following real gaps:

- `rust/bridge/src/main.rs` — Readiness recovery branches still call proxy directly for seat/dialog/relaunch work. proxy_with_timeout goes to the optional sidecar or returns sidecar_absent. Audit and connect these branches to existing Rust implementations; do not reintroduce retired Python merely to satisfy imports.
- `rust/bridge/src/main.rs` — This helper still builds an inline Python modeling script and calls proxy. Its reachability/behavior requires a Rust-path audit; inline scripts are not counted in the ten .py template files.
- `rust/bridge/src/addin.rs` — A connection-refused hint names the retired Python installer despite existing Rust installation code. Migrate stale guidance with the new native installer; do not count the missing installer as a live dependency.
- `tools/freeze-contract.py` — The contributing skill names this Python contract generation tool. Its source is absent from this snapshot; locate/port or explicitly retire it as tooling, not a host runtime dependency.
- `tests/run_verb_tests.py` — The contributing skill names this Python daily verb test. Its source is absent from this snapshot; qualify a Rust replacement or deliberate retirement separately from runtime migration.

This tree's 60-file inventory is complete for the supplied snapshot. The named but absent contract-generation and daily-test tools are a tooling coverage gap, not evidence that the production host still launches Python. Inline SDK scripts and indirect add-in calls remain runtime work beyond extension counts.

## Nonmechanical add-in commands

The mechanical router plus status/capability commands below are entry points, not three extra mechanical implementations. Existing nonmechanical Python handlers remain separately unqualified in r7. Capture features are inventoried for migration; all capture execution stays disabled during this campaign.

| Command | Actual dispatcher expression | Legacy source line | r7 parity / Fusion / release |
|---|---|---:|---|
| `mechanical` | `handle_mechanical` | 71 | — / — / — |
| `mech_capabilities` | `capabilities` | 72 | — / — / — |
| `mech_operation_status` | `operation_status` | 73 | — / — / — |
| `activity_banner` | `handle_activity_banner` | 74 | — / — / — |
| `get_app_state` | `handle_get_app_state` | 76 | — / — / — |
| `document_info` | `handle_document_info` | 77 | — / — / — |
| `activate_document` | `handle_activate_document` | 78 | — / — / — |
| `import_file` | `handle_import_file` | 80 | — / — / — |
| `export_step` | `handle_export_step` | 81 | — / — / — |
| `export_stl` | `handle_export_stl` | 82 | — / — / — |
| `export_3mf` | `handle_export_3mf` | 83 | — / — / — |
| `export_f3d` | `handle_export_f3d` | 84 | — / — / — |
| `export_usdz` | `handle_export_usdz` | 85 | — / — / — |
| `export_fbx` | `handle_export_fbx` | 86 | — / — / — |
| `export_dxf` | `handle_export_dxf` | 87 | — / — / — |
| `export_dwg` | `handle_export_dwg` | 88 | — / — / — |
| `export_iges` | `handle_export_iges` | 89 | — / — / — |
| `export_obj` | `handle_export_obj` | 90 | — / — / — |
| `export_sat` | `handle_export_sat` | 91 | — / — / — |
| `export_skp` | `handle_export_skp` | 92 | — / — / — |
| `get_design_info` | `handle_get_design_info` | 93 | — / — / — |
| `assembly_bom` | `handle_assembly_bom` | 94 | — / — / — |
| `physical_properties` | `handle_physical_properties` | 95 | — / — / — |
| `get_parameters` | `handle_get_parameters` | 96 | — / — / — |
| `set_parameter` | `handle_set_parameter` | 97 | — / — / — |
| `take_screenshot` | `handle_take_screenshot` | 98 | — / — / — |
| `electron_run` | `handle_electron_run` | 100 | — / — / — |
| `electron_zoom` | `handle_electron_zoom` | 101 | — / — / — |
| `electron_pan` | `handle_electron_pan` | 102 | — / — / — |
| `electron_select` | `handle_electron_select` | 103 | — / — / — |
| `execute_text_command` | `handle_execute_text_command` | 104 | — / — / — |
| `open_electronics` | `handle_open_electronics` | 105 | — / — / — |
| `list_text_commands` | `handle_list_text_commands` | 106 | — / — / — |
| `board_info` | `handle_board_info` | 107 | — / — / — |
| `export_source` | `handle_export_source` | 108 | — / — / — |
| `export_eagle_source` | `handle_export_eagle_source` | 109 | — / — / — |
| `open_lbr` | `handle_open_lbr` | 111 | — / — / — |
| `export_lbr` | `handle_export_lbr` | 112 | — / — / — |
| `close_document` | `handle_close_document` | 114 | — / — / — |
| `close_all_documents` | `handle_close_all_documents` | 115 | — / — / — |
| `open_schematic` | `handle_open_schematic` | 117 | — / — / — |
| `open_board` | `handle_open_board` | 118 | — / — / — |
| `show_3d_board` | `handle_show_3d_board` | 119 | — / — / — |
| `show_2d_board` | `handle_show_2d_board` | 120 | — / — / — |
| `show_schematic` | `handle_show_schematic` | 121 | — / — / — |
| `import_electronics` | `handle_import_electronics` | 122 | — / — / — |
| `save_to_cloud` | `handle_save_to_cloud` | 124 | — / — / — |
| `list_cloud_projects` | `handle_list_cloud_projects` | 125 | — / — / — |
| `list_cloud_files` | `handle_list_cloud_files` | 126 | — / — / — |
| `delete_cloud_file` | `handle_delete_cloud_file` | 127 | — / — / — |
| `create_cloud_folder` | `handle_create_cloud_folder` | 128 | — / — / — |
| `open_cloud_file` | `handle_open_cloud_file` | 129 | — / — / — |
| `open_by_urn` | `handle_open_by_urn` | 130 | — / — / — |
| `check_recovery` | `handle_check_recovery` | 131 | — / — / — |
| `search_cloud_files` | `handle_search_cloud_files` | 132 | — / — / — |
| `walk_cloud_tree` | `handle_walk_cloud_tree` | 133 | — / — / — |
| `export_cloud_file` | `handle_export_cloud_file` | 134 | — / — / — |
| `export_bom` | `handle_export_bom` | 136 | — / — / — |
| `export_cpl` | `handle_export_cpl` | 137 | — / — / — |
| `export_gerbers` | `handle_export_gerbers` | 138 | — / — / — |
| `set_design_rules` | `handle_set_design_rules` | 139 | — / — / — |
| `export_board_image` | `handle_export_board_image` | 140 | — / — / — |
| `detect_layers` | `handle_detect_layers` | 141 | — / — / — |
| `take_silkscreen_screenshot` | `handle_take_silkscreen_screenshot` | 143 | — / — / — |
| `run_modeling_script` | `handle_run_modeling_script` | 145 | — / — / — |

## Host and tooling surface

The existing Rust executable owns default host dispatch, readiness, launch/lifecycle, dialogs, APS and add-in installation/orchestration. Those implementations are credited; a new all-Rust in-app SDK replacement still requires parity and host regression tests. The 50 entries below inventory the retired Python host source for compatibility history, **not 50 missing Rust handlers**. Some legacy aliases are intentionally retired; some existing Rust orchestration branches still require closure of sidecar calls. The full frozen 179-verb host catalog is separately preserved, and neither catalog nor source table is a complete dynamic call graph.

| Historical Python host command | Source line | New r7 integration qualification |
|---|---:|---|
| `describe` | 2819 | — |
| `readiness` | 2820 | — |
| `set_preference` | 2821 | — |
| `get_preferences` | 2822 | — |
| `install_fusion` | 2823 | — |
| `open_design` | 2824 | — |
| `close` | 2825 | — |
| `stop` | 2826 | — |
| `kill` | 2827 | — |
| `launch` | 2828 | — |
| `start` | 2829 | — |
| `dismiss_recovery` | 2830 | — |
| `relocate_recovery` | 2831 | — |
| `open_cloud_file` | 2832 | — |
| `new_electronics_from_eagle` | 2837 | — |
| `screenshot_fusion` | 2838 | — |
| `click_fusion` | 2839 | — |
| `send_key` | 2840 | — |
| `close_window` | 2841 | — |
| `window_info` | 2842 | — |
| `screenshot_all` | 2843 | — |
| `check_dialogs` | 2848 | — |
| `managed_library_create` | 2853 | — |
| `web_signin` | 2854 | — |
| `managed_library_push` | 2855 | — |
| `managed_library_pull` | 2856 | — |
| `managed_library_link` | 2857 | — |
| `managed_library_unlink` | 2858 | — |
| `library_manager` | 2859 | — |
| `alerts` | 2860 | — |
| `addin_status` | 2870 | — |
| `notify_owner` | 2876 | — |
| `aps_status` | 2879 | — |
| `aps_set_client_id` | 2880 | — |
| `aps_signin` | 2881 | — |
| `aps_set_browser` | 2882 | — |
| `aps_get_browser` | 2883 | — |
| `aps_forget_browser` | 2884 | — |
| `aps_search` | 2887 | — |
| `aps_browse` | 2888 | — |
| `aps_recent` | 2889 | — |
| `aps_file_info` | 2890 | — |
| `aps_versions` | 2891 | — |
| `aps_download` | 2892 | — |
| `aps_create_folder` | 2893 | — |
| `aps_upload` | 2894 | — |
| `aps_open` | 2895 | — |
| `aps_get` | 2896 | — |
| `search_cloud_files` | 2899 | — |
| `walk_cloud_tree` | 2900 | — |

### Full host catalog names

- `fusion_account`
- `fusion_action_ledger`
- `fusion_activate_document`
- `fusion_addin_status`
- `fusion_alerts`
- `fusion_apply_fusion_update`
- `fusion_apply_pours`
- `fusion_aps_browse`
- `fusion_aps_create_folder`
- `fusion_aps_download`
- `fusion_aps_file_info`
- `fusion_aps_forget_browser`
- `fusion_aps_get`
- `fusion_aps_get_browser`
- `fusion_aps_open`
- `fusion_aps_recent`
- `fusion_aps_search`
- `fusion_aps_set_browser`
- `fusion_aps_set_client_id`
- `fusion_aps_signin`
- `fusion_aps_status`
- `fusion_aps_upload`
- `fusion_aps_versions`
- `fusion_assembly_bom`
- `fusion_attach_3d_package`
- `fusion_bind_library_3d`
- `fusion_board_bom`
- `fusion_board_info`
- `fusion_board_seed`
- `fusion_board_source`
- `fusion_board_stackup`
- `fusion_build_library_3d`
- `fusion_cancel_run`
- `fusion_capture_library_views`
- `fusion_change_pours`
- `fusion_check_recovery`
- `fusion_claim_seat`
- `fusion_cleanup_cloud_files`
- `fusion_close_all_documents`
- `fusion_close_document`
- `fusion_close_window`
- `fusion_create_cloud_folder`
- `fusion_create_library`
- `fusion_delete_cloud_file`
- `fusion_demo`
- `fusion_demo_progress`
- `fusion_describe`
- `fusion_detect_layers`
- `fusion_dismiss_blocking_dialogs`
- `fusion_document_info`
- `fusion_eagle_import_status`
- `fusion_electron_pan`
- `fusion_electron_run`
- `fusion_electron_select`
- `fusion_electron_zoom`
- `fusion_electronics_query`
- `fusion_execute_text_command`
- `fusion_export_3mf`
- `fusion_export_board_image`
- `fusion_export_bom`
- `fusion_export_cloud_file`
- `fusion_export_cpl`
- `fusion_export_dwg`
- `fusion_export_dxf`
- `fusion_export_eagle_source`
- `fusion_export_f3d`
- `fusion_export_fbx`
- `fusion_export_gerbers`
- `fusion_export_iges`
- `fusion_export_lbr`
- `fusion_export_obj`
- `fusion_export_optimized_glb`
- `fusion_export_sat`
- `fusion_export_skp`
- `fusion_export_source`
- `fusion_export_step`
- `fusion_export_stl`
- `fusion_export_usdz`
- `fusion_fetch_optimized_glb`
- `fusion_generate_package`
- `fusion_get_app_state`
- `fusion_get_design_info`
- `fusion_get_parameters`
- `fusion_get_preferences`
- `fusion_import_electronics`
- `fusion_import_step`
- `fusion_inspect_bodies`
- `fusion_install_fusion`
- `fusion_install_library_bundle`
- `fusion_interactive`
- `fusion_kill`
- `fusion_library_manager`
- `fusion_library_parts`
- `fusion_list_cloud_files`
- `fusion_list_cloud_projects`
- `fusion_list_text_commands`
- `fusion_live_feed`
- `fusion_make_3d_package`
- `fusion_make_showcase_project`
- `fusion_make_test_project`
- `fusion_managed_library_create`
- `fusion_managed_library_link`
- `fusion_managed_library_pull`
- `fusion_managed_library_push`
- `fusion_managed_library_unlink`
- `fusion_mcp_call`
- `fusion_mcp_enable`
- `fusion_mcp_resources`
- `fusion_mcp_status`
- `fusion_mcp_tools`
- `fusion_mech_capabilities`
- `fusion_mech_operation_status`
- `fusion_mechanical`
- `fusion_move_parts`
- `fusion_narrate`
- `fusion_new_electronics_from_eagle`
- `fusion_notify_owner`
- `fusion_open_board`
- `fusion_open_by_urn`
- `fusion_open_by_urn_note`
- `fusion_open_cloud_file`
- `fusion_open_design`
- `fusion_open_electronics`
- `fusion_open_lbr`
- `fusion_open_schematic`
- `fusion_physical_properties`
- `fusion_place_vias`
- `fusion_plan_library_import`
- `fusion_plan_route`
- `fusion_pour_state`
- `fusion_preflight_lbr`
- `fusion_prefs_close`
- `fusion_prefs_open`
- `fusion_prewarm`
- `fusion_prove_result`
- `fusion_readiness`
- `fusion_release_seat`
- `fusion_remove_pours`
- `fusion_reset_workspace`
- `fusion_resolve_seat_dialog`
- `fusion_restart_for_addin`
- `fusion_ripup_nets`
- `fusion_route_net`
- `fusion_routing_drc`
- `fusion_routing_state`
- `fusion_run_modeling_script`
- `fusion_save_to_cloud`
- `fusion_schematic`
- `fusion_schematic_plan`
- `fusion_schematic_run`
- `fusion_screenshot_fusion`
- `fusion_search_cloud_files`
- `fusion_set_auto_update`
- `fusion_set_design_rules`
- `fusion_set_driving_badge`
- `fusion_set_parameter`
- `fusion_set_preference`
- `fusion_set_seat_takeover`
- `fusion_set_window_sizing`
- `fusion_show_2d_board`
- `fusion_show_3d_board`
- `fusion_show_3d_package`
- `fusion_show_attributes`
- `fusion_show_footprint`
- `fusion_show_library`
- `fusion_show_project`
- `fusion_show_schematic`
- `fusion_show_symbol`
- `fusion_signin`
- `fusion_signin_2fa`
- `fusion_start`
- `fusion_state`
- `fusion_stop`
- `fusion_take_screenshot`
- `fusion_uninstall`
- `fusion_verb_times`
- `fusion_view`
- `fusion_walk_cloud_tree`
- `fusion_web_signin`

## Promotion and completion gates

- Bind exact source/build hashes and advertise only the implemented subset.
- Match legacy units, selectors, side effects, error/recovery semantics or document an accepted narrower contract.
- Pass meaningful Rust unit/fault and independent external behavior tests on Linux and Windows.
- Build against the actual Autodesk SDK and run bounded owned-fixture Fusion flows with exact readback, replay/conflict/stale/no-second-effect tests.
- Preserve failure/unknown receipts and refuse automatic effect retry; clean only exact owned fixtures.
- Qualify Python-free installer/bootstrap, signing, packaging, existing bridge regressions and public release separately.

Runtime handlers, embedded scripts, host fallback, catalog generation, test tools, installer/bootstrap and release packaging are separate work categories. A Python-free claim requires all reachable runtime/bootstrap paths to be Rust/typed C++ and no runtime Python provisioning. Keep test/tool retirement explicit too; do not hide them behind a runtime-only claim. Arbitrary Python modeling needs an explicit replacement/compatibility decision.

## Reproducibility

The generator reads source bytes; it neither imports Python nor invokes Fusion. It validates 61 unique reviewed handler mappings, exact top-level function locations and semantic equality of the legacy/Rust schemas, inventories dispatcher literals and all Python files, and binds source hashes. This is a reviewed textual inventory, not Python AST analysis or a proof that every dynamically dispatched host path was discovered.

From r7:

```sh
/home/adom/.cargo/bin/cargo run --offline --manifest-path verification/native-operations/Cargo.toml --bin porting_ledger -- . ../exploded-preview-preparation/runtime-dev9-build-r1/source
```

Running the generator resets the ledger to this starting baseline. Promote later checkpoint evidence deliberately in a separately reviewed update; never infer qualification from changing source files alone. The independent native-operation test design is in [TEST-PLAN.md](../verification/native-operations/TEST-PLAN.md).
