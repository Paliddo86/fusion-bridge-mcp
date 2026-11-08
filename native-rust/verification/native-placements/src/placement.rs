const FIRST_POSITION: [f64; 3] = [25., -10., 5.];
const SECOND_POSITION: [f64; 3] = [-5., 20., 10.];
const SELECTED_OCCURRENCE: usize = 2;

fn occurrence_ref(state: &Value, index: usize) -> Result<Value> {
    let row = state["occurrences"]
        .as_array()
        .and_then(|v| v.get(index))
        .ok_or("selected occurrence")?;
    Ok(
        json!({"documentId":state["target"]["documentId"],"revision":state["target"]["revision"],"objectType":"Occurrence","occurrenceId":row["occurrenceId"]}),
    )
}

fn matrix_for_mm(mm: [f64; 3]) -> [f64; 16] {
    let mut matrix = [0.; 16];
    for i in [0, 5, 10, 15] {
        matrix[i] = 1.;
    }
    for (i, x) in [3, 7, 11].into_iter().zip(mm) {
        matrix[i] = x / 10.;
    }
    matrix
}

fn exact_pose(matrix: &Value, mm: [f64; 3]) -> Result<()> {
    let m = matrix.as_array().ok_or("matrix array")?;
    ensure(m.len() == 16, "matrix length")?;
    for (v, expected) in m.iter().zip(matrix_for_mm(mm)) {
        ensure(
            v.as_f64()
                .is_some_and(|x| x.is_finite() && (x - expected).abs() <= 1e-10),
            "absolute mm-to-cm pose/basis",
        )?;
    }
    Ok(())
}

// Independently require the entire published state to match the prior state,
// except the requested matrix, one captured timeline item and scoped revisions.
// Native component revisions are deliberately NOT normalized.
fn translated_projection(before: &Value, after: &Value, mm: [f64; 3]) -> Result<()> {
    let index = SELECTED_OCCURRENCE;
    let os = after["occurrences"].as_array().ok_or("after occurrences")?;
    ensure(
        os.len() == 3
            && before["occurrences"]
                .as_array()
                .is_some_and(|x| x.len() == 3),
        "exact fixture occurrences",
    )?;
    for state in [before, after] {
        ensure(
            state["occurrences"][index]["grounded"] == false
                && state["occurrences"][index]["groundedToParent"] == false,
            "selected occurrence remains ungrounded",
        )?;
    }
    exact_pose(&after["occurrences"][index]["matrix"], mm)?;
    let prior_timeline = before["root"]["timeline"]
        .as_u64()
        .ok_or("prior timeline")?;
    ensure(
        (3..=4).contains(&prior_timeline) && before["root"]["timelinePosition"] == prior_timeline,
        "known baseline timeline",
    )?;
    ensure(
        after["root"]["timeline"] == prior_timeline + 1
            && after["root"]["timelinePosition"] == prior_timeline + 1,
        "exact single captured timeline item",
    )?;
    ensure(
        after["target"] != before["target"]
            && after["target"]["documentId"] == before["target"]["documentId"],
        "new same-document revision",
    )?;
    let mut shape = after.clone();
    shape["occurrences"][index]["matrix"] = json!(matrix_for_mm([0.; 3]));
    shape["root"]["timeline"] = json!(3);
    shape["root"]["timelinePosition"] = json!(3);
    assembly(&shape, &before["target"]["documentId"], 2, 3, true, true)?;
    let mut normalized = after.clone();
    normalized["target"] = before["target"].clone();
    for field in ["revision", "timeline", "timelinePosition"] {
        normalized["root"][field] = before["root"][field].clone();
    }
    for i in 0..2 {
        normalized["components"][i]["ref"]["revision"] =
            before["components"][i]["ref"]["revision"].clone();
    }
    normalized["occurrences"][index]["matrix"] = before["occurrences"][index]["matrix"].clone();
    ensure(
        normalized == *before,
        "unintended projection change beyond requested pose/capture",
    )
}

impl Campaign {
    fn placement_args(&self, suffix: &str, state: &Value, mm: [f64; 3]) -> Result<Value> {
        let mut args = self.write_args(suffix, &state["target"]);
        args["op"] = json!("occurrence_update");
        args["occurrence"] = occurrence_ref(state, SELECTED_OCCURRENCE)?;
        args["transform"] = json!({"position":mm});
        Ok(args)
    }
    fn known_placement_absence(&mut self, label: &str, args: &Value, code: &str) -> Result<()> {
        let denied = self.mechanical(label, args)?;
        terminal(&denied, args, false, false)?;
        response_failure(&denied, &self.runtime["epoch"], code)?;
        ensure(
            denied.body["operationStatus"]["durable"]["record"]["phase"] == "not_executed",
            "placement refusal durable phase",
        )?;
        self.durable_record(&format!("{label}-history"), args, &denied)?;
        self.settle(label, "durable_not_executed")
    }
    fn run_placements(&mut self, initial: &Value) -> Result<Value> {
        ensure(
            initial["occurrences"][SELECTED_OCCURRENCE]["grounded"] == false
                && initial["occurrences"][SELECTED_OCCURRENCE]["groundedToParent"] == false,
            "fixture shared occurrence must already be ungrounded",
        )?;
        let header = self.permit(
            "placement-header",
            self.placement_args("placement-header", initial, FIRST_POSITION)?,
        )?;
        let denied = self.request_header(
            "placement-header",
            "/v1/mechanical",
            Some(&header),
            CONTRACT,
        )?;
        ensure(
            denied.code == 409
                && denied.body["success"] == false
                && denied.body["nativeExecutionStarted"] == false
                && denied.body["errorCode"] == "native_placement_contract_required",
            "placement-specific header refusal",
        )?;
        no_admission(&self.status("placement-header-history", text(&header, "operationId")?)?)?;
        self.settle("placement-header", "pre_admission_absence_verified")?;
        for (suffix, field, value, code) in [
            (
                "placement-axis",
                "axis",
                json!([0, 0, 1]),
                "native_placement_position_only",
            ),
            (
                "placement-angle",
                "angle",
                json!(0),
                "native_placement_position_only",
            ),
            (
                "placement-grounded",
                "grounded",
                json!(false),
                "native_placement_grounding_changes_unsupported",
            ),
            (
                "placement-parent-grounded",
                "groundedToParent",
                json!(false),
                "native_placement_grounding_changes_unsupported",
            ),
            (
                "placement-foreign",
                "documentId",
                json!("foreign-document"),
                "native_placement_exact_occurrence_required",
            ),
        ] {
            let mut args = self.placement_args(suffix, initial, FIRST_POSITION)?;
            match field {
                "axis" | "angle" => args["transform"][field] = value,
                "documentId" => args["occurrence"][field] = value,
                _ => args[field] = value,
            }
            let args = self.permit(suffix, args)?;
            self.absent_refusal(suffix, &args, code)?;
        }
        let mut stale = self.placement_args("placement-stale", initial, FIRST_POSITION)?;
        let rev = false_revision(ASSEMBLY_PREFIX, &initial["target"]["revision"])?;
        stale["target"]["revision"] = rev.clone();
        stale["occurrence"]["revision"] = rev;
        let stale = self.permit("placement-stale", stale)?;
        self.known_placement_absence("placement-stale", &stale, "target_revision_mismatch")?;
        let noop = self.permit(
            "placement-noop",
            self.placement_args("placement-noop", initial, [0.; 3])?,
        )?;
        self.known_placement_absence("placement-noop", &noop, "native_placement_no_change")?;
        ensure(
            self.read_assembly("placement-after-refusals", &initial["target"])? == *initial,
            "placement refusals changed assembly",
        )?;
        let mut prior = initial.clone();
        for (ordinal, suffix, mm) in [
            (0, "move-first", FIRST_POSITION),
            (1, "move-second", SECOND_POSITION),
        ] {
            let args = self.permit(suffix, self.placement_args(suffix, &prior, mm)?)?;
            let done = self.proven_effect(suffix, &args)?;
            placement_result(&prior, &done.body, mm, ordinal)?;
            let next = done.body["data"].clone();
            ensure(
                self.read_assembly(&format!("{suffix}-readback"), &next["target"])? == next,
                "translated projection repeated read mismatch",
            )?;
            self.inspections.insert(suffix.into(), next.clone());
            let mut conflict = args.clone();
            conflict["transform"]["position"] = json!([9, 8, 7]);
            self.replay_conflict(&format!("{suffix}-proof"), &args, &done, &conflict)?;
            prior = next;
        }
        let noop = self.permit(
            "placement-noop-after",
            self.placement_args("placement-noop-after", &prior, SECOND_POSITION)?,
        )?;
        self.known_placement_absence("placement-noop-after", &noop, "native_placement_no_change")?;
        ensure(
            self.read_assembly("placement-final", &prior["target"])? == prior,
            "post-placement refusal changed assembly",
        )?;
        self.checks
            .push("two_absolute_translations_preserve_shared_siblings_and_component_definitions");
        self.checks
            .push("each_translation_captures_exactly_one_position_and_clears_pending_state");
        self.checks
            .push("placement_scope_stale_and_noop_refusals_with_confirmed_only_replay");
        Ok(prior)
    }
}

fn placement_result(before: &Value, body: &Value, mm: [f64; 3], ordinal: usize) -> Result<()> {
    ensure(
        body["nativeExecutionStarted"] == true
            && body["automaticReplayAllowed"] == false
            && body["rollbackAttempted"] == false
            && body["legacyParity"] == false,
        "placement effect policy",
    )?;
    let after = &body["data"];
    translated_projection(before, after, mm)?;
    ensure(
        body["occurrenceId"] == after["occurrences"][SELECTED_OCCURRENCE]["occurrenceId"]
            && body["occurrenceRef"] == occurrence_ref(after, SELECTED_OCCURRENCE)?,
        "returned exact occurrence",
    )?;
    let stages = body["stages"].as_array().ok_or("placement stages")?;
    ensure(stages.len() == 2, "two independent effect stages")?;
    for (i, name) in ["occurrenceTransform", "positionSnapshot"]
        .into_iter()
        .enumerate()
    {
        ensure(
            stages[i]["stage"] == name
                && stages[i]["attempt"]["invoked"] == true
                && stages[i]["attempt"]["acknowledged"] == true,
            "placement stage acknowledgement/order",
        )?;
    }
    ensure(
        stages[0]["attempt"]["component"] == 0
            && stages[0]["attempt"]["occurrence"] == SELECTED_OCCURRENCE,
        "selected shared-instance stage",
    )?;
    ensure(
        stages[1]["attempt"].get("component") == Some(&Value::Null)
            && stages[1]["attempt"].get("occurrence") == Some(&Value::Null)
            && stages[1]["snapshotSlot"] == ordinal,
        "design-wide capture identity/scope",
    )?;
    let e = &body["placementEvidence"];
    ensure(
        e["coordinateSpace"] == "root_assembly"
            && e["inputUnit"] == "mm"
            && e["matrixTranslationUnit"] == "cm"
            && e["requestedPositionMm"] == json!(mm),
        "placement evidence units/intent",
    )?;
    exact_pose(&e["requestedMatrixCm"], mm)?;
    exact_pose(&e["observedMatrixCm"], mm)?;
    ensure(
        e["stableCapturesPerStage"] == 2
            && e["maximumTransformInvocations"] == 1
            && e["maximumCaptureInvocations"] == 1
            && e["effectRetryAttempted"] == false,
        "bounded once-only placement policy",
    )?;
    revision(&e["intermediateProjectionDigest"], ASSEMBLY_PREFIX)?;
    let timeline = before["root"]["timeline"].as_u64().ok_or("timeline")?;
    for (field, pending, add) in [
        ("before", false, 0),
        ("intermediate", true, 0),
        ("after", false, 1),
    ] {
        let s = &e[field];
        let snapshots = s["snapshots"].as_array().ok_or("snapshot inventory")?;
        ensure(
            s["pending"] == pending
                && s["timeline"] == timeline + add
                && s["timelinePosition"] == timeline + add
                && snapshots.len() == ordinal + add as usize,
            "pending/timeline/capture count",
        )?;
        for (i, fact) in snapshots.iter().enumerate() {
            ensure(
                fact["slot"] == i && fact["timelineIndex"] == 3 + i && fact["health"] == 0,
                "exact retained captured-position facts",
            )?;
        }
    }
    let mut intermediate = e["intermediate"].clone();
    intermediate["pending"] = json!(false);
    ensure(
        intermediate == e["before"],
        "capture collection changed during transform",
    )?;
    Ok(())
}
