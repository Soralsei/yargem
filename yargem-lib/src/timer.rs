use std::{
    cell::{Cell, RefCell},
    convert::Infallible,
    pin::pin,
    rc::Rc,
};

use crate::{
    clock::{edge, Cpu, CpuClock, Cycles, EdgeListener, EdgeRaiser, Timeline, M_CYCLE},
    cpu::interrupt::InterruptRequest,
    is_bit_set,
    memory::mmu::{MemoryHandler, MemoryRead, MemoryWrite, Next},
    util::future::Race,
};
const TAC_ENABLE: u8 = 2;
const TIMA_PERIODS: [u16; 4] = [1024, 16, 64, 256];

/// Cycles from `counter` to the next multiple of `period` strictly after it:
/// where bit `period / 2` falls.
fn until_fall(counter: u16, period: u16) -> u16 {
    period - (counter & (period - 1))
}

/// The timer is only ever touched by its own `task` and its own register
/// handler, which never nest, so one cell at the device boundary is enough and
/// all of the logic below keeps plain `&mut self`.
pub struct Timer {
    div_epoch: Cell<Cycles>,
    /// Raised on the writes that move the next edge, so the task re-plans.
    writes: EdgeRaiser,
    /// Taken by the task when it starts: it is the only consumer.
    writes_listener: Cell<Option<EdgeListener>>,
    div_apu: EdgeRaiser,
    /// Handed to the APU frame sequencer, the edge's only consumer.
    div_apu_listener: Cell<Option<EdgeListener>>,
    clock: Rc<CpuClock>,
    state: RefCell<TimerState>,
}

impl Timer {
    pub fn new(interrupt_request: InterruptRequest, clock: Rc<CpuClock>, is_cgb: bool) -> Self {
        let (writes, writes_listener) = edge();
        let (div_apu, div_apu_listener) = edge();
        Self {
            div_epoch: 0.into(),
            writes,
            writes_listener: Cell::new(Some(writes_listener)),
            div_apu,
            div_apu_listener: Cell::new(Some(div_apu_listener)),
            clock,
            state: RefCell::new(TimerState {
                interrupt_request,
                tima: 0,
                tma: 0,
                tac: 0,
                overflowed: false,
                is_cgb,
            }),
        }
    }

    pub fn div(&self) -> u16 {
        self.clock.now().saturating_sub(self.div_epoch.get()) as u16
    }

    fn reset_div(&self) {
        self.div_epoch.set(self.clock.now())
    }
    fn div_before(&self, n: Cycles) -> u16 {
        let now = self.clock.now();
        now.saturating_sub(n)
            .max(self.div_epoch.get())
            .wrapping_sub(self.div_epoch.get()) as u16
    }

    /// DIV-APU: bit 4 of DIV (bit 12 of the counter) falling, at 512 Hz. Double
    /// speed moves it to bit 13 so the rate holds.
    fn div_apu_bit(&self) -> u16 {
        0x1000 << u8::from(self.clock.speed())
    }

    fn div_apu_edge(&self, previous: u16, next: u16) {
        if previous & !next & self.div_apu_bit() != 0 {
            self.div_apu.raise();
        }
    }

    pub fn div_apu(&self) -> EdgeListener {
        self.div_apu_listener
            .take()
            .expect("DIV-APU listener already taken")
    }

    /// Sleeps to the next M-cycle with work, then runs that M-cycle exactly as
    /// if it had been stepping through every one. A DIV or TAC write moves
    /// that point, so it cuts the sleep short: the write's own instant belongs
    /// to the handler, and `after` resumes the task on the M-cycle following it.
    pub async fn task(this: Rc<Self>, timeline: Timeline<Cpu>) -> Infallible {
        let mut writes = this
            .writes_listener
            .take()
            .expect("timer task spawned twice");
        loop {
            let n = this
                .state
                .borrow()
                .cycles_to_next_event(this.div(), this.div_apu_bit());

            let write = pin!(timeline.after(writes.next()));
            let deadline = pin!(timeline.wait(n as Cycles));

            Race::new(write, deadline).await;

            let div = this.div();
            let previous = this.div_before(M_CYCLE);
            this.state.borrow_mut().advance_to(previous, div);
            this.div_apu_edge(previous, div);
        }
    }
}

struct TimerState {
    interrupt_request: InterruptRequest,
    tima: u8, // address 0xFF05
    tma: u8,  // address 0xFF06
    tac: u8,  // address 0xFF07
    overflowed: bool,
    is_cgb: bool,
}

impl TimerState {
    fn enabled(&self) -> bool {
        is_bit_set!(self.tac, TAC_ENABLE)
    }

    fn tima_period(tac: u8) -> u16 {
        TIMA_PERIODS[(tac & 0b11) as usize]
    }

    fn selected_bit(counter: u16, tac: u8) -> bool {
        counter & (Self::tima_period(tac) >> 1) != 0
    }

    /// Cycles from `counter` to the next M-cycle with work: a TIMA increment,
    /// the reload after an overflow, or a DIV-APU edge.
    fn cycles_to_next_event(&self, counter: u16, div_apu_bit: u16) -> u16 {
        let mut n = until_fall(counter, div_apu_bit << 1);
        if self.enabled() {
            n = n.min(until_fall(counter, Self::tima_period(self.tac)));
        }
        if self.overflowed {
            n = n.min(M_CYCLE as u16);
        }
        n
    }

    /// One M-cycle of counter advance: the pending TMA reload, then a falling
    /// edge of the selected bit between `counter - M_CYCLE` and `counter`.
    fn advance_to(&mut self, previous: u16, counter: u16) {
        if self.overflowed {
            self.tima = self.tma;
            self.overflowed = false;
            self.interrupt_request.timer(true);
        }
        self.state_change(previous, counter, self.tac);
    }

    /// Sample the selected bit either side of a change to the counter or TAC.
    /// The counter is passed in rather than read twice from the clock: a write
    /// changes it under the detector, so both samples must be explicit.
    fn state_change(&mut self, previous_counter: u16, next_counter: u16, next_tac: u8) {
        let previous_active = self.enabled();
        let previous_selected_set = Self::selected_bit(previous_counter, self.tac);

        self.tac = next_tac;

        let current_active = self.enabled();
        let current_selected_set = Self::selected_bit(next_counter, self.tac);

        // Changing which bit of the system counter is selected (by changing the “Clock select”
        // bits of TAC) from a bit currently set to another that is currently unset, will send
        // a “Timer tick” pulse.
        let selected_edge_falling =
            previous_selected_set && !current_selected_set && current_active && previous_active;
        // On monochrome consoles, disabling the timer if the currently selected bit is set, will
        // send a “Timer tick” once. This does not happen on Color models.
        let dmg_set_disabled =
            !self.is_cgb && current_selected_set && !current_active && previous_active;
        // On CGB, a write to TAC that enables the timer while the current selected counter bit is
        // set will sometimes send a "Timer tick" (not always, depends on the individual console,
        // here, we do it but not required)
        let cgb_set_enabled =
            self.is_cgb && current_selected_set && !previous_active && current_active;

        if selected_edge_falling || dmg_set_disabled || cgb_set_enabled {
            // On overflow TIMA reads 0 for one M-cycle; `advance_to` does the
            // TMA reload and raises the interrupt on the next one.
            let (tima, overflowed) = self.tima.overflowing_add(1);
            self.tima = tima;
            self.overflowed |= overflowed;
        }
    }
}

impl MemoryHandler for Timer {
    fn read(&self, address: u16, _: Next<'_>) -> u8 {
        let state = self.state.borrow();
        match address {
            0xFF04 => (self.div() >> 8) as u8,
            0xFF05 => state.tima,
            0xFF06 => state.tma,
            0xFF07 => state.tac,
            _ => unreachable!("unhandled read in Timer at address {:04X}", address),
        }
    }

    fn write(&self, address: u16, value: u8, _: Next<'_>) {
        let mut state = self.state.borrow_mut();
        match address {
            0xFF04 => {
                let previous = self.div();
                let tac = state.tac;
                self.reset_div();
                state.state_change(previous, 0, tac);
                self.div_apu_edge(previous, 0);
            }
            0xFF05 => {
                state.tima = value;
                state.overflowed = false;
            }
            0xFF06 => state.tma = value,
            0xFF07 => {
                let counter = self.div();
                state.state_change(counter, counter, value & 0b111);
            }
            _ => unreachable!("unhandled write in Timer at address {:04X}", address),
        }
        // DIV and TAC decide when the next edge falls, and either can arm a
        // reload through a glitch tick. TIMA and TMA never move a deadline.
        if matches!(address, 0xFF04 | 0xFF07) {
            self.writes.raise();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Time;
    use crate::cpu::interrupt::InterruptController;
    use crate::memory::mmu::{AddressMap, Mmu};

    /// Drives the timer alone. Registers go through its own handler between
    /// ticks, which is where the CPU writes: after the timer's poll for that
    /// cycle.
    struct Harness {
        time: Time,
        timer: Rc<Timer>,
        irq: Rc<InterruptController>,
        // The handlers never read through the MMU; they only need one to call.
        mmu: Mmu,
    }

    impl Harness {
        fn new() -> Self {
            let mut time = Time::new();
            let irq = Rc::new(InterruptController::new());
            let timer = Rc::new(Timer::new(irq.request(), time.cpu.clone(), false));
            let t = timer.clone();
            time.spawn(move |timeline: Timeline<Cpu>| Timer::task(t, timeline));
            Self {
                time,
                timer,
                irq,
                mmu: Mmu::new(AddressMap::new()),
            }
        }

        fn write(&self, address: u16, value: u8) {
            let _ = self.timer.write(&self.mmu, address, value);
        }

        fn tima(&self) -> u8 {
            match self.timer.read(&self.mmu, 0xFF05) {
                MemoryRead::Replace(value) => value,
                MemoryRead::Pass => unreachable!(),
            }
        }

        fn timer_irq(&self) -> bool {
            match self.irq.read(&self.mmu, 0xFF0F) {
                MemoryRead::Replace(flags) => flags & 0b100 != 0,
                MemoryRead::Pass => unreachable!(),
            }
        }

        fn ticks(&mut self, n: u32) {
            for _ in 0..n {
                self.time.tick();
            }
        }
    }

    #[test]
    fn reload_and_interrupt_land_one_m_cycle_after_overflow() {
        let mut h = Harness::new();
        h.write(0xFF06, 0xF0);
        h.write(0xFF05, 0xFE);
        h.write(0xFF07, 0b101); // enabled, 16-cycle period

        // Increments at 16 and 32; the second overflows.
        h.ticks(32);
        assert_eq!(h.tima(), 0x00);
        assert!(!h.timer_irq(), "interrupt raised in the overflow cycle");

        h.ticks(3);
        assert_eq!(h.tima(), 0x00, "reloaded before the next M-cycle");
        h.ticks(1);
        assert_eq!(h.tima(), 0xF0);
        assert!(h.timer_irq());
    }

    #[test]
    fn div_apu_falls_every_0x2000_cycles_and_not_at_power_on() {
        let mut h = Harness::new();
        let steps = Rc::new(Cell::new(0u32));

        let mut div_apu = h.timer.div_apu();
        let s = steps.clone();
        h.time.spawn(async move |()| loop {
            div_apu.next().raw_fut().await;
            s.set(s.get() + 1);
        });

        h.ticks(0x2000 - 1);
        assert_eq!(steps.get(), 0);
        h.ticks(1);
        assert_eq!(steps.get(), 1);
        h.ticks(0x2000);
        assert_eq!(steps.get(), 2);
    }

    #[test]
    fn edge_on_the_m_cycle_after_a_tac_write_is_counted() {
        let mut h = Harness::new();
        h.ticks(12);
        // The 16-cycle bit is set at 12 and falls at 16: the M-cycle the
        // timer resumes on after waking for the write.
        h.write(0xFF07, 0b101);

        h.ticks(3);
        assert_eq!(h.tima(), 0);
        h.ticks(1);
        assert_eq!(h.tima(), 1);
    }

    #[test]
    fn div_is_derived_and_resettable() {
        let mut h = Harness::new();

        h.ticks(0x120);
        assert_eq!(h.timer.div(), 0x120);

        h.timer.reset_div();
        assert_eq!(h.timer.div(), 0);

        h.ticks(8);
        assert_eq!(h.timer.div(), 8);
    }

    #[test]
    fn tima_apu_does_not_clock_on_stopped_clock() {
        let mut h = Harness::new();

        let steps = Rc::new(Cell::new(0u32));
        let mut div_apu = h.timer.div_apu();
        let s = steps.clone();

        h.time.spawn(async move |()| loop {
            div_apu.next().raw_fut().await;
            s.set(s.get() + 1);
        });

        h.ticks(0x100);
        h.write(0xFF07, 0b101);
        h.ticks(4);
        h.time.cpu.stop();
        h.write(0xFF04, 0); // DIV = 0x104: bit 3 and bit 12 both clear, so no tick glitch
        h.ticks(0x50);

        assert_eq!(h.tima(), 0);
        assert_eq!(steps.get(), 0);

        h.time.cpu.resume();
        h.ticks(0x2000);

        assert_eq!(h.tima(), 0); // 512 wrapping around
        assert_eq!(steps.get(), 1);
    }
}
