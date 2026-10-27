use std::cell::Cell;

use crate::{
    cpu::interrupt::InterruptRequest,
    memory::mmu::{MemoryContext, MemoryHandler},
};

#[derive(Copy, Clone)]
pub enum Button {
    A,
    B,
    Select,
    Start,
    Right,
    Left,
    Up,
    Down,
}

enum Group {
    Buttons,
    Dpad,
}

impl Button {
    fn line(self) -> (Group, u8) {
        match self {
            Button::Start => (Group::Buttons, 0x08),
            Button::Select => (Group::Buttons, 0x04),
            Button::B => (Group::Buttons, 0x02),
            Button::A => (Group::Buttons, 0x01),
            Button::Down => (Group::Dpad, 0x08),
            Button::Up => (Group::Dpad, 0x04),
            Button::Left => (Group::Dpad, 0x02),
            Button::Right => (Group::Dpad, 0x01),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Joypad {
    select_bits: u8,     // bits 5-4 as written
    buttons_pressed: u8, // bit 3..0 = Start, Select, B, A   (1 = pressed)
    dpad_pressed: u8,    // bit 3..0 = Down, Up, Left, Right
}

impl Joypad {
    pub fn select(&mut self, value: u8) {
        // ignore bits other than 5 and 4
        self.select_bits = value & 0x30;
    }

    pub fn get(&self) -> u8 {
        let mut inputs = 0x0F;
        if self.select_bits & 0x20 == 0 {
            inputs &= !self.buttons_pressed;
        }
        if self.select_bits & 0x10 == 0 {
            inputs &= !self.dpad_pressed;
        }
        0xC0 | self.select_bits | (inputs & 0x0F)
    }

    pub fn set(&mut self, button: Button, down: bool) {
        let (group, btn) = button.line();

        let btn_group = match group {
            Group::Buttons => &mut self.buttons_pressed,
            Group::Dpad => &mut self.dpad_pressed,
        };

        if down {
            *btn_group |= btn;
        } else {
            *btn_group &= !btn;
        }
    }
}

/// `Joypad` is `Copy`, so the whole state fits in one `Cell`: get, mutate the
/// copy, set. No borrow semantics on the bus path at all.
pub struct JoypadHandler {
    interrupt_request: InterruptRequest,
    joypad: Cell<Joypad>,
}

impl JoypadHandler {
    pub fn new(interrupt_request: InterruptRequest) -> Self {
        Self {
            interrupt_request,
            joypad: Cell::new(Joypad::default()),
        }
    }

    pub fn set(&self, button: Button, down: bool) {
        self.state_change(|joypad| joypad.set(button, down))
    }

    fn state_change(&self, change: impl FnOnce(&mut Joypad)) {
        let mut joypad = self.joypad.get();
        let before = joypad.get() & 0x0F;
        change(&mut joypad);
        let after = joypad.get() & 0x0F;
        self.joypad.set(joypad);

        let fell = before & !after != 0;
        if fell {
            self.interrupt_request.joypad(true);
        }
    }
}

impl MemoryHandler for JoypadHandler {
    fn read(&self, address: u16, _ctx: MemoryContext<'_>) -> u8 {
        if address == 0xFF00 {
            return self.joypad.get().get();
        }
        unreachable!("Invalid read in JoypadHandler at address 0x{:04X}", address)
    }

    fn write(&self, address: u16, value: u8, _ctx: MemoryContext<'_>) {
        if address == 0xFF00 {
            self.state_change(|joypad| joypad.select(value));
            return;
        }
        unreachable!(
            "unhandled write in JoypadHandler at address 0x{:04X}",
            address
        )
    }
}
