use crate::is_bit_set;
use crate::system::PowerOn;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TileArea {
    #[default]
    Low = 0,
    High = 1,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TileAddressing {
    #[default]
    Mode8800 = 0,
    Mode8000 = 1,
}
impl From<TileAddressing> for u8 {
    fn from(value: TileAddressing) -> Self {
        value as u8
    }
}

impl From<bool> for TileAddressing {
    fn from(value: bool) -> Self {
        if value {
            Self::Mode8000
        } else {
            Self::Mode8800
        }
    }
}

impl From<TileArea> for u8 {
    fn from(value: TileArea) -> Self {
        value as u8
    }
}

impl From<bool> for TileArea {
    fn from(value: bool) -> Self {
        if value {
            Self::High
        } else {
            Self::Low
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjSize {
    #[default]
    Size8x8 = 0,
    Size8x16 = 1,
}

impl ObjSize {
    pub fn as_height(&self) -> u8 {
        match self {
            ObjSize::Size8x8 => 8,
            ObjSize::Size8x16 => 16,
        }
    }
}

impl From<ObjSize> for u8 {
    fn from(value: ObjSize) -> Self {
        value as u8
    }
}

impl From<bool> for ObjSize {
    fn from(value: bool) -> Self {
        if value {
            Self::Size8x16
        } else {
            Self::Size8x8
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LcdControl {
    pub lcd_ppu_enable: bool,                 // bit 7
    pub window_tilemap_area: TileArea,        // bit 6
    pub window_enable: bool,                  // bit 5
    pub bg_window_tiles_area: TileAddressing, // bit 4
    pub bg_tilemap_area: TileArea,            // bit 3
    pub obj_size: ObjSize,                    // bit 2
    pub obj_enable: bool,                     // bit 1
    pub bg_window_enable: bool,               // bit 0, has different meaning in CGB mode
}

impl From<LcdControl> for u8 {
    fn from(value: LcdControl) -> Self {
        (value.lcd_ppu_enable as u8) << 7
            | (value.window_tilemap_area as u8) << 6
            | (value.window_enable as u8) << 5
            | (value.bg_window_tiles_area as u8) << 4
            | (value.bg_tilemap_area as u8) << 3
            | (value.obj_size as u8) << 2
            | (value.obj_enable as u8) << 1
            | (value.bg_window_enable as u8)
    }
}

impl From<u8> for LcdControl {
    fn from(value: u8) -> Self {
        Self {
            lcd_ppu_enable: is_bit_set!(value, 7),
            window_tilemap_area: is_bit_set!(value, 6).into(),
            window_enable: is_bit_set!(value, 5),
            bg_window_tiles_area: is_bit_set!(value, 4).into(),
            bg_tilemap_area: is_bit_set!(value, 3).into(),
            obj_size: is_bit_set!(value, 2).into(),
            obj_enable: is_bit_set!(value, 1),
            bg_window_enable: is_bit_set!(value, 0),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PpuMode {
    HBlank = 0, // Mode 0
    VBlank = 1, // Mode 1
    #[default]
    OamScan = 2, // Mode 2
    Drawing = 3, // Mode 3
}

impl From<PpuMode> for u8 {
    fn from(value: PpuMode) -> Self {
        value as u8
    }
}

impl From<u8> for PpuMode {
    fn from(value: u8) -> Self {
        match value & 0b11 {
            0 => PpuMode::HBlank,
            1 => PpuMode::VBlank,
            2 => PpuMode::OamScan,
            _ => PpuMode::Drawing,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LcdStat {
    pub lyc_int_select: bool,   // bit 6,   R/W
    pub mode2_int_select: bool, // bit 5,   R/W
    pub mode1_int_select: bool, // bit 4,   R/W
    pub mode0_int_select: bool, // bit 3,   R/W
    pub lyc_ly_eq: bool,        // bit 2,   RO
    pub ppu_mode: PpuMode,      // bit 0-1, RO
}

impl From<LcdStat> for u8 {
    fn from(value: LcdStat) -> Self {
        0x80 // bit 7 is unused and always reads as 1
            | (value.lyc_int_select as u8) << 6
            | (value.mode2_int_select as u8) << 5
            | (value.mode1_int_select as u8) << 4
            | (value.mode0_int_select as u8) << 3
            | (value.lyc_ly_eq as u8) << 2
            | (value.ppu_mode as u8)
    }
}

impl LcdStat {
    pub fn set(&mut self, value: u8) {
        self.lyc_int_select = is_bit_set!(value, 6);
        self.mode2_int_select = is_bit_set!(value, 5);
        self.mode1_int_select = is_bit_set!(value, 4);
        self.mode0_int_select = is_bit_set!(value, 3);
        // lyc_ly_eq and ppu_mode are both read-only
        // they are only set by the PPU, never by the CPU
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PpuRegisters {
    lcdc: LcdControl,
    ly: u8,
    lyc: u8,
    stat: LcdStat,
    wx: u8,
    wy: u8,
    scy: u8,
    scx: u8,
    bgp: u8,
    obp0: u8,
    obp1: u8,
    vram_bank: u8,
}

impl PpuRegisters {
    pub fn new(power_on: PowerOn) -> Self {
        match power_on {
            PowerOn::BootRom => Self::default(),
            // What the boot ROM leaves behind (Pan Docs "Power Up Sequence").
            PowerOn::PostBoot => Self {
                lcdc: 0x91.into(),
                bgp: 0xFC,
                ..Self::default()
            },
        }
    }

    pub fn read(&self, address: u16) -> u8 {
        match address {
            0xFF40 => self.lcdc.into(),
            0xFF41 => self.stat.into(),
            0xFF42 => self.scy,
            0xFF43 => self.scx,
            0xFF44 => self.ly,
            0xFF45 => self.lyc,
            // 0xFF46 handled by OAM DMA device
            0xFF47 => self.bgp,
            0xFF48 => self.obp0,
            0xFF49 => self.obp1,
            0xFF4A => self.wy,
            0xFF4B => self.wx,
            0xFF4F => self.vram_bank,
            _ => unreachable!("Ppu register at address 0x{:04X} does not exist", address),
        }
    }

    pub fn write(&mut self, address: u16, value: u8) {
        match address {
            0xFF40 => self.lcdc = value.into(),
            // STAT bits 0-2 are read-only, LcdStat::set only takes bits 3-6
            0xFF41 => self.stat.set(value),
            0xFF42 => self.scy = value,
            0xFF43 => self.scx = value,
            // LY is read-only, the PPU is the only one driving it (see set_ly)
            0xFF44 => {}
            // writing LYC re-evaluates the coincidence flag immediately
            0xFF45 => {
                self.lyc = value;
                self.stat.lyc_ly_eq = self.ly == self.lyc;
            }
            // 0xFF46 handled by OAM DMA device
            0xFF47 => self.bgp = value,
            0xFF48 => self.obp0 = value,
            0xFF49 => self.obp1 = value,
            0xFF4A => self.wy = value,
            0xFF4B => self.wx = value,
            0xFF4F => self.vram_bank = value,
            // the actual OAM transfer is started by the Ppu memory handler
            _ => unreachable!("Ppu register at address 0x{:04X} does not exist", address),
        }
    }

    /// LY and the read-only STAT bits are driven by the PPU, not by CPU writes.
    pub fn set_ly(&mut self, ly: u8) {
        self.ly = ly;
        self.stat.lyc_ly_eq = self.ly == self.lyc;
    }

    pub fn set_mode(&mut self, mode: PpuMode) {
        self.stat.ppu_mode = mode;
    }

    pub fn lcdc(&self) -> LcdControl {
        self.lcdc
    }

    pub fn stat(&self) -> LcdStat {
        self.stat
    }

    pub fn ly(&self) -> u8 {
        self.ly
    }

    pub fn scx(&self) -> u8 {
        self.scx
    }

    pub fn scy(&self) -> u8 {
        self.scy
    }

    pub fn wx(&self) -> u8 {
        self.wx
    }

    pub fn wy(&self) -> u8 {
        self.wy
    }

    pub fn vram_bank(&self) -> u8 {
        self.vram_bank
    }

    pub fn bgp(&self) -> u8 {
        self.bgp
    }

    pub fn obp0(&self) -> u8 {
        self.obp0
    }

    pub fn obp1(&self) -> u8 {
        self.obp1
    }
}
