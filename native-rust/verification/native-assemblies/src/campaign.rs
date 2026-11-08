impl Campaign {
    fn proven_effect(&mut self, label: &str, args: &Value) -> Result<Response> {
        let result = self.mechanical(label, args)?;
        terminal(&result, args, true, false)?;
        self.durable_record(&format!("{label}-history"), args, &result)?;
        self.settle(label, "durable_succeeded")?;
        // Preserve the proven effect before any later geometric assertion.
        self.successful.push(json!({"label":label,"request":args,
            "durableRecord":result.body["operationStatus"]["durable"]["record"],
            "result":result.body}));
        Ok(result)
    }
    fn replay_conflict(
        &mut self,
        label: &str,
        args: &Value,
        original: &Response,
        changed: &Value,
    ) -> Result<()> {
        ensure(
            args["operationId"] == changed["operationId"] && args != changed,
            "conflict probe must alter same operation",
        )?;
        let before = self.counters(&format!("{label}-before"))?;
        let replay_label = format!("{label}-replay");
        let replay = self.mechanical(&replay_label, args)?;
        terminal(&replay, args, true, true)?;
        ensure(
            replay.body["operationStatus"]["durable"]["record"]
                == original.body["operationStatus"]["durable"]["record"],
            "replay changed result",
        )?;
        self.settle(&replay_label, "durable_replay_unchanged")?;
        let conflict_label = format!("{label}-conflict");
        let rejected = self.mechanical(&conflict_label, changed)?;
        ensure(
            rejected.code == 409
                && rejected.body["epoch"] == self.runtime["epoch"]
                && rejected.body["success"] == false
                && rejected.body["errorCode"] == "operation_conflict"
                && rejected.body["nativeExecutionStarted"] == false,
            "conflict did not refuse before execution",
        )?;
        self.durable_record(&format!("{label}-history"), args, original)?;
        self.settle(
            &conflict_label,
            "conflict_original_durable_record_unchanged",
        )?;
        ensure(
            self.counters(&format!("{label}-after"))? == before,
            "replay/conflict dispatched SDK work",
        )
    }
    fn absent_refusal(&mut self, label: &str, args: &Value, code: &str) -> Result<()> {
        let response = self.mechanical(label, args)?;
        response_failure(&response, &self.runtime["epoch"], code)?;
        let status = self.status(&format!("{label}-history"), text(args, "operationId")?)?;
        no_admission(&status)?;
        self.settle(label, "pre_admission_absence_verified")
    }
    fn durable_record(&mut self, label: &str, args: &Value, response: &Response) -> Result<()> {
        let status = self.status(label, text(args, "operationId")?)?;
        ensure(
            status["authority"] == "durable_terminal"
                && status["reconciliationRequired"] == false
                && status["durable"]["record"]
                    == response.body["operationStatus"]["durable"]["record"],
            "fresh operation status differs from terminal authority",
        )
    }
}
impl Campaign {
    fn read_data(&mut self, label: &str, args: &Value) -> Result<Value> {
        let r = self.mechanical(label, args)?;
        ensure(
            r.code == 200 && r.body["success"] == true && r.body["epoch"] == self.runtime["epoch"],
            "read unavailable/identity",
        )?;
        Ok(r.body["data"].clone())
    }
}
impl Campaign {
    fn read_assembly(&mut self, label: &str, target: &Value) -> Result<Value> {
        let args = json!({"op":"native_assembly_inspect","sessionId":self.runtime["epoch"],"target":target,"nativeMetadata":false});
        self.read_data(label, &args)
    }
    fn fixed_create(&self, suffix: &str, state: &Value, name: &str, metadata: Value) -> Value {
        let mut args = self.write_args(suffix, &state["target"]);
        args["name"] = json!(name);
        args["metadata"] = metadata;
        args
    }
    fn verify_read(
        &mut self,
        label: &str,
        state: &Value,
        n: usize,
        m: usize,
        ca: bool,
        cb: bool,
    ) -> Result<()> {
        assembly(state, &self.fixture["documentId"], n, m, ca, cb)?;
        ensure(
            self.read_assembly(label, &state["target"])? == *state,
            "published assembly changed on repeated read",
        )?;
        self.inspections.insert(label.into(), state.clone());
        Ok(())
    }
    fn run(&mut self) -> Result<Value> {
        self.counters("health-before")?;
        let cap = self.request("capabilities", "/v1/capabilities", None)?;
        capabilities(&cap)?;
        let baseline = self.documents("documents-before")?;
        ensure(
            baseline == document_map(&self.runtime["before"])?,
            "authorized baseline changed",
        )?;
        let mut create = self.write_args("create", &Value::Null);
        create["op"] = json!("new_design");
        create.as_object_mut().unwrap().remove("target");
        create["name"] = json!("Assembly qualification");
        let create = self.permit("create", create)?;
        let made = self.proven_effect("create", &create)?;
        let d = &made.body["data"];
        let doc = text(&d["target"], "documentId")?.to_owned();
        ensure(
            current_document_id(&doc, text(&self.runtime, "epoch")?)
                && !baseline.contains_key(&doc)
                && d["ownedScratch"] == true
                && d["creationOperationId"] == create["operationId"]
                && d["requestedName"] == create["name"]
                && d["requestedNameScope"] == "session_and_operation_receipt_metadata"
                && d["legacyParity"] == false,
            "durable exact task creation binding",
        )?;
        self.fixture["documentId"] = json!(doc);
        write_new(
            &self.output.join("created-task.json"),
            &json!({"documentId":doc,"creationId":d["document"]["creationId"],"creationOperationId":create["operationId"],"durableRecord":made.body["operationStatus"]["durable"]["record"],"cleanupAuthorized":false}),
        )?;
        let roster = self.documents("documents-created")?;
        assert_roster(&baseline, &roster, &doc)?;
        ensure(
            roster[&doc] == d["document"],
            "created document substituted",
        )?;
        let mut conflict = create.clone();
        conflict["name"] = json!("Assembly qualification conflict");
        self.replay_conflict("create-proof", &create, &made, &conflict)?;
        let bare = json!({"documentId":doc});
        let legacy =
            json!({"op":"native_sketch_inspect","sessionId":self.runtime["epoch"],"target":bare});
        let old = self.request_header(
            "legacy-empty-sketch-read",
            "/v1/mechanical",
            Some(&legacy),
            "native-sketch-dimensions-v1",
        )?;
        ensure(
            old.code == 200
                && old.body["data"]["target"]["documentId"] == doc
                && old.body["data"]["root"]["sketchCount"] == 0,
            "existing sketch read/header regression",
        )?;
        let empty = self.read_assembly("assembly-empty", &bare)?;
        self.verify_read("assembly-empty-repeat", &empty, 0, 0, false, false)?;
        let header = self.permit(
            "header",
            self.fixed_create("header", &empty, "Assembly fixture A", a_metadata()),
        )?;
        let denied = self.request_header(
            "assembly-old-header",
            "/v1/mechanical",
            Some(&header),
            "native-sketch-dimensions-v1",
        )?;
        ensure(
            denied.code == 409
                && denied.body["nativeExecutionStarted"] == false
                && denied.body["errorCode"] == "native_assembly_contract_required",
            "assembly old header did not refuse",
        )?;
        no_admission(&self.status("assembly-old-header-history", text(&header, "operationId")?)?)?;
        self.settle("assembly-old-header", "pre_admission_absence_verified")?;
        let mut transform = self.fixed_create(
            "unsupported-transform",
            &empty,
            "Assembly fixture A",
            a_metadata(),
        );
        transform["transform"] = json!({"position":[1,0,0]});
        let transform = self.permit("unsupported-transform", transform)?;
        self.absent_refusal(
            "unsupported-transform",
            &transform,
            "native_assembly_identity_placement_required",
        )?;
        let mut native = self.fixed_create(
            "unsupported-create-native",
            &empty,
            "Assembly fixture A",
            a_metadata(),
        );
        native["metadata"]["partNumber"] = json!("Unsupported during create");
        let native = self.permit("unsupported-create-native", native)?;
        self.absent_refusal(
            "unsupported-create-native",
            &native,
            "native_assembly_native_metadata_separate_update_required",
        )?;
        let mut stale = self.fixed_create("stale", &empty, "Assembly fixture A", a_metadata());
        stale["target"]["revision"] =
            false_revision(ASSEMBLY_PREFIX, &empty["target"]["revision"])?;
        let stale = self.permit("stale", stale)?;
        let denied = self.mechanical("stale", &stale)?;
        terminal(&denied, &stale, false, false)?;
        response_failure(&denied, &self.runtime["epoch"], "target_revision_mismatch")?;
        ensure(
            denied.body["operationStatus"]["durable"]["record"]["phase"] == "not_executed",
            "stale assembly effect not proven absent",
        )?;
        self.durable_record("stale-history", &stale, &denied)?;
        self.settle("stale", "durable_not_executed")?;
        ensure(
            self.read_assembly("empty-after-refusals", &empty["target"])? == empty,
            "refused requests changed root",
        )?;
        self.checks
            .push("legacy_header_compatibility_and_precise_no_effect_guards");
        let a = self.permit(
            "a",
            self.fixed_create("a", &empty, "Assembly fixture A", a_metadata()),
        )?;
        let done_a = self.proven_effect("a", &a)?;
        let mut a_fields = a_metadata();
        a_fields["name"] = a["name"].clone();
        stages(&done_a.body, "component_create", Some(&a_fields), 0)?;
        let state_a = done_a.body["data"].clone();
        self.verify_read("a-readback", &state_a, 1, 1, false, false)?;
        let mut conflict = a.clone();
        conflict["name"] = json!("Assembly fixture conflict");
        self.replay_conflict("a-proof", &a, &done_a, &conflict)?;
        let b = self.permit(
            "b",
            self.fixed_create("b", &state_a, "Assembly fixture B", b_metadata()),
        )?;
        let done_b = self.proven_effect("b", &b)?;
        let mut b_fields = b_metadata();
        b_fields["name"] = b["name"].clone();
        stages(&done_b.body, "component_create", Some(&b_fields), 1)?;
        let state_b = done_b.body["data"].clone();
        self.verify_read("b-readback", &state_b, 2, 2, false, false)?;
        let mut copy = self.write_args("unsupported-copy", &state_b["target"]);
        copy["op"] = json!("occurrence_insert");
        copy["component"] = state_b["components"][0]["ref"].clone();
        copy["independentCopy"] = json!(true);
        let copy = self.permit("unsupported-copy", copy)?;
        self.absent_refusal(
            "unsupported-copy",
            &copy,
            "native_assembly_shared_instances_only",
        )?;
        let mut shared = self.write_args("shared", &state_b["target"]);
        shared["op"] = json!("occurrence_insert");
        shared["component"] = state_b["components"][0]["ref"].clone();
        shared["independentCopy"] = json!(false);
        let shared = self.permit("shared", shared)?;
        let done_shared = self.proven_effect("shared", &shared)?;
        stages(&done_shared.body, "occurrence_insert", None, 0)?;
        let shared_state = done_shared.body["data"].clone();
        self.verify_read("shared-readback", &shared_state, 2, 3, false, false)?;
        let mut update = self.write_args("metadata-a", &shared_state["target"]);
        update["op"] = json!("component_metadata");
        update["component"] = shared_state["components"][0]["ref"].clone();
        update["changes"] = a_changes();
        let update = self.permit("metadata-a", update)?;
        let done = self.proven_effect("metadata-a", &update)?;
        stages(&done.body, "component_metadata", Some(&a_changes()), 0)?;
        let a_changed = done.body["data"].clone();
        self.verify_read("metadata-a-readback", &a_changed, 2, 3, true, false)?;
        let mut conflict = update.clone();
        conflict["changes"]["supplier"] = json!("Conflict vendor");
        self.replay_conflict("metadata-a-proof", &update, &done, &conflict)?;
        let mut update = self.write_args("metadata-b", &a_changed["target"]);
        update["op"] = json!("component_metadata");
        update["component"] = a_changed["components"][1]["ref"].clone();
        update["changes"] = b_changes();
        let update = self.permit("metadata-b", update)?;
        let done = self.proven_effect("metadata-b", &update)?;
        stages(&done.body, "component_metadata", Some(&b_changes()), 1)?;
        let final_state = done.body["data"].clone();
        self.verify_read("assembly-final", &final_state, 2, 3, true, true)?;
        self.checks
            .push("two_distinct_definitions_three_occurrences_and_shared_identity");
        self.checks
            .push("json_escaped_metadata_name_changes_and_purchasing_flags");
        self.checks
            .push("separate_native_stages_durable_history_and_confirmed_only_replay");
        let roster = self.documents("documents-final")?;
        assert_roster(&baseline, &roster, &doc)?;
        self.counters("health-final")?;
        ensure(
            self.successful.len() == 6,
            "expected six confirmed operations",
        )?;
        Ok(
            json!({"documentId":doc,"creationId":roster[&doc]["creationId"],"baseline":baseline,"finalRoster":roster,"assembly":final_state,"successfulOperations":6,"definitionCount":2,"occurrenceCount":3,"definitionQuantities":[2,1],"onlineNativeMetadataQualified":false,"bomTraversalQualified":false,"independentSdkTruthQualified":false,"cleanupAuthorized":false}),
        )
    }
}
