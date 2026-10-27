use std::collections::BTreeSet;

use crate::debug::protocol::BankAddr;

pub struct Breakpoints {
    breaks: BTreeSet<BankAddr>,
}

impl Breakpoints {
    pub fn new() -> Self {
        Self {
            breaks: BTreeSet::new(),
        }
    }

    pub fn add(&mut self, bank_addr: BankAddr) {
        self.breaks.insert(bank_addr);
    }

    pub fn remove(&mut self, bank_addr: BankAddr) {
        self.breaks.retain(|b| *b != bank_addr);
    }
}
