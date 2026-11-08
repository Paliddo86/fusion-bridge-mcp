#[cfg(not(feature = "native-wake-probe"))]
use super::service::{WakeAck, WakeCoalescer};
use super::{
    bytes::{collect, field, Fields},
    execution::{Reply, Task},
    identity::{
        document_lifecycle::{CreationAttempt, EffectAttempt, LifecycleSdk, ScratchProfile},
        DocumentSdk, Registry,
    },
    lifecycle::{CallbackGate, ChildLeaseFence, CloseOnce, StateSlot},
    mechanics::{self, Backend, DesignState, HealthItem, Parameter, ParameterValue},
    operation_status::OperationStatusReader,
    service::{CallbackStage, Queue, Service, ShutdownReason, Stats, Wake},
    Document,
};
use serde_json::{json, Value};
mod assembly_adapter;
mod bom_adapter;
mod material_adapter;
mod physical_adapter;
mod purchased_adapter;
mod sketch_adapter;
#[cfg(not(feature = "native-wake-probe"))]
use std::cell::OnceCell;
use std::{
    ffi::{c_char, c_void},
    marker::PhantomData,
    net::TcpListener,
    path::{Path, PathBuf},
    ptr::NonNull,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

#[repr(C)]
struct RawDocument {
    active: u32,
    saved: u32,
    modified: u32,
}
#[repr(C)]
#[derive(Default)]
struct RawDesignState {
    active: u32,
    parametric: u32,
    component_count: u64,
    body_count: u64,
    sketch_count: u64,
    timeline_count: u64,
    timeline_position: u64,
}
#[repr(C)]
#[derive(Default)]
struct RawParameter {
    value: f64,
    value_type: u32,
    is_user: u32,
    is_deletable: u32,
}
type Sink = unsafe extern "C" fn(*mut c_void, u32, *const c_char, usize) -> i32;
extern "C" {
    fn af_active_command(
        context: *mut c_void,
        sink: unsafe extern "C" fn(*mut c_void, u32, *const c_char, usize) -> i32,
        user: *mut c_void,
    ) -> i32;
    fn af_document_create(context: *mut c_void, out: *mut *mut c_void, invoked: *mut u32) -> i32;
    fn af_document_parametric(
        context: *mut c_void,
        document: *mut c_void,
        invoked: *mut u32,
        ack: *mut u32,
    ) -> i32;
    fn af_document_data_file(context: *mut c_void, document: *mut c_void, out: *mut u32) -> i32;
    fn af_scratch_profile(
        context: *mut c_void,
        document: *mut c_void,
        out: *mut ScratchProfile,
    ) -> i32;
    fn af_document_discard(
        context: *mut c_void,
        document: *mut c_void,
        invoked: *mut u32,
        ack: *mut u32,
    ) -> i32;
    fn af_document_activate(
        context: *mut c_void,
        document: *mut c_void,
        invoked: *mut u32,
        ack: *mut u32,
    ) -> i32;
    fn af_design_state(
        c: *mut c_void,
        d: *mut c_void,
        out: *mut RawDesignState,
        sink: Sink,
        user: *mut c_void,
    ) -> i32;
    fn af_parameter_count(c: *mut c_void, d: *mut c_void, out: *mut u64) -> i32;
    fn af_parameter_read(
        c: *mut c_void,
        d: *mut c_void,
        index: u64,
        out: *mut RawParameter,
        sink: Sink,
        user: *mut c_void,
    ) -> i32;
    fn af_timeline_health(
        c: *mut c_void,
        d: *mut c_void,
        index: u64,
        out: *mut u32,
        sink: Sink,
        user: *mut c_void,
    ) -> i32;
    fn af_parameter_evaluate(
        c: *mut c_void,
        d: *mut c_void,
        expr: *const c_char,
        expr_len: usize,
        unit: *const c_char,
        unit_len: usize,
        out: *mut f64,
    ) -> i32;
    fn af_parameter_create(
        c: *mut c_void,
        d: *mut c_void,
        name: *const c_char,
        name_len: usize,
        expr: *const c_char,
        expr_len: usize,
        unit: *const c_char,
        unit_len: usize,
        comment: *const c_char,
        comment_len: usize,
    ) -> i32;
    fn af_parameter_set(
        c: *mut c_void,
        d: *mut c_void,
        name: *const c_char,
        name_len: usize,
        expr: *const c_char,
        expr_len: usize,
    ) -> i32;
    fn af_design_compute(c: *mut c_void, d: *mut c_void) -> i32;
    fn af_open(out: *mut *mut c_void) -> i32;
    fn af_close(context: *mut c_void) -> i32;
    fn af_version(context: *mut c_void, sink: Sink, user: *mut c_void) -> i32;
    fn af_document_count(context: *mut c_void, out: *mut u64) -> i32;
    fn af_document_retain(context: *mut c_void, index: u64, out: *mut *mut c_void) -> i32;
    fn af_document_same(
        context: *mut c_void,
        left: *mut c_void,
        right: *mut c_void,
        out: *mut u32,
    ) -> i32;
    fn af_document_valid(context: *mut c_void, document: *mut c_void, out: *mut u32) -> i32;
    fn af_document_release(context: *mut c_void, document: *mut c_void) -> i32;
    fn af_document_ref_read(
        context: *mut c_void,
        document: *mut c_void,
        out: *mut RawDocument,
        sink: Sink,
        user: *mut c_void,
    ) -> i32;
    fn af_thread_id() -> u32;
    fn af_event_open(
        id: *const c_char,
        len: usize,
        callback: unsafe extern "C" fn(*mut c_void),
        user: *mut c_void,
        out: *mut *mut c_void,
    ) -> i32;
    fn af_event_fire(event: *mut c_void) -> i32;
    fn af_event_close(event: *mut c_void) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleExW(flags: u32, address: *const u16, module: *mut *mut c_void) -> i32;
    fn GetModuleFileNameW(module: *mut c_void, path: *mut u16, len: u32) -> u32;
    fn FreeLibrary(module: *mut c_void) -> i32;
}

struct Context {
    ptr: NonNull<c_void>,
    close: CloseOnce,
    child_leases: ChildLeaseFence,
    sketches: std::cell::RefCell<crate::sketches::Book<sketch_adapter::Handle>>,
    assemblies: std::cell::RefCell<crate::assembly::Book<assembly_adapter::Handle>>,
    extrusion_diagnostic: std::cell::RefCell<serde_json::Value>,
    _thread: PhantomData<Rc<()>>,
}
fn checked(code: i32) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("Fusion SDK adapter status {code}"))
    }
}
impl Context {
    fn open() -> Result<Self, String> {
        let mut ptr = std::ptr::null_mut();
        unsafe { checked(af_open(&mut ptr))? };
        Ok(Self {
            ptr: NonNull::new(ptr).ok_or("null SDK context")?,
            close: CloseOnce::default(),
            child_leases: ChildLeaseFence::default(),
            sketches: std::cell::RefCell::new(crate::sketches::Book::default()),
            assemblies: std::cell::RefCell::new(crate::assembly::Book::default()),
            extrusion_diagnostic: std::cell::RefCell::new(serde_json::Value::Null),
            _thread: PhantomData,
        })
    }
    fn close(&mut self) -> Result<(), String> {
        self.child_leases.ensure_clear()?;
        self.sketches
            .try_borrow()
            .map_err(|_| "sketch registry busy")?
            .ensure_released()?;
        self.assemblies
            .try_borrow()
            .map_err(|_| "assembly registry busy")?
            .ensure_released()?;
        let ptr = self.ptr;
        self.close
            .close(|| unsafe { checked(af_close(ptr.as_ptr())) })
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        // Normal teardown explicitly checks the result before claiming release.
        // CloseOnce also prevents a destructor retry after an uncertain delete.
        let _ = self.close();
    }
}
// No Drop: release is explicit and checked by the registry on its owner thread.
struct DocumentReference {
    ptr: NonNull<c_void>,
    _thread: PhantomData<Rc<()>>,
}
struct NativeMechanics<'a> {
    context: &'a Context,
    document: &'a DocumentReference,
}
impl LifecycleSdk for Context {
    fn active_command(&self) -> Result<String, String> {
        self.close.ensure_open()?;
        let mut fields = Fields::default();
        unsafe {
            checked(af_active_command(
                self.ptr.as_ptr(),
                collect,
                (&mut fields as *mut Fields).cast(),
            ))?;
        }
        field(&mut fields, 0)
    }
    fn create_blank(&self) -> CreationAttempt<DocumentReference> {
        if let Err(error) = self.close.ensure_open() {
            return CreationAttempt {
                handle: None,
                invoked: false,
                acknowledgement: Err(error),
            };
        }
        let mut raw = std::ptr::null_mut();
        let mut invoked = 0;
        let status = unsafe { af_document_create(self.ptr.as_ptr(), &mut raw, &mut invoked) };
        // Capture the owned holder before inspecting acknowledgement status.
        let handle = NonNull::new(raw).map(|ptr| DocumentReference {
            ptr,
            _thread: PhantomData,
        });
        CreationAttempt {
            handle,
            invoked: invoked != 0,
            acknowledgement: checked(status).and_then(|_| flag(invoked).map(|_| ())),
        }
    }
    fn make_parametric(&self, document: &DocumentReference) -> EffectAttempt {
        let mut invoked = 0;
        let mut ack = 0;
        let status = unsafe {
            af_document_parametric(
                self.ptr.as_ptr(),
                document.ptr.as_ptr(),
                &mut invoked,
                &mut ack,
            )
        };
        EffectAttempt {
            invoked: invoked != 0,
            acknowledgement: checked(status).and_then(|_| flag(ack)),
        }
    }
    fn scratch_profile(&self, document: &DocumentReference) -> Result<ScratchProfile, String> {
        let mut value = ScratchProfile::default();
        unsafe {
            checked(af_scratch_profile(
                self.ptr.as_ptr(),
                document.ptr.as_ptr(),
                &mut value,
            ))?;
        }
        Ok(value)
    }
    fn has_data_file(&self, document: &DocumentReference) -> Result<bool, String> {
        let mut value = 0;
        unsafe {
            checked(af_document_data_file(
                self.ptr.as_ptr(),
                document.ptr.as_ptr(),
                &mut value,
            ))?;
        }
        flag(value)
    }
    fn inspect_mechanical(
        &self,
        document: &DocumentReference,
        id: &str,
    ) -> Result<mechanics::Inspection, String> {
        mechanics::inspect(
            &NativeMechanics {
                context: self,
                document,
            },
            id,
        )
        .map_err(|e| format!("{}: {}", e.code, e.message))
    }
    fn discard_document(&self, document: &DocumentReference) -> EffectAttempt {
        let mut invoked = 0;
        let mut ack = 0;
        let status = unsafe {
            af_document_discard(
                self.ptr.as_ptr(),
                document.ptr.as_ptr(),
                &mut invoked,
                &mut ack,
            )
        };
        EffectAttempt {
            invoked: invoked != 0,
            acknowledgement: checked(status).and_then(|_| flag(ack)),
        }
    }
    fn activate_document(&self, document: &DocumentReference) -> EffectAttempt {
        let mut invoked = 0;
        let mut ack = 0;
        let status = unsafe {
            af_document_activate(
                self.ptr.as_ptr(),
                document.ptr.as_ptr(),
                &mut invoked,
                &mut ack,
            )
        };
        EffectAttempt {
            invoked: invoked != 0,
            acknowledgement: checked(status).and_then(|_| flag(ack)),
        }
    }
}
fn flag(raw: u32) -> Result<bool, String> {
    match raw {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err("invalid native Boolean".into()),
    }
}
impl Backend for NativeMechanics<'_> {
    fn design_state(&self) -> Result<DesignState, String> {
        self.context.close.ensure_open()?;
        let mut raw = RawDesignState::default();
        let mut fields = Fields::default();
        unsafe {
            checked(af_design_state(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                &mut raw,
                collect,
                (&mut fields as *mut Fields).cast(),
            ))?;
        }
        Ok(DesignState {
            active: flag(raw.active)?,
            parametric: flag(raw.parametric)?,
            root_name: field(&mut fields, 0)?,
            root_revision: field(&mut fields, 1)?,
            default_length_units: field(&mut fields, 2)?,
            component_count: raw.component_count,
            body_count: raw.body_count,
            sketch_count: raw.sketch_count,
            timeline_count: raw.timeline_count,
            timeline_position: raw.timeline_position,
        })
    }
    fn parameter_count(&self) -> Result<usize, String> {
        let mut count = 0;
        unsafe {
            checked(af_parameter_count(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                &mut count,
            ))?;
        }
        usize::try_from(count).map_err(|_| "parameter count overflow".into())
    }
    fn parameter(&self, index: usize) -> Result<Parameter, String> {
        let mut raw = RawParameter::default();
        let mut fields = Fields::default();
        unsafe {
            checked(af_parameter_read(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                index as u64,
                &mut raw,
                collect,
                (&mut fields as *mut Fields).cast(),
            ))?;
        }
        let text = field(&mut fields, 4)?;
        let value = match raw.value_type {
            0 if text.is_empty() => ParameterValue::Numeric(raw.value),
            1 => ParameterValue::Text(text),
            _ => return Err("invalid native parameter value type".into()),
        };
        Ok(Parameter {
            name: field(&mut fields, 0)?,
            expression: field(&mut fields, 1)?,
            unit: field(&mut fields, 2)?,
            comment: field(&mut fields, 3)?,
            is_user: flag(raw.is_user)?,
            is_deletable: flag(raw.is_deletable)?,
            value,
        })
    }
    fn timeline_health(&self, index: usize) -> Result<HealthItem, String> {
        let mut state = 0;
        let mut fields = Fields::default();
        unsafe {
            checked(af_timeline_health(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                index as u64,
                &mut state,
                collect,
                (&mut fields as *mut Fields).cast(),
            ))?;
        }
        Ok(HealthItem {
            index,
            state,
            message: field(&mut fields, 0)?,
        })
    }
    fn evaluate_expression(&self, expression: &str, unit: &str) -> Result<f64, String> {
        let mut value = 0.;
        unsafe {
            checked(af_parameter_evaluate(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                expression.as_ptr().cast(),
                expression.len(),
                unit.as_ptr().cast(),
                unit.len(),
                &mut value,
            ))?;
        }
        Ok(value)
    }
    fn create_parameter(
        &self,
        name: &str,
        expression: &str,
        unit: &str,
        comment: &str,
    ) -> Result<(), String> {
        unsafe {
            checked(af_parameter_create(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                name.as_ptr().cast(),
                name.len(),
                expression.as_ptr().cast(),
                expression.len(),
                unit.as_ptr().cast(),
                unit.len(),
                comment.as_ptr().cast(),
                comment.len(),
            ))
        }
    }
    fn set_parameter(&self, name: &str, expression: &str) -> Result<(), String> {
        unsafe {
            checked(af_parameter_set(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
                name.as_ptr().cast(),
                name.len(),
                expression.as_ptr().cast(),
                expression.len(),
            ))
        }
    }
    fn compute_all(&self) -> Result<(), String> {
        unsafe {
            checked(af_design_compute(
                self.context.ptr.as_ptr(),
                self.document.ptr.as_ptr(),
            ))
        }
    }
}
impl DocumentSdk for Context {
    fn prepare_release(&self) -> Result<(), String> {
        self.child_leases.ensure_clear()?;
        self.assemblies
            .try_borrow_mut()
            .map_err(|_| "assembly registry busy")?
            .close(|handle| assembly_adapter::release(self, handle))?;
        self.sketches
            .try_borrow_mut()
            .map_err(|_| "sketch registry busy")?
            .close(|handle| sketch_adapter::release(self, handle))
    }
    type Handle = DocumentReference;
    fn version(&self) -> Result<String, String> {
        self.close.ensure_open()?;
        let mut fields = Fields::default();
        unsafe {
            checked(af_version(
                self.ptr.as_ptr(),
                collect,
                (&mut fields as *mut Fields).cast(),
            ))?;
        }
        field(&mut fields, 0)
    }
    fn thread_id(&self) -> u32 {
        unsafe { af_thread_id() }
    }
    fn count(&self) -> Result<usize, String> {
        self.close.ensure_open()?;
        let mut count = 0;
        unsafe {
            checked(af_document_count(self.ptr.as_ptr(), &mut count))?;
        }
        usize::try_from(count).map_err(|_| "native count overflow".into())
    }
    fn retain(&self, index: usize) -> Result<DocumentReference, String> {
        self.close.ensure_open()?;
        let mut ptr = std::ptr::null_mut();
        unsafe {
            checked(af_document_retain(
                self.ptr.as_ptr(),
                index as u64,
                &mut ptr,
            ))?;
        }
        Ok(DocumentReference {
            ptr: NonNull::new(ptr).ok_or("null retained document reference")?,
            _thread: PhantomData,
        })
    }
    fn same(&self, left: &DocumentReference, right: &DocumentReference) -> Result<bool, String> {
        self.close.ensure_open()?;
        let mut value = 0;
        unsafe {
            checked(af_document_same(
                self.ptr.as_ptr(),
                left.ptr.as_ptr(),
                right.ptr.as_ptr(),
                &mut value,
            ))?;
        }
        checked_bool(value)
    }
    fn is_valid(&self, document: &DocumentReference) -> Result<bool, String> {
        self.close.ensure_open()?;
        let mut value = 0;
        unsafe {
            checked(af_document_valid(
                self.ptr.as_ptr(),
                document.ptr.as_ptr(),
                &mut value,
            ))?;
        }
        checked_bool(value)
    }
    fn release(&self, document: DocumentReference) -> Result<(), String> {
        self.close.ensure_open()?;
        unsafe {
            checked(af_document_release(
                self.ptr.as_ptr(),
                document.ptr.as_ptr(),
            ))
        }
    }
    fn read(&self, document: &DocumentReference) -> Result<Document, String> {
        self.close.ensure_open()?;
        let mut raw = RawDocument {
            active: 0,
            saved: 0,
            modified: 0,
        };
        let mut fields = Fields::default();
        unsafe {
            checked(af_document_ref_read(
                self.ptr.as_ptr(),
                document.ptr.as_ptr(),
                &mut raw,
                collect,
                (&mut fields as *mut Fields).cast(),
            ))?;
        }
        if [raw.active, raw.saved, raw.modified].iter().any(|v| *v > 1) {
            return Err("invalid SDK Boolean".into());
        }
        Ok(Document {
            document_id: None,
            name: field(&mut fields, 0)?,
            creation_id: field(&mut fields, 1)?,
            active: raw.active == 1,
            saved: raw.saved == 1,
            modified: raw.modified == 1,
        })
    }
}
fn checked_bool(value: u32) -> Result<bool, String> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err("invalid SDK Boolean".into()),
    }
}

struct Callback {
    gate: CallbackGate,
    #[cfg(not(feature = "native-wake-probe"))]
    wake: OnceCell<Arc<WakeCoalescer>>,
    registry: Registry<Context>,
    queue: Arc<Queue>,
    stats: Arc<Stats>,
}
impl Callback {
    fn perform(&self, task: &mut Task) -> Result<Reply, String> {
        match task {
            Task::AssemblyPreflight(request) => {
                use crate::assembly::Backend;
                let id = request.arguments()["target"]["documentId"]
                    .as_str()
                    .ok_or("missing exact document target")?;
                self.registry.with_document(id,|context,document,_|{
                    context.extrusion_diagnostic.replace(Value::Null);
                    let adapter=assembly_adapter::Adapter{context,document};
                    let mut book=context.assemblies.try_borrow_mut().map_err(|_|"assembly registry busy")?;
                    Ok(match book.preflight(&adapter,request){Ok(plan)=>Reply::AssemblyPlan(plan),Err(code)=>Reply::Value(json!({"success":false,"errorCode":code,"nativeExecutionStarted":false,"sdkDiagnostic":adapter.diagnostic()}))})
                })
            }
            Task::AssemblyExecute(plan) => {
                let plan = plan.take().ok_or("assembly plan already consumed")?;
                let id = plan.document_id().to_owned();
                let observation = std::cell::RefCell::new(None);
                let result = self.registry.with_document(&id, |context, document, _| {
                    context.extrusion_diagnostic.replace(Value::Null);
                    let mut book = context
                        .assemblies
                        .try_borrow_mut()
                        .map_err(|_| "assembly registry busy")?;
                    let value =
                        book.execute(&assembly_adapter::Adapter { context, document }, plan);
                    observation.replace(Some(value.clone()));
                    Ok(Reply::Value(value))
                });
                match result {
                    Ok(reply) => Ok(reply),
                    Err(_) => Ok(Reply::Value(
                        json!({"success":false,"errorCode":"native_document_guard_failed","outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":null,"observedNativeResult":observation.into_inner()}),
                    )),
                }
            }
            Task::DimensionPreflight(request) => {
                use crate::sketches::relations::RelationBackend;
                let id = request.arguments()["target"]["documentId"]
                    .as_str()
                    .ok_or("missing exact document target")?;
                self.registry.with_document(id,|context,document,_|{
                    let adapter=sketch_adapter::Adapter{context,document};
                    let book=context.sketches.try_borrow().map_err(|_|"sketch registry busy")?;
                    Ok(match book.dimension_preflight(&adapter,request){Ok(plan)=>Reply::DimensionPlan(plan),Err(code)=>Reply::Value(json!({"success":false,"errorCode":code,"nativeExecutionStarted":false,"sdkDiagnostic":adapter.relation_diagnostic()}))})
                })
            }
            Task::DimensionExecute(plan) => {
                let plan = plan.take().ok_or("dimension plan already consumed")?;
                let id = plan.document_id().to_owned();
                let observation = std::cell::RefCell::new(None);
                let result = self.registry.with_document(&id, |context, document, _| {
                    let mut book = context
                        .sketches
                        .try_borrow_mut()
                        .map_err(|_| "sketch registry busy")?;
                    let value = book
                        .execute_dimension(&sketch_adapter::Adapter { context, document }, plan);
                    observation.replace(Some(value.clone()));
                    Ok(Reply::Value(value))
                });
                match result {
                    Ok(reply) => Ok(reply),
                    Err(_) => Ok(Reply::Value(
                        json!({"success":false,"errorCode":"native_document_guard_failed","outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":null,"observedNativeResult":observation.into_inner()}),
                    )),
                }
            }
            Task::RelationPreflight(request) => {
                use crate::sketches::relations::RelationBackend;
                let id = request.arguments()["target"]["documentId"]
                    .as_str()
                    .ok_or("missing exact document target")?;
                self.registry.with_document(id,|context,document,_|{
                    let adapter=sketch_adapter::Adapter{context,document};
                    let book=context.sketches.try_borrow().map_err(|_|"sketch registry busy")?;
                    Ok(match book.relation_preflight(&adapter,request){Ok(plan)=>Reply::RelationPlan(plan),Err(code)=>Reply::Value(json!({"success":false,"errorCode":code,"nativeExecutionStarted":false,"sdkDiagnostic":adapter.relation_diagnostic()}))})
                })
            }
            Task::RelationExecute(plan) => {
                let plan = plan.take().ok_or("relation plan already consumed")?;
                let id = plan.document_id().to_owned();
                let observation = std::cell::RefCell::new(None);
                let result = self.registry.with_document(&id, |context, document, _| {
                    let mut book = context
                        .sketches
                        .try_borrow_mut()
                        .map_err(|_| "sketch registry busy")?;
                    let value =
                        book.execute_relation(&sketch_adapter::Adapter { context, document }, plan);
                    observation.replace(Some(value.clone()));
                    Ok(Reply::Value(value))
                });
                match result {
                    Ok(reply) => Ok(reply),
                    Err(_) => Ok(Reply::Value(
                        json!({"success":false,"errorCode":"native_document_guard_failed","outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":null,"observedNativeResult":observation.into_inner()}),
                    )),
                }
            }
            Task::ExtrudePreflight(request) => {
                use crate::sketches::extrusion::ExtrusionBackend;
                let id = request.arguments()["target"]["documentId"]
                    .as_str()
                    .ok_or("missing exact document target")?;
                self.registry.with_document(id,|context,document,_|{
                    let adapter=sketch_adapter::Adapter{context,document};
                    let book=context.sketches.try_borrow().map_err(|_|"sketch registry busy")?;
                    Ok(match book.extrude_preflight(&adapter,request){Ok(plan)=>Reply::ExtrudePlan(plan),Err(code)=>Reply::Value(json!({"success":false,"errorCode":code,"nativeExecutionStarted":false,"sdkDiagnostic":adapter.diagnostic()}))})
                })
            }
            Task::ExtrudeExecute(plan) => {
                let plan = plan.take().ok_or("extrude plan already consumed")?;
                let id = plan.document_id().to_owned();
                let observation = std::cell::RefCell::new(None);
                let result = self.registry.with_document(&id, |context, document, _| {
                    let mut book = context
                        .sketches
                        .try_borrow_mut()
                        .map_err(|_| "sketch registry busy")?;
                    let value =
                        book.execute_extrude(&sketch_adapter::Adapter { context, document }, plan);
                    observation.replace(Some(value.clone()));
                    Ok(Reply::Value(value))
                });
                match result {
                    Ok(reply) => Ok(reply),
                    Err(_) => Ok(Reply::Value(
                        json!({"success":false,"errorCode":"native_document_guard_failed","outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":null,"observedNativeResult":observation.into_inner()}),
                    )),
                }
            }
            Task::GeometryPreflight(request) => {
                let id = request.arguments()["target"]["documentId"]
                    .as_str()
                    .ok_or("missing exact document target")?;
                self.registry.with_document(id, |context, document, _| {
                    let adapter = sketch_adapter::Adapter { context, document };
                    let book = context.sketches.try_borrow().map_err(|_| "sketch registry busy")?;
                    Ok(match book.geometry_preflight(&adapter, request) {
                        Ok(plan) => Reply::GeometryPlan(plan),
                        Err(code) => Reply::Value(json!({"success":false,"errorCode":code,"nativeExecutionStarted":false})),
                    })
                })
            }
            Task::GeometryExecute(plan) => {
                let plan = plan.take().ok_or("geometry plan already consumed")?;
                let id = plan.document_id().to_owned();
                let observation = std::cell::RefCell::new(None);
                let result = self.registry.with_document(&id, |context, document, _| {
                    let mut book = context
                        .sketches
                        .try_borrow_mut()
                        .map_err(|_| "sketch registry busy")?;
                    let value =
                        book.execute_geometry(&sketch_adapter::Adapter { context, document }, plan);
                    observation.replace(Some(value.clone()));
                    Ok(Reply::Value(value))
                });
                match result {
                    Ok(reply) => Ok(reply),
                    Err(_) => Ok(Reply::Value(
                        json!({"success":false,"errorCode":"native_document_guard_failed","outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":null,"observedNativeResult":observation.into_inner()}),
                    )),
                }
            }
            Task::SketchPreflight(request) => {
                let id = request.arguments()["target"]["documentId"]
                    .as_str()
                    .ok_or("missing exact document target")?;
                self.registry.with_document(id,|context,document,_|{
                    let adapter=sketch_adapter::Adapter{context,document};
                    let book=context.sketches.try_borrow().map_err(|_|"sketch registry busy")?;
                    Ok(match book.preflight(&adapter,request){Ok(plan)=>Reply::SketchPlan(plan),Err(code)=>Reply::Value(json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}))})
                })
            }
            Task::SketchExecute(plan) => {
                let plan = plan.take().ok_or("sketch plan already consumed")?;
                let id = plan.document_id().to_owned();
                let observation = std::cell::RefCell::new(None);
                let result = self.registry.with_document(&id, |context, document, _| {
                    let mut book = context
                        .sketches
                        .try_borrow_mut()
                        .map_err(|_| "sketch registry busy")?;
                    let value = book.execute(&sketch_adapter::Adapter { context, document }, plan);
                    observation.replace(Some(value.clone()));
                    Ok(Reply::Value(value))
                });
                match result {
                    Ok(reply) => Ok(reply),
                    Err(_) => Ok(Reply::Value(
                        json!({"success":false,"errorCode":"native_document_guard_failed","outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":null,"observedNativeResult":observation.into_inner()}),
                    )),
                }
            }
            Task::BomExportPreflight(request) => {
                let id = request.arguments()["target"]["documentId"]
                    .as_str()
                    .ok_or("missing exact document target")?;
                self.registry.with_document(id,|context,document,_|{
                    let result=bom_adapter::generate(context,document,&self.queue.epoch().to_string(),id,request.arguments()["target"]["revision"].as_str(),request.arguments()["explodePurchased"].as_bool().unwrap_or(false))
                        .and_then(|captured|crate::bom_export::Plan::prepare(request,captured));
                    Ok(match result {Ok(plan)=>Reply::BomExportPlan(plan),Err(code)=>Reply::Value(json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}))})
                })
            }
            Task::LifecyclePreflight(request, proof) => Ok(
                match self.registry.lifecycle_preflight(request, proof.clone()) {
                    Ok(plan) => Reply::LifecyclePlan(plan),
                    Err(error) => Reply::Value(
                        json!({"success":false,"errorCode":"native_lifecycle_preflight_refused","message":error,"nativeExecutionStarted":false}),
                    ),
                },
            ),
            Task::LifecycleExecute(plan) => Ok(Reply::Value(
                self.registry
                    .lifecycle_execute(plan.take().ok_or("lifecycle plan already consumed")?),
            )),
            Task::Snapshot => {
                self.stats.snapshot_calls.fetch_add(1, Ordering::Relaxed);
                self.registry.refresh().map(Reply::Snapshot)
            }
            Task::Inspect(request) => {
                if matches!(
                    request.operation(),
                    "bom"
                        | "material_list"
                        | "mcmaster_discover"
                        | "assembly_properties"
                        | "native_sketch_inspect"
                        | "native_sketch_profiles"
                        | "native_sketch_entities"
                        | "native_sketch_line_state"
                        | "native_assembly_inspect"
                        | "native_sketch_dimensions"
                        | "native_sketch_relations"
                ) && request.arguments()["target"]["documentId"]
                    .as_str()
                    .is_none_or(str::is_empty)
                {
                    return Ok(Reply::Value(
                        json!({"success":false,"errorCode":"native_bom_exact_target_required","nativeExecutionStarted":false}),
                    ));
                }
                if request.arguments().get("target").is_some_and(|target| {
                    target
                        .get("documentId")
                        .is_some_and(|id| id.as_str().is_none_or(str::is_empty))
                }) {
                    return crate::execution::read_target_refusal(
                        crate::identity::ReadTargetError::Malformed,
                    );
                }
                let id = if let Some(id) = request.arguments()["target"]["documentId"].as_str() {
                    id.to_owned()
                } else {
                    self.registry
                        .refresh()?
                        .documents
                        .into_iter()
                        .find(|d| d.active)
                        .and_then(|d| d.document_id)
                        .ok_or("no active document")?
                };
                self.registry.with_read_document(&id,|context,document,_| {
                    if request.operation()=="native_assembly_inspect" {
                        context.extrusion_diagnostic.replace(Value::Null);
                        let adapter=assembly_adapter::Adapter{context,document};
                        let mut book=context.assemblies.try_borrow_mut().map_err(|_|"assembly registry busy")?;
                        let a=request.arguments();
                        return Ok(Reply::Value(match book.inspect(&adapter,&id,a["target"]["revision"].as_str(),a["nativeMetadata"].as_bool().unwrap_or(false)) {
                            Ok(value)=>value,
                            Err(code) if !crate::assembly::known_refusal(&code)=>json!({"success":false,"errorCode":code,"outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":false,"readBackendInvoked":true,"sdkDiagnostic":context.extrusion_diagnostic.borrow().clone()}),
                            Err(code)=>json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
                        }));
                    }
                    if request.operation()=="native_sketch_dimensions" {
                        let adapter=sketch_adapter::Adapter{context,document};
                        context.extrusion_diagnostic.replace(Value::Null);
                        let book=context.sketches.try_borrow().map_err(|_|"sketch registry busy")?;
                        let a=request.arguments();
                        return Ok(Reply::Value(match book.inspect_dimensions(&adapter,&id,a["sketchId"].as_str().ok_or("missing sketch id")?,a["entityId"].as_str().ok_or("missing entity id")?,a["target"]["revision"].as_str()) {
                            Ok(data)=>json!({"success":true,"data":data}),
                            Err(code) if !crate::sketches::dimensions::known_refusal(&code)=>json!({"success":false,"errorCode":code,"outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":false,"readBackendInvoked":true,"sdkDiagnostic":context.extrusion_diagnostic.borrow().clone()}),
                            Err(code)=>json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
                        }));
                    }
                    if request.operation()=="native_sketch_relations" {
                        let adapter=sketch_adapter::Adapter{context,document};
                        context.extrusion_diagnostic.replace(Value::Null);
                        let book=context.sketches.try_borrow().map_err(|_|"sketch registry busy")?;
                        let a=request.arguments();
                        return Ok(Reply::Value(match book.inspect_relations(&adapter,&id,a["sketchId"].as_str().ok_or("missing sketch id")?,a["entityId"].as_str().ok_or("missing entity id")?,a["target"]["revision"].as_str()) {
                            Ok(data)=>json!({"success":true,"data":data}),
                            Err(code) if !crate::sketches::line_state::known_refusal(&code)=>json!({"success":false,"errorCode":code,"outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":false,"readBackendInvoked":true,"sdkDiagnostic":context.extrusion_diagnostic.borrow().clone()}),
                            Err(code)=>json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
                        }));
                    }
                    if request.operation()=="native_sketch_line_state" {
                        let adapter=sketch_adapter::Adapter{context,document};
                        context.extrusion_diagnostic.replace(Value::Null);
                        let book=context.sketches.try_borrow().map_err(|_|"sketch registry busy")?;
                        let a=request.arguments();
                        return Ok(Reply::Value(match book.inspect_line_state(&adapter,&id,a["sketchId"].as_str().ok_or("missing sketch id")?,a["entityId"].as_str().ok_or("missing entity id")?,a["target"]["revision"].as_str()) {
                            Ok(data)=>json!({"success":true,"data":data}),
                            Err(code) if !crate::sketches::line_state::known_refusal(&code)=>json!({"success":false,"errorCode":code,"outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":false,"readBackendInvoked":true,"sdkDiagnostic":context.extrusion_diagnostic.borrow().clone()}),
                            Err(code)=>json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
                        }));
                    }
                    if request.operation()=="assembly_properties" {
                        let adapter=physical_adapter::Adapter{context,document};
                        return match crate::physical::inspect(&adapter,&self.queue.epoch().to_string(),&id,&request){Ok(data)=>Ok(Reply::Value(json!({"success":true,"data":data}))),Err(code) if code=="target_revision_mismatch"=>Ok(Reply::Value(json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}))),Err(code)=>Err(code)};
                    }
                    if request.operation()=="mcmaster_discover" {
                        let adapter=purchased_adapter::Adapter{context,document};
                        return match crate::purchased::inspect(&adapter,&self.queue.epoch().to_string(),&id,&request){Ok(data)=>Ok(Reply::Value(json!({"success":true,"data":data}))),Err(code) if code=="target_revision_mismatch"=>Ok(Reply::Value(json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}))),Err(code)=>Err(code)};
                    }
                    if request.operation()=="material_list" {
                        let adapter=material_adapter::Adapter{context,document};
                        return Ok(Reply::Value(match crate::materials::inspect(&adapter,&id,&request) {
                            Ok(data)=>json!({"success":true,"data":data}),Err(code)=>json!({"success":false,"errorCode":code,"nativeExecutionStarted":false})
                        }));
                    }
                    if request.operation()=="native_sketch_entities" {
                        let adapter=sketch_adapter::Adapter{context,document};
                        let book=context.sketches.try_borrow().map_err(|_|"sketch registry busy")?;
                        return Ok(Reply::Value(match book.inspect_entities(&adapter,&id,request.arguments()["sketchId"].as_str().ok_or("missing sketch id")?,request.arguments()["target"]["revision"].as_str()){
                            Ok(data)=>json!({"success":true,"data":data}),Err(code)=>json!({"success":false,"errorCode":code,"nativeExecutionStarted":false})
                        }));
                    }
                    if request.operation()=="native_sketch_profiles" {
                        use crate::sketches::extrusion::ExtrusionBackend;
                        let adapter=sketch_adapter::Adapter{context,document};
                        let book=context.sketches.try_borrow().map_err(|_|"sketch registry busy")?;
                        return Ok(Reply::Value(match book.inspect_profiles(&adapter,&id,request.arguments()["sketchId"].as_str().ok_or("missing sketch id")?,request.arguments()["target"]["revision"].as_str()){
                            Ok(data)=>json!({"success":true,"data":data}),Err(code)=>json!({"success":false,"errorCode":code,"nativeExecutionStarted":false,"sdkDiagnostic":adapter.diagnostic()})
                        }));
                    }
                    if request.operation()=="native_sketch_inspect" {
                        let book=context.sketches.try_borrow().map_err(|_|"sketch registry busy")?;
                        let observed=book.inspect(&sketch_adapter::Adapter{context,document},&id,request.arguments()["sketchId"].as_str());
                        return Ok(Reply::Value(match observed {
                            Ok(data) if request.arguments()["target"]["revision"].as_str().is_some_and(|r|data["target"]["revision"]!=r)=>json!({"success":false,"errorCode":"target_revision_mismatch","nativeExecutionStarted":false}),
                            Ok(data)=>json!({"success":true,"data":data}),
                            Err(code)=>json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
                        }));
                    }
                    if request.operation()=="bom" {
                        return Ok(Reply::Value(match bom_adapter::generate(context,document,&self.queue.epoch().to_string(),&id,request.arguments()["target"]["revision"].as_str(),request.arguments()["explodePurchased"].as_bool().unwrap_or(false)) {
                            Ok(data)=>json!({"success":true,"data":data.data}),
                            Err(code)=>json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
                        }));
                    }
                    let backend=NativeMechanics{context,document};
                    Ok(Reply::Value(match mechanics::inspect(&backend,&id) {
                        Ok(data)=>{
                            if request.arguments()["target"]["revision"].as_str().is_some_and(|revision|revision!=data.target.revision) {
                                json!({"success":false,"errorCode":"target_revision_mismatch","nativeExecutionStarted":false})
                            }else{json!({"success":true,"data":data})}
                        },
                        Err(error)=>json!({"success":false,"errorCode":error.code,"message":error.message,"nativeExecutionStarted":false}),
                    }))
                }).or_else(crate::execution::read_target_refusal)
            }
            Task::Preflight(request) => {
                let id = request.arguments()["target"]["documentId"]
                    .as_str()
                    .ok_or("missing exact document target")?;
                self.registry.with_document(id,|context,document,_| {
                    let backend=NativeMechanics{context,document};
                    Ok(match mechanics::preflight(&backend,request,id) {
                        Ok(plan)=>Reply::Plan(plan),
                        Err(error)=>Reply::Value(json!({"success":false,"errorCode":error.code,"message":error.message,"nativeExecutionStarted":false})),
                    })
                })
            }
            Task::Execute(plan) => {
                let plan = plan.take().ok_or("execution plan already consumed")?;
                let id = plan.document_id().to_owned();
                let observation = std::cell::RefCell::new(None);
                let result = self.registry.with_document(&id, |context, document, _| {
                    let value = mechanics::execute(&NativeMechanics { context, document }, plan);
                    observation.replace(Some(value.clone()));
                    Ok(Reply::Value(value))
                });
                match result {
                    Ok(reply) => Ok(reply),
                    Err(_) => Ok(Reply::Value(
                        json!({"success":false,"errorCode":"native_document_guard_failed","outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":null,"observedNativeResult":observation.into_inner()}),
                    )),
                }
            }
        }
    }
}
unsafe extern "C" fn on_wake(user: *mut c_void) {
    if user.is_null() {
        return;
    }
    // Fusion's handler invokes this only on its registering main thread. The
    // Box remains pinned in NativeState until handler removal and worker join.
    let callback = &*user.cast::<Callback>();
    #[cfg(not(feature = "native-wake-probe"))]
    let Some(_flight) = callback.gate.enter() else {
        return;
    };
    callback.stats.callback(&callback.queue, || {
        #[cfg(not(feature = "native-wake-probe"))]
        let Some(generation) = callback.wake.get().and_then(|wake| wake.begin_callback()) else {
            // Coordinator refusal precedes queue shutdown in another thread.
            // It must independently forbid native claims in that small gap.
            return Ok(());
        };
        callback.stats.callback_stage(CallbackStage::ThreadId);
        let native_thread = af_thread_id();
        callback
            .stats
            .native_thread
            .store(native_thread as u64, Ordering::Relaxed);
        #[cfg(feature = "native-wake-probe")]
        callback.stats.probe_callback_arrived(native_thread);
        #[cfg(feature = "native-wake-probe")]
        let Some(_flight) = callback.gate.enter() else {
            return Ok(());
        };
        // Unique random event ID isolates DLL epochs. Event payloads are ignored;
        // only requests already admitted by the authenticated Rust listener run.
        callback.stats.callback_stage(CallbackStage::Drain);
        let drained =
            super::service::drain_tasks_with_stats(&callback.queue, &callback.stats, |task| {
                callback.stats.callback_stage(CallbackStage::Snapshot);
                let result = callback.perform(task);
                callback.stats.callback_stage(CallbackStage::Drain);
                result
            });
        // Publish the claimed read's actual result before stopping admission.
        // An irrecoverably faulted registry cannot serve another native read.
        if callback.registry.is_faulted() {
            callback
                .stats
                .shutdown(&callback.queue, ShutdownReason::DocumentRegistryFaulted);
        }
        drained?;
        // Autodesk's installed Application.h guarantees fireCustomEvent queues
        // the event for a later idle cycle, never an immediate handler call. Keep
        // the flight guard alive through this FFI call so reentrant stop cannot
        // release its handle. Rearm work whose original nested wake was consumed.
        #[cfg(not(feature = "native-wake-probe"))]
        {
            callback.stats.callback_stage(CallbackStage::Rearm);
            if let Some(wake) = callback.wake.get() {
                let _ = wake.finish_callback(generation);
            }
        }
        Ok(())
    });
}

#[derive(Clone, Copy)]
struct WakeHandle(NonNull<c_void>);
// The sole allowed worker-side access is af_event_fire, wrapping Autodesk's
// documented thread-safe fireCustomEvent. NativeState joins every worker before
// changing or releasing this event or its application reference.
unsafe impl Send for WakeHandle {}
unsafe impl Sync for WakeHandle {}
impl WakeHandle {
    #[cfg(not(feature = "native-wake-probe"))]
    fn fire(&self, stats: &Stats) -> WakeAck {
        let mut status = 0;
        stats.native_wake(|| {
            status = unsafe { af_event_fire(self.0.as_ptr()) };
            status
        });
        WakeAck::from_status(status)
    }
    #[cfg(feature = "native-wake-probe")]
    fn probe_fire(&self, stats: &Stats, queue: &Queue) -> bool {
        stats.probe_wake(queue, || unsafe { af_event_fire(self.0.as_ptr()) })
    }
}

struct NativeState {
    callback: Option<Box<Callback>>,
    event: Option<NonNull<c_void>>,
    service: Option<Service>,
    directory: PathBuf,
    epoch: String,
    event_close: CloseOnce,
    module: Option<ModuleReference>,
}
impl NativeState {
    fn cleanup(&mut self) -> Result<bool, String> {
        // Refuse before joining workers or touching SDK storage: an SDK getter
        // may have reentered Fusion's stop callback on this same main thread.
        if let Some(callback) = &self.callback {
            callback.gate.begin_stop()?;
        }
        let joined = self.service.take().is_none_or(|mut service| service.stop());
        if let Some(callback) = &self.callback {
            callback
                .stats
                .shutdown(&callback.queue, ShutdownReason::NativeCleanup);
        }
        if let Some(event) = self.event {
            self.event_close
                .close(|| unsafe { checked(af_event_close(event.as_ptr())) })?;
            self.event = None;
        }
        if let Some(callback) = &mut self.callback {
            callback.registry.close()?;
            callback.registry.released_backend()?.close()?;
        }
        self.callback = None;
        Ok(joined)
    }
    fn stop(&mut self) -> Result<(), String> {
        let callback = self
            .callback
            .as_ref()
            .ok_or("native state has no callback")?;
        callback.gate.begin_stop()?;
        let after = callback.registry.refresh();
        let owned_scratch = callback.registry.owned_scratch_status();
        let stats = Arc::clone(&callback.stats);
        let queue = Arc::clone(&callback.queue);
        let joined = self.cleanup()?;
        let stats = stats.diagnostic_json();
        write_new(
            &self.directory.join("native-stop.json"),
            &json!({"success":joined,
            "epoch":self.epoch,"workersJoined":true,"workersExitedNormally":joined,
            "eventRemoved":true,"sdkReferencesReleased":true,"outstanding":queue.outstanding(),"ownedScratchAtStop":owned_scratch,"documentsAutomaticallyClosed":false,
            "after":after,"stats":stats}),
        )?;
        if joined {
            Ok(())
        } else {
            Err("worker panic observed; workers joined and SDK released".into())
        }
    }
}
impl Drop for NativeState {
    fn drop(&mut self) {
        if self.event.is_none() && self.service.is_none() && self.callback.is_none() {
            return;
        }
        if let Err(error) = self.cleanup() {
            // The module reference was acquired and checked BEFORE registration.
            // Retain it and callback storage on uncertainty. No unload or SDK
            // deletion retry is justified by an unconfirmed teardown.
            if let Some(module) = self.module.take() {
                std::mem::forget(module);
            }
            if let Some(callback) = self.callback.take() {
                Box::leak(callback);
            }
            let _ = write_new(
                &self.directory.join("native-retained.json"),
                &json!({"epoch":self.epoch,
                "error":error,"reconciliationRequired":true,"moduleReferenceRetained":true,
                "reason":"SDK cleanup unconfirmed; native storage retained"}),
            );
        }
    }
}
thread_local! {static STATE:StateSlot<NativeState>=const{StateSlot::new()};}
static ACTIVE: AtomicBool = AtomicBool::new(false);

fn module_directory() -> Result<PathBuf, String> {
    use std::os::windows::ffi::OsStringExt;
    let mut module = std::ptr::null_mut();
    unsafe {
        if GetModuleHandleExW(6, run as *const () as *const u16, &mut module) == 0 {
            return Err("module lookup failed".into());
        }
    }
    let mut path = vec![0u16; 32768];
    let len = unsafe { GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) } as usize;
    if len == 0 || len >= path.len() {
        return Err("module path unavailable".into());
    }
    let module_path = PathBuf::from(std::ffi::OsString::from_wide(&path[..len]));
    Ok(module_path
        .parent()
        .ok_or("module has no parent")?
        .to_path_buf())
}
struct ModuleReference(NonNull<c_void>);
impl ModuleReference {
    fn acquire() -> Result<Self, String> {
        let mut module = std::ptr::null_mut();
        // FROM_ADDRESS increments the module reference count. Unlike a late
        // best-effort pin, failure here prevents any callback registration.
        if unsafe { GetModuleHandleExW(4, run as *const () as *const u16, &mut module) } == 0 {
            return Err("cannot retain native module lifetime".into());
        }
        Ok(Self(
            NonNull::new(module).ok_or("null native module reference")?,
        ))
    }
}
impl Drop for ModuleReference {
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.0.as_ptr());
        }
    }
}
fn write_new(path: &Path, value: &Value) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}
fn start() -> Result<(), String> {
    let directory = module_directory()?;
    write_new(
        &directory.join("native-start-attempt.json"),
        &json!({"pid":std::process::id(),"threadId":unsafe{af_thread_id()},"scope":if cfg!(feature="native-wake-probe") {"read_only_probe"} else {"native-sketch-v1"}}),
    )?;
    let queue = Queue::new(8).map_err(|e| e.to_string())?;
    let epoch = queue.epoch().to_string();
    let event_id = format!("AdomFusionRustNative.{epoch}");
    let stats = Arc::new(Stats::default());
    let module = ModuleReference::acquire()?;
    let context = Context::open()?;
    stats
        .native_thread
        .store(context.thread_id() as u64, Ordering::Relaxed);
    let mut callback = Box::new(Callback {
        gate: CallbackGate::default(),
        #[cfg(not(feature = "native-wake-probe"))]
        wake: OnceCell::new(),
        registry: Registry::new(epoch.clone(), context)?,
        queue: Arc::clone(&queue),
        stats: Arc::clone(&stats),
    });
    let user = (&mut *callback as *mut Callback).cast();
    let mut state = NativeState {
        callback: Some(callback),
        event: None,
        service: None,
        directory: directory.clone(),
        epoch: epoch.clone(),
        event_close: CloseOnce::default(),
        module: Some(module),
    };
    let before = state.callback.as_ref().unwrap().registry.refresh()?;
    let mut event = std::ptr::null_mut();
    let code = unsafe {
        af_event_open(
            event_id.as_ptr().cast(),
            event_id.len(),
            on_wake,
            user,
            &mut event,
        )
    };
    state.event = NonNull::new(event);
    checked(code)?;
    let wake_handle = WakeHandle(state.event.ok_or("null event handle")?);
    let wake_stats = Arc::clone(&stats);
    #[cfg(not(feature = "native-wake-probe"))]
    let wake: Wake = {
        let coordinator = WakeCoalescer::with_stats(
            Arc::clone(&queue),
            Arc::new(move || wake_handle.fire(&wake_stats)),
            Arc::clone(&stats),
        );
        state
            .callback
            .as_ref()
            .unwrap()
            .wake
            .set(Arc::clone(&coordinator))
            .map_err(|_| "wake coordinator already assigned")?;
        coordinator
    };
    #[cfg(feature = "native-wake-probe")]
    let wake: Wake = {
        let wake_queue = Arc::clone(&queue);
        Arc::new(move || wake_handle.probe_fire(&wake_stats, &wake_queue))
    };
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let mut token_bytes = [0u8; 32];
    getrandom::getrandom(&mut token_bytes).map_err(|e| e.to_string())?;
    let token: String = token_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let reader = OperationStatusReader::new(directory.join("journal"), epoch.clone());
    #[cfg(not(feature = "native-wake-probe"))]
    let service =
        Service::start_with_operations(listener, queue, token.clone(), wake, stats, reader);
    #[cfg(feature = "native-wake-probe")]
    let service =
        Service::start_with_status(listener, queue, token.clone(), wake, stats, Some(reader));
    state.service = Some(service.map_err(|e| e.to_string())?);
    let address = state.service.as_ref().unwrap().address();
    write_new(
        &directory.join("native-runtime.json"),
        &json!({"protocolVersion":1,"pid":std::process::id(),
        "epoch":epoch,"eventId":event_id,"address":address.to_string(),"token":token,
        "implementation":"Rust with native C++ ABI adapter","mutationSupport":!cfg!(feature="native-wake-probe"),
        "operationStatusConfigured":true,"journalInitialization":"background",
        "effectJournalConfigured":!cfg!(feature="native-wake-probe"),"effectJournalConnected":false,"connectionStateSource":"GET /v1/health","operations":if cfg!(feature="native-wake-probe") {vec!["document_snapshot","operation_status"]} else {vec!["document_snapshot","operation_status","inspect","parameter_create","parameter_set","new_design","native_scratch_discard","native_document_activate","bom","bom_export","sketch_create","native_sketch_inspect","sketch_geometry","native_sketch_profiles","feature_create","native_sketch_entities","material_list","mcmaster_discover","assembly_properties","native_sketch_line_state","native_sketch_relations","sketch_constraint","native_sketch_dimensions","sketch_dimension","native_assembly_inspect","component_create","occurrence_insert","component_metadata","occurrence_update"]},"mechanicalContract":if cfg!(feature="native-wake-probe") {None} else {Some("native-root-assembly-v1")},"before":before,
        "experimentalSingleReadProbe":cfg!(feature="native-wake-probe")}),
    )?;
    STATE
        .with(|slot| slot.put(state))
        .map_err(|_| "native state already owned")?;
    Ok(())
}
#[no_mangle]
pub extern "C" fn run(_context: *const c_char) -> bool {
    // A duplicate invocation must not clear the serving instance's ownership.
    if ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    match std::panic::catch_unwind(start) {
        Ok(Ok(())) => true,
        result => {
            let error = match result {
                Ok(Err(error)) => error,
                _ => "native startup panic".into(),
            };
            if let Ok(directory) = module_directory() {
                let _ = write_new(
                    &directory.join("native-start-error.json"),
                    &json!({"success":false,"error":error}),
                );
            }
            ACTIVE.store(false, Ordering::Release);
            false
        }
    }
}
#[no_mangle]
pub extern "C" fn stop(_context: *const c_char) -> bool {
    std::panic::catch_unwind(|| {
        STATE.with(|slot| {
            let mut state = match slot.take() {
                Some(state) => state,
                None => return !ACTIVE.load(Ordering::Acquire),
            };
            match state.stop() {
                Ok(()) => {
                    ACTIVE.store(false, Ordering::Release);
                    true
                }
                Err(_) => {
                    if state.event.is_some() || state.callback.is_some() || state.service.is_some()
                    {
                        let _ = slot.put(state);
                    } else {
                        ACTIVE.store(false, Ordering::Release);
                    }
                    false
                }
            }
        })
    })
    .unwrap_or(false)
}
