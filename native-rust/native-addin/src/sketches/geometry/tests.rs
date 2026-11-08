use super::*;
use std::cell::{Cell, RefCell};
const DOC: &str = "d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001";
const ID: &str = "s:d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001:0000000000000001";
#[derive(Default)]
pub(crate) struct Fake {
    pub(crate) values: RefCell<Vec<ObservedEntity>>,
    pub(crate) calls: Cell<usize>,
    pub(crate) fail_at: Cell<usize>,
    pub(crate) mode: Cell<u32>,
    pub(crate) changed: Cell<bool>,
    reads: Cell<usize>,
}
impl Backend for Fake {
    type Handle = u64;
    fn root(&self) -> Result<RootState, String> {
        if self.mode.get() == 11 && !self.values.borrow().is_empty() {
            panic!("root getter after effect");
        }
        Ok(RootState {
            active: true,
            parametric: true,
            hybrid: true,
            configured: false,
            root_revision: format!("root-{}", self.values.borrow().len()),
            sketch_count: 1,
            timeline_count: 1,
            timeline_position: 1,
        })
    }
    fn command(&self) -> Result<String, String> {
        if self.mode.get() == 12 && !self.values.borrow().is_empty() {
            panic!("command getter after prefix");
        }
        Ok(if self.mode.get() == 8 {
            "BusyCommand"
        } else {
            "SelectCommand"
        }
        .into())
    }
    fn create(&self, _: u32) -> CreateAttempt<u64> {
        panic!("geometry must never create another sketch")
    }
    fn read(&self, handle: &u64) -> Result<SketchState, String> {
        if self.mode.get() == 13 && !self.values.borrow().is_empty() {
            panic!("sketch getter after effect");
        }
        assert_eq!(*handle, 42);
        self.reads.set(self.reads.get() + 1);
        let values = self.values.borrow();
        Ok(SketchState {
            name: "Sketch1".into(),
            sdk_revision: format!("rev-{}-{}", values.len(), self.changed.get()),
            plane: 1,
            transform: [0.; 16],
            curves: values
                .iter()
                .filter(|v| !matches!(v.geometry, Geometry::Point { .. }))
                .count() as u64,
            points: 1 + values.len() as u64,
            profiles: if values.is_empty() { 0 } else { 2 },
            dimensions: 0,
            constraints: 0,
            health: if self.mode.get() == 7 && !values.is_empty() {
                1
            } else {
                0
            },
            deferred: false,
        })
    }
}
impl GeometryBackend for Fake {
    fn empty_origin_only(&self, handle: &u64) -> Result<bool, String> {
        assert_eq!(*handle, 42);
        Ok(self.values.borrow().is_empty() && self.mode.get() != 9)
    }
    fn insert(&self, handle: &u64, entity: &Entity) -> InsertAttempt {
        assert_eq!(*handle, 42);
        let call = self.calls.get() + 1;
        self.calls.set(call);
        if self.mode.get() == 1 && call == self.fail_at.get() {
            return InsertAttempt {
                lease: None,
                invoked: false,
                acknowledged: false,
                stage: "prepare",
            };
        }
        let mut geometry = entity.geometry.clone();
        if let Geometry::Arc {
            start_cm,
            end_cm,
            sweep_radians,
            ..
        } = &mut geometry
        {
            if *sweep_radians < 0. {
                std::mem::swap(start_cm, end_cm);
                *sweep_radians = -*sweep_radians;
            }
        }
        let index = self.values.borrow().len() as u64;
        self.values.borrow_mut().push(ObservedEntity {
            construction: if matches!(geometry, Geometry::Point { .. }) {
                None
            } else {
                Some(entity.construction.unwrap_or(false))
            },
            geometry,
            reference: false,
        });
        if self.mode.get() == 3 && call == self.fail_at.get() {
            panic!("panic after SDK effect");
        }
        InsertAttempt {
            lease: Some(if self.mode.get() == 10 { 0 } else { index }),
            invoked: true,
            acknowledged: !(self.mode.get() == 2 && call == self.fail_at.get()),
            stage: if self.mode.get() == 2 {
                "construction"
            } else {
                "complete"
            },
        }
    }
    fn entity_equalities(&self, _: &u64, leases: &[u64]) -> Result<Vec<Vec<bool>>, String> {
        Ok(leases
            .iter()
            .map(|a| leases.iter().map(|b| a == b).collect())
            .collect())
    }
    fn entity(&self, _: &u64, lease: u64) -> Result<ObservedEntity, String> {
        if self.mode.get() == 14 && lease == 1 {
            panic!("entity getter after prefix");
        }
        if self.mode.get() == 4 {
            return Err("readback lost".into());
        }
        let mut v = self.values.borrow()[lease as usize].clone();
        if self.mode.get() == 5 {
            v.reference = true;
        }
        if self.mode.get() == 6 && self.values.borrow().len() > 1 && lease == 0 {
            if let Geometry::Point { position_cm } = &mut v.geometry {
                position_cm[0] += 1.;
            }
        }
        Ok(v)
    }
}
pub(crate) fn book() -> Book<u64> {
    let mut b = Book::default();
    b.entries.push(Entry {
        id: ID.into(),
        document_id: DOC.into(),
        handle: Some(42),
        retired: Cell::new(false),
        profile_published: Cell::new(false),
        profile_retired: Cell::new(false),
        geometry_leases: None,
        entities: RefCell::new(None),
        entities_retired: Cell::new(false),
        dimension_endpoints_published: Cell::new(false),
    });
    b
}
pub(crate) fn request(b: &Book<u64>, sdk: &Fake, items: Value, operation: &str) -> PreparedRequest {
    let data = b.inspect(sdk, DOC, Some(ID)).unwrap();
    fusion_mechanical_core::prepare_request(json!({"op":"sketch_geometry","sessionId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","operationId":operation,"target":data["target"],"sketch":{"documentId":DOC,"revision":data["target"]["revision"],"objectType":"Sketch","sketchId":ID},"items":items}),"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap()
}
fn points() -> Value {
    json!([{"type":"point","position":[10,20]},{"type":"point","position":[30,40]}])
}
#[test]
fn all_eight_primitives_have_actual_readback_and_explicit_non_atomic_scope() {
    let mut book = book();
    let sdk = Fake::default();
    let items = json!([
        {"type":"point","position":[10,20]},
        {"type":"line","start":[10,0],"end":[20,0],"construction":true},
        {"type":"rectangle","start":[30,0],"end":[50,10]},
        {"type":"circle","center":[70,0],"radius":10},
        {"type":"arc","center":[100,0],"start":[110,0],"sweep":-90},
        {"type":"polygon","center":[140,0],"radius":10,"sides":3},
        {"type":"slot","start":[160,0],"end":[180,0],"width":10},
        {"type":"spline","points":[[210,0],[220,10],[230,0]],"closed":true}
    ]);
    let request = request(&book, &sdk, items, "all");
    let plan = book.geometry_preflight(&sdk, &request).unwrap();
    assert_eq!(sdk.calls.get(), 0);
    let out = book.execute_geometry(&sdk, plan);
    assert_eq!(out["success"], true, "{out}");
    assert_eq!(sdk.calls.get(), 16);
    assert_eq!(out["atomic"], false);
    assert_eq!(out["geometry"]["itemsCompleted"], 8);
    assert_eq!(out["geometry"]["entitiesCompleted"], 16);
    assert_eq!(out["data"]["sketch"]["profiles"], 2);
    assert_eq!(
        out["geometry"]["observed"][0]["observed"]["geometry"]["position_cm"],
        json!([1., 2., 0.])
    );
    assert_eq!(
        out["geometry"]["observed"][1]["observed"]["construction"],
        true
    );
    assert_eq!(
        out["geometry"]["observed"][6]["observed"]["geometry"]["radius_cm"],
        1.
    );
    let arc = &out["geometry"]["observed"][7]["observed"]["geometry"];
    assert!(near(arc["start_cm"][0].as_f64().unwrap(), 10.));
    assert!(near(arc["start_cm"][1].as_f64().unwrap(), -1.));
    assert_eq!(arc["end_cm"], json!([11., 0., 0.]));
    assert!(near(
        arc["sweep_radians"].as_f64().unwrap(),
        std::f64::consts::FRAC_PI_2
    ));
    assert!(book.inspect(&sdk, DOC, Some(ID)).is_ok());
}
#[test]
fn whole_plan_rejects_late_invalid_geometry_before_any_sdk_effect() {
    let b = book();
    let sdk = Fake::default();
    for items in [
        json!([{"type":"point","position":[1,2]},{"type":"circle","center":[1,2],"radius":0.000001}]),
        json!([{"type":"point","position":[1,2,1]}]),
        json!([{"type":"arc","center":[0,0],"start":[10,0],"sweep":360}]),
        json!([{"type":"line","start":[0,0],"end":[0.000001,0]}]),
    ] {
        assert!(b
            .geometry_preflight(&sdk, &request(&b, &sdk, items, "invalid"))
            .is_err());
    }
    assert_eq!(sdk.calls.get(), 0);
}
#[test]
fn capacity_rejects_expansion_and_item_count_before_effect() {
    let b = book();
    let sdk = Fake::default();
    for items in [
        json!([{"type":"polygon","center":[0,0],"radius":10,"sides":65}]),
        json!(vec![json!({"type":"point","position":[1,2]}); 33]),
    ] {
        assert!(prepare_arguments(&request(&b, &sdk, items, "bound")).is_err());
    }
    assert_eq!(sdk.calls.get(), 0);
}
#[test]
fn exact_ids_and_both_revision_witnesses_are_required() {
    let b = book();
    let sdk = Fake::default();
    let original = request(&b, &sdk, points(), "identity");
    for case in 0..5 {
        let mut a = original.arguments().clone();
        match case {
            0 => a["sketch"]["sketchId"] = json!("same-name-other-object"),
            1 => a["sketch"]["documentId"] = json!("foreign"),
            2 => a["sketch"]["revision"] = json!("stale"),
            3 => a["sketch"]["entityToken"] = json!("unsupported"),
            _ => a["target"]["revision"] = json!("sketch-projection-v1:stale"),
        };
        let r =
            fusion_mechanical_core::prepare_request(a, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        assert!(b.geometry_preflight(&sdk, &r).is_err());
    }
    assert_eq!(sdk.calls.get(), 0);
}
#[test]
fn stale_after_preflight_refuses_without_insertion() {
    let mut b = book();
    let sdk = Fake::default();
    let p = b
        .geometry_preflight(&sdk, &request(&b, &sdk, points(), "stale"))
        .unwrap();
    sdk.changed.set(true);
    let out = b.execute_geometry(&sdk, p);
    assert_eq!(out["nativeExecutionStarted"], false);
    assert_eq!(sdk.calls.get(), 0);
}
#[test]
fn origin_only_guard_refuses_non_curve_geometry_and_text() {
    let b = book();
    let sdk = Fake::default();
    sdk.mode.set(9);
    assert!(b
        .geometry_preflight(&sdk, &request(&b, &sdk, points(), "hidden-content"))
        .is_err());
    assert_eq!(sdk.calls.get(), 0);
}
#[test]
fn pre_invoke_failure_has_no_observed_effect_and_releases_fault_latch() {
    let mut b = book();
    let sdk = Fake::default();
    let p = b
        .geometry_preflight(&sdk, &request(&b, &sdk, points(), "refuse"))
        .unwrap();
    sdk.mode.set(1);
    sdk.fail_at.set(1);
    let out = b.execute_geometry(&sdk, p);
    assert_eq!(out["nativeExecutionStarted"], false);
    assert!(sdk.values.borrow().is_empty());
    assert!(b.inspect(&sdk, DOC, Some(ID)).is_ok());
}
#[test]
fn failed_second_effect_preserves_prefix_and_blocks_new_attempts() {
    for mode in [1, 2] {
        let mut b = book();
        let sdk = Fake::default();
        let r = request(&b, &sdk, points(), "partial");
        let p = b.geometry_preflight(&sdk, &r).unwrap();
        sdk.mode.set(mode);
        sdk.fail_at.set(2);
        let out = b.execute_geometry(&sdk, p);
        assert_eq!(out["outcome"], "unknown");
        assert_eq!(out["completedEntities"].as_array().unwrap().len(), 1);
        assert_eq!(out["failedAttempt"]["inputIndex"], 1);
        assert_eq!(out["rollbackAttempted"], false);
        assert!(b.geometry_preflight(&sdk, &r).is_err());
        assert_eq!(sdk.calls.get(), 2);
    }
}
#[test]
fn lost_readback_reference_and_final_solver_changes_never_report_success() {
    for mode in [4, 5, 6, 7, 10] {
        let mut b = book();
        let sdk = Fake::default();
        let p = b
            .geometry_preflight(&sdk, &request(&b, &sdk, points(), "fault"))
            .unwrap();
        sdk.mode.set(mode);
        let out = b.execute_geometry(&sdk, p);
        assert_eq!(out["outcome"], "unknown", "mode {mode}: {out}");
        assert!(b.inspect(&sdk, DOC, Some(ID)).is_err());
    }
}
#[test]
fn panic_after_effect_latches_book_and_releases_only_exact_parent_once() {
    let mut b = book();
    let sdk = Fake::default();
    let r = request(&b, &sdk, points(), "panic");
    let p = b.geometry_preflight(&sdk, &r).unwrap();
    sdk.mode.set(3);
    sdk.fail_at.set(2);
    let out = b.execute_geometry(&sdk, p);
    assert_eq!(out["outcome"], "unknown");
    assert_eq!(out["errorCode"], "native_geometry_insert_panicked");
    assert_eq!(out["completedEntities"].as_array().unwrap().len(), 1);
    assert_eq!(sdk.values.borrow().len(), 2);
    assert!(b.geometry_preflight(&sdk, &r).is_err());
    let mut releases = vec![];
    b.close(|h| {
        releases.push(h);
        Ok(())
    })
    .unwrap();
    b.close(|_| panic!("release twice")).unwrap();
    assert_eq!(releases, vec![42]);
}
#[test]
fn uncertain_entity_parent_release_blocks_unload_and_retry() {
    let mut b = book();
    let sdk = Fake::default();
    let p = b
        .geometry_preflight(&sdk, &request(&b, &sdk, points(), "created"))
        .unwrap();
    assert_eq!(b.execute_geometry(&sdk, p)["success"], true);
    assert!(b.close(|_| Err("release uncertain".into())).is_err());
    assert!(b.ensure_released().is_err());
    assert!(b.close(|_| panic!("must not retry release")).is_err());
}
#[test]
fn negative_arc_verifier_rejects_wrong_orientation_and_unrelated_equal_endpoints() {
    let expected = Geometry::Arc {
        center_cm: [0., 0., 0.],
        start_cm: [1., 0., 0.],
        end_cm: [0., -1., 0.],
        sweep_radians: -std::f64::consts::FRAC_PI_2,
    };
    let correct = Geometry::Arc {
        center_cm: [0., 0., 0.],
        start_cm: [0., -1., 0.],
        end_cm: [1., 0., 0.],
        sweep_radians: std::f64::consts::FRAC_PI_2,
    };
    assert!(matches_geometry(&expected, &correct));
    assert!(!matches_geometry(&expected, &expected));
    let mut wrong = correct;
    if let Geometry::Arc { sweep_radians, .. } = &mut wrong {
        *sweep_radians = 3. * std::f64::consts::FRAC_PI_2;
    }
    assert!(!matches_geometry(&expected, &wrong));
}

#[test]
fn every_post_prefix_sdk_getter_panic_preserves_observations_and_unknown() {
    for mode in [11, 12, 13, 14] {
        let mut b = book();
        let sdk = Fake::default();
        let r = request(&b, &sdk, points(), "getter-panic");
        let p = b.geometry_preflight(&sdk, &r).unwrap();
        sdk.mode.set(mode);
        let out = b.execute_geometry(&sdk, p);
        assert_eq!(out["outcome"], "unknown", "mode {mode}: {out}");
        assert!(!out["completedEntities"].as_array().unwrap().is_empty());
        assert!(b.geometry_preflight(&sdk, &r).is_err());
    }
}
#[test]
fn maximum_stored_points_keep_full_observation_within_reserved_result_budget() {
    let mut b = book();
    let sdk = Fake::default();
    let items:Vec<Value>=(0..4).map(|s|json!({"type":"spline","points":(0..128).map(|i|vec![i as f64*1.123456789,s as f64*10.+(i%2) as f64]).collect::<Vec<_>>()})).collect();
    let request = request(&b, &sdk, json!(items), "max-points");
    let plan = b.geometry_preflight(&sdk, &request).unwrap();
    assert_eq!(plan.prepared.stored_point_count(), 512);
    let out = b.execute_geometry(&sdk, plan);
    assert_eq!(out["success"], true);
    assert!(serde_json::to_vec(&out).unwrap().len() < 128 * 1024);
    assert_eq!(out["geometry"]["observed"].as_array().unwrap().len(), 4);
}
