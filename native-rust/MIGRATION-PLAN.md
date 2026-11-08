# Full Rust bridge migration

User authorized porting everything with detailed testing on 2026-10-14. Preserve frozen r6. All new product and verification clients Rust, minimal Autodesk C++ ABI glue. No new Python. Root sole Windows/Fusion operator; all captures, feeds, failure screenshots and Shotlog off. Preserve pinned install and user documents. Public release authorization persists after thorough acceptance, installer/signing and regression checks.

First functional slice: authenticated operation dispatch with retained-journal admission, exact current document membership and design revision guards. Port native design inspection, parameters (create/set/batch), new design and owned scratch lifecycle first, with real SDK readback. Unsupported operations must refuse before admission/effect. Results distinguish durable terminal, nondurable observation and unknown; never automatically retry effects.

Then implement the full legacy operation inventory in useful slices: sketch construction/constraints/dimensions/project/edit; planes/axes/solid features; entity inspection/query/measure/delete/export; components/occurrences/joints/rigid groups/interference; purchased CAD/McMaster/BOM/materials; exploded views/presentation/render/configuration/motion; remaining nonmechanical add-in handlers and Python tooling. Inventory will track implemented, offline-tested, actual-app-tested and released separately. No schema-only success claims.

Three-musketeers structural roles: builder native SDK/mechanical adapter; adversarial reviewer read-only design/source/results; verifier independent external client and parity inventory. Root owns HTTP/journal/queue integration and actual app testing. Only available model family is used; no Opus/Sonnet or model-diversity claim. Fresh agents attempted when available.

Qualification per slice: Rust unit and fault tests, independent behavior tests, Linux/Windows full regressions at checkpoints, MSVC native SDK build, bounded real Fusion scratch flows with exact readback/replay/conflict/stale guards, owned fixture cleanup and complete stop/unload. Existing document metadata and installed hashes checked before/after. No unbounded screenshot or polling loops.

## R10 isolated next checkpoint

The r10 candidate adds `native-sketch-v1` exact retained root origin-plane creation and scoped summary inspection. See `docs/R10-SKETCH-CONTRACT.md` and `evidence/builder-linux/REPORT.md`. Eight-primitive geometry preparation is copied from the reviewed Rust core; SDK insertion is still explicitly unsupported. This is not full legacy sketch parity and does not broaden parameter-only scratch discard. Author Linux/compile receipts do not substitute for root's separate Windows/Fusion qualification and release gates.

## r11 candidate checkpoint — retained sketch primitive insertion

This copy begins from r10 product freeze2 (`BASELINE-R10-PRODUCT-FREEZE2.txt`), preserving earlier frozen products and evidence. The r11 delta is documented in `docs/R11-GEOMETRY-CONTRACT.md`: exact retained empty origin-root sketch, explicit new contract, whole-plan pure preparation before admission, durable execution start before typed SDK effects, actual geometry and prefix readback, no automatic rollback/retry. The eight input primitives are bounded, non-atomic and not full legacy constraint/transaction parity. Product geometry scratch cleanup remains unsupported. Independent review and source-bound Windows/live qualification must precede any publication. Root owns host operation and external verifier clients; this author performs local Rust/ABI source qualification only. No live qualification from an older checkpoint is promoted automatically.

## r12 candidate checkpoint — retained sole-profile solid extrusion

The exact r11 freeze1 baseline is preserved separately. R12 adds an explicit `native-extrude-v1` profile-inspection/read and finite `feature_create` extrusion subset, documented in `docs/R12-EXTRUDE-CONTRACT.md`. Exact SDK profile/feature/body references remain beneath the owner-thread sketch lease. Prepared metadata crosses the journal worker; SDK pointers do not. One durable start precedes one feature-add invocation. Successful readback checks exact membership, feature health, solid volume, extent and transformed center of mass; any post-invocation uncertainty retains observations without retry or rollback. Product geometric scratch cleanup is still unsupported.

R9-fix1 freeze5 BOM offline guards and immediate diagnostics are propagated as four exact files after checking the inherited copies against original r9; only BOM capability wording/online flag are merged into changed execution code. This carries forward qualified offline refusal, not an unperformed successful assembly campaign. Parent owns independent review, Windows/native SDK execution, live fixtures and publication gates. Fresh reviewer agents are unavailable under the thread limit; reused roles are disclosed rather than described as fresh-model independence.

## R15 read-only purchased-command discovery

Exact R14 freeze1 copied without targets/live receipts. Add bounded native McMaster command registration/design-intent observation with an explicit discovery-only contract. No supplier launch/download/import. Add typed read-only document-selector refusal; cached malformed/foreign/absent selectors never invoke SDK or material backend, successful fresh retirement never enters the operation backend, genuine incomplete refresh remains unavailable. Write paths and journal semantics unchanged. Full author Linux workspace plus binding/SDK syntax checks precede parent review and Windows/live gates. At freeze1, the M12-fix2 profile-getter ordering correction remained a separate pending propagation; the amendment below records freeze2 integration. R15 does not retroactively resolve the original unknown operation.


## R15 freeze2 source-only propagation sequencing

Parent authorized the reviewed six-file M12-fix2 correction to proceed before fresh actual-app acceptance, because the app roster is changing and app effects are gated. Freeze1 is archived and hash-verified; three exact source copies plus three narrow merges preserve R13 identity fields/equality and R14/R15 typed APIs. Full source/build tests and independent parent merge review precede any Windows/app use. Actual-app status stays PENDING; original unknown journal and old fixtures remain untouched.

## R19 finite driving length dimension

Start from exact R18 freeze2 and preserve the full baseline. Add a separate
endpoint/dimension projection and one driving aligned numeric-mm distance on the
exact previously returned H/V relation's isolated line. Publish exact ordered
point refs before admission; retain both returned typed dimension and driving
model parameter before follow-up getters/setter. Journal executing precedes the
add; add and value-set stages stay separately visible. Any post-add failure or
lost outcome remains unknown with retained prefix/children, no rollback/replay.
Pure/native author tests are distinct from root's independent source, Windows
and actual Fusion qualification. Record each later R18 SDK-precondition fix
explicitly; no frozen historical result is promoted or rewritten.
