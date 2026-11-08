# Explicit extrusion start input

This isolated checkpoint copies the 103 files in r12 product freeze 1 exactly
before editing. BASELINE-R12-PRODUCT-FREEZE1.txt is SHA256
17e32b0a851d1a299ccd68a6d1a0b4019ec4df957d1be037df2d2fdf4539ea8d;
evidence/baseline-verification.log records every file passing. Vendor is a link
to the existing offline vendor tree. No targets, live receipts, verification
harnesses or installed artifacts were copied or operated.

The actual r12 live request57 failed at stage208, ABI7/SDK2,
`InternalValidationError : res`. Original immutable evidence is
`../rust-native-migration-r12/evidence/native-geometry-live1/campaign/57-circle-extrude-result.json`,
SHA256 44f8726c32dcef1caf3b402b3e3d17449b49532103deb4d2c8f69d083d9e164e.
Its durable terminal record says failed, nativeExecutionStarted:false, and no
reconciliation required. Stage208 comprises the input startExtent getter,
conversion to ProfilePlaneStartDefinition, and immediate SDK error check. The
receipt does not isolate which internal getter/conversion step failed.

Replace that observation of the default input with an explicit
ProfilePlaneStartDefinition::create (new stage213) and checked
input->startExtent(start) assignment (214). The installed
Fusion/Features/ProfilePlaneStartDefinition.h documents create as feature input;
ExtrudeFeatureInput.h:188-194 documents the property and default profile plane.
[Official input property](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_ExtrudeFeatureInput_startExtent.htm)

There is no error clearing, fallback, retry, altered geometric target, or change
to journal policy. Failure of either input-construction stage precedes the one
add invocation. Stage209 still marks/retains the exact actual feature creation.
The actual feature's startExtent getter/readback at320 and all ownership,
direction, health, volume/centroid and final-projection checks remain unchanged.
The new native stage labels classify213/214 as feature_input.

Three new Rust regressions exercise both explicit-input failure boundaries and
diagnostics, durable terminal refusal replay after a later backend would succeed,
and a non-profile-plane actual result remaining unknown/faulted. They validate
the behavioral contract, not Autodesk's runtime implementation of the new setter.
Installed-SDK syntax compilation verifies the typed calls; actual Windows link
and live qualification are separate root-owned gates.

Local checks:179 native tests and2 compile-fail doctests pass; Windows GNU tests
and native-bindings check pass; rustfmt passes. Adapted installed-SDK MinGW syntax
check passes with __declspec redefinition and SDK std::iterator deprecation
warnings. No host or Fusion operation by the author. The original r12 product
and evidence are preserved unchanged.
