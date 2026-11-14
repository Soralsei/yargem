use std::{cell::Cell, ops::Deref, rc::Rc};

use serde::{Deserialize, Serialize};

use crate::{
    cpu::interrupt::InterruptRequest,
    executor::{signal, Listener, Raiser, Until, Untimed},
    memory::mmu::{MemoryContext, MemoryHandler},
};

#[derive(Copy, Clone, Serialize, Deserialize)]
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

pub struct InputLine {
    listener: Listener,
    got_input: Rc<Cell<bool>>,
}

impl InputLine {
    pub fn until_input(&mut self) -> Untimed<Until<'_, impl Fn() -> bool + '_>> {
        self.got_input.set(false);
        self.listener.until(|| self.got_input.take())
    }
}

/// `Joypad` is `Copy`, so the whole state fits in one `Cell`: get, mutate the
/// copy, set. No borrow semantics on the bus path at all.
pub struct JoypadHandler {
    interrupt_request: InterruptRequest,
    joypad: Cell<Joypad>,
    input_line: Cell<Option<InputLine>>,
    input_pulse: Raiser,
    got_input: Rc<Cell<bool>>,
}

impl JoypadHandler {
    pub fn new(interrupt_request: InterruptRequest) -> Self {
        let (input_pulse, listener) = signal();
        let got_input = Rc::new(Cell::new(false));

        let input_line = Cell::new(Some(InputLine {
            got_input: got_input.clone(),
            listener,
        }));
        Self {
            interrupt_request,
            joypad: Cell::new(Joypad::default()),
            got_input,
            input_line,
            input_pulse,
        }
    }

    pub fn take_input_line(&self) -> InputLine {
        self.input_line.take().expect("Input line was taken twice")
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
            self.got_input.set(true);
            self.input_pulse.raise();
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
