use std::{
    cell::{Cell, RefCell},
    convert::Infallible,
    rc::Rc,
};

use crate::{
    clock::{edge, Cpu, CpuClock, Cycles, EdgeListener, EdgeRaiser, Timeline, M_CYCLE},
    cpu::interrupt::InterruptRequest,
    is_bit_set,
    memory::mmu::{MemoryHandler, MemoryRead, MemoryWrite},
};
const TAC_ENABLE: u8 = 2;
const TIMA_PERIODS: [u16; 4] = [1024, 16, 64, 256];

/// The timer is only ever touched by its own `step` and its own register
/// handler, which never nest, so one cell at the device boundary is enough and
/// all of the logic below keeps plain `&mut self`.
pub struct Timer {
    div_apu: EdgeRaiser,
    /// Handed to the APU frame sequencer, the edge's only consumer.
    div_apu_listener: Cell<Option<EdgeListener>>,
    clock: Rc<CpuClock>,
    state: RefCell<TimerState>,
}

impl Timer {
    pub fn new(interrupt_request: InterruptRequest, clock: Rc<CpuClock>, is_cgb: bool) -> Self {
        let (div_apu, div_apu_listener) = edge();
        Self {
            clock,
            div_apu,
            div_apu_listener: Cell::new(Some(div_apu_listener)),
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

    /// DIV-APU: bit 4 of DIV (bit 12 of the counter) falling, at 512 Hz. Double
    /// speed moves it to bit 13 so the rate holds.
    fn div_apu_edge(&self, previous: u16, next: u16) {
        let bit = 0x1000 << u8::from(self.clock.speed());
        if previous & !next & bit != 0 {
            self.div_apu.raise();
        }
    }

    pub fn div_apu(&self) -> EdgeListener {
        self.div_apu_listener
            .take()
            .expect("DIV-APU listener already taken")
    }

    pub async fn task(this: Rc<Self>, timeline: Timeline<Cpu>) -> Infallible {
        loop {
            timeline.wait(M_CYCLE as Cycles).await;
            let mut state = this.state.borrow_mut();
            let div = this.clock.div();
            state.advance_to(div);
            this.div_apu_edge(div.wrapping_sub(M_CYCLE as u16), div);
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
    fn enabled(tac: u8) -> bool {
        is_bit_set!(tac, TAC_ENABLE)
    }

    fn selected_bit(counter: u16, tac: u8) -> bool {
        counter & (TIMA_PERIODS[(tac & 0b11) as usize] >> 1) != 0
    }

    /// One M-cycle of counter advance: the pending TMA reload, then a falling
    /// edge of the selected bit between `counter - M_CYCLE` and `counter`.
    fn advance_to(&mut self, counter: u16) {
        if self.overflowed {
            self.tima = self.tma;
            self.overflowed = false;
            self.interrupt_request.timer(true);
        }
        self.state_change(counter.wrapping_sub(M_CYCLE as u16), counter, self.tac);
    }

    /// Sample the selected bit either side of a change to the counter or TAC.
    /// The counter is passed in rather than read twice from the clock: a write
    /// changes it under the detector, so both samples must be explicit.
    fn state_change(&mut self, previous_counter: u16, next_counter: u16, next_tac: u8) {
        let previous_active = Self::enabled(self.tac);
        let previous_selected_set = Self::selected_bit(previous_counter, self.tac);

        self.tac = next_tac;

        let current_active = Self::enabled(self.tac);
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
    fn read(&self, _: &crate::memory::mmu::Mmu, address: u16) -> crate::memory::mmu::MemoryRead {
        let state = self.state.borrow();
        match address {
            0xFF04 => MemoryRead::Replace((self.clock.div() >> 8) as u8),
            0xFF05 => MemoryRead::Replace(state.tima),
            0xFF06 => MemoryRead::Replace(state.tma),
            0xFF07 => MemoryRead::Replace(state.tac),
            _ => MemoryRead::Pass,
        }
    }

    fn write(
        &self,
        _: &crate::memory::mmu::Mmu,
        address: u16,
        value: u8,
    ) -> crate::memory::mmu::MemoryWrite {
        let mut state = self.state.borrow_mut();
        match address {
            0xFF04 => {
                let previous = self.clock.div();
                let tac = state.tac;
                self.clock.reset_div();
                state.state_change(previous, 0, tac);
                self.div_apu_edge(previous, 0);
                return MemoryWrite::Block;
            }
            0xFF05 => {
                state.tima = value;
                state.overflowed = false;
            }
            0xFF06 => state.tma = value,
            0xFF07 => {
                let counter = self.clock.div();
                state.state_change(counter, counter, value & 0b111);
            }
            _ => {}
        }
        MemoryWrite::Pass
    }
}
