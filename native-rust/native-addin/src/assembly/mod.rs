//! Finite root assembly authoring. Exact persistent SDK leases stay on the owner
//! thread; plans contain data only. The coordinator journals start before execute.
use fusion_mechanical_core::PreparedRequest;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    marker::PhantomData,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    thread::{self, ThreadId},
};
mod arguments;
pub mod placement;
mod readback;
pub use arguments::{is_write, supported_arguments, Action, Change};
pub use readback::Capture;
#[cfg(test)]
pub(crate) mod placement_tests;
#[cfg(test)]
mod readback_tests;
#[cfg(test)]
pub(crate) mod tests;
pub const CONTRACT: &str = "native-root-assembly-v1";
pub const SCOPE:&str="root-assembly-projection-v1: current-session exact native flat root components/occurrences, revisions, names, AdomMechanical metadata, identity-orientation placements including bounded captured translations, bounded resource/timeline state; no child-solid authoring, arbitrary adoption, rollback or full legacy parity";
pub const PREFIX: &str = "root-assembly-projection-v1:";
pub const MAX_DOCUMENTS: usize = 16;
pub const MAX_COMPONENTS: usize = 32;
pub const MAX_OCCURRENCES: usize = 64;
pub const MAX_BYTES: usize = 128 * 1024;
pub type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Root {
    pub revision: String,
    pub name: String,
    pub active: bool,
    pub parametric: bool,
    pub hybrid: bool,
    pub configured: bool,
    pub offline: bool,
    pub pending_snapshot: bool,
    pub components: u64,
    pub occurrences: u64,
    pub timeline: u64,
    pub timeline_position: u64,
    pub bodies: u64,
    pub sketches: u64,
    pub mesh_bodies: u64,
    pub joints: u64,
    pub as_built_joints: u64,
    pub rigid_groups: u64,
    pub assembly_constraints: u64,
    pub motion_links: u64,
    pub joint_origins: u64,
}
/// Preserve the two already-read roots when the exact capture guard refuses.
/// Pure diagnostic construction: no additional SDK calls or tolerance changes.
pub(crate) fn root_capture_diagnostic(before: &Root, after: &Root) -> Option<Value> {
    if before == after {
        return None;
    }
    let expected = json!(before);
    let observed = json!(after);
    let changed_fields: Vec<_> = expected
        .as_object()
        .expect("Root serializes as an object")
        .iter()
        .filter(|(key, value)| observed.get(*key) != Some(*value))
        .map(|(key, _)| key.clone())
        .collect();
    Some(
        json!({"kind":"native_assembly_root_capture_change","errorCode":"native_assembly_root_changed_during_capture","comparison":"exact_equality","changedFields":changed_fields,"expected":expected,"observed":observed,"additionalSdkReads":0,"retryAttempted":false}),
    )
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    pub name: String,
    pub revision: String,
    pub attributes: BTreeMap<String, String>,
    pub part_number: Option<String>,
    pub description: Option<String>,
    pub bodies: u64,
    pub sketches: u64,
    pub occurrences: u64,
    pub mesh_bodies: u64,
    pub joints: u64,
    pub as_built_joints: u64,
    pub rigid_groups: u64,
    pub assembly_constraints: u64,
    pub motion_links: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Occurrence {
    pub component: usize,
    pub name: String,
    pub path: String,
    pub matrix: [f64; 16],
    pub grounded: bool,
    pub grounded_to_parent: bool,
    pub referenced: bool,
    pub configured: bool,
    pub suppressed: bool,
    pub rolled_back: bool,
    pub native: bool,
    pub children: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Projection {
    pub root: Root,
    pub components: Vec<Component>,
    pub occurrences: Vec<Occurrence>,
    pub cloud_metadata: bool,
}
pub struct Open<H> {
    pub handle: Option<H>,
    pub acknowledged: bool,
}
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub invoked: bool,
    pub acknowledged: bool,
    pub component: Option<usize>,
    pub occurrence: Option<usize>,
}
#[derive(Clone, Debug, Serialize)]
struct Stage {
    stage: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    attempt: Attempt,
}
pub trait Backend {
    type Handle;
    fn open(&self) -> Open<Self::Handle>;
    fn command(&self) -> Result<String>;
    fn snapshot(&self, handle: &Self::Handle, cloud: bool) -> Result<Projection>;
    /// Raw bracket for post-effect read settling. Existing strict reads use snapshot.
    fn capture(&self, handle: &Self::Handle, cloud: bool) -> Result<Capture> {
        let projection = self.snapshot(handle, cloud)?;
        Ok(Capture {
            root_after: projection.root.clone(),
            projection,
        })
    }
    fn add(&self, handle: &Self::Handle, component: Option<usize>) -> Attempt;
    fn change(&self, handle: &Self::Handle, component: usize, change: &Change) -> Attempt;
    fn positions(&self, _handle: &Self::Handle) -> Result<placement::PositionState> {
        Err("native_placement_backend_unavailable".into())
    }
    fn translate(
        &self,
        _handle: &Self::Handle,
        _occurrence: usize,
        _matrix: &[f64; 16],
    ) -> Attempt {
        Attempt::default()
    }
    fn capture_position(&self, _handle: &Self::Handle) -> placement::SnapshotAttempt {
        placement::SnapshotAttempt::default()
    }
    fn record_observation_diagnostic(&self, _value: Value) {}
    fn diagnostic(&self) -> Value {
        Value::Null
    }
}
#[derive(Debug)]
pub struct Plan {
    authority: u64,
    document_id: String,
    revision: String,
    before: Projection,
    action: Action,
    placement_before: Option<placement::PositionState>,
}
impl Plan {
    pub fn document_id(&self) -> &str {
        &self.document_id
    }
    pub fn observed_document(&self) -> Value {
        json!({"documentId":self.document_id,"revision":self.revision,"revisionScope":SCOPE})
    }
}
struct Entry<H> {
    document: String,
    handle: Option<H>,
    retired: bool,
}
static NEXT_BOOK: AtomicU64 = AtomicU64::new(1);
fn next_authority() -> u64 {
    let mut old = NEXT_BOOK.load(Ordering::Relaxed);
    loop {
        let Some(next) = old.checked_add(1) else {
            return 0;
        };
        match NEXT_BOOK.compare_exchange_weak(old, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return old,
            Err(value) => old = value,
        }
    }
}
pub struct Book<H> {
    authority: u64,
    entries: Vec<Entry<H>>,
    owner: ThreadId,
    closed: bool,
    faulted: bool,
    release_uncertain: bool,
    _thread: PhantomData<Rc<()>>,
}
impl<H> Default for Book<H> {
    fn default() -> Self {
        Self {
            authority: next_authority(),
            entries: vec![],
            owner: thread::current().id(),
            closed: false,
            faulted: false,
            release_uncertain: false,
            _thread: PhantomData,
        }
    }
}
fn diagnostic<B: Backend>(b: &B) -> Value {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.diagnostic()))
        .unwrap_or_else(|_| json!({"unavailable":"diagnostic_panicked"}))
}
fn error(code: &str) -> Value {
    json!({"success":false,"errorCode":code,"nativeExecutionStarted":false})
}
fn text(s: &str) -> bool {
    s.len() <= 4096 && !s.contains('\0')
}
fn validate(p: &Projection) -> Result<()> {
    validate_projection(p, false)
}
fn validate_projection(p: &Projection, allow_pending: bool) -> Result<()> {
    let r = &p.root;
    if !r.active
        || !r.parametric
        || !r.hybrid
        || r.configured
        || (r.pending_snapshot && !allow_pending)
        || r.mesh_bodies != 0
        || r.joints != 0
        || r.as_built_joints != 0
        || r.rigid_groups != 0
        || r.assembly_constraints != 0
        || r.motion_links != 0
        || r.joint_origins != 0
    {
        return Err("native_assembly_root_scope_unsupported".into());
    }
    if r.revision.is_empty()
        || !text(&r.revision)
        || !text(&r.name)
        || r.timeline > 4096
        || r.timeline_position != r.timeline
        || r.bodies > 128
        || r.sketches > 512
        || p.components.len() > MAX_COMPONENTS
        || p.occurrences.len() > MAX_OCCURRENCES
    {
        return Err("native_assembly_projection_bound".into());
    }
    if r.components != p.components.len() as u64 + 1 || r.occurrences != p.occurrences.len() as u64
    {
        return Err("native_assembly_unowned_membership".into());
    }
    if p.cloud_metadata && r.offline {
        return Err("native_assembly_native_metadata_requires_online".into());
    }
    for c in &p.components {
        if c.revision.is_empty()
            || !text(&c.revision)
            || !text(&c.name)
            || c.bodies != 0
            || c.sketches != 0
            || c.occurrences != 0
            || c.mesh_bodies != 0
            || c.joints != 0
            || c.as_built_joints != 0
            || c.rigid_groups != 0
            || c.assembly_constraints != 0
            || c.motion_links != 0
        {
            return Err("native_assembly_owned_empty_leaf_required".into());
        }
        if c.attributes.len() > 128 || c.attributes.iter().any(|(k, v)| !text(k) || !text(v)) {
            return Err("native_assembly_metadata_bound".into());
        }
        if (if p.cloud_metadata {
            c.part_number.is_none() || c.description.is_none()
        } else {
            c.part_number.is_some() || c.description.is_some()
        }) || c.part_number.as_ref().is_some_and(|v| !text(v))
            || c.description.as_ref().is_some_and(|v| !text(v))
        {
            return Err("native_assembly_metadata_incomplete".into());
        }
    }
    for o in &p.occurrences {
        if o.component >= p.components.len()
            || !text(&o.name)
            || !text(&o.path)
            || o.referenced
            || o.configured
            || o.suppressed
            || o.rolled_back
            || !o.native
            || o.children != 0
        {
            return Err("native_assembly_occurrence_scope_unsupported".into());
        }
        for i in 0..16 {
            if [3, 7, 11].contains(&i) {
                if !o.matrix[i].is_finite() || o.matrix[i].abs() > placement::MAX_MM / 10. {
                    return Err("native_placement_position_bound".into());
                }
                continue;
            }
            let expected = if i % 5 == 0 { 1. } else { 0. };
            if !o.matrix[i].is_finite() || (o.matrix[i] - expected).abs() > 1e-10 {
                return Err("native_assembly_identity_orientation_required".into());
            }
        }
    }
    if p.components
        .iter()
        .enumerate()
        .any(|(i, _)| !p.occurrences.iter().any(|o| o.component == i))
    {
        return Err("native_assembly_definition_without_occurrence".into());
    }
    if serde_json::to_vec(p)
        .map_err(|_| "native_assembly_serialization")?
        .len()
        > MAX_BYTES
    {
        return Err("native_assembly_projection_bound".into());
    }
    Ok(())
}
fn snapshot<B: Backend>(b: &B, h: &B::Handle, cloud: bool) -> Result<Projection> {
    let a = b.snapshot(h, cloud)?;
    if a.cloud_metadata != cloud {
        return Err("native_assembly_metadata_mode_mismatch".into());
    }
    validate(&a)?;
    let z = b.snapshot(h, cloud)?;
    if z.cloud_metadata != cloud {
        return Err("native_assembly_metadata_mode_mismatch".into());
    }
    validate(&z)?;
    if a != z {
        return Err("native_assembly_projection_changed".into());
    }
    Ok(a)
}
fn revision(p: &Projection) -> Result<String> {
    let bytes = serde_json::to_vec(p).map_err(|_| "native_assembly_serialization")?;
    Ok(format!(
        "{PREFIX}{}",
        fusion_mechanical_journal::request_digest(&bytes)
    ))
}
fn component_id(doc: &str, i: usize) -> String {
    format!("c:{doc}:{:016x}", i + 1)
}
fn occurrence_id(doc: &str, i: usize) -> String {
    format!("o:{doc}:{:016x}", i + 1)
}
fn output(doc: &str, p: &Projection) -> Result<Value> {
    let revision = revision(p)?;
    let mut data = serde_json::to_value(p).map_err(|_| "native_assembly_serialization")?;
    for (i, c) in data["components"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        c["componentId"] = json!(component_id(doc, i));
        c["ref"] = json!({"documentId":doc,"revision":revision,"objectType":"Component","componentId":component_id(doc,i)});
    }
    for (i, o) in data["occurrences"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        o["occurrenceId"] = json!(occurrence_id(doc, i));
        o["componentId"] = json!(component_id(doc, p.occurrences[i].component));
    }
    data["target"] = json!({"documentId":doc,"revision":revision});
    data["revisionScope"] = json!(SCOPE);
    data["complete"] = json!(false);
    data["nativeMetadataObserved"] = json!(p.cloud_metadata);
    Ok(data)
}
impl<H> Book<H> {
    fn ready(&self) -> Result<()> {
        if self.owner != thread::current().id() {
            return Err("native_assembly_wrong_thread".into());
        }
        if self.authority == 0 || self.closed || self.faulted || self.release_uncertain {
            return Err("native_assembly_book_unavailable".into());
        }
        Ok(())
    }
    fn index<B: Backend<Handle = H>>(&mut self, b: &B, doc: &str) -> Result<usize> {
        self.ready()?;
        if doc.is_empty() || doc.len() > 128 {
            return Err("native_assembly_document_id_bound".into());
        }
        if let Some(i) = self.entries.iter().position(|e| e.document == doc) {
            if self.entries[i].retired {
                return Err("native_assembly_document_retired".into());
            }
            return Ok(i);
        }
        if self.entries.len() >= MAX_DOCUMENTS {
            return Err("native_assembly_document_capacity".into());
        }
        self.entries
            .try_reserve(1)
            .map_err(|_| "native_assembly_capacity")?;
        let i = self.entries.len();
        self.entries.push(Entry {
            document: doc.into(),
            handle: None,
            retired: true,
        });
        self.faulted = true;
        let opened = b.open();
        self.entries[i].handle = opened.handle;
        if !opened.acknowledged || self.entries[i].handle.is_none() {
            return Err("native_assembly_open_unconfirmed".into());
        }
        self.entries[i].retired = false;
        self.faulted = false;
        Ok(i)
    }
    pub fn inspect<B: Backend<Handle = H>>(
        &mut self,
        b: &B,
        doc: &str,
        expected: Option<&str>,
        cloud: bool,
    ) -> Result<Value> {
        let i = self.index(b, doc)?;
        let result = snapshot(b, self.entries[i].handle.as_ref().unwrap(), cloud);
        let p = match result {
            Ok(p) => p,
            Err(e) => {
                if e != "native_assembly_native_metadata_requires_online" {
                    self.entries[i].retired = true;
                }
                return Err(e);
            }
        };
        let digest = revision(&p)?;
        if expected.is_some_and(|s| s != digest) {
            return Err("target_revision_mismatch".into());
        }
        Ok(
            json!({"success":true,"data":output(doc,&p)?,"nativeExecutionStarted":false,"legacyParity":false}),
        )
    }
    pub fn preflight<B: Backend<Handle = H>>(
        &mut self,
        b: &B,
        r: &PreparedRequest,
    ) -> Result<Plan> {
        supported_arguments(r).map_err(str::to_owned)?;
        let action = arguments::parse(r).map_err(str::to_owned)?;
        let doc = r.arguments()["target"]["documentId"].as_str().unwrap();
        let i = self.index(b, doc)?;
        if b.command()? != "SelectCommand" {
            return Err("native_assembly_idle_command_required".into());
        }
        let p = snapshot(b, self.entries[i].handle.as_ref().unwrap(), action.cloud())?;
        let mut placement_before = None;
        let rev = revision(&p)?;
        if r.arguments()["target"]["revision"].as_str() != Some(&rev) {
            return Err("target_revision_mismatch".into());
        }
        match &action {
            Action::Create { .. } => {
                if p.components.len() >= MAX_COMPONENTS || p.occurrences.len() >= MAX_OCCURRENCES {
                    return Err("native_assembly_capacity".into());
                }
            }
            Action::Insert { component_id: id } => {
                resolve_component(doc, id, &p)?;
                if p.occurrences.len() >= MAX_OCCURRENCES {
                    return Err("native_assembly_capacity".into());
                }
            }
            Action::Metadata {
                component_id: id, ..
            } => {
                resolve_component(doc, id, &p)?;
            }
            Action::Translate {
                occurrence_id,
                position_mm,
            } => {
                placement::admit(doc, occurrence_id, position_mm, &p)?;
                let (guarded, positions) =
                    placement::observe(b, self.entries[i].handle.as_ref().unwrap(), false)?;
                if guarded != p {
                    return Err("target_revision_mismatch".into());
                }
                if positions.snapshots.len() >= placement::MAX_SNAPSHOTS
                    || positions.timeline >= 4096
                {
                    return Err("native_placement_snapshot_capacity".into());
                }
                placement_before = Some(positions);
            }
        }
        Ok(Plan {
            authority: self.authority,
            document_id: doc.into(),
            revision: rev,
            before: p,
            action,
            placement_before,
        })
    }
    pub fn execute<B: Backend<Handle = H>>(&mut self, b: &B, plan: Plan) -> Value {
        if matches!(plan.action, Action::Translate { .. }) {
            return self.execute_translation(b, plan);
        }
        if let Err(e) = self.ready() {
            return error(&e);
        }
        if plan.authority != self.authority {
            return error("native_assembly_foreign_plan");
        }
        let Some(i) = self
            .entries
            .iter()
            .position(|e| e.document == plan.document_id && !e.retired && e.handle.is_some())
        else {
            return error("native_assembly_document_retired");
        };
        let h = self.entries[i].handle.as_ref().unwrap();
        let mut stages = match prepare_stages(&plan.action) {
            Ok(v) => v,
            Err(e) => return error(&e),
        };
        let mut readback_trace = match readback::Trace::new() {
            Ok(v) => v,
            Err(e) => return error(&e),
        };
        let mut completed_stages = 0;
        let mut invoked = false;
        let mut target = None;
        let before = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
            if b.command()? != "SelectCommand" {
                return Err("native_assembly_idle_command_required".into());
            }
            if snapshot(b, h, plan.action.cloud())? != plan.before {
                return Err("target_revision_mismatch".into());
            }
            Ok(())
        }));
        match before {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return error(&e),
            Err(_) => return error("native_assembly_pre_effect_panic"),
        }
        self.faulted = true;
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Projection> {
                match &plan.action {
                    Action::Create { .. } | Action::Insert { .. } => {
                        let existing = match &plan.action {
                            Action::Insert { component_id: id } => {
                                Some(resolve_component(&plan.document_id, id, &plan.before)?)
                            }
                            _ => None,
                        };
                        // Invoked is conservatively true across a panicking SDK boundary.
                        invoked = true;
                        stages[completed_stages].attempt.invoked = true;
                        completed_stages += 1;
                        let a = b.add(h, existing);
                        invoked = a.invoked;
                        stages[completed_stages - 1].attempt = a.clone();
                        if !a.invoked || !a.acknowledged {
                            return Err("native_assembly_add_unconfirmed".into());
                        }
                        let ci = existing.unwrap_or(plan.before.components.len());
                        if a.component != Some(ci)
                            || a.occurrence != Some(plan.before.occurrences.len())
                        {
                            return Err("native_assembly_add_identity_mismatch".into());
                        }
                        target = Some(ci);
                    }
                    Action::Metadata {
                        component_id: id, ..
                    } => {
                        target = Some(resolve_component(&plan.document_id, id, &plan.before)?);
                    }
                    Action::Translate { .. } => {
                        return Err("native_placement_dispatch_mismatch".into())
                    }
                }
                let ci = target.unwrap();
                for change in plan.action.changes() {
                    let was = invoked;
                    invoked = true;
                    stages[completed_stages].attempt.invoked = true;
                    stages[completed_stages].attempt.component = Some(ci);
                    completed_stages += 1;
                    let a = b.change(h, ci, change);
                    invoked = was || a.invoked;
                    stages[completed_stages - 1].attempt = a.clone();
                    if !a.invoked || !a.acknowledged {
                        return Err("native_assembly_metadata_unconfirmed".into());
                    }
                }
                if b.command()? != "SelectCommand" {
                    return Err("native_assembly_command_changed_after_effect".into());
                }
                let after = readback::snapshot(b, h, plan.action.cloud(), &mut readback_trace)?;
                verify_delta(&plan, &after, ci)?;
                Ok(after)
            }));
        stages.truncate(completed_stages);
        // Observation formatting is inside its own guard: a diagnostic failure
        // cannot erase the already-recorded effect prefix or report success.
        let (result, readback_evidence) = match std::panic::catch_unwind(
            std::panic::AssertUnwindSafe(|| readback_trace.evidence()),
        ) {
            Ok(e) => (result, e),
            Err(_) => (
                Ok(Err("native_assembly_readback_evidence_unavailable".into())),
                json!({"payloadUnavailable":true,"errorCode":"native_assembly_readback_evidence_panic","effectRetryAttempted":false}),
            ),
        };
        match result {
            Ok(Ok(after)) => match output(&plan.document_id, &after) {
                Ok(data) => {
                    self.faulted = false;
                    json!({"success":true,"data":data,"componentId":component_id(&plan.document_id,target.unwrap()),"stages":stages,"readbackEvidence":readback_evidence,"nativeExecutionStarted":invoked,"automaticReplayAllowed":false,"rollbackAttempted":false,"legacyParity":false})
                }
                Err(e) => {
                    json!({"success":false,"errorCode":e,"outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":invoked,"stages":stages,"readbackEvidence":readback_evidence})
                }
            },
            failed => {
                let e = match failed {
                    Ok(Err(e)) => e,
                    _ => "native_assembly_effect_or_readback_panic".into(),
                };
                if !invoked {
                    self.faulted = false;
                    return json!({"success":false,"errorCode":e,"nativeExecutionStarted":false,"stages":stages,"readbackEvidence":readback_evidence,"sdkDiagnostic":diagnostic(b)});
                }
                json!({"success":false,"errorCode":e,"outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":true,"stages":stages,"readbackEvidence":readback_evidence,"observedComponentId":target.map(|c|component_id(&plan.document_id,c)),"sdkDiagnostic":diagnostic(b),"automaticReplayAllowed":false,"rollbackAttempted":false})
            }
        }
    }
    pub fn close(&mut self, mut release: impl FnMut(H) -> Result<()>) -> Result<()> {
        if self.owner != thread::current().id() {
            return Err("native_assembly_wrong_thread".into());
        }
        if self.release_uncertain {
            return Err("native_assembly_release_uncertain".into());
        }
        if self.closed {
            return self.ensure_released();
        }
        self.closed = true;
        for e in &mut self.entries {
            e.retired = true;
            if let Some(h) = e.handle.take() {
                self.release_uncertain = true;
                release(h)?;
                self.release_uncertain = false;
            }
        }
        self.ensure_released()
    }
    pub fn ensure_released(&self) -> Result<()> {
        if self.release_uncertain || self.entries.iter().any(|e| e.handle.is_some()) {
            Err("native_assembly_children_not_released".into())
        } else {
            Ok(())
        }
    }
}
fn resolve_component(doc: &str, id: &str, p: &Projection) -> Result<usize> {
    (0..p.components.len())
        .find(|&i| component_id(doc, i) == id)
        .ok_or_else(|| "native_assembly_component_not_owned".into())
}
fn verify_delta(plan: &Plan, after: &Projection, target: usize) -> Result<()> {
    let before = &plan.before;
    let create = matches!(plan.action, Action::Create { .. });
    let added = matches!(plan.action, Action::Create { .. } | Action::Insert { .. });
    if after.components.len() != before.components.len() + usize::from(create)
        || after.occurrences.len() != before.occurrences.len() + usize::from(added)
    {
        return Err("native_assembly_membership_delta".into());
    }
    let mut r = after.root.clone();
    r.revision = before.root.revision.clone();
    r.components = before.root.components;
    r.occurrences = before.root.occurrences;
    if added {
        if !(before.root.timeline..=before.root.timeline + 1).contains(&r.timeline) {
            return Err("native_assembly_timeline_delta".into());
        }
        r.timeline = before.root.timeline;
        r.timeline_position = before.root.timeline_position;
    }
    if r != before.root {
        return Err("native_assembly_unrelated_root_changed".into());
    }
    for (i, c) in before.components.iter().enumerate() {
        let mut a = after.components[i].clone();
        if i == target {
            a.revision = c.revision.clone();
            for change in plan.action.changes() {
                match change.key.as_str() {
                    "name" => a.name = c.name.clone(),
                    "partNumber" => a.part_number = c.part_number.clone(),
                    "description" => a.description = c.description.clone(),
                    key => {
                        if let Some(v) = c.attributes.get(key) {
                            a.attributes.insert(key.into(), v.clone());
                        } else {
                            a.attributes.remove(key);
                        }
                    }
                }
            }
        }
        if a != *c {
            return Err("native_assembly_unrelated_definition_changed".into());
        }
    }
    for (i, o) in before.occurrences.iter().enumerate() {
        let mut a = after.occurrences[i].clone();
        if o.component == target && plan.action.changes().iter().any(|c| c.key == "name") {
            a.name = o.name.clone();
            a.path = o.path.clone();
        }
        if a != *o {
            return Err("native_assembly_unrelated_occurrence_changed".into());
        }
    }
    if added && after.occurrences.last().unwrap().component != target {
        return Err("native_assembly_new_occurrence_definition_mismatch".into());
    }
    let c = &after.components[target];
    for change in plan.action.changes() {
        let expected = if change.key == "partNumber" && change.value.is_empty() {
            &c.name
        } else {
            &change.value
        };
        let actual = match change.key.as_str() {
            "name" => Some(&c.name),
            "partNumber" => c.part_number.as_ref(),
            "description" => c.description.as_ref(),
            key => c.attributes.get(key),
        };
        if actual != Some(expected) {
            return Err("native_assembly_metadata_readback_mismatch".into());
        }
    }
    Ok(())
}
fn prepare_stages(action: &Action) -> Result<Vec<Stage>> {
    let add = matches!(action, Action::Create { .. } | Action::Insert { .. });
    let count = action.changes().len() + usize::from(add);
    if count > 9 {
        return Err("native_assembly_observation_capacity".into());
    }
    let mut stages = Vec::new();
    stages
        .try_reserve_exact(count)
        .map_err(|_| "native_assembly_observation_capacity")?;
    if add {
        stages.push(Stage {
            stage: "occurrenceAdd",
            key: None,
            attempt: Attempt::default(),
        });
    }
    for c in action.changes() {
        stages.push(Stage {
            stage: "componentMetadata",
            key: Some(c.key.clone()),
            attempt: Attempt::default(),
        });
    }
    Ok(stages)
}
pub fn known_refusal(code: &str) -> bool {
    code == "target_revision_mismatch"
        || matches!(
            code,
            "native_assembly_component_not_owned"
                | "native_assembly_document_retired"
                | "native_assembly_native_metadata_requires_online"
                | "native_assembly_root_scope_unsupported"
                | "native_assembly_owned_empty_leaf_required"
                | "native_assembly_unowned_membership"
                | "native_assembly_identity_placement_required"
                | "native_assembly_identity_orientation_required"
                | "native_placement_position_bound"
                | "native_assembly_occurrence_scope_unsupported"
        )
}
