use super::*;
fn campaign() -> Campaign {
    Campaign {
        runtime: json!({"epoch":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","token":"abcdefghijklmnopqrstuvwxyz0123456789"}),
        fixture: json!({"campaignId":"fixture20261015","documentId":"d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000002","permittedRequests":{}}),
        address: "127.0.0.1:1".parse().unwrap(),
        output: std::env::temp_dir(),
        started: Instant::now(),
        count: 0,
        checks: vec![],
        uncertain: false,
        attempts: vec![],
        successful: vec![],
        inspections: BTreeMap::new(),
    }
}
fn create_request() -> Value {
    json!({"op":"component_create","sessionId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","operationId":"r20-fixture20261015-a","target":{"documentId":"d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000002","revision":format!("{ASSEMBLY_PREFIX}{}","a".repeat(64))},"name":"Assembly fixture A","metadata":a_metadata()})
}
fn absent() -> Value {
    json!({"success":true,"historical":false,"nativeExecutionAuthorized":false,"effectJournalConnected":true,"effectJournalConfigured":true,"authority":"unknown","reconciliationRequired":true,"durable":{"record":null,"state":"unknown","outcome":"unknown","historical":false,"reconciliationRequired":true},"observed":null,"storageError":null,"retentionError":null})
}
#[test]
fn precise_empty_history_is_not_execution_permission() {
    let a = absent();
    known_status_envelope(&a, true).unwrap();
    assert!(known_status_envelope(&a, false).is_err());
    for field in ["durable", "storageError", "retentionError"] {
        let mut b = a.clone();
        b[field] = json!("unknown");
        assert!(known_status_envelope(&b, true).is_err());
    }
}
#[test]
fn malformed_read_flags_sticky_before_next_request() {
    for bad in [
        json!(null),
        json!({}),
        json!({"success":true,"outcome":"unknown"}),
        json!({"success":true,"readBackendInvoked":"false"}),
        json!({"success":true,"reconciliationRequired":true}),
        json!({"success":true,"nativeExecutionStarted":true}),
    ] {
        let mut c = campaign();
        assert!(c
            .accept_response(
                "/v1/documents",
                None,
                &Response {
                    code: 200,
                    body: bad
                }
            )
            .is_err());
        assert!(c.uncertain);
        assert!(c.request("later", "/v1/health", None).is_err());
        assert_eq!(c.count, 0);
    }
}
#[test]
fn effect_without_durable_authority_never_succeeds() {
    for bad in [
        json!({"success":true,"nativeExecutionStarted":true}),
        json!({"success":false,"nativeExecutionStarted":"false"}),
        json!({"success":false,"nativeExecutionStarted":false,"outcome":"unknown"}),
    ] {
        let mut c = campaign();
        let req = create_request();
        assert!(c
            .accept_response(
                "/v1/mechanical",
                Some(&req),
                &Response {
                    code: if bad["success"] == true { 200 } else { 400 },
                    body: bad
                }
            )
            .is_err());
        assert!(c.uncertain);
    }
}
#[test]
fn correct_boolean_read_refusal_is_known() {
    let mut c = campaign();
    c.accept_response("/v1/mechanical",Some(&json!({"op":"native_assembly_inspect"})),&Response{code:400,body:json!({"epoch":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","success":false,"readTargetRefused":true,"readBackendInvoked":false,"nativeExecutionStarted":false,"reconciliationRequired":false})}).unwrap();
    assert!(!c.uncertain);
}
#[test]
fn literal_malformed_response_is_preserved_before_sticky_stop() {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut b = [0; 2048];
        s.read(&mut b).unwrap();
        let body = "{\"success\":true,\"reconciliationRequired\":\"false\"}";
        write!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let mut c = campaign();
    c.address = addr;
    c.output = std::env::temp_dir().join(format!(
        "assembly-client-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&c.output).unwrap();
    assert!(c.request("bad-read", "/v1/health", None).is_err());
    assert!(c.uncertain);
    let raw = read_json(&c.output.join("01-bad-read-result.json")).unwrap();
    assert_eq!(raw["body"]["reconciliationRequired"], "false");
    assert!(c.request("blocked", "/v1/health", None).is_err());
    assert_eq!(c.count, 1);
    server.join().unwrap();
    fs::remove_dir_all(c.output).unwrap();
}
use std::time::{SystemTime, UNIX_EPOCH};
#[test]
fn roster_changes_or_substitution_cannot_pass() {
    let original =
        json!({"documentId":"old","active":true,"saved":false,"name":"Untitled","modified":false});
    let before = BTreeMap::from([("old".into(), original.clone())]);
    let mut after = before.clone();
    after.get_mut("old").unwrap()["active"] = json!(false);
    after.insert("created".into(),json!({"documentId":"created","active":true,"saved":false,"name":"Untitled","modified":true}));
    assert_roster(&before, &after, "created").unwrap();
    after.get_mut("old").unwrap()["modified"] = json!(true);
    assert!(assert_roster(&before, &after, "created").is_err());
}
#[test]
fn parse_rejects_ambiguous_framing() {
    for s in ["HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n{}","HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 3\r\n\r\n{}","HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nTransfer-Encoding: chunked\r\n\r\n{}"]{assert!(parse_response(s.as_bytes()).is_err());}
}
fn terminal_response(request: &Value, success: bool) -> Response {
    let prepared = fusion_mechanical_core::prepare_request(
        request.clone(),
        text(request, "sessionId").unwrap(),
    )
    .unwrap();
    let mut h = Sha256::new();
    h.update(fusion_mechanical_core::CANONICAL_FORMAT.as_bytes());
    h.update([0]);
    h.update(prepared.canonical_bytes());
    let result = json!({"success":success,"nativeExecutionStarted":success,"data":{"value":7}});
    let phase = if success { "succeeded" } else { "not_executed" };
    let record = json!({"sessionId":request["sessionId"],"operationId":request["operationId"],"operation":request["op"],"requestDigest":format!("{:x}",h.finalize()),"phase":phase,"result":result});
    let status = json!({"success":true,"sessionId":request["sessionId"],"operationId":request["operationId"],"historical":false,"nativeExecutionAuthorized":false,"effectJournalConnected":true,"authority":"durable_terminal","reconciliationRequired":false,"durable":{"record":record,"state":phase,"outcome":"recorded","historical":false,"reconciliationRequired":false},"observed":null,"storageError":null,"retentionError":null});
    let mut body = result;
    for (k, v) in [
        ("epoch", request["sessionId"].clone()),
        ("sessionId", request["sessionId"].clone()),
        ("operationId", request["operationId"].clone()),
        ("replayed", json!(false)),
        ("operationStatus", status),
    ] {
        body[k] = v;
    }
    Response {
        code: if success { 200 } else { 400 },
        body,
    }
}
#[test]
fn valid_durable_effects_and_not_executed_remain_accepted() {
    for success in [true, false] {
        let mut c = campaign();
        let r = create_request();
        let response = terminal_response(&r, success);
        c.accept_response("/v1/mechanical", Some(&r), &response)
            .unwrap();
        assert!(!c.uncertain);
    }
}
#[test]
fn malformed_durable_phase_digest_or_flags_stop() {
    for mode in 0..6 {
        let mut c = campaign();
        let r = create_request();
        let mut response = terminal_response(&r, true);
        match mode {
            0 => response.body["operationStatus"]["durable"]["record"]["phase"] = json!("failed"),
            1 => {
                response.body["operationStatus"]["durable"]["record"]["requestDigest"] =
                    json!("bad")
            }
            2 => response.body["operationStatus"]["reconciliationRequired"] = json!("false"),
            3 => response.body["operationStatus"]["storageError"] = json!({"code":"bad"}),
            4 => response.body["operationStatus"]["nativeExecutionAuthorized"] = json!(true),
            _ => response.body["replayed"] = json!(null),
        };
        assert!(c
            .accept_response("/v1/mechanical", Some(&r), &response)
            .is_err());
        assert!(c.uncertain);
    }
}
fn projection() -> Value {
    let doc = json!("d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000002");
    let rev = format!("{ASSEMBLY_PREFIX}{}", "a".repeat(64));
    let cs=(0..2).map(|i|{let(name,attrs)=expected_definition(i,true);let attrs=attrs.as_object().unwrap().iter().map(|(k,v)|(k.clone(),json!(serde_json::to_string(v).unwrap()))).collect::<serde_json::Map<_,_>>();let id=format!("c:{}:{:016x}",doc.as_str().unwrap(),i+1);json!({"componentId":id,"ref":{"documentId":doc,"revision":rev,"objectType":"Component","componentId":id},"name":name,"revision":"component-revision","attributes":attrs,"partNumber":null,"description":null,"bodies":0,"sketches":0,"occurrences":0,"meshBodies":0,"joints":0,"asBuiltJoints":0,"rigidGroups":0,"assemblyConstraints":0,"motionLinks":0})}).collect::<Vec<_>>();
    let os=[0,1,0].into_iter().enumerate().map(|(i,c)|json!({"component":c,"componentId":cs[c]["componentId"],"occurrenceId":format!("o:{}:{:016x}",doc.as_str().unwrap(),i+1),"name":format!("fixture:{i}"),"path":format!("fixture:{i}"),"matrix":[1.,0.,0.,0.,0.,1.,0.,0.,0.,0.,1.,0.,0.,0.,0.,1.],"grounded":false,"groundedToParent":true,"referenced":false,"configured":false,"suppressed":false,"rolledBack":false,"native":true,"children":0})).collect::<Vec<_>>();
    json!({"target":{"documentId":doc,"revision":rev},"complete":false,"nativeMetadataObserved":false,"cloudMetadata":false,"components":cs,"occurrences":os,"root":{"active":true,"parametric":true,"hybrid":true,"configured":false,"offline":false,"pendingSnapshot":false,"components":3,"occurrences":3,"bodies":0,"sketches":0,"meshBodies":0,"joints":0,"asBuiltJoints":0,"rigidGroups":0,"assemblyConstraints":0,"motionLinks":0,"jointOrigins":0,"revision":"root-revision","timeline":3,"timelinePosition":3}})
}
#[test]
fn shared_quantity_identity_and_json_escaping_are_verified() {
    let p = projection();
    assembly(&p, &p["target"]["documentId"], 2, 3, true, true).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(
            p["components"][0]["attributes"]["supplier"]
                .as_str()
                .unwrap()
        )
        .unwrap(),
        a_metadata()["supplier"]
    );
}
#[test]
fn metadata_identity_or_completeness_corruption_cannot_pass() {
    for i in 0..9 {
        let mut p = projection();
        match i {
            0 => p["components"][0]["partNumber"] = json!("invented"),
            1 => p["components"][0]["description"] = json!(""),
            2 => p["occurrences"][2]["componentId"] = p["components"][1]["componentId"].clone(),
            3 => p["occurrences"][2]["occurrenceId"] = p["occurrences"][0]["occurrenceId"].clone(),
            4 => p["components"][0]["attributes"]["supplier"] = json!("not JSON"),
            5 => p["components"][1]["attributes"]["excludeFromBom"] = json!("false"),
            6 => p["components"][0]["ref"]["documentId"] = json!("foreign"),
            7 => p["root"]["pendingSnapshot"] = json!(true),
            _ => p["components"][0]["revision"] = json!(""),
        };
        assert!(
            assembly(&p, &p["target"]["documentId"], 2, 3, true, true).is_err(),
            "case{i}"
        );
    }
}
#[test]
fn every_matrix_coefficient_and_unsupported_occurrence_flag_checked() {
    for i in 0..16 {
        let mut p = projection();
        p["occurrences"][2]["matrix"][i] = json!(0.125);
        assert!(assembly(&p, &p["target"]["documentId"], 2, 3, true, true).is_err());
    }
    for field in ["referenced", "configured", "suppressed", "rolledBack"] {
        let mut p = projection();
        p["occurrences"][1][field] = json!(true);
        assert!(assembly(&p, &p["target"]["documentId"], 2, 3, true, true).is_err());
    }
}
#[test]
fn unregistered_changed_and_foreign_requests_never_reach_socket() {
    let mut c = campaign();
    let r = create_request();
    assert!(c.allowed("/v1/mechanical", Some(&r)).is_err());
    c.permit("a", r.clone()).unwrap();
    c.allowed("/v1/mechanical", Some(&r)).unwrap();
    for field in ["name", "metadata", "target", "op"] {
        let mut r = r.clone();
        r[field] = json!("tampered");
        assert!(c.request("bad", "/v1/mechanical", Some(&r)).is_err());
        assert_eq!(c.count, 0);
    }
    for op in ["native_scratch_discard", "export", "execute_text_command"] {
        let mut r = r.clone();
        r["op"] = json!(op);
        assert!(c.allowed("/v1/mechanical", Some(&r)).is_err());
    }
}
#[test]
fn repeated_effect_requires_same_confirmed_operation() {
    let mut c = campaign();
    let r = create_request();
    c.permit("a", r.clone()).unwrap();
    c.attempts
        .push(json!({"operationId":r["operationId"],"settlement":null}));
    assert!(c.allowed("/v1/mechanical", Some(&r)).is_err());
    c.attempts[0]["settlement"] = json!("durable_succeeded");
    assert!(c.allowed("/v1/mechanical", Some(&r)).is_err());
    c.successful.push(json!({"request":r}));
    c.allowed("/v1/mechanical", Some(&r)).unwrap();
    let mut conflict = r.clone();
    conflict["name"] = json!("Assembly fixture conflict");
    c.allowed("/v1/mechanical", Some(&conflict)).unwrap();
    conflict["name"] = json!("arbitrary change");
    assert!(c.allowed("/v1/mechanical", Some(&conflict)).is_err());
}
#[test]
fn every_effect_stage_requires_its_own_acknowledgment() {
    let p = projection();
    let body = json!({"nativeExecutionStarted":true,"automaticReplayAllowed":false,"rollbackAttempted":false,"legacyParity":false,"componentId":p["components"][0]["componentId"],"data":p,"stages":[{"stage":"componentMetadata","key":"name","attempt":{"invoked":true,"acknowledged":true,"component":0,"occurrence":null}},{"stage":"componentMetadata","key":"asPurchased","attempt":{"invoked":true,"acknowledged":true,"component":0,"occurrence":null}},{"stage":"componentMetadata","key":"supplierPartNumber","attempt":{"invoked":true,"acknowledged":true,"component":0,"occurrence":null}}]});
    stages(&body, "component_metadata", Some(&a_changes()), 0).unwrap();
    for i in 0..3 {
        for k in ["invoked", "acknowledged"] {
            let mut b = body.clone();
            b["stages"][i]["attempt"][k] = json!(false);
            assert!(stages(&b, "component_metadata", Some(&a_changes()), 0).is_err());
        }
    }
    let mut b = body.clone();
    b["stages"].as_array_mut().unwrap().swap(1, 2);
    assert!(stages(&b, "component_metadata", Some(&a_changes()), 0).is_err());
}
#[test]
fn reviewed_create_metadata_and_shared_insert_are_valid_wire_requests() {
    let r = create_request();
    fusion_mechanical_core::prepare_request(r.clone(), r["sessionId"].as_str().unwrap()).unwrap();
    let p = projection();
    for op in ["component_metadata", "occurrence_insert"] {
        let mut q = json!({"op":op,"sessionId":r["sessionId"],"operationId":"wire-check","target":p["target"],"component":p["components"][0]["ref"]});
        if op == "component_metadata" {
            q["changes"] = a_changes()
        } else {
            q["independentCopy"] = json!(false)
        }
        fusion_mechanical_core::prepare_request(q, r["sessionId"].as_str().unwrap()).unwrap();
    }
}

fn authority_fixture() -> (Value, Value, Vec<u8>) {
    let r = json!({"pid":123,"epoch":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","before":{"documents":[{"documentId":"d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001","creationId":"blank","active":true,"saved":false,"modified":false,"name":"Untitled"}]}});
    let raw = serde_json::to_vec(&r).unwrap();
    let a = json!({"schemaVersion":1,"purpose":"r20-native-assembly-qualification","pid":123,"epoch":r["epoch"],"runtimeSha256":format!("{:x}",Sha256::digest(&raw)),"processCreationTime100ns":"134365647242566202","allowCreateOneTaskDocument":true,"allowTwoNativeComponentDefinitions":true,"allowOneSharedOccurrence":true,"allowTwoLocalMetadataUpdates":true,"fixture":"two-definitions-three-occurrences-local-metadata-v1","campaignId":"assembly20261015","baselineDocumentCount":1,"baselineSha256":hash_value(&r["before"]).unwrap(),"priorTaskDocumentIds":[]});
    (r, a, raw)
}
#[test]
fn exact_authority_runtime_permissions_and_baseline_are_required() {
    let (r, a, raw) = authority_fixture();
    validate_authority(&r, &a, &raw).unwrap();
    for field in [
        "runtimeSha256",
        "pid",
        "epoch",
        "baselineSha256",
        "purpose",
        "fixture",
        "campaignId",
        "processCreationTime100ns",
        "allowCreateOneTaskDocument",
        "allowTwoNativeComponentDefinitions",
        "allowOneSharedOccurrence",
        "allowTwoLocalMetadataUpdates",
    ] {
        let mut bad = a.clone();
        bad[field] = json!(false);
        assert!(validate_authority(&r, &bad, &raw).is_err(), "{field}");
    }
    for field in ["permittedRequests", "documentId", "allowCleanup"] {
        let mut bad = a.clone();
        bad[field] = json!(true);
        assert!(validate_authority(&r, &bad, &raw).is_err());
    }
    let mut extra = raw.clone();
    extra.push(b' ');
    assert!(validate_authority(&r, &a, &extra).is_err());
    let mut bad = a.clone();
    bad["priorTaskDocumentIds"] = json!(["foreign"]);
    assert!(validate_authority(&r, &bad, &raw).is_err());
}
#[test]
fn all_root_and_component_foreign_solver_graphs_are_rejected() {
    for field in [
        "bodies",
        "sketches",
        "meshBodies",
        "joints",
        "asBuiltJoints",
        "rigidGroups",
        "assemblyConstraints",
        "motionLinks",
        "jointOrigins",
    ] {
        let mut p = projection();
        p["root"][field] = json!(1);
        assert!(
            assembly(&p, &p["target"]["documentId"], 2, 3, true, true).is_err(),
            "root {field}"
        );
    }
    for field in [
        "bodies",
        "sketches",
        "occurrences",
        "meshBodies",
        "joints",
        "asBuiltJoints",
        "rigidGroups",
        "assemblyConstraints",
        "motionLinks",
    ] {
        let mut p = projection();
        p["components"][0][field] = json!(1);
        assert!(
            assembly(&p, &p["target"]["documentId"], 2, 3, true, true).is_err(),
            "component {field}"
        );
    }
}
#[test]
fn shared_add_must_report_the_actual_new_occurrence_and_existing_definition() {
    let p = projection();
    let body = json!({"nativeExecutionStarted":true,"automaticReplayAllowed":false,"rollbackAttempted":false,"legacyParity":false,"componentId":p["components"][0]["componentId"],"data":p,"stages":[{"stage":"occurrenceAdd","attempt":{"invoked":true,"acknowledged":true,"component":0,"occurrence":2}}]});
    stages(&body, "occurrence_insert", None, 0).unwrap();
    for (field, value) in [
        ("component", json!(1)),
        ("occurrence", json!(1)),
        ("invoked", json!(false)),
        ("acknowledged", json!(false)),
    ] {
        let mut b = body.clone();
        b["stages"][0]["attempt"][field] = value;
        assert!(stages(&b, "occurrence_insert", None, 0).is_err());
    }
}
#[test]
fn online_reads_and_foreign_routes_are_not_in_this_campaign() {
    let c = campaign();
    let read = json!({"op":"native_assembly_inspect","sessionId":c.runtime["epoch"],"target":{"documentId":c.fixture["documentId"]},"nativeMetadata":false});
    c.allowed("/v1/mechanical", Some(&read)).unwrap();
    for (field, value) in [
        ("nativeMetadata", json!(true)),
        ("target", json!({"documentId":"foreign"})),
        ("sessionId", json!("foreign")),
    ] {
        let mut r = read.clone();
        r[field] = value;
        assert!(c.allowed("/v1/mechanical", Some(&r)).is_err());
    }
    assert!(c.allowed("/command", Some(&read)).is_err());
    assert!(c
        .allowed("/v1/operations/foreign/r20-fixture20261015-a", None)
        .is_err());
}

#[test]
fn dynamic_read_epoch_is_required_and_static_present_epoch_must_match() {
    for route in [
        "/v1/health",
        "/v1/documents",
        "/v1/mechanical",
        "/v1/capabilities",
    ] {
        let request = (route == "/v1/mechanical").then(|| json!({"op":"native_sketch_inspect"}));
        for epoch in [None, Some(json!("foreign")), Some(json!(null))] {
            let mut c = campaign();
            let mut body = json!({"success":true});
            if let Some(e) = epoch.clone() {
                body["epoch"] = e;
            }
            let accepted =
                c.accept_response(route, request.as_ref(), &Response { code: 200, body });
            if route == "/v1/capabilities" && epoch.is_none() {
                assert!(accepted.is_ok());
                assert!(!c.uncertain);
            } else {
                assert!(accepted.is_err());
                assert!(c.uncertain);
                let r = create_request();
                c.permit("a", r.clone()).unwrap();
                assert!(c
                    .request("blocked-effect", "/v1/mechanical", Some(&r))
                    .is_err());
                assert_eq!(c.count, 0);
                assert!(c.attempts.is_empty());
            }
        }
        let mut c = campaign();
        let body = json!({"success":true,"epoch":c.runtime["epoch"]});
        c.accept_response(route, request.as_ref(), &Response { code: 200, body })
            .unwrap();
        assert!(!c.uncertain);
    }
}
