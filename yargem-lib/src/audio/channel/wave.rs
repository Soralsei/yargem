use crate::{
    audio::channel::{core_accessors, period::Period, Channel, ChannelCore, NRx4},
    is_bit_set,
};

#[derive(Debug)]
pub struct Wave {
    core: ChannelCore<256>,
    period: Period<1>,
    output_level: u8,     // NR32
    wave_ram: [u8; 0x10], // 0xFF30-0xFF3F

    sample_index: u8,
    sample_buffer: u8,
}

impl Default for Wave {
    fn default() -> Self {
        Self {
            core: ChannelCore::new(),
            period: Period::default(),
            output_level: 0,
            wave_ram: [0; 0x10],
            sample_index: 0,
            sample_buffer: 0,
        }
    }
}

impl Wave {
    pub fn read_wave(&self, address: u16) -> u8 {
        debug_assert!(
            (0xFF30..=0xFF3F).contains(&address),
            "wave ram write out of bounds at 0x{:04X}",
            address
        );
        let offset = address - 0xFF30;
        self.wave_ram[offset as usize]
    }
    pub fn write_wave(&mut self, address: u16, value: u8) {
        debug_assert!(
            (0xFF30..=0xFF3F).contains(&address),
            "wave ram write out of bounds at 0x{:04X}",
            address
        );
        let offset = address - 0xFF30;
        self.wave_ram[offset as usize] = value;
    }

    fn sample(&mut self) {
        // wave ram samples are 4 bit: sample index needs to be divided by 2
        let offset = self.sample_index >> 1;
        // get whether the first or second nibble is needed (upper nibble first)
        // ex: sample 2 => 0b10 => index 1 upper nibble ((wave_ram[1] >> 4) & 0xF)
        // sample 3 => 0b11 => index 1 lower nibble ((wave_ram[1] & 0xF)
        let nibble = 1 - (self.sample_index & 1);
        self.sample_buffer = (self.wave_ram[(offset) as usize] >> (4 * nibble)) & 0xF;
    }
}

impl Channel<256> for Wave {
    const READ_MASK: [u8; 5] = [0x7F, 0xFF, 0x9F, 0xFF, 0xBF];

    fn read_raw(&self, reg: usize) -> u8 {
        debug_assert!(
            reg <= 4,
            "[Wave::read_raw] reg value should never be > 4, got {}",
            reg
        );
        match reg {
            0 => (self.core.dac as u8) << 7,
            1 => 0,
            2 => self.output_level << 5,
            3 => 0,
            _ => self.core.read_control(),
        }
    }

    fn write(&mut self, reg: usize, value: u8, do_clock_length: bool) {
        debug_assert!(
            reg <= 4,
            "[Wave::write] reg value should never be > 4, got {}",
            reg
        );
        match reg {
            0 => self.core.set_dac(is_bit_set!(value, 7)),
            1 => self.core.length.load(value),
            2 => {
                let level = (value >> 5) & 0x3;
                self.output_level = level;
            }
            3 => {
                self.period.write_low(value);
            }
            _ => {
                let control = NRx4(value);
                // before trigger: trigger uses the new period
                self.period.write_high(control.period());
                self.core.write_control(control, do_clock_length);
                if control.trigger() {
                    self.sample_index = 0;
                    self.period.reload();
                }
            }
        }
    }

    fn fresh(&self) -> Self {
        Self {
            wave_ram: self.wave_ram,
            ..Self::default()
        }
    }

    fn clock(&mut self) {
        if self.enabled() && self.period.clock() {
            // on clock, advance sample index and wrap around when reaching last sample (index = 32)
            self.sample_index = (self.sample_index + 1) & 0x1F;
            self.sample();
        }
    }

    fn output(&self) -> u8 {
        if self.enabled() && self.output_level > 0 {
            self.sample_buffer >> (self.output_level - 1)
        } else {
            0
        }
    }

    core_accessors!(256);
}
