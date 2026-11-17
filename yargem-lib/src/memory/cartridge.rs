use std::cell::{Cell, RefCell};

use core::fmt;

const CGB_BIT: u8 = 7;
const CGB_ONLY_BIT: u8 = 6;

const ROM_SIZE: usize = 32 * 1024;

const BATTERY_CODES: phf::Set<u8> =
    phf_set!(0x03u8, 0x06, 0x09, 0x0D, 0x0F, 0x10, 0x13, 0x1B, 0x1E, 0x22, 0xFF);

use phf::phf_set;

use crate::{
    is_bit_set,
    memory::{
        mbc::{Mapper, Mbc1, Mbc2, MbcNone, RAM_BANK_SIZE},
        mmu::{MemoryContext, MemoryHandler},
    },
    SizeMismatch,
};

pub struct Cartridge {
    title: String,
    cgb: bool,
    cgb_only: bool,
    /// Banking registers are the only mutable cartridge state, and nothing
    /// re-enters the cartridge, so one cell here keeps `MemoryBank` on `&mut self`.
    mbc: RefCell<Box<dyn Mapper>>,
    rom_size: usize,
    ram_size: usize,
    ram_dirty: Cell<bool>,
    has_battery: bool,
}

fn checksum(rom: &[u8]) {
    println!("Should verify cartridge checksum");
}

fn mapper_for(mbc_type: u8, rom: Vec<u8>, ram_size: usize) -> Box<dyn Mapper> {
    match mbc_type {
        0x00 => Box::new(MbcNone::new(rom, ram_size)),
        0x01..=0x03 => Box::new(Mbc1::new(rom, ram_size)),
        0x05 | 0x06 => Box::new(Mbc2::new(rom)),
        _ => panic!("unhandled MBC type ${mbc_type:02X}"),
    }
}

impl Cartridge {
    pub fn new(rom: Vec<u8>) -> Self {
        let title = String::from_utf8_lossy(&rom[0x134..=0x142]);
        let mbc_type = rom[0x147];
        let rom_size = ROM_SIZE * (1 << rom[0x148]);
        let ram_size = match rom[0x149] {
            0x02 => RAM_BANK_SIZE,
            0x03 => RAM_BANK_SIZE * 4,
            0x04 => RAM_BANK_SIZE * 16,
            0x05 => RAM_BANK_SIZE * 8,
            _ => 0,
        };
        assert_eq!(
            rom_size,
            rom.len(),
            "Rom looks truncated : expected {}, got {}",
            rom_size,
            rom.len()
        );
        Self {
            title: title.to_string(),
            cgb: is_bit_set!(rom[0x143], CGB_BIT),
            cgb_only: is_bit_set!(rom[0x143], CGB_ONLY_BIT),
            mbc: RefCell::new(mapper_for(mbc_type, rom, ram_size)),
            rom_size,
            ram_size,
            ram_dirty: Cell::new(false),
            has_battery: BATTERY_CODES.contains(&mbc_type),
        }
    }

    pub fn cgb_flag(&self) -> bool {
        self.cgb
    }

    pub fn battery_ram(&self) -> Option<Vec<u8>> {
        self.has_battery
            .then_some(self.mbc.borrow_mut().ram().to_vec())
    }

    pub fn load_battery_ram(&self, data: &[u8]) -> Result<(), SizeMismatch> {
        let mut mbc = self.mbc.borrow_mut();
        let ram = mbc.ram();

        let expected = ram.len();
        let actual = data.len();

        if expected != actual {
            return Err(SizeMismatch { expected, actual });
        }

        ram.copy_from_slice(data);

        Ok(())
    }

    pub fn take_ram_dirty(&self) -> bool {
        self.ram_dirty.take()
    }
}

impl fmt::Display for Cartridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Cartridge {{
            Title : {},
            MBC: {},
            CGB : {} | CGB only : {},
            RAM size : {} KiB,
            ROM size : {} KiB
            Has battery : {}
        }}",
            self.title,
            self.mbc.borrow().name(),
            self.cgb,
            self.cgb_only,
            self.ram_size / 1024,
            self.rom_size / 1024,
            self.has_battery
        )
    }
}

impl MemoryHandler for Cartridge {
    fn read(&self, address: u16, _: MemoryContext<'_>) -> u8 {
        self.mbc.borrow().read(address)
    }

    fn write(&self, address: u16, value: u8, _: MemoryContext<'_>) {
        if (0xA000..=0xBFFF).contains(&address) {
            self.ram_dirty.set(true);
        }
        self.mbc.borrow_mut().write(address, value)
    }

    fn bank_at(&self, address: u16) -> Option<u16> {
        self.mbc.borrow().bank_at(address)
    }
}
