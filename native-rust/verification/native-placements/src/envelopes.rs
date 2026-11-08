// Response admission is separate from geometry oracles. Malformed uncertainty
// metadata must stop the campaign before another request, including a mutation.
fn typed_flags(body: &Value) -> Result<()> {
    ensure(body.is_object(), "response/status must be an object")?;
    for key in [
        "success",
        "reconciliationRequired",
        "nativeExecutionStarted",
        "nativeExecutionAuthorized",
        "readBackendInvoked",
        "readTargetRefused",
        "replayed",
        "historical",
        "effectJournalConnected",
        "effectJournalConfigured",
    ] {
        ensure(
            body.get(key).is_none_or(Value::is_boolean),
            "malformed response flag",
        )?;
    }
    Ok(())
}

fn known_result_flags(body: &Value) -> Result<()> {
    typed_flags(body)?;
    ensure(body["success"].is_boolean(), "missing Boolean success")?;
    ensure(
        body.get("outcome").is_none(),
        "unexpected terminal/unknown outcome metadata",
    )?;
    ensure(
        body.get("reconciliationRequired")
            .is_none_or(|v| v == false),
        "unconfirmed result",
    )?;
    ensure(
        body.get("nativeExecutionAuthorized")
            .is_none_or(|v| v == false),
        "response grants execution authority",
    )?;
    if body["readTargetRefused"] == true {
        ensure(
            body["success"] == false
                && body["readBackendInvoked"] == false
                && body["nativeExecutionStarted"] == false
                && body["reconciliationRequired"] == false,
            "contradictory read-target refusal",
        )?;
    }
    Ok(())
}

fn known_read_envelope(body: &Value) -> Result<()> {
    known_result_flags(body)?;
    ensure(
        body.get("nativeExecutionStarted")
            .is_none_or(|v| v == false),
        "read claims a native effect",
    )?;
    for field in [
        "operationStatus",
        "authority",
        "durable",
        "observed",
        "storageError",
        "retentionError",
        "replayed",
        "historical",
    ] {
        ensure(
            body.get(field).is_none(),
            "journal metadata on ordinary read",
        )?;
    }
    Ok(())
}

fn known_status_envelope(status: &Value, allow_absence: bool) -> Result<()> {
    typed_flags(status)?;
    ensure(
        status["success"] == true
            && status.get("outcome").is_none()
            && status["historical"] == false
            && status["nativeExecutionAuthorized"] == false
            && status["effectJournalConnected"] == true
            && status
                .get("effectJournalConfigured")
                .is_none_or(|v| v == true)
            && status.get("nativeExecutionStarted").is_none()
            && status.get("replayed").is_none()
            && status.get("operationStatus").is_none()
            && status.get("storageError") == Some(&Value::Null)
            && status.get("retentionError") == Some(&Value::Null),
        "unavailable or contradictory journal status",
    )?;
    // This exact successful empty lookup is unknown history, not permission to
    // replay an effect. Its identity is separately bound to the GET route.
    if allow_absence && no_admission(status).is_ok() {
        return Ok(());
    }
    let durable = &status["durable"];
    typed_flags(durable)?;
    let record = &durable["record"];
    let phase = record["phase"].as_str().ok_or("missing durable phase")?;
    ensure(
        status["authority"] == "durable_terminal"
            && status["reconciliationRequired"] == false
            && durable["historical"] == false
            && durable["reconciliationRequired"] == false
            && durable["outcome"] == "recorded"
            && durable["state"] == phase
            && matches!(phase, "succeeded" | "failed" | "not_executed")
            && record["sessionId"] == status["sessionId"]
            && record["operationId"] == status["operationId"],
        "nonterminal or inconsistent durable authority",
    )?;
    known_result_flags(&record["result"])?;
    ensure(
        record["result"]["success"] == (phase == "succeeded"),
        "phase/result contradiction",
    )?;
    if phase == "not_executed" {
        ensure(
            record["result"]["nativeExecutionStarted"] == false,
            "not-executed result claims execution",
        )?;
    }
    // Process-local observed cache is not allowed to override this validated
    // durable terminal authority. Detailed request/result equality is checked by
    // the operation oracle, not inferred from an observed-success flag.
    ensure(
        status
            .get("observed")
            .is_some_and(|v| v.is_null() || v.is_object()),
        "malformed observed cache",
    )
}

impl Campaign {
    fn accept_response(
        &mut self,
        route: &str,
        request: Option<&Value>,
        response: &Response,
    ) -> Result<()> {
        if self.uncertain {
            return Err("prior response uncertainty remains latched".into());
        }
        let result = (|| {
            ensure(
                matches!(response.code, 200 | 400 | 409),
                "unexpected or unavailable response status",
            )?;
            let body = &response.body;
            typed_flags(body)?;
            ensure(
                body["success"] == (response.code == 200),
                "HTTP/success contradiction",
            )?;
            if request.is_none() && route.starts_with("/v1/operations/") {
                ensure(response.code == 200, "journal lookup unavailable")?;
                let prefix = format!("/v1/operations/{}/", text(&self.runtime, "epoch")?);
                let operation = route.strip_prefix(&prefix).ok_or("foreign status route")?;
                ensure(
                    body["epoch"] == self.runtime["epoch"]
                        && body["sessionId"] == self.runtime["epoch"]
                        && body["operationId"] == operation,
                    "status response identity differs from route",
                )?;
                known_status_envelope(body, true)
            } else if request.is_some_and(is_effect) {
                known_result_flags(body)?;
                if let Some(status) = body.get("operationStatus") {
                    known_status_envelope(status, false)?;
                    let request = request.unwrap();
                    ensure(
                        status["sessionId"] == request["sessionId"]
                            && status["operationId"] == request["operationId"],
                        "effect status identity mismatch",
                    )?;
                    terminal(
                        response,
                        request,
                        body["success"] == true,
                        body["replayed"].as_bool().ok_or("missing replay flag")?,
                    )
                } else {
                    ensure(
                        body["success"] == false
                            && body["nativeExecutionStarted"] == false
                            && body.get("replayed").is_none(),
                        "unconfirmed effect lacks durable authority",
                    )
                }
            } else {
                if route == "/v1/capabilities" && request.is_none() {
                    ensure(
                        body.get("epoch")
                            .is_none_or(|e| e == &self.runtime["epoch"]),
                        "static capabilities epoch mismatch",
                    )?;
                } else {
                    ensure(
                        body["epoch"] == self.runtime["epoch"],
                        "dynamic read epoch mismatch",
                    )?;
                }
                known_read_envelope(body)
            }
        })();
        if result.is_err() {
            self.uncertain = true;
        }
        result
    }
}
