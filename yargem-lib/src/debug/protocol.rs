#![allow(dead_code)]

use crate::cpu::registers::Registers;

pub struct BankAddr(u8, u16);

impl BankAddr {
    pub fn bank(&self) -> u8 {
        self.0
    }
    pub fn addr(&self) -> u16 {
        self.1
    }
}

pub enum DebugCommand {
    Pause,
    Resume,
    StepInstruction,
    AddBreakpoint(BankAddr),
    RemoveBreakpoint(usize), // breakpoint number
    ReadMemory { start: u16, len: u16 },
    WriteMemory { addr: u16, value: u8 },
    ReadRegisters,
}

pub enum DebugEvent {
    Paused { reason: StopReason, pc: u16 },
    Memory { start: u16, bytes: Vec<u8> },
    Registers(Registers),
}

pub enum StopReason {
    Breakpoint(BankAddr),
    Watchpoint {
        addr: u16,
        old: u8,
        new: u8,
        pc: u16,
    },
    Step,
    UserPause,
}
