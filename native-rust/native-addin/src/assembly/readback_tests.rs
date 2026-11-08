use super::tests::{prepare, Fake, DOC};
use super::*;
use std::cell::Cell;
struct Script {
    inner: Fake,
    mode: u32,
    captures: Cell<usize>,
}
impl Script {
    fn new(mode: u32) -> Self {
        Self {
            inner: Fake::default(),
            mode,
            captures: Cell::new(0),
        }
    }
}
impl Backend for Script {
    type Handle = u64;
    fn open(&self) -> Open<u64> {
        self.inner.open()
    }
    fn command(&self) -> Result<String> {
        self.inner.command()
    }
    fn snapshot(&self, h: &u64, cloud: bool) -> Result<Projection> {
        self.inner.snapshot(h, cloud)
    }
    fn add(&self, h: &u64, c: Option<usize>) -> Attempt {
        self.inner.add(h, c)
    }
    fn change(&self, h: &u64, c: usize, v: &Change) -> Attempt {
        self.inner.change(h, c, v)
    }
    fn capture(&self, h: &u64, cloud: bool) -> Result<Capture> {
        let n = self.captures.get() + 1;
        self.captures.set(n);
        if (self.mode == 3 && n == 1) || (self.mode == 9 && n == 2) || (self.mode == 16 && n == 2) {
            return Err("scripted_sdk_failure".into());
        }
        if self.mode == 10 && n == 2 {
            panic!("readback panic after first retained capture");
        }
        let mut p = self.inner.snapshot(h, cloud)?;
        let mut after = p.root.clone();
        if n == 1 && ![0, 2, 3, 8, 13, 16, 19].contains(&self.mode) {
            after.revision = "settled-root".into();
            self.inner.state.borrow_mut().root.revision = after.revision.clone();
        }
        match (self.mode, n) {
            (2, 1) => after.name = "unrelated root name".into(),
            (4, 2) | (8, 2) | (14, 3) => after.revision = "second-changing-root".into(),
            (5, 2) | (7, 3) => {
                p.components[0]
                    .attributes
                    .insert("foreign".into(), "true".into());
            }
            (6, 2) => {
                p.root.revision = "different-between-captures".into();
                after = p.root.clone();
            }
            (11, 1) => after.revision.clear(),
            (12, 1) => {
                p.root.name = "\u{1}".repeat(4096);
                after.name = p.root.name.clone();
            }
            (13, 1) => p.cloud_metadata = !cloud,
            (15, 2) => p.components[0].revision = "foreign-component-revision".into(),
            (17, 1) => after.pending_snapshot = true,
            (18..=23, 2) => {
                for (i, c) in p.components.iter_mut().enumerate() {
                    c.revision = if self.mode == 23 {
                        "\u{1}".repeat(4096)
                    } else {
                        format!("settled-component-{i}")
                    };
                }
                if self.mode == 21 {
                    p.components[0].name = "changed-name".into();
                }
                self.inner.state.borrow_mut().components = p.components.clone();
            }
            (20, 3) => p.components[0].revision = "changed-later".into(),
            _ => {}
        }
        Ok(Capture {
            projection: p,
            root_after: after,
        })
    }
}
fn execute(mode: u32) -> (Value, Script, Book<u64>) {
    let b = Script::new(mode);
    let mut book = Book::default();
    let s = book.inspect(&b, DOC, None, false).unwrap();
    let r = prepare(
        json!({"op":"component_create","sessionId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","operationId":"settle","target":s["data"]["target"],"name":"A","metadata":{"supplier":"S"}}),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    let plan = book.preflight(&b, &r).unwrap();
    assert_eq!(b.captures.get(), 0);
    let result = book.execute(&b, plan);
    (result, b, book)
}
#[test]
fn stable_path_uses_two_and_revision_only_first_path_uses_three_without_effect_retry() {
    for (mode, reads, settled) in [(0, 2, false), (1, 3, true)] {
        let (v, b, _) = execute(mode);
        assert_eq!(v["success"], true, "{v}");
        assert_eq!(b.captures.get(), reads);
        assert_eq!(b.inner.add_calls.get(), 1);
        assert_eq!(b.inner.change_calls.get(), 2);
        assert_eq!(v["readbackEvidence"]["settlingUsed"], settled);
        assert_eq!(v["readbackEvidence"]["captureAttempts"], reads);
        assert_eq!(v["stages"].as_array().unwrap().len(), 3);
        if settled {
            let e = &v["readbackEvidence"]["captures"];
            assert_eq!(
                e[0]["rootChangeDiagnostic"]["changedFields"],
                json!(["revision"])
            );
            assert_eq!(e[1]["rootBeforeSha256"], e[0]["rootAfterSha256"]);
            assert_eq!(e[1]["projectionSha256"], e[2]["projectionSha256"]);
            assert_eq!(v["data"]["root"]["revision"], "settled-root");
        }
    }
}
#[test]
fn every_instability_or_sdk_error_refuses_at_first_failure_with_stage_prefix_and_no_replay() {
    for (mode, reads) in [
        (2, 1),
        (3, 1),
        (4, 2),
        (5, 2),
        (6, 2),
        (7, 3),
        (8, 2),
        (9, 2),
        (10, 2),
        (11, 1),
        (12, 1),
        (13, 1),
        (14, 3),
        (15, 3),
        (16, 2),
        (17, 1),
        (19, 2),
        (20, 3),
        (21, 2),
        (23, 2),
    ] {
        let (v, b, mut book) = execute(mode);
        assert_eq!(v["success"], false, "mode{mode}:{v}");
        assert_eq!(v["outcome"], "unknown");
        assert_eq!(v["reconciliationRequired"], true);
        assert_eq!(v["nativeExecutionStarted"], true);
        assert_eq!(b.captures.get(), reads, "mode{mode}");
        assert_eq!(b.inner.add_calls.get(), 1);
        assert_eq!(b.inner.change_calls.get(), 2);
        let stages = v["stages"].as_array().unwrap();
        assert_eq!(stages.len(), 3);
        assert!(stages
            .iter()
            .all(|s| s["attempt"]["invoked"] == true && s["attempt"]["acknowledged"] == true));
        assert!(book.inspect(&b, DOC, None, false).is_err());
        assert_eq!(b.inner.add_calls.get(), 1);
        assert_eq!(b.inner.change_calls.get(), 2);
        assert_eq!(v["readbackEvidence"]["captureAttempts"], reads);
        if mode == 12 {
            assert_eq!(v["errorCode"], "native_assembly_settling_diagnostic_bound");
            assert_eq!(
                v["readbackEvidence"]["captures"][0]["rootChangeDiagnostic"]["expected"]["name"],
                "\u{1}".repeat(4096)
            );
        }
        if mode == 10 {
            assert_eq!(
                v["readbackEvidence"]["captures"].as_array().unwrap().len(),
                1
            );
            assert_eq!(v["errorCode"], "native_assembly_effect_or_readback_panic");
        }
    }
}
#[test]
fn read_and_preflight_never_use_post_effect_capture_or_settling() {
    let b = Script::new(1);
    b.inner.mode.set(10);
    let mut book = Book::default();
    assert_eq!(
        book.inspect(&b, DOC, None, false).unwrap_err(),
        "native_assembly_projection_changed"
    );
    assert_eq!(b.captures.get(), 0);
    assert_eq!(b.inner.add_calls.get(), 0);
    assert_eq!(b.inner.change_calls.get(), 0);
    let b = Script::new(1);
    let mut book = Book::default();
    let s = book.inspect(&b, DOC, None, false).unwrap();
    let r = prepare(
        json!({"op":"component_create","sessionId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","operationId":"no-settle","target":s["data"]["target"],"name":"A"}),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    b.inner.mode.set(10);
    assert_eq!(
        book.preflight(&b, &r).unwrap_err(),
        "native_assembly_projection_changed"
    );
    assert_eq!(b.captures.get(), 0);
    assert_eq!(b.inner.add_calls.get(), 0);
}
#[test]
fn full_first_projection_is_retained_before_validation_and_fingerprint_binds_it() {
    let (v, _, _) = execute(5);
    let captures = v["readbackEvidence"]["captures"].as_array().unwrap();
    assert_eq!(captures.len(), 2);
    assert_ne!(
        captures[0]["projectionSha256"],
        captures[1]["projectionSha256"]
    );
    assert!(captures[0]["projectionBytes"].as_u64().unwrap() > 0);
    assert_eq!(
        v["errorCode"],
        "native_assembly_settling_projection_changed"
    );
    let (v, _, _) = execute(2);
    assert_eq!(
        v["readbackEvidence"]["captures"][0]["rootChangeDiagnostic"]["changedFields"],
        json!(["name"])
    );
}
#[test]
fn fixed_capture_bound_has_no_second_chance() {
    let b = Script::new(1);
    let mut trace = readback::Trace::new().unwrap();
    for _ in 0..readback::MAX_CAPTURES {
        trace.next(&b, &9, false).unwrap();
    }
    assert_eq!(
        trace.next(&b, &9, false).unwrap_err(),
        "native_assembly_readback_bound"
    );
    assert_eq!(b.captures.get(), 3);
}
#[test]
fn stable_large_root_does_not_invoke_revision_settling_bound() {
    let b = Script::new(0);
    b.inner.state.borrow_mut().root.name = "\u{1}".repeat(4096);
    let mut t = readback::Trace::new().unwrap();
    let p = readback::snapshot(&b, &9, false, &mut t).unwrap();
    assert_eq!(p.root.name.len(), 4096);
    assert_eq!(t.evidence()["settlingUsed"], false);
    assert_eq!(b.captures.get(), 2);
}

#[test]
fn invalid_oversized_or_nonfinite_capture_evidence_stays_bounded_without_panicking() {
    for mode in 0..3 {
        let b = Script::new(0);
        match mode {
            0 => b.inner.state.borrow_mut().root.name = "X".repeat(1024 * 1024),
            1 => {
                let mut p = b.inner.state.borrow_mut();
                p.root.components = 2;
                p.components.push(super::tests::component());
                p.root.occurrences = 1;
                let mut o = super::tests::occurrence(0);
                o.matrix[0] = f64::NAN;
                p.occurrences.push(o);
            }
            _ => {
                let mut p = b.inner.state.borrow_mut();
                p.root.components = 2;
                p.components.push(super::tests::component());
                p.components[0]
                    .attributes
                    .insert("huge".into(), "x".repeat(1024 * 1024));
            }
        }
        let mut trace = readback::Trace::new().unwrap();
        assert!(readback::snapshot(&b, &9, false, &mut trace).is_err());
        let evidence =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| trace.evidence())).unwrap();
        assert_eq!(evidence["captureAttempts"], 1);
        assert!(serde_json::to_vec(&evidence).unwrap().len() < 16 * 1024);
        if mode == 0 || mode == 2 {
            assert_eq!(evidence["captures"][0]["projectionJsonOmitted"], true);
        }
        if mode == 0 {
            assert_eq!(
                evidence["captures"][0]["rootChangeDiagnostic"]["payloadUnavailable"],
                true
            );
        }
        if mode == 1 {
            assert_eq!(evidence["captures"][0]["projectionContainsNonFinite"], true);
        }
    }
}

#[test]
fn projection_mismatch_preserves_first_exact_path_values_and_full_digest() {
    let (v, _, _) = execute(15);
    let d = &v["readbackEvidence"]["firstProjectionDifference"];
    assert_eq!(d["path"], "/components/0/revision");
    assert_eq!(d["expected"]["value"], "foreign-component-revision");
    assert_eq!(d["comparison"], "settled_pair");
    assert_eq!(d["additionalSdkReads"], 0);
    let (v, _, _) = execute(5);
    let d = &v["readbackEvidence"]["firstProjectionDifference"];
    assert_eq!(d["path"], "/components/0/attributes/foreign");
    assert_eq!(d["expected"]["present"], false);
    assert_eq!(d["observed"]["value"], "true");
    let mut a = super::tests::empty();
    a.root.name = "a".repeat(300);
    let mut b = a.clone();
    b.root.name = "b".repeat(301);
    let d = readback::projection_difference(&a, &b, "test").unwrap();
    assert_eq!(d["path"], "/root/name");
    assert_eq!(d["expected"]["payloadUnavailable"], true);
    assert_eq!(d["observed"]["payloadUnavailable"], true);
    assert_eq!(d["observed"]["serializedBytes"], 303);
    assert_eq!(
        d["observed"]["sha256"],
        fusion_mechanical_journal::request_digest(&serde_json::to_vec(&b.root.name).unwrap())
    );
    assert!(serde_json::to_vec(&d).unwrap().len() < 2048);
}
#[test]
fn maximum_permitted_settling_diagnostic_produces_bounded_success_evidence() {
    let b = Script::new(1);
    let mut n = 0;
    for i in 0..4096 {
        let mut before = super::tests::empty().root;
        before.name = "\u{1}".repeat(i);
        let mut after = before.clone();
        after.revision = "settled-root".into();
        if serde_json::to_vec(&json!({"rootChangeDiagnostic":root_capture_diagnostic(&before, &after).unwrap(),"componentRevisionChanges":[]}))
            .unwrap()
            .len()
            > readback::MAX_SETTLING_DIAGNOSTIC_BYTES
        {
            break;
        }
        n = i;
    }
    b.inner.state.borrow_mut().root.name = "\u{1}".repeat(n);
    let mut trace = readback::Trace::new().unwrap();
    readback::snapshot(&b, &9, false, &mut trace).unwrap();
    let e = trace.evidence();
    assert_eq!(e["settlingUsed"], true);
    assert_eq!(e["captures"].as_array().unwrap().len(), 3);
    assert!(
        serde_json::to_vec(&e).unwrap().len() <= readback::MAX_SETTLING_DIAGNOSTIC_BYTES + 2048
    );
}

#[test]
fn initial_root_and_component_revisions_settle_once_then_two_full_captures_agree() {
    let (v, b, _) = execute(18);
    assert_eq!(v["success"], true, "{v}");
    assert_eq!(b.captures.get(), 3);
    assert_eq!(b.inner.add_calls.get(), 1);
    assert_eq!(b.inner.change_calls.get(), 2);
    let e = &v["readbackEvidence"];
    let changes = e["initialComponentRevisionChanges"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["componentSlot"], 0);
    assert_eq!(changes[0]["observed"], "settled-component-0");
    assert_eq!(changes[0]["payloadUnavailable"], false);
    assert_eq!(
        e["captures"][1]["projectionSha256"],
        e["captures"][2]["projectionSha256"]
    );
    assert_eq!(
        v["data"]["components"][0]["revision"],
        "settled-component-0"
    );
}

#[test]
fn combined_revision_diagnostic_bound_keeps_complete_slot_hashes_and_stops_before_third() {
    let (v, b, _) = execute(23);
    assert_eq!(v["errorCode"], "native_assembly_settling_diagnostic_bound");
    assert_eq!(v["outcome"], "unknown");
    assert_eq!(b.captures.get(), 2);
    assert_eq!(b.inner.add_calls.get(), 1);
    assert_eq!(b.inner.change_calls.get(), 2);
    let e = &v["readbackEvidence"];
    assert_eq!(e["componentRevisionPayloadUnavailable"], true);
    let change = &e["initialComponentRevisionChanges"][0];
    assert_eq!(change["observedBytes"], 4096);
    assert_eq!(change["observed"], Value::Null);
    assert_eq!(
        change["observedSha256"],
        fusion_mechanical_journal::request_digest("\u{1}".repeat(4096).as_bytes())
    );
    assert!(serde_json::to_vec(e).unwrap().len() < 16 * 1024);
}

#[test]
fn accepted_capture_stability_does_not_bypass_unrelated_pre_effect_revision_guard() {
    let b = Script::new(18);
    {
        let mut p = b.inner.state.borrow_mut();
        p.root.components = 2;
        p.root.occurrences = 1;
        p.components.push(super::tests::component());
        p.occurrences.push(super::tests::occurrence(0));
    }
    let mut book = Book::default();
    let before = book.inspect(&b, DOC, None, false).unwrap();
    let r = prepare(
        json!({"op":"component_create","sessionId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","operationId":"unrelated-revision","target":before["data"]["target"],"name":"B"}),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    let plan = book.preflight(&b, &r).unwrap();
    let v = book.execute(&b, plan);
    assert_eq!(
        v["errorCode"],
        "native_assembly_unrelated_definition_changed"
    );
    assert_eq!(v["outcome"], "unknown");
    assert_eq!(b.captures.get(), 3);
    assert_eq!(b.inner.add_calls.get(), 1);
    assert_eq!(b.inner.change_calls.get(), 1);
    assert_eq!(
        v["readbackEvidence"]["initialComponentRevisionChanges"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(book.inspect(&b, DOC, None, false).is_err());
    assert_eq!(b.inner.add_calls.get(), 1);
}

#[test]
fn maximum_component_revision_diagnostics_preserve_every_changed_slot_with_bounded_payload() {
    let b = Script::new(22);
    {
        let mut p = b.inner.state.borrow_mut();
        p.root.components = MAX_COMPONENTS as u64 + 1;
        p.root.occurrences = MAX_COMPONENTS as u64;
        for i in 0..MAX_COMPONENTS {
            let mut c = super::tests::component();
            c.revision = format!("initial-component-{i}");
            p.components.push(c);
            p.occurrences.push(super::tests::occurrence(i));
        }
    }
    let mut trace = readback::Trace::new().unwrap();
    assert_eq!(
        readback::snapshot(&b, &9, false, &mut trace).unwrap_err(),
        "native_assembly_settling_diagnostic_bound"
    );
    let e = trace.evidence();
    let changes = e["initialComponentRevisionChanges"].as_array().unwrap();
    assert_eq!(changes.len(), MAX_COMPONENTS);
    for (i, c) in changes.iter().enumerate() {
        assert_eq!(c["componentSlot"], i);
        assert_eq!(
            c["expectedSha256"],
            fusion_mechanical_journal::request_digest(format!("initial-component-{i}").as_bytes())
        );
        assert_eq!(
            c["observedSha256"],
            fusion_mechanical_journal::request_digest(format!("settled-component-{i}").as_bytes())
        );
        assert_eq!(c["payloadUnavailable"], true);
    }
    assert!(serde_json::to_vec(&e).unwrap().len() < 32 * 1024);
    assert_eq!(b.captures.get(), 2);
    assert_eq!(b.inner.add_calls.get(), 0);
}
