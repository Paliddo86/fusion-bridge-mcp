//! One owned coordinator preserves native outcomes beyond HTTP observer timeouts.
//! Journal I/O runs here; the SDK callback receives owned data and no I/O locks.
use crate::{
    identity::document_lifecycle::{self, CreationProof, Plan as LifecyclePlan},
    mechanics::Plan,
    operation_status::OperationStatusReader,
    service::{Queue, Wake},
    Snapshot,
};
use fusion_mechanical_core::PreparedRequest;
use fusion_mechanical_journal::{now_ms, retained::Admission};
use fusion_native_dispatch::Outcome;
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Debug)]
pub enum Task {
    Snapshot,
    Inspect(Arc<PreparedRequest>),
    Preflight(Arc<PreparedRequest>),
    Execute(Option<Plan>),
    LifecyclePreflight(Arc<PreparedRequest>, Option<CreationProof>),
    LifecycleExecute(Option<LifecyclePlan>),
    BomExportPreflight(Arc<PreparedRequest>),
    SketchPreflight(Arc<PreparedRequest>),
    SketchExecute(Option<crate::sketches::Plan>),
    GeometryPreflight(Arc<PreparedRequest>),
    GeometryExecute(Option<crate::sketches::geometry::Plan>),
    AssemblyPreflight(Arc<PreparedRequest>),
    AssemblyExecute(Option<crate::assembly::Plan>),
    DimensionPreflight(Arc<PreparedRequest>),
    DimensionExecute(Option<crate::sketches::dimensions::Plan>),
    RelationPreflight(Arc<PreparedRequest>),
    RelationExecute(Option<crate::sketches::relations::Plan>),
    ExtrudePreflight(Arc<PreparedRequest>),
    ExtrudeExecute(Option<crate::sketches::extrusion::Plan>),
}
#[derive(Debug)]
pub enum Reply {
    Snapshot(Snapshot),
    Value(Value),
    Plan(Plan),
    LifecyclePlan(LifecyclePlan),
    BomExportPlan(crate::bom_export::Plan),
    SketchPlan(crate::sketches::Plan),
    GeometryPlan(crate::sketches::geometry::Plan),
    AssemblyPlan(crate::assembly::Plan),
    DimensionPlan(crate::sketches::dimensions::Plan),
    RelationPlan(crate::sketches::relations::Plan),
    ExtrudePlan(crate::sketches::extrusion::Plan),
}
enum ExecutionPlan {
    Mechanical(Plan),
    Lifecycle(LifecyclePlan),
    BomExport(crate::bom_export::Plan),
    Sketch(crate::sketches::Plan),
    Geometry(crate::sketches::geometry::Plan),
    Assembly(crate::assembly::Plan),
    Dimension(crate::sketches::dimensions::Plan),
    Relation(crate::sketches::relations::Plan),
    Extrude(crate::sketches::extrusion::Plan),
}
impl ExecutionPlan {
    fn observed_document(&self) -> Value {
        match self {
            Self::Mechanical(p) => p.observed_document(),
            Self::Lifecycle(p) => p.observed_document(),
            Self::BomExport(p) => p.observed_document(),
            Self::Sketch(p) => p.observed_document(),
            Self::Geometry(p) => p.observed_document(),
            Self::Assembly(p) => p.observed_document(),
            Self::Dimension(p) => p.observed_document(),
            Self::Relation(p) => p.observed_document(),
            Self::Extrude(p) => p.observed_document(),
        }
    }
}
type Response = (u16, Value);
struct Job {
    request: Arc<PreparedRequest>,
    deadline: Instant,
    response: SyncSender<Response>,
}
pub struct Executor {
    queue: Arc<Queue>,
    wake: Wake,
    status: Arc<OperationStatusReader>,
    sender: SyncSender<Job>,
    receiver: Mutex<Option<Receiver<Job>>>,
    faulted: AtomicBool,
}
pub fn capabilities() -> Value {
    let mut schema = fusion_mechanical_core::validated_schema()
        .unwrap()
        .contracts_json()
        .clone();
    schema
        .as_object_mut()
        .unwrap()
        .retain(|name, _| supported(name));
    schema["mcmaster_discover"]["description"]=json!("Read-only exact native McMaster command registration and target design intent. Requires current sessionId, exact documentId and native-purchased-discovery-v1 header. No command execution, online/applicability verification, UI, download or CAD import support.");
    schema["mcmaster_discover"]["required"] = json!(["op", "sessionId", "target"]);
    schema["assembly_properties"]["description"] = json!("Read-only bounded high-accuracy physical properties of the exact document root; hybrid, local solid BRep bodies only. No occurrences, meshes, configurations or entity selectors. Requires current sessionId and native-root-physical-v1 header; optional root-physical-projection-v1 revision. At most128 bodies/20000 faces before physical calculation, two equal root-revision-bracketed observations. Mass kg, volume mm3, area mm2, center mm; no material assignment or full assembly parity.");
    schema["assembly_properties"]["required"] = json!(["op", "sessionId", "target"]);
    schema["assembly_properties"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("entity");
    schema["material_list"]["description"]=json!("Read-only bounded document/loaded-library material IDs and names, query and limits. Exact documentId, two full equal catalogs, no load/copy/assignment. Guard optional material-catalog-projection-v1; requires native-material-catalog-v1 header. Bounds:64 libraries,20000 selected-source materials,1MiB projection.");
    schema["inspect"]["description"] = json!("Native parameter projection, design summary and timeline health only. complete:false; body geometry and occurrences are not included.");
    schema["parameter_set"]["description"] = json!("Native once-only expression set, recompute and readback. No rollback or recovery is promised by native-parameters-v1; uncertain failure requires reconciliation. This is not full legacy parameter_set parity.");
    schema["parameter_create"]["description"] = json!("Native once-only user parameter creation, recompute and readback. No rollback is promised; uncertain failure requires reconciliation.");
    schema["new_design"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("box");
    schema["new_design"]["description"]=json!("Create one unnamed unsaved blank parametric hybrid Design and adopt its exact returned SDK object. requestedName is session/receipt metadata only. No box, transaction probe, root attribute, automatic failure cleanup, or full legacy inspection parity.");
    schema["bom"]["description"]=json!("Native complete BOM requires online Fusion for component metadata and supports only bounded local unconfigured hybrid assemblies, unsuppressed current timeline and solid BRep bodies. Exact SDK equality groups definitions within one capture; IDs are not stable across captures. Definition and occurrence-context high-accuracy properties are separately observed. Two complete passes must agree. Initial target may omit revision; later reads may guard returned bom-projection-v1.");
    schema["bom_export"]["description"]=json!("Native flat/structured CSV of an immutable complete supported BOM snapshot. Requires exact documentId and fresh bom-projection-v1 revision. After durable journal start, Rust publishes once on its worker with exclusive/default or explicit overwrite semantics. No CAD mutation, automatic retry, geometry-wide revision or power-loss durability claim.");
    schema["sketch_create"]["description"]=json!("Create one exact root XY/XZ/YZ origin-plane sketch with its SDK default name. Requires sketch-root-projection-v1. No custom name/component/reference plane, insertion, rollback or complete legacy parity.");
    schema["sketch_create"]["properties"] = json!({"plane":{"enum":["xy","xz","yz"]}});
    schema["sketch_geometry"]["description"] = json!("Bounded non-atomic insertion of 8 prepared primitives into an exact retained empty root origin sketch. Explicit native-sketch-geometry-v1 contract, matching sketch-projection revisions and exact opaque sketchId required. Sketch-space mm input; actual cm/radian readback. Rectangle/polygon/slot use explicit edges with no legacy constraint parity; failure after invocation remains unknown with observed prefix. No retry or rollback.");
    schema["sketch_geometry"]["properties"]["sketch"] = json!({"type":"object","required":["documentId","revision","objectType","sketchId"],"additionalProperties":false,"properties":{"documentId":{"type":"string"},"revision":{"type":"string"},"objectType":{"enum":["Sketch"]},"sketchId":{"type":"string"}}});
    schema["sketch_geometry"]["properties"]["items"]["maxItems"] =
        json!(crate::sketches::geometry::MAX_ITEMS);
    schema["feature_create"]["description"] = json!("Native finite new-body extrusion of one explicitly published retained sole outer-loop profile. Numeric mm positive/negative one-sided zero-taper; empty root body/extrude collections. No expressions, booleans, symmetry, component/name selection, rollback or full legacy parity. Requires native-extrude-v1.");
    schema["feature_create"]["properties"]["type"] = json!({"enum":["extrude"]});
    let mut extrude_shape = schema["feature_create"]["oneOf"][0].clone();
    for key in ["component", "name", "taper", "bodies"] {
        extrude_shape["properties"]
            .as_object_mut()
            .unwrap()
            .remove(key);
    }
    extrude_shape["properties"]["distance"] =
        json!({"type":"number","minimum":0.0001,"maximum":100000.});
    extrude_shape["properties"]["direction"] = json!({"enum":["positive","negative"]});
    extrude_shape["properties"]["operation"] = json!({"enum":["new_body"]});
    extrude_shape["properties"]["profiles"]["maxItems"] = json!(1);
    extrude_shape["properties"]["profiles"]["items"] = json!({"type":"object","required":["documentId","revision","objectType","sketchId","profileId"],"additionalProperties":false,"properties":{"documentId":{"type":"string"},"revision":{"type":"string"},"objectType":{"enum":["Profile"]},"sketchId":{"type":"string"},"profileId":{"type":"string"}}});
    schema["feature_create"]["properties"] = extrude_shape["properties"].clone();
    schema["feature_create"]["oneOf"] = json!([extrude_shape]);
    schema["sketch_constraint"]["description"]=json!("Once-only horizontal or vertical on one exact retained unfixed isolated line. Requires complete sketch-line-state-projection-v1, exact matching sketch/entity refs and native-sketch-relations-v1. No arbitrary graph, endpoint placement/length promise, rollback, retry or full legacy parity.");
    schema["sketch_constraint"]["properties"]["type"] = json!({"enum":["horizontal","vertical"]});
    schema["sketch_constraint"]["properties"]["entities"]["maxItems"] = json!(1);
    for op in [
        "component_create",
        "component_metadata",
        "occurrence_insert",
        "native_assembly_inspect",
    ] {
        schema[op]["description"] = json!(crate::assembly::SCOPE);
    }
    for op in ["component_create", "occurrence_insert"] {
        schema[op]["properties"]
            .as_object_mut()
            .unwrap()
            .remove("parent");
        schema[op]["properties"]["transform"]["description"]=json!("Only absent or exactly identity transform is supported. Nonzero translation/rotation refuses before admission; no snapshot capture.");
    }
    schema["occurrence_update"]["description"] = json!(crate::assembly::placement::SCOPE);
    schema["occurrence_update"]["properties"] = json!({"occurrence":{"type":"object","required":["documentId","revision","objectType","occurrenceId"],"additionalProperties":false,"properties":{"documentId":{"type":"string"},"revision":{"type":"string"},"objectType":{"enum":["Occurrence"]},"occurrenceId":{"type":"string"}}},"transform":{"type":"object","required":["position"],"additionalProperties":false,"properties":{"position":{"type":"array","minItems":3,"maxItems":3,"items":{"type":"number","minimum":-1000000,"maximum":1000000}}}}});
    schema["occurrence_insert"]["properties"]["independentCopy"] = json!({"enum":[false]});
    for key in ["partNumber", "description"] {
        schema["component_create"]["properties"]["metadata"]["properties"]
            .as_object_mut()
            .unwrap()
            .remove(key);
    }
    schema["component_metadata"]["required"] = json!([
        "op",
        "sessionId",
        "operationId",
        "target",
        "component",
        "changes"
    ]);
    for op in ["occurrence_insert", "component_metadata"] {
        schema[op]["properties"]["component"] = json!({"type":"object","required":["documentId","revision","objectType","componentId"],"additionalProperties":false,"properties":{"documentId":{"type":"string"},"revision":{"type":"string"},"objectType":{"enum":["Component"]},"componentId":{"type":"string"}}});
    }
    schema["sketch_dimension"]["description"] = json!(crate::sketches::dimensions::SCOPE);
    schema["sketch_dimension"]["properties"]["type"] = json!({"enum":["distance"]});
    schema["sketch_dimension"]["properties"]["expression"] =
        json!({"type":"number","minimum":0.001,"maximum":10000});
    schema["sketch_dimension"]["properties"]["entities"]["minItems"] = json!(2);
    schema["sketch_dimension"]["properties"]["entities"]["maxItems"] = json!(2);
    schema["sketch_dimension"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("parameterName");
    let mut result = json!({"success":true,"contractVersion":"native-root-assembly-v1","requiredHeader":"X-Adom-Mechanical-Contract: native-root-assembly-v1","dimensionsCompatibilityHeader":"X-Adom-Mechanical-Contract: native-sketch-dimensions-v1","entitiesCompatibilityHeader":"X-Adom-Mechanical-Contract: native-sketch-entities-v1","extrudeCompatibilityHeader":"X-Adom-Mechanical-Contract: native-extrude-v1","geometryCompatibilityHeader":"X-Adom-Mechanical-Contract: native-sketch-geometry-v1","sketchCompatibilityHeader":"X-Adom-Mechanical-Contract: native-sketch-v1","bomCompatibilityHeader":"X-Adom-Mechanical-Contract: native-bom-v1","lifecycleCompatibilityHeader":"X-Adom-Mechanical-Contract: native-lifecycle-v1","parameterCompatibilityHeader":"X-Adom-Mechanical-Contract: native-parameters-v1",
        "schemas":schema,
        "mutationSupport":true,"legacyParity":false,"transactional":false,"automaticRecovery":false,
        "revisionScope":crate::mechanics::REVISION_SCOPE,"maxQueuedOperations":8,
        "extrudeSupport":true,"extrudeScope":"One published exact sole outer-loop profile, numeric one-sided new-body solid, empty root body/extrude collections, no taper/booleans/rollback or legacy parity","extrudeCompleteInspection":false,"sketchRootRevisionScope":crate::sketches::ROOT_SCOPE,"sketchRevisionScope":crate::sketches::SKETCH_SCOPE,"sketchGeometrySupport":true,"geometryScope":"retained empty root origin sketch; explicit primitives, no legacy constraints or atomicity","geometryMaxItems":32,"geometryMaxEntities":64,"geometryMaxStoredPoints":512,"sketchCompleteInspection":false,"maxRetainedSketchesPerSession":crate::sketches::MAX_RETAINED,"sketchPlanes":["xy","xz","yz"],
        "bomRequiresOnline":true,"bomRevisionScope":crate::bom::REVISION_SCOPE,"bomMaxComponents":crate::bom::MAX_COMPONENTS,"bomMaxOccurrences":crate::bom::MAX_OCCURRENCES,"bomMaxBytes":crate::bom::MAX_BYTES,"bomConfiguredDesignSupport":false,"bomExternalOccurrenceSupport":false,
        "maxOwnedScratchPerSession":crate::identity::document_lifecycle::MAX_OWNED_SCRATCH,"commandGuardScope":"checked SelectCommand on the owning custom-event thread; no global transaction-state guarantee",
        "unknownOutcomePolicy":"Inspect operation status and actual state; never automatically replay an uncertain effect."});
    result["relationsCompatibilityHeader"] =
        json!("X-Adom-Mechanical-Contract: native-sketch-relations-v1");
    result["materialCompatibilityHeader"] =
        json!("X-Adom-Mechanical-Contract: native-material-catalog-v1");
    result["purchasedCompatibilityHeader"] =
        json!("X-Adom-Mechanical-Contract: native-purchased-discovery-v1");
    result["rootPhysicalSupport"] = json!(true);
    result["rootPhysicalRevisionScope"] = json!(crate::physical::SCOPE);
    result["rootPhysicalMaxBodies"] = json!(crate::physical::MAX_BODIES);
    result["rootPhysicalMaxFaces"] = json!(crate::physical::MAX_FACES);
    result["assemblyOccurrencePhysicalSupport"] = json!(false);
    result["mcmasterDiscoverySupport"] = json!(true);
    result["mcmasterCommandExecutionSupport"] = json!(false);
    result["mcmasterRevisionScope"] = json!(crate::purchased::SCOPE);
    result["readTargetRefusalScope"]=json!("malformed, foreign-epoch, never-published and retired selectors are refused before the operation backend; fresh registry SDK failures remain unknown");
    result["materialCatalogSupport"] = json!(true);
    result["materialCatalogRequiredHeader"] =
        json!("X-Adom-Mechanical-Contract: native-material-catalog-v1");
    result["materialCatalogRevisionScope"] = json!(crate::materials::SCOPE);
    result["sketchEntitySupport"] = json!(true);
    result["sketchEntityRevisionScope"] = json!(crate::sketches::entities::SCOPE);
    result["sketchEntityMaxBytes"] = json!(crate::sketches::entities::MAX_BYTES);
    result["physicalCompatibilityHeader"] =
        json!("X-Adom-Mechanical-Contract: native-root-physical-v1");
    result["operations"] = json!([
        "inspect",
        "parameter_create",
        "parameter_set",
        "new_design",
        "native_scratch_discard",
        "native_document_activate",
        "bom",
        "bom_export",
        "sketch_create",
        "native_sketch_inspect",
        "sketch_geometry",
        "native_sketch_profiles",
        "feature_create",
        "native_sketch_entities",
        "material_list",
        "mcmaster_discover",
        "assembly_properties",
        "native_sketch_line_state",
        "native_sketch_relations",
        "sketch_constraint",
        "native_sketch_dimensions",
        "sketch_dimension",
        "native_assembly_inspect",
        "component_create",
        "occurrence_insert",
        "component_metadata",
        "occurrence_update"
    ]);
    result["sketchLineStateSupport"] = json!(true);
    result["sketchLineStateRevisionScope"] = json!(crate::sketches::line_state::SCOPE);
    result["sketchConstraintSupport"] = json!(true);
    result["sketchConstraintTypes"] = json!(["horizontal", "vertical"]);
    result["rootAssemblySupport"] = json!(true);
    result["rootAssemblyScope"] = json!(crate::assembly::SCOPE);
    result["assemblyNativeMetadataRequiresOnline"] = json!(true);
    result["assemblyTransformsSupported"] = json!(false);
    result["occurrenceTranslationSupported"] = json!(true);
    result["placementContract"] = json!(crate::assembly::placement::CONTRACT);
    result["placementRequiredHeader"] =
        json!("X-Adom-Mechanical-Contract: native-root-occurrence-translation-v1");
    result["placementScope"] = json!(crate::assembly::placement::SCOPE);
    result["sketchDimensionSupport"] = json!(true);
    result["sketchDimensionScope"] = json!(crate::sketches::dimensions::SCOPE);
    result["sketchRelationScope"] = json!(crate::sketches::relations::SCOPE);
    result["lineStateCompatibilityHeader"] =
        json!("X-Adom-Mechanical-Contract: native-sketch-line-state-v1");
    result
}
pub fn supported(op: &str) -> bool {
    matches!(
        op,
        "inspect"
            | "parameter_create"
            | "parameter_set"
            | "bom"
            | "bom_export"
            | "material_list"
            | "mcmaster_discover"
            | "assembly_properties"
            | "sketch_create"
            | "sketch_geometry"
            | "feature_create"
            | "native_sketch_profiles"
            | "native_sketch_entities"
            | "native_assembly_inspect"
            | "component_create"
            | "component_metadata"
            | "occurrence_insert"
            | "occurrence_update"
            | "native_sketch_dimensions"
            | "sketch_dimension"
            | "native_sketch_relations"
            | "sketch_constraint"
            | "native_sketch_line_state"
            | "native_sketch_inspect"
    ) || document_lifecycle::is_lifecycle(op)
}
pub fn read_target_refusal(error: crate::identity::ReadTargetError) -> Result<Reply, String> {
    if let Some(code) = error.code() {
        Ok(Reply::Value(
            json!({"success":false,"errorCode":code,"nativeExecutionStarted":false,"reconciliationRequired":false,"readTargetRefused":true,"readBackendInvoked":false}),
        ))
    } else {
        match error {
            crate::identity::ReadTargetError::Unavailable(error) => Err(error),
            _ => unreachable!(),
        }
    }
}
fn refusal(code: &str) -> Response {
    (
        503,
        json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
    )
}
fn unknown(code: &str) -> Response {
    (
        503,
        json!({"success":false,"errorCode":code,"outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":null}),
    )
}
impl Executor {
    pub fn new(queue: Arc<Queue>, wake: Wake, status: Arc<OperationStatusReader>) -> Arc<Self> {
        let (sender, receiver) = mpsc::sync_channel(8);
        Arc::new(Self {
            queue,
            wake,
            status,
            sender,
            receiver: Mutex::new(Some(receiver)),
            faulted: AtomicBool::new(false),
        })
    }
    pub fn ready(&self) -> bool {
        !self.faulted.load(Ordering::Acquire)
            && !self.queue.is_shutdown()
            && self.status.journal().is_ok()
    }
    pub fn request(&self, request: PreparedRequest, deadline: Instant) -> Response {
        if !supported(request.operation()) {
            return (
                400,
                json!({"success":false,"errorCode":"unsupported_native_operation","nativeExecutionStarted":false}),
            );
        }
        if let Err(code) = document_lifecycle::supported_arguments(&request) {
            return (
                400,
                json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
            );
        }
        if let Err(code) =
            crate::physical::supported_arguments(&request, &self.queue.epoch().to_string())
        {
            return (
                400,
                json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
            );
        }
        if let Err(code) =
            crate::purchased::supported_arguments(&request, &self.queue.epoch().to_string())
        {
            return (
                400,
                json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
            );
        }
        if let Err(code) = crate::materials::supported_arguments(&request) {
            return (
                400,
                json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
            );
        }
        if let Err(code) = crate::bom_export::supported_arguments(&request) {
            return (
                400,
                json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
            );
        }
        if let Err(code) = crate::sketches::supported_arguments(&request) {
            return (
                400,
                json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
            );
        }
        if request.operation() == "sketch_geometry" {
            if let Err(code) = crate::sketches::geometry::prepare_arguments(&request) {
                return (
                    400,
                    json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
                );
            }
        }
        if let Err(code) = crate::sketches::entities::supported_arguments(&request) {
            return (
                400,
                json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
            );
        }
        if let Err(code) = crate::assembly::supported_arguments(&request) {
            return (
                400,
                json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
            );
        }
        if let Err(code) = crate::sketches::dimensions::supported_arguments(&request) {
            return (
                400,
                json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
            );
        }
        if let Err(code) = crate::sketches::relations::supported_arguments(&request) {
            return (
                400,
                json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
            );
        }
        if let Err(code) = crate::sketches::line_state::supported_arguments(&request) {
            return (
                400,
                json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
            );
        }
        if let Err(code) = crate::sketches::extrusion::supported_arguments(&request) {
            return (
                400,
                json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),
            );
        }
        if request.is_write()
            && !crate::http::status_selector(
                request.arguments()["operationId"].as_str().unwrap_or(""),
            )
        {
            return (
                400,
                json!({"success":false,"errorCode":"invalid_operation_selector","nativeExecutionStarted":false}),
            );
        }
        if !self.ready() {
            return refusal("native_execution_unavailable");
        }
        let (response, receiver) = mpsc::sync_channel(1);
        if self
            .sender
            .try_send(Job {
                request: Arc::new(request),
                deadline,
                response,
            })
            .is_err()
        {
            return refusal("native_execution_queue_full");
        }
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(result) => result,
            Err(_) => unknown("operation_observer_deadline"),
        }
    }
    /// Called exactly once by Service's owned initializer/coordinator worker.
    pub(crate) fn run(&self, stop: &AtomicBool) {
        self.status.initialize();
        let Some(receiver) = self
            .receiver
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        else {
            return;
        };
        loop {
            if stop.load(Ordering::Acquire) {
                while let Ok(job) = receiver.try_recv() {
                    let _ = job.response.try_send(refusal("runtime_stopping"));
                }
                break;
            }
            let job = match receiver.recv_timeout(Duration::from_millis(20)) {
                Ok(job) => job,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.execute(&job.request, job.deadline)
            }));
            let response = match result {
                Ok(value) => value,
                Err(_) => {
                    self.faulted.store(true, Ordering::Release);
                    unknown("native_coordinator_panicked")
                }
            };
            let _ = job.response.try_send(response);
        }
    }
    fn dispatch(&self, task: Task, deadline: Instant) -> Outcome<Result<Reply, String>> {
        match self.queue.submit(task, deadline) {
            Ok(pending) => {
                // A false acknowledgment is never authority to cancel a task
                // that may already have been claimed. Owned observation waits
                // through final callback settlement even after shutdown.
                let _ = self.wake.request();
                let outcome = pending.wait_settled();
                self.wake.settled();
                outcome
            }
            Err(_) => Outcome::NotExecuted(fusion_native_dispatch::NotExecutedReason::Shutdown),
        }
    }
    fn execute(&self, request: &Arc<PreparedRequest>, deadline: Instant) -> Response {
        if self.faulted.load(Ordering::Acquire) {
            return refusal("native_coordinator_faulted");
        }
        if Instant::now() >= deadline {
            return refusal("operation_deadline_before_admission");
        }
        if !request.is_write() {
            return match self.dispatch(Task::Inspect(Arc::clone(request)), deadline) {
                Outcome::Completed(Ok(Reply::Value(value))) => (
                    if value["outcome"] == "unknown" {
                        503
                    } else if value["success"] == true {
                        200
                    } else {
                        400
                    },
                    value,
                ),
                Outcome::NotExecuted(_) => refusal("dispatch_not_executed"),
                _ => unknown("native_inspection_unavailable"),
            };
        }
        let journal = match self.status.journal() {
            Ok(j) => j,
            Err(code) => return refusal(code),
        };
        let at = match now_ms() {
            Ok(n) => n,
            Err(_) => return refusal("operation_clock_unavailable"),
        };
        let admitted = match journal.admit(request, at) {
            Ok(Admission::Created(token)) => token,
            Ok(Admission::Existing(_)) => return self.status_response(request, true),
            Err(error) => {
                return (
                    if error.code == "operation_conflict" {
                        409
                    } else {
                        503
                    },
                    json!({"success":false,"errorCode":error.code,"reconciliationRequired":error.reconciliation_required,"nativeExecutionStarted":false}),
                )
            }
        };
        let dispatch_id = format!(
            "native:{}",
            fusion_mechanical_journal::request_digest(request.canonical_bytes())
        );
        let dispatched = match now_ms().and_then(|at| journal.dispatch(admitted, &dispatch_id, at))
        {
            Ok(token) => token,
            Err(_) => return self.status_response(request, false),
        };
        let preflight_task = if document_lifecycle::is_lifecycle(request.operation()) {
            let proof = if request.operation() == "native_scratch_discard" {
                let proof = creation_proof(&journal, request);
                match proof {
                    Ok(p) => Some(p),
                    Err(code) => {
                        let _=journal.not_executed(dispatched,json!({"success":false,"errorCode":code,"nativeExecutionStarted":false}),now_ms());
                        return self.status_response(request, false);
                    }
                }
            } else {
                None
            };
            Task::LifecyclePreflight(Arc::clone(request), proof)
        } else if request.operation() == "bom_export" {
            Task::BomExportPreflight(Arc::clone(request))
        } else if request.operation() == "sketch_create" {
            Task::SketchPreflight(Arc::clone(request))
        } else if request.operation() == "sketch_geometry" {
            Task::GeometryPreflight(Arc::clone(request))
        } else if crate::assembly::is_write(request.operation()) {
            Task::AssemblyPreflight(Arc::clone(request))
        } else if request.operation() == "sketch_dimension" {
            Task::DimensionPreflight(Arc::clone(request))
        } else if request.operation() == "sketch_constraint" {
            Task::RelationPreflight(Arc::clone(request))
        } else if request.operation() == "feature_create" {
            Task::ExtrudePreflight(Arc::clone(request))
        } else {
            Task::Preflight(Arc::clone(request))
        };
        let preflight = self.dispatch(preflight_task, deadline);
        let plan = match preflight {
            Outcome::Completed(Ok(Reply::Plan(plan))) => ExecutionPlan::Mechanical(plan),
            Outcome::Completed(Ok(Reply::LifecyclePlan(plan))) => ExecutionPlan::Lifecycle(plan),
            Outcome::Completed(Ok(Reply::BomExportPlan(plan))) => ExecutionPlan::BomExport(plan),
            Outcome::Completed(Ok(Reply::SketchPlan(plan))) => ExecutionPlan::Sketch(plan),
            Outcome::Completed(Ok(Reply::GeometryPlan(plan))) => ExecutionPlan::Geometry(plan),
            Outcome::Completed(Ok(Reply::AssemblyPlan(plan))) => ExecutionPlan::Assembly(plan),
            Outcome::Completed(Ok(Reply::DimensionPlan(plan))) => ExecutionPlan::Dimension(plan),
            Outcome::Completed(Ok(Reply::RelationPlan(plan))) => ExecutionPlan::Relation(plan),
            Outcome::Completed(Ok(Reply::ExtrudePlan(plan))) => ExecutionPlan::Extrude(plan),
            other => {
                // Preflight is strictly read-only, even if its observer failed.
                let body = match other {
                    Outcome::Completed(Ok(Reply::Value(mut body))) => {
                        body["nativeExecutionStarted"] = json!(false);
                        body["success"] = json!(false);
                        body
                    }
                    _ => {
                        json!({"success":false,"errorCode":"native_preflight_not_completed","nativeExecutionStarted":false})
                    }
                };
                let _ = journal.not_executed(dispatched, body, now_ms());
                return self.status_response(request, false);
            }
        };
        if Instant::now() >= deadline {
            let _=journal.not_executed(dispatched,json!({"success":false,"errorCode":"operation_deadline_before_execution","nativeExecutionStarted":false}),now_ms());
            return self.status_response(request, false);
        }
        // Observed document is honestly the preflight observation. The SDK
        // CAD execute repeats membership+revision checks after this I/O. BOM
        // export instead publishes the immutable checked snapshot with its own
        // provenance; it never promises to lock the model until file commit.
        let permit =
            match now_ms().and_then(|at| journal.start(dispatched, plan.observed_document(), at)) {
                Ok(permit) => permit,
                Err(_) => return self.status_response(request, false),
            };
        let outcome = match plan {
            ExecutionPlan::Assembly(plan) => {
                self.dispatch(Task::AssemblyExecute(Some(plan)), deadline)
            }
            ExecutionPlan::Dimension(plan) => {
                self.dispatch(Task::DimensionExecute(Some(plan)), deadline)
            }
            ExecutionPlan::Relation(plan) => {
                self.dispatch(Task::RelationExecute(Some(plan)), deadline)
            }
            ExecutionPlan::Extrude(plan) => {
                self.dispatch(Task::ExtrudeExecute(Some(plan)), deadline)
            }
            ExecutionPlan::Mechanical(plan) => self.dispatch(Task::Execute(Some(plan)), deadline),
            ExecutionPlan::Sketch(plan) => self.dispatch(Task::SketchExecute(Some(plan)), deadline),
            ExecutionPlan::Geometry(plan) => {
                self.dispatch(Task::GeometryExecute(Some(plan)), deadline)
            }
            ExecutionPlan::Lifecycle(plan) => {
                self.dispatch(Task::LifecycleExecute(Some(plan)), deadline)
            }
            ExecutionPlan::BomExport(plan) => {
                Outcome::Completed(Ok(Reply::Value(if Instant::now() >= deadline {
                    json!({"success":false,"errorCode":"operation_deadline_before_publication","nativeExecutionStarted":false})
                } else {
                    plan.execute()
                })))
            }
        };
        let (body, uncertain) = match outcome {
            Outcome::Completed(Ok(Reply::Value(body))) => {
                let uncertain = body["outcome"] == "unknown"
                    || body["reconciliationRequired"] == true
                    || !body["success"].is_boolean();
                (body, uncertain)
            }
            Outcome::NotExecuted(_) => (
                json!({"success":false,"errorCode":"dispatch_not_executed","nativeExecutionStarted":false}),
                false,
            ),
            _ => (
                json!({"success":false,"errorCode":"native_execution_unobserved","outcome":"unknown","reconciliationRequired":true,"nativeExecutionStarted":null}),
                true,
            ),
        };
        // This coordinator outlives the HTTP observer. An actual late result is
        // retained before clock/persistence failure, never lost with the socket.
        let _completion = if uncertain {
            journal.finish_unknown(permit, body)
        } else {
            journal.finish(permit, body, now_ms())
        };
        self.status_response(request, false)
    }
    fn status_response(&self, request: &PreparedRequest, replayed: bool) -> Response {
        let session = request.arguments()["sessionId"].as_str().unwrap();
        let operation = request.arguments()["operationId"].as_str().unwrap();
        let (_, mut status) =
            self.status
                .query(session, operation, Instant::now() + Duration::from_secs(2));
        status["effectJournalConnected"] = json!(true);
        let durable = status["authority"] == "durable_terminal";
        let mut body = if durable {
            status["durable"]["record"]["result"].clone()
        } else {
            status["observed"]["result"].clone()
        };
        if !body.is_object() {
            body = json!({"success":false,"outcome":"unknown","reconciliationRequired":true});
        }
        let code = if durable {
            if body["success"] == true {
                200
            } else {
                400
            }
        } else {
            503
        };
        if !durable {
            body["success"] = json!(false);
            body["reconciliationRequired"] = json!(true);
        }
        body["operationStatus"] = status;
        body["replayed"] = json!(replayed);
        body["sessionId"] = json!(session);
        body["operationId"] = json!(operation);
        (code, body)
    }
}
fn creation_proof(
    journal: &fusion_mechanical_journal::retained::RetainedJournal,
    request: &PreparedRequest,
) -> Result<CreationProof, &'static str> {
    let args = request.arguments();
    let operation = args["creationOperationId"]
        .as_str()
        .filter(|id| crate::http::status_selector(id))
        .ok_or("invalid_creation_operation_selector")?;
    let session = args["sessionId"].as_str().ok_or("session_mismatch")?;
    let status = journal
        .status(session, operation)
        .map_err(|_| "creation_history_unavailable")?;
    if status.historical
        || status.reconciliation_required
        || status.authority != fusion_mechanical_journal::retained::Authority::DurableTerminal
    {
        return Err("durable_creation_receipt_required");
    }
    let record = status
        .durable
        .and_then(|s| s.record)
        .ok_or("creation_record_missing")?;
    if record.phase != fusion_mechanical_journal::Phase::Succeeded
        || record.operation != "new_design"
        || record.session_id != session
    {
        return Err("creation_record_not_successful");
    }
    let result = record.result.as_ref().ok_or("creation_result_missing")?;
    if result["success"] != true || result["data"]["ownedScratch"] != true {
        return Err("creation_result_not_owned_scratch");
    }
    let document_id = result["data"]["target"]["documentId"]
        .as_str()
        .ok_or("creation_target_missing")?
        .to_owned();
    if args["target"]["documentId"].as_str() != Some(&document_id) {
        return Err("creation_target_mismatch");
    }
    Ok(CreationProof {
        session: session.into(),
        operation_id: operation.into(),
        request_digest: record.request_digest,
        document_id,
    })
}
#[cfg(test)]
#[path = "execution_tests.rs"]
mod tests;
