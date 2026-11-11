use std::cell::{Cell, RefCell};

use phf::phf_set;

use crate::{
    is_bit_set, memory::mmu::MemoryContext, util::bit_operations::addressing_number_of_bits,
    SizeMismatch,
};

use super::mmu::MemoryHandler;
use core::fmt;

const CGB: u8 = 7;
const CGB_ONLY: u8 = 6;
const ROM_SIZE: usize = 32 * 1024;
const RAM_BANK_SIZE: usize = 8 * 1024;
const ROM_BANK_SIZE: usize = 16 * 1024;
const MAX_ROM_SIZE_NORMAL: usize = 512 * 1024;

const BATTERY_CODES: phf::Set<u8> =
    phf_set!(0x03u8, 0x06, 0x09, 0x0D, 0x0F, 0x10, 0x13, 0x1B, 0x1E, 0x22, 0xFF);

trait Mapper {
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

fn get_bank_num_mask(rom_size: usize) -> usize {
    let num_banks: usize = rom_size.div_ceil(ROM_BANK_SIZE) as usize;
    let num_bits = addressing_number_of_bits(num_banks);
    // Should never occur, but just to be safe
    // avoids overflow
    let usize_bits = usize::BITS as usize;
    if num_bits >= usize_bits {
        return !0usize;
    }
    (1 << num_bits) - 1
}

struct MbcNone {
    rom: Vec<u8>,
    ram: Vec<u8>,
}

impl MbcNone {
    pub fn new(rom: Vec<u8>, ram_size: usize) -> Self {
        Self {
            rom,
            ram: vec![0u8; ram_size],
        }
    }

    fn has_ram(&self) -> bool {
        !self.ram.is_empty()
    }
}

impl Mapper for MbcNone {
    fn read(&self, address: u16) -> u8 {
        match address {
            0x0000..=0x7FFF => self.rom[address as usize],
            0xA000..=0xBFFF => {
                if !self.has_ram() {
                    return 0xFF;
                }
                self.ram[address as usize & (RAM_BANK_SIZE - 1)]
            }
            _ => unreachable!("Invalid read in MbcNone at address 0x{:04X}", address),
        }
    }

    fn write(&mut self, address: u16, value: u8) {
        match address {
            0x0000..=0x7FFF => (),
            0xA000..=0xBFFF => {
                if self.has_ram() {
                    self.ram[address as usize & (RAM_BANK_SIZE - 1)] = value;
                }
            }
            _ => unreachable!("Invalid memory write at address 0x{:04X}", address),
        }
    }

    fn name(&self) -> &'static str {
        "No MBC"
    }
}

struct Mbc1 {
    rom: Vec<u8>,
    ram: Vec<u8>,
    ram_mask: usize,
    // Registers
    ram_enable: bool, // written at 0x
    rom_bank_number: u8,
    ram_bank_number: u8,
    advanced_mode: bool,
}

impl Mbc1 {
    fn new(rom: Vec<u8>, ram_size: usize) -> Self {
        debug_assert!(ram_size == 0 || ram_size.is_power_of_two());
        Self {
            rom,
            ram: vec![0u8; ram_size],
            ram_mask: ram_size.saturating_sub(1),
            ram_enable: false,
            rom_bank_number: 0x00,
            ram_bank_number: 0x00,
            advanced_mode: false,
        }
    }

    /// Offset of `address` within cart RAM, or None when nothing answers (no RAM
    /// chip, or RAM not enabled). The bus floats then: reads 0xFF, writes vanish.
    fn ram_offset(&self, address: u16) -> Option<usize> {
        if !self.has_ram() || !self.ram_enable {
            return None;
        }
        let offset = address as usize & (RAM_BANK_SIZE - 1);
        // A >512 KiB ROM spends the 2-bit register on upper ROM bank bits, so RAM
        // is pinned to bank 0.
        let bank = self.ram_bank() as usize;
        Some(((bank * RAM_BANK_SIZE) | offset) & self.ram_mask)
    }

    fn maybe_one_bank(bank_number: u8) -> u8 {
        if bank_number & 0b11111 == 0x00 {
            0x01
        } else {
            bank_number
        }
    }

    fn has_ram(&self) -> bool {
        !self.ram.is_empty()
    }

    fn rom_needs_extended_banking(&self) -> bool {
        self.rom.len() > MAX_ROM_SIZE_NORMAL
    }

    fn rom_bank(&self) -> u16 {
        let bank_mask = get_bank_num_mask(self.rom.len()) as u8;
        let corrected_bank1 = Mbc1::maybe_one_bank(self.rom_bank_number);
        let bank2_number: u8 = if self.rom_needs_extended_banking() {
            (self.ram_bank_number & 0b11) << 5
        } else {
            0
        };

        ((bank2_number | corrected_bank1) & bank_mask) as u16
    }

    fn ram_bank(&self) -> u16 {
        // Should be 0b0[ram_bank_number & 0b11]00000
        // ex: ram_bank_number = 0b10 => bank_number = 0b01000000
        // ram_bank_number can only ever be in {0x00, 0x20, 0x40, 0x60}
        if self.advanced_mode && !self.rom_needs_extended_banking() {
            (self.ram_bank_number & 0b11) as u16
        } else {
            0
        }
    }

    fn bank0_offset(&self, address: u16) -> usize {
        let ram_bank = if self.advanced_mode && self.rom_needs_extended_banking() {
            (self.ram_bank_number & 0b11) << 5 as u16
        } else {
            0
        };
        let bank_number = ram_bank as usize;
        // bits 20 and 19 => bank_number, bits 18-14 => 0, bits 13-0 => address <= 0x3FFF
        (bank_number * ROM_BANK_SIZE) | (address as usize)
    }

    fn bank1_offset(&self, address: u16) -> usize {
        let bank_number = self.rom_bank() as usize;
        // Get the address inside the selected bank
        // strictly equivalent to bank_number * ROM_BANK_SIZE + (address - 0x4000)
        (bank_number * ROM_BANK_SIZE) | (address as usize & (ROM_BANK_SIZE - 1))
    }
}

impl Mapper for Mbc1 {
    fn read(&self, address: u16) -> u8 {
        match address {
            0x0000..=0x3FFF => {
                let offset = self.bank0_offset(address);
                self.rom[offset]
            }
            0x4000..=0x7FFF => {
                let offset = self.bank1_offset(address);
                self.rom[offset]
            }
            0xA000..=0xBFFF => {
                // Reads from disabled ram return open-bus values
                let read_value = match self.ram_offset(address) {
                    Some(offset) => self.ram[offset],
                    None => 0xFF,
                };
                read_value
            }
            _ => unreachable!("Invalid memory read at 0x{address:04X}"),
        }
    }

    fn write(&mut self, address: u16, value: u8) {
        match address {
            0x0000..=0x1FFF => {
                self.ram_enable = value & 0xF == 0xA;
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
            0xA000..=0xBFFF => {
                if let Some(offset) = self.ram_offset(address) {
                    self.ram[offset] = value;
                }
            }
            _ => unreachable!("Invalid memory write at 0x{address:04X}"),
        }
    }

    fn bank_at(&self, address: u16) -> Option<u16> {
        match address {
            0x0000..=0x3FFF => Some(self.ram_bank() << 5),
            0xA000..=0xBFFF => Some(self.ram_bank()),
            0x4000..=0x7FFF => Some(self.rom_bank()),
            _ => unreachable!("unhandled bank resolution in MBC1 at address 0x{address:04X}",),
        }
    }

    fn name(&self) -> &'static str {
        "MBC1"
    }

    fn ram(&mut self) -> &mut [u8] {
        &mut self.ram
    }
}

struct Mbc2 {
    rom: Vec<u8>,
    ram: Vec<u8>,
    ram_enable: bool,
    rom_bank_mask: usize,
    rom_bank_number: u8,
}

impl Mbc2 {
    fn new(rom: Vec<u8>) -> Self {
        let rom_size = rom.len();
        Self {
            rom,
            ram: vec![0u8; 0x200],
            ram_enable: false,
            rom_bank_mask: get_bank_num_mask(rom_size),
            rom_bank_number: 0x00,
        }
    }

    fn maybe_one_bank(bank_number: u8) -> u8 {
        if bank_number & 0b1111 == 0x00 {
            0x01
        } else {
            bank_number & 0b1111
        }
    }

    fn bank1_offset(&self, address: u16) -> usize {
        let bank_number = Mbc2::maybe_one_bank(self.rom_bank_number) as usize & self.rom_bank_mask;
        // Get the address inside the selected bank
        // strictly equivalent to bank_number * ROM_BANK_SIZE + (address - 0x4000)
        (bank_number * ROM_BANK_SIZE) | (address as usize & (ROM_BANK_SIZE - 1))
    }
}

impl Mapper for Mbc2 {
    fn read(&self, address: u16) -> u8 {
        match address {
            0x0000..=0x3FFF => self.rom[address as usize],
            0x4000..=0x7FFF => {
                let offset = self.bank1_offset(address);
                self.rom[offset]
            }
            0xA000..=0xBFFF => {
                let offset = (address & (0x200 - 1)) as usize; // echoes 15 times
                self.ram[offset]
            }
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
                    self.ram_enable = (value & 0xF) == 0xA;
                }
            }
            0x4000..=0x9FFF => {} // Block writes
            0xA000..=0xBFFF => {
                let offset = (address & (0x200 - 1)) as usize; // echoes 15 times
                self.ram[offset] = value;
            }
            _ => unreachable!("invalid write in Mbc2 at 0x{address:04X}"),
        }
    }

    fn bank_at(&self, address: u16) -> Option<u16> {
        if (0x4000..=0x7FFF).contains(&address) {
            Some(Mbc2::maybe_one_bank(self.rom_bank_number) as u16)
        } else {
            None
        }
    }

    fn name(&self) -> &'static str {
        "MBC2"
    }
}

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
            cgb: is_bit_set!(rom[0x143], CGB),
            cgb_only: is_bit_set!(rom[0x143], CGB_ONLY),
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
