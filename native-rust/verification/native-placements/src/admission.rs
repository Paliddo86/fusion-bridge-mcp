const EFFECT_SUFFIXES: &[&str] = &[
    "create",
    "a",
    "b",
    "shared",
    "metadata-a",
    "metadata-b",
    "unsupported-transform",
    "unsupported-create-native",
    "unsupported-copy",
    "stale",
    "header",
    "placement-header",
    "placement-axis",
    "placement-angle",
    "placement-grounded",
    "placement-parent-grounded",
    "placement-stale",
    "placement-foreign",
    "placement-noop",
    "placement-noop-after",
    "move-first",
    "move-second",
];
impl Campaign {
    fn permit(&mut self, suffix: &str, args: Value) -> Result<Value> {
        ensure(
            EFFECT_SUFFIXES.contains(&suffix),
            "unreviewed effect suffix",
        )?;
        let key = format!("r21-{}-{suffix}", text(&self.fixture, "campaignId")?);
        ensure(args["operationId"] == key, "registration ID")?;
        let map = self.fixture["permittedRequests"]
            .as_object_mut()
            .ok_or("private request table")?;
        ensure(
            map.insert(key, args.clone()).is_none(),
            "request registration already consumed",
        )?;
        Ok(args)
    }
    fn allowed(&self, route: &str, body: Option<&Value>) -> Result<()> {
        let prefix = format!("r21-{}-", text(&self.fixture, "campaignId")?);
        if let Some(b) = body {
            ensure(
                route == "/v1/mechanical" && b["sessionId"] == self.runtime["epoch"],
                "foreign route/session",
            )?;
            let op = text(b, "op")?;
            if is_effect(b) {
                let id = text(b, "operationId")?;
                let suffix = id.strip_prefix(&prefix).ok_or("operation namespace")?;
                ensure(EFFECT_SUFFIXES.contains(&suffix), "unreviewed effect ID")?;
                let expected = self.fixture["permittedRequests"]
                    .get(id)
                    .ok_or("unregistered fixed request")?;
                let mut conflict = expected.clone();
                match suffix {
                    "create" => conflict["name"] = json!("Assembly qualification conflict"),
                    "a" | "b" => conflict["name"] = json!("Assembly fixture conflict"),
                    "shared" => conflict["independentCopy"] = json!(true),
                    "metadata-a" | "metadata-b" => {
                        conflict["changes"]["supplier"] = json!("Conflict vendor")
                    }
                    "move-first" | "move-second" => {
                        conflict["transform"]["position"] = json!([9, 8, 7]);
                    }
                    _ => {}
                }
                let confirmed = self
                    .successful
                    .iter()
                    .any(|s| s["request"]["operationId"] == id);
                ensure(
                    b == expected || (confirmed && *b == conflict),
                    "effect differs from fixed authored plan",
                )?;
                if let Some(first) = self.attempts.iter().find(|a| a["operationId"] == id) {
                    ensure(
                        confirmed && !first["settlement"].is_null(),
                        "no retry of unconfirmed effect",
                    )?;
                }
                if op == "new_design" {
                    ensure(
                        suffix == "create"
                            && b.get("target").is_none()
                            && b.as_object().is_some_and(|o| o.len() == 4),
                        "one exact blank creation",
                    )?;
                    return Ok(());
                }
            }
            let doc = self
                .fixture
                .get("documentId")
                .filter(|v| v.is_string())
                .ok_or("no exact created document")?;
            ensure(b["target"]["documentId"] == *doc, "foreign document")?;
            ensure(
                matches!(
                    op,
                    "component_create"
                        | "component_metadata"
                        | "occurrence_insert"
                        | "occurrence_update"
                        | "native_assembly_inspect"
                        | "native_sketch_inspect"
                ),
                "unreviewed operation",
            )?;
            if op == "native_assembly_inspect" {
                ensure(
                    b["nativeMetadata"] == false && b.as_object().is_some_and(|o| o.len() == 4),
                    "local read only",
                )?;
            }
            if op == "native_sketch_inspect" {
                ensure(
                    b.as_object().is_some_and(|o| o.len() == 3) && self.successful.len() == 1,
                    "legacy empty-root read only before assembly effects",
                )?;
            }
        } else {
            let p = format!("/v1/operations/{}/{prefix}", text(&self.runtime, "epoch")?);
            ensure(
                ["/v1/health", "/v1/capabilities", "/v1/documents"].contains(&route)
                    || route
                        .strip_prefix(&p)
                        .is_some_and(|s| EFFECT_SUFFIXES.contains(&s)),
                "unreviewed GET",
            )?;
        }
        Ok(())
    }
}
fn a_metadata() -> Value {
    json!({"supplier":"Vendor \"A\" \\ depot","supplierPartNumber":"=TEST,42","sourceUrl":"https://example.invalid/A","asPurchased":true,"excludeFromBom":false})
}
fn b_metadata() -> Value {
    json!({"supplier":"Vendor \"A\" \\ depot","supplierPartNumber":"=TEST,42","sourceUrl":"https://example.invalid/B","asPurchased":true,"excludeFromBom":true})
}
fn a_changes() -> Value {
    json!({"name":"Assembly fixture A revised","supplierPartNumber":"=TEST,43","asPurchased":false})
}
fn b_changes() -> Value {
    json!({"supplier":"Other vendor","sourceUrl":"https://example.invalid/B-revised"})
}
