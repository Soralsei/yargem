use std::cell::Cell;
use std::convert::Infallible;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::ptr;
use std::rc::Rc;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

/// Length of a machine cycle, the granularity at which the CPU touches the bus.
pub const M_CYCLE: u64 = 4;

/// Absolute cycle count since power-on. Wide enough never to wrap in practice.
pub type Cycles = u64;
type BoxFuture<T> = Pin<Box<dyn Future<Output = T>>>;

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
    tag: PhantomData<D>,
    /// Represents the clock of the domain this timelines represents
    now: Rc<Cell<Cycles>>,
    cursor: Rc<Cell<Cycles>>,
}

impl<D: TimeDomain> Timeline<D> {
    /// Suspend until `n` more cycles pass. Cumulative, not `now + n`: a tick can
    /// carry several M-cycles, and a task must catch up rather than drop the
    /// overshoot.
    pub fn wait(&self, n: Cycles) -> Wait<D> {
        debug_assert!(
            self.now.get().saturating_sub(self.cursor.get()) < D::CYCLE_ALIGNMENT,
            "stale timeline: untimed wait without `after`?"
        );
        let wake_at = self.cursor.get().wrapping_add(n);
        self.cursor.set(wake_at);
        Wait {
            tag: PhantomData,
            now: self.now.clone(),
            at: wake_at,
        }
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

    /// Absorb a branch's progress back into this timeline.
    pub fn join(&self, branch: &Timeline<D>) {
        let furthest = self.cursor.get().max(branch.cursor.get());
        self.cursor.set(furthest);
    }

    /// Wait for an event that doesn't track time, then resume on this domain's
    /// grid: at the first grid point at or after the event, as if the task had
    /// been polling for it every step.
    pub async fn after<F>(&self, event: Untimed<F>)
    where
        F: Future<Output = ()>,
    {
        event.raw_fut().await;

        let cursor = self.cursor.get();
        let dt = self.now.get().saturating_sub(cursor);

        let step_size = D::CYCLE_ALIGNMENT;

        let steps = (dt as f64 / step_size as f64).ceil() as Cycles;
        self.cursor.set(cursor + steps * step_size);

        // Not a no-op: the deadline is the cursor rounded up. However, moving just the
        // cursor only shifts later deadlines (written by future `Timeline::wait`s), meaning
        // the code right after this function returns could execute misaligned with the
        // tasks cycle grid (for example a halted Cpu waiting on an interrupt, woken by the
        // Ppu running on a different time domain)
        self.wait(0).await
    }
}

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

struct Slot {
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
}

fn slot_of<'a>(ctx: &'a Context<'_>) -> Option<&'a Slot> {
    let waker = ctx.waker();
    if ptr::eq(waker.vtable(), &VTABLE) {
        return Some(unsafe { &*(waker.data() as *const Slot) });
    }
    None
}

#[must_use = "does nothing unless awaited"]
pub struct Wait<D: TimeDomain> {
    tag: PhantomData<D>,
    now: Rc<Cell<Cycles>>,
    at: Cycles,
}

impl<D> Future for Wait<D>
where
    D: TimeDomain,
{
    type Output = ();
    fn poll(self: Pin<&mut Self>, ctx: &mut Context) -> Poll<()> {
        if self.now.get() >= self.at {
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

#[must_use = "does nothing unless awaited"]
pub struct Until<'a, F: Fn() -> bool> {
    signal: &'a Signal,
    predicate: F,
}

impl<'a, F> Until<'a, F>
where
    F: Fn() -> bool,
{
    fn register(&self, waker: &Waker) {
        let new = match self.signal.0.take() {
            Some(old) if old.will_wake(waker) => old,
            old => {
                debug_assert!(
                    old.is_none(),
                    "Signal `until` can only have a single waiter"
                );
                waker.clone()
            }
        };
        self.signal.store_waker(new);
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
        self.signal.0.take();
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

pub struct Signal(Cell<Option<Waker>>);

impl Signal {
    pub fn new() -> Self {
        Self(Cell::new(None))
    }

    pub fn store_waker(&self, waker: Waker) {
        self.0.set(Some(waker));
    }

    pub fn raise(&self) {
        if let Some(waker) = self.0.take() {
            waker.wake();
        }
    }

    pub fn until<F: Fn() -> bool>(&self, predicate: F) -> Untimed<Until<'_, F>> {
        let until = Until {
            signal: self,
            predicate: predicate,
        };
        Untimed { fut: until }
    }
}

#[derive(Clone)]
pub struct Pulse {
    count: Rc<Cell<u32>>,
    signal: Rc<Signal>,
}

impl Pulse {
    pub fn new() -> Self {
        Self {
            count: Rc::new(0.into()),
            signal: Rc::new(Signal::new()),
        }
    }

    pub fn raise(&self) {
        self.count.set(self.count.get() + 1);
        self.signal.raise();
    }

    pub fn next(&self) -> Untimed<Until<'_, impl Fn() -> bool + use<'_>>> {
        self.signal.until(|| {
            let count = self.count.get();
            self.count.set(count.saturating_sub(1));
            count > 0
        })
    }
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

    //
    pub fn should_poll(&self, now: [Cycles; 2]) -> bool {
        self.slot.notified.get()
            || self
                .slot
                .wake_at
                .iter()
                .zip(now)
                .any(|(wake, now)| wake.get() <= now)
    }

    pub fn poll(&mut self) {
        self.slot.notified.set(false);
        self.slot
            .wake_at
            .iter()
            .for_each(|wake| wake.set(Cycles::MAX));

        let _ = self
            .fut
            .as_mut()
            .poll(&mut Context::from_waker(&self.waker));
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]

pub enum Speed {
    #[default]
    Normal = 0,
    Double = 1,
}

impl From<Speed> for u8 {
    fn from(value: Speed) -> Self {
        value as u8
    }
}

struct Counter {
    now: Rc<Cell<Cycles>>,
    stopped: Cell<bool>,
}

impl Counter {
    fn new() -> Self {
        Self {
            now: Rc::new(Cell::new(0)),
            stopped: Cell::new(false),
        }
    }

    fn advance(&self, n: Cycles) {
        if !self.stopped.get() {
            self.now.set(self.now.get() + n);
        }
    }

    /// A task's cursor into this counter, starting where the counter is now.
    fn timeline<D: TimeDomain>(&self) -> Timeline<D> {
        Timeline {
            tag: PhantomData::<D>,
            now: self.now.clone(),
            cursor: Rc::new(Cell::new(self.now())),
        }
    }

    fn now(&self) -> Cycles {
        self.now.get()
    }
    fn stop(&self) {
        self.stopped.set(true);
    }
    fn resume(&self) {
        self.stopped.set(false);
    }
}

/// Dot clock. Never changes rate; the PPU and APU are clocked to it.
pub struct FixedClock(Counter);

impl FixedClock {
    pub fn new() -> Rc<Self> {
        Rc::new(Self(Counter::new()))
    }
    fn tick(&self) {
        self.0.advance(1);
    }
    fn timeline(&self) -> Timeline<Fixed> {
        self.0.timeline()
    }
    pub fn stop(&self) {
        self.0.stop();
    }
    pub fn resume(&self) {
        self.0.resume();
    }
}

/// CPU clock. Doubles in CGB double speed, and carries the state derived from
/// it: the divider, and the speed switch that changes its own rate.
pub struct CpuClock {
    counter: Counter,
    div_epoch: Cell<Cycles>,
    speed: Cell<Speed>,
    switch_armed: Cell<bool>,
}

impl CpuClock {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            counter: Counter::new(),
            div_epoch: Cell::new(0),
            speed: Cell::default(),
            switch_armed: Cell::new(false),
        })
    }

    fn tick(&self) {
        self.counter.advance(1 << self.speed.get() as u8);
    }
    fn timeline(&self) -> Timeline<Cpu> {
        self.counter.timeline()
    }
    pub fn resume(&self) {
        self.counter.resume();
    }

    /// STOP and the speed-switch stall both land here. Freezing the counter
    /// freezes DIV with it, since DIV is derived from it.
    pub fn stop(&self) {
        self.counter.stop();
        self.reset_div();
    }

    pub fn div(&self) -> u16 {
        self.counter.now().wrapping_sub(self.div_epoch.get()) as u16
    }
    pub fn reset_div(&self) {
        self.div_epoch.set(self.counter.now());
    }

    pub fn arm(&self, value: bool) {
        self.switch_armed.set(value);
    }
    pub fn switch_armed(&self) -> bool {
        self.switch_armed.get()
    }
    pub fn key1(&self) -> u8 {
        0x7E | (self.speed.get() as u8) << 7 | self.switch_armed.get() as u8
    }
    pub fn speed(&self) -> Speed {
        self.speed.get()
    }
    pub fn switch_speed(&self) {
        self.speed.set(match self.speed.get() {
            Speed::Normal => Speed::Double,
            Speed::Double => Speed::Normal,
        });
        self.switch_armed.set(false);
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

        let _ = task.poll();

        self.tasks.push(task);
    }

    /// Every task, in spawn order, unconditionally. Order is savestate contract.
    pub fn poll(&mut self, now: [Cycles; 2]) {
        for task in self.tasks.iter_mut() {
            if task.should_poll(now) {
                let _ = task.poll();
            }
        }
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
        (*slot).notified.set(true);
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
        self.fixed.tick();
        self.cpu.tick();
        self.exec.poll([self.cpu.counter.now(), self.fixed.0.now()]);
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

    pub fn resume(&self) {
        self.cpu.resume();
        self.fixed.resume();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::future::Race;
    use std::{cell::RefCell, future::pending, pin::pin};

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
        signal: Rc<Signal>,
    }

    impl Flag {
        fn new() -> Self {
            Self {
                level: Rc::new(Cell::new(false)),
                signal: Rc::new(Signal::new()),
            }
        }

        fn set(&self) {
            self.level.set(true);
            self.signal.raise();
        }

        fn until(&self) -> Untimed<Until<'_, impl Fn() -> bool + '_>> {
            self.signal.until(|| self.level.get())
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
    fn spawn_waiter(clock: &mut Time, flag: Flag, woke: Log) {
        let fixed = clock.fixed.clone();
        clock.spawn(move |_: Timeline<Fixed>| async move {
            flag.until().raw_fut().await;
            woke.borrow_mut().push(fixed.0.now());
            pending::<Infallible>().await
        });
    }

    /// Parks on `flag` through `after`, logs its position, then logs it again
    /// one M-cycle later.
    fn spawn_realigner(clock: &mut Time, flag: Flag, log: Log) {
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
    fn div_is_derived_and_resettable() {
        let mut clock = Time::new();

        clock_tick(&mut clock, 0x120);
        assert_eq!(clock.cpu.div(), 0x120);

        clock.cpu.reset_div();
        assert_eq!(clock.cpu.div(), 0);

        clock_tick(&mut clock, 8);
        assert_eq!(clock.cpu.div(), 8);
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
        let flag = Flag::new();
        let polls = Rc::new(Cell::new(0));

        let (f, p) = (flag.clone(), polls.clone());
        clock.spawn(move |_: Timeline<Fixed>| {
            PollCounter::new(
                async move {
                    f.until().raw_fut().await;
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
    fn pulse_replays_every_edge_then_parks() {
        let mut clock = Time::new();
        let pulse = Pulse::new();
        let raised = Rc::new(Cell::new(0u32));
        let steps = Rc::new(Cell::new(0u32));
        let polls = Rc::new(Cell::new(0));

        let (pl, r, s, p) = (pulse.clone(), raised.clone(), steps.clone(), polls.clone());
        clock.spawn(move |_: Timeline<Fixed>| {
            PollCounter::new(
                async move {
                    loop {
                        pl.next().raw_fut().await;
                        s.set(s.get() + 1);
                        // An edge that is never consumed would spin here forever.
                        assert!(s.get() <= r.get(), "stepped more often than raised");
                    }
                },
                p,
            )
        });

        clock_tick(&mut clock, 10);
        assert_eq!(steps.get(), 0, "stepped without an edge");

        for _ in 0..3 {
            raised.set(raised.get() + 1);
            pulse.raise();
        }
        clock_tick(&mut clock, 1);
        assert_eq!(steps.get(), 3, "edges not all replayed in one poll");

        let polls_after = polls.get();
        clock_tick(&mut clock, 10);
        assert_eq!(steps.get(), 3);
        assert_eq!(polls.get(), polls_after, "consumer did not park again");
    }

    #[test]
    fn raise_from_earlier_task_lands_in_the_same_pass() {
        let mut clock = Time::new();
        let flag = Flag::new();
        let woke = log();

        spawn_raiser(&mut clock, flag.clone(), 8);
        spawn_waiter(&mut clock, flag, woke.clone());

        clock_tick(&mut clock, 8);
        assert_eq!(*woke.borrow(), vec![8]);
    }

    #[test]
    fn raise_from_later_task_lands_on_the_next_tick() {
        let mut clock = Time::new();
        let flag = Flag::new();
        let woke = log();

        spawn_waiter(&mut clock, flag.clone(), woke.clone());
        spawn_raiser(&mut clock, flag, 8);

        clock_tick(&mut clock, 8);
        assert!(woke.borrow().is_empty());

        clock_tick(&mut clock, 1);
        assert_eq!(*woke.borrow(), vec![9]);
    }

    #[test]
    fn raise_before_parking_is_not_lost() {
        let mut clock = Time::new();
        let flag = Flag::new();
        let woke = log();
        let fixed = clock.fixed.clone();

        let (f, w) = (flag.clone(), woke.clone());
        clock.spawn(move |t: Timeline<Fixed>| async move {
            t.wait(8).await;
            f.until().raw_fut().await;
            w.borrow_mut().push(fixed.0.now());
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
        let flag = Flag::new();
        let woke = log();
        let polls = Rc::new(Cell::new(0));
        let fixed = clock.fixed.clone();

        let (f, w, p) = (flag.clone(), woke.clone(), polls.clone());
        clock.spawn(move |_: Timeline<Fixed>| {
            PollCounter::new(
                async move {
                    f.until().raw_fut().await;
                    w.borrow_mut().push(fixed.0.now());
                    pending::<Infallible>().await
                },
                p,
            )
        });

        // Raised with the level still low.
        flag.signal.raise();
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
        let flag = Flag::new();
        let ticks = log();
        let woke = log();
        let fixed = clock.fixed.clone();

        let (f, tl, w) = (flag.clone(), ticks.clone(), woke.clone());
        clock.spawn(move |t: Timeline<Fixed>| async move {
            let background = pin!(ticker(t, 4, tl));
            let main = pin!(f.until().raw_fut());
            Race::new(main, background).await;
            w.borrow_mut().push(fixed.0.now());
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
        let flag = Flag::new();
        let log = log();
        spawn_realigner(&mut clock, flag.clone(), log.clone());

        clock_tick(&mut clock, 5);
        flag.set();
        // Woken at 6, between grid points 4 and 8.
        clock_tick(&mut clock, 1);
        assert_eq!(*log.borrow(), vec![8]);

        clock_tick(&mut clock, 5);
        assert_eq!(*log.borrow(), vec![8]);
        clock_tick(&mut clock, 1);
        assert_eq!(*log.borrow(), vec![8, 12]);
    }

    #[test]
    fn after_on_a_grid_point_keeps_the_position() {
        let mut clock = Time::new();
        let flag = Flag::new();
        let log = log();
        // Spawned first, so its raise at 8 reaches the waiter in the same pass.
        spawn_raiser(&mut clock, flag.clone(), 8);
        spawn_realigner(&mut clock, flag, log.clone());

        clock_tick(&mut clock, 8);
        assert_eq!(*log.borrow(), vec![8]);

        clock_tick(&mut clock, 4);
        assert_eq!(*log.borrow(), vec![8, 12]);
    }

    #[test]
    fn after_rounds_up_in_double_speed() {
        let mut clock = Time::new();
        clock.cpu.switch_speed();
        let flag = Flag::new();
        let log = log();
        spawn_realigner(&mut clock, flag.clone(), log.clone());

        clock_tick(&mut clock, 2);
        flag.set();
        // Woken at 6: half an M-cycle past the grid point at 4.
        clock_tick(&mut clock, 1);
        assert_eq!(*log.borrow(), vec![8]);

        clock_tick(&mut clock, 2);
        assert_eq!(*log.borrow(), vec![8]);
        clock_tick(&mut clock, 1);
        assert_eq!(*log.borrow(), vec![8, 12]);
    }

    #[test]
    fn after_keeps_the_task_phase() {
        let mut clock = Time::new();
        clock_tick(&mut clock, 1);
        let flag = Flag::new();
        let log = log();
        // Spawned at 1: this task's grid is 1, 5, 9, 13...
        spawn_realigner(&mut clock, flag.clone(), log.clone());

        clock_tick(&mut clock, 5);
        flag.set();
        clock_tick(&mut clock, 1);
        assert_eq!(*log.borrow(), vec![9]);

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
        let flag = Flag::new();

        let f = flag.clone();
        clock.spawn(move |t: Timeline<Cpu>| async move {
            f.until().raw_fut().await;
            t.wait(M_CYCLE).await;
            pending::<Infallible>().await
        });

        clock_tick(&mut clock, 10);
        flag.set();
        clock_tick(&mut clock, 1);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "Signal")]
    fn second_waiter_on_a_signal_is_rejected() {
        let mut clock = Time::new();
        let flag = Flag::new();

        for _ in 0..2 {
            let f = flag.clone();
            clock.spawn(move |_: Timeline<Fixed>| async move {
                f.until().raw_fut().await;
                pending::<Infallible>().await
            });
        }
    }

    #[test]
    fn waker_refcount_stays_balanced() {
        let mut clock = Time::new();
        let flag = Flag::new();

        let f = flag.clone();
        clock.spawn(move |_: Timeline<Fixed>| async move {
            f.until().raw_fut().await;
            pending::<Infallible>().await
        });

        let slot = clock.exec.tasks[0].slot.clone();
        // The task's handle, its waker, the clone parked in the signal, and ours.
        assert_eq!(Rc::strong_count(&slot), 4);

        // Repolling the same task keeps the stored waker rather than cloning.
        for _ in 0..5 {
            slot.notified.set(true);
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
