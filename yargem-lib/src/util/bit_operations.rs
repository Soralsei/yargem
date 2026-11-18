/// 0 Indexed (first bit in byte is at index 0)
#[macro_export]
macro_rules! is_bit_set {
    ($value:expr, $bit:expr) => {
        ($value & (1 << $bit)) != 0
    };
}
#[macro_export]
macro_rules! bit_value {
    ($value:expr, $bit:expr) => {
        ($value >> ($bit - 1) & 0x1)
    };
}

#[inline(always)]
pub fn bytes_to_word(msb: u8, lsb: u8) -> u16 {
    ((msb as u16) << 8) | lsb as u16
}

#[inline(always)]
pub fn word_to_bytes(word: u16) -> (u8, u8) {
    ((word >> 8) as u8, word as u8)
}
