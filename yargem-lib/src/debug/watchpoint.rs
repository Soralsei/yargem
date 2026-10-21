#![allow(dead_code)]

use std::{cell::Cell, ops::RangeInclusive};

pub enum AccessKind {
    Read,
    Write,
    Execute,
}

pub struct Watchpoint {
    bank: Option<u16>,
    range: RangeInclusive<u16>,
    kind: AccessKind,
    enabled: Cell<bool>,
}

impl Watchpoint {
    pub fn new(bank: Option<u16>, range: RangeInclusive<u16>, kind: AccessKind) -> Watchpoint {
        Self {
            bank,
            range,
            kind,
            enabled: true.into(),
        }
    }

    fn hit(&self, addr: u16, bank: Option<u16>) -> bool {
        let in_bank = bank == self.bank;
        self.range.contains(&addr) && in_bank
    }
}
