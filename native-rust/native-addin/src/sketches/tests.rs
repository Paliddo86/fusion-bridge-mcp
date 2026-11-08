use super::*;
use std::cell::{Cell, RefCell};
const DOC: &str = "d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001";
#[derive(Default)]
pub(crate) struct Fake {
    pub(crate) creates: Cell<usize>,
    reads: Cell<usize>,
    revision: Cell<u64>,
    plane: Cell<u32>,
    pub(crate) mode: Cell<u32>,
    changed: Cell<bool>,
    retired: Cell<bool>,
    name: RefCell<String>,
}
impl Backend for Fake {
    type Handle = usize;
    fn root(&self) -> Result<RootState, String> {
        Ok(RootState {
            active: true,
            parametric: true,
            hybrid: true,
            configured: false,
            root_revision: format!("r{}", self.revision.get()),
            sketch_count: self.creates.get() as u64,
            timeline_count: self.creates.get() as u64,
            timeline_position: self.creates.get() as u64,
        })
    }
    fn command(&self) -> Result<String, String> {
        Ok(if self.mode.get() == 9 {
            "CommandBusy"
        } else {
            "SelectCommand"
        }
        .into())
    }
    fn create(&self, plane: u32) -> CreateAttempt<usize> {
        if self.mode.get() == 8 {
            panic!("lost create acknowledgment");
        }
        if self.mode.get() == 7 {
            return CreateAttempt {
                handle: None,
                invoked: false,
                acknowledged: false,
            };
        }
        self.creates.set(self.creates.get() + 1);
        self.revision.set(self.revision.get() + 1);
        self.plane.set(plane);
        CreateAttempt {
            handle: if self.mode.get() == 6 {
                None
            } else {
                Some(self.creates.get())
            },
            invoked: true,
            acknowledged: self.mode.get() != 1,
        }
    }
    fn read(&self, handle: &usize) -> Result<SketchState, String> {
        self.reads.set(self.reads.get() + 1);
        if self.retired.get() || self.mode.get() == 2 {
            return Err("object invalid or not member".into());
        }
        let mut state = SketchState {
            name: self.name.borrow().clone(),
            sdk_revision: format!("s{handle}-{}", self.changed.get()),
            plane: self.plane.get(),
            transform: [0.; 16],
            curves: 0,
            points: 1,
            profiles: 0,
            dimensions: 0,
            constraints: 0,
            health: 0,
            deferred: false,
        };
        if self.mode.get() == 3 {
            state.curves = 1;
        }
        if self.mode.get() == 10 && self.reads.get() > 2 {
            state.curves = 1;
        }
        if self.mode.get() == 4 {
            state.transform[0] = f64::NAN;
        }
        if self.mode.get() == 5 {
            state.sdk_revision = self.reads.get().to_string();
        }
        Ok(state)
    }
}
fn request(book: &Book<usize>, sdk: &Fake, plane: &str) -> PreparedRequest {
    let revision = book.inspect(sdk, DOC, None).unwrap()["target"]["revision"].clone();
    fusion_mechanical_core::prepare_request(json!({"op":"sketch_create","sessionId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","operationId":"sketch-once","target":{"documentId":DOC,"revision":revision},"plane":plane}),"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap()
}
fn create(book: &mut Book<usize>, sdk: &Fake, plane: &str) -> Value {
    let p = book.preflight(sdk, &request(book, sdk, plane)).unwrap();
    book.execute(sdk, p)
}
#[test]
fn three_origin_planes_return_exact_retained_ids_and_observed_counts() {
    for plane in ["xy", "xz", "yz"] {
        let mut b = Book::default();
        let sdk = Fake::default();
        let out = create(&mut b, &sdk, plane);
        assert_eq!(out["success"], true);
        assert_eq!(out["data"]["sketch"]["points"], 1);
        assert_eq!(out["data"]["complete"], false);
        assert_eq!(out["data"]["geometryEnumerated"], false);
        let id = out["created"]["sketchId"].as_str().unwrap();
        assert_eq!(b.inspect(&sdk, DOC, Some(id)).unwrap()["sketchId"], id);
        assert_eq!(sdk.creates.get(), 1);
    }
}
#[test]
fn stale_plan_or_command_refuses_before_sdk_create() {
    let mut b = Book::default();
    let sdk = Fake::default();
    let p = b.preflight(&sdk, &request(&b, &sdk, "xy")).unwrap();
    sdk.revision.set(1);
    assert_eq!(b.execute(&sdk, p)["errorCode"], "target_revision_mismatch");
    assert_eq!(sdk.creates.get(), 0);
    let p = b.preflight(&sdk, &request(&b, &sdk, "xy")).unwrap();
    sdk.mode.set(9);
    assert_eq!(b.execute(&sdk, p)["nativeExecutionStarted"], false);
    assert_eq!(sdk.creates.get(), 0);
}
#[test]
fn rename_and_sketch_edit_change_projection_without_changing_identity() {
    let mut b = Book::default();
    let sdk = Fake::default();
    let out = create(&mut b, &sdk, "xy");
    let id = out["created"]["sketchId"].as_str().unwrap();
    let initial = b.inspect(&sdk, DOC, Some(id)).unwrap();
    sdk.name.replace("renamed".into());
    sdk.changed.set(true);
    let next = b.inspect(&sdk, DOC, Some(id)).unwrap();
    assert_eq!(next["sketchId"], id);
    assert_ne!(next["target"]["revision"], initial["target"]["revision"]);
}
#[test]
fn retired_object_or_wrong_document_never_retargets_same_name() {
    let mut b = Book::default();
    let sdk = Fake::default();
    let out = create(&mut b, &sdk, "xy");
    let id = out["created"]["sketchId"].as_str().unwrap();
    assert!(b.inspect(&sdk, "another-document", Some(id)).is_err());
    sdk.retired.set(true);
    assert!(b.inspect(&sdk, DOC, Some(id)).is_err());
    assert_eq!(sdk.creates.get(), 1);
}
#[test]
fn failures_after_invocation_are_unknown_and_poison_future_effects() {
    for mode in 1..=6 {
        let mut b = Book::default();
        let sdk = Fake::default();
        let p = b.preflight(&sdk, &request(&b, &sdk, "xy")).unwrap();
        sdk.mode.set(mode);
        let out = b.execute(&sdk, p);
        assert_eq!(out["outcome"], "unknown", "{mode}: {out}");
        assert_eq!(sdk.creates.get(), 1);
        assert!(b.inspect(&sdk, DOC, None).is_err());
        let mut released = 0;
        let close = b.close(|_| {
            released += 1;
            Ok(())
        });
        if mode == 6 {
            assert!(close.is_err());
            assert_eq!(released, 0);
        } else {
            assert!(close.is_ok());
            assert_eq!(released, 1);
        }
    }
}
#[test]
fn failed_read_and_release_do_not_retry_or_allow_parent_cleanup() {
    let mut b = Book::default();
    let sdk = Fake::default();
    create(&mut b, &sdk, "xy");
    let mut releases = 0;
    assert!(b
        .close(|_| {
            releases += 1;
            Err("uncertain release".into())
        })
        .is_err());
    assert!(b
        .close(|_| {
            releases += 1;
            Ok(())
        })
        .is_err());
    assert_eq!(releases, 1);
    assert!(b.ensure_released().is_err());
}
#[test]
fn release_panic_and_create_panic_keep_cleanup_fenced() {
    let mut b = Book::default();
    let sdk = Fake::default();
    create(&mut b, &sdk, "xy");
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        || b.close(|_| panic!("release"))
    ))
    .is_err());
    assert!(b.close(|_| Ok(())).is_err());
    let mut b = Book::default();
    let p = b.preflight(&sdk, &request(&b, &sdk, "xy")).unwrap();
    sdk.mode.set(8);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.execute(&sdk, p))).is_err());
    assert!(b.close(|_| Ok(())).is_err());
}
#[test]
fn hard_capacity_refuses_before_add_without_eviction() {
    let mut b = Book::default();
    let sdk = Fake::default();
    let first = create(&mut b, &sdk, "xy");
    for _ in 1..MAX_RETAINED {
        assert_eq!(create(&mut b, &sdk, "xz")["success"], true);
    }
    assert_eq!(
        b.preflight(&sdk, &request(&b, &sdk, "yz")).unwrap_err(),
        "native_sketch_capacity"
    );
    assert_eq!(sdk.creates.get(), MAX_RETAINED);
    assert!(b
        .inspect(&sdk, DOC, first["created"]["sketchId"].as_str())
        .is_ok());
}
#[test]
fn noninvoked_failure_consumes_bounded_slot_without_false_effect() {
    let mut b = Book::default();
    let sdk = Fake::default();
    let p = b.preflight(&sdk, &request(&b, &sdk, "xy")).unwrap();
    sdk.mode.set(7);
    let out = b.execute(&sdk, p);
    assert_eq!(out["nativeExecutionStarted"], false);
    assert_eq!(sdk.creates.get(), 0);
    sdk.mode.set(0);
    let out = create(&mut b, &sdk, "xy");
    assert!(out["created"]["sketchId"]
        .as_str()
        .unwrap()
        .ends_with("0000000000000002"));
}
#[test]
fn release_all_exact_handles_once_and_idempotent_confirmed_close() {
    let mut b = Book::default();
    let sdk = Fake::default();
    for _ in 0..3 {
        create(&mut b, &sdk, "xy");
    }
    let mut released = vec![];
    b.close(|h| {
        released.push(h);
        Ok(())
    })
    .unwrap();
    b.close(|_| panic!("retry")).unwrap();
    assert_eq!(released, vec![1, 2, 3]);
    assert!(b.ensure_released().is_ok());
    assert!(b.inspect(&sdk, DOC, None).is_err());
}
#[test]
fn capability_subset_rejects_name_component_plane_and_wrong_revision() {
    let b = Book::default();
    let sdk = Fake::default();
    let base = request(&b, &sdk, "xy");
    for (key, value) in [
        ("name", json!("requested")),
        (
            "component",
            json!({"documentId":DOC,"revision":"x","objectType":"Component"}),
        ),
        (
            "plane",
            json!({"documentId":DOC,"revision":"x","objectType":"ConstructionPlane"}),
        ),
    ] {
        let mut v = base.arguments().clone();
        v[key] = value;
        let p =
            fusion_mechanical_core::prepare_request(v, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        assert!(supported_arguments(&p).is_err());
    }
}
#[test]
fn pure_geometry_preparation_does_not_create_an_sdk_effect() {
    let sdk = Fake::default();
    let request =
        serde_json::from_value(json!({"items":[{"type":"circle","center":[10,20],"radius":5}]}))
            .unwrap();
    let prepared = geometry_preparation::prepare(&request).unwrap();
    assert_eq!(prepared.entity_count(), 1);
    assert_eq!(sdk.creates.get(), 0);
    assert!(crate::execution::supported("sketch_geometry"));
}
#[test]
fn observed_invalidity_retires_id_even_if_backend_later_reports_valid() {
    let mut b = Book::default();
    let sdk = Fake::default();
    let out = create(&mut b, &sdk, "xy");
    let id = out["created"]["sketchId"].as_str().unwrap();
    sdk.retired.set(true);
    assert!(b.inspect(&sdk, DOC, Some(id)).is_err());
    let reads = sdk.reads.get();
    sdk.retired.set(false);
    assert_eq!(
        b.inspect(&sdk, DOC, Some(id)).unwrap_err(),
        "native_sketch_id_retired"
    );
    assert_eq!(sdk.reads.get(), reads);
}

#[test]
fn changed_final_readback_cannot_bypass_validated_empty_sketch_postconditions() {
    let mut book = Book::default();
    let sdk = Fake::default();
    let plan = book.preflight(&sdk, &request(&book, &sdk, "xy")).unwrap();
    sdk.mode.set(10);
    let result = book.execute(&sdk, plan);
    assert_eq!(result["outcome"], "unknown");
    assert_eq!(
        result["errorCode"],
        "native_sketch_final_projection_changed"
    );
    assert_eq!(sdk.creates.get(), 1);
    assert_eq!(sdk.reads.get(), 4);
    assert!(book.inspect(&sdk, DOC, None).is_err());
}
