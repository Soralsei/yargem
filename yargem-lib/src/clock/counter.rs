use std::{cell::Cell, marker::PhantomData, rc::Rc};

use crate::clock::timeline::{TimeDomain, Timeline};

/// Absolute cycle count since power-on. Wide enough never to wrap in practice.
pub type Cycles = u64;
/// Length of a machine cycle, the granularity at which the CPU touches the bus.
pub const M_CYCLE: Cycles = 4;

pub(crate) struct Counter {
    now: Rc<Cell<Cycles>>,
    stopped: Cell<bool>,
}

impl Counter {
    pub fn new() -> Self {
        Self {
            now: Rc::new(Cell::new(0)),
            stopped: Cell::new(false),
        }
    }

    pub fn advance(&self, n: Cycles) {
        if !self.stopped.get() {
            self.now.set(self.now.get() + n);
        }
    }

    /// A task's cursor into this counter, starting where the counter is now.
    pub fn timeline<D: TimeDomain>(&self) -> Timeline<D> {
        Timeline {
            tag: PhantomData::<D>,
            now: self.now.clone(),
            cursor: Rc::new(Cell::new(self.now())),
        }
    }

    pub fn now(&self) -> Cycles {
        self.now.get()
    }
    pub fn stop(&self) {
        self.stopped.set(true);
    }
    pub fn resume(&self) {
        self.stopped.set(false);
    }
    pub fn stopped(&self) -> bool {
        self.stopped.get()
    }
}
