# R20 first assembly capture diagnostic

This is an instrument-only copy of the exact 178-file R20 product freeze `bd1d41d25dc8984289f96d96aa9588208e7325784b1b7884b5064996381daff0`. The baseline manifest and exact original source are preserved under `BASELINE-R20-PRODUCT-FREEZE1.txt` and `evidence/baseline-source`.

The first live component-create response reported `native_assembly_root_changed_during_capture` after seven acknowledged stages. The old response did not record the differing root field. Its operation remains executing/observed unknown; neither this source change nor a later observer reconciles it.

The adapter reads exactly the same first root, component/attribute/occurrence properties, and final root as before. The strict `Root` equality comparison and error code are unchanged. On inequality, a pure Rust helper now puts both already-read roots and the changed field names into the existing SDK diagnostic result field. `additionalSdkReads` is zero, and no new SDK invocation, effect retry, tolerance, recovery, rollback or admission change is introduced. An SDK error while obtaining either root follows the existing SDK-error path.

Two portable tests cover equality/no diagnostic, revision-only drift, and multiple nonrevision differences. The diagnostic does not invent an outcome or execution-start classification; existing journal/effect handling remains authoritative. Source-only validation is separate from MSVC and actual Fusion qualification.
