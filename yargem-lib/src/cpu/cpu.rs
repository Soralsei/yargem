use std::cell::{Cell, Ref, RefCell, RefMut};
use std::convert::Infallible;
use std::rc::Rc;

use super::registers::Registers;
use crate::clock::{self, CpuClock, FixedClock, Timeline, M_CYCLE};
use crate::cpu::instructions::{execute_prefixed, execute_unprefixed};
use crate::cpu::interrupt::InterruptController;
use crate::cpu::registers::{Flags, Reg16, Reg8};
use crate::memory::bus::CpuBus;
use crate::util::bit_operations::*;
use common::Condition;

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Ime {
    #[default]
    Disabled,
    Pending,
    Enabled,
}

pub struct Cpu {
    pub registers: RefCell<Registers>,
    instr: Cell<u64>,
    halted: Cell<bool>,
    halt_bug: Cell<bool>,
    ime: Cell<Ime>,

    // Happens on CPU panic (unknown opcode decode wedges the cpu and stops fetches)
    wedged: Cell<bool>,
}

impl Cpu {
    pub fn new(is_cgb: bool) -> Cpu {
        Cpu {
            registers: RefCell::new(Registers::new(is_cgb)),
            ime: Cell::new(Ime::Disabled),
            instr: Cell::new(0),
            halted: Cell::new(false),
            halt_bug: Cell::new(false),
            wedged: Cell::new(false),
        }
    }

    pub async fn task(
        cpu: Rc<Cpu>,
        irq: Rc<InterruptController>,
        bus: CpuBus,
        t: Timeline<clock::Cpu>,
        t_fixed: Timeline<clock::Fixed>,
        clock: Rc<CpuClock>,
        fixed_clock: Rc<FixedClock>,
    ) -> Infallible {
        let task = CpuTask {
            t,
            t_fixed,
            cpu,
            irq,
            bus,
            clock,
            fixed_clock,
        };
        loop {
            task.step().await
        }
    }

    pub fn instr_count(&self) -> u64 {
        self.instr.get()
    }

    pub fn halt(&self) {
        self.halted.set(true);
    }

    pub fn set_ime(&self, active: Ime) {
        self.ime.set(active);
    }
}

pub struct CpuTask {
    cpu: Rc<Cpu>,
    irq: Rc<InterruptController>,
    bus: CpuBus,
    clock: Rc<CpuClock>,
    fixed_clock: Rc<FixedClock>,
    t: Timeline<clock::Cpu>,
    t_fixed: Timeline<clock::Fixed>,
}

impl CpuTask {
    pub async fn step(&self) {
        if self.cpu.halted.get() {
            if self.cpu.ime.get() == Ime::Disabled && self.irq.peek().is_some() {
                // halt bug: PC fails to increment, next byte executes twice
                self.cpu.halt_bug.set(true);
            } else {
                self.t
                    .after(self.irq.signal().until(|| self.irq.peek().is_some()))
                    .await;
            }
            self.cpu.halted.set(false);
        }

        if self.cpu.ime.get() == Ime::Enabled {
            if let Some(vector) = self.irq.consume() {
                self.service(vector).await;
            }
        }

        if self.cpu.ime.get() == Ime::Pending {
            self.cpu.ime.set(Ime::Enabled);
        }

        self.execute().await;
    }

    async fn execute(&self) {
        self.fetch(Reg8::Z).await;
        let opcode = self.registers().z;
        match opcode {
            0xCB => {
                self.fetch(Reg8::Z).await;
                let opcode = self.registers().z;
                execute_prefixed(self, opcode).await;
            }
            _ => execute_unprefixed(self, opcode).await,
        }
        self.cpu.instr.set(self.cpu.instr.get() + 1);
    }

    async fn service(&self, vector: u8) {
        self.idle().await;
        self.idle().await;

        let pc = self.registers().pc;
        let (msb, lsb) = word_to_bytes(pc);

        self.push(msb).await;
        self.push(lsb).await;

        self.cpu.ime.set(Ime::Disabled);

        self.registers_mut().pc = vector as u16;

        self.idle().await;
    }

    fn registers_mut(&self) -> RefMut<'_, Registers> {
        self.cpu.registers.borrow_mut()
    }

    pub fn registers(&self) -> Ref<'_, Registers> {
        self.cpu.registers.borrow()
    }

    pub async fn fetch(&self, dst: Reg8) {
        self.t.wait(M_CYCLE).await;

        let mut registers = self.registers_mut();
        let pc = registers.pc;
        registers.write_u8(dst, self.bus.read(pc));

        if !self.cpu.halt_bug.take() {
            registers.pc = registers.pc.wrapping_add(1);
        }
    }

    pub async fn load(&self, dst: Reg8, addr: u16) {
        self.t.wait(M_CYCLE).await;

        self.registers_mut().write_u8(dst, self.bus.read(addr));
    }

    pub async fn store(&self, address: u16, src: Reg8) {
        self.t.wait(M_CYCLE).await;

        self.bus.write(address, self.registers().read_u8(src));
    }

    pub async fn store16(&self, addr: u16, src: Reg16) {
        let (msb, lsb) = word_to_bytes(self.registers().read_u16(src));

        self.t.wait(M_CYCLE).await;
        self.bus.write(addr, lsb);

        self.t.wait(M_CYCLE).await;
        self.bus.write(addr.wrapping_add(1), msb);
    }

    pub async fn idle(&self) {
        self.t.wait(M_CYCLE).await;
    }

    pub fn cond(&self, cond: Condition) -> bool {
        let flags = &self.registers().f;
        match cond {
            Condition::Unconditional => true,
            Condition::NotZero => !flags.zero,
            Condition::Zero => flags.zero,
            Condition::NotCarry => !flags.carry,
            Condition::Carry => flags.carry,
        }
    }

    pub fn high(&self, reg: Reg8) -> u16 {
        0xFF00 | self.registers().read_u8(reg) as u16
    }

    pub fn rel(&self, reg: Reg8) -> u16 {
        let pc = self.registers().pc;
        pc.wrapping_add_signed(self.registers().read_u8(reg) as i8 as i16)
    }

    pub fn jump(&self, addr: u16) {
        self.registers_mut().pc = addr;
    }

    pub async fn push(&self, value: u8) {
        self.t.wait(M_CYCLE).await;

        let mut registers = self.registers_mut();
        let new_sp = registers.sp.wrapping_sub(1);
        registers.sp = new_sp;

        self.bus.write(new_sp, value);
    }

    pub async fn pop(&self) -> u8 {
        self.t.wait(M_CYCLE).await;

        let mut registers = self.registers_mut();
        let sp = registers.sp;
        registers.sp = sp.wrapping_add(1);

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
        self.registers().read_u16(src)
    }

    pub fn alu(&self, register: Reg8, op: impl FnOnce(&mut Flags, u8) -> u8) {
        let mut registers = self.registers_mut();
        let value = registers.read_u8(register);
        let result = op(&mut registers.f, value);
        registers.write_u8(register, result);
    }

    pub fn alu2(&self, dst: Reg8, src: Reg8, op: impl FnOnce(&mut Flags, u8, u8) -> u8) {
        let mut registers = self.registers_mut();
        let (a, b) = (registers.read_u8(dst), registers.read_u8(src));
        let result = op(&mut registers.f, a, b);
        registers.write_u8(dst, result);
    }

    pub fn alu16(&self, register: Reg16, op: impl FnOnce(&mut Flags, u16) -> u16) {
        let mut registers = self.registers_mut();
        let value = registers.read_u16(register);
        let result = op(&mut registers.f, value);
        registers.write_u16(register, result);
    }

    pub fn alu16_2(
        &self,
        dst: Reg16,
        a: Reg16,
        b: Reg16,
        op: impl FnOnce(&mut Flags, u16, u16) -> u16,
    ) {
        let mut registers = self.registers_mut();
        let (a, b) = (registers.read_u16(a), registers.read_u16(b));
        let result = op(&mut registers.f, a, b);
        registers.write_u16(dst, result);
    }

    /// mixed-width one, for `add sp,r8` and `ld hl,sp+r8`. `dst` is
    pub fn alu16_8(
        &self,
        dst: Reg16,
        a: Reg16,
        b: Reg8,
        op: impl FnOnce(&mut Flags, u16, u8) -> u16,
    ) {
        let mut registers = self.registers_mut();
        let (a, b) = (registers.read_u16(a), registers.read_u8(b));
        let result = op(&mut registers.f, a, b);
        registers.write_u16(dst, result);
    }

    pub fn mov(&self, dst: Reg8, src: Reg8) {
        let mut registers = self.registers_mut();
        let val = registers.read_u8(src);
        registers.write_u8(dst, val);
    }

    pub fn mov16(&self, dst: Reg16, src: Reg16) {
        let mut registers = self.registers_mut();
        let val = registers.read_u16(src);
        registers.write_u16(dst, val);
    }

    pub fn set16(&self, dst: Reg16, value: u16) {
        self.registers_mut().write_u16(dst, value);
    }

    /// `bit b,r`: one register in, flags out, nothing written back. The bit
    /// index rides in the closure, since it comes from the opcode rather than
    /// the register file.
    pub fn test(&self, register: Reg8, op: impl FnOnce(&mut Flags, u8)) {
        let mut registers = self.registers_mut();
        let value = registers.read_u8(register);
        op(&mut registers.f, value);
    }

    /// `cp`: two registers in, neither written.
    pub fn test2(&self, left: Reg8, right: Reg8, op: impl FnOnce(&mut Flags, u8, u8)) {
        let mut registers = self.registers_mut();
        let (a, b) = (registers.read_u8(left), registers.read_u8(right));
        op(&mut registers.f, a, b);
    }

    pub fn flags(&self, op: impl FnOnce(&mut Flags)) {
        op(&mut self.registers_mut().f)
    }

    pub fn ei(&self) {
        self.cpu.set_ime(Ime::Pending);
    }

    pub fn ei_now(&self) {
        self.cpu.set_ime(Ime::Enabled);
    }

    pub fn di(&self) {
        self.cpu.set_ime(Ime::Disabled);
    }

    pub fn halt(&self) {
        self.cpu.halt();
    }

    pub async fn stop(&self) {
        let interrupt_pending = self.bus.peek(0xFFFF) & self.bus.peek(0xFF0F) & 0x1F;
        let joyp = self.bus.peek(0xFF00) & 0x0F;

        // If a button is being held on a selected line in JOYP
        if joyp != 0x0F {
            // No pending interrupt
            if interrupt_pending == 0 {
                // STOP => 2 bytes
                self.fetch(Reg8::Z).await;
                self.cpu.halt();
            }
            return;
        }

        if self.clock.switch_armed() {
            if interrupt_pending == 0 {
                // STOP => 2 bytes
                self.fetch(Reg8::Z).await;
            }
            //     if !self.ime {
            //         // Maybe
            //         return Err(StopGlitchError);
            //     }
            self.clock.reset_div();
            self.clock.switch_speed();
            self.clock.stop();

            // The CPU sits out the next 2050 M-cycles
            self.t_fixed.wait(2050 * M_CYCLE).await;

            self.clock.resume();

            return;
        }

        // If no pending speed switch and no JOYP currently pressed
        if interrupt_pending == 0 {
            // STOP => 2 bytes
            let _ = self.fetch(Reg8::Z).await;
        }
        // For both, enter STOP mode and reset DIV
        self.clock.reset_div();
        self.clock.stop();
        self.fixed_clock.stop();
    }

    pub async fn illegal(&self) -> Infallible {
        self.cpu.wedged.set(true);
        loop {
            self.idle().await;
        }
    }
}
