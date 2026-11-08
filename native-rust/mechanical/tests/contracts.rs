use fusion_mechanical_core::{
    prepare_json, prepare_request, validated_schema, Access, Schema, CANONICAL_FORMAT,
    MAX_REQUEST_BYTES, MAX_WIRE_BYTES, OPERATION_CONTRACTS,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn write(op: &str) -> Value {
    json!({"op": op, "sessionId": "live-session", "operationId": "write-1"})
}

fn sample(index: Value) -> Value {
    let mut request = write("exploded_preview_sample");
    request["target"] = json!({"documentId": "presentation", "revision": "revision"});
    request["previewId"] = json!("preview");
    request["expectedGeneration"] = json!(6);
    request["sampleIndex"] = index;
    request
}

fn sketch(item: Value) -> Value {
    let mut request = write("sketch_geometry");
    request["target"] = json!({"documentId":"doc", "revision":"rev"});
    request["sketch"] = json!({"documentId":"doc", "revision":"rev", "objectType":"Sketch"});
    request["items"] = json!([item]);
    request
}

#[test]
fn exact_schema_contract_inventory_loads_without_unrecognized_keywords() {
    let schema = validated_schema().unwrap();
    let classified: BTreeSet<_> = OPERATION_CONTRACTS
        .iter()
        .map(|operation| operation.name)
        .collect();
    let defined: BTreeSet<_> = schema
        .contracts_json()
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(classified, defined);
    assert_eq!(classified.len(), 70); // 61 legacy contracts + nine explicit native extensions.
    assert_eq!(OPERATION_CONTRACTS.len(), classified.len());
    // 48 unconditional writes, 21 reads, and one conditional health operation.
    assert_eq!(
        OPERATION_CONTRACTS
            .iter()
            .filter(|op| op.access == Access::Write)
            .count(),
        48
    );
    assert_eq!(
        OPERATION_CONTRACTS
            .iter()
            .filter(|op| op.access == Access::Read)
            .count(),
        21
    );
}

#[test]
fn reads_are_prepared_without_write_identity_or_native_support_claims() {
    let request = prepare_request(json!({"op":"inspect"}), "live-session").unwrap();
    assert!(!request.is_write());
    assert_eq!(request.operation(), "inspect");
    assert_eq!(request.arguments(), &json!({"op":"inspect"}));
}

#[test]
fn unknown_operation_and_non_object_are_rejected() {
    for request in [
        json!({"op":"execute_python"}),
        json!({"op":true}),
        json!({}),
    ] {
        assert_eq!(
            prepare_request(request, "live-session").unwrap_err().code,
            "unsupported_operation"
        );
    }
    for request in [Value::Null, json!([]), json!(true)] {
        assert_eq!(
            prepare_request(request, "live-session").unwrap_err().code,
            "invalid_input"
        );
    }
}

#[test]
fn only_boolean_transport_flags_are_stripped_and_annotations_do_not_change_request() {
    let baseline = prepare_request(write("new_design"), "live-session").unwrap();
    let mut annotated = write("new_design");
    annotated["noAutoStart"] = json!(true);
    annotated["noAutoRepair"] = json!(false);
    annotated["caller"] = json!({"id":"caller-2"});
    annotated["_callerBudgetMs"] = json!(25_000);
    let prepared = prepare_request(annotated, "live-session").unwrap();
    assert_eq!(prepared.arguments(), baseline.arguments());
    assert_eq!(prepared.canonical_bytes(), baseline.canonical_bytes());
    for name in ["noAutoStart", "noAutoRepair"] {
        for invalid in [json!(0), json!(1), json!("true"), Value::Null, json!({})] {
            let mut request = write("new_design");
            request[name] = invalid;
            let error = prepare_request(request, "live-session").unwrap_err();
            assert_eq!(error.code, "invalid_input");
            assert!(error.message.contains(name));
        }
    }
}

#[test]
fn write_session_must_match_and_cannot_be_an_empty_runtime_session() {
    for session in [
        json!("old-session"),
        json!(true),
        json!(17),
        Value::Null,
        json!(""),
    ] {
        let mut request = write("new_design");
        request["sessionId"] = session;
        assert_eq!(
            prepare_request(request, "live-session").unwrap_err().code,
            "session_mismatch"
        );
    }
    let mut request = write("new_design");
    request["sessionId"] = json!("");
    assert_eq!(
        prepare_request(request, "").unwrap_err().code,
        "session_mismatch"
    );
    assert!(prepare_request(write("new_design"), "live-session")
        .unwrap()
        .is_write());
}

#[test]
fn operation_ids_use_one_to_128_unicode_scalars() {
    for invalid in [
        json!(""),
        json!("x".repeat(129)),
        json!(false),
        json!(1),
        Value::Null,
    ] {
        let mut request = write("new_design");
        request["operationId"] = invalid;
        assert_eq!(
            prepare_request(request, "live-session").unwrap_err().code,
            "operation_id_required"
        );
    }
    let mut request = write("new_design");
    request["operationId"] = json!("🦀".repeat(128));
    assert!(prepare_request(request, "live-session").is_ok());
}

#[test]
fn missing_required_ids_fail_schema_before_any_admission() {
    for key in ["sessionId", "operationId"] {
        let mut request = write("new_design");
        request.as_object_mut().unwrap().remove(key);
        let error = prepare_request(request, "live-session").unwrap_err();
        assert_eq!(error.code, "invalid_input");
        assert!(error.message.contains(&format!("args.{key} is required")));
    }
}

#[test]
fn health_recompute_switches_to_write_guards() {
    let mut request = json!({"op":"health", "target":{}});
    assert!(!prepare_request(request.clone(), "live-session")
        .unwrap()
        .is_write());
    request["recompute"] = json!(false);
    assert!(!prepare_request(request.clone(), "live-session")
        .unwrap()
        .is_write());
    request["recompute"] = json!(true);
    assert_eq!(
        prepare_request(request.clone(), "live-session")
            .unwrap_err()
            .code,
        "session_mismatch"
    );
    request["sessionId"] = json!("live-session");
    assert_eq!(
        prepare_request(request.clone(), "live-session")
            .unwrap_err()
            .code,
        "operation_id_required"
    );
    request["operationId"] = json!("recompute-1");
    assert!(prepare_request(request.clone(), "live-session")
        .unwrap()
        .is_write());
    request["recompute"] = json!(1);
    assert_eq!(
        prepare_request(request, "live-session").unwrap_err().code,
        "invalid_input"
    );
}

#[test]
fn unknown_top_level_args_and_nested_closed_object_fields_are_rejected() {
    let error = prepare_request(json!({"op":"inspect", "z":1, "a":2}), "live-session").unwrap_err();
    assert_eq!(error.message, "Unknown operation arguments: a, z");
    let mut request = write("new_design");
    request["box"] = json!({"width":1, "height":2, "depth":3, "silentOption":true});
    assert_eq!(
        prepare_request(request, "live-session")
            .unwrap_err()
            .message,
        "args.box has unknown fields"
    );
}

#[test]
fn preview_sample_static_limit_is_checked_before_runtime_plan_length() {
    for valid in [0, 5, 63] {
        // 5 can be outside an eventual 5-sample native plan, but is schema-valid.
        assert!(prepare_request(sample(json!(valid)), "live-session").is_ok());
    }
    for invalid in [64, 99] {
        let error = prepare_request(sample(json!(invalid)), "live-session").unwrap_err();
        assert_eq!(error.message, "args.sampleIndex is too large");
    }
    assert_eq!(
        prepare_request(sample(json!(-1)), "live-session")
            .unwrap_err()
            .message,
        "args.sampleIndex is too small"
    );
}

#[test]
fn booleans_and_float_literals_are_not_integer_indices() {
    for invalid in [json!(true), json!(false), json!(1.0), json!("1")] {
        assert_eq!(
            prepare_request(sample(invalid), "live-session")
                .unwrap_err()
                .message,
            "args.sampleIndex must be integer"
        );
    }
    let mut request = write("new_design");
    request["box"] = json!({"width":true, "height":2, "depth":3});
    assert_eq!(
        prepare_request(request, "live-session")
            .unwrap_err()
            .message,
        "args.box.width must be number"
    );
}

#[test]
fn discriminator_errors_explain_selected_primitive() {
    let invalid = sketch(json!({"type":"circle", "center":[0,0], "radius":0}));
    let error = prepare_request(invalid, "live-session").unwrap_err();
    assert_eq!(error.message, "args.items[0].radius is too small");
    let invalid = sketch(json!({"type":"circle", "center":[0,0]}));
    assert_eq!(
        prepare_request(invalid, "live-session")
            .unwrap_err()
            .message,
        "args.items[0].radius is required"
    );
    let valid = sketch(json!({"type":"circle", "center":[0,0], "radius":1.25}));
    assert!(prepare_request(valid, "live-session").is_ok());
    let unknown = sketch(json!({"type":"unsupported", "center":[0,0], "radius":1}));
    assert!(prepare_request(unknown, "live-session")
        .unwrap_err()
        .message
        .contains("exactly one"));
}

#[test]
fn one_of_rejects_zero_or_multiple_matches_and_selects_action_diagnostics() {
    let schema: Schema =
        serde_json::from_value(json!({"oneOf":[{"type":"number"},{"type":"integer"}]})).unwrap();
    assert!(schema.validate(&json!(1.5), "args").is_ok());
    assert!(schema.validate(&json!(1), "args").is_err());
    assert!(schema.validate(&json!(false), "args").is_err());
    let schema: Schema = serde_json::from_value(json!({"oneOf":[
        {"type":"object", "properties":{"action":{"enum":["move"]}, "distance":{"type":"number","exclusiveMinimum":0}}, "required":["action","distance"]},
        {"type":"object", "properties":{"action":{"enum":["delete"]}}, "required":["action"]}
    ]})).unwrap();
    assert_eq!(
        schema
            .validate(&json!({"action":"move","distance":-1}), "args")
            .unwrap_err()
            .message,
        "args.distance is too small"
    );
}

#[test]
fn array_bounds_and_typed_additional_properties_are_enforced() {
    let schema: Schema = serde_json::from_value(json!({"type":"object", "minProperties":1, "additionalProperties":{"type":"array", "minItems":1, "maxItems":2, "items":{"type":"number"}}})).unwrap();
    assert!(schema.validate(&json!({"arbitrary":[1,2]}), "args").is_ok());
    for invalid in [
        json!({}),
        json!({"a":[]}),
        json!({"a":[1,2,3]}),
        json!({"a":[true]}),
    ] {
        assert!(schema.validate(&invalid, "args").is_err());
    }
}

#[test]
fn schema_string_limits_are_enforced_in_unicode_scalars() {
    let schema: Schema =
        serde_json::from_value(json!({"type":"string", "minLength":1, "maxLength":2})).unwrap();
    assert!(schema.validate(&json!("🦀ö"), "args").is_ok());
    assert!(schema.validate(&json!(""), "args").is_err());
    assert!(schema.validate(&json!("abc"), "args").is_err());
}

#[test]
fn schema_growth_cannot_silently_add_unimplemented_keywords() {
    assert!(serde_json::from_value::<Schema>(json!({"type":"number", "multipleOf":2})).is_err());
    assert!(serde_json::from_value::<Schema>(json!({"type":"imaginary"})).is_err());
}

#[test]
fn float_roundtrip_preserves_known_native_one_ulp_regression() {
    let request = br#"{"op":"new_design","sessionId":"live-session","operationId":"precision","box":{"width":14.999999999999993,"height":1791822402.6348333,"depth":1}}"#;
    let prepared = prepare_json(request, "live-session").unwrap();
    assert_eq!(
        prepared.arguments()["box"]["width"]
            .as_f64()
            .unwrap()
            .to_bits(),
        0x402d_ffff_ffff_fffc
    );
    let again = prepare_json(prepared.canonical_bytes(), "live-session").unwrap();
    for field in ["width", "height", "depth"] {
        assert_eq!(
            prepared.arguments()["box"][field]
                .as_f64()
                .unwrap()
                .to_bits(),
            again.arguments()["box"][field].as_f64().unwrap().to_bits()
        );
    }
}

#[test]
fn exact_large_integers_survive_but_oversized_integer_literals_are_refused() {
    let raw = br#"{"op":"inspect_entity","target":{},"entity":{"token":18446744073709551615,"smaller":9007199254740993}}"#;
    let prepared = prepare_json(raw, "live-session").unwrap();
    assert_eq!(
        prepared.arguments()["entity"]["token"].as_u64(),
        Some(u64::MAX)
    );
    assert_eq!(
        prepared.arguments()["entity"]["smaller"].as_u64(),
        Some(9_007_199_254_740_993)
    );
    assert!(std::str::from_utf8(prepared.canonical_bytes())
        .unwrap()
        .contains("9007199254740993"));
    let raw = br#"{"op":"inspect_entity","target":{},"entity":{"token":18446744073709551616}}"#;
    assert!(prepare_json(raw, "live-session")
        .unwrap_err()
        .message
        .contains("exact supported"));
    let raw = br#"{"op":"inspect_entity","target":{},"entity":{"token":"18446744073709551616000","escaped":"\"99999999999999999999999"}}"#;
    assert!(prepare_json(raw, "live-session").is_ok());
}

#[test]
fn nonfinite_json_and_trailing_junk_are_rejected() {
    for raw in [
        br#"{"op":"new_design","box":{"width":NaN}}"#.as_slice(),
        br#"{"op":"new_design","box":{"width":Infinity}}"#,
        br#"{"op":"new_design","box":{"width":1e400}}"#,
        br#"{"op":"inspect"} {}"#,
    ] {
        assert_eq!(
            prepare_json(raw, "live-session").unwrap_err().code,
            "invalid_input"
        );
    }
}

#[test]
fn duplicate_json_keys_are_refused_including_transport_options_and_nested_references() {
    for raw in [
        br#"{"op":"inspect","op":"inspect"}"#.as_slice(),
        br#"{"op":"inspect","noAutoRepair":false,"noAutoRepair":true}"#,
        br#"{"op":"inspect_entity","target":{},"entity":{"token":"first","token":"second"}}"#,
    ] {
        let error = prepare_json(raw, "live-session").unwrap_err();
        assert_eq!(error.code, "invalid_input");
        assert!(error.message.contains("duplicate JSON field"));
    }
}

#[test]
fn canonical_form_is_order_independent_and_explicitly_not_legacy() {
    let first = prepare_json(
        r#"{"op":"inspect_entity","target":{"b":2,"a":1},"entity":{"z":"ö","n":2}}"#.as_bytes(),
        "live-session",
    )
    .unwrap();
    let second = prepare_json(
        br#"{"entity":{"n":2,"z":"\u00f6"},"target":{"a":1,"b":2},"op":"inspect_entity"}"#,
        "live-session",
    )
    .unwrap();
    assert_eq!(first.canonical_bytes(), second.canonical_bytes());
    assert_eq!(first.canonical_format(), CANONICAL_FORMAT);
    assert!(CANONICAL_FORMAT.contains("rust"));
}

#[test]
fn canonical_byte_limit_applies_after_envelope_stripping() {
    let mut request = json!({"op":"inspect", "caller":"a".repeat(MAX_REQUEST_BYTES + 1)});
    assert!(prepare_request(request.clone(), "live-session").is_ok());
    request.as_object_mut().unwrap().remove("caller");
    request["op"] = json!("new_design");
    request["sessionId"] = json!("live-session");
    request["operationId"] = json!("limit");
    request["name"] = json!("a".repeat(MAX_REQUEST_BYTES));
    assert_eq!(
        prepare_request(request, "live-session").unwrap_err().code,
        "operation_record_too_large"
    );
}

#[test]
fn oversized_wire_body_is_refused_before_parsing_or_envelope_stripping() {
    let invalid_json = vec![b'x'; MAX_WIRE_BYTES + 1];
    let error = prepare_json(&invalid_json, "live-session").unwrap_err();
    assert_eq!(error.code, "operation_record_too_large");
    assert!(error.message.contains("Wire request"));
    let request =
        serde_json::to_vec(&json!({"op":"inspect", "caller":"a".repeat(MAX_WIRE_BYTES)})).unwrap();
    assert_eq!(
        prepare_json(&request, "live-session").unwrap_err().code,
        "operation_record_too_large"
    );
}

#[test]
fn depth_guard_handles_programmatic_values_before_recursive_schema_or_copy() {
    let mut nested = json!(0);
    for _ in 0..130 {
        nested = json!([nested]);
    }
    let request = json!({"op":"inspect_entity", "target":{}, "entity":{"nested":nested}});
    assert_eq!(
        prepare_request(request, "live-session").unwrap_err().code,
        "operation_record_invalid"
    );
}

#[test]
fn integer_bounds_compare_exactly_above_f64_precision_limit() {
    let schema: Schema =
        serde_json::from_value(json!({"type":"integer", "maximum":9_007_199_254_740_992u64}))
            .unwrap();
    assert!(schema
        .validate(&json!(9_007_199_254_740_992u64), "args")
        .is_ok());
    assert!(schema
        .validate(&json!(9_007_199_254_740_993u64), "args")
        .is_err());
}

#[test]
fn native_assembly_projection_is_read_and_requires_exact_document_selector() {
    let request=prepare_request(json!({"op":"native_assembly_inspect","sessionId":"live-session","target":{"documentId":"owned-doc"},"nativeMetadata":false}),"live-session").unwrap();
    assert!(!request.is_write());
    assert!(prepare_request(
        json!({"op":"native_assembly_inspect","sessionId":"live-session"}),
        "live-session"
    )
    .is_err());
    assert!(prepare_request(json!({"op":"native_assembly_inspect","sessionId":"live-session","target":{"documentId":"owned-doc"},"nativeMetadata":"yes"}),"live-session").is_err());
}
