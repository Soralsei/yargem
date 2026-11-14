mod counter;
mod timeline;

use std::{
    cell::{Cell, RefCell},
    convert::Infallible,
    rc::Rc,
};

pub use timeline::*;

pub use crate::clock::counter::{Cycles, M_CYCLE};
use crate::{
    clock::counter::Counter,
    executor::{signal, Listener, Raiser, Until, Untimed},
    input::InputLine,
    memory::mmu::{MemoryContext, MemoryHandler},
};

impl From<Speed> for u8 {
    fn from(value: Speed) -> Self {
        value as u8
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Speed {
    #[default]
    Normal = 0,
    Double = 1,
}

/// Dot clock. Never changes rate; the PPU and APU are clocked to it.
pub struct FixedClock(Counter);

impl FixedClock {
    pub fn new() -> Rc<Self> {
        Rc::new(Self(Counter::new()))
    }
    pub fn tick(&self, ticks: Cycles) {
        self.0.advance(ticks);
    }
    pub fn timeline(&self) -> Timeline<Fixed> {
        self.0.timeline()
    }
    pub fn now(&self) -> Cycles {
        self.0.now()
    }
    pub fn stop(&self) {
        self.0.stop();
    }
    pub fn resume(&self) {
        self.0.resume();
    }
    pub fn stopped(&self) -> bool {
        self.0.stopped()
    }
}

/// CPU clock. Doubles in CGB double speed
pub struct CpuClock {
    counter: Counter,
    speed: Cell<Speed>,
    switch_armed: Rc<Cell<bool>>,
}

impl CpuClock {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            counter: Counter::new(),
            speed: Cell::default(),
            switch_armed: Rc::new(Cell::new(false)),
        })
    }

    pub fn tick(&self, ticks: Cycles) {
        self.counter.advance(ticks << self.speed.get() as u8);
    }
    pub fn timeline(&self) -> Timeline<Cpu> {
        self.counter.timeline()
    }
    pub fn resume(&self) {
        self.counter.resume();
    }

    pub fn now(&self) -> Cycles {
        self.counter.now()
    }

    /// STOP and the speed-switch stall both land here.
    pub fn stop(&self) {
        self.counter.stop();
    }
    pub fn stopped(&self) -> bool {
        self.counter.stopped()
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RequestState {
    Idle,
    Stop,
    SpeedSwitch,
}

/// CPU side of the line
pub struct OscillatorRequest {
    request: Raiser,
    released: Listener,
    request_state: Rc<Cell<RequestState>>,
    switch_armed: Rc<Cell<bool>>,
}

impl OscillatorRequest {
    pub fn speed_switch(&mut self) -> Untimed<Until<'_, impl Fn() -> bool + '_>> {
        self.request_state.set(RequestState::SpeedSwitch);
        let state = self.request_state.clone();
        self.request.raise();
        self.released
            .until(move || state.get() == RequestState::Idle)
    }

    pub fn stop_mode(&mut self) -> Untimed<Until<'_, impl Fn() -> bool + '_>> {
        self.request_state.set(RequestState::Stop);
        let state = self.request_state.clone();
        self.request.raise();
        self.released
            .until(move || state.get() == RequestState::Idle)
    }

    pub fn switch_armed(&self) -> bool {
        self.switch_armed.get()
    }
}

/// Oscillator side of the line
pub struct OscillatorLine {
    requested: Listener,
    release: Raiser,
    request_state: Rc<Cell<RequestState>>,
}

impl OscillatorLine {
    pub fn until_requested(&mut self) -> Untimed<Until<'_, impl Fn() -> bool + '_>> {
        let state = self.request_state.clone();
        self.requested
            .until(move || state.get() != RequestState::Idle)
    }

    pub fn release(&self) {
        self.request_state.set(RequestState::Idle);
        self.release.raise();
    }
}

pub struct Oscillator {
    clock: Rc<CpuClock>,
    fixed: Rc<FixedClock>,
}

impl Oscillator {
    pub fn new(clock: Rc<CpuClock>, fixed: Rc<FixedClock>) -> Self {
        Self { clock, fixed }
    }

    pub fn line(&self) -> (OscillatorRequest, OscillatorLine) {
        let (request, requested) = signal();
        let (release, released) = signal();
        let state = Rc::new(Cell::new(RequestState::Idle));
        (
            OscillatorRequest {
                request,
                released,
                request_state: state.clone(),
                switch_armed: self.clock.switch_armed.clone(),
            },
            OscillatorLine {
                requested,
                release,
                request_state: state,
            },
        )
    }

    pub async fn task(
        this: Rc<Self>,
        mut line: OscillatorLine,
        mut input_line: InputLine,
        t: Timeline<Fixed>,
    ) -> Infallible {
        loop {
            t.after(line.until_requested()).await;
            match line.request_state.get() {
                RequestState::SpeedSwitch => {
                    this.clock.switch_speed();
                    this.clock.stop();
                    t.wait(2050 * M_CYCLE).await;
                    this.clock.resume();
                }
                RequestState::Stop => {
                    this.clock.stop();
                    this.fixed.stop();
                    t.after(input_line.until_input()).await;
                    this.clock.resume();
                    this.fixed.resume();
                }
                RequestState::Idle => unreachable!(),
            }
            line.release();
        }
    }
}

impl MemoryHandler for Oscillator {
    fn read(&self, _address: u16, _ctx: MemoryContext<'_>) -> u8 {
        self.clock.key1()
    }

    fn write(&self, _address: u16, value: u8, _ctx: MemoryContext<'_>) {
        self.clock.arm(value & 1 != 0);
    }
}
