use super::*;
use fusion_mechanical_core::prepare_request;
use std::cell::{Cell, RefCell};

struct Fake {
    design: RefCell<DesignState>,
    parameters: RefCell<Vec<Parameter>>,
    health: RefCell<Vec<HealthItem>>,
    effects: RefCell<Vec<String>>,
    evaluations: Cell<usize>,
    reads: Cell<usize>,
    fail_read: Cell<Option<usize>>,
    failure_after_effect: Cell<bool>,
    compute_failure: Cell<bool>,
    wrong_readback: Cell<bool>,
    ignored_setter: Cell<bool>,
}
impl Fake {
    fn new() -> Self {
        Self {
            design: RefCell::new(DesignState {
                active: true,
                parametric: true,
                root_name: "Root".into(),
                root_revision: "r1".into(),
                default_length_units: "mm".into(),
                component_count: 1,
                body_count: 0,
                sketch_count: 0,
                timeline_count: 0,
                timeline_position: 0,
            }),
            parameters: RefCell::new(vec![Parameter {
                name: "width".into(),
                expression: "10 mm".into(),
                unit: "mm".into(),
                comment: "driver".into(),
                is_user: true,
                is_deletable: true,
                value: ParameterValue::Numeric(1.0),
            }]),
            health: RefCell::new(vec![]),
            effects: RefCell::new(vec![]),
            evaluations: Cell::new(0),
            reads: Cell::new(0),
            fail_read: Cell::new(None),
            failure_after_effect: Cell::new(false),
            compute_failure: Cell::new(false),
            wrong_readback: Cell::new(false),
            ignored_setter: Cell::new(false),
        }
    }
    fn request(&self, op: &str, name: &str, expression: &str) -> PreparedRequest {
        let target = inspect(self, "document-a").unwrap().target;
        let mut value = json!({"op":op,"sessionId":"session-a","operationId":"operation-a","target":target,"name":name,"expression":expression});
        if op == "parameter_create" {
            value["unit"] = json!("mm");
            value["comment"] = json!("new driver");
        }
        prepare_request(value, "session-a").unwrap()
    }
}
impl Backend for Fake {
    fn design_state(&self) -> std::result::Result<DesignState, String> {
        Ok(self.design.borrow().clone())
    }
    fn parameter_count(&self) -> std::result::Result<usize, String> {
        Ok(self.parameters.borrow().len())
    }
    fn parameter(&self, index: usize) -> std::result::Result<Parameter, String> {
        let call = self.reads.get() + 1;
        self.reads.set(call);
        if self.fail_read.get() == Some(call) {
            return Err("injected read failure".into());
        }
        Ok(self.parameters.borrow()[index].clone())
    }
    fn timeline_health(&self, index: usize) -> std::result::Result<HealthItem, String> {
        Ok(self.health.borrow()[index].clone())
    }
    fn evaluate_expression(
        &self,
        expression: &str,
        unit: &str,
    ) -> std::result::Result<f64, String> {
        self.evaluations.set(self.evaluations.get() + 1);
        let value = expression
            .strip_suffix(" mm")
            .ok_or("invalid expression")?
            .parse::<f64>()
            .map_err(|_| "invalid number")?;
        if unit != "mm" {
            return Err("unit mismatch".into());
        }
        Ok(value / 10.0)
    }
    fn create_parameter(
        &self,
        name: &str,
        expression: &str,
        unit: &str,
        comment: &str,
    ) -> std::result::Result<(), String> {
        self.effects.borrow_mut().push(format!("create:{name}"));
        let value = if self.wrong_readback.get() {
            42.0
        } else {
            self.evaluate_expression(expression, unit)?
        };
        self.parameters.borrow_mut().push(Parameter {
            name: name.into(),
            expression: expression.into(),
            unit: unit.into(),
            comment: comment.into(),
            is_user: true,
            is_deletable: true,
            value: ParameterValue::Numeric(value),
        });
        if self.failure_after_effect.get() {
            Err("lost creation acknowledgement".into())
        } else {
            Ok(())
        }
    }
    fn set_parameter(&self, name: &str, expression: &str) -> std::result::Result<(), String> {
        self.effects.borrow_mut().push(format!("set:{name}"));
        if self.ignored_setter.get() {
            return Ok(());
        }
        let value = if self.wrong_readback.get() {
            42.0
        } else {
            self.evaluate_expression(expression, "mm")?
        };
        let mut parameters = self.parameters.borrow_mut();
        let parameter = parameters
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or("missing")?;
        parameter.expression = expression.into();
        parameter.value = ParameterValue::Numeric(value);
        if self.failure_after_effect.get() {
            Err("lost setter acknowledgement".into())
        } else {
            Ok(())
        }
    }
    fn compute_all(&self) -> std::result::Result<(), String> {
        self.effects.borrow_mut().push("compute".into());
        if self.compute_failure.get() {
            Err("compute failed".into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn projection_is_explicit_and_changes_for_unused_parameters_and_default_units() {
    let fake = Fake::new();
    let before = inspect(&fake, "document-a").unwrap();
    assert!(!before.complete && before.target.revision.starts_with("parameters-v1:"));
    fake.parameters.borrow_mut()[0].expression = "20 mm".into();
    let changed = inspect(&fake, "document-a").unwrap();
    assert_ne!(before.target.revision, changed.target.revision);
    assert_eq!(before.design.root_revision, changed.design.root_revision);
    fake.design.borrow_mut().default_length_units = "in".into();
    assert_ne!(
        changed.target.revision,
        inspect(&fake, "document-a").unwrap().target.revision
    );
    assert!(fake.effects.borrow().is_empty());
}

#[test]
fn reordered_parameter_collection_has_same_projection_and_text_is_readable() {
    let fake = Fake::new();
    fake.parameters.borrow_mut().push(Parameter {
        name: "label".into(),
        expression: "'sample'".into(),
        unit: "Text".into(),
        comment: "".into(),
        is_user: true,
        is_deletable: true,
        value: ParameterValue::Text("sample".into()),
    });
    let before = inspect(&fake, "document-a").unwrap();
    fake.parameters.borrow_mut().reverse();
    assert_eq!(before.target, inspect(&fake, "document-a").unwrap().target);
    let request = fake.request("parameter_set", "label", "20 mm");
    assert_eq!(
        preflight(&fake, &request, "document-a").unwrap_err().code,
        "unsupported_capability"
    );
    assert!(fake.effects.borrow().is_empty());
}

#[test]
fn create_and_set_produce_one_mutation_compute_and_actual_readback() {
    for (op, name) in [("parameter_create", "height"), ("parameter_set", "width")] {
        let fake = Fake::new();
        let request = fake.request(op, name, "20 mm");
        let plan = preflight(&fake, &request, "document-a").unwrap();
        assert_eq!(plan.document_id(), "document-a");
        assert!(fake.effects.borrow().is_empty());
        let result = execute(&fake, plan);
        assert_eq!(result["success"], true);
        assert_eq!(result["nativeExecutionStarted"], true);
        assert_eq!(
            result["parameter"]["value"],
            json!({"type":"numeric","value":2.0})
        );
        assert_eq!(result["transactional"], false);
        assert_eq!(result["recoveryAttempted"], false);
        assert_eq!(fake.effects.borrow().len(), 2);
    }
}

#[test]
fn wrong_document_stale_preflight_and_late_changes_never_call_mutator() {
    let fake = Fake::new();
    let request = fake.request("parameter_set", "width", "20 mm");
    assert_eq!(
        preflight(&fake, &request, "document-b").unwrap_err().code,
        "wrong_document"
    );
    let plan = preflight(&fake, &request, "document-a").unwrap();
    fake.parameters.borrow_mut()[0].comment = "changed outside bridge".into();
    assert_eq!(
        preflight(&fake, &request, "document-a").unwrap_err().code,
        "stale_revision"
    );
    let result = execute(&fake, plan);
    assert_eq!(result["outcome"], "not_executed");
    assert_eq!(result["nativeExecutionStarted"], false);
    assert!(fake.effects.borrow().is_empty());
}

#[test]
fn failed_sdk_acknowledgment_never_infers_rollback_or_retries() {
    for op in ["parameter_create", "parameter_set"] {
        let fake = Fake::new();
        let name = if op == "parameter_create" {
            "height"
        } else {
            "width"
        };
        let request = fake.request(op, name, "20 mm");
        let plan = preflight(&fake, &request, "document-a").unwrap();
        fake.failure_after_effect.set(true);
        let result = execute(&fake, plan);
        assert_eq!(result["success"], false);
        assert_eq!(result["outcome"], "unknown");
        assert_eq!(result["nativeExecutionStarted"], true);
        assert_eq!(result["retryAuthorized"], false);
        assert_eq!(fake.effects.borrow().len(), 1);
        assert_eq!(
            fake.parameters
                .borrow()
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .value,
            ParameterValue::Numeric(2.0)
        );
    }
}

#[test]
fn recompute_failure_and_wrong_numeric_readback_leave_unknown_effect() {
    for mode in [0, 1] {
        let fake = Fake::new();
        let request = fake.request("parameter_set", "width", "20 mm");
        let plan = preflight(&fake, &request, "document-a").unwrap();
        if mode == 0 {
            fake.compute_failure.set(true);
        } else {
            fake.wrong_readback.set(true);
        }
        let result = execute(&fake, plan);
        assert_eq!(result["outcome"], "unknown");
        assert_eq!(result["reconciliationRequired"], true);
        assert_eq!(fake.effects.borrow().len(), 2);
    }
}

#[test]
fn invalid_expression_existing_create_missing_set_and_batch_refuse_pre_effect() {
    let fake = Fake::new();
    for (op, name, expr, code) in [
        ("parameter_set", "width", "bad", "invalid_expression"),
        ("parameter_create", "width", "20 mm", "parameter_exists"),
        ("parameter_set", "missing", "20 mm", "parameter_missing"),
    ] {
        let request = fake.request(op, name, expr);
        assert_eq!(
            preflight(&fake, &request, "document-a").unwrap_err().code,
            code
        );
    }
    let target = inspect(&fake, "document-a").unwrap().target;
    let request=prepare_request(json!({"op":"parameter_batch","sessionId":"session-a","operationId":"batch","target":target,"changes":[{"action":"set","name":"width","expression":"20 mm"}]}),"session-a").unwrap();
    assert_eq!(
        preflight(&fake, &request, "document-a").unwrap_err().code,
        "unsupported_capability"
    );
    assert!(fake.effects.borrow().is_empty());
}

#[test]
fn unhealthy_inactive_direct_and_rolled_back_designs_refuse_pre_effect() {
    for mode in 0..4 {
        let fake = Fake::new();
        if mode == 0 {
            fake.design.borrow_mut().active = false;
        }
        if mode == 1 {
            fake.design.borrow_mut().parametric = false;
        }
        if mode >= 2 {
            fake.design.borrow_mut().timeline_count = 1;
            fake.health.borrow_mut().push(HealthItem {
                index: 0,
                state: if mode == 2 { 1 } else { 0 },
                message: "warning".into(),
            });
            fake.design.borrow_mut().timeline_position = if mode == 2 { 1 } else { 0 };
        }
        let request = fake.request("parameter_set", "width", "20 mm");
        assert!(preflight(&fake, &request, "document-a").is_err());
        assert!(fake.effects.borrow().is_empty());
    }
}

#[test]
fn bounded_faulted_duplicate_and_nonfinite_inspection_never_publishes_partial_state() {
    for mode in 0..4 {
        let fake = Fake::new();
        if mode == 0 {
            fake.fail_read.set(Some(1));
        }
        if mode == 1 {
            let p = fake.parameters.borrow()[0].clone();
            fake.parameters.borrow_mut().push(p);
        }
        if mode == 2 {
            fake.parameters.borrow_mut()[0].value = ParameterValue::Numeric(f64::NAN);
        }
        if mode == 3 {
            fake.parameters.borrow_mut()[0].comment = "x".repeat(4097);
        }
        assert!(inspect(&fake, "document-a").is_err());
        assert!(fake.effects.borrow().is_empty());
    }
}

#[test]
fn plan_is_owned_send_for_journal_worker_hop() {
    fn requires_send<T: Send>() {}
    requires_send::<Plan>();
}

#[test]
fn equivalent_numeric_value_does_not_prove_new_expression_was_applied() {
    let fake = Fake::new();
    let request = fake.request("parameter_set", "width", "10.0 mm");
    let plan = preflight(&fake, &request, "document-a").unwrap();
    fake.ignored_setter.set(true);
    let result = execute(&fake, plan);
    assert_eq!(result["success"], false);
    assert_eq!(result["outcome"], "unknown");
    assert_eq!(result["errorCode"], "native_readback_mismatch");
    assert_eq!(fake.parameters.borrow()[0].expression, "10 mm");
    assert_eq!(fake.effects.borrow().as_slice(), ["set:width", "compute"]);
}
