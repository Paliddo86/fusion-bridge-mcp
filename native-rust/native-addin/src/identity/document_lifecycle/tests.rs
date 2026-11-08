use super::*;
use crate::mechanics::{DesignState, Target};
use fusion_mechanical_core::prepare_request;
use std::collections::{BTreeMap, BTreeSet};

const EPOCH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
#[derive(Clone)]
pub(crate) struct Fake(pub(crate) Rc<RefCell<World>>);
pub(crate) struct World {
    order: Vec<u64>,
    nodes: BTreeMap<u64, Node>,
    next: u64,
    lease: u64,
    leases: BTreeSet<u64>,
    pub(crate) creates: usize,
    pub(crate) closes: usize,
    pub(crate) activations: usize,
    parametric: usize,
    command: String,
    create_loss: bool,
    return_old: bool,
    post_read_failure: bool,
    pub(crate) close_loss: bool,
    close_stays_open: bool,
    activation_wrong: bool,
}
struct Node {
    document: Document,
    valid: bool,
    profile: ScratchProfile,
    revision: String,
}
pub(crate) struct Handle {
    object: u64,
    lease: u64,
}
impl Fake {
    pub(crate) fn new() -> Self {
        let mut nodes = BTreeMap::new();
        for id in [1, 2] {
            nodes.insert(
                id,
                Node {
                    document: Document {
                        document_id: None,
                        name: "Untitled".into(),
                        creation_id: "copied-metadata".into(),
                        active: id == 1,
                        saved: false,
                        modified: false,
                    },
                    valid: true,
                    profile: blank(),
                    revision: "initial".into(),
                },
            );
        }
        Self(Rc::new(RefCell::new(World {
            order: vec![1, 2],
            nodes,
            next: 3,
            lease: 0,
            leases: BTreeSet::new(),
            creates: 0,
            closes: 0,
            activations: 0,
            parametric: 0,
            command: "SelectCommand".into(),
            create_loss: false,
            return_old: false,
            post_read_failure: false,
            close_loss: false,
            close_stays_open: false,
            activation_wrong: false,
        })))
    }
    fn lease(&self, object: u64) -> Handle {
        let mut w = self.0.borrow_mut();
        w.lease += 1;
        let lease = w.lease;
        assert!(w.leases.insert(lease));
        Handle { object, lease }
    }
    fn created(&self) -> u64 {
        self.0.borrow().next - 1
    }
}
fn blank() -> ScratchProfile {
    ScratchProfile {
        parametric: 1,
        hybrid: 1,
        components: 1,
        ..Default::default()
    }
}
impl DocumentSdk for Fake {
    type Handle = Handle;
    fn version(&self) -> Result<String, String> {
        Ok("instrumented".into())
    }
    fn thread_id(&self) -> u32 {
        1
    }
    fn count(&self) -> Result<usize, String> {
        Ok(self.0.borrow().order.len())
    }
    fn retain(&self, index: usize) -> Result<Handle, String> {
        let id = *self.0.borrow().order.get(index).ok_or("missing index")?;
        Ok(self.lease(id))
    }
    fn read(&self, h: &Handle) -> Result<Document, String> {
        let w = self.0.borrow();
        if w.post_read_failure && h.object > 2 {
            return Err("injected read failure".into());
        }
        Ok(w.nodes
            .get(&h.object)
            .ok_or("missing document")?
            .document
            .clone())
    }
    fn same(&self, a: &Handle, b: &Handle) -> Result<bool, String> {
        Ok(a.object == b.object)
    }
    fn is_valid(&self, h: &Handle) -> Result<bool, String> {
        Ok(self
            .0
            .borrow()
            .nodes
            .get(&h.object)
            .is_some_and(|n| n.valid))
    }
    fn release(&self, h: Handle) -> Result<(), String> {
        assert!(
            self.0.borrow_mut().leases.remove(&h.lease),
            "release attempted twice"
        );
        Ok(())
    }
}
impl LifecycleSdk for Fake {
    fn active_command(&self) -> Result<String, String> {
        Ok(self.0.borrow().command.clone())
    }
    fn create_blank(&self) -> CreationAttempt<Handle> {
        let (id, loss) = {
            let mut w = self.0.borrow_mut();
            w.creates += 1;
            if w.return_old {
                (1, false)
            } else {
                let id = w.next;
                w.next += 1;
                for n in w.nodes.values_mut() {
                    n.document.active = false;
                }
                w.nodes.insert(
                    id,
                    Node {
                        document: Document {
                            document_id: None,
                            name: "Untitled".into(),
                            creation_id: "copied-metadata".into(),
                            active: true,
                            saved: false,
                            modified: false,
                        },
                        valid: true,
                        profile: blank(),
                        revision: format!("revision-{id}"),
                    },
                );
                w.order.push(id);
                (id, w.create_loss)
            }
        };
        CreationAttempt {
            handle: Some(self.lease(id)),
            invoked: true,
            acknowledgement: if loss {
                Err("lost creation ack".into())
            } else {
                Ok(())
            },
        }
    }
    fn make_parametric(&self, _: &Handle) -> EffectAttempt {
        self.0.borrow_mut().parametric += 1;
        EffectAttempt {
            invoked: true,
            acknowledgement: Ok(true),
        }
    }
    fn scratch_profile(&self, h: &Handle) -> Result<ScratchProfile, String> {
        Ok(self.0.borrow().nodes.get(&h.object).unwrap().profile)
    }
    fn has_data_file(&self, h: &Handle) -> Result<bool, String> {
        Ok(self.0.borrow().nodes.get(&h.object).unwrap().document.saved)
    }
    fn inspect_mechanical(&self, h: &Handle, id: &str) -> Result<Inspection, String> {
        let w = self.0.borrow();
        let n = w.nodes.get(&h.object).unwrap();
        Ok(Inspection {
            target: Target {
                document_id: id.into(),
                revision: n.revision.clone(),
            },
            revision_scope: crate::mechanics::REVISION_SCOPE,
            complete: false,
            scope: "instrumented parameters",
            design: DesignState {
                active: n.document.active,
                parametric: true,
                root_name: "fixture".into(),
                root_revision: n.revision.clone(),
                default_length_units: "mm".into(),
                component_count: 1,
                body_count: n.profile.bodies,
                sketch_count: 0,
                timeline_count: 0,
                timeline_position: 0,
            },
            parameters: vec![],
            health: vec![],
        })
    }
    fn discard_document(&self, h: &Handle) -> EffectAttempt {
        let mut w = self.0.borrow_mut();
        w.closes += 1;
        if !w.close_stays_open {
            w.nodes.get_mut(&h.object).unwrap().valid = false;
            w.order.retain(|id| *id != h.object);
            w.nodes.get_mut(&1).unwrap().document.active = true;
        }
        EffectAttempt {
            invoked: true,
            acknowledgement: if w.close_loss {
                Err("lost close ack".into())
            } else {
                Ok(true)
            },
        }
    }
    fn activate_document(&self, h: &Handle) -> EffectAttempt {
        let mut w = self.0.borrow_mut();
        w.activations += 1;
        let id = if w.activation_wrong { 1 } else { h.object };
        for (key, n) in &mut w.nodes {
            n.document.active = *key == id;
        }
        EffectAttempt {
            invoked: true,
            acknowledgement: Ok(true),
        }
    }
}
pub(crate) fn create_request(epoch: &str, operation: &str) -> PreparedRequest {
    prepare_request(json!({"op":"new_design","sessionId":epoch,"operationId":operation,"name":"requested only"}),epoch).unwrap()
}
fn setup() -> (Registry<Fake>, Fake) {
    let fake = Fake::new();
    let registry = Registry::new(EPOCH.into(), fake.clone()).unwrap();
    (registry, fake)
}
fn create(registry: &Registry<Fake>, operation: &str) -> (Value, CreationProof) {
    let request = create_request(EPOCH, operation);
    let plan = registry.lifecycle_preflight(&request, None).unwrap();
    let value = registry.lifecycle_execute(plan);
    assert_eq!(value["success"], true, "{value}");
    let proof = CreationProof {
        session: EPOCH.into(),
        operation_id: operation.into(),
        request_digest: fusion_mechanical_journal::request_digest(request.canonical_bytes()),
        document_id: value["data"]["target"]["documentId"]
            .as_str()
            .unwrap()
            .into(),
    };
    (value, proof)
}
fn discard_request(value: &Value, proof: &CreationProof) -> PreparedRequest {
    prepare_request(json!({"op":"native_scratch_discard","sessionId":EPOCH,"operationId":"discard","creationOperationId":proof.operation_id,"target":value["data"]["target"]}),EPOCH).unwrap()
}

#[test]
fn exact_create_and_discard_ignore_copied_metadata_and_preserve_originals() {
    let (registry, fake) = setup();
    let before = registry.refresh().unwrap();
    let (value, proof) = create(&registry, "create");
    assert!(
        value["data"]["target"]["documentId"].as_str().unwrap()
            != before.documents[0].document_id.as_ref().unwrap()
    );
    assert_eq!(value["data"]["document"]["name"], "Untitled");
    assert_eq!(value["data"]["requestedName"], "requested only");
    fake.0.borrow_mut().order.reverse(); // Order is not identity.
    let plan = registry
        .lifecycle_preflight(&discard_request(&value, &proof), Some(proof))
        .unwrap();
    let closed = registry.lifecycle_execute(plan);
    assert_eq!(closed["success"], true, "{closed}");
    assert!(fake.0.borrow().nodes[&1].valid && fake.0.borrow().nodes[&2].valid);
    assert_eq!(fake.0.borrow().closes, 1);
    registry.close().unwrap();
    assert!(fake.0.borrow().leases.is_empty());
}
#[test]
fn existing_document_return_cannot_be_adopted_or_modified() {
    let (registry, fake) = setup();
    fake.0.borrow_mut().return_old = true;
    let plan = registry
        .lifecycle_preflight(&create_request(EPOCH, "create"), None)
        .unwrap();
    let result = registry.lifecycle_execute(plan);
    assert_eq!(result["outcome"], "unknown");
    assert_eq!(fake.0.borrow().parametric, 0);
    assert!(registry.is_faulted());
    registry.close().unwrap();
    assert!(fake.0.borrow().leases.is_empty());
}
#[test]
fn lost_create_ack_or_post_create_read_failure_never_replays_or_auto_closes() {
    for lost_ack in [true, false] {
        let (registry, fake) = setup();
        let request = create_request(EPOCH, "create");
        let plan = registry.lifecycle_preflight(&request, None).unwrap();
        if lost_ack {
            fake.0.borrow_mut().create_loss = true;
        } else {
            fake.0.borrow_mut().post_read_failure = true;
        }
        let result = registry.lifecycle_execute(plan);
        assert_eq!(result["outcome"], "unknown");
        assert!(registry.lifecycle_preflight(&request, None).is_err());
        assert_eq!(fake.0.borrow().creates, 1);
        assert_eq!(fake.0.borrow().closes, 0);
        assert!(!fake.0.borrow().leases.is_empty());
        registry.close().unwrap();
        assert!(fake.0.borrow().leases.is_empty());
    }
}
#[test]
fn discard_requires_live_owner_and_matching_current_session_proof() {
    let (registry, fake) = setup();
    let (value, proof) = create(&registry, "create");
    let request = discard_request(&value, &proof);
    assert!(registry.lifecycle_preflight(&request, None).is_err());
    for change in 0..4 {
        let mut bad = proof.clone();
        match change {
            0 => bad.session = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            1 => bad.document_id = "other".into(),
            2 => bad.request_digest = "wrong".into(),
            _ => bad.operation_id = "other".into(),
        };
        assert!(registry.lifecycle_preflight(&request, Some(bad)).is_err());
    }
    assert_eq!(fake.0.borrow().closes, 0);
    registry.close().unwrap();
}
#[test]
fn renamed_saved_geometry_and_stale_revision_refuse_before_close() {
    for change in 0..4 {
        let (registry, fake) = setup();
        let (value, proof) = create(&registry, "create");
        let id = fake.created();
        {
            let mut w = fake.0.borrow_mut();
            let n = w.nodes.get_mut(&id).unwrap();
            match change {
                0 => n.document.name = "user work".into(),
                1 => n.document.saved = true,
                2 => n.profile.bodies = 1,
                _ => n.revision = "changed".into(),
            }
        }
        assert!(registry
            .lifecycle_preflight(&discard_request(&value, &proof), Some(proof))
            .is_err());
        assert_eq!(fake.0.borrow().closes, 0);
        registry.close().unwrap();
    }
}
#[test]
fn second_hop_rechecks_baseline_command_and_parameter_revision() {
    for change in 0..3 {
        let (registry, fake) = setup();
        let (value, proof) = create(&registry, "create");
        let plan = registry
            .lifecycle_preflight(&discard_request(&value, &proof), Some(proof))
            .unwrap();
        let id = fake.created();
        {
            let mut w = fake.0.borrow_mut();
            match change {
                0 => w.nodes.get_mut(&1).unwrap().document.modified = true,
                1 => w.command = "SketchCommand".into(),
                _ => w.nodes.get_mut(&id).unwrap().revision = "new expression".into(),
            }
        }
        let result = registry.lifecycle_execute(plan);
        assert_eq!(result["nativeExecutionStarted"], false);
        assert_eq!(fake.0.borrow().closes, 0);
        registry.close().unwrap();
    }
}
#[test]
fn uncertain_close_is_spent_even_when_document_disappeared() {
    for stays in [true, false] {
        let (registry, fake) = setup();
        let (value, proof) = create(&registry, "create");
        let request = discard_request(&value, &proof);
        let plan = registry
            .lifecycle_preflight(&request, Some(proof.clone()))
            .unwrap();
        {
            let mut w = fake.0.borrow_mut();
            w.close_loss = true;
            w.close_stays_open = stays;
        }
        let result = registry.lifecycle_execute(plan);
        assert_eq!(result["outcome"], "unknown");
        assert!(registry.lifecycle_preflight(&request, Some(proof)).is_err());
        assert_eq!(fake.0.borrow().closes, 1);
        registry.close().unwrap();
    }
}
#[test]
fn activation_uses_exact_id_and_requires_exact_active_readback() {
    for wrong in [false, true] {
        let (registry, fake) = setup();
        let snapshot = registry.refresh().unwrap();
        let id = snapshot.documents[1].document_id.clone().unwrap();
        let request=prepare_request(json!({"op":"native_document_activate","sessionId":EPOCH,"operationId":"activate","target":{"documentId":id},"expectedActiveDocumentId":snapshot.documents[0].document_id}),EPOCH).unwrap();
        let plan = registry.lifecycle_preflight(&request, None).unwrap();
        fake.0.borrow_mut().activation_wrong = wrong;
        let result = registry.lifecycle_execute(plan);
        assert_eq!(result["success"], !wrong);
        assert_eq!(fake.0.borrow().activations, 1);
        registry.close().unwrap();
    }
}
#[test]
fn bounded_owned_slots_and_serial_exhaustion_precede_creation() {
    let (registry, fake) = setup();
    for i in 0..MAX_OWNED_SCRATCH {
        create(&registry, &format!("create-{i}"));
    }
    assert!(registry
        .lifecycle_preflight(&create_request(EPOCH, "one-too-many"), None)
        .is_err());
    assert_eq!(fake.0.borrow().creates, MAX_OWNED_SCRATCH);
    registry.close().unwrap();
    let (registry, fake) = setup();
    registry.state.borrow_mut().next = u64::MAX;
    assert!(registry
        .lifecycle_preflight(&create_request(EPOCH, "overflow"), None)
        .is_err());
    assert_eq!(fake.0.borrow().creates, 0);
    registry.close().unwrap();
}
#[test]
fn unsupported_box_refuses_before_any_sdk_effect() {
    let (registry, fake) = setup();
    let request=prepare_request(json!({"op":"new_design","sessionId":EPOCH,"operationId":"box","box":{"width":1,"height":2,"depth":3}}),EPOCH).unwrap();
    assert!(supported_arguments(&request).is_err());
    assert!(registry.lifecycle_preflight(&request, None).is_err());
    assert_eq!(fake.0.borrow().creates, 0);
    registry.close().unwrap();
}

#[test]
fn externally_replaced_same_metadata_document_never_inherits_scratch_ownership() {
    let (registry, fake) = setup();
    let (value, proof) = create(&registry, "create");
    let old = fake.created();
    {
        let mut w = fake.0.borrow_mut();
        let document = w.nodes[&old].document.clone();
        w.nodes.get_mut(&old).unwrap().valid = false;
        w.order.retain(|id| *id != old);
        let id = w.next;
        w.next += 1;
        w.nodes.insert(
            id,
            Node {
                document,
                valid: true,
                profile: blank(),
                revision: "replacement".into(),
            },
        );
        w.order.push(id);
    }
    assert!(registry
        .lifecycle_preflight(&discard_request(&value, &proof), Some(proof))
        .is_err());
    assert_eq!(fake.0.borrow().closes, 0);
    registry.close().unwrap();
    assert!(fake.0.borrow().leases.is_empty());
}
#[test]
fn document_member_capacity_refuses_before_create_and_stop_releases_only_leases() {
    let (registry, fake) = setup();
    {
        let mut w = fake.0.borrow_mut();
        for id in 3..=MAX_DOCUMENTS as u64 {
            w.nodes.insert(
                id,
                Node {
                    document: Document {
                        document_id: None,
                        name: format!("Untitled {id}"),
                        creation_id: format!("creation {id}"),
                        active: false,
                        saved: false,
                        modified: false,
                    },
                    valid: true,
                    profile: blank(),
                    revision: "initial".into(),
                },
            );
            w.order.push(id);
        }
    }
    assert!(registry
        .lifecycle_preflight(&create_request(EPOCH, "full"), None)
        .is_err());
    assert_eq!(fake.0.borrow().creates, 0);
    registry.close().unwrap();
    assert_eq!(fake.0.borrow().closes, 0);
    assert_eq!(fake.0.borrow().order.len(), MAX_DOCUMENTS);
    assert!(fake.0.borrow().leases.is_empty());
}
