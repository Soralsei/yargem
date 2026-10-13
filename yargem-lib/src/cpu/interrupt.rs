use std::{cell::Cell, rc::Rc};

use crate::{
    clock::Signal,
    is_bit_set,
    memory::mmu::{MemoryHandler, MemoryRead, MemoryWrite, Mmu},
};

const VBLANK: u8 = 0;
const LCD: u8 = 1;
const TIMER: u8 = 2;
const SERIAL: u8 = 3;
const JOYPAD: u8 = 4;

/// The five interrupt lines. Every device holds a request handle to a shared
/// instance, so this is one of the two devices that needs per-field
/// granularity: a line is set from inside another device's `step`.
#[derive(Default)]
pub struct Interrupts {
    vblank: Cell<bool>,
    lcd: Cell<bool>,
    timer: Cell<bool>,
    serial: Cell<bool>,
    joypad: Cell<bool>,
}

impl Interrupts {
    fn set(&self, value: u8) {
        self.vblank.set(is_bit_set!(value, VBLANK));
        self.lcd.set(is_bit_set!(value, LCD));
        self.timer.set(is_bit_set!(value, TIMER));
        self.serial.set(is_bit_set!(value, SERIAL));
        self.joypad.set(is_bit_set!(value, JOYPAD));
    }

    fn get(&self) -> u8 {
        u8::from(self)
    }
}

impl std::convert::From<u8> for Interrupts {
    fn from(value: u8) -> Self {
        let interrupts = Self::default();
        interrupts.set(value);
        interrupts
    }
}

impl std::convert::From<&Interrupts> for u8 {
    fn from(value: &Interrupts) -> Self {
        let mut res = 0;
        res |= value.vblank.get() as u8;
        res |= (value.lcd.get() as u8) << 1;
        res |= (value.timer.get() as u8) << 2;
        res |= (value.serial.get() as u8) << 3;
        res |= (value.joypad.get() as u8) << 4;
        res
    }
}

pub struct InterruptRequest {
    request: Rc<Interrupts>,
    signal: Rc<Signal>,
}

impl InterruptRequest {
    pub fn new(request: Rc<Interrupts>, signal: Rc<Signal>) -> InterruptRequest {
        Self { request, signal }
    }

    pub fn vblank(&self, value: bool) {
        self.request.vblank.set(value);
        self.signal.raise();
        #[cfg(feature = "debug")]
        if value {
            println!("VBlank requested");
        }
    }

    pub fn lcd(&self, value: bool) {
        self.request.lcd.set(value);
        self.signal.raise();
        #[cfg(feature = "debug")]
        if value {
            println!("LCD interrupt requested");
        }
    }

    pub fn timer(&self, value: bool) {
        self.request.timer.set(value);
        self.signal.raise();
        #[cfg(feature = "debug")]
        if value {
            println!("Timer interrupt requested");
        }
    }

    pub fn serial(&self, value: bool) {
        self.request.serial.set(value);
        self.signal.raise();
        #[cfg(feature = "debug")]
        if value {
            println!("serial interrupt requested");
        }
    }

    pub fn joypad(&self, value: bool) {
        self.request.joypad.set(value);
        self.signal.raise();
        #[cfg(feature = "debug")]
        if value {
            println!("Joypad interrupt requested");
        }
    }
}

pub struct InterruptController {
    enable: Rc<Interrupts>,
    flags: Rc<Interrupts>,
    signal: Rc<Signal>,
}

impl InterruptController {
    pub fn new() -> Self {
        Self {
            enable: Rc::new(Interrupts::default()),
            flags: Rc::new(Interrupts::default()),
            signal: Rc::new(Signal::new()),
        }
    }

    pub fn signal(&self) -> &Signal {
        &self.signal
    }

    pub fn request(&self) -> InterruptRequest {
        InterruptRequest::new(self.flags.clone(), self.signal.clone())
    }

    pub fn peek(&self) -> Option<u8> {
        self.check(false)
    }

    pub fn consume(&self) -> Option<u8> {
        self.check(true)
    }

    fn check(&self, consume: bool) -> Option<u8> {
        let (enable, flags) = (&*self.enable, &*self.flags);
        for (enabled, requested, vector) in [
            (&enable.vblank, &flags.vblank, 0x40),
            (&enable.lcd, &flags.lcd, 0x48),
            (&enable.timer, &flags.timer, 0x50),
            (&enable.serial, &flags.serial, 0x58),
            (&enable.joypad, &flags.joypad, 0x60),
        ] {
            if enabled.get() && requested.get() {
                requested.set(!consume);
                return Some(vector);
            }
        }
        None
    }
}

impl MemoryHandler for InterruptController {
    fn read(&self, _: &Mmu, address: u16) -> MemoryRead {
        match address {
            0xffff => MemoryRead::Replace(self.enable.get()),
            0xff0f => MemoryRead::Replace(self.flags.get()),
            _ => {
                #[cfg(feature = "debug")]
                println!("IC received weird read to address 0x{:04X}", address);
                MemoryRead::Pass
            }
        }
    }

    fn write(&self, _: &Mmu, address: u16, value: u8) -> MemoryWrite {
        match address {
            0xffff => {
                self.enable.set(value);
                self.signal.raise();
                MemoryWrite::Block
            }
            0xff0f => {
                self.flags.set(value);
                self.signal.raise();
                MemoryWrite::Block
            }
            _ => {
                #[cfg(feature = "debug")]
                println!("IC received weird write to address 0x{:04X}", address);
                MemoryWrite::Pass
            }
        }
    }
}
