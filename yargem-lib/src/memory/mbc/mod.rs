mod components;

use crate::{
    is_bit_set,
    memory::mbc::components::{CartRam, Rom, MAX_ROM_SIZE_NORMAL},
};

pub use crate::memory::mbc::components::RAM_BANK_SIZE;

fn maybe_one_bank(bank_number: u8, mask: u8) -> u8 {
    if bank_number & mask == 0x00 {
        0x01
    } else {
        bank_number & 0b1111
    }
}

pub trait Mapper {
    fn read(&self, address: u16) -> u8;
    fn write(&mut self, address: u16, value: u8);
    fn bank_at(&self, _address: u16) -> Option<u16> {
        None
    }
    fn name(&self) -> &'static str;
    fn ram(&mut self) -> &mut [u8] {
        &mut []
    }
}

pub struct MbcNone {
    rom: Rom,
    ram: CartRam,
}

impl MbcNone {
    pub fn new(rom: Vec<u8>, ram_size: usize) -> Self {
        Self {
            rom: Rom::new(rom),
            ram: CartRam::new(ram_size, true),
        }
    }
}

impl Mapper for MbcNone {
    fn read(&self, address: u16) -> u8 {
        match address {
            0x0000..=0x3FFF => self.rom.read(0, address),
            0x4000..=0x7FFF => self.rom.read(1, address),
            0xA000..=0xBFFF => self.ram.read(0, address),
            _ => unreachable!("Invalid read in MbcNone at address 0x{:04X}", address),
        }
    }

    fn write(&mut self, address: u16, value: u8) {
        match address {
            0x0000..=0x7FFF => (),
            0xA000..=0xBFFF => {
                self.ram.write(0, address, value);
            }
            _ => unreachable!("Invalid memory write at address 0x{:04X}", address),
        }
    }

    fn name(&self) -> &'static str {
        "No MBC"
    }
}

pub struct Mbc1 {
    rom: Rom,
    ram: CartRam,
    // Registers
    rom_bank_number: u8,
    ram_bank_number: u8,
    advanced_mode: bool,
}

impl Mbc1 {
    pub fn new(rom: Vec<u8>, ram_size: usize) -> Self {
        debug_assert!(ram_size == 0 || ram_size.is_power_of_two());
        Self {
            rom: Rom::new(rom),
            ram: CartRam::new(ram_size, false),
            rom_bank_number: 0x00,
            ram_bank_number: 0x00,
            advanced_mode: false,
        }
    }

    fn has_ram(&self) -> bool {
        !self.ram.as_slice().is_empty()
    }

    fn rom_needs_extended_banking(&self) -> bool {
        self.rom.len() > MAX_ROM_SIZE_NORMAL
    }

    fn ram_bank(&self) -> usize {
        // Should be 0b0[ram_bank_number & 0b11]00000
        // ex: ram_bank_number = 0b10 => bank_number = 0b01000000
        // ram_bank_number can only ever be in {0x00, 0x20, 0x40, 0x60}
        (if self.advanced_mode && !self.rom_needs_extended_banking() {
            (self.ram_bank_number & 0b11) as u16
        } else {
            0
        }) as usize
    }

    fn bank0(&self) -> usize {
        (if self.advanced_mode && self.rom_needs_extended_banking() {
            (self.ram_bank_number & 0b11) << 5 as u16
        } else {
            0
        }) as usize
    }

    fn bank1(&self) -> usize {
        let corrected_bank1 = maybe_one_bank(self.rom_bank_number, 0b11111);
        let bank2_number: u8 = if self.rom_needs_extended_banking() {
            (self.ram_bank_number & 0b11) << 5
        } else {
            0
        };

        (bank2_number | corrected_bank1) as usize
    }
}

impl Mapper for Mbc1 {
    fn read(&self, address: u16) -> u8 {
        match address {
            0x0000..=0x3FFF => self.rom.read(self.bank0(), address),
            0x4000..=0x7FFF => self.rom.read(self.bank1(), address),
            0xA000..=0xBFFF => self.ram.read(self.ram_bank(), address),
            _ => unreachable!("Invalid memory read at 0x{address:04X}"),
        }
    }

    fn write(&mut self, address: u16, value: u8) {
        match address {
            0x0000..=0x1FFF => {
                self.ram.enabled = value & 0xF == 0xA;
            }
            0x2000..=0x3FFF => {
                self.rom_bank_number = value & 0b11111;
            }
            0x4000..=0x5FFF => {
                self.ram_bank_number = value & 0b11;
            }
            0x6000..=0x7FFF => {
                self.advanced_mode = value & 0b1 != 0;
            }
            0xA000..=0xBFFF => self.ram.write(self.ram_bank(), address, value),
            _ => unreachable!("Invalid memory write at 0x{address:04X}"),
        }
    }

    fn bank_at(&self, address: u16) -> Option<u16> {
        match address {
            0x0000..=0x3FFF => Some(self.bank0() as u16),
            0x4000..=0x7FFF => Some(self.bank1() as u16),
            0xA000..=0xBFFF => Some(self.ram_bank() as u16),
            _ => unreachable!("unhandled bank resolution in MBC1 at address 0x{address:04X}",),
        }
    }

    fn name(&self) -> &'static str {
        "MBC1"
    }

    fn ram(&mut self) -> &mut [u8] {
        self.ram.as_mut_slice()
    }
}

pub struct Mbc2 {
    rom: Rom,
    ram: CartRam,
    rom_bank_number: u8,
}

impl Mbc2 {
    pub fn new(rom: Vec<u8>) -> Self {
        Self {
            rom: Rom::new(rom),
            ram: CartRam::new(0x200, false),
            rom_bank_number: 0x00,
        }
    }

    fn bank1(&self) -> usize {
        maybe_one_bank(self.rom_bank_number, 0b1111) as usize
    }
}

impl Mapper for Mbc2 {
    fn read(&self, address: u16) -> u8 {
        match address {
            0x0000..=0x3FFF => self.rom.read(0, address),
            0x4000..=0x7FFF => self.rom.read(self.bank1(), address),
            0xA000..=0xBFFF => self.ram.read(0, address) | 0xF0,
            _ => unreachable!("invalid read in Mbc2 at 0x{address:04X}"),
        }
    }

    fn write(&mut self, address: u16, value: u8) {
        match address {
            0x0000..=0x3FFF => {
                // ROM bank1 number write
                if is_bit_set!(address, 8) {
                    self.rom_bank_number = value & 0xF;
                }
                // RAM enable write
                else {
                    self.ram.enabled = (value & 0xF) == 0xA;
                }
            }
            0x4000..=0x9FFF => {} // Block writes
            0xA000..=0xBFFF => self.ram.write(0, address, value),
            _ => unreachable!("invalid write in Mbc2 at 0x{address:04X}"),
        }
    }

    fn bank_at(&self, address: u16) -> Option<u16> {
        if (0x4000..=0x7FFF).contains(&address) {
            Some(maybe_one_bank(self.rom_bank_number, 0b1111) as u16)
        } else {
            None
        }
    }

    fn name(&self) -> &'static str {
        "MBC2"
    }
}

pub struct Mbc3 {
    rom: Vec<u8>,
    ram: Vec<u8>,
    ram_mask: usize,
    // Registers
    ram_enable: bool,
    rom_bank_number: u8,
    ram_bank_number: u8,
    advanced_mode: bool,
}

impl Mbc3 {}

impl Mapper for Mbc3 {
    fn read(&self, address: u16) -> u8 {
        todo!()
    }

    fn write(&mut self, address: u16, value: u8) {
        todo!()
    }

    fn name(&self) -> &'static str {
        todo!()
    }
}
