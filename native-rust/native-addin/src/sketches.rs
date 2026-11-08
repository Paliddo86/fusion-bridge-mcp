//! Exact retained sketches, origin-plane creation, and bounded observed summaries.
//! SDK calls are owner-thread only; plans carry data, never SDK handles. The caller
//! must durably journal execution before consuming a Plan. The geometry submodule
//! adds bounded non-atomic insertion; no rollback, arbitrary reference adoption
//! or full legacy parity is provided.
use fusion_mechanical_core::PreparedRequest;
pub use fusion_sketch_plan_core as geometry_preparation;
pub mod dimensions;
pub mod entities;
pub mod extrusion;
pub mod geometry;
pub mod line_state;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    cell::Cell,
    marker::PhantomData,
    rc::Rc,
    thread::{self, ThreadId},
};
pub const MAX_RETAINED: usize = 64;
pub const MAX_ROOT_SKETCHES: u64 = 512;
pub const ROOT_SCOPE: &str = "sketch-root-projection-v1: exact document/root SDK revision, root sketch count, active/parametric/hybrid/configuration/timeline state. No complete existing-sketch geometry inventory.";
pub const SKETCH_SCOPE: &str = "sketch-projection-v1: retained exact sketch, documented SDK sketch revisionId, native name/origin plane/frame/counts/health/deferred state. Geometry coordinates and constraints are not enumerated; complete:false.";
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootState {
    pub active: bool,
    pub parametric: bool,
    pub hybrid: bool,
    pub configured: bool,
    pub root_revision: String,
    pub sketch_count: u64,
    pub timeline_count: u64,
    pub timeline_position: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchState {
    pub name: String,
    pub sdk_revision: String,
    pub plane: u32,
    pub transform: [f64; 16],
    pub curves: u64,
    pub points: u64,
    pub profiles: u64,
    pub dimensions: u64,
    pub constraints: u64,
    pub health: u32,
    pub deferred: bool,
}
/// The holder is published before any SDK create call. A returned holder owns
/// independent parent references; it must not borrow a document-registry handle.
pub struct CreateAttempt<H> {
    pub handle: Option<H>,
    pub invoked: bool,
    pub acknowledged: bool,
}
pub trait Backend {
    type Handle;
    fn root(&self) -> Result<RootState, String>;
    fn command(&self) -> Result<String, String>;
    fn create(&self, plane: u32) -> CreateAttempt<Self::Handle>;
    /// Revalidates exact document/root parent, validity and current membership.
    fn read(&self, handle: &Self::Handle) -> Result<SketchState, String>;
}
#[derive(Debug)]
pub struct Plan {
    document_id: String,
    revision: String,
    before: RootState,
    plane: u32,
}
impl Plan {
    pub fn document_id(&self) -> &str {
        &self.document_id
    }
    pub fn observed_document(&self) -> Value {
        json!({"documentId":self.document_id,"revision":self.revision,"revisionScope":ROOT_SCOPE})
    }
}
struct Entry<H> {
    id: String,
    document_id: String,
    handle: Option<H>,
    retired: Cell<bool>,
    profile_published: Cell<bool>,
    profile_retired: Cell<bool>,
    geometry_leases: Option<Vec<u64>>,
    entities: std::cell::RefCell<Option<entities::Publication>>,
    entities_retired: Cell<bool>,
    dimension_endpoints_published: Cell<bool>,
}
/// No implicit SDK cleanup. Explicit close consumes every lease once; failed or
/// abandoned release prevents successful parent/context teardown. Lifetime slots
/// are never reused, including failed creates. The book is owner-thread bound.
pub struct Book<H> {
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
            entries: Vec::new(),
            owner: thread::current().id(),
            closed: false,
            faulted: false,
            release_uncertain: false,
            _thread: PhantomData,
        }
    }
}
fn err(code: &str) -> Value {
    json!({"success":false,"errorCode":code,"nativeExecutionStarted":false})
}
fn unknown(code: &str, id: &str) -> Value {
    json!({"success":false,"errorCode":code,"outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":true,"sketchId":id})
}
fn digest(scope: &str, v: &impl Serialize) -> Result<String, String> {
    let bytes = serde_json::to_vec(v).map_err(|_| "native_sketch_serialization_failed")?;
    if bytes.len() > 32768 {
        return Err("native_sketch_projection_bound".into());
    }
    Ok(format!(
        "{scope}:{}",
        fusion_mechanical_journal::request_digest(&bytes)
    ))
}
fn root_snapshot<B: Backend>(backend: &B) -> Result<RootState, String> {
    let a = backend.root()?;
    let b = backend.root()?;
    if a != b {
        return Err("native_sketch_projection_changed".into());
    }
    if a.root_revision.is_empty()
        || a.root_revision.len() > 4096
        || a.sketch_count > MAX_ROOT_SKETCHES
        || a.timeline_count > 4096
        || a.timeline_position > a.timeline_count
    {
        return Err("native_sketch_projection_bound".into());
    }
    Ok(a)
}
fn sketch_snapshot<B: Backend>(backend: &B, handle: &B::Handle) -> Result<SketchState, String> {
    let a = backend.read(handle)?;
    let b = backend.read(handle)?;
    if a != b {
        return Err("native_sketch_projection_changed".into());
    }
    if a.name.len() > 4096
        || a.sdk_revision.is_empty()
        || a.sdk_revision.len() > 4096
        || !a.transform.iter().all(|x| x.is_finite())
        || a.health > 5
        || a.plane > 3
        || [a.curves, a.points, a.profiles, a.dimensions, a.constraints]
            .into_iter()
            .any(|n| n > 16384)
    {
        return Err("native_sketch_projection_bound".into());
    }
    Ok(a)
}
fn sketch_data(document_id: &str, id: &str, state: &SketchState) -> Result<Value, String> {
    let revision = digest("sketch-projection-v1", &(id, document_id, state))?;
    Ok(
        json!({"target":{"documentId":document_id,"revision":revision},"sketchId":id,"revisionScope":SKETCH_SCOPE,"complete":false,"geometryEnumerated":false,"sketch":state}),
    )
}
pub fn supported_arguments(request: &PreparedRequest) -> Result<(), &'static str> {
    if !matches!(
        request.operation(),
        "sketch_create" | "native_sketch_inspect"
    ) {
        return Ok(());
    }
    let a = request.arguments();
    let target = a["target"]
        .as_object()
        .ok_or("native_sketch_exact_target_required")?;
    if target
        .keys()
        .any(|k| !matches!(k.as_str(), "documentId" | "revision"))
        || target
            .get("documentId")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err("native_sketch_exact_target_required");
    }
    if request.operation() == "sketch_create" {
        if a.get("component").is_some() || a.get("name").is_some() {
            return Err("native_sketch_root_default_name_only");
        }
        if a.get("plane")
            .is_some_and(|v| !matches!(v.as_str(), Some("xy" | "xz" | "yz")))
        {
            return Err("native_sketch_origin_plane_required");
        }
        if !target
            .get("revision")
            .and_then(Value::as_str)
            .is_some_and(|s| s.starts_with("sketch-root-projection-v1:"))
        {
            return Err("native_sketch_root_revision_required");
        }
    } else if let Some(v) = target.get("revision") {
        let expected = if a.get("sketchId").is_some() {
            "sketch-projection-v1:"
        } else {
            "sketch-root-projection-v1:"
        };
        if !v.as_str().is_some_and(|s| s.starts_with(expected)) {
            return Err("native_sketch_revision_required");
        }
    }
    Ok(())
}
impl<H> Book<H> {
    fn ready(&self) -> Result<(), String> {
        if thread::current().id() != self.owner {
            return Err("native_sketch_wrong_thread".into());
        }
        if self.closed || self.faulted || self.release_uncertain {
            return Err("native_sketch_registry_unavailable".into());
        }
        Ok(())
    }
    pub fn preflight<B: Backend<Handle = H>>(
        &self,
        backend: &B,
        request: &PreparedRequest,
    ) -> Result<Plan, String> {
        self.ready()?;
        supported_arguments(request).map_err(str::to_string)?;
        if request.operation() != "sketch_create" {
            return Err("native_sketch_operation_required".into());
        }
        if self.entries.len() >= MAX_RETAINED {
            return Err("native_sketch_capacity".into());
        }
        let before = root_snapshot(backend)?;
        if !before.active
            || !before.parametric
            || !before.hybrid
            || before.configured
            || before.timeline_position != before.timeline_count
            || before.sketch_count >= MAX_ROOT_SKETCHES
            || backend.command()? != "SelectCommand"
        {
            return Err("native_sketch_context_refused".into());
        }
        let revision = digest("sketch-root-projection-v1", &before)?;
        if request.arguments()["target"]["revision"] != revision {
            return Err("target_revision_mismatch".into());
        }
        let plane = match request.arguments()["plane"].as_str().unwrap_or("xy") {
            "xy" => 1,
            "xz" => 2,
            "yz" => 3,
            _ => return Err("native_sketch_origin_plane_required".into()),
        };
        Ok(Plan {
            document_id: request.arguments()["target"]["documentId"]
                .as_str()
                .unwrap()
                .into(),
            revision,
            before,
            plane,
        })
    }
    pub fn inspect<B: Backend<Handle = H>>(
        &self,
        backend: &B,
        document_id: &str,
        sketch_id: Option<&str>,
    ) -> Result<Value, String> {
        self.ready()?;
        if let Some(id) = sketch_id {
            let entry = self
                .entries
                .iter()
                .find(|e| e.id == id && e.document_id == document_id)
                .ok_or("native_sketch_id_unknown")?;
            if entry.retired.get() {
                return Err("native_sketch_id_retired".into());
            }
            let data = sketch_snapshot(
                backend,
                entry
                    .handle
                    .as_ref()
                    .ok_or("native_sketch_creation_unknown")?,
            )
            .map_err(|error| {
                entry.retired.set(true);
                error
            })?;
            sketch_data(document_id, id, &data)
        } else {
            let data = root_snapshot(backend)?;
            let revision = digest("sketch-root-projection-v1", &data)?;
            Ok(
                json!({"target":{"documentId":document_id,"revision":revision},"revisionScope":ROOT_SCOPE,"complete":false,"root":data}),
            )
        }
    }
    pub fn execute<B: Backend<Handle = H>>(&mut self, backend: &B, plan: Plan) -> Value {
        if let Err(code) = self.ready() {
            return err(&code);
        }
        if self.entries.len() >= MAX_RETAINED {
            return err("native_sketch_capacity");
        }
        let current = match root_snapshot(backend) {
            Ok(s) => s,
            Err(code) => return err(&code),
        };
        if current != plan.before {
            return err("target_revision_mismatch");
        }
        if backend.command().ok().as_deref() != Some("SelectCommand") {
            return err("native_sketch_context_refused");
        }
        let id = format!("s:{}:{:016x}", plan.document_id, self.entries.len() + 1);
        // Reserve and poison before crossing the effect boundary. No unwind can
        // make this slot replayable or erase a possibly acquired SDK reference.
        self.entries.push(Entry {
            id: id.clone(),
            document_id: plan.document_id.clone(),
            handle: None,
            retired: Cell::new(false),
            profile_published: Cell::new(false),
            profile_retired: Cell::new(false),
            geometry_leases: None,
            entities: std::cell::RefCell::new(None),
            entities_retired: Cell::new(false),
            dimension_endpoints_published: Cell::new(false),
        });
        self.faulted = true;
        self.release_uncertain = true;
        let attempt = backend.create(plan.plane);
        let entry = self.entries.last_mut().unwrap();
        entry.handle = attempt.handle;
        self.release_uncertain = entry.handle.is_none() && attempt.invoked;
        if !attempt.invoked && entry.handle.is_none() {
            self.faulted = false;
            return err("native_sketch_create_not_invoked");
        }
        if entry.handle.is_none() {
            self.release_uncertain = true;
            return unknown("native_sketch_create_handle_unknown", &id);
        }
        if !attempt.invoked || !attempt.acknowledged {
            return unknown("native_sketch_create_unconfirmed", &id);
        }
        let observed = match sketch_snapshot(backend, entry.handle.as_ref().unwrap()) {
            Ok(s) => s,
            Err(_) => return unknown("native_sketch_readback_unconfirmed", &id),
        };
        let after = match root_snapshot(backend) {
            Ok(s) => s,
            Err(_) => return unknown("native_sketch_root_readback_unconfirmed", &id),
        };
        if observed.plane != plan.plane
            || observed.curves != 0
            || observed.profiles != 0
            || observed.dimensions != 0
            || observed.constraints != 0
            || observed.deferred
            || observed.health != 0
            || after.sketch_count != plan.before.sketch_count + 1
            || !after.active
            || !after.parametric
            || !after.hybrid
            || after.configured
            || after.timeline_position != after.timeline_count
        {
            return unknown("native_sketch_readback_mismatch", &id);
        }
        if sketch_snapshot(backend, entry.handle.as_ref().unwrap())
            .ok()
            .as_ref()
            != Some(&observed)
            || root_snapshot(backend).ok().as_ref() != Some(&after)
        {
            return unknown("native_sketch_final_projection_changed", &id);
        }
        match sketch_data(&plan.document_id, &id, &observed) {
            Ok(data) => {
                self.faulted = false;
                json!({"success":true,"nativeExecutionStarted":true,"legacyParity":false,"data":data,"rootTarget":{"documentId":plan.document_id,"revision":digest("sketch-root-projection-v1",&after).unwrap()},"created":{"sketchId":id,"plane":plan.plane},"rollbackAttempted":false})
            }
            Err(_) => {
                self.faulted = true;
                unknown("native_sketch_final_readback_unconfirmed", &id)
            }
        }
    }
    /// Called before ANY parent document handle is released. Failures consume
    /// handles once, permanently latch uncertainty, and prohibit teardown retry.
    pub fn close(
        &mut self,
        mut release: impl FnMut(H) -> Result<(), String>,
    ) -> Result<(), String> {
        if thread::current().id() != self.owner {
            return Err("native_sketch_wrong_thread".into());
        }
        if self.release_uncertain {
            return Err("native_sketch_release_unconfirmed".into());
        }
        if self.closed {
            return Ok(());
        }
        self.faulted = true;
        for entry in &mut self.entries {
            if let Some(handle) = entry.handle.take() {
                self.release_uncertain = true;
                release(handle)?;
                self.release_uncertain = false;
            }
        }
        self.closed = true;
        Ok(())
    }
    pub fn ensure_released(&self) -> Result<(), String> {
        if (self.closed || self.entries.is_empty()) && !self.release_uncertain {
            Ok(())
        } else {
            Err("native_sketch_references_retained".into())
        }
    }
}
#[cfg(test)]
pub(crate) mod tests;

pub mod relations;
