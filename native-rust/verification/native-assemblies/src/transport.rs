impl Campaign {
    fn final_receipt(&self, result: &Result<Value>) -> Value {
        let pending = self
            .attempts
            .iter()
            .filter(|a| a["settlement"].is_null())
            .count();
        json!({"success":result.is_ok() && !self.uncertain && pending==0,"checksPassed":self.checks,"requestCount":self.count,"elapsedMs":self.started.elapsed().as_millis(),
            "reconciliationRequired":self.uncertain || pending>0 || (!self.attempts.is_empty() && result.is_err()),
            "pendingOrUnknownAttempts":pending,"attempts":self.attempts,"successfulOperations":self.successful,
            "latestInspections":self.inspections,"result":result.as_ref().ok(),"error":result.as_ref().err(),
            "cleanupAuthorized":false,"cleanup":"Root must separately qualify, archive/reopen and retire this exact task document. No automatic cleanup.",
            "scope":"One task-owned flat root assembly; two definitions, three occurrences and local metadata; independent SDK truth and online BOM qualification pending"})
    }
    fn request(&mut self, label: &str, route: &str, body: Option<&Value>) -> Result<Response> {
        self.request_header(label, route, body, CONTRACT)
    }
    fn request_header(
        &mut self,
        label: &str,
        route: &str,
        body: Option<&Value>,
        contract: &str,
    ) -> Result<Response> {
        ensure(
            contract == CONTRACT
                || (contract == "native-sketch-dimensions-v1"
                    && body.is_some_and(|b| {
                        matches!(
                            b["op"].as_str(),
                            Some(
                                "native_assembly_inspect"
                                    | "component_create"
                                    | "native_sketch_inspect"
                            )
                        )
                    })),
            "unreviewed header probe",
        )?;
        self.allowed(route, body)?;
        ensure(
            !self.uncertain,
            "no further requests after observed uncertainty",
        )?;
        ensure(
            self.count < MAX_REQUESTS && self.started.elapsed() < CAMPAIGN_LIMIT,
            "campaign budget exceeded",
        )?;
        ensure(
            !label.is_empty()
                && label.len() <= 80
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-'),
            "receipt label",
        )?;
        let data = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        ensure(
            data.len() <= fusion_mechanical_core::MAX_WIRE_BYTES,
            "request too large",
        )?;
        if let Some(body) = body {
            fusion_mechanical_core::prepare_request(body.clone(), text(&self.runtime, "epoch")?)
                .map_err(|e| e.to_string())?;
        }
        self.count += 1;
        let start = self.started.elapsed().as_micros();
        let effect = body.is_some_and(is_effect);
        let intent = json!({"label":label,"route":route,"body":body,"mechanicalContract":if body.is_some(){Some(contract)}else{None},"startUs":start,"possibleEffectRequest":effect,"automaticRetry":false});
        write_new(
            &self
                .output
                .join(format!("{:02}-{label}-intent.json", self.count)),
            &redact(intent, text(&self.runtime, "token")?),
        )?;
        if effect {
            self.attempts.push(json!({"label":label,"operationId":body.unwrap()["operationId"],"request":body,"settlement":null}));
        }
        let method = if body.is_some() { "POST" } else { "GET" };
        let mut bytes=format!("{method} {route} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nX-Adom-Fusion-Epoch: {}\r\nX-Adom-Timeout-Ms: 2000\r\n",self.address,text(&self.runtime,"token")?,text(&self.runtime,"epoch")?).into_bytes();
        if body.is_some() {
            bytes.extend_from_slice(format!("X-Adom-Mechanical-Contract: {contract}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",data.len()).as_bytes());
        }
        bytes.extend_from_slice(b"\r\n");
        bytes.extend_from_slice(&data);
        let response = exchange(
            self.address,
            &bytes,
            (Instant::now() + Duration::from_secs(8)).min(self.started + CAMPAIGN_LIMIT),
        );
        let record = match &response {
            Ok(r) => {
                json!({"label":label,"status":r.code,"body":r.body,"startUs":start,"endUs":self.started.elapsed().as_micros()})
            }
            Err(e) => {
                self.uncertain = true;
                json!({"label":label,"transportError":e,"possibleEffectRequest":effect,"effectOutcome":"unconfirmed","automaticRetry":false})
            }
        };
        write_new(
            &self
                .output
                .join(format!("{:02}-{label}-result.json", self.count)),
            &redact(record, text(&self.runtime, "token")?),
        )?;
        if let Ok(r) = &response {
            // Preserve the literal bounded response receipt before interpreting
            // route-specific flags or latching malformed/unknown outcomes.
            self.accept_response(route, body, r)?;
        }
        response
    }
    fn mechanical(&mut self, label: &str, args: &Value) -> Result<Response> {
        self.request(label, "/v1/mechanical", Some(args))
    }
    fn write_args(&self, suffix: &str, target: &Value) -> Value {
        json!({"op":"component_create","sessionId":self.runtime["epoch"],"operationId":format!("r20-{}-{suffix}",self.fixture["campaignId"].as_str().unwrap()),"target":target})
    }
    fn settle(&mut self, label: &str, value: &str) -> Result<()> {
        let entry = self
            .attempts
            .iter_mut()
            .find(|a| a["label"] == label)
            .ok_or("missing attempt")?;
        ensure(entry["settlement"].is_null(), "attempt already settled")?;
        entry["settlement"] = json!(value);
        Ok(())
    }
    fn status(&mut self, label: &str, operation: &str) -> Result<Value> {
        let route = format!(
            "/v1/operations/{}/{operation}",
            text(&self.runtime, "epoch")?
        );
        let response = self.request(label, &route, None)?;
        ensure(
            response.code == 200
                && response.body["success"] == true
                && response.body["epoch"] == self.runtime["epoch"]
                && response.body["sessionId"] == self.runtime["epoch"]
                && response.body["operationId"] == operation,
            "status identity/unavailability",
        )?;
        Ok(response.body)
    }
    fn counters(&mut self, label: &str) -> Result<Value> {
        let response = self.request(label, "/v1/health", None)?;
        ensure(
            response.code == 200
                && response.body["success"] == true
                && response.body["epoch"] == self.runtime["epoch"]
                && response.body["dispatchReady"] == true
                && response.body["effectJournalConnected"] == true
                && response.body["stats"]["callbackPanics"] == 0
                && response.body["stats"]["drainErrors"] == 0,
            "runtime health/readiness failed",
        )?;
        let mut counts = serde_json::Map::new();
        for key in ["nativeWakeCalls", "eventCallbacks", "snapshotCalls"] {
            counts.insert(
                key.into(),
                json!(response.body["stats"][key]
                    .as_u64()
                    .ok_or("missing counter")?),
            );
        }
        Ok(Value::Object(counts))
    }
    fn documents(&mut self, label: &str) -> Result<BTreeMap<String, Value>> {
        let response = self.request(label, "/v1/documents", None)?;
        ensure(
            response.code == 200
                && response.body["success"] == true
                && response.body["epoch"] == self.runtime["epoch"],
            "document read unavailable",
        )?;
        let map = document_map(&response.body["data"])?;
        ensure(map.len() <= 256, "document bound")?;
        active_id(&map)?;
        for (id, doc) in &map {
            ensure(
                current_document_id(id, text(&self.runtime, "epoch")?)
                    && doc["name"].is_string()
                    && doc["creationId"].is_string()
                    && ["active", "saved", "modified"]
                        .iter()
                        .all(|k| doc[k].is_boolean()),
                "invalid document inventory",
            )?;
        }
        Ok(map)
    }
}
