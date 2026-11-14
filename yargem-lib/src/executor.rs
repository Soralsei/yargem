use std::cell::Cell;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::ptr;
use std::rc::Rc;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use crate::clock::{Cpu, CpuClock, Cycles, Fixed, FixedClock, TimeDomain, Timeline};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T>>>;

pub(crate) trait FromTask {
    fn build(cpu: &CpuClock, fixed: &FixedClock) -> Self;
}

impl FromTask for Timeline<Cpu> {
    fn build(cpu: &CpuClock, _fixed: &FixedClock) -> Self {
        cpu.timeline()
    }
}

impl FromTask for Timeline<Fixed> {
    fn build(_cpu: &CpuClock, fixed: &FixedClock) -> Self {
        fixed.timeline()
    }
}

impl FromTask for (Timeline<Cpu>, Timeline<Fixed>) {
    fn build(cpu: &CpuClock, fixed: &FixedClock) -> Self {
        (cpu.timeline(), fixed.timeline())
    }
}

impl FromTask for (Timeline<Fixed>, Timeline<Cpu>) {
    fn build(cpu: &CpuClock, fixed: &FixedClock) -> Self {
        (fixed.timeline(), cpu.timeline())
    }
}

impl FromTask for () {
    fn build(_: &CpuClock, _: &FixedClock) -> Self {}
}

pub struct Slot {
    wake_at: [Cell<Cycles>; 2],
    notified: Cell<bool>,
}

impl Slot {
    pub fn new() -> Self {
        Self {
            wake_at: [const { Cell::new(Cycles::MAX) }; 2],
            notified: Cell::new(true),
        }
    }

    pub fn notified(&self) -> bool {
        self.notified.get()
    }

    pub fn wake_at(&self) -> &[Cell<Cycles>] {
        &self.wake_at
    }

    pub fn set_notified(&self, notified: bool) {
        self.notified.set(notified);
    }
}

pub fn slot_of<'a>(ctx: &'a Context<'_>) -> Option<&'a Slot> {
    let waker = ctx.waker();
    if ptr::eq(waker.vtable(), &VTABLE) {
        return Some(unsafe { &*(waker.data() as *const Slot) });
    }
    None
}

#[must_use = "does nothing unless awaited"]
pub struct Wait<'a, D: TimeDomain> {
    pub timeline: &'a Timeline<D>,
    pub at: Cycles,
}

impl<D> Future for Wait<'_, D>
where
    D: TimeDomain,
{
    type Output = ();
    fn poll(self: Pin<&mut Self>, ctx: &mut Context) -> Poll<()> {
        if self.timeline.now.get() >= self.at {
            let cursor = &self.timeline.cursor;
            cursor.set(cursor.get().max(self.at));
            Poll::Ready(())
        } else {
            if let Some(slot) = slot_of(ctx) {
                let wake_at = &slot.wake_at[D::ID];
                wake_at.set(wake_at.get().min(self.at));
            }
            Poll::Pending
        }
    }
}

#[derive(Clone)]
pub struct Raiser(Rc<Cell<Option<Waker>>>);

impl Raiser {
    pub fn raise(&self) {
        if let Some(waker) = self.0.take() {
            waker.wake();
        }
    }
}

pub struct Listener(Rc<Cell<Option<Waker>>>);

impl Listener {
    pub fn until<F: Fn() -> bool>(&mut self, predicate: F) -> Untimed<Until<'_, F>> {
        let until = Until {
            listener: self,
            predicate,
        };
        Untimed { fut: until }
    }
}

pub fn signal() -> (Raiser, Listener) {
    let waker = Rc::new(Cell::new(None));
    (Raiser(waker.clone()), Listener(waker))
}

#[must_use = "does nothing unless awaited"]
pub struct Until<'a, F: Fn() -> bool> {
    listener: &'a Listener,
    predicate: F,
}

impl<'a, F> Until<'a, F>
where
    F: Fn() -> bool,
{
    fn register(&self, waker: &Waker) {
        let new = match self.listener.0.take() {
            Some(old) if old.will_wake(waker) => old,
            _ => waker.clone(),
        };
        self.listener.0.set(Some(new));
    }
}

impl<'a, F> Future for Until<'a, F>
where
    F: Fn() -> bool,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if (self.predicate)() {
            Poll::Ready(())
        } else {
            self.register(cx.waker());
            Poll::Pending
        }
    }
}

impl<F: Fn() -> bool> Drop for Until<'_, F> {
    fn drop(&mut self) {
        self.listener.0.take();
    }
}

pub struct Untimed<F: Future> {
    fut: F,
}

impl<F: Future> Untimed<F> {
    pub fn raw_fut(self) -> F {
        self.fut
    }
}

#[derive(Clone)]
pub struct EdgeRaiser {
    pending: Rc<Cell<bool>>,
    raiser: Raiser,
}

impl EdgeRaiser {
    pub fn raise(&self) {
        debug_assert!(
            !self.pending.get(),
            "at most one pending edge in EdgeRaiser/Listener"
        );
        self.pending.set(true);
        self.raiser.raise();
    }
}

pub struct EdgeListener {
    pending: Rc<Cell<bool>>,
    listener: Listener,
}

impl EdgeListener {
    pub fn next(&mut self) -> Untimed<Until<'_, impl Fn() -> bool + use<'_>>> {
        let pending = &self.pending;
        self.listener.until(|| pending.take())
    }
}

pub fn edge() -> (EdgeRaiser, EdgeListener) {
    let pending = Rc::new(Cell::new(false));
    let (raiser, listener) = signal();
    (
        EdgeRaiser {
            pending: pending.clone(),
            raiser,
        },
        EdgeListener { pending, listener },
    )
}

/// Tasks never complete: `Infallible` makes that checked rather than conventional.
struct Task {
    fut: BoxFuture<Infallible>,
    slot: Rc<Slot>,
    waker: Waker,
}

impl Task {
    pub fn new(fut: BoxFuture<Infallible>, slot: Rc<Slot>, waker: Waker) -> Self {
        Self { fut, slot, waker }
    }

    pub fn should_poll(&self, now: [Cycles; 2]) -> bool {
        self.slot.notified()
            || self
                .slot
                .wake_at()
                .iter()
                .zip(now)
                .any(|(wake, now)| wake.get() <= now)
    }

    fn notified(&self) -> bool {
        self.slot.notified()
    }

    pub fn poll(&mut self) {
        self.slot.set_notified(false);
        self.slot
            .wake_at()
            .iter()
            .for_each(|wake| wake.set(Cycles::MAX));

        let _ = self
            .fut
            .as_mut()
            .poll(&mut Context::from_waker(&self.waker));
    }
}

struct Executor {
    tasks: Vec<Task>,
}

static VTABLE: RawWakerVTable = RawWakerVTable::new(
    Executor::waker_clone,
    Executor::wake,
    Executor::wake_by_ref,
    Executor::waker_drop,
);

impl Executor {
    pub fn new() -> Self {
        Self {
            tasks: Vec::with_capacity(10),
        }
    }

    /// Polled once here so the task reaches its first `await` at the current
    /// cycle rather than the next tick's.
    pub fn spawn(&mut self, future: impl Future<Output = Infallible> + 'static) {
        let future = Box::pin(future);

        let slot = Rc::new(Slot::new());

        // SAFETY: The waker should NEVER cross a thread, as it is implemented
        // with a plain Rc instead of an Arc. Using it in a multithreaded context
        // would risk race conditions on the refcount which would lead to memory leaks
        let raw_waker = RawWaker::new(Rc::into_raw(slot.clone()).cast(), &VTABLE);
        let waker = unsafe { Waker::from_raw(raw_waker) };

        let mut task = Task::new(future, slot, waker);

        task.poll();

        self.tasks.push(task);
    }

    /// Every awoken task in a previous poll (if it was awoken by a previous task in the vector)
    /// or a previous clock (if it was awoken by a task in the second pass), in spawn order.
    /// Order is savestate contract.
    pub fn poll(&mut self, now: [Cycles; 2]) {
        for task in self.tasks.iter_mut() {
            if task.should_poll(now) {
                task.poll();
            }
        }

        // Pass 2: notified tasks get woken on the same dot.
        // tasks that get notified in this pass will only be polled on the next dot
        for task in self.tasks.iter_mut() {
            if task.notified() {
                task.poll();
            }
        }
    }

    fn notified(&self) -> bool {
        self.tasks.iter().any(|t| t.slot.notified())
    }

    fn closest_deadlines(&self, now: [Cycles; 2]) -> Option<(Cycles, Cycles)> {
        let mut deadlines = (Cycles::MAX, Cycles::MAX);
        for task in &self.tasks {
            if task.slot.notified() {
                return None;
            }
            deadlines.0 = deadlines
                .0
                .min(task.slot.wake_at()[Cpu::ID].get() - now[Cpu::ID]);
            deadlines.1 = deadlines
                .1
                .min(task.slot.wake_at()[Fixed::ID].get() - now[Fixed::ID]);
        }
        Some(deadlines)
    }

    unsafe fn waker_clone(slot: *const ()) -> RawWaker {
        let slot: *const Slot = slot.cast();
        Rc::increment_strong_count(slot);
        RawWaker::new(slot.cast(), &VTABLE)
    }
    unsafe fn wake(slot: *const ()) {
        Self::wake_by_ref(slot);
        Self::waker_drop(slot);
    }
    unsafe fn wake_by_ref(slot: *const ()) {
        let slot: *const Slot = slot.cast();
        (*slot).set_notified(true);
    }
    unsafe fn waker_drop(slot: *const ()) {
        let slot: *const Slot = slot.cast();
        Rc::decrement_strong_count(slot);
    }
}

pub struct Time {
    pub cpu: Rc<CpuClock>,
    pub fixed: Rc<FixedClock>,
    exec: Executor,
}

impl Time {
    pub fn new() -> Self {
        Self {
            cpu: CpuClock::new(),
            fixed: FixedClock::new(),
            exec: Executor::new(),
        }
    }

    pub fn tick(&mut self) {
        if self.fixed.stopped() && self.cpu.stopped() && !self.exec.notified() {
            return;
        }
        self.fixed.tick(1);
        self.cpu.tick(1);

        self.exec.poll([self.cpu.now(), self.fixed.now()]);
    }

    /// Polls the tasks woken since the last poll on the current dot, without
    /// advancing time. Lets a test stand-in spawned last act inside a dot's
    /// poll, as the CPU does (see `util::cpu_stand_in`).
    #[cfg(test)]
    pub fn poll_woken(&mut self) {
        self.exec.poll([self.cpu.now(), self.fixed.now()]);
    }

    pub fn advance(&mut self, max: Cycles) -> Cycles {
        if self.fixed.stopped() && self.cpu.stopped() && !self.exec.notified() {
            return 0;
        }
        let deadlines = self
            .exec
            .closest_deadlines([self.cpu.now(), self.fixed.now()]);
        let ticks = if let Some((d_cpu, d_fixed)) = deadlines {
            (d_cpu.div_ceil(1 << self.cpu.speed() as u8)).min(d_fixed)
        } else {
            1
        }
        .min(max);
        self.fixed.tick(ticks);
        self.cpu.tick(ticks);

        self.exec.poll([self.cpu.now(), self.fixed.now()]);

        ticks
    }

    pub fn spawn<F, TD, Fut>(&mut self, get_fut: F)
    where
        F: FnOnce(TD) -> Fut,
        TD: FromTask,
        Fut: Future<Output = Infallible> + 'static,
    {
        let fut = get_fut(TD::build(&self.cpu, &self.fixed));
        self.exec.spawn(fut);
    }

    pub fn stopped(&self) -> bool {
        self.cpu.stopped() && self.fixed.stopped()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{clock::M_CYCLE, util::future::Race};
    use std::{cell::RefCell, future::pending, pin::pin, ptr};

    type Log = Rc<RefCell<Vec<Cycles>>>;

    fn log() -> Log {
        Rc::new(RefCell::new(Vec::new()))
    }

    /// Counts every poll the executor makes, including the ones that stop at a
    /// pending leaf without running any of the task's own code.
    struct PollCounter<T> {
        inner: BoxFuture<T>,
        count: Rc<Cell<u32>>,
    }

    impl<T> PollCounter<T> {
        fn new(inner: impl Future<Output = T> + 'static, count: Rc<Cell<u32>>) -> Self {
            Self {
                inner: Box::pin(inner),
                count,
            }
        }
    }

    impl<T> Future for PollCounter<T> {
        type Output = T;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            self.count.set(self.count.get() + 1);
            self.inner.as_mut().poll(cx)
        }
    }

    /// A level the producer sets, and the signal it raises once it has.
    #[derive(Clone)]
    struct Flag {
        level: Rc<Cell<bool>>,
        raiser: Raiser,
    }

    /// The consumer side of a [`Flag`].
    struct FlagWaiter {
        level: Rc<Cell<bool>>,
        listener: Listener,
    }

    impl Flag {
        fn new() -> (Flag, FlagWaiter) {
            let level = Rc::new(Cell::new(false));
            let (raiser, listener) = signal();
            (
                Flag {
                    level: level.clone(),
                    raiser,
                },
                FlagWaiter { level, listener },
            )
        }

        fn set(&self) {
            self.level.set(true);
            self.raiser.raise();
        }
    }

    impl FlagWaiter {
        fn until(&mut self) -> Untimed<Until<'_, impl Fn() -> bool + use<'_>>> {
            let level = &self.level;
            self.listener.until(|| level.get())
        }
    }

    async fn ticker<D: TimeDomain>(t: Timeline<D>, period: Cycles, log: Log) -> Infallible {
        loop {
            t.wait(period).await;
            log.borrow_mut().push(t.position());
        }
    }

    async fn idle<D: TimeDomain>(t: Timeline<D>, period: Cycles) -> Infallible {
        loop {
            t.wait(period).await;
        }
    }

    /// Sets `flag` once, `at` fixed cycles in.
    fn spawn_raiser(clock: &mut Time, flag: Flag, at: Cycles) {
        clock.spawn(move |t: Timeline<Fixed>| async move {
            t.wait(at).await;
            flag.set();
            pending::<Infallible>().await
        });
    }

    /// Parks on `flag`, then logs the fixed cycle it resumed on.
    fn spawn_waiter(clock: &mut Time, mut flag: FlagWaiter, woke: Log) {
        let fixed = clock.fixed.clone();
        clock.spawn(move |_: Timeline<Fixed>| async move {
            flag.until().raw_fut().await;
            woke.borrow_mut().push(fixed.now());
            pending::<Infallible>().await
        });
    }

    /// Parks on `flag` through `after`, logs its position, then logs it again
    /// one M-cycle later.
    fn spawn_realigner(clock: &mut Time, mut flag: FlagWaiter, log: Log) {
        clock.spawn(move |t: Timeline<Cpu>| async move {
            t.after(flag.until()).await;
            log.borrow_mut().push(t.position());
            t.wait(M_CYCLE).await;
            log.borrow_mut().push(t.position());
            pending::<Infallible>().await
        });
    }

    fn clock_tick(clock: &mut Time, ticks: u32) {
        for _ in 0..ticks {
            clock.tick();
        }
    }

    fn spawn_ticker<D>(clock: &mut Time, period: Cycles) -> Log
    where
        Timeline<D>: FromTask,
        D: TimeDomain + 'static,
    {
        let log = log();
        let handle = log.clone();
        clock.spawn(|timeline: Timeline<D>| ticker(timeline, period, handle));
        log
    }

    #[test]
    fn does_not_wake_before_its_deadline() {
        let mut clock = Time::new();
        let log = spawn_ticker::<Fixed>(&mut clock, 4);

        clock_tick(&mut clock, 2);
        assert!(log.borrow().is_empty());

        clock_tick(&mut clock, 2);
        assert_eq!(*log.borrow(), vec![4]);
    }

    #[test]
    fn catches_up_inside_one_poll() {
        let mut clock = Time::new();
        let log = spawn_ticker::<Fixed>(&mut clock, 4);

        // Three periods in one tick: three wakes, on multiples of the period.
        clock_tick(&mut clock, 12);
        assert_eq!(*log.borrow(), vec![4, 8, 12]);
    }

    #[test]
    fn double_speed_splits_the_domains() {
        let mut clock = Time::new();
        let cpu = spawn_ticker::<Cpu>(&mut clock, 4);
        let fixed = spawn_ticker::<Fixed>(&mut clock, 4);

        clock.cpu.switch_speed();

        clock_tick(&mut clock, 2);
        assert_eq!(*cpu.borrow(), vec![4]);
        assert!(fixed.borrow().is_empty(), "fixed domain ran at CPU rate");

        clock_tick(&mut clock, 2);
        assert_eq!(*cpu.borrow(), vec![4, 8]);
        assert_eq!(*fixed.borrow(), vec![4]);
    }

    #[test]
    fn poll_order_is_spawn_order() {
        let mut clock = Time::new();
        let order: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));

        for id in 0..3u8 {
            let order = order.clone();
            clock.spawn(|t: Timeline<Fixed>| async move {
                loop {
                    t.wait(4).await;
                    order.borrow_mut().push(id);
                }
            });
        }

        clock_tick(&mut clock, 4);
        assert_eq!(*order.borrow(), vec![0, 1, 2]);
    }

    #[test]
    fn stopped_clock_advances_nothing() {
        let mut clock = Time::new();
        let log = spawn_ticker::<Cpu>(&mut clock, 4);

        clock.cpu.stop();
        clock_tick(&mut clock, 16);
        assert!(log.borrow().is_empty());

        clock.cpu.resume();
        clock_tick(&mut clock, 4);
        assert_eq!(*log.borrow(), vec![4]);
    }

    #[test]
    fn parked_task_is_not_polled() {
        let mut clock = Time::new();
        let (_flag, mut waiter) = Flag::new();
        let polls = Rc::new(Cell::new(0));

        let p = polls.clone();
        clock.spawn(move |_: Timeline<Fixed>| {
            PollCounter::new(
                async move {
                    waiter.until().raw_fut().await;
                    pending::<Infallible>().await
                },
                p,
            )
        });
        assert_eq!(polls.get(), 1, "spawn polls exactly once");

        clock_tick(&mut clock, 100);
        assert_eq!(polls.get(), 1, "parked task was polled");
    }

    #[test]
    fn time_wait_is_polled_only_at_its_deadline() {
        let mut clock = Time::new();
        let polls = Rc::new(Cell::new(0));

        let p = polls.clone();
        clock.spawn(move |t: Timeline<Fixed>| PollCounter::new(idle(t, 8), p));

        clock_tick(&mut clock, 7);
        assert_eq!(polls.get(), 1, "polled before its deadline");

        clock_tick(&mut clock, 1);
        assert_eq!(polls.get(), 2);

        clock_tick(&mut clock, 8);
        assert_eq!(polls.get(), 3);
    }

    #[test]
    fn stopped_domain_parks_its_tasks() {
        let mut clock = Time::new();
        let polls = Rc::new(Cell::new(0));

        let p = polls.clone();
        clock.spawn(move |t: Timeline<Cpu>| PollCounter::new(idle(t, 4), p));

        clock.cpu.stop();
        clock_tick(&mut clock, 16);
        assert_eq!(polls.get(), 1, "task on a stopped clock was polled");

        clock.cpu.resume();
        clock_tick(&mut clock, 4);
        assert_eq!(polls.get(), 2);
    }

    #[test]
    fn edge_steps_once_per_raise_then_parks() {
        let mut clock = Time::new();
        let (raiser, mut listener) = edge();
        let steps = Rc::new(Cell::new(0u32));
        let polls = Rc::new(Cell::new(0));

        let (s, p) = (steps.clone(), polls.clone());
        clock.spawn(move |_: Timeline<Fixed>| {
            PollCounter::new(
                async move {
                    loop {
                        listener.next().raw_fut().await;
                        s.set(s.get() + 1);
                    }
                },
                p,
            )
        });

        clock_tick(&mut clock, 10);
        assert_eq!(steps.get(), 0, "stepped without an edge");

        raiser.raise();
        clock_tick(&mut clock, 1);
        assert_eq!(steps.get(), 1);

        let polls_after = polls.get();
        clock_tick(&mut clock, 10);
        assert_eq!(steps.get(), 1, "one edge stepped twice");
        assert_eq!(polls.get(), polls_after, "consumer did not park again");
    }

    #[test]
    fn raise_from_earlier_task_lands_in_the_same_pass() {
        let mut clock = Time::new();
        let (flag, waiter) = Flag::new();
        let woke = log();

        spawn_raiser(&mut clock, flag, 8);
        spawn_waiter(&mut clock, waiter, woke.clone());

        clock_tick(&mut clock, 8);
        assert_eq!(*woke.borrow(), vec![8]);
    }

    #[test]
    fn raise_from_later_task_lands_on_same_tick() {
        let mut clock = Time::new();
        let (flag, waiter) = Flag::new();
        let woke = log();

        spawn_waiter(&mut clock, waiter, woke.clone());
        spawn_raiser(&mut clock, flag, 8);

        clock_tick(&mut clock, 8);
        assert_eq!(*woke.borrow(), vec![8]);
    }

    #[test]
    fn raise_before_parking_is_not_lost() {
        let mut clock = Time::new();
        let (flag, mut waiter) = Flag::new();
        let woke = log();
        let fixed = clock.fixed.clone();

        let w = woke.clone();
        clock.spawn(move |t: Timeline<Fixed>| async move {
            t.wait(8).await;
            waiter.until().raw_fut().await;
            w.borrow_mut().push(fixed.now());
            pending::<Infallible>().await
        });

        // The signal has no waiter yet: only the level carries the event.
        clock_tick(&mut clock, 4);
        flag.set();

        clock_tick(&mut clock, 4);
        assert_eq!(*woke.borrow(), vec![8]);
    }

    #[test]
    fn spurious_raise_costs_one_poll() {
        let mut clock = Time::new();
        let (flag, mut waiter) = Flag::new();
        let woke = log();
        let polls = Rc::new(Cell::new(0));
        let fixed = clock.fixed.clone();

        let (w, p) = (woke.clone(), polls.clone());
        clock.spawn(move |_: Timeline<Fixed>| {
            PollCounter::new(
                async move {
                    waiter.until().raw_fut().await;
                    w.borrow_mut().push(fixed.now());
                    pending::<Infallible>().await
                },
                p,
            )
        });

        // Raised with the level still low.
        flag.raiser.raise();
        clock_tick(&mut clock, 1);
        assert_eq!(polls.get(), 2);
        assert!(woke.borrow().is_empty());

        clock_tick(&mut clock, 10);
        assert_eq!(polls.get(), 2, "did not park again after a spurious wake");

        flag.set();
        clock_tick(&mut clock, 1);
        assert_eq!(*woke.borrow(), vec![12]);
    }

    #[test]
    fn race_keeps_the_time_branch_running_while_the_event_branch_is_parked() {
        let mut clock = Time::new();
        let (flag, mut waiter) = Flag::new();
        let ticks = log();
        let woke = log();
        let fixed = clock.fixed.clone();

        let (tl, w) = (ticks.clone(), woke.clone());
        clock.spawn(move |t: Timeline<Fixed>| async move {
            let background = pin!(ticker(t, 4, tl));
            let main = pin!(waiter.until().raw_fut());
            Race::new(background, main).await;
            w.borrow_mut().push(fixed.now());
            pending::<Infallible>().await
        });

        clock_tick(&mut clock, 12);
        assert_eq!(*ticks.borrow(), vec![4, 8, 12]);
        assert!(woke.borrow().is_empty());

        flag.set();
        clock_tick(&mut clock, 1);
        assert_eq!(*woke.borrow(), vec![13]);

        clock_tick(&mut clock, 8);
        assert_eq!(
            *ticks.borrow(),
            vec![4, 8, 12],
            "background outlived the race"
        );
    }

    #[test]
    fn after_rounds_up_to_the_next_grid_point() {
        let mut clock = Time::new();
        let (flag, waiter) = Flag::new();
        let log = log();
        spawn_realigner(&mut clock, waiter, log.clone());

        clock_tick(&mut clock, 5);
        flag.set();
        // Woken at 6, between grid points 4 and 8.
        clock_tick(&mut clock, 1);
        assert_eq!(*log.borrow(), vec![]);

        clock_tick(&mut clock, 5);
        assert_eq!(*log.borrow(), vec![8]);
        clock_tick(&mut clock, 1);
        assert_eq!(*log.borrow(), vec![8, 12]);
    }

    #[test]
    fn after_on_a_grid_point_keeps_the_position() {
        let mut clock = Time::new();
        let (flag, waiter) = Flag::new();
        let log = log();
        // Spawned first, so its raise at 8 reaches the waiter in the same pass.
        spawn_raiser(&mut clock, flag, 8);
        spawn_realigner(&mut clock, waiter, log.clone());

        clock_tick(&mut clock, 8);
        assert_eq!(*log.borrow(), vec![8]);

        clock_tick(&mut clock, 4);
        assert_eq!(*log.borrow(), vec![8, 12]);
    }

    #[test]
    fn after_rounds_up_in_double_speed() {
        let mut clock = Time::new();
        clock.cpu.switch_speed();
        let (flag, waiter) = Flag::new();
        let log = log();
        spawn_realigner(&mut clock, waiter, log.clone());

        clock_tick(&mut clock, 2);
        flag.set();
        // Woken at 6: half an M-cycle past the grid point at 4.
        clock_tick(&mut clock, 1);
        assert_eq!(*log.borrow(), vec![]);

        clock_tick(&mut clock, 2);
        assert_eq!(*log.borrow(), vec![8]);
        clock_tick(&mut clock, 1);
        assert_eq!(*log.borrow(), vec![8, 12]);
    }

    #[test]
    fn after_keeps_the_task_phase() {
        let mut clock = Time::new();
        clock_tick(&mut clock, 1);
        let (flag, waiter) = Flag::new();
        let log = log();
        // Spawned at 1: this task's grid is 1, 5, 9, 13...
        spawn_realigner(&mut clock, waiter, log.clone());

        clock_tick(&mut clock, 5);
        flag.set();
        clock_tick(&mut clock, 1);
        assert_eq!(*log.borrow(), vec![]);

        clock_tick(&mut clock, 5);
        assert_eq!(*log.borrow(), vec![9]);
        clock_tick(&mut clock, 1);
        assert_eq!(*log.borrow(), vec![9, 13]);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "stale timeline")]
    fn untimed_wait_without_after_trips_the_stale_check() {
        let mut clock = Time::new();
        let (flag, mut waiter) = Flag::new();

        clock.spawn(move |t: Timeline<Cpu>| async move {
            waiter.until().raw_fut().await;
            t.wait(M_CYCLE).await;
            pending::<Infallible>().await
        });

        clock_tick(&mut clock, 10);
        flag.set();
        clock_tick(&mut clock, 1);
    }

    #[test]
    fn waker_refcount_stays_balanced() {
        let mut clock = Time::new();
        let (flag, mut waiter) = Flag::new();

        clock.spawn(move |_: Timeline<Fixed>| async move {
            waiter.until().raw_fut().await;
            pending::<Infallible>().await
        });

        let slot = clock.exec.tasks[0].slot.clone();
        // The task's handle, its waker, the clone parked in the listener, and ours.
        assert_eq!(Rc::strong_count(&slot), 4);

        // Repolling the same task keeps the stored waker rather than cloning.
        for _ in 0..5 {
            slot.set_notified(true);
            clock_tick(&mut clock, 1);
        }
        assert_eq!(Rc::strong_count(&slot), 4);

        // Raising consumes the stored waker; once ready, nothing re-registers.
        flag.set();
        assert_eq!(Rc::strong_count(&slot), 3);
        clock_tick(&mut clock, 1);
        assert_eq!(Rc::strong_count(&slot), 3);
    }

    #[test]
    fn slot_of_recognises_only_its_own_waker() {
        let mut clock = Time::new();
        clock.spawn(|t: Timeline<Fixed>| idle(t, 4));

        let task = &clock.exec.tasks[0];
        let own = Context::from_waker(&task.waker);
        assert!(slot_of(&own).is_some_and(|slot| ptr::eq(slot, &*task.slot)));

        let mut foreign = Context::from_waker(Waker::noop());
        assert!(slot_of(&foreign).is_none());

        // A wait polled from elsewhere stays pending and registers nothing.
        let timeline = clock.fixed.timeline();
        let mut wait = pin!(timeline.wait(4));
        assert_eq!(wait.as_mut().poll(&mut foreign), Poll::Pending);
    }
}
