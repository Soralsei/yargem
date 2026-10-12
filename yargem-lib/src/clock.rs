use std::cell::Cell;
use std::convert::Infallible;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

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

pub(crate) trait TimeDomain {}

impl TimeDomain for Cpu {}
impl TimeDomain for Fixed {}

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
    pub fn wait(&self, n: Cycles) -> Wait {
        let wake_at = self.cursor.get().wrapping_add(n);
        self.cursor.set(wake_at);
        Wait {
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

#[must_use = "does nothing unless awaited"]
pub struct Wait {
    now: Rc<Cell<Cycles>>,
    at: Cycles,
}

impl Future for Wait {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _: &mut Context) -> Poll<()> {
        if self.now.get() >= self.at {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

#[derive(Clone, Default)]
pub struct Pulse(Rc<Cell<u32>>);

impl Pulse {
    pub fn new() -> Self {
        Self(Rc::new(Cell::new(0u32)))
    }

    pub fn raise(&self) {
        self.0.set(self.0.get() + 1);
    }

    #[must_use = "does nothing unless awaited"]
    pub fn next(&self) -> impl Future<Output = ()> + '_ {
        std::future::poll_fn(|_| match self.0.get() {
            0 => Poll::Pending,
            n => {
                self.0.set(n - 1);
                Poll::Ready(())
            }
        })
    }
}

#[derive(Clone, Default)]
pub struct NotifyHandle(Rc<Cell<bool>>);

impl NotifyHandle {
    pub fn notify(&self) {
        self.0.set(true);
    }
}

/// Tasks never complete: `Infallible` makes that checked rather than conventional.
struct Task {
    fut: BoxFuture<Infallible>,
    slot: Rc<Slot>,
}

impl Task {
    pub fn new(fut: BoxFuture<Infallible>) -> Self {
        Self {
            fut,
            slot: Rc::new(Slot::new()),
        }
    }

    //
    pub fn should_poll(&self, now: [Cycles; 2]) -> bool {
        self.slot.notified.get()
            || self
                .slot
                .wake_at
                .iter()
                .zip(now)
                .any(|(wake, now)| wake.get() >= now)
    }

    pub fn poll(&mut self, cx: &mut Context) {
        self.slot.notified.set(false);
        self.slot
            .wake_at
            .iter()
            .for_each(|wake| wake.set(Cycles::MAX));

        let _ = self.fut.as_mut().poll(cx);
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
    pub fn tick(&self) {
        self.0.advance(1);
    }
    pub fn timeline(&self) -> Timeline<Fixed> {
        self.0.timeline()
    }
    pub fn stop(&self) {
        self.0.stop();
    }
    pub fn resume(&self) {
        self.0.resume();
    }
    pub fn now_cell(&self) -> Rc<Cell<Cycles>> {
        self.0.now.clone()
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

    pub fn now_cell(&self) -> Rc<Cell<Cycles>> {
        self.counter.now.clone()
    }

    pub fn tick(&self) {
        self.counter.advance(1 << self.speed.get() as u8);
    }
    pub fn timeline(&self) -> Timeline<Cpu> {
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

impl Executor {
    pub fn new() -> Self {
        Self {
            tasks: Vec::with_capacity(10),
        }
    }

    /// Polled once here so the task reaches its first `await` at the current
    /// cycle rather than the next tick's.
    pub fn spawn(&mut self, future: impl Future<Output = Infallible> + 'static) {
        let mut future = Box::pin(future);
        let _ = future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()));

        let task = Task::new(future);

        self.tasks.push(task);
    }

    /// Every task, in spawn order, unconditionally. Order is savestate contract.
    pub fn poll(&mut self) {
        let mut cx = Context::from_waker(Waker::noop());
        for task in self.tasks.iter_mut() {
            let _ = task.poll(&mut cx);
        }
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
        self.exec.poll();
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

    pub fn stop(&self) {
        self.cpu.stop();
        self.fixed.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    async fn ticker<D: TimeDomain>(
        t: Timeline<D>,
        period: Cycles,
        log: Rc<RefCell<Vec<Cycles>>>,
    ) -> Infallible {
        loop {
            t.wait(period).await;
            log.borrow_mut().push(t.position());
        }
    }

    fn clock_tick(clock: &mut Time, ticks: u32) {
        for _ in 0..ticks {
            clock.tick();
        }
    }

    fn spawn_ticker<D>(clock: &mut Time, period: Cycles) -> Rc<RefCell<Vec<Cycles>>>
    where
        Timeline<D>: FromTask,
        D: TimeDomain + 'static,
    {
        let log: Rc<RefCell<Vec<Cycles>>> = Rc::new(RefCell::new(Vec::new()));
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
}
