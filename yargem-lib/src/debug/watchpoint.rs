#![allow(dead_code)]

use std::cell::Cell;

pub struct Watchpoint {
    bank: u8,
    addr: u16,
    hit: Cell<bool>,
    enabled: Cell<bool>,
}

impl Watchpoint {
    pub fn new(bank: u8, addr: u16) -> Watchpoint {
        Self {
            bank,
            addr,
            hit: false.into(),
            enabled: true.into(),
        }
    }

    fn try_read(&self, addr: u16) -> bool {
        false
    }
}
