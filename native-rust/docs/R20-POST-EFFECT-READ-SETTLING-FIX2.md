# Bounded assembly post-effect read settling

R20-fix1's fresh actual component-create returned an unknown result after seven acknowledged stages. Its diagnostic identified only `root.revision` changing across the first post-effect capture. All other 20 root fields were equal. The receipt is preserved at the root-owned `r20-fix1-1` campaign's `24-a-result.json`; the old journal remains executing/observed unknown. A later stable observer cannot reconstruct why that historical revision changed. This correction makes no claim about the underlying SDK mechanism.

The exact 180-file R20-fix1 baseline is archived under `evidence/baseline-source`, bound by `BASELINE-R20-FIX1-PRODUCT-FREEZE1.txt`. No C++ ABI/getter, effect order, journal format, opaque identity, capacity, metadata mode or earlier finite API changes.

## Policy

Only the acknowledged post-effect readback path uses the following rule. Ordinary assembly inspection, preflight and the final validation before effects still require the original strict two complete captures. The SDK adapter exposes its existing raw capture without changing any getter order; its existing strict `snapshot` continues to enforce equality for those earlier paths.

1. Reserve three complete capture slots and the effect-stage buffer before any possible SDK effect.
2. Capture the whole projection and its final root, retaining it before validation. Each capture is preceded by the existing observed `SelectCommand` guard. SDK errors, invalid mode, malformed values or unsupported state stop immediately.
3. If the initial root bracket is stable, use the original two-capture rule. A later difference never gets a settling opportunity.
4. If the first bracket differs, replace only the first projection's root revision with its final observed root revision. That normalized root must exactly equal the final root, including every nonrevision field. The full changed-root diagnostic must fit 8 KiB; otherwise refuse before another capture.
5. The next capture must be internally stable and exactly equal the entire normalized first projection. In particular its root equals the first final root, and every component revision, name, attribute, occurrence identity/matrix and resource fact is unchanged.
6. One final capture must be internally stable and exactly equal the preceding entire projection. Three is the hard maximum; any change or SDK failure stops with unknown outcome. There is no second settling opportunity.

Every effect still occurs at most once after durable journal start. Failure poisons the authoring book and preserves the acknowledged prefix; no retry, compensation, rollback, SDK-error reset or implicit snapshot action is introduced. Existing operation replay and uncertainty handling remain authoritative. A fresh successful campaign cannot convert either previous unknown operation into success.

## Evidence and bounds

All returned complete projections remain in the reserved in-process trace through validation/result construction. The public observation includes each bounded projection's SHA256 and byte count, bracket stability and root hashes. Failed brackets include both complete bounded roots and changed field names. Invalid oversized root strings are explicitly omitted with lengths/hashes. No duplicate full model projections are added to the durable response.

For a full-projection mismatch, `firstProjectionDifference` gives the first JSON-pointer path and expected/observed value SHA256 plus serialized length. Scalar payload and encoded path each have a 256-byte exposure limit; larger values are explicitly unavailable. Missing values are distinct from null. This applies to the ordinary stable pair, normalized-first/second pair and settled second/third pair. It performs no additional SDK reads.

Serialization of invalid observations uses fixed-cap writers. Trace rendering is guarded; a rendering panic produces explicit unavailable evidence and an unknown result while preserving effect stages. Successful settling adds at most an 8 KiB root diagnostic plus tested bounded hash overhead. The maximum published-reference/result regression includes that overhead and remains below the native retained result's 256 KiB limit. Bound failures never imply a successful effect or justify replay.

## Validation and remaining gates

Nine new portable tests cover stable two-capture success, revision-only three-capture success, exact add/setter counts, first/later root and component/metadata changes, errors/panic, mode mismatch, maximum captures, diagnostic/result budgets, oversized and nonfinite malformed observations, and bounded first-difference witnesses. Existing journal replay/conflict/late persistence tests remain in the full suite. The focused assembly run passed 37 tests; the full Linux workspace passed 490 including 312 native tests and four compile-fail doctests. GNU Windows workspace and native `--tests` bindings, installed SDK syntax and formatting passed.

These are author-side source tests, not independent behavioral verification. Root performs independent source review, actual Windows build/tests and a fresh exact-authority Fusion campaign. Positive applicability, complete two-definition/three-occurrence qualification, online metadata and BOM/CSV qualification remain separate actual-app gates. This source author performed no host operations.
