# Native Rust development checkpoint, 16 October 2026

This draft contains R21 development source. It is an isolated workspace and does not replace the existing bridge's build, installer, runtime selection, package manifest or release channel.

The product's 193 files match [PRODUCT-SOURCE-SHA256.txt](PRODUCT-SOURCE-SHA256.txt), SHA-256 c2b42e34335a2cc49a1762b3bd140dffe542d544aa7e81721d87449082cbe197. R21 has 18 finite subsets of the 61 legacy mechanical operations and nine native helpers. Full legacy parity is not established. The new subset is absolute translation of an exact owned empty root occurrence, with a captured position; rotations, grounding changes, nested/physical assemblies and general placement are not included.

## Verification state

R21 passed 508 Linux and 512 Windows tests, binding and installed SDK checks, and release DLL linking. Its protocol verifier passed 28 tests on each platform. The separate public-SDK placement observer passed 20 tests on each platform and SDK linking. Independent source reviews passed. R21 has not yet been run in Fusion; actual pose/snapshot qualification, semantic archive/reopen persistence and package/regression release gates remain pending.

The previous R20-fix3 actual Fusion assembly result remains a separately bound historical result: 52 requests, six durable successes, zero uncertain attempts and independent public-SDK checks of two empty definitions/three shared occurrences, local metadata and identity poses. See [that receipt](../docs/mechanical-progress/r20-assembly-qualification.json). It does not qualify R21 translations or reconcile failed historical attempts.

The new contract-authoring tool ports catalog expansion and contract freezing to Rust, including explicit changed-catalog refreezing and fresh output bundles. All 21 tool tests pass on Linux and Windows, including actual CLI and atomic publication. All 665,236 historical contract bytes match. It uses a caller-supplied date and stricter malformed-input validation. Its historical Python files are inert test fixtures, never executed. Release-CI adoption and macOS atomic-publication support remain pending. See [tool instructions](tools/contract-authoring/README.md).

Rust owns policy, transport, dispatch, identities, journals and results. Typed C++ glue invokes Autodesk's SDK. The preserved installed bridge still contains Python. Current priority is complete existing-behavior conversion before additional features or the AI Flow evidence adapter.

## Source layout and builds

The product, placement protocol verifier, observer and authoring tool each have source manifests. Older baseline manifests, READMEs and ledgers retain their original dated statements; use this document and [current progress](../docs/mechanical-progress/README.md) for current qualification claims. The previous C20 assembly verifier remains for historical reproduction.

Vendored dependencies, build products, SDK headers/libraries, live authorities, runtime credentials and active journals are excluded. In each isolated workspace, bootstrap with cargo vendor --locked vendor before cargo test --workspace --locked --offline. Do not use --respect-source-config while the vendor directory is absent. The placement verifier additionally uses the exact product mechanical crate through its relative path. Windows native DLL builds require the installed Autodesk Fusion C++ SDK; see each build.rs.

Live test clients require fresh explicit fixture authority and exact host/process/document checks. Passing portable tests is not authority to mutate an arbitrary design. No installable release is created by this source publication.
