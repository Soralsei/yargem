#![allow(dead_code)]

use std::ops::{Deref, RangeInclusive};

pub use crate::cpu::registers::Flags;
use crate::cpu::registers::Registers;
pub use crate::debug::watchpoint::{AccessKind, Watchpoint};

#[derive(Ord, Eq, PartialEq, PartialOrd, Debug, Clone, Copy)]
pub struct BankAddr {
    pub bank: Option<u16>,
    pub address: u16,
}

impl BankAddr {
    pub fn matches(&self, other: &Self) -> bool {
        self.address == other.address
            && match (self.bank, other.bank) {
                (Some(self_bank), Some(other_bank)) => self_bank == other_bank,
                _ => true,
            }
    }
}

pub struct BankRange {
    pub bank: Option<u16>,
    pub range: RangeInclusive<u16>,
}

pub enum DebugCommand {
    Attach,
    Detach,
    Pause,
    Resume,
    StepInstruction,
    AddBreakpoint(BankAddr),
    AddWatchpoint {
        bank_range: BankRange,
        access: AccessKind,
    },
    RemoveBreakpoint(BankAddr), // breakpoint number
    RemoveWatchpoint(WatchId),
    ReadMemory {
        start: u16,
        len: u16,
    },
    WriteMemory {
        addr: u16,
        value: u8,
    },
}

#[derive(Debug)]
pub struct RegisterSnapshot(pub Registers);

impl Deref for RegisterSnapshot {
    type Target = Registers;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub enum DebugEvent {
    Attached {
        regs: RegisterSnapshot,
        breaks: Vec<BankAddr>,
        watches: Vec<(WatchId, Watchpoint)>,
    },
    Detached,
    Paused {
        reason: StopReason,
        regs: RegisterSnapshot,
    },
    Resumed,
    Memory {
        start: u16,
        bytes: Vec<u8>,
    },
    Registers(RegisterSnapshot),
    Watchpoints(Vec<(WatchId, Watchpoint)>),
    Breakpoints(Vec<BankAddr>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WatchId(pub u32);

#[derive(Clone, Copy, Debug)]
pub struct WatchpointHit {
    pub id: WatchId,
    pub access: AccessKind,
    pub addr: u16,
    pub bank: Option<u16>,
    pub old: u8,
    pub new: u8,
}

#[derive(Debug)]
pub enum StopReason {
    Breakpoint(BankAddr),
    Watchpoint(WatchpointHit),
    Step,
    UserPause,
}
