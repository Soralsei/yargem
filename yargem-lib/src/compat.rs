use std::cell::Cell;

use crate::{
    memory::mmu::{MemoryContext, MemoryHandler},
    system::PowerOn,
};

pub struct DmgCompat {
    locked: Cell<bool>,
}

impl DmgCompat {
    pub fn new(power_on: PowerOn) -> Self {
        Self {
            locked: Cell::new(power_on == PowerOn::PostBoot),
        }
    }

    pub fn lock(&self) {
        self.locked.set(true);
    }
}

impl MemoryHandler for DmgCompat {
    fn read(&self, address: u16, context: MemoryContext<'_>) -> u8 {
        if !self.locked.get() {
            return context.read(address);
        }
        0xFF
    }

    fn write(&self, address: u16, value: u8, context: MemoryContext<'_>) {
        if !self.locked.get() {
            if address == 0xFF50 {
                self.lock();
            }
            context.write(address, value);
        }
    }
}
