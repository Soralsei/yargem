use std::io::Write;
use std::{
    cell::{Cell, RefCell},
    convert::Infallible,
    rc::Rc,
};

use super::mmu::{MemoryHandler, MemoryRead, MemoryWrite, Mmu};
use crate::clock::{signal, Cpu, Listener, Raiser};
use crate::memory::mmu::Next;
use crate::{
    clock::{Cycles, Timeline},
    cpu::interrupt::InterruptRequest,
    is_bit_set,
};

const CYCLES_TO_SEND: Cycles = 512 * 8; // 8192Hz clock => 512 cpu cycles * 8 bits
const CLOCK_SELECT: u8 = 0;
const CLOCK_SPEED: u8 = 1;
const TRANSFER_ENABLE: u8 = 7;

/// The other end of the link cable.
///
/// A transfer is an exchange, not a send: the same eight clock pulses shift a
/// byte out and a byte in at once. So one call covers both directions, and an
/// implementation that only consumes — a log — leaves `recv` at its default and
/// reads as an unplugged port.
pub trait ByteSink {
    /// The byte a completed transfer shifted out.
    fn send(&self, byte: u8);

    /// The byte that same transfer shifted in. An unplugged port pulls the line
    /// high, which is what a game polling an absent peer expects to see.
    fn recv(&self) -> u8 {
        0xFF
    }
}

/// Nothing plugged in.
pub struct NullSink;

impl ByteSink for NullSink {
    fn send(&self, _byte: u8) {}
}

/// Test ROMs report their results over the link port. Bytes are written and
/// flushed as they arrive, so progress shows up during a run rather than only
/// at the end.
pub struct LogSink;

impl ByteSink for LogSink {
    fn send(&self, byte: u8) {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(&[byte]);
        let _ = out.flush();
    }
}

/// Like the timer, serial is only reached by its own task and its own register
/// handler, so one cell at the boundary keeps the logic on `&mut self`.
pub struct Serial<S: ByteSink> {
    sink: S,
    state: RefCell<SerialState>,
    raiser: Raiser,
    /// Taken by the task when it starts: it is the only consumer.
    listener: Cell<Option<Listener>>,
}

impl<S: ByteSink> Serial<S> {
    pub fn new(interrupt_request: InterruptRequest, sink: S) -> Self {
        let (raiser, listener) = signal();
        Self {
            sink,
            state: RefCell::new(SerialState::new(interrupt_request)),
            raiser,
            listener: Cell::new(Some(listener)),
        }
    }

    pub async fn task(this: Rc<Serial<S>>, timeline: Timeline<Cpu>) -> Infallible {
        let mut writes = this.listener.take().expect("serial task spawned twice");
        loop {
            // Idle until software arms a transfer. No borrow may straddle an
            // await: the SC write handler takes the same cell.
            timeline
                .after(writes.until(|| {
                    let state = this.state.borrow();
                    state.transfer_enable && state.clock_select
                }))
                .await;

            timeline.wait(CYCLES_TO_SEND).await;
            this.state.borrow_mut().complete_transfer(&this.sink);
        }
    }
}

struct SerialState {
    interrupt_request: InterruptRequest,
    send_byte: u8,         // address 0xFF01, holds the received byte afterwards
    transfer_enable: bool, // true if there is an ongoing or pending transfer
    clock_speed: bool,     // CGB only: false: normal, true: fast
    clock_select: bool,    // false: external clock, true : internal
}

impl SerialState {
    fn new(interrupt_request: InterruptRequest) -> Self {
        Self {
            interrupt_request,
            send_byte: 0x0,
            transfer_enable: false,
            clock_speed: false,
            clock_select: true,
        }
    }

    /// Both directions land at once, because on the wire they are the same
    /// eight pulses: SB's outgoing byte leaves and the peer's byte replaces it.
    fn complete_transfer<S: ByteSink>(&mut self, sink: &S) {
        sink.send(self.send_byte);
        self.send_byte = sink.recv();
        self.transfer_enable = false;
        self.interrupt_request.serial(true);
    }

    fn set_sc(&mut self, value: u8) {
        self.transfer_enable = is_bit_set!(value, TRANSFER_ENABLE);
        self.clock_speed = is_bit_set!(value, CLOCK_SPEED);
        self.clock_select = is_bit_set!(value, CLOCK_SELECT);
    }

    fn get_sc(&self) -> u8 {
        let mut res = 0;
        res |= (self.transfer_enable as u8) << TRANSFER_ENABLE;
        res |= (self.clock_speed as u8) << CLOCK_SPEED;
        res |= (self.clock_select as u8) << CLOCK_SELECT;
        res
    }
}

impl<S: ByteSink> MemoryHandler for Serial<S> {
    fn read(&self, address: u16, _next: Next<'_>) -> u8 {
        self.state.borrow().read(address)
    }

    fn write(&self, address: u16, value: u8, _next: Next<'_>) {
        self.state.borrow_mut().write(address, value);
        self.raiser.raise();
    }
}

impl SerialState {
    fn read(&self, address: u16) -> u8 {
        match address {
            0xFF01 => self.send_byte,
            0xFF02 => self.get_sc(),
            _ => unreachable!("Invalid serial read : 0x{:04X}", address),
        }
    }

    fn write(&mut self, address: u16, value: u8) {
        match address {
            0xFF01 => {
                self.send_byte = value;
            }
            0xFF02 => {
                self.set_sc(value);
            }
            _ => unreachable!("Invalid serial write : 0x{:04X}", address),
        }
    }
}
