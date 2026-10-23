#![allow(dead_code)]

use std::ops::RangeInclusive;

pub enum AccessKind {
    Read,
    Write,
    Execute,
}

pub struct Watchpoint {
    bank: Option<u16>,
    range: RangeInclusive<u16>,
    kind: AccessKind,
}

impl Watchpoint {
    pub fn new(bank: Option<u16>, range: RangeInclusive<u16>, kind: AccessKind) -> Watchpoint {
        Self { bank, range, kind }
    }

    pub fn hit(&self, addr: u16, bank: Option<u16>) -> bool {
        let in_bank = match (self.bank, bank) {
            (Some(watched), Some(mapped)) => watched == mapped,
            _ => true,
        };
        self.range.contains(&addr) && in_bank
    }
}
