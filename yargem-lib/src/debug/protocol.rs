#![allow(dead_code)]

use crate::cpu::registers::Registers;

pub struct BankAddr {
    pub bank: Option<u16>,
    pub address: u16,
}

pub enum DebugCommand {
    Pause,
    Resume,
    StepInstruction,
    AddBreakpoint(BankAddr),
    AddWatchpoint(BankAddr),
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

pub struct WatchpointHit {
    pub addr: u16,
    pub bank: Option<u16>,
    pub old: u8,
    pub new: u8,
    pub pc: u16,
}

pub enum StopReason {
    Breakpoint(BankAddr),
    Watchpoint(WatchpointHit),
    Step,
    UserPause,
}
