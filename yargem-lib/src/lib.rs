mod audio;
mod clock;
mod compat;
mod cpu;
pub mod debug;
mod graphics;
pub mod input;
mod memory;
mod mode;
pub mod system;
mod timer;
mod util;

use std::{error::Error, fmt::Display};

pub use audio::SAMPLE_RATE;
pub use graphics::{ppu::Frame, SCREEN_H, SCREEN_W};

#[derive(Debug, Clone, Copy)]
pub struct SizeMismatch {
    pub expected: usize,
    pub actual: usize,
}

impl Error for SizeMismatch {}

impl Display for SizeMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Size mismatch error (expected {}, got {})",
            self.expected, self.actual
        )
    }
}

#[derive(Debug)]
pub enum YargemError {
    BootRomMismatch(SizeMismatch),
    SaveMismatch(SizeMismatch),
    // ...
}

impl Display for YargemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            YargemError::BootRomMismatch(..) => "boot rom mismatch",
            YargemError::SaveMismatch(..) => "save file mismatch",
        })
    }
}

impl Error for YargemError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(match self {
            YargemError::BootRomMismatch(err) => err,
            YargemError::SaveMismatch(err) => err,
        })
    }
}
