fn placement_projection() -> Value {
    let mut p = projection();
    for o in p["occurrences"].as_array_mut().unwrap() {
        o["groundedToParent"] = json!(false);
    }
    p
}
fn translated_sample(before: &Value, mm: [f64; 3], digest: char) -> Value {
    let mut after = before.clone();
    let rev = format!("{ASSEMBLY_PREFIX}{}", digest.to_string().repeat(64));
    after["target"]["revision"] = json!(rev);
    for c in after["components"].as_array_mut().unwrap() {
        c["ref"]["revision"] = json!(rev);
    }
    after["root"]["revision"] = json!(format!("root-{digest}"));
    let n = before["root"]["timeline"].as_u64().unwrap() + 1;
    after["root"]["timeline"] = json!(n);
    after["root"]["timelinePosition"] = json!(n);
    after["occurrences"][SELECTED_OCCURRENCE]["matrix"] = json!(matrix_for_mm(mm));
    after
}
#[test]
fn second_translation_is_absolute_and_keeps_shared_siblings_and_definitions_exact() {
    let before = placement_projection();
    let first = translated_sample(&before, FIRST_POSITION, 'b');
    translated_projection(&before, &first, FIRST_POSITION).unwrap();
    let second = translated_sample(&first, SECOND_POSITION, 'c');
    translated_projection(&first, &second, SECOND_POSITION).unwrap();
    assert_eq!(second["occurrences"][2]["matrix"][3], json!(-0.5));
    assert_eq!(second["occurrences"][2]["matrix"][7], json!(2.));
    assert_eq!(second["occurrences"][2]["matrix"][11], json!(1.));
    let mut wrong = second.clone();
    wrong["occurrences"][2]["matrix"][3] = json!(2.);
    assert!(translated_projection(&first, &wrong, SECOND_POSITION).is_err());
}
#[test]
fn translation_oracle_rejects_every_matrix_cell_and_sibling_change() {
    let before = placement_projection();
    let after = translated_sample(&before, FIRST_POSITION, 'b');
    for occurrence in 0..3 {
        for cell in 0..16 {
            let mut wrong = after.clone();
            wrong["occurrences"][occurrence]["matrix"][cell] = json!(
                wrong["occurrences"][occurrence]["matrix"][cell]
                    .as_f64()
                    .unwrap()
                    + 0.01
            );
            assert!(
                translated_projection(&before, &wrong, FIRST_POSITION).is_err(),
                "occurrence{occurrence},cell{cell}"
            );
        }
    }
}
#[test]
fn translation_oracle_never_ignores_component_revision_metadata_or_extra_state() {
    let before = placement_projection();
    let after = translated_sample(&before, FIRST_POSITION, 'b');
    for case in 0..10 {
        let mut wrong = after.clone();
        match case {
            0 => wrong["components"][0]["revision"] = json!("different"),
            1 => wrong["components"][1]["name"] = json!("changed"),
            2 => wrong["components"][0]["attributes"]["supplierPartNumber"] = json!("\"changed\""),
            3 => wrong["occurrences"][2]["grounded"] = json!(true),
            4 => wrong["occurrences"][2]["groundedToParent"] = json!(true),
            5 => wrong["root"]["pendingSnapshot"] = json!(true),
            6 => wrong["root"]["timeline"] = json!(5),
            7 => wrong["root"]["timelinePosition"] = json!(3),
            8 => wrong["occurrences"][1]["name"] = json!("changed"),
            _ => wrong["unexpectedField"] = json!(true),
        }
        assert!(
            translated_projection(&before, &wrong, FIRST_POSITION).is_err(),
            "case{case}"
        );
    }
}
#[test]
fn placement_wire_is_exact_owned_and_unknown_attempt_cannot_retry() {
    let mut c = campaign();
    let p = placement_projection();
    let req = c.placement_args("move-first", &p, FIRST_POSITION).unwrap();
    assert_eq!(req["occurrence"]["objectType"], "Occurrence");
    assert_eq!(req["occurrence"]["revision"], p["target"]["revision"]);
    fusion_mechanical_core::prepare_request(req.clone(), text(&c.runtime, "epoch").unwrap())
        .unwrap();
    assert!(c.allowed("/v1/mechanical", Some(&req)).is_err());
    c.permit("move-first", req.clone()).unwrap();
    c.allowed("/v1/mechanical", Some(&req)).unwrap();
    c.attempts
        .push(json!({"operationId":req["operationId"],"settlement":null}));
    assert!(c.allowed("/v1/mechanical", Some(&req)).is_err());
    c.attempts[0]["settlement"] = json!("durable_succeeded");
    c.successful.push(json!({"request":req}));
    c.allowed("/v1/mechanical", Some(&req)).unwrap();
    let mut conflict = req.clone();
    conflict["transform"]["position"] = json!([9, 8, 7]);
    c.allowed("/v1/mechanical", Some(&conflict)).unwrap();
    conflict["transform"]["position"] = json!([10, 8, 7]);
    assert!(c.allowed("/v1/mechanical", Some(&conflict)).is_err());
}

fn successful_placement_receipt(before: &Value, mm: [f64; 3], ordinal: usize) -> Value {
    let after = translated_sample(before, mm, if ordinal == 0 { 'b' } else { 'c' });
    let snapshots: Vec<Value> = (0..ordinal)
        .map(|i| json!({"slot":i,"timelineIndex":3+i,"health":0}))
        .collect();
    let mut captured = snapshots.clone();
    captured.push(json!({"slot":ordinal,"timelineIndex":3+ordinal,"health":0}));
    let n = 3 + ordinal;
    json!({
        "nativeExecutionStarted":true,"automaticReplayAllowed":false,
        "rollbackAttempted":false,"legacyParity":false,
        "occurrenceId":after["occurrences"][2]["occurrenceId"],
        "occurrenceRef":occurrence_ref(&after,2).unwrap(),"data":after,
        "stages":[
            {"stage":"occurrenceTransform","attempt":{"invoked":true,"acknowledged":true,"component":0,"occurrence":2}},
            {"stage":"positionSnapshot","attempt":{"invoked":true,"acknowledged":true,"component":null,"occurrence":null},"snapshotSlot":ordinal}
        ],
        "placementEvidence":{
            "coordinateSpace":"root_assembly","inputUnit":"mm","matrixTranslationUnit":"cm",
            "requestedPositionMm":mm,"requestedMatrixCm":matrix_for_mm(mm),"observedMatrixCm":matrix_for_mm(mm),
            "stableCapturesPerStage":2,"maximumTransformInvocations":1,"maximumCaptureInvocations":1,"effectRetryAttempted":false,
            "intermediateProjectionDigest":format!("{ASSEMBLY_PREFIX}{}","d".repeat(64)),
            "before":{"pending":false,"timeline":n,"timelinePosition":n,"snapshots":snapshots},
            "intermediate":{"pending":true,"timeline":n,"timelinePosition":n,"snapshots":snapshots},
            "after":{"pending":false,"timeline":n+1,"timelinePosition":n+1,"snapshots":captured}
        }
    })
}

#[test]
fn placement_receipts_require_both_acknowledged_effects_and_explicit_capture_identity() {
    let before = placement_projection();
    let good = successful_placement_receipt(&before, FIRST_POSITION, 0);
    placement_result(&before, &good, FIRST_POSITION, 0).unwrap();
    for stage in 0..2 {
        for key in ["invoked", "acknowledged"] {
            for remove in [false, true] {
                let mut bad = good.clone();
                if remove {
                    bad["stages"][stage]["attempt"]
                        .as_object_mut()
                        .unwrap()
                        .remove(key);
                } else {
                    bad["stages"][stage]["attempt"][key] = json!(false);
                }
                assert!(placement_result(&before, &bad, FIRST_POSITION, 0).is_err());
            }
        }
    }
    for key in ["component", "occurrence"] {
        let mut bad = good.clone();
        bad["stages"][1]["attempt"]
            .as_object_mut()
            .unwrap()
            .remove(key);
        assert!(placement_result(&before, &bad, FIRST_POSITION, 0).is_err());
    }
    let mut bad = good.clone();
    bad["stages"].as_array_mut().unwrap().swap(0, 1);
    assert!(placement_result(&before, &bad, FIRST_POSITION, 0).is_err());
    bad = good.clone();
    bad["stages"][0]["attempt"]["occurrence"] = json!(0);
    assert!(placement_result(&before, &bad, FIRST_POSITION, 0).is_err());
}

#[test]
fn placement_receipts_reject_false_snapshot_history_and_wrong_pending_lifecycle() {
    let initial = placement_projection();
    let first = successful_placement_receipt(&initial, FIRST_POSITION, 0);
    let before = &first["data"];
    let good = successful_placement_receipt(before, SECOND_POSITION, 1);
    placement_result(before, &good, SECOND_POSITION, 1).unwrap();
    for phase in ["before", "intermediate", "after"] {
        for key in ["pending", "timeline", "timelinePosition"] {
            let mut bad = good.clone();
            let v = &mut bad["placementEvidence"][phase][key];
            *v = if key == "pending" {
                json!(!v.as_bool().unwrap())
            } else {
                json!(v.as_u64().unwrap() + 1)
            };
            assert!(
                placement_result(before, &bad, SECOND_POSITION, 1).is_err(),
                "{phase}/{key}"
            );
        }
        for key in ["slot", "timelineIndex", "health"] {
            let mut bad = good.clone();
            bad["placementEvidence"][phase]["snapshots"][0][key] = json!(42);
            assert!(
                placement_result(before, &bad, SECOND_POSITION, 1).is_err(),
                "{phase}/{key}"
            );
        }
    }
    let mut bad = good.clone();
    bad["placementEvidence"]["after"]["snapshots"][1] =
        bad["placementEvidence"]["after"]["snapshots"][0].clone();
    assert!(placement_result(before, &bad, SECOND_POSITION, 1).is_err());
    bad = good;
    bad["placementEvidence"]["after"]["snapshots"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(placement_result(before, &bad, SECOND_POSITION, 1).is_err());
}

#[test]
fn placement_receipts_reject_wrong_units_retry_claims_and_unbound_result() {
    let before = placement_projection();
    let good = successful_placement_receipt(&before, FIRST_POSITION, 0);
    for (key, value) in [
        ("inputUnit", json!("cm")),
        ("matrixTranslationUnit", json!("mm")),
        ("coordinateSpace", json!("component")),
        ("stableCapturesPerStage", json!(1)),
        ("maximumTransformInvocations", json!(2)),
        ("maximumCaptureInvocations", json!(2)),
        ("effectRetryAttempted", json!(true)),
        ("intermediateProjectionDigest", json!("missing")),
        ("requestedPositionMm", json!([250, -100, 50])),
    ] {
        let mut bad = good.clone();
        bad["placementEvidence"][key] = value;
        assert!(
            placement_result(&before, &bad, FIRST_POSITION, 0).is_err(),
            "{key}"
        );
    }
    for key in [
        "occurrenceId",
        "occurrenceRef",
        "nativeExecutionStarted",
        "automaticReplayAllowed",
        "rollbackAttempted",
        "legacyParity",
    ] {
        let mut bad = good.clone();
        bad.as_object_mut().unwrap().remove(key);
        assert!(
            placement_result(&before, &bad, FIRST_POSITION, 0).is_err(),
            "{key}"
        );
    }
}
