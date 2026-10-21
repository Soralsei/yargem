use std::{cell::Cell, rc::Rc};

/// The address map, built with `&mut` before the bus is shared. `Mmu::new`
/// takes it by value, so no handler can be added once dispatch is possible.
pub struct AddressMap(Box<[Vec<Rc<dyn MemoryHandler>>]>);

impl AddressMap {
    pub fn new() -> Self {
        Self(
            (0..0x10000)
                .map(|_| Vec::with_capacity(2))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        )
    }

    /// Registration order is dispatch order: the first handler to answer wins.
    pub fn add(&mut self, address_range: (u16, u16), handler: Rc<dyn MemoryHandler>) {
        for address in address_range.0..=address_range.1 {
            self.0[address as usize].push(handler.clone());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRead {
    Replace(u8),
    Pass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryWrite {
    // Replace(u8),
    Pass,
    Block,
}

/// A memory-mapped device. Handlers are shared (`Rc`) and keep their own
/// interior mutability, so a bus access never needs a mutable borrow of the
/// device and cannot conflict with that device being stepped by the clock.
pub trait MemoryHandler {
    fn read(&self, mmu: &Mmu, address: u16) -> MemoryRead;
    fn write(&self, mmu: &Mmu, address: u16, value: u8) -> MemoryWrite;
    fn enabled(&self) -> bool {
        true
    }
}

pub struct Mmu {
    /// Indexed by address rather than keyed by it. The map is dense, so a tree
    /// walk per access buys nothing over an indexed load. Costs 1.5MB.
    handlers: AddressMap,
    /// `Cell<u8>` is `repr(transparent)`, so this has the layout and access cost
    /// of `[u8; 0x10000]`. Interior mutability is what lets the bus be shared as
    /// an `Rc<Mmu>`: a clocked device (OAM DMA, the PPU fetcher) only ever holds
    /// `&Mmu`, and still has to be able to drive a write.
    memory: Box<[Cell<u8>]>,
}

impl Mmu {
    pub fn new(address_map: AddressMap) -> Mmu {
        Mmu {
            handlers: address_map,
            memory: vec![Cell::new(0u8); 0x10000].into_boxed_slice(),
        }
    }

    /// Read a byte. The address map only: no machine cycle, no arbitration.
    /// The CPU reaches memory through `CpuBus`, which adds both.
    pub fn peek(&self, addr: u16) -> u8 {
        let handlers = &self.handlers.0;
        for handler in handlers[addr as usize]
            .iter()
            .filter(|handler| handler.enabled())
        {
            match handler.read(self, addr) {
                MemoryRead::Replace(value) => return value,
                MemoryRead::Pass => (),
            }
        }

        match addr {
            // echo ram read
            0xE000..=0xFDFF => self.memory[(addr - 0x2000) as usize].get(),
            // normal ram read
            _ => self.memory[addr as usize].get(),
        }
    }

    /// Write a byte. The address map only: no machine cycle, no arbitration.
    /// This is the path a clocked device uses to drive the bus it owns.
    pub fn poke(&self, addr: u16, value: u8) {
        let handlers = &self.handlers.0;
        let mut outcome = MemoryWrite::Pass;
        for handler in handlers[addr as usize]
            .iter()
            .filter(|handler| handler.enabled())
        {
            match handler.write(self, addr, value) {
                MemoryWrite::Pass => (),
                decided => {
                    outcome = decided;
                    break;
                }
            }
        }

        let value = match outcome {
            MemoryWrite::Block => return,
            // MemoryWrite::Replace(replacement) => replacement,
            MemoryWrite::Pass => value,
        };

        match addr {
            // echo ram write
            0xE000..=0xFDFF => self.memory[(addr - 0x2000) as usize].set(value),
            // normal ram write
            _ => self.memory[addr as usize].set(value),
        }
    }
}
