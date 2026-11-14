use std::{
    cell::Cell,
    future::{self, Future},
    marker::PhantomData,
    pin::pin,
    rc::Rc,
    task::Poll,
};

use crate::{
    clock::counter::{Cycles, M_CYCLE},
    executor::{Untimed, Wait},
};

// Timeline domain tags
#[derive(Clone)]
pub struct Cpu;
#[derive(Clone)]
pub struct Fixed;

pub(crate) trait TimeDomain {
    const ID: usize;
    const CYCLE_ALIGNMENT: Cycles;
}

impl TimeDomain for Cpu {
    const ID: usize = 0;
    const CYCLE_ALIGNMENT: Cycles = M_CYCLE;
}
impl TimeDomain for Fixed {
    const ID: usize = 1;
    const CYCLE_ALIGNMENT: Cycles = 1;
}

/// A task's view of time. Holds the clock's counter :
/// the clock owns the tasks, so an `Rc<Time>` in a task would be a cycle.
/// Cloning shares the position, so a helper stays on its caller's timeline.
#[derive(Clone)]
pub struct Timeline<D> {
    pub tag: PhantomData<D>,
    /// Represents the clock of the domain this timelines represents
    pub now: Rc<Cell<Cycles>>,
    pub cursor: Rc<Cell<Cycles>>,
}

impl<D: TimeDomain> Timeline<D> {
    /// Suspend until `n` more cycles pass. Cumulative, not `now + n`: a tick can
    /// carry several M-cycles, and a task must catch up rather than drop the
    /// overshoot.
    pub fn wait(&self, n: Cycles) -> Wait<'_, D> {
        debug_assert!(
            self.now.get().saturating_sub(self.cursor.get()) < D::CYCLE_ALIGNMENT,
            "stale timeline: untimed wait without `after`?"
        );
        self.wait_until(self.cursor.get() + n)
    }

    fn wait_until(&self, at: Cycles) -> Wait<'_, D> {
        Wait { timeline: self, at }
    }

    /// The cycle this task has advanced itself to. Trails `now` while catching up.
    pub fn position(&self) -> Cycles {
        self.cursor.get()
    }

    /// A concurrent branch: shares the clock, owns its own position. `clone`
    /// is for helpers that run *on* the caller's timeline; `fork` is for
    /// sub-tasks that advance beside it.
    pub fn fork(&self) -> Timeline<D> {
        Timeline {
            tag: PhantomData,
            now: self.now.clone(),
            cursor: Rc::new(Cell::new(self.cursor.get())),
        }
    }

    fn get_align(&self) -> Cycles {
        let cursor = self.cursor.get();
        let dt = self.now.get().saturating_sub(cursor);

        let step_size = D::CYCLE_ALIGNMENT;
        let steps = dt.div_ceil(step_size);

        cursor + steps * step_size
    }

    /// Wait for an event that doesn't track time, then resume on this domain's
    /// grid: at the first grid point at or after the event, as if the task had
    /// been polling for it every step.
    pub async fn after<F>(&self, event: Untimed<F>)
    where
        F: Future<Output = ()>,
    {
        event.raw_fut().await;

        self.wait_until(self.get_align()).await;
    }

    pub async fn after_delayed<F>(&self, event: Untimed<F>)
    where
        F: Future<Output = ()>,
    {
        let mut fut = pin!(event.raw_fut());
        let mut waited = false;
        future::poll_fn(|cx| {
            let poll = fut.as_mut().poll(cx);
            waited |= poll.is_pending();

            poll
        })
        .await;

        self.wait_until(self.get_align()).await;
        if waited {
            self.wait(D::CYCLE_ALIGNMENT).await;
        }
    }
}
