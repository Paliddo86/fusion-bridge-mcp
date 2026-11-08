use super::*;
use std::{
    cell::RefCell,
    ffi::{c_char, c_void, OsString},
    mem::ManuallyDrop,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    ptr,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    time::Instant,
};
type Handle = *mut c_void;
type Bytes = extern "C" fn(Handle, u32, *const c_char, usize) -> i32;
#[repr(C)]
#[derive(Default, Serialize)]
struct Facts {
    saved: u32,
    modified: u32,
    active: u32,
    has_data_file: u32,
}
#[repr(C)]
#[derive(Default, Serialize)]
struct Init {
    components: u64,
    occurrences: u64,
}
#[repr(C)]
struct Diagnostic {
    sdk_error: i32,
    stage: u32,
    length: u32,
    truncated: u32,
    text: [u8; 4096],
}
const _: () = assert!(
    std::mem::size_of::<AssemblyFacts>() == 968
        && std::mem::offset_of!(AssemblyFacts, matrices) == 536
        && std::mem::offset_of!(AssemblyFacts, snapshot_count) == 920
);
extern "C" {
    fn eo_thread() -> u32;
    fn eo_open(out: *mut Handle) -> i32;
    fn eo_guard(c: Handle, s: Bytes, u: Handle, jobs: *mut u32) -> i32;
    fn eo_roster(c: Handle, n: *mut u64) -> i32;
    fn eo_document(c: Handle, index: u64, f: *mut Facts, s: Bytes, u: Handle) -> i32;
    fn eo_prepare(c: Handle, index: u64, out: *mut Init) -> i32;
    fn eo_revisions(c: Handle, s: Bytes, u: Handle) -> i32;
    fn eo_assembly(c: Handle, out: *mut AssemblyFacts, s: Bytes, u: Handle) -> i32;
    fn eo_attribute(c: Handle, ci: u64, ai: u64, s: Bytes, u: Handle) -> i32;
    fn eo_capture_end(c: Handle, facts: *const AssemblyFacts) -> i32;
    fn eo_diagnostic(c: Handle, out: *mut Diagnostic) -> i32;
    fn eo_release(c: Handle) -> i32;
}
#[repr(C)]
#[derive(Default)]
struct FileTime {
    low: u32,
    high: u32,
}
#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleExW(flags: u32, name: *const u16, out: *mut Handle) -> i32;
    fn GetModuleFileNameW(module: Handle, out: *mut u16, len: u32) -> u32;
    fn FreeLibrary(module: Handle) -> i32;
    fn GetCurrentProcess() -> Handle;
    fn GetProcessTimes(
        p: Handle,
        a: *mut FileTime,
        b: *mut FileTime,
        c: *mut FileTime,
        d: *mut FileTime,
    ) -> i32;
}
static STARTED: AtomicBool = AtomicBool::new(false);
static ENTERED: AtomicBool = AtomicBool::new(false);
static REENTRANT: AtomicBool = AtomicBool::new(false);
static OWNER: AtomicU32 = AtomicU32::new(0);
thread_local! {static STATE:ManuallyDrop<RefCell<Option<State>>>=const{ManuallyDrop::new(RefCell::new(None))};}
const _: () = assert!(!std::mem::needs_drop::<ManuallyDrop<RefCell<Option<State>>>>());
#[derive(Default, Serialize)]
struct Texts {
    values: Vec<(u32, String)>,
}
extern "C" fn collect_text(u: Handle, field: u32, data: *const c_char, len: usize) -> i32 {
    std::panic::catch_unwind(|| {
        if u.is_null() || field > 23 || len > 4096 || (len > 0 && data.is_null()) {
            return 0;
        }
        let t = unsafe { &mut *u.cast::<Texts>() };
        if t.values.len() >= 24 || t.values.iter().any(|v| v.0 == field) {
            return 0;
        }
        let bytes = if len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len) }
        };
        let Ok(s) = std::str::from_utf8(bytes) else {
            return 0;
        };
        if s.contains('\0') {
            return 0;
        }
        t.values.push((field, s.into()));
        1
    })
    .unwrap_or(0)
}
fn owner() -> Result<()> {
    if unsafe { eo_thread() } != OWNER.load(Ordering::Acquire) || REENTRANT.load(Ordering::Acquire)
    {
        Err("owner_thread_or_reentrancy_guard".into())
    } else {
        Ok(())
    }
}
fn creation() -> Result<String> {
    let (mut a, mut b, mut c, mut d) = (
        FileTime::default(),
        FileTime::default(),
        FileTime::default(),
        FileTime::default(),
    );
    if unsafe { GetProcessTimes(GetCurrentProcess(), &mut a, &mut b, &mut c, &mut d) } == 0 {
        return Err("process_creation_unavailable".into());
    }
    Ok((((a.high as u64) << 32) | a.low as u64).to_string())
}
fn boolean(v: u32) -> Result<bool> {
    match v {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err("invalid_sdk_boolean".into()),
    }
}
struct Native {
    context: Handle,
}
impl Native {
    fn diagnostic(&self, status: i32) -> Value {
        let mut d = Diagnostic {
            sdk_error: -1,
            stage: 0,
            length: 0,
            truncated: 0,
            text: [0; 4096],
        };
        let result = unsafe { eo_diagnostic(self.context, &mut d) };
        let n = (d.length as usize).min(d.text.len());
        json!({"abiStatus":status,"diagnosticStatus":result,"stage":d.stage,"sdkError":if d.sdk_error<0{None}else{Some(d.sdk_error)},"description":String::from_utf8_lossy(&d.text[..n]),"descriptionLossy":std::str::from_utf8(&d.text[..n]).is_err(),"descriptionTruncated":d.truncated!=0||d.length as usize>n,"noResetOrRetry":true})
    }
    fn checked(&self, label: &str, status: i32) -> Result<()> {
        owner()?;
        if status == 0 {
            Ok(())
        } else {
            Err(format!("{label}: {}", self.diagnostic(status)))
        }
    }
    fn guard(&self) -> Result<()> {
        owner()?;
        let mut t = Texts::default();
        let mut jobs = 2;
        self.checked("idle_guard", unsafe {
            eo_guard(
                self.context,
                collect_text,
                (&mut t as *mut Texts).cast(),
                &mut jobs,
            )
        })?;
        owner()?;
        if jobs != 0 || t.values != vec![(0, "SelectCommand".into())] {
            return Err("observed_idle_command_required".into());
        }
        Ok(())
    }
    fn roster(&self) -> Result<Vec<Document>> {
        owner()?;
        let mut n = 0;
        self.checked("roster", unsafe { eo_roster(self.context, &mut n) })?;
        if !(1..=16).contains(&n) {
            return Err("roster_bound".into());
        }
        let mut out = Vec::new();
        for i in 0..n {
            let mut f = Facts::default();
            let mut t = Texts::default();
            self.checked("document_read", unsafe {
                eo_document(
                    self.context,
                    i,
                    &mut f,
                    collect_text,
                    (&mut t as *mut Texts).cast(),
                )
            })?;
            if t.values.len() != 2 || t.values[0].0 != 0 || t.values[1].0 != 1 {
                return Err("document_texts_incomplete".into());
            }
            out.push(Document {
                name: t.values[0].1.clone(),
                creation_id: t.values[1].1.clone(),
                saved: boolean(f.saved)?,
                modified: boolean(f.modified)?,
                active: boolean(f.active)?,
                has_data_file: boolean(f.has_data_file)?,
            });
        }
        Ok(out)
    }
}
impl Backend for Native {
    fn observe(&mut self, kind: Kind) -> Observation {
        let started = Instant::now();
        let mut t = Texts::default();
        let user = (&mut t as *mut Texts).cast();
        let (mut status, data, numbers) = if owner().is_err() {
            (-2, Value::Null, vec![])
        } else {
            match kind {
                Kind::Revisions => {
                    let status = unsafe { eo_revisions(self.context, collect_text, user) };
                    let complete = t.values.len() == 3
                        && t.values.iter().enumerate().all(|(i, v)| v.0 == i as u32);
                    (
                        if status == 0 && !complete { -3 } else { status },
                        json!(t),
                        vec![],
                    )
                }
                Kind::Assembly => {
                    let mut facts = AssemblyFacts::default();
                    let mut status =
                        unsafe { eo_assembly(self.context, &mut facts, collect_text, user) };
                    let mut attributes: [Vec<Attribute>; 2] = [vec![], vec![]];
                    if status == 0
                        && (facts.component_counts.iter().any(|a| a[4] > 128)
                            || t.values.iter().map(|x| x.0).collect::<Vec<_>>()
                                != [0, 1, 2, 3, 4, 5, 6, 7, 8])
                    {
                        status = -3;
                    }
                    if status == 0 {
                        'attributes: for ci in 0..2 {
                            for ai in 0..facts.component_counts[ci][4] {
                                if owner().is_err() {
                                    status = -2;
                                    break 'attributes;
                                }
                                let mut fields = Texts::default();
                                status = unsafe {
                                    eo_attribute(
                                        self.context,
                                        ci as u64,
                                        ai,
                                        collect_text,
                                        (&mut fields as *mut Texts).cast(),
                                    )
                                };
                                if status != 0 {
                                    break 'attributes;
                                }
                                if fields.values.iter().map(|x| x.0).collect::<Vec<_>>()
                                    != [0, 1, 2]
                                {
                                    status = -3;
                                    break 'attributes;
                                }
                                attributes[ci].push(Attribute {
                                    group: fields.values[0].1.clone(),
                                    name: fields.values[1].1.clone(),
                                    value: fields.values[2].1.clone(),
                                });
                            }
                        }
                    }
                    if status == 0 {
                        if owner().is_err() {
                            status = -2;
                        } else {
                            status = unsafe { eo_capture_end(self.context, &facts) };
                        }
                    }
                    let numbers = facts.numbers();
                    (
                        status,
                        json!(Capture {
                            raw: facts,
                            texts: t.values,
                            attributes
                        }),
                        numbers,
                    )
                }
            }
        };
        let mut diagnostic = self.diagnostic(status);
        if let Err(e) = owner() {
            diagnostic["postCallGuardError"] = json!(e);
            status = -2;
        }
        let (float_bits, non_finite) = float_bits(numbers);
        Observation {
            kind,
            abi_status: status,
            data,
            float_bits,
            non_finite,
            diagnostic,
            elapsed_micros: started.elapsed().as_micros(),
        }
    }
}
struct State {
    module: Handle,
    backend: Native,
    authority: Authority,
    directory: PathBuf,
    stop_attempted: bool,
    before: Vec<Document>,
    after: Vec<Document>,
    samples: Vec<Observation>,
    initial: Value,
}
impl State {
    fn new() -> Result<Self> {
        let mut module = ptr::null_mut();
        if unsafe { GetModuleHandleExW(4, run as *const () as *const u16, &mut module) } == 0
            || module.is_null()
        {
            return Err("module_retention_failed_before_sdk".into());
        }
        let mut path = vec![0u16; 32768];
        let n =
            unsafe { GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) } as usize;
        if n == 0 || n >= path.len() {
            return Err("module_path_failed_retained".into());
        }
        let path = PathBuf::from(OsString::from_wide(&path[..n]));
        let parent = path.parent().ok_or("module_parent")?;
        let prepared = (|| {
            let bytes = storage::read_private(&parent.join("observer-authority.json"), 65536)?;
            let authority: Authority = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            authority.validate(std::process::id(), &creation()?)?;
            let directory = parent.join(format!("observer-{}", authority.nonce));
            storage::create_output(&directory)?;
            storage::immutable_receipt(
                &directory.join("attempt.json"),
                &json!({"schemaVersion":1,"authority":authority,"sdkAcquisitionAttempted":false,"cadEffectsAllowed":false,"sequence":SEQUENCE,"retryAllowed":false}),
            )?;
            Ok((authority, directory))
        })();
        let (authority, directory) = match prepared {
            Ok(v) => v,
            Err(e) => {
                let _ = storage::immutable_receipt(
                    &parent.join("startup-failed.json"),
                    &json!({"success":false,"error":e,"sdkAcquisitionAttempted":false,"moduleRetained":true,"retryAllowed":false}),
                );
                return Err(e);
            }
        };
        Ok(Self {
            module,
            backend: Native {
                context: ptr::null_mut(),
            },
            authority,
            directory,
            stop_attempted: false,
            before: vec![],
            after: vec![],
            samples: vec![],
            initial: Value::Null,
        })
    }
    fn run(&mut self) -> Result<()> {
        owner()?;
        let status = unsafe { eo_open(&mut self.backend.context) };
        self.backend.checked("open_public_sdk", status)?;
        self.backend.guard()?;
        self.before = self.backend.roster()?;
        let index = unique_document(&self.before, &self.authority.document_creation_id)?;
        let mut init = Init::default();
        self.backend.checked("prepare_retained_document", unsafe {
            eo_prepare(self.backend.context, index as u64, &mut init)
        })?;
        self.initial = json!(init);
        if init.components != 3 || init.occurrences != 3 {
            return Err("exact_two_definition_three_occurrence_assembly_required".into());
        }
        self.backend.guard()?;
        self.samples = collect(&mut self.backend);
        if self.samples.len() != SEQUENCE.len() || self.samples.iter().any(|s| !s.succeeded()) {
            return Err("read_observation_failed_or_incomplete_no_retry".into());
        }
        if !qualified(&self.samples) {
            return Err("finite_assembly_truth_or_stability_check_failed".into());
        }
        self.backend.guard()?;
        self.after = self.backend.roster()?;
        if self.before != self.after {
            return Err("document_roster_metadata_changed".into());
        }
        Ok(())
    }
    fn report(&self, result: &Result<()>) -> Result<()> {
        storage::immutable_receipt(
            &self.directory.join("observation-result.json"),
            &json!({"schemaVersion":1,"purpose":self.authority.purpose,"success":result.is_ok(),"error":result.as_ref().err(),"pid":std::process::id(),"processCreationTime100ns":self.authority.process_creation_time100ns,"threadId":unsafe{eo_thread()},"authority":self.authority,"before":self.before,"after":self.after,"initial":self.initial,"samples":self.samples,"comparisons":comparisons(&self.samples),"observationScope":"exact retained public document/root/two definitions/three root occurrences; two local projections bracketed by three root and definition revision captures; typed shared component identity, complete finite membership, metadata, two identity sibling matrices, final translated third-occurrence matrix and two healthy captured-position timeline rows","sdkReferencesReleased":false,"effectsAttempted":false,"journalReconciled":false,"historicalFailureCauseProven":false,"limitation":"Finite current-state SDK qualification only; no operation journal reconciliation or historical failure cause proof. No product private handles, model mutation, import/export, activation or close; no cloud PN/description/material getters. Empty assembly graph/metadata/shared identities only, not solid-body or BOM traversal qualification. Matrix expected-pose tolerance 1e-10; raw bits retained. First intermediate pose and semantic archive/reopen persistence are not qualified."}),
        )
    }
}
struct Entry;
impl Drop for Entry {
    fn drop(&mut self) {
        ENTERED.store(false, Ordering::Release);
    }
}
fn enter() -> Result<Entry> {
    if ENTERED.swap(true, Ordering::AcqRel) {
        REENTRANT.store(true, Ordering::Release);
        Err("nested_entry_refused".into())
    } else {
        Ok(Entry)
    }
}
#[no_mangle]
pub extern "C" fn run(_: *const c_char) -> bool {
    std::panic::catch_unwind(|| {
        let Ok(_entry) = enter() else { return false };
        if STARTED.swap(true, Ordering::AcqRel) {
            return false;
        }
        OWNER.store(unsafe { eo_thread() }, Ordering::Release);
        STATE.with(|slot| {
            let mut slot = slot.borrow_mut();
            let state = match State::new() {
                Ok(s) => s,
                Err(_) => return false,
            };
            *slot = Some(state);
            let s = slot.as_mut().unwrap();
            let result = s.run();
            let report = s.report(&result);
            result.is_ok() && report.is_ok()
        })
    })
    .unwrap_or_else(|_| {
        REENTRANT.store(true, Ordering::Release);
        false
    })
}
#[no_mangle]
pub extern "C" fn stop(_: *const c_char) -> bool {
    std::panic::catch_unwind(||{let Ok(_entry)=enter()else{return false};if owner().is_err(){return false;}STATE.with(|slot|{let mut slot=slot.borrow_mut();let Some(s)=slot.as_mut()else{return false};if s.stop_attempted{return false;}s.stop_attempted=true;let result=(||{storage::immutable_receipt(&s.directory.join("release-attempt.json"),&json!({"onlyObserverReferences":true,"noModelClose":true,"retryAllowed":false}))?;if !s.backend.context.is_null(){let status=unsafe{eo_release(s.backend.context)};owner()?;if status!=0{return Err(format!("observer_release_uncertain_status_{status}"));}s.backend.context=ptr::null_mut();}storage::immutable_receipt(&s.directory.join("release-result.json"),&json!({"success":true,"observerReferencesReleased":true,"productAndFixtureReferencesUntouched":true,"documentCloseAttempted":false}))?;if unsafe{FreeLibrary(s.module)}==0{return Err("module_reference_release_uncertain".into());}s.module=ptr::null_mut();Ok(())})();result.is_ok()})}).unwrap_or_else(|_|{REENTRANT.store(true,Ordering::Release);false})
}
