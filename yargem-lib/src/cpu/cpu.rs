use std::cell::Cell;
use std::convert::Infallible;
use std::future;
use std::hint::cold_path;
use std::rc::Rc;

use super::registers::Registers;
use crate::clock::{self, OscillatorRequest, Timeline, M_CYCLE};
use crate::cpu::instructions::{execute_prefixed, execute_unprefixed};
use crate::cpu::interrupt::InterruptController;
use crate::cpu::registers::{Flags, Reg16, Reg8};
use crate::executor::Listener;
use crate::memory::bus::CpuBus;
use crate::system::PowerOn;
use crate::util::bit_operations::*;
use yargem_common::Condition;

macro_rules! t {
    ($self:expr) => {
        $self.t.as_ref().expect("missing timeline in Cpu")
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Ime {
    #[default]
    Disabled,
    Pending,
    Enabled,
}

#[derive(Debug)]
struct CpuState {
    registers: Cell<Registers>,
    ime: Cell<Ime>,
    boundary: Cell<Option<u16>>,
    halted: Cell<bool>,

    // Happens on CPU panic (unknown opcode decode wedges the cpu and stops fetches)
    wedged: Cell<bool>,
}

impl CpuState {
    fn new(power_on: PowerOn, is_cgb: bool) -> Self {
        let registers = match power_on {
            PowerOn::BootRom => Registers::default(),
            PowerOn::PostBoot => Registers::new(is_cgb),
        };
        CpuState {
            registers: Cell::new(registers),
            ime: Cell::new(Ime::Disabled),
            boundary: Cell::new(None),
            halted: Cell::new(false),
            wedged: Cell::new(false),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CpuView(Rc<CpuState>);

impl CpuView {
    pub fn registers(&self) -> Registers {
        self.0.registers.get()
    }

    pub fn take_boundary(&self) -> Option<u16> {
        self.0.boundary.take()
    }

    pub fn halted(&self) -> bool {
        self.0.halted.get()
    }

    pub fn wedged(&self) -> bool {
        self.0.wedged.get()
    }
}

pub struct Cpu {
    // lazily initialized in the run() method
    t: Option<Timeline<clock::Cpu>>,

    bus: CpuBus,
    irq: Rc<InterruptController>,
    irq_listener: Listener,

    oscillator_req: OscillatorRequest,

    state: Rc<CpuState>,

    halt_bug: bool,
}

impl Cpu {
    pub fn new(
        bus: CpuBus,
        irq: Rc<InterruptController>,
        oscillator_req: OscillatorRequest,
        is_cgb: bool,
        power_on: PowerOn,
    ) -> Cpu {
        let listener = irq.take_listener();
        Self {
            t: None,
            bus,
            irq,
            irq_listener: listener,
            oscillator_req,
            state: Rc::new(CpuState::new(power_on, is_cgb)),
            halt_bug: false,
        }
    }

    pub fn view(&self) -> CpuView {
        CpuView(self.state.clone())
    }

    pub async fn run(mut self, t: Timeline<clock::Cpu>) -> Infallible {
        self.t = Some(t);
        loop {
            self.step().await
        }
    }

    pub fn halt(&self) {
        self.state.halted.set(true);
    }

    pub fn set_ime(&self, active: Ime) {
        self.state.ime.set(active);
    }

    pub async fn step(&mut self) {
        if self.state.halted.get() {
            if self.state.ime.get() == Ime::Disabled && self.irq.peek().is_some() {
                // halt bug: PC fails to increment, next byte executes twice
                self.halt_bug = true;
            } else {
                let t = t!(self);
                t.after(self.irq_listener.until(|| self.irq.peek().is_some()))
                    .await;
            }
            self.state.halted.set(false);
        }

        if self.state.ime.get() == Ime::Enabled {
            if let Some(vector) = self.irq.consume() {
                self.service(vector).await;
            }
        }

        if self.state.ime.get() == Ime::Pending {
            self.state.ime.set(Ime::Enabled);
        }

        self.execute().await;
    }

    async fn execute(&mut self) {
        // Set current pc boundary before fetch
        self.state.boundary.set(Some(self.state.registers.get().pc));

        self.fetch(Reg8::Z).await;
        let opcode = self.state.registers.get().z;
        match opcode {
            0xCB => {
                self.fetch(Reg8::Z).await;
                let opcode = self.state.registers.get().z;
                execute_prefixed(self, opcode).await;
            }
            _ => execute_unprefixed(self, opcode).await,
        }
    }

    async fn service(&self, vector: u8) {
        self.idle().await;
        self.idle().await;

        let pc = self.state.registers.get().pc;
        let (msb, lsb) = word_to_bytes(pc);

        self.push(msb).await;
        self.push(lsb).await;

        self.state.ime.set(Ime::Disabled);

        self.state.registers.update(|mut regs| {
            regs.pc = vector as u16;
            regs
        });

        self.idle().await;
    }

    pub async fn fetch(&mut self, dst: Reg8) {
        t!(self).wait(M_CYCLE).await;

        let mut registers = self.state.registers.get();
        let pc = registers.pc;
        registers.write_u8(dst, self.bus.read(pc));

        let halt_bug = self.halt_bug;
        self.halt_bug = false;

        if !halt_bug {
            registers.pc = registers.pc.wrapping_add(1);
        } else {
            cold_path();
        }

        self.state.registers.set(registers);
    }

    pub async fn load(&self, dst: Reg8, addr: u16) {
        t!(self).wait(M_CYCLE).await;

        self.state.registers.update(|mut regs| {
            regs.write_u8(dst, self.bus.read(addr));
            regs
        });
    }

    pub async fn store(&self, address: u16, src: Reg8) {
        t!(self).wait(M_CYCLE).await;

        self.bus
            .write(address, self.state.registers.get().read_u8(src));
    }

    pub async fn store16(&self, addr: u16, src: Reg16) {
        let (msb, lsb) = word_to_bytes(self.state.registers.get().read_u16(src));

        t!(self).wait(M_CYCLE).await;
        self.bus.write(addr, lsb);

        t!(self).wait(M_CYCLE).await;
        self.bus.write(addr.wrapping_add(1), msb);
    }

    pub fn cond(&self, cond: Condition) -> bool {
        let flags = &self.state.registers.get().f;
        match cond {
            Condition::Unconditional => true,
            Condition::NotZero => !flags.zero,
            Condition::Zero => flags.zero,
            Condition::NotCarry => !flags.carry,
            Condition::Carry => flags.carry,
        }
    }

    pub fn high(&self, reg: Reg8) -> u16 {
        0xFF00 | self.state.registers.get().read_u8(reg) as u16
    }

    pub fn rel(&self, reg: Reg8) -> u16 {
        let pc = self.state.registers.get().pc;
        pc.wrapping_add_signed(self.state.registers.get().read_u8(reg) as i8 as i16)
    }

    pub fn jump(&self, addr: u16) {
        self.state.registers.update(|mut regs| {
            regs.pc = addr;
            regs
        });
    }

    pub async fn push(&self, value: u8) {
        t!(self).wait(M_CYCLE).await;

        let mut regs = self.state.registers.get();

        let new_sp = regs.sp.wrapping_sub(1);
        regs.sp = new_sp;

        self.state.registers.set(regs);

        self.bus.write(new_sp, value);
    }

    pub async fn pop(&self) -> u8 {
        t!(self).wait(M_CYCLE).await;

        let mut regs = self.state.registers.get();

        let sp = regs.sp;
        regs.sp = regs.sp.wrapping_add(1);

        self.state.registers.set(regs);

        self.bus.read(sp)
    }

    pub async fn push16(&self, value: u16) {
        let (msb, lsb) = word_to_bytes(value);
        self.push(msb).await;
        self.push(lsb).await;
    }

    pub async fn pop16(&self) -> u16 {
        let lsb = self.pop().await;
        let msb = self.pop().await;
        bytes_to_word(msb, lsb)
    }

    pub fn addr(&self, src: Reg16) -> u16 {
        self.state.registers.get().read_u16(src)
    }

    pub fn alu(&self, register: Reg8, op: impl FnOnce(&mut Flags, u8) -> u8) {
        self.state.registers.update(|mut regs| {
            let value = regs.read_u8(register);
            let result = op(&mut regs.f, value);

            regs.write_u8(register, result);

            regs
        });
    }

    pub fn alu2(&self, dst: Reg8, src: Reg8, op: impl FnOnce(&mut Flags, u8, u8) -> u8) {
        self.state.registers.update(|mut regs| {
            let (a, b) = (regs.read_u8(dst), regs.read_u8(src));
            let result = op(&mut regs.f, a, b);
            regs.write_u8(dst, result);

            regs
        });
    }

    pub fn alu16(&self, register: Reg16, op: impl FnOnce(&mut Flags, u16) -> u16) {
        self.state.registers.update(|mut regs| {
            let value = regs.read_u16(register);
            let result = op(&mut regs.f, value);
            regs.write_u16(register, result);

            regs
        });
    }

    pub fn alu16_2(
        &self,
        dst: Reg16,
        a: Reg16,
        b: Reg16,
        op: impl FnOnce(&mut Flags, u16, u16) -> u16,
    ) {
        self.state.registers.update(|mut regs| {
            let (a, b) = (regs.read_u16(a), regs.read_u16(b));
            let result = op(&mut regs.f, a, b);
            regs.write_u16(dst, result);

            regs
        });
    }

    /// mixed-width one, for `add sp,r8` and `ld hl,sp+r8`. `dst` is
    pub fn alu16_8(
        &self,
        dst: Reg16,
        a: Reg16,
        b: Reg8,
        op: impl FnOnce(&mut Flags, u16, u8) -> u16,
    ) {
        self.state.registers.update(|mut regs| {
            let (a, b) = (regs.read_u16(a), regs.read_u8(b));
            let result = op(&mut regs.f, a, b);
            regs.write_u16(dst, result);

            regs
        });
    }

    pub fn mov(&self, dst: Reg8, src: Reg8) {
        self.state.registers.update(|mut regs| {
            let val = regs.read_u8(src);
            regs.write_u8(dst, val);

            regs
        });
    }

    pub fn mov16(&self, dst: Reg16, src: Reg16) {
        self.state.registers.update(|mut regs| {
            let val = regs.read_u16(src);
            regs.write_u16(dst, val);

            regs
        });
    }

    pub fn set16(&self, dst: Reg16, value: u16) {
        self.state.registers.update(|mut regs| {
            regs.write_u16(dst, value);
            regs
        });
    }

    /// `bit b,r`: one register in, flags out, nothing written back. The bit
    /// index rides in the closure, since it comes from the opcode rather than
    /// the register file.
    pub fn test(&self, register: Reg8, op: impl FnOnce(&mut Flags, u8)) {
        self.state.registers.update(|mut regs| {
            let value = regs.read_u8(register);
            op(&mut regs.f, value);

            regs
        });
    }

    /// `cp`: two registers in, neither written.
    pub fn test2(&self, left: Reg8, right: Reg8, op: impl FnOnce(&mut Flags, u8, u8)) {
        self.state.registers.update(|mut regs| {
            let (a, b) = (regs.read_u8(left), regs.read_u8(right));
            op(&mut regs.f, a, b);

            regs
        });
    }

    pub fn flags(&self, op: impl FnOnce(&mut Flags)) {
        self.state.registers.update(|mut regs| {
            op(&mut regs.f);
            regs
        });
    }

    pub fn ei(&self) {
        self.set_ime(Ime::Pending);
    }

    pub fn ei_now(&self) {
        self.set_ime(Ime::Enabled);
    }

    pub fn di(&self) {
        self.set_ime(Ime::Disabled);
    }

    pub async fn stop(&mut self) {
        let int_enabled = self.bus.peek(0xFFFF);
        let int_req = self.bus.peek(0xFF0F);
        let interrupt_pending = int_enabled & int_req & 0x1F;

        let joyp = self.bus.peek(0xFF00) & 0x0F;

        // If a button is being held on a selected line in JOYP
        if joyp != 0x0F {
            // No pending interrupt
            if interrupt_pending == 0 {
                // STOP => 2 bytes
                self.fetch(Reg8::Z).await;
                self.halt();
            }
            return;
        }

        if self.oscillator_req.switch_armed() {
            if interrupt_pending == 0 {
                // STOP => 2 bytes
                self.fetch(Reg8::Z).await;
            }

            // Maybe return some error for the non-deterministic stop glitch? idk
            //     if !self.ime {
            //         // Maybe
            //         return Err(StopGlitchError);
            //     }

            // Reset DIV
            self.bus.poke(0xFF04, 0); // value does not matter
            t!(self).after(self.oscillator_req.speed_switch()).await;

            return;
        }

        // If no pending speed switch and no JOYP currently pressed
        if interrupt_pending == 0 {
            // STOP => 2 bytes
            let _ = self.fetch(Reg8::Z).await;
        }
        // Reset DIV
        self.bus.poke(0xFF04, 0); // value does not matter

        t!(self).after(self.oscillator_req.stop_mode()).await;
    }

    pub async fn idle(&self) {
        t!(self).wait(M_CYCLE).await;
    }

    pub async fn illegal(&self) -> Infallible {
        self.state.wedged.set(true);
        future::pending::<Infallible>().await
    }
}
