# Mechanical build and Rust migration status

Verified 16 October 2026. The latest installable package is **1.11.39**, published
6 October. The new native mechanical implementation has **no public replacement
release**. Source publication, passing tests and installed functionality are
separate states.

## What is shared

The earlier Python-based mechanical work is available in open PRs
[#11: mechanical foundation](https://wiki.adom.inc/adom/fusion-bridge/prs/11),
[#12: render inspection](https://wiki.adom.inc/adom/fusion-bridge/prs/12),
[#14: exploded plans](https://wiki.adom.inc/adom/fusion-bridge/prs/14), and
[#15: exploded previews](https://wiki.adom.inc/adom/fusion-bridge/prs/15).
PR #10 was closed and replaced by the current-master foundation work. These
four PRs are unmerged; they are historical implementation references, not
approval to ship Python-backed additions under the Rust migration requirement.

The R21 native Rust checkpoint is available in [draft PR #19](https://wiki.adom.inc/adom/fusion-bridge/prs/19)
on the isolated `native-rust-checkpoint-2026-10-16` branch. It does not
replace or configure the installed bridge. The frozen product source, protocol verifiers, independent placement observer
and Rust contract-authoring tool are included. R21 actual-app qualification is pending.

## Python to Rust

**18 of 61 legacy mechanical operations have limited native implementations;
43 do not yet have native handlers.** That is approximately 30% by operation
count; it does not measure engineering completion or complete behavioral parity. Nine native
helper operations bring the current source candidate to 27 exposed operations.
No operation is being claimed as fully qualified across every legacy behavior.

The 18 implemented subsets are `assembly_properties`, `bom`, `bom_export`,
`component_create`, `component_metadata`, `feature_create`, `inspect`,
`material_list`, `mcmaster_discover`, `new_design`, `occurrence_insert`, `occurrence_update`,
`parameter_create`, `parameter_set`, `sketch_constraint`, `sketch_create`,
`sketch_dimension`, and `sketch_geometry`.

The host executable was already Rust. The new in-process native candidate puts
mechanical policy, dispatch, transport, identity handling and durable operation
records in Rust, using typed C++ glue for Autodesk's SDK. Existing Python remains
in the preserved production installation. The historical migration inventory
contains 60 Python files: 47 add-in files, 10 embedded runtime templates, two
authoring/packaging tools and one retired host reference. This is a baseline
inventory, not a count of remaining files or completed ports. A trustworthy
whole-codebase conversion percentage has not yet been established.

## Feature buildout

| Area | Current native progress | Remaining scope |
| --- | --- | --- |
| Runtime and safety | Rust transport, owner-thread execution, document/revision checks, durable results, replay/conflict handling; bounded live campaigns | Broader restart/failure/soak tests, deployment and installer integration |
| Parameters and sketches | Parameter create/set, origin sketches, eight primitive types; finite horizontal/vertical constraint and 42 mm driving-dimension live tests | General solver graphs, other dimensions, edits, offsets, projection and broader persistence |
| Solids and physical properties | Basic one-sided new-body extrusion and bounded root physical-property reads exercised live | Independent geometry/material qualification gaps; additional solid features, arbitrary assemblies and editing |
| Assemblies | Empty flat component creation, shared occurrences and local purchasing metadata independently checked in Fusion | Physical child components, nesting, native online metadata; R21 absolute translation compiled and tested; actual Fusion pending; joints and motion |
| BOM | Rust collection/CSV core and native handlers implemented; portable/Windows core tests pass | Successful complete actual-app BOM traversal and CSV export qualification |
| McMaster | Native command registration discovery exercised live | Plugin launch, model download, CAD import and placement |
| Exploded views, animation and rendering | Standalone Rust exploded planner passes 31 tests on each platform; older Python feature PRs are available | Native preview lifecycle, presentation/animation/render workflows and persistence |
| Geometry and service access | Identity and revision foundations exist | Import/export, general measurements, interference/collision, configurations and connection to AI Flow Mechanical |
| Other bridge functions | Existing electronics and other production features remain available through the legacy bridge | Remaining Python runtime, embedded execution and tooling migration; full regression qualification |

## Current conversion work

R21 absolute occurrence positioning has passed **508 Linux tests and 512 Windows
tests**, including native SDK compilation and release linking. Its 28-test
protocol verifier and 20-test independent pose/snapshot observer have passed
portable tests, native Windows tests and binding compilation checks. The
observer also links against the installed Fusion SDK. Actual Fusion qualification
is still pending. This is a finite translation
subset, not general placement parity.

A Rust replacement for contract-authoring tooling reproduces all 665,236 bytes
of the historical contract, including 179 frozen entries, 178 runtime verbs and
61 mechanical schemas. Explicit changed-catalog refreezing is implemented; all 21 tests pass on Linux
and Windows, including actual CLI and atomic-publication checks. Release CI adoption
and macOS atomic publication remain pending. No Python runtime has been
removed from the installed bridge by these source changes.

## Latest completed verification

R20-fix3: **494 Linux tests and 498 Windows tests passed**, plus binding/SDK
checks. Its actual Fusion assembly campaign passed **52 requests, six durable
successful operations and zero uncertain attempts**. A separate SDK observer
confirmed the two empty definitions and three occurrences, shared identity,
local metadata, identity placements and exact revisions. Journal integrity and
normal stop/unload checks passed. These suites overlap earlier checkpoints and
must not be added together as unique test coverage.

See the [source-bound qualification receipt](r20-assembly-qualification.json).
This result is limited to the stated assembly fixture. It is not complete
mechanical support. Historical failed campaigns remain failed. Full semantic
save/reopen coverage, crash/restart and extended soak acceptance are incomplete.

Background application testing now uses bounded named campaigns and SDK evidence.
Repeated screenshots and Shotlog capture are disabled. The existing development
pin and production bridge configuration remain preserved.

## AI Flow Mechanical

[John's current prototype](https://wiki.adom.inc/adom/adom-aiflow-mechanical)
checks a declared JSON mechanical review. It currently has no calls to this
native bridge or CAD collision engine. The projects are complementary, but
**there is no implemented integration today**. See the
[integration assessment and proposed boundary](AI-FLOW-INTEGRATION.md).

The current priority is to complete Python-to-Rust conversion of existing bridge
behavior and supporting tooling before adding more features. That includes
portable and Windows tests, bounded background Fusion qualification, persistence
and regression checks. The AI Flow evidence adapter and additional service-access
features are deferred until this migration is complete.
Public releases require the relevant live tests plus package, installation,
regression and release-channel qualification; source publication alone does
not meet that gate.
