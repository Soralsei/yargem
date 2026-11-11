use std::cell::Cell;

use crate::{
    memory::mmu::{MemoryContext, MemoryHandler},
    SizeMismatch,
};

pub struct BootRom {
    boot_rom: Vec<u8>,
    boot_rom_enabled: Cell<bool>,
}

impl BootRom {
    pub fn new(boot_rom: Option<Vec<u8>>, is_cgb: bool) -> Result<Self, SizeMismatch> {
        match boot_rom {
            Some(boot_rom) => {
                let expected = if is_cgb { 0x900 } else { 0x100 };
                if boot_rom.len() != expected {
                    return Err(SizeMismatch {
                        actual: boot_rom.len(),
                        expected,
                    });
                }
                Ok(Self {
                    boot_rom,
                    boot_rom_enabled: Cell::new(true),
                })
            }
            None => Ok(Self {
                boot_rom: Vec::with_capacity(0),
                boot_rom_enabled: Cell::new(false),
            }),
        }
    }

    #[inline]
    fn covers(&self, address: u16) -> bool {
        address < 0x100 || (self.boot_rom.len() == 0x900 && (0x200..0x900).contains(&address))
    }
}

impl MemoryHandler for BootRom {
    fn read(&self, address: u16, ctx: MemoryContext<'_>) -> u8 {
        if self.boot_rom_enabled.get() && self.covers(address) {
            return self.boot_rom[address as usize];
        }
        if address == 0xFF50 {
            0xFF
        } else {
            ctx.read(address)
        }
    }

    fn write(&self, address: u16, value: u8, ctx: MemoryContext<'_>) {
        if address == 0xFF50 {
            self.boot_rom_enabled.set(false);
        }
        ctx.write(address, value)
    }
}
