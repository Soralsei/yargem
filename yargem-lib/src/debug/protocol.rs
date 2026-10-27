#![allow(dead_code)]

use std::ops::RangeInclusive;

use crate::cpu::registers::Registers;

#[derive(Ord, Eq, PartialEq, PartialOrd)]
pub struct BankAddr {
    pub bank: Option<u16>,
    pub address: u16,
}

pub struct BankRange {
    pub bank: Option<u16>,
    pub range: RangeInclusive<u16>,
}

pub enum DebugCommand {
    Pause,
    Resume,
    StepInstruction,
    AddBreakpoint(BankAddr),
    AddWatchpoint { id: WatchId, range: BankRange },
    RemoveBreakpoint(BankAddr), // breakpoint number
    RemoveWatchpoint(WatchId),
    ReadMemory { start: u16, len: u16 },
    WriteMemory { addr: u16, value: u8 },
    ReadRegisters,
}

pub enum DebugEvent {
    Paused { reason: StopReason, pc: u16 },
    Memory { start: u16, bytes: Vec<u8> },
    Registers(Registers),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WatchId(pub u32);

#[derive(Clone, Copy, Debug)]
pub struct WatchpointHit {
    pub id: WatchId,
    pub addr: u16,
    pub bank: Option<u16>,
    pub old: u8,
    pub new: u8,
}

pub enum StopReason {
    Breakpoint(BankAddr),
    Watchpoint(WatchpointHit),
    Step,
    UserPause,
}
