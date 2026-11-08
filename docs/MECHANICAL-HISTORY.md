# Mechanical layer: development history

The changelog of the mechanical layer's development builds (1.12.0-dev.2 to dev.10), kept here when the
layer was rebased onto 1.12.8 as 1.13.0-dev.1 so the main CHANGELOG reads as releases. Newest first.

## 1.12.0-dev.10 (2026-10-20)

- The dev.9 mechanical layer (61 typed operations through `fusion_mechanical`) merged onto 1.11.44.
  Development build only, not released. The notes below it are the earlier development builds.
- The exe owns the mechanical layer's validation, durable journal and replies, as the rest of the bridge
  does; the add-in only runs the Fusion API work. `fusion_mech_capabilities` and
  `fusion_mech_operation_status` answer from files, so they no longer wait on Fusion's main thread.
- John's export, BOM, mass-properties and add-in server files are his again: the dev.9 source had
  replaced them with mechanical wrappers and reverted two of his fixes.

## Staged exploded-preview controller (2026-10-06)

- Add a source candidate for four direct journaled preview writes and live inspection (61 typed operations; 179 public verbs). Exact current-session creation history, native identity and blank-state checks bind a caller-created presentation document.
- Retain independent BRep graphics through finite absolute samples, explicit document activation and exact disposal. Preserve source geometry/joints/visibility, uncertain native effects and unresolved operation history; leave the empty document open for the caller.
- Initialize all bounded native mesh collections before capturing any component revision IDs. Native diagnosis found first child collection access advancing ancestor IDs; retain strict target/history checks and refuse observed document flag changes or unavailable reads without claiming rollback. A separate native function gate passed a fresh clean unsaved blank and cold already-modified local F3D control; saved/cloud-clean multi-component behavior remains unqualified.
- Track presentation-only native revision changes across exact owned graphics effects, bind a bounded blank semantic witness, and preserve immutable creation receipts through a verified disposal/reuse chain. Top-level ownership uses the retained Component's exact graphics inventory; descendant parent checks remain strict. Uncertain deletion attempts cannot be reissued.
- Revised native backend qualification passed on candidate-initialized owned fixtures: 66 instance poses, 55 copied-body geometry observations, eight refusals, two disposals, two archive roundtrips and four exact closures. The first two failures and separate retained-tree reconciliation remain preserved. Installed routing, viewport framing and visible output remain unqualified. Preserve current master's PCB fabrication resource rename. No version bump, automatic camera/foreground action, document close, render, capture or storyboard is added.

## Unreleased exploded-view planning (2026-10-06)

- Add read-only `exploded_view_plan` (56 typed operations; 179 public verbs): exact per-instance home transforms, normalized world/parent/local translation directions, simultaneous moves within ordered steps, instructions and component-linked labels.
- Sample finite explode/reassembly timelines deterministically; nested instances inherit each ancestor offset once and reverse playback returns exact home poses. Native source geometry, joint fields and engineering BOM quantities remain unchanged.
- Refuse stale, ambiguous, unsupported configuration, hierarchy, matrix and budget inputs. Native function qualification passed six instances across 60 poses, source/joint/BOM preservation and archive reopening. Installed public routing remains untested. No native movement, graphics preview, trails, storyboard, image/video output or collision-free path claim. Release versions remain unchanged.

## Unreleased scene inspection (2026-10-06)

- Add guarded `render_scene_inspect` (55 typed operations; 179 public verbs) with bounded scene settings, viewport camera and environment observations. Missing runtime data stays explicit and partial.
- Prepare and verify an optional detached world-space camera without assigning it to the viewport or changing render settings. No render execution, complete scene witness, queue ownership or pixel-output qualification is provided. Production and add-in release versions remain unchanged.
- Preserve false transient camera validity flags reported by Fusion (Autodesk FUS-263280), while independently checking the exact newly created Camera wrapper and geometry/lens readback. Other native-object guards remain unchanged.

## Unreleased integration (2026-10-06)

- Integrate the mechanical foundation with current master while preserving newer routing and electronics workflows. Regenerate the combined 179-verb contract; retain all 54 typed mechanical operations.
- Keep master's release metadata and separate add-in version unchanged. Historical native evidence stays tied to its original source; merged-source native acceptance remains pending. See docs/integration-2026-10-06.md.
- Prioritize exploded views and ordered assembly instructions for the next animation checkpoint, with BOM-linked labels, trails and reversible presentation poses that preserve source joints. This remains planned work.

## 1.12.0-dev.8 (2026-10-05)

- Persist typed mechanical admission, dispatch claims, exact document identity, request digest and terminal results in a bounded app-owned SQLite journal. Same-session duplicates never redispatch; old-session history is readable without native replay.
- Report interrupted work and uncertain persistence explicitly. Preserve observed execution across duplicate-event faults, recover hot SQLite journals, and refuse corrupt, incompatible, unsafe or exhausted storage before dispatch.
- Route operation-status reads directly to the add-in without launch/restart, window etiquette, modal handling or timeout remediation; unavailable history remains unknown. Bind client reconciliation to immutable request digests and coherent terminal identity/durability.
- Add real subprocess-crash, concurrent claim, lost-acknowledgement, and loopback HTTP/CustomEvent regressions for fast replies, false delivery returns, lost waiters and storage failures. Native durability/installer gates remain required before release.
- Give each loaded add-in copy its own event registration and enforce an exclusive local listener. Losing copies release only their own resources; failed or repeated startup cannot replace a live copy's event or leave a bound socket behind. Native event-delivery acceptance remains a separate gate.
- Preserve a bounded callback waiter when native `fireCustomEvent` returns false: Fusion 2705.1.30 has delivered callbacks despite that return. Confirm completion from the callback and arbitrate expired queued work against claimed execution through the durable journal; never infer nonexecution from the Boolean alone.
- Flush a newly staged SQLite journal through a writable handle on Windows; read-only handles fail `fsync` with `EBADF`. Flush errors still prevent publication and dispatch.
- Report a conflicting submitted payload as not executed only when the admission check proves an existing different request digest. Preserve the original operation record; unrelated storage and execution uncertainty still reports unknown.
- Keep Ralph capture selective: model/API state, logs, operation records and archive hashes are the default evidence. Continuous screenshot feeds and automatic Shotlog draining are disabled; select and deduplicate any visual evidence before upload.

## 1.12.0-dev.7 (2026-10-05)

- Add read-only `configuration_inspect` (54 typed operations): ordered native table/row/column identities, numeric/text cells, controlled-source graph, explicit/effective suppression and active row. No configuration activation or lifecycle writes are enabled.
- Witness the native reserved test row separately from ordinary configurations and include its cells in revision checks. Preserve per-field diagnostics when unsaved/offline property values are unavailable. Native conversion survived an acknowledged Abort in the owned probe; configured model writes remain disabled, and complete configured archive recovery remains unverified.
- Include inactive configuration cells and source relationships in stale-target and recovery witnesses. Unknown columns, themes, preview hierarchies and inserted configurations are explicit incomplete scopes; model mutations refuse them. Configured-design model writes also remain gated until native configuration recovery/archive acceptance, including indirect edits through parameters or features.
- Stamp current-active-row BOM JSON and both CSV modes with configuration and exact document/revision provenance. Visibility remains independent of engineering quantity; unsupported configuration scope cannot produce a purportedly complete BOM.
- Add offline configuration graph, preflight, typed-cell, wrapper-lifetime and CSV regressions. Native configured-table readback and persistence remain release gates.

## 1.12.0-dev.6 (2026-10-02)

- Add eight guarded operations (53 total): geometry-solving regular-joint create/redefine, reusable joint-origin lifecycle, and native motion-link lifecycle. Preserve the existing as-built and joint drive/limit operations.
- Verify attachment identities and native parameter readback; restore the exact timeline marker in finally for historical redefinition. Expand stale/recovery witnesses to joint geometry, origin frames and orientation sources, motion-link members/ratios, and timeline position.
- Reacquire attachment sources after native recompute invalidates retained geometry wrappers. Preserve graph identity through token lookup followed by fresh API equality, and report native joint-transform provenance without applying occurrence transforms twice.
- Handle rigid locks using measured native semantics: refuse `locked:true`, skip the unsupported false setter, and verify the implicit false state after creation or redefinition. Motion-link ratio edits may re-solve member positions; later drive checks use the post-edit pose.
- Add focused offline graph, repeated-instance, grounded-placement, unit, failed-setter and timeline-restoration regressions. Native solver, motion-link behavior and archive persistence remain required release gates.


## 1.12.0-dev.5 (2026-10-05)

- Add six typed everyday-modeling operations (45 total): body-scoped geometry queries with explicit
  frames and ambiguity refusal, native distance/angle measurements with witness points, sketch
  trim/extend/split/break, signed parameter-driven offsets through `addOffset2`, construction axes from
  existing geometry, and native sketch constraint/health diagnostics.
- Curve edits return replacement references, reject silent no-ops and require explicit consent for
  trimming an entire curve away. Root-space child geometry requires its exact occurrence context;
  cross-component axes require an explicit creation occurrence when authored outside the root.
- Recovery witnesses now include spline control data, constraint/dimension graph links, offset
  relationships and construction axes; unreadable graphs refuse mutation. Selected-curve shape
  changes are checked separately so unrelated sketch changes cannot hide a refused curve edit.
  Offline boundary checks cover frames, selection cardinality, witness consistency, failed/no-op edits
  and signed offset readback. Native application acceptance passed on Windows Fusion 2705.1.11:
  16 groups, 254 assertions, 32 exact owned closures and 16 reviewed viewport captures.
- Handle native component-space measurement limitations with temporary geometry copies; preserve
  and label degenerate parallel-angle witness points using independent direction verification.
  Native in-place extension is accepted only after the selected curve's persisted change is verified.
- Add the maintained everyday Ralph runner with independently measured topology, constraint graphs,
  archive reopening and campaign-aware recovery. Recovery recomputes aggregate check counts while
  preserving historical failures. Exact embedded catalog checks reject stale cached binaries before
  packaging; installed payload hashes and live native catalogs are verified separately.

## 1.12.0-dev.4 (2026-10-02)

- Add a maintained native Ralph campaign for bracket, assembly and optional local supplier fixtures,
  archive round trips, numerical geometry/BOM witnesses, failure cleanup and image-hash review.
  Offline gates record source and embedded-asset hashes with individual build/test logs. Live-feed
  capture verifies actual PNG pixels and Shotlog transfer without declaring an unreviewed visual pass.
- The legacy verb runner defaults to an explicit read-only argument allowlist. Manual and fixture
  workflows remain separately reported; `--only` cannot authorize them. Production board name
  workflows, preference changes, name-based document polling and automatic mutation retries are disabled.
- Preserve raw relay/native envelopes and reject semantic errors beneath transport success. Negative
  cases require exact refusal contracts; infrastructure failures never count as guard coverage.
  Timed-out typed operations can be reconciled through their matching session/operation ledger only.
- Each run has unique reports, caller identity and scratch ownership. Reusable campaign helpers persist
  exact document IDs, verified native checkpoints and file ownership; cleanup runs in `finally`, reports
  failures and refuses changed identities. Foreground behavior remains explicitly unverified without
  timestamped, attributed transitions. Preference restoration preserves concurrent user changes.
- The add-in HTTP endpoint validates bounded request framing, bodies and timeouts, and enforces an
  absolute request deadline. Duplicate callbacks cannot replace completed results; diagnostic watchdogs
  no longer release another request's main-thread lock. Real socket and offline harness regressions cover
  malformed requests, callback interleavings, contradictory responses, unsafe retries and cleanup guards.

## 1.12.0-dev.3 (2026-10-02)

- Assembly checkpoint adds 20 typed operations (39 total): internal components and independent copies,
  nested occurrence placement/grounding, seven native as-built joint motions with limits and drive
  readback, rigid groups, interference without creating bodies, materials and assembly physical
  properties including center of mass. Existing design intent is preserved; internal creation requires Hybrid.
- Import downloaded STEP/SAT supplier CAD into a dedicated component, validate an optional expected
  SHA-256, and persist supplier/part/source/hash metadata. Discover or explicitly launch Fusion's native
  McMaster UI. Browser selection/download remains interactive; no cloud save is implied.
- Structured instance BOM and flat identity-grouped quantities include purchased-part provenance;
  both CSV modes publish atomically. Purchased collapse/exclusion remains separate from physical mass.
  Root bodies and repeated nested bodies count once per instance, with traversal budgets/cycle guards.
- Revision/recovery witnesses now include component metadata/materials, occurrence grounding,
  joint motion/limits and rigid groups. Nested occurrence references preserve their instance path.
  Model recovery excludes files, cloud effects and interactive command completion.
- Offline coverage checks nested quantities/mass, purchased collapse/exclusion, hidden cycles,
  supplier hash guards, joint limit conversions and CSV publication boundaries. Independent live
  verification passed all seven joint motions, placement/reuse, native McMaster launch, supplier CAD,
  purchasing/physical BOM and archive persistence. Captured positions are checked in every affected
  root assembly context after compute; BOM physical totals use explicit high calculation accuracy.
  See docs/build-2026-10-02.md for the 815 retained native checks, source audit and coverage limits.

## 1.12.0-dev.2 (2026-10-02)

- Typed parametric modeling adds sketch primitives, driving dimensions and geometric constraints,
  sketch edits/projections, construction planes and solid feature creation. Feature operations cover
  extrude/cut, revolve, hole, fillet, chamfer, shell, loft, sweep, combine, split, move, mirror,
  rectangular/circular patterns, draft and straight threads through runtime-checked Fusion APIs.
- A runtime probe verifies PTransaction abort and commit behavior before model mutations are enabled.
  Failed operations report semantic recovery checks. Atomic parameter batches support create/set/delete;
  feature edits support naming, suppression and owned model-parameter expressions. Independent feature
  and sketch-geometry deletion uses the same health and transaction gate.
- Inspection returns sketch curves/points/profiles and detailed face/edge geometry with guarded references.
  Profile references bind a sketch, current revision, index and geometric fingerprint. Health walks
  timeline groups and reports intentionally suppressed items separately.
- Operation status and capability requests bypass the main-thread queue and native bridge busy gate.
  The ledger uses a short lock around metadata snapshots; no lock is held over Fusion API work.
- The Autodesk desktop MCP example now uses its supported activeCommand query. Shipping schemas remain
  explicit about runtime API availability and the boundary between model transactions and external files.

## 1.12.0-dev.1 (2026-10-02)

- Development checkpoint: `fusion_mechanical` adds typed create/inspect/parameter/health/export operations,
  document and revision guards, occurrence-context references and a session operation ledger. Discover
  schemas with `fusion_mech_capabilities`; query timed-out work with `fusion_mech_operation_status`.
- Parameter failures restore their previous expression and report semantic recovery checks. New designs
  stay unnamed to prevent save dialogs during cleanup. Generic atomic modeling batches are not promised.
- Physical properties include root-owned bodies; distinct same-named component definitions stay separate
  in mass reports and BOMs. BOMs count bodies and children, preserve explicit empty kit rules, and report CSV failures.
- Exports validate capabilities and stage output before atomic publication. Existing files survive export
  failures; postpublication cleanup errors remain successful publications with warnings. STL uses mm;
  STL/3MF never launch a print utility or silently produce one file per body.
- Register HTTP result waiters before firing Fusion events, preventing fast commands from being dropped as stale.
