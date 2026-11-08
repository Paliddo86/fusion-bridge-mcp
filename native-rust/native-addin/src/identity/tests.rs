use super::*;
use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicUsize, Ordering},
};

#[derive(Clone)]
struct Fake(Rc<Facts>);
#[derive(Default)]
struct Facts {
    rows: RefCell<Vec<(u64, String, String)>>,
    leases: RefCell<HashMap<u64, u64>>,
    invalid: RefCell<HashSet<u64>>,
    next: Cell<u64>,
    retain_calls: Cell<usize>,
    sdk_calls: Cell<usize>,
    fail_retain: Cell<Option<usize>>,
    fail_release: Cell<bool>,
    hook: RefCell<Option<Box<dyn Fn()>>>,
    releases: Cell<usize>,
    backend_drops: AtomicUsize,
    child_leases: crate::lifecycle::ChildLeaseFence,
}
struct Handle {
    lease: u64,
    object: u64,
}
impl Drop for Fake {
    fn drop(&mut self) {
        self.0.backend_drops.fetch_add(1, Ordering::Relaxed);
    }
}
impl Fake {
    fn new(objects: &[u64]) -> Self {
        let value = Self(Rc::new(Facts::default()));
        value.rows(objects);
        value
    }
    fn rows(&self, objects: &[u64]) {
        *self.0.rows.borrow_mut() = objects
            .iter()
            .map(|&id| (id, format!("name {id}"), "copied-creation".into()))
            .collect();
    }
}
impl DocumentSdk for Fake {
    fn prepare_release(&self) -> Result<(), String> {
        self.0.sdk_calls.set(self.0.sdk_calls.get() + 1);
        self.0.child_leases.ensure_clear()
    }
    type Handle = Handle;
    fn version(&self) -> Result<String, String> {
        self.0.sdk_calls.set(self.0.sdk_calls.get() + 1);
        Ok("test".into())
    }
    fn thread_id(&self) -> u32 {
        self.0.sdk_calls.set(self.0.sdk_calls.get() + 1);
        9
    }
    fn count(&self) -> Result<usize, String> {
        self.0.sdk_calls.set(self.0.sdk_calls.get() + 1);
        Ok(self.0.rows.borrow().len())
    }
    fn retain(&self, index: usize) -> Result<Handle, String> {
        self.0.sdk_calls.set(self.0.sdk_calls.get() + 1);
        let call = self.0.retain_calls.get() + 1;
        self.0.retain_calls.set(call);
        if self.0.fail_retain.get() == Some(call) {
            return Err("retain failed".into());
        }
        let object = self.0.rows.borrow()[index].0;
        let lease = self.0.next.get() + 1;
        self.0.next.set(lease);
        assert!(self.0.leases.borrow_mut().insert(lease, object).is_none());
        Ok(Handle { lease, object })
    }
    fn read(&self, handle: &Handle) -> Result<Document, String> {
        self.0.sdk_calls.set(self.0.sdk_calls.get() + 1);
        let hook = self.0.hook.borrow_mut().take();
        if let Some(hook) = hook {
            hook();
        }
        let rows = self.0.rows.borrow();
        let row = rows
            .iter()
            .find(|row| row.0 == handle.object)
            .ok_or("removed")?;
        Ok(Document {
            document_id: Some("backend-forgery".into()),
            name: row.1.clone(),
            creation_id: row.2.clone(),
            active: false,
            saved: true,
            modified: false,
        })
    }
    fn same(&self, a: &Handle, b: &Handle) -> Result<bool, String> {
        self.0.sdk_calls.set(self.0.sdk_calls.get() + 1);
        Ok(a.object == b.object)
    }
    fn is_valid(&self, handle: &Handle) -> Result<bool, String> {
        self.0.sdk_calls.set(self.0.sdk_calls.get() + 1);
        Ok(!self.0.invalid.borrow().contains(&handle.lease))
    }
    fn release(&self, handle: Handle) -> Result<(), String> {
        self.0.sdk_calls.set(self.0.sdk_calls.get() + 1);
        assert_eq!(
            self.0.leases.borrow_mut().remove(&handle.lease),
            Some(handle.object)
        );
        self.0.releases.set(self.0.releases.get() + 1);
        if self.0.fail_release.replace(false) {
            Err("release uncertain".into())
        } else {
            Ok(())
        }
    }
}
fn registry(fake: &Fake) -> Registry<Fake> {
    Registry::new("0123456789abcdef0123456789abcdef".into(), fake.clone()).unwrap()
}
fn ids(snapshot: Snapshot) -> Vec<String> {
    snapshot
        .documents
        .into_iter()
        .map(|d| d.document_id.unwrap())
        .collect()
}

#[test]
fn uncertain_child_release_retains_parent_documents_and_backend_without_retry() {
    let fake = Fake::new(&[1, 2]);
    let mut registry = registry(&fake);
    registry.refresh().unwrap();
    let leases = fake.0.leases.borrow().clone();
    let released = fake.0.releases.get();
    let drops = fake.0.backend_drops.load(Ordering::Relaxed);
    let close_calls = Cell::new(0);
    let child = fake.0.child_leases.begin().unwrap();
    assert!(child
        .run(
            (),
            |_| (),
            |_| {
                close_calls.set(close_calls.get() + 1);
                Err("child native delete uncertain".into())
            }
        )
        .is_err());
    assert!(registry.close().is_err());
    assert!(registry.close().is_err());
    assert!(registry.released_backend().is_err());
    assert_eq!(fake.0.releases.get(), released);
    assert_eq!(*fake.0.leases.borrow(), leases);
    assert_eq!(close_calls.get(), 1);
    drop(registry);
    assert_eq!(fake.0.backend_drops.load(Ordering::Relaxed), drops);
    assert_eq!(*fake.0.leases.borrow(), leases);
}

#[test]
fn exact_objects_keep_ids_across_rename_reorder_and_copies_stay_distinct() {
    let fake = Fake::new(&[1, 2]);
    let registry = registry(&fake);
    let first = registry.refresh().unwrap();
    assert!(!first.creation_ids_unique);
    let first = ids(first);
    assert_ne!(first[0], first[1]);
    fake.rows(&[2, 1]);
    fake.0.rows.borrow_mut()[1].1 = "renamed".into();
    assert_eq!(
        ids(registry.refresh().unwrap()),
        [first[1].clone(), first[0].clone()]
    );
    assert_eq!(registry.resolve(&first[0]).unwrap().name, "renamed");
    registry.close().unwrap();
    assert!(fake.0.leases.borrow().is_empty());
}
#[test]
fn removed_but_valid_object_retires_id_and_reappearance_never_resurrects_it() {
    let fake = Fake::new(&[1]);
    let registry = registry(&fake);
    let first = ids(registry.refresh().unwrap()).remove(0);
    fake.rows(&[]);
    assert!(registry
        .resolve(&first)
        .unwrap_err()
        .contains("not a current member"));
    fake.rows(&[1]);
    assert_ne!(ids(registry.refresh().unwrap())[0], first);
    assert!(registry.resolve(&first).is_err());
    registry.close().unwrap();
}
#[test]
fn invalid_old_reference_cannot_recover_its_id() {
    let fake = Fake::new(&[1]);
    let registry = registry(&fake);
    let first = ids(registry.refresh().unwrap()).remove(0);
    fake.0
        .invalid
        .borrow_mut()
        .extend(fake.0.leases.borrow().keys());
    assert_ne!(ids(registry.refresh().unwrap())[0], first);
    fake.0.invalid.borrow_mut().clear();
    assert!(registry.resolve(&first).is_err());
    registry.close().unwrap();
}
#[test]
fn partial_fault_permanently_refuses_reads_and_releases_all_leases() {
    let fake = Fake::new(&[1, 2]);
    let registry = registry(&fake);
    let first = ids(registry.refresh().unwrap());
    fake.0.fail_retain.set(Some(fake.0.retain_calls.get() + 2));
    assert!(registry.refresh().is_err());
    fake.0.fail_retain.set(None);
    assert!(registry.refresh().is_err());
    assert!(registry.resolve(&first[0]).is_err());
    registry.close().unwrap();
    assert!(fake.0.leases.borrow().is_empty());
}
#[test]
fn duplicate_sdk_object_and_same_count_replacement_refuse_publication() {
    let fake = Fake::new(&[1, 1]);
    let registry = registry(&fake);
    assert!(registry.refresh().unwrap_err().contains("duplicate SDK"));
    registry.close().unwrap();
    let fake = Fake::new(&[1, 2]);
    let registry = super::tests::registry(&fake);
    let changed = fake.clone();
    *fake.0.hook.borrow_mut() = Some(Box::new(move || {
        let second = changed.clone();
        *changed.0.hook.borrow_mut() = Some(Box::new(move || second.rows(&[2, 1])));
    }));
    assert!(registry
        .refresh()
        .unwrap_err()
        .contains("membership changed"));
    registry.close().unwrap();
}
#[test]
fn registry_reentrancy_faults_outer_operation_without_panic() {
    let fake = Fake::new(&[1]);
    let registry = Rc::new(registry(&fake));
    let weak = Rc::downgrade(&registry);
    *fake.0.hook.borrow_mut() = Some(Box::new(move || {
        assert!(weak
            .upgrade()
            .unwrap()
            .refresh()
            .unwrap_err()
            .contains("reentrancy"));
    }));
    assert!(registry.refresh().is_err());
    assert!(registry.refresh().is_err());
    registry.close().unwrap();
}
#[test]
fn panic_keeps_acquired_reference_for_checked_cleanup_and_faults_registry() {
    let fake = Fake::new(&[1]);
    let registry = registry(&fake);
    *fake.0.hook.borrow_mut() = Some(Box::new(|| panic!("injected getter panic")));
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| registry.refresh())).is_err());
    assert!(registry.refresh().is_err());
    registry.close().unwrap();
    assert!(fake.0.leases.borrow().is_empty());
}
#[test]
fn cleanup_consumes_all_references_after_first_error_and_never_retries() {
    let fake = Fake::new(&[1, 2]);
    let mut registry = registry(&fake);
    registry.refresh().unwrap();
    fake.0.fail_release.set(true);
    assert!(registry.close().is_err());
    assert!(fake.0.leases.borrow().is_empty());
    let calls = fake.0.releases.get();
    assert!(registry.close().is_err());
    assert_eq!(fake.0.releases.get(), calls);
    assert!(registry.released_backend().is_err());
}
#[test]
fn epoch_mismatch_malformed_ids_and_serial_exhaustion_never_retarget() {
    let fake = Fake::new(&[1]);
    let registry = registry(&fake);
    let old = ids(registry.refresh().unwrap()).remove(0);
    registry.close().unwrap();
    let next = Registry::new("ffffffffffffffffffffffffffffffff".into(), fake.clone()).unwrap();
    assert!(next.resolve(&old).is_err());
    assert!(next
        .resolve("d:ffffffffffffffffffffffffffffffff:fffffffffffffffff")
        .is_err());
    next.state.borrow_mut().next = u64::MAX;
    assert!(next.refresh().unwrap_err().contains("sequence exhausted"));
    next.close().unwrap();
}
#[test]
fn capacity_refused_before_any_sdk_reference_acquisition() {
    let fake = Fake::new(&(0..257).collect::<Vec<_>>());
    let registry = registry(&fake);
    assert!(registry.refresh().unwrap_err().contains("bounded"));
    assert_eq!(fake.0.retain_calls.get(), 0);
    registry.close().unwrap();
}
#[test]
fn abandoned_registry_never_invokes_backend_drop_or_sdk_release() {
    let fake = Fake::new(&[1]);
    let registry = registry(&fake);
    registry.refresh().unwrap();
    let releases = fake.0.releases.get();
    drop(registry);
    assert_eq!(fake.0.releases.get(), releases);
    assert_eq!(fake.0.backend_drops.load(Ordering::Relaxed), 0);
}

#[test]
fn scoped_document_uses_exact_member_after_reorder_and_never_stale_id() {
    let fake = Fake::new(&[1, 2]);
    let registry = registry(&fake);
    let first = ids(registry.refresh().unwrap()).remove(0);
    fake.rows(&[2, 1]);
    let result = registry
        .with_document(&first, |_, handle, doc| {
            Ok((handle.object, doc.document_id.clone()))
        })
        .unwrap();
    assert_eq!(result, (1, Some(first.clone())));
    fake.rows(&[2]);
    let called = Cell::new(false);
    assert!(registry
        .with_document(&first, |_, _, _| {
            called.set(true);
            Ok(())
        })
        .is_err());
    assert!(!called.get());
    registry.close().unwrap();
}

#[test]
fn scoped_document_reentrancy_or_panic_faults_without_publishing_value() {
    let fake = Fake::new(&[1]);
    let registry = registry(&fake);
    let id = ids(registry.refresh().unwrap()).remove(0);
    assert!(registry
        .with_document(&id, |_, _, _| {
            assert!(registry.refresh().is_err());
            Ok(42)
        })
        .is_err());
    assert!(registry.is_faulted());
    registry.close().unwrap();

    let other = registry_for_panic();
    let id = ids(other.refresh().unwrap()).remove(0);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        other.with_document::<()>(&id, |_, _, _| panic!("injected callback panic"))
    }));
    assert!(result.is_err() && other.is_faulted());
    other.close().unwrap();
}

fn registry_for_panic() -> Registry<Fake> {
    registry(&Fake::new(&[1]))
}

#[test]
fn scoped_document_business_refusal_does_not_fault_valid_identity() {
    let fake = Fake::new(&[1]);
    let registry = registry(&fake);
    let id = ids(registry.refresh().unwrap()).remove(0);
    assert_eq!(
        registry
            .with_document::<()>(&id, |_, _, _| Err("stale parameter revision".into()))
            .unwrap_err(),
        "stale parameter revision"
    );
    assert!(!registry.is_faulted());
    assert_eq!(
        registry
            .with_document(&id, |_, handle, _| Ok(handle.object))
            .unwrap(),
        1
    );
    registry.close().unwrap();
}

#[test]
fn proven_read_selector_refusals_call_no_sdk_or_material_backend() {
    let fake = Fake::new(&[1]);
    let registry = registry(&fake);
    let id = ids(registry.refresh().unwrap()).remove(0);
    fake.rows(&[]);
    registry.refresh().unwrap();
    let queries = [
        ("not-an-id".to_owned(), ReadTargetError::Malformed),
        (
            "d:0123456789abcdef0123456789abcdef:0000000000000000".into(),
            ReadTargetError::Malformed,
        ),
        (
            "d:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0000000000000001".into(),
            ReadTargetError::ForeignEpoch,
        ),
        (id, ReadTargetError::Retired),
        (
            "d:0123456789abcdef0123456789abcdef:0000000000000002".into(),
            ReadTargetError::NotRegistered,
        ),
    ];
    for (id, expected) in queries {
        let before = fake.0.sdk_calls.get();
        let material_calls = Cell::new(0);
        let result = registry.with_read_document(&id, |_, _, _| {
            material_calls.set(material_calls.get() + 1);
            Ok(())
        });
        assert_eq!(result, Err(expected.clone()));
        assert_eq!(fake.0.sdk_calls.get(), before);
        assert_eq!(material_calls.get(), 0);
        let crate::execution::Reply::Value(response) =
            crate::execution::read_target_refusal(expected).unwrap()
        else {
            panic!("wrong reply")
        };
        assert_eq!(response["readBackendInvoked"], false);
        assert_eq!(response["reconciliationRequired"], false);
        assert_eq!(response["nativeExecutionStarted"], false);
        assert!(response.get("outcome").is_none());
    }
    assert!(!registry.is_faulted());
    registry.close().unwrap();
}
#[test]
fn successful_membership_refresh_can_retire_a_read_target_without_material_call() {
    let fake = Fake::new(&[1]);
    let registry = registry(&fake);
    let id = ids(registry.refresh().unwrap()).remove(0);
    fake.rows(&[]);
    let before = fake.0.sdk_calls.get();
    let material_calls = Cell::new(0);
    assert_eq!(
        registry.with_read_document(&id, |_, _, _| {
            material_calls.set(1);
            Ok(())
        }),
        Err(ReadTargetError::Retired)
    );
    assert!(fake.0.sdk_calls.get() > before);
    assert_eq!(material_calls.get(), 0);
    assert!(!registry.is_faulted());
    registry.close().unwrap();
}
#[test]
fn incomplete_refresh_and_backend_error_never_become_known_selector_refusal() {
    let fake = Fake::new(&[1]);
    let registry = registry(&fake);
    let id = ids(registry.refresh().unwrap()).remove(0);
    let result =
        registry.with_read_document::<()>(&id, |_, _, _| Err("material SDK diagnostic".into()));
    assert_eq!(
        result,
        Err(ReadTargetError::Unavailable(
            "material SDK diagnostic".into()
        ))
    );
    assert_eq!(
        crate::execution::read_target_refusal(result.unwrap_err()).unwrap_err(),
        "material SDK diagnostic"
    );
    assert!(!registry.is_faulted());
    fake.0.fail_retain.set(Some(fake.0.retain_calls.get() + 1));
    let called = Cell::new(false);
    let error = registry
        .with_read_document(&id, |_, _, _| {
            called.set(true);
            Ok(())
        })
        .unwrap_err();
    assert!(matches!(error, ReadTargetError::Unavailable(_)));
    assert!(!called.get());
    assert!(registry.is_faulted());
    assert!(matches!(
        registry.preflight_read_target(&id),
        Err(ReadTargetError::Unavailable(_))
    ));
    registry.close().unwrap();
}
