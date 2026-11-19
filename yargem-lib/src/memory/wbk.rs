use std::cell::{Cell, RefCell};

use crate::memory::{
    mem::BankedMemory,
    mmu::{MemoryContext, MemoryHandler},
};

pub struct WramMapper {
    ram: RefCell<BankedMemory<{ const { 4 * 1024 } }>>,
    bank: Cell<u8>,
}

impl WramMapper {
    pub fn new() -> Self {
        Self {
            ram: RefCell::new(BankedMemory::zeroed(32 * 1024)),
            bank: 0.into(),
        }
    }

    fn bank1(&self) -> u8 {
        self.bank.get().max(1)
    }
}

impl MemoryHandler for WramMapper {
    fn read(&self, address: u16, _: MemoryContext<'_>) -> u8 {
        let ram = self.ram.borrow();
        match address {
            0xC000..=0xCFFF => ram.read(0, address),
            0xD000..=0xDFFF => ram.read(self.bank1() as usize, address),
            0xFF70 => 0xF8 | self.bank.get(),
            _ => unreachable!("invalid read in WramMapper at ${address:04X}"),
        }
    }

    fn write(&self, address: u16, value: u8, _: MemoryContext<'_>) {
        let mut ram = self.ram.borrow_mut();
        match address {
            0xC000..=0xCFFF => ram.write(0, address, value),
            0xD000..=0xDFFF => ram.write(self.bank1() as usize, address, value),
            0xFF70 => self.bank.set(value & 7),
            _ => unreachable!("invalid read in WramMapper at ${address:04X}"),
        }
    }

    fn bank_at(&self, address: u16) -> Option<u16> {
        Some(match address {
            0xC000..=0xCFFF => 0,
            0xD000..=0xDFFF => self.bank1() as u16,
            _ => unreachable!("invalid read in WramMapper at ${address:04X}"),
        })
    }
}
