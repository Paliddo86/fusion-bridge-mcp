use super::*;
use std::cell::Cell;
const EPOCH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DOC: &str = "d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001";
fn request(extra: Value) -> PreparedRequest {
    let mut value = json!({"op":"mcmaster_discover","sessionId":EPOCH,"target":{"documentId":DOC}});
    for (key, value2) in extra.as_object().unwrap() {
        value[key] = value2.clone();
    }
    fusion_mechanical_core::prepare_request(value, EPOCH).unwrap()
}
struct Fake {
    snapshot: Snapshot,
    calls: Cell<usize>,
    fail: usize,
    changed: bool,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            snapshot: Snapshot {
                design_intent: 2,
                command: Some(Command {
                    id: COMMAND_ID.into(),
                    name: "McMaster-Carr".into(),
                    native: true,
                }),
            },
            calls: Cell::new(0),
            fail: 0,
            changed: false,
        }
    }
}
impl Backend for Fake {
    fn snapshot(&self) -> Result<Snapshot> {
        let n = self.calls.get() + 1;
        self.calls.set(n);
        if self.fail == n {
            return Err("sdk exact diagnostic".into());
        }
        let mut s = self.snapshot.clone();
        if self.changed && n == 2 {
            s.design_intent = 1;
        }
        Ok(s)
    }
}
#[test]
fn native_registration_is_read_only_scoped_and_not_applicability() {
    let b = Fake::default();
    let q = request(json!({}));
    assert!(!q.is_write());
    let out = inspect(&b, EPOCH, DOC, &q).unwrap();
    assert_eq!(b.calls.get(), 2);
    assert_eq!(out["commandId"], COMMAND_ID);
    assert_eq!(out["available"], true);
    assert_eq!(out["hybridTarget"], true);
    for k in [
        "commandExecutionSupported",
        "downloadSupported",
        "applicabilityVerified",
        "onlineAccessVerified",
    ] {
        assert_eq!(out[k], false)
    }
    assert_eq!(out["downloadedCadAlternative"]["nativeSupported"], false);
    assert_eq!(out["target"]["documentId"], DOC);
    assert_eq!(out["revisionScope"], SCOPE);
}
#[test]
fn absent_command_and_all_intents_are_explicit() {
    for intent in 0..=2 {
        let mut b = Fake::default();
        b.snapshot.design_intent = intent;
        b.snapshot.command = None;
        let out = inspect(&b, EPOCH, DOC, &request(json!({}))).unwrap();
        assert_eq!(out["available"], false);
        assert!(out["name"].is_null());
        assert!(out["definitionIsNative"].is_null());
        assert_eq!(out["hybridTarget"], intent == 2);
        assert_eq!(b.calls.get(), 2);
    }
}
#[test]
fn failed_or_changing_snapshots_never_publish_partial_registration() {
    for fail in 1..=2 {
        let b = Fake {
            fail,
            ..Default::default()
        };
        assert_eq!(
            inspect(&b, EPOCH, DOC, &request(json!({}))).unwrap_err(),
            "sdk exact diagnostic"
        );
        assert_eq!(b.calls.get(), fail);
    }
    let b = Fake {
        changed: true,
        ..Default::default()
    };
    assert_eq!(
        inspect(&b, EPOCH, DOC, &request(json!({}))).unwrap_err(),
        "native_mcmaster_discovery_changed"
    );
    assert_eq!(b.calls.get(), 2);
}
#[test]
fn registration_identity_and_text_limits_are_fail_closed() {
    for mode in 0..5 {
        let mut b = Fake::default();
        match mode {
            0 => b.snapshot.design_intent = 3,
            1 => b.snapshot.command.as_mut().unwrap().id = "lookalike".into(),
            2 => b.snapshot.command.as_mut().unwrap().native = false,
            3 => b.snapshot.command.as_mut().unwrap().name = "x".repeat(4097),
            _ => b.snapshot.command.as_mut().unwrap().name = "bad\0name".into(),
        };
        assert!(inspect(&b, EPOCH, DOC, &request(json!({}))).is_err());
        assert_eq!(b.calls.get(), 1);
    }
    let mut b = Fake::default();
    b.snapshot.command.as_mut().unwrap().name = "x".repeat(4096);
    assert!(inspect(&b, EPOCH, DOC, &request(json!({}))).is_ok());
}
#[test]
fn exact_target_and_session_are_required_before_backend() {
    for extra in [
        json!({"sessionId":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}),
        json!({"target":{}}),
        json!({"target":{"documentId":""}}),
        json!({"target":{"documentId":DOC,"name":"not-identity"}}),
        json!({"target":{"documentId":DOC,"revision":null}}),
    ] {
        let b = Fake::default();
        assert!(inspect(&b, EPOCH, DOC, &request(extra)).is_err());
        assert_eq!(b.calls.get(), 0);
    }
    let b = Fake::default();
    assert!(inspect(&b, EPOCH, "different", &request(json!({}))).is_err());
    assert_eq!(b.calls.get(), 0);
}
#[test]
fn stale_revision_and_changes_outside_available_flag_are_detected() {
    let b = Fake::default();
    let first = inspect(&b, EPOCH, DOC, &request(json!({}))).unwrap();
    assert!(inspect(&b, EPOCH, DOC, &request(json!({"target":first["target"]}))).is_ok());
    let mut changed = Fake::default();
    changed.snapshot.command.as_mut().unwrap().name = "renamed registration".into();
    assert_eq!(
        inspect(
            &changed,
            EPOCH,
            DOC,
            &request(json!({"target":first["target"]}))
        )
        .unwrap_err(),
        "target_revision_mismatch"
    );
    let other = "d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000002";
    let out = inspect(
        &Fake::default(),
        EPOCH,
        other,
        &request(json!({"target":{"documentId":other}})),
    )
    .unwrap();
    assert_ne!(out["target"]["revision"], first["target"]["revision"]);
    let epoch = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let doc = "d:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb:0000000000000001";
    let out = inspect(
        &Fake::default(),
        epoch,
        doc,
        &request(json!({"sessionId":epoch,"target":{"documentId":doc}})),
    )
    .unwrap();
    assert_ne!(out["target"]["revision"], first["target"]["revision"]);
}
