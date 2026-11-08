//! The single read-settling allowance is available only after acknowledged effects.
use super::*;
pub(super) const MAX_CAPTURES: usize = 3;
pub(super) const MAX_SETTLING_DIAGNOSTIC_BYTES: usize = 8 * 1024;
#[derive(Clone, Debug)]
pub struct Capture {
    pub projection: Projection,
    pub root_after: Root,
}
pub(super) struct Trace {
    captures: Vec<Capture>,
    attempted: usize,
    settling_used: bool,
    projection_mismatch: Option<Value>,
    component_revision_changes: Vec<usize>,
    revision_payload_omitted: bool,
}
impl Trace {
    pub(super) fn new() -> Result<Self> {
        let mut captures = Vec::new();
        captures
            .try_reserve_exact(MAX_CAPTURES)
            .map_err(|_| "native_assembly_readback_capacity")?;
        let mut component_revision_changes = Vec::new();
        component_revision_changes
            .try_reserve_exact(MAX_COMPONENTS)
            .map_err(|_| "native_assembly_readback_capacity")?;
        Ok(Self {
            captures,
            attempted: 0,
            settling_used: false,
            projection_mismatch: None,
            component_revision_changes,
            revision_payload_omitted: false,
        })
    }
    pub(super) fn next<B: Backend>(&mut self, b: &B, h: &B::Handle, cloud: bool) -> Result<usize> {
        if self.attempted >= MAX_CAPTURES {
            return Err("native_assembly_readback_bound".into());
        }
        if b.command()? != "SelectCommand" {
            return Err("native_assembly_command_changed_after_effect".into());
        }
        self.attempted += 1;
        let captured = b.capture(h, cloud)?;
        // The complete returned projection is retained before any validation/refusal.
        self.captures.push(captured);
        let i = self.captures.len() - 1;
        let c = &self.captures[i];
        if c.projection.cloud_metadata != cloud {
            return Err("native_assembly_metadata_mode_mismatch".into());
        }
        validate(&c.projection)?;
        if c.root_after.revision.is_empty()
            || !text(&c.root_after.revision)
            || !text(&c.root_after.name)
        {
            return Err("native_assembly_projection_bound".into());
        }
        Ok(i)
    }
    pub(super) fn evidence(&self) -> Value {
        let captures:Vec<_>=self.captures.iter().enumerate().map(|(i,c)|{
            let projection=bounded_json(&c.projection,MAX_BYTES);
            let before=bounded_json(&c.projection.root,64*1024);
            let after=bounded_json(&c.root_after,64*1024);
            let digest=|v:&Option<Vec<u8>>|v.as_ref().map(|b|fusion_mechanical_journal::request_digest(b));
            let roots_bounded=[&c.projection.root,&c.root_after].iter().all(|r|text(&r.name)&&text(&r.revision));
            let changed=if roots_bounded {root_capture_diagnostic(&c.projection.root,&c.root_after)}else{
                Some(json!({"kind":"invalid_root_text_omitted","payloadUnavailable":true,"expectedNameBytes":c.projection.root.name.len(),"observedNameBytes":c.root_after.name.len(),"expectedRevisionBytes":c.projection.root.revision.len(),"observedRevisionBytes":c.root_after.revision.len(),"expectedNameSha256":fusion_mechanical_journal::request_digest(c.projection.root.name.as_bytes()),"observedNameSha256":fusion_mechanical_journal::request_digest(c.root_after.name.as_bytes()),"expectedRevisionSha256":fusion_mechanical_journal::request_digest(c.projection.root.revision.as_bytes()),"observedRevisionSha256":fusion_mechanical_journal::request_digest(c.root_after.revision.as_bytes())}))
            };
            json!({"index":i,"projectionSha256":digest(&projection),"projectionBytes":projection.as_ref().map(Vec::len),"projectionJsonOmitted":projection.is_none(),"projectionContainsNonFinite":c.projection.occurrences.iter().any(|o|o.matrix.iter().any(|v|!v.is_finite())),"rootBeforeSha256":digest(&before),"rootAfterSha256":digest(&after),"rootStable":c.projection.root==c.root_after,"rootChangeDiagnostic":changed})
        }).collect();
        json!({"policy":"post-effect-initial-root-and-component-revision-settling-v2","maximumCaptures":MAX_CAPTURES,"captureAttempts":self.attempted,"settlingUsed":self.settling_used,"captures":captures,"firstProjectionDifference":self.projection_mismatch,"initialComponentRevisionChanges":self.component_changes(!self.revision_payload_omitted),"combinedSettlingDiagnosticLimitBytes":MAX_SETTLING_DIAGNOSTIC_BYTES,"componentRevisionPayloadUnavailable":self.revision_payload_omitted,"wholeProjectionsRetainedForComparison":true,"effectRetryAttempted":false,"acceptedRevisionIgnored":false})
    }
    fn component_changes(&self, expose: bool) -> Vec<Value> {
        self.component_revision_changes.iter().map(|&i| {
            let before=&self.captures[0].projection.components[i].revision;
            let after=&self.captures[1].projection.components[i].revision;
            json!({"componentSlot":i,"expected":if expose{Some(before)}else{None},"observed":if expose{Some(after)}else{None},"expectedBytes":before.len(),"observedBytes":after.len(),"expectedSha256":fusion_mechanical_journal::request_digest(before.as_bytes()),"observedSha256":fusion_mechanical_journal::request_digest(after.as_bytes()),"payloadUnavailable":!expose})
        }).collect()
    }
}
fn strict(c: &Capture) -> Result<()> {
    if c.projection.root != c.root_after {
        return Err("native_assembly_root_changed_during_capture".into());
    }
    Ok(())
}
pub(super) fn snapshot<B: Backend>(
    b: &B,
    h: &B::Handle,
    cloud: bool,
    t: &mut Trace,
) -> Result<Projection> {
    let first = t.next(b, h, cloud)?;
    let c = &t.captures[first];
    if c.projection.root == c.root_after {
        let second = t.next(b, h, cloud)?;
        strict(&t.captures[second])?;
        if t.captures[first].projection != t.captures[second].projection {
            t.projection_mismatch = projection_difference(
                &t.captures[first].projection,
                &t.captures[second].projection,
                "stable_pair",
            );
            return Err("native_assembly_projection_changed".into());
        }
        return Ok(t.captures[second].projection.clone());
    }
    let mut normalized = c.projection.clone();
    normalized.root.revision = c.root_after.revision.clone();
    if normalized.root != c.root_after {
        return Err("native_assembly_root_changed_during_capture".into());
    }
    let diagnostic =
        root_capture_diagnostic(&c.projection.root, &c.root_after).expect("changed root");
    if serde_json::to_vec(&diagnostic)
        .map_err(|_| "native_assembly_serialization")?
        .len()
        > MAX_SETTLING_DIAGNOSTIC_BYTES
    {
        return Err("native_assembly_settling_diagnostic_bound".into());
    }
    // One discarded initial capture only. No errors or later changes are retried.
    t.settling_used = true;
    let second = t.next(b, h, cloud)?;
    strict(&t.captures[second])?;
    // A first root drift permits only initial component revision witnesses to
    // settle as well. Exact membership and every other field still compare.
    if normalized.components.len() == t.captures[second].projection.components.len() {
        for (i, (before, after)) in normalized
            .components
            .iter_mut()
            .zip(&t.captures[second].projection.components)
            .enumerate()
        {
            if before.revision != after.revision {
                t.component_revision_changes.push(i);
                before.revision = after.revision.clone();
            }
        }
    }
    // A failure while formatting/checking the combined budget must itself
    // leave the safe hash-only representation selected in the retained trace.
    t.revision_payload_omitted = true;
    let combined = json!({"rootChangeDiagnostic":diagnostic,"componentRevisionChanges":t.component_changes(true)});
    if bounded_json(&combined, MAX_SETTLING_DIAGNOSTIC_BYTES).is_none() {
        // Preserve all changed slots and complete hashes/lengths without
        // flooding the bounded observation with failed raw revision strings.
        return Err("native_assembly_settling_diagnostic_bound".into());
    }
    t.revision_payload_omitted = false;
    if normalized != t.captures[second].projection {
        t.projection_mismatch = projection_difference(
            &normalized,
            &t.captures[second].projection,
            "initial_projection_with_root_after_vs_first_stable",
        );
        return Err("native_assembly_settling_projection_changed".into());
    }
    let third = t.next(b, h, cloud)?;
    strict(&t.captures[third])?;
    if t.captures[second].projection != t.captures[third].projection {
        t.projection_mismatch = projection_difference(
            &t.captures[second].projection,
            &t.captures[third].projection,
            "settled_pair",
        );
        return Err("native_assembly_projection_changed".into());
    }
    Ok(t.captures[third].projection.clone())
}

// Fixed-cap serializer for diagnostics, including invalid backend observations.
// Rejected oversized input never causes an unbounded duplicate allocation.
fn bounded_json(value: &impl Serialize, limit: usize) -> Option<Vec<u8>> {
    struct Writer {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other("diagnostic bound"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut w = Writer {
        bytes: Vec::new(),
        limit,
    };
    w.bytes.try_reserve_exact(limit).ok()?;
    serde_json::to_writer(&mut w, value).ok()?;
    Some(w.bytes)
}

// A single bounded first difference avoids another diagnostic app cycle.
// Complete projections remain in Trace; evidence binds values without duplication.
pub(super) fn projection_difference(
    before: &Projection,
    after: &Projection,
    comparison: &str,
) -> Option<Value> {
    let expected = serde_json::to_value(before).ok()?;
    let observed = serde_json::to_value(after).ok()?;
    fn first<'a>(
        a: Option<&'a Value>,
        b: Option<&'a Value>,
        path: String,
    ) -> Option<(String, Option<&'a Value>, Option<&'a Value>)> {
        if a == b {
            return None;
        }
        match (a, b) {
            (Some(Value::Object(a)), Some(Value::Object(b))) => {
                for (key, av) in a {
                    let escaped = key.replace('~', "~0").replace('/', "~1");
                    if let Some(d) = first(Some(av), b.get(key), format!("{path}/{escaped}")) {
                        return Some(d);
                    }
                }
                for (key, bv) in b {
                    if !a.contains_key(key) {
                        let escaped = key.replace('~', "~0").replace('/', "~1");
                        return Some((format!("{path}/{escaped}"), None, Some(bv)));
                    }
                }
                None
            }
            (Some(Value::Array(a)), Some(Value::Array(b))) => {
                for i in 0..a.len().max(b.len()) {
                    if let Some(d) = first(a.get(i), b.get(i), format!("{path}/{i}")) {
                        return Some(d);
                    }
                }
                None
            }
            _ => Some((path, a, b)),
        }
    }
    fn summary(v: Option<&Value>) -> Value {
        let Some(v) = v else {
            return json!({"present":false});
        };
        let bytes = serde_json::to_vec(v).expect("JSON value serializable");
        let expose = !v.is_array() && !v.is_object() && bytes.len() <= 256;
        json!({"present":true,"serializedBytes":bytes.len(),"sha256":fusion_mechanical_journal::request_digest(&bytes),"value":if expose{Some(v)}else{None},"payloadUnavailable":!expose})
    }
    let (path, a, b) = first(Some(&expected), Some(&observed), String::new())?;
    let path_json = serde_json::to_vec(&path).ok()?;
    let short = path_json.len() <= 256;
    Some(
        json!({"comparison":comparison,"path":if short{Some(&path)}else{None},"pathSha256":fusion_mechanical_journal::request_digest(path.as_bytes()),"pathBytes":path.len(),"pathUnavailable":!short,"expected":summary(a),"observed":summary(b),"additionalSdkReads":0}),
    )
}
