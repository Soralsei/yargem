use std::cell::Cell;

use crate::{
    graphics::{attributes::TilePriority, fifo::Pixel, registers::PpuRegisters},
    is_bit_set,
};

#[derive(Clone, Copy)]
pub struct PaletteIndex(u8);
impl PaletteIndex {
    pub fn auto_incr(&self) -> bool {
        is_bit_set!(self.0, 7)
    }

    pub fn addr(&self) -> u8 {
        self.0 & 0x3F
    }

    pub fn incr(&mut self) {
        self.0 = (self.0 & 0x80) | ((self.addr() + 1) & 0x3F)
    }
}

pub struct CgbPpu {
    vbk: Cell<u8>,
    obj_cram: CRam,
    bg_cram: CRam,
}

impl CgbPpu {
    pub fn new() -> Self {
        Self {
            vbk: Cell::new(0),
            obj_cram: CRam::obj_new(),
            bg_cram: CRam::bg_new(),
        }
    }

    pub fn compat(dmg_palette: &[u16; 4]) -> Self {
        let this = Self::new();
        this.obj_cram.load(0, dmg_palette);
        this.obj_cram.load(1, dmg_palette);
        this.bg_cram.load(0, dmg_palette);
        this
    }

    pub fn vbk(&self) -> u8 {
        self.vbk.get()
    }

    pub fn mix(&self, regs: &PpuRegisters, bg: Pixel, obj: Pixel) -> u16 {
        let lcdc = regs.lcdc();
        let obj_wins = obj.color != 0
            && lcdc.obj_enable
            && (bg.color == 0
                || !lcdc.bg_window_enable
                || (bg.bg_priority == TilePriority::None && obj.bg_priority == TilePriority::None));
        if obj_wins {
            self.obj_cram.get(obj.palette, obj.color)
        } else {
            self.bg_cram.get(bg.palette, bg.color)
        }
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF4F => 0xFE | self.vbk.get(),
            0xFF68 => self.bg_cram.index(),
            0xFF69 => self.bg_cram.read(),
            0xFF6A => self.obj_cram.index(),
            0xFF6B => self.obj_cram.read(),
            _ => unreachable!("invalid read in CgbPpu at 0x{addr:04X}"),
        }
    }

    pub fn write(&self, addr: u16, value: u8) {
        match addr {
            0xFF4F => self.vbk.set(value & 1),
            0xFF68 => self.bg_cram.write_index(value),
            0xFF69 => self.bg_cram.write(value),
            0xFF6A => self.obj_cram.write_index(value),
            0xFF6B => self.obj_cram.write(value),
            _ => unreachable!("invalid write in CgbPpu at 0x{addr:04X}"),
        }
    }

    pub fn set_locked_obj(&self, locked: bool) {
        self.obj_cram.set_lock(locked);
    }

    pub fn set_locked_bg(&self, locked: bool) {
        self.bg_cram.set_lock(locked);
    }

    pub fn bg_cram(&self) -> &CRam {
        &self.bg_cram
    }

    pub fn obj_cram(&self) -> &CRam {
        &self.obj_cram
    }
}

pub struct CRam {
    index: Cell<PaletteIndex>,
    locked: Cell<bool>,
    ram: [Cell<u8>; 64],
}

impl CRam {
    fn bg_new() -> Self {
        Self {
            index: Cell::new(PaletteIndex(0)),
            ram: [const { Cell::new(0xFF) }; 64],
            locked: Cell::new(false),
        }
    }

    fn obj_new() -> Self {
        Self {
            index: Cell::new(PaletteIndex(0)),
            ram: [const { Cell::new(0x00) }; 64],
            locked: Cell::new(false),
        }
    }

    fn load(&self, palette: u8, colors: &[u16; 4]) {
        for (i, color) in colors.iter().enumerate() {
            let offset = palette as usize * 8 + i * 2;
            self.ram[offset].set(*color as u8);
            self.ram[offset + 1].set((*color >> 8) as u8);
        }
    }

    pub fn set_lock(&self, locked: bool) {
        self.locked.set(locked);
    }

    pub fn index(&self) -> u8 {
        self.index.get().0 | 0x40
    }

    pub fn write_index(&self, value: u8) {
        self.index.set(PaletteIndex(value));
    }

    pub fn write(&self, data: u8) {
        let mut idx = self.index.get();
        if !self.locked.get() {
            self.ram[idx.addr() as usize].set(data);
        }
        if idx.auto_incr() {
            idx.incr();
            self.index.set(idx);
        }
    }

    pub fn read(&self) -> u8 {
        if self.locked.get() {
            return 0xFF;
        }
        self.ram[(self.index.get().addr()) as usize].get()
    }

    pub fn get(&self, palette: u8, color: u8) -> u16 {
        debug_assert!(color < 4, "color index must be < 4, got {color}");
        debug_assert!(palette < 8, "palette index must be < 8, got {palette}");

        let offset = (palette * 8 + color * 2) as usize;

        let lsb = self.ram[offset].get() as u16;
        let msb = self.ram[offset + 1].get() as u16;

        (msb << 8) | lsb
    }
}
