//! Exact-object document lifecycle. Journal permission is supplied separately by
//! the coordinator; these owner-thread methods never access storage or retry SDK effects.
use super::*;
use crate::mechanics::Inspection;
use fusion_mechanical_core::PreparedRequest;
use serde::Serialize;
use serde_json::{json, Value};

pub const MAX_OWNED_SCRATCH: usize = 8;
pub const CONTRACT: &str = "native-lifecycle-v1";

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScratchProfile {
    pub parametric: u32,
    pub hybrid: u32,
    pub components: u64,
    pub bodies: u64,
    pub sketches: u64,
    pub occurrences: u64,
    pub timeline: u64,
    pub graphics: u64,
    pub mesh_bodies: u64,
    pub features: u64,
    pub planes: u64,
    pub axes: u64,
    pub points: u64,
    pub parameters: u64,
    pub user_parameters: u64,
}
impl ScratchProfile {
    fn allowed(&self, blank: bool) -> bool {
        self.parametric == 1
            && self.hybrid == 1
            && self.components == 1
            && [
                self.bodies,
                self.sketches,
                self.occurrences,
                self.timeline,
                self.graphics,
                self.mesh_bodies,
                self.features,
                self.planes,
                self.axes,
                self.points,
            ]
            .iter()
            .all(|n| *n == 0)
            && self.parameters == self.user_parameters
            && self.parameters <= 512
            && (!blank || self.parameters == 0)
    }
}
/// A backend publishes an available exact created lease even when acknowledgement
/// or later SDK work failed. It must not unwind after acquisition before returning
/// the lease; handle destructors never perform SDK work.
pub struct CreationAttempt<H> {
    pub handle: Option<H>,
    pub invoked: bool,
    pub acknowledgement: Result<(), String>,
}
pub struct EffectAttempt {
    pub invoked: bool,
    pub acknowledgement: Result<bool, String>,
}
pub trait LifecycleSdk: DocumentSdk {
    fn active_command(&self) -> Result<String, String>;
    fn create_blank(&self) -> CreationAttempt<Self::Handle>;
    fn make_parametric(&self, document: &Self::Handle) -> EffectAttempt;
    fn scratch_profile(&self, document: &Self::Handle) -> Result<ScratchProfile, String>;
    fn has_data_file(&self, document: &Self::Handle) -> Result<bool, String>;
    fn inspect_mechanical(&self, document: &Self::Handle, id: &str) -> Result<Inspection, String>;
    fn discard_document(&self, document: &Self::Handle) -> EffectAttempt;
    fn activate_document(&self, document: &Self::Handle) -> EffectAttempt;
}
#[derive(Debug, Clone)]
pub struct CreationProof {
    pub session: String,
    pub operation_id: String,
    pub request_digest: String,
    pub document_id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OwnedState {
    Ready,
    Closing,
    Closed,
}
#[derive(Debug)]
pub(super) struct OwnedScratch {
    serial: u64,
    creation_operation: String,
    digest: String,
    original: Document,
    state: OwnedState,
}
#[derive(Debug)]
enum Kind {
    Create {
        requested_name: String,
    },
    Discard {
        document_id: String,
        revision: String,
        proof: CreationProof,
    },
    Activate {
        document_id: String,
    },
}
#[derive(Debug)]
pub struct Plan {
    kind: Kind,
    baseline: Vec<Document>,
    session: String,
    operation_id: String,
    digest: String,
}
impl Plan {
    pub fn observed_document(&self) -> Value {
        match &self.kind {
            Kind::Create { .. } => {
                json!({"operationKind":"document_create","targetBefore":null,"baselineDocuments":self.baseline,"scope":CONTRACT})
            }
            Kind::Discard {
                document_id,
                revision,
                ..
            } => {
                json!({"documentId":document_id,"revision":revision,"scope":"owned_parameter_only_scratch_discard"})
            }
            Kind::Activate { document_id } => {
                json!({"documentId":document_id,"scope":"exact_document_activation","expectedActiveDocumentId":self.baseline.iter().find(|d|d.active).and_then(|d|d.document_id.as_ref())})
            }
        }
    }
}
pub fn is_lifecycle(operation: &str) -> bool {
    matches!(
        operation,
        "new_design" | "native_scratch_discard" | "native_document_activate"
    )
}
/// This runs before durable admission and SDK dispatch.
pub fn supported_arguments(request: &PreparedRequest) -> Result<(), String> {
    let a = request.arguments();
    if request.operation() == "new_design" {
        if a.get("box").is_some() || a.get("target").is_some() {
            return Err("native_blank_design_only".into());
        }
        let name = a
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Adom mechanical");
        if name.is_empty() || name.chars().count() > 120 || name.contains('\0') {
            return Err("invalid_requested_name".into());
        }
    }
    Ok(())
}
fn refused(code: &str) -> Value {
    json!({"success":false,"errorCode":code,"nativeExecutionStarted":false})
}
fn unknown(code: &str, id: Option<&str>) -> Value {
    json!({"success":false,"errorCode":code,"outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":true,"observedDocumentId":id})
}
fn same_metadata(a: &Document, b: &Document) -> bool {
    a.document_id == b.document_id
        && a.name == b.name
        && a.creation_id == b.creation_id
        && a.saved == b.saved
        && a.modified == b.modified
}
fn stable_before(before: &[Document], after: &[Document]) -> bool {
    before.len() == after.len()
        && before.iter().all(|a| {
            after
                .iter()
                .any(|b| same_metadata(a, b) && a.active == b.active)
        })
}
fn originals_preserved(
    before: &[Document],
    after: &[Document],
    omit: Option<&str>,
    extra: usize,
) -> bool {
    let expected = before
        .iter()
        .filter(|d| d.document_id.as_deref() != omit)
        .collect::<Vec<_>>();
    after.len() == expected.len() + extra
        && expected
            .iter()
            .all(|a| after.iter().any(|b| same_metadata(a, b)))
}

impl<S: LifecycleSdk> Registry<S> {
    /// Process-local ownership evidence only. No SDK calls, no document close,
    /// and no authority to reconstruct these capabilities after restart.
    pub fn owned_scratch_status(&self) -> Result<Value, String> {
        let state = self.borrow()?;
        Ok(
            json!({"scope":"current_runtime_live_ownership_table","registryFaulted":self.faulted.get(),
            "complete":!self.faulted.get(),"maxLifetimeEntries":MAX_OWNED_SCRATCH,
            "entries":state.owned.iter().map(|entry|json!({"documentId":format!("d:{}:{:016x}",self.epoch,entry.serial),
                "creationOperationId":entry.creation_operation,"state":match entry.state{OwnedState::Ready=>"owned",OwnedState::Closing=>"close_unconfirmed",OwnedState::Closed=>"closed"}})).collect::<Vec<_>>()}),
        )
    }
    fn neutral(&self) -> Result<(), String> {
        self.owner()?;
        if self.sdk.active_command()? != "SelectCommand" {
            return Err("native_lifecycle_command_not_neutral".into());
        }
        if self.faulted.get() {
            return Err("document registry faulted".into());
        }
        Ok(())
    }
    fn lifecycle_entry<'a>(
        &self,
        state: &'a State<S::Handle>,
        id: &str,
    ) -> Result<&'a Entry<S::Handle>, String> {
        state
            .entries
            .iter()
            .find(|e| format!("d:{}:{:016x}", self.epoch, e.serial) == id)
            .ok_or("document identity is not current".into())
    }
    fn discard_guard(
        &self,
        state: &State<S::Handle>,
        id: &str,
        revision: &str,
        proof: &CreationProof,
    ) -> Result<(), String> {
        let entry = self.lifecycle_entry(state, id)?;
        let owned = state
            .owned
            .iter()
            .find(|o| o.serial == entry.serial)
            .ok_or("no live owned scratch capability")?;
        if owned.state != OwnedState::Ready
            || proof.session != self.epoch
            || proof.document_id != id
            || proof.operation_id != owned.creation_operation
            || proof.request_digest != owned.digest
        {
            return Err("owned scratch creation proof mismatch or capability spent".into());
        }
        let current = self.sdk.read(&entry.handle)?;
        if !self.sdk.is_valid(&entry.handle)?
            || !current.active
            || current.saved
            || self.sdk.has_data_file(&entry.handle)?
            || current.name != owned.original.name
            || current.creation_id != owned.original.creation_id
            || !current.name.starts_with("Untitled")
        {
            return Err("owned scratch document state refused".into());
        }
        if !self.sdk.scratch_profile(&entry.handle)?.allowed(false) {
            return Err("owned scratch model profile refused".into());
        }
        if self
            .sdk
            .inspect_mechanical(&entry.handle, id)?
            .target
            .revision
            != revision
        {
            return Err("target_revision_mismatch".into());
        }
        Ok(())
    }
    pub fn lifecycle_preflight(
        &self,
        request: &PreparedRequest,
        proof: Option<CreationProof>,
    ) -> Result<Plan, String> {
        supported_arguments(request)?;
        let snapshot = self.refresh()?;
        let state = self.borrow()?;
        self.neutral()?;
        if snapshot.documents.iter().filter(|d| d.active).count() != 1 {
            return Err("unique_active_document_required".into());
        }
        let a = request.arguments();
        let session = a["sessionId"]
            .as_str()
            .ok_or("session required")?
            .to_owned();
        if session != self.epoch {
            return Err("session_mismatch".into());
        }
        let operation_id = a["operationId"]
            .as_str()
            .ok_or("operation required")?
            .to_owned();
        let kind = match request.operation() {
            "new_design" => {
                if state.entries.len() >= MAX_DOCUMENTS
                    || state.owned.len() >= MAX_OWNED_SCRATCH
                    || state.next == u64::MAX
                {
                    return Err("native_lifecycle_capacity".into());
                }
                Kind::Create {
                    requested_name: a["name"].as_str().unwrap_or("Adom mechanical").to_owned(),
                }
            }
            "native_scratch_discard" => {
                let id = a["target"]["documentId"]
                    .as_str()
                    .ok_or("exact document target required")?
                    .to_owned();
                let revision = a["target"]["revision"]
                    .as_str()
                    .ok_or("scoped revision required")?
                    .to_owned();
                let proof = proof.ok_or("durable creation receipt required")?;
                self.discard_guard(&state, &id, &revision, &proof)?;
                Kind::Discard {
                    document_id: id,
                    revision,
                    proof,
                }
            }
            "native_document_activate" => {
                let id = a["target"]["documentId"]
                    .as_str()
                    .ok_or("exact document target required")?
                    .to_owned();
                let expected = a["expectedActiveDocumentId"]
                    .as_str()
                    .ok_or("expected active identity required")?;
                if snapshot
                    .documents
                    .iter()
                    .find(|d| d.active)
                    .and_then(|d| d.document_id.as_deref())
                    != Some(expected)
                {
                    return Err("active_document_mismatch".into());
                }
                let entry = self.lifecycle_entry(&state, &id)?;
                if !self.sdk.is_valid(&entry.handle)? {
                    return Err("activation target invalid".into());
                }
                Kind::Activate { document_id: id }
            }
            _ => return Err("unsupported native lifecycle operation".into()),
        };
        self.neutral()?;
        Ok(Plan {
            kind,
            baseline: snapshot.documents,
            session,
            operation_id,
            digest: fusion_mechanical_journal::request_digest(request.canonical_bytes()),
        })
    }
    /// Requires one durable execution permit from the coordinator. No plan clone,
    /// no retry, no filesystem work, and no SDK lease leaves this registry.
    pub fn lifecycle_execute(&self, plan: Plan) -> Value {
        let fresh = match self.refresh() {
            Ok(s) => s,
            Err(_) => return refused("lifecycle_identity_unavailable"),
        };
        if plan.session != self.epoch || !stable_before(&plan.baseline, &fresh.documents) {
            return refused("lifecycle_baseline_changed");
        }
        let mut state = match self.borrow() {
            Ok(s) => s,
            Err(_) => return refused("lifecycle_registry_unavailable"),
        };
        if self.neutral().is_err() {
            return refused("native_lifecycle_command_not_neutral");
        }
        match plan.kind {
            Kind::Create { requested_name } => self.create_owned(
                &mut state,
                &plan.baseline,
                &plan.operation_id,
                &plan.digest,
                &requested_name,
            ),
            Kind::Discard {
                document_id,
                revision,
                proof,
            } => self.discard_owned(&mut state, &plan.baseline, &document_id, &revision, &proof),
            Kind::Activate { document_id } => {
                self.activate_exact(&mut state, &plan.baseline, &document_id)
            }
        }
    }
    fn create_owned(
        &self,
        state: &mut State<S::Handle>,
        before: &[Document],
        operation: &str,
        digest: &str,
        name: &str,
    ) -> Value {
        if state.entries.len() >= MAX_DOCUMENTS
            || state.owned.len() >= MAX_OWNED_SCRATCH
            || state.next == u64::MAX
        {
            return refused("native_lifecycle_capacity");
        }
        // Reserve storage before any SDK call can acquire or create an object.
        state.entries.reserve(1);
        state.owned.reserve(1);
        let serial = state.next;
        state.next += 1;
        let id = format!("d:{}:{serial:016x}", self.epoch);
        let mut guard = FaultGuard {
            faulted: &self.faulted,
            armed: true,
        };
        let attempt = self.sdk.create_blank();
        let has_handle = attempt.handle.is_some();
        if let Some(handle) = attempt.handle {
            state.entries.push(Entry { handle, serial });
        }
        if !attempt.invoked {
            if has_handle {
                return unknown("create_lease_without_invocation", Some(&id));
            }
            guard.armed = false;
            return refused("document_create_not_invoked");
        }
        if attempt.acknowledgement.is_err() || !has_handle {
            return unknown(
                "document_create_unconfirmed",
                has_handle.then_some(id.as_str()),
            );
        }
        let observed = (|| -> Result<Value, String> {
            let entry = state.entries.last().unwrap();
            for existing in &state.entries[..state.entries.len() - 1] {
                if self.sdk.same(&existing.handle, &entry.handle)? {
                    return Err("creation returned an existing document".into());
                }
            }
            if !self.sdk.is_valid(&entry.handle)? {
                return Err("created document invalid".into());
            }
            let mut document = self.sdk.read(&entry.handle)?;
            document.document_id = Some(id.clone());
            if !document.active
                || document.saved
                || self.sdk.has_data_file(&entry.handle)?
                || !document.name.starts_with("Untitled")
            {
                return Err("created document state refused".into());
            }
            self.neutral()?;
            let changed = self.sdk.make_parametric(&entry.handle);
            if !changed.invoked || changed.acknowledgement != Ok(true) {
                return Err("parametric setup unconfirmed".into());
            }
            let profile = self.sdk.scratch_profile(&entry.handle)?;
            if !profile.allowed(true) {
                return Err("created design is not a blank parametric hybrid".into());
            }
            let inspection = self.sdk.inspect_mechanical(&entry.handle, &id)?;
            let after = self.refresh_inner(state)?;
            if !originals_preserved(before, &after.documents, None, 1) {
                return Err("original document metadata or membership changed".into());
            }
            let document = after
                .documents
                .into_iter()
                .find(|d| d.document_id.as_deref() == Some(&id))
                .ok_or("created exact reference not adopted")?;
            if !document.active || document.saved {
                return Err("created document state changed".into());
            }
            self.neutral()?;
            state.owned.push(OwnedScratch {
                serial,
                creation_operation: operation.into(),
                digest: digest.into(),
                original: document.clone(),
                state: OwnedState::Ready,
            });
            Ok(
                json!({"success":true,"nativeExecutionStarted":true,"data":{"target":inspection.target,"document":document,"inspection":inspection,
                "requestedName":name,"requestedNameScope":"session_and_operation_receipt_metadata","creationOperationId":operation,
                "ownedScratch":true,"cleanupProfile":"parameter_only_current_session","legacyParity":false}}),
            )
        })();
        match observed {
            Ok(value) => {
                guard.armed = false;
                value
            }
            Err(_) => unknown("created_document_readback_unconfirmed", Some(&id)),
        }
    }
    fn discard_owned(
        &self,
        state: &mut State<S::Handle>,
        before: &[Document],
        id: &str,
        revision: &str,
        proof: &CreationProof,
    ) -> Value {
        if self.discard_guard(state, id, revision, proof).is_err() {
            return refused("owned_scratch_guard_refused");
        }
        if self.neutral().is_err() {
            return refused("native_lifecycle_command_not_neutral");
        }
        let serial = match self.lifecycle_entry(state, id) {
            Ok(e) => e.serial,
            Err(_) => return refused("owned_scratch_not_current"),
        };
        state
            .owned
            .iter_mut()
            .find(|o| o.serial == serial)
            .unwrap()
            .state = OwnedState::Closing;
        let mut guard = FaultGuard {
            faulted: &self.faulted,
            armed: true,
        };
        let observed = (|| -> Result<Value, String> {
            let entry = self.lifecycle_entry(state, id)?;
            let attempt = self.sdk.discard_document(&entry.handle);
            if !attempt.invoked || attempt.acknowledgement != Ok(true) {
                return Err("close unconfirmed".into());
            }
            if self.sdk.is_valid(&entry.handle)? {
                return Err("closed document remains valid".into());
            }
            let after = self.refresh_inner(state)?;
            if after
                .documents
                .iter()
                .any(|d| d.document_id.as_deref() == Some(id))
                || !originals_preserved(before, &after.documents, Some(id), 0)
            {
                return Err("closed roster is not exact".into());
            }
            state
                .owned
                .iter_mut()
                .find(|o| o.serial == serial)
                .unwrap()
                .state = OwnedState::Closed;
            Ok(
                json!({"success":true,"nativeExecutionStarted":true,"data":{"closedDocumentId":id,"closeReturned":true,"createdObjectValid":false,
                "originalDocumentMetadataUnchanged":true,"activeDocumentId":after.documents.iter().find(|d|d.active).and_then(|d|d.document_id.as_ref()),"automaticActivationPerformed":false}}),
            )
        })();
        match observed {
            Ok(value) => {
                guard.armed = false;
                value
            }
            Err(_) => unknown("owned_scratch_close_unconfirmed", Some(id)),
        }
    }
    fn activate_exact(&self, state: &mut State<S::Handle>, before: &[Document], id: &str) -> Value {
        let entry = match self.lifecycle_entry(state, id) {
            Ok(e) => e,
            Err(_) => return refused("activation_target_not_current"),
        };
        let mut guard = FaultGuard {
            faulted: &self.faulted,
            armed: true,
        };
        let attempt = self.sdk.activate_document(&entry.handle);
        if !attempt.invoked || attempt.acknowledgement != Ok(true) {
            return unknown("document_activation_unconfirmed", Some(id));
        }
        match self.refresh_inner(state) {
            Ok(after)
                if originals_preserved(before, &after.documents, None, 0)
                    && after
                        .documents
                        .iter()
                        .find(|d| d.active)
                        .and_then(|d| d.document_id.as_deref())
                        == Some(id)
                    && !self.faulted.get() =>
            {
                guard.armed = false;
                json!({"success":true,"nativeExecutionStarted":true,"data":{"activatedDocumentId":id,"activeIdentityVerified":true}})
            }
            _ => unknown("document_activation_readback_unconfirmed", Some(id)),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
