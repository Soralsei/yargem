use std::{cell::Cell, rc::Rc};

use crate::{
    is_bit_set,
    memory::{
        bus::Bus,
        mmu::{MemoryContext, MemoryHandler},
    },
    util::bit_operations::bytes_to_word,
};

#[derive(Debug, Clone, Copy)]
enum DmaType {
    General,
    HBlank,
}

#[derive(Clone, Copy, Debug)]
struct DmaControl(u8);

impl DmaControl {
    fn dma_type(&self) -> DmaType {
        if is_bit_set!(self.0, 7) {
            DmaType::HBlank
        } else {
            DmaType::General
        }
    }

    fn len(&self) -> u8 {
        self.0 & 0x7F
    }
}

struct HdmaState {
    source_hi: Cell<u8>, // HDMA1
    source_lo: Cell<u8>, // HDMA2
    dest_hi: Cell<u8>,   // HDMA3
    dest_lo: Cell<u8>,   // HDMA4
    remaining: Cell<u8>, // HDMA5
    active: Cell<bool>,
}

impl HdmaState {
    fn source(&self) -> u16 {
        bytes_to_word(self.source_hi.get(), self.source_lo.get())
    }

    fn dest(&self) -> u16 {
        bytes_to_word(self.dest_hi.get(), self.dest_lo.get())
    }
}

pub struct HdmaController {
    bus: Rc<Bus>,
    state: HdmaState,
}

impl MemoryHandler for HdmaController {
    fn read(&self, address: u16, _: MemoryContext<'_>) -> u8 {
        match address {
            0xFF51..=0xFF54 => 0xFF,
            0xFF55 => (if self.state.active.get() { 0 } else { 0x80 }) | self.state.remaining.get(),
            _ => unreachable!("invalid read in HdmaController at 0x{address:04X}"),
        }
    }

    fn write(&self, address: u16, value: u8, _: MemoryContext<'_>) {
        match address {
            0xFF51 => self.state.source_hi.set(value),
            0xFF52 => self.state.source_lo.set(value & 0xF0),
            0xFF53 => self.state.dest_hi.set(0x1F),
            0xFF54 => self.state.dest_lo.set(value & 0xF0),
            0xFF55 => {
                // let new = DmaControl(value);
                // // TODO start DMA
                // if control.dma_type() == DmaType::HBlank {}
                // self.state.dma_control.set(DmaControl(value));
            }
            _ => unreachable!("invalid read in HdmaController at 0x{address:04X}"),
        };
    }
}
