//! Test stand-in for the CPU's register writes.

use std::{cell::RefCell, rc::Rc};

use crate::executor::{signal, Raiser, Time};

type Write = Box<dyn FnOnce()>;

/// Stands in for the CPU in device tests. Spawn it after the device under test
/// so it sits last in the task vector, like the CPU. Its writes then land inside
/// a dot's poll, after every device has been polled for that dot, and a device
/// woken by a write runs in the executor's second pass on the same dot.
pub struct CpuStandIn {
    pending: Rc<RefCell<Option<Write>>>,
    wake: Raiser,
}

impl CpuStandIn {
    pub fn spawn(time: &mut Time) -> Self {
        let pending: Rc<RefCell<Option<Write>>> = Rc::new(RefCell::new(None));
        let (wake, mut listener) = signal();
        let queue = pending.clone();
        time.spawn(async move |()| loop {
            listener.until(|| queue.borrow().is_some()).raw_fut().await;
            let write = queue.borrow_mut().take().expect("checked by the predicate");
            write();
        });
        Self { pending, wake }
    }

    /// Runs `write` the way the CPU does: inside the current dot's poll, after
    /// every device has been polled for that dot.
    pub fn write(&self, time: &mut Time, write: impl FnOnce() + 'static) {
        *self.pending.borrow_mut() = Some(Box::new(write));
        self.wake.raise();
        time.poll_woken();
        assert!(
            self.pending.borrow().is_none(),
            "CPU stand-in did not run its write"
        );
    }
}
