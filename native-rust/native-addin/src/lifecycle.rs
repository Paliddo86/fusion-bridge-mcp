//! Main-thread lifecycle guards shared by the native adapter and fault tests.
use std::{
    cell::{Cell, RefCell},
    mem::ManuallyDrop,
};

/// Child SDK storage can borrow document/context pointers. Parent teardown must
/// remain blocked until its one explicit release has a confirmed outcome.
#[derive(Default)]
pub(crate) struct ChildLeaseFence(Cell<bool>);
impl ChildLeaseFence {
    pub fn ensure_clear(&self) -> Result<(), String> {
        if self.0.get() {
            Err("child SDK lease outstanding or release unconfirmed; teardown refused".into())
        } else {
            Ok(())
        }
    }
    pub fn begin(&self) -> Result<ChildLease<'_>, String> {
        self.ensure_clear()?;
        self.0.set(true);
        Ok(ChildLease(self))
    }
}
/// Not Clone; abandoning the token keeps the parent fence closed. No destructor
/// invokes SDK cleanup, including unwind/TLS paths.
pub(crate) struct ChildLease<'a>(&'a ChildLeaseFence);
impl ChildLease<'_> {
    /// Only for an opening ABI whose failed return guarantees no acquired lease.
    pub fn unopened(self) {
        self.0 .0.set(false);
    }
    pub fn run<H, R>(
        self,
        handle: H,
        work: impl FnOnce(&H) -> R,
        release: impl FnOnce(H) -> Result<(), String>,
    ) -> Result<R, String> {
        let observed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&handle)));
        let closed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| release(handle)));
        match closed {
            Ok(Ok(())) => self.0 .0.set(false),
            Ok(Err(error)) => return Err(error),
            Err(_) => return Err("child SDK release panicked; teardown refused".into()),
        }
        match observed {
            Ok(result) => Ok(result),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
}

/// TLS destruction can occur under Windows loader constraints. Only an explicit
/// Fusion stop callback may recover this value and perform joins/SDK teardown.
/// A skipped stop deliberately retains it until the process exits.
pub(crate) struct StateSlot<T>(RefCell<Option<ManuallyDrop<T>>>);
impl<T> StateSlot<T> {
    pub const fn new() -> Self {
        Self(RefCell::new(None))
    }
    pub fn put(&self, value: T) -> Result<(), T> {
        let mut slot = self.0.borrow_mut();
        if slot.is_some() {
            return Err(value);
        }
        *slot = Some(ManuallyDrop::new(value));
        Ok(())
    }
    pub fn take(&self) -> Option<T> {
        self.0.borrow_mut().take().map(ManuallyDrop::into_inner)
    }
}

#[derive(Default)]
pub(crate) enum CloseOnce {
    #[default]
    Open,
    Attempting,
    Closed,
    Failed(String),
}
impl CloseOnce {
    pub fn ensure_open(&self) -> Result<(), String> {
        match self {
            Self::Open => Ok(()),
            _ => Err("SDK resource close was already attempted".into()),
        }
    }
    pub fn close(&mut self, close: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        match self {
            Self::Closed => return Ok(()),
            Self::Failed(error) => return Err(error.clone()),
            Self::Attempting => return Err("SDK close outcome unknown; retry refused".into()),
            Self::Open => {}
        }
        // A panic or an uncertain native delete can never trigger a second delete.
        *self = Self::Attempting;
        match close() {
            Ok(()) => {
                *self = Self::Closed;
                Ok(())
            }
            Err(error) => {
                *self = Self::Failed(error.clone());
                Err(error)
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct CallbackGate {
    active: Cell<bool>,
    stopping: Cell<bool>,
}
impl CallbackGate {
    pub fn enter(&self) -> Option<CallbackFlight<'_>> {
        if self.stopping.get() || self.active.replace(true) {
            return None;
        }
        Some(CallbackFlight(self))
    }
    pub fn begin_stop(&self) -> Result<(), String> {
        if self.active.get() {
            return Err("callback is in flight; teardown deferred".into());
        }
        self.stopping.set(true);
        Ok(())
    }
}
pub(crate) struct CallbackFlight<'a>(&'a CallbackGate);
impl Drop for CallbackFlight<'_> {
    fn drop(&mut self) {
        self.0.active.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct Tracked(Arc<AtomicUsize>);
    impl Drop for Tracked {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    #[test]
    fn child_work_panic_releases_once_before_resuming_unwind() {
        let fence = ChildLeaseFence::default();
        let calls = Cell::new(0);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let token = fence.begin().unwrap();
            let _: Result<(), String> = token.run(
                7,
                |_| panic!("work panic after acquisition"),
                |id| {
                    assert_eq!(id, 7);
                    calls.set(calls.get() + 1);
                    Ok(())
                },
            );
        }));
        assert!(result.is_err());
        assert_eq!(calls.get(), 1);
        fence.ensure_clear().unwrap();
        fence.begin().unwrap().unopened();
        fence.ensure_clear().unwrap();
    }
    #[test]
    fn failed_or_panicked_child_release_keeps_parent_fenced_without_retry() {
        for panic_release in [false, true] {
            let fence = ChildLeaseFence::default();
            let calls = Cell::new(0);
            let token = fence.begin().unwrap();
            let result = token.run(
                (),
                |_| 42,
                |_| {
                    calls.set(calls.get() + 1);
                    if panic_release {
                        panic!("release panic");
                    }
                    Err("release outcome unknown".into())
                },
            );
            assert!(result.is_err());
            assert_eq!(calls.get(), 1);
            assert!(fence.ensure_clear().is_err());
            assert!(fence.begin().is_err());
            assert_eq!(calls.get(), 1);
        }
    }
    #[test]
    fn abandoned_child_token_never_claims_cleanup_or_calls_a_destructor() {
        let fence = ChildLeaseFence::default();
        let token = fence.begin().unwrap();
        drop(token);
        assert!(fence.ensure_clear().is_err());
        assert!(fence.begin().is_err());
    }
    #[test]
    fn tls_exit_never_implicitly_joins_workers_or_releases_sdk() {
        thread_local! {static SLOT:StateSlot<Tracked>=const{StateSlot::new()};}
        let drops = Arc::new(AtomicUsize::new(0));
        let worker_count = drops.clone();
        std::thread::spawn(move || {
            SLOT.with(|slot| assert!(slot.put(Tracked(worker_count)).is_ok()))
        })
        .join()
        .unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn only_explicit_take_releases_owned_state() {
        let slot = StateSlot::new();
        let drops = Arc::new(AtomicUsize::new(0));
        assert!(slot.put(Tracked(drops.clone())).is_ok());
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(slot.take().unwrap());
        assert!(slot.take().is_none());
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn uncertain_close_is_never_repeated_or_reported_released() {
        let mut close = CloseOnce::default();
        assert!(close.close(|| Err("unknown delete result".into())).is_err());
        assert!(close.ensure_open().is_err());
        assert_eq!(
            close
                .close(|| panic!("must not retry native delete"))
                .unwrap_err(),
            "unknown delete result"
        );
    }
    #[test]
    fn confirmed_close_is_idempotent_without_native_retry() {
        let mut close = CloseOnce::default();
        close.close(|| Ok(())).unwrap();
        close.close(|| panic!("must not delete twice")).unwrap();
        assert!(close.ensure_open().is_err());
    }
    #[test]
    fn panic_during_close_latches_unknown() {
        let mut close = CloseOnce::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            close.close(|| panic!("injected"))
        }));
        assert!(result.is_err());
        assert!(close
            .close(|| panic!("must not retry after panic"))
            .is_err());
    }
    #[test]
    fn callback_refuses_reentrancy_and_defers_teardown_until_exit() {
        let gate = CallbackGate::default();
        let flight = gate.enter().unwrap();
        assert!(gate.enter().is_none());
        assert!(gate.begin_stop().is_err());
        drop(flight);
        // A refused stop must leave the running instance usable.
        drop(gate.enter().unwrap());
        gate.begin_stop().unwrap();
        assert!(gate.enter().is_none());
    }
    #[test]
    fn panic_unwinds_flight_before_stop() {
        let gate = CallbackGate::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _flight = gate.enter().unwrap();
            panic!("injected callback panic");
        }));
        assert!(result.is_err());
        gate.begin_stop().unwrap();
    }
}
