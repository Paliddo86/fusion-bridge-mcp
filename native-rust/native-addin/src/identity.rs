//! Main-thread document identity. The backend supplies exact SDK object equality;
//! names, creation IDs, collection positions and native addresses are never IDs.
use crate::{assemble_snapshot, Document, Snapshot};
use std::{
    cell::{Cell, RefCell},
    marker::PhantomData,
    mem::ManuallyDrop,
    rc::Rc,
    thread::{self, ThreadId},
};

pub const MAX_DOCUMENTS: usize = 256;
pub mod document_lifecycle;
pub const IDENTITY_SCOPE: &str = "Opaque document IDs identify retained SDK document objects only within this runtime epoch. Resolve revalidates current document membership. IDs are not durable across restart and do not identify content revisions. Names, creation IDs and inventory order are metadata only; mutations require separate supported-capability, revision and durable execution guards.";

/// All methods, including release, run on the registry's creating thread.
/// `retain` returns one owned SDK reference; `release` consumes it exactly once,
/// even on error. Handle destructors must NOT call the SDK. `same` must compare
/// retained references of the same SDK type, not metadata or recycled addresses.
/// A backend must not unwind after acquiring a reference but before returning it.
pub trait DocumentSdk {
    type Handle;
    fn version(&self) -> Result<String, String>;
    fn thread_id(&self) -> u32;
    fn count(&self) -> Result<usize, String>;
    fn retain(&self, index: usize) -> Result<Self::Handle, String>;
    fn read(&self, handle: &Self::Handle) -> Result<Document, String>;
    fn same(&self, left: &Self::Handle, right: &Self::Handle) -> Result<bool, String>;
    fn is_valid(&self, handle: &Self::Handle) -> Result<bool, String>;
    fn release(&self, handle: Self::Handle) -> Result<(), String>;
    /// Refuse before releasing parent document references while an adapter
    /// child lease may still borrow them. No cleanup retry is implied.
    fn prepare_release(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Read-only selector errors. Only proven selector refusals are known; SDK,
/// registry and closure failures retain the existing unknown-read path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadTargetError {
    Malformed,
    ForeignEpoch,
    Retired,
    NotRegistered,
    Unavailable(String),
}
impl ReadTargetError {
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Malformed => Some("native_document_target_malformed"),
            Self::ForeignEpoch => Some("native_document_target_foreign_epoch"),
            Self::Retired => Some("native_document_target_retired"),
            Self::NotRegistered => Some("native_document_target_not_registered"),
            Self::Unavailable(_) => None,
        }
    }
}

struct Entry<H> {
    handle: H,
    serial: u64,
}
struct State<H> {
    entries: Vec<Entry<H>>,
    scratch: Vec<H>,
    next: u64,
    release_uncertain: bool,
    closed: bool,
    owned: Vec<document_lifecycle::OwnedScratch>,
}

/// Owns the SDK backend and retained references. Supply a fresh 128-bit runtime
/// epoch (32 lower-case hexadecimal characters); never reuse an epoch.
/// Any incomplete/ambiguous/faulted refresh permanently faults this registry.
/// Explicit `close` is mandatory: abandoned or uncertain state deliberately
/// retains references/backend rather than invoking SDK cleanup during unwinding
/// or TLS destruction. No SDK handle is exposed by snapshot or resolve.
///
/// ```compile_fail
/// use AdomFusionRustNative::identity::{Registry, DocumentSdk};
/// fn requires_send<T: Send>() {}
/// fn check<S: DocumentSdk + Send>() where S::Handle: Send {
///     requires_send::<Registry<S>>();
/// }
/// ```
/// ```compile_fail
/// use AdomFusionRustNative::identity::{Registry, DocumentSdk};
/// fn requires_sync<T: Sync>() {}
/// fn check<S: DocumentSdk + Sync>() where S::Handle: Sync {
///     requires_sync::<Registry<S>>();
/// }
/// ```
pub struct Registry<S: DocumentSdk> {
    sdk: ManuallyDrop<S>,
    state: RefCell<State<S::Handle>>,
    epoch: String,
    owner: ThreadId,
    faulted: Cell<bool>,
    _thread: PhantomData<Rc<()>>,
}
struct FaultGuard<'a> {
    faulted: &'a Cell<bool>,
    armed: bool,
}
impl Drop for FaultGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.faulted.set(true);
        }
    }
}

impl<S: DocumentSdk> Registry<S> {
    /// A fault is permanent. Native dispatch must stop admitting further reads;
    /// already claimed reads still publish their actual error before shutdown.
    pub fn is_faulted(&self) -> bool {
        self.faulted.get()
    }
    pub fn new(epoch: String, sdk: S) -> Result<Self, String> {
        if epoch.len() != 32
            || !epoch
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("invalid document registry epoch".into());
        }
        Ok(Self {
            sdk: ManuallyDrop::new(sdk),
            state: RefCell::new(State {
                entries: Vec::new(),
                scratch: Vec::new(),
                next: 1,
                release_uncertain: false,
                closed: false,
                owned: Vec::new(),
            }),
            epoch,
            owner: thread::current().id(),
            faulted: Cell::new(false),
            _thread: PhantomData,
        })
    }
    fn owner(&self) -> Result<(), String> {
        if thread::current().id() != self.owner {
            return Err("document registry called from wrong thread".into());
        }
        Ok(())
    }
    fn borrow(&self) -> Result<std::cell::RefMut<'_, State<S::Handle>>, String> {
        self.owner()?;
        self.state.try_borrow_mut().map_err(|_| {
            self.faulted.set(true);
            "document registry reentrancy refused".into()
        })
    }
    pub fn refresh(&self) -> Result<Snapshot, String> {
        let mut state = self.borrow()?;
        if state.closed || self.faulted.get() {
            return Err("document registry closed or faulted".into());
        }
        let mut guard = FaultGuard {
            faulted: &self.faulted,
            armed: true,
        };
        let snapshot = self.refresh_inner(&mut state)?;
        if self.faulted.get() {
            return Err("document registry faulted during SDK operation".into());
        }
        guard.armed = false;
        Ok(snapshot)
    }
    fn refresh_inner(&self, state: &mut State<S::Handle>) -> Result<Snapshot, String> {
        let count = self.sdk.count()?;
        if count > MAX_DOCUMENTS {
            return Err("document enumeration exceeds bounded registry limit".into());
        }
        // Capacity is at most 256 old + 256 candidate + 1 verification handles.
        state.scratch.reserve(count + 1);
        let mut documents: Vec<Document> = Vec::with_capacity(count);
        let mut bytes = 0usize;
        let mut creation_ids_unique = true;
        for index in 0..count {
            state.scratch.push(self.sdk.retain(index)?);
            let handle = &state.scratch[index];
            if !self.sdk.is_valid(handle)? {
                return Err("invalid candidate document reference".into());
            }
            for prior in &state.scratch[..index] {
                if self.sdk.same(prior, handle)? {
                    return Err("duplicate SDK document in enumeration".into());
                }
            }
            let mut document = self.sdk.read(handle)?;
            document.document_id = None; // Only Rust registry policy assigns IDs.
            bytes = bytes
                .checked_add(document.name.len())
                .and_then(|n| n.checked_add(document.creation_id.len()))
                .ok_or("document snapshot size overflow")?;
            if bytes > 1024 * 1024 {
                return Err("document snapshot byte limit exceeded".into());
            }
            if document.creation_id.is_empty() {
                return Err("missing document creation ID in probe".into());
            }
            if documents
                .iter()
                .any(|d| d.creation_id == document.creation_id)
            {
                creation_ids_unique = false;
            }
            documents.push(document);
        }
        if self.sdk.count()? != count {
            return Err("document count changed while reading".into());
        }
        // Re-read object membership, not just count: a same-count replacement or
        // reordered/incomplete SDK enumeration must not publish a mixed view.
        for index in 0..count {
            state.scratch.push(self.sdk.retain(index)?);
            let verified = state.scratch.last().unwrap();
            if !self.sdk.is_valid(verified)? || !self.sdk.same(&state.scratch[index], verified)? {
                return Err("document membership changed while reading".into());
            }
            let verified = state.scratch.pop().unwrap();
            self.release(state, verified)?;
        }
        if self.sdk.count()? != count {
            return Err("document count changed while verifying".into());
        }
        let version = self.sdk.version()?;
        let mut old_valid = Vec::with_capacity(state.entries.len());
        for entry in &state.entries {
            old_valid.push(self.sdk.is_valid(&entry.handle)?);
        }
        let mut serials = Vec::with_capacity(count);
        let mut next = state.next;
        for (index, handle) in state.scratch.iter().enumerate() {
            if !self.sdk.is_valid(handle)? {
                return Err("document invalidated before publication".into());
            }
            let mut matched = None;
            for (old_index, entry) in state.entries.iter().enumerate() {
                if old_valid[old_index] && self.sdk.same(&entry.handle, handle)? {
                    if matched.replace(entry.serial).is_some() {
                        return Err("ambiguous retained SDK identity".into());
                    }
                }
            }
            let serial = match matched {
                Some(serial) => serial,
                None => {
                    let serial = next;
                    next = next
                        .checked_add(1)
                        .ok_or("document ID sequence exhausted")?;
                    serial
                }
            };
            if serials.contains(&serial) {
                return Err("ambiguous document identity assignment".into());
            }
            serials.push(serial);
            documents[index].document_id = Some(format!("d:{}:{serial:016x}", self.epoch));
        }
        let snapshot = assemble_snapshot(
            documents,
            version,
            self.sdk.thread_id(),
            creation_ids_unique,
            IDENTITY_SCOPE,
        )?;
        if self.faulted.get() {
            return Err("document registry faulted during SDK operation".into());
        }
        // Releases are checked before publication. A failure consumes that lease,
        // faults the registry and prevents any partial IDs from being returned.
        while let Some(entry) = state.entries.pop() {
            self.release(state, entry.handle)?;
        }
        for handle in &state.scratch {
            if !self.sdk.is_valid(handle)? {
                return Err("document invalidated at publication".into());
            }
        }
        if self.faulted.get() {
            return Err("document registry faulted during SDK release".into());
        }
        state.next = next;
        state.entries = state
            .scratch
            .drain(..)
            .zip(serials)
            .map(|(handle, serial)| Entry { handle, serial })
            .collect();
        Ok(snapshot)
    }
    fn release(&self, state: &mut State<S::Handle>, handle: S::Handle) -> Result<(), String> {
        let previous = state.release_uncertain;
        state.release_uncertain = true; // Panic/uncertain release is never retried.
        self.sdk.release(handle)?;
        state.release_uncertain = previous;
        Ok(())
    }
    /// Returns owned metadata only after a complete fresh membership validation.
    /// A live but no-longer-member SDK reference cannot satisfy this read.
    pub fn resolve(&self, id: &str) -> Result<Document, String> {
        let prefix = format!("d:{}:", self.epoch);
        let serial = id
            .strip_prefix(&prefix)
            .ok_or("document ID belongs to another epoch or is malformed")?;
        if serial.len() != 16
            || !serial
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || serial == "0000000000000000"
        {
            return Err("malformed document ID".into());
        }
        self.refresh()?
            .documents
            .into_iter()
            .find(|document| document.document_id.as_deref() == Some(id))
            .ok_or_else(|| "document ID is retired or not a current member".into())
    }
    /// Execute a trusted owner-thread closure against the exact current member.
    /// The registry remains exclusively borrowed throughout the closure so a
    /// nested refresh/resolve/close faults rather than replacing its lease. Only
    /// owned Send results may leave; borrowed SDK references cannot escape.
    /// No native work is authorized by this method alone: callers still enforce
    /// revision guards and durable operation admission/execution claims.
    pub fn with_document<R: Send + 'static>(
        &self,
        id: &str,
        operation: impl for<'a> FnOnce(&'a S, &'a S::Handle, &'a Document) -> Result<R, String>,
    ) -> Result<R, String> {
        let document = self.resolve(id)?;
        self.with_resolved_document(id, document, operation)
    }
    fn with_resolved_document<R: Send + 'static>(
        &self,
        id: &str,
        document: Document,
        operation: impl for<'a> FnOnce(&'a S, &'a S::Handle, &'a Document) -> Result<R, String>,
    ) -> Result<R, String> {
        let state = self.borrow()?;
        if state.closed || self.faulted.get() {
            return Err("document registry closed or faulted".into());
        }
        let mut guard = FaultGuard {
            faulted: &self.faulted,
            armed: true,
        };
        let serial = u64::from_str_radix(id.rsplit(':').next().ok_or("malformed document ID")?, 16)
            .map_err(|_| "malformed document ID")?;
        let entry = state
            .entries
            .iter()
            .find(|entry| entry.serial == serial)
            .ok_or("document ID retired before scoped operation")?;
        if !self.sdk.is_valid(&entry.handle)? {
            return Err("document invalidated before scoped operation".into());
        }
        let result = operation(&self.sdk, &entry.handle, &document);
        if self.faulted.get() || !self.sdk.is_valid(&entry.handle)? {
            return Err("document invalidated or registry faulted during scoped operation".into());
        }
        guard.armed = false;
        result
    }
    /// Read-only selector admission. This performs no SDK calls and never
    /// authorizes effects. Published but absent serials cannot be reused.
    pub fn preflight_read_target(&self, id: &str) -> Result<(), ReadTargetError> {
        let mut pieces = id.split(':');
        let tag = pieces.next();
        let epoch = pieces.next().ok_or(ReadTargetError::Malformed)?;
        let serial = pieces.next().ok_or(ReadTargetError::Malformed)?;
        if tag != Some("d")
            || pieces.next().is_some()
            || epoch.len() != 32
            || !epoch
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || serial.len() != 16
            || !serial
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || serial == "0000000000000000"
        {
            return Err(ReadTargetError::Malformed);
        }
        if epoch != self.epoch {
            return Err(ReadTargetError::ForeignEpoch);
        }
        let serial = u64::from_str_radix(serial, 16).map_err(|_| ReadTargetError::Malformed)?;
        let state = self.borrow().map_err(ReadTargetError::Unavailable)?;
        if state.closed || self.faulted.get() {
            return Err(ReadTargetError::Unavailable(
                "document registry closed or faulted".into(),
            ));
        }
        if !state.entries.iter().any(|e| e.serial == serial) {
            return Err(if serial < state.next {
                ReadTargetError::Retired
            } else {
                ReadTargetError::NotRegistered
            });
        }
        Ok(())
    }
    /// Exact fresh-membership read. Malformed/foreign/cached-retired selectors
    /// never reach SDK work; retirement observed by a successful refresh is also
    /// a known refusal. Incomplete refresh/SDK failure is never called retired.
    pub fn with_read_document<R: Send + 'static>(
        &self,
        id: &str,
        operation: impl for<'a> FnOnce(&'a S, &'a S::Handle, &'a Document) -> Result<R, String>,
    ) -> Result<R, ReadTargetError> {
        self.preflight_read_target(id)?;
        let snapshot = self.refresh().map_err(ReadTargetError::Unavailable)?;
        let document = snapshot
            .documents
            .into_iter()
            .find(|d| d.document_id.as_deref() == Some(id))
            .ok_or(ReadTargetError::Retired)?;
        self.with_resolved_document(id, document, operation)
            .map_err(ReadTargetError::Unavailable)
    }
    /// Releases all leases once, continuing after a reported release error.
    /// Any uncertain release permanently prevents a successful cleanup receipt.
    pub fn close(&self) -> Result<(), String> {
        let mut state = self.borrow()?;
        if state.closed {
            return Ok(());
        }
        self.faulted.set(true);
        self.sdk.prepare_release()?;
        while let Some(handle) = state.scratch.pop() {
            let _ = self.release(&mut state, handle);
        }
        while let Some(entry) = state.entries.pop() {
            let _ = self.release(&mut state, entry.handle);
        }
        if state.release_uncertain {
            return Err("document reference release unconfirmed; retry refused".into());
        }
        state.closed = true;
        Ok(())
    }
    /// Native teardown closes the SDK context only after checked reference release.
    pub fn released_backend(&mut self) -> Result<&mut S, String> {
        self.owner()?;
        if !self.state.get_mut().closed {
            return Err("document references not confirmed released".into());
        }
        Ok(&mut self.sdk)
    }
}
impl<S: DocumentSdk> Drop for Registry<S> {
    fn drop(&mut self) {
        let state = self.state.get_mut();
        if state.closed {
            // SAFETY: explicit close consumed every SDK lease on the owner thread.
            unsafe {
                ManuallyDrop::drop(&mut self.sdk);
            }
        } else {
            for entry in state.entries.drain(..) {
                std::mem::forget(entry.handle);
            }
            for handle in state.scratch.drain(..) {
                std::mem::forget(handle);
            }
        }
    }
}

#[cfg(test)]
mod tests;
