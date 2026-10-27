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

#[derive(Clone, Copy)]
pub enum Source {
    Cpu,
    Dma,
    Debugger,
    Internal,
}

#[derive(Clone, Copy)]
pub struct MemoryContext<'a> {
    pub mmu: &'a Mmu,
    pub next: &'a [Rc<dyn MemoryHandler>],
    pub source: Source,
}

impl MemoryContext<'_> {
    pub fn read(self, addr: u16) -> u8 {
        match self.next.split_first() {
            Some((handler, rest)) => handler.read(addr, MemoryContext { next: rest, ..self }),
            None => self.mmu.ram_read(addr), // echo mapping + backing array
        }
    }

    pub fn write(self, addr: u16, value: u8) {
        match self.next.split_first() {
            Some((handler, rest)) => {
                handler.write(addr, value, MemoryContext { next: rest, ..self })
            }
            None => self.mmu.ram_write(addr, value),
        }
    }

    pub fn source(&self) -> Source {
        self.source
    }

    pub fn as_source(self, source: Source) -> Self {
        Self { source, ..self }
    }
}

/// A memory-mapped device. Handlers are shared (`Rc`) and keep their own
/// interior mutability, so a bus access never needs a mutable borrow of the
/// device and cannot conflict with that device being stepped by the clock.
pub trait MemoryHandler {
    fn read(&self, address: u16, context: MemoryContext<'_>) -> u8 {
        context.read(address)
    }
    fn write(&self, address: u16, value: u8, context: MemoryContext<'_>) {
        context.write(address, value);
    }

    /// Returns the currently mapped bank (if any) at `addr`
    fn bank_at(&self, _: u16) -> Option<u16> {
        None
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
    memory: Box<[Cell<u8>; 0x10000]>,
}

impl Mmu {
    pub fn new(address_map: AddressMap) -> Mmu {
        Mmu {
            handlers: address_map,
            memory: vec![Cell::new(0u8); 0x10000]
                .into_boxed_slice()
                .try_into()
                .expect("Mmu::new can't fail"),
        }
    }

    /// Read a byte. The address map only: no machine cycle, no arbitration.
    /// The CPU reaches memory through `CpuBus`, which adds both.
    pub fn read(&self, addr: u16, source: Source) -> u8 {
        let maybe_handlers = self.handlers.0[addr as usize].split_first();
        if let Some((handler, rest)) = maybe_handlers {
            return handler.read(
                addr,
                MemoryContext {
                    mmu: self,
                    next: rest,
                    source,
                },
            );
        }
        self.ram_read(addr)
    }

    pub fn ram_read(&self, addr: u16) -> u8 {
        match addr {
            // echo ram read
            0xE000..=0xFDFF => self.memory[(addr - 0x2000) as usize].get(),
            // normal ram read
            _ => self.memory[addr as usize].get(),
        }
    }

    /// Write a byte. The address map only: no machine cycle, no arbitration.
    /// This is the path a clocked device uses to drive the bus it owns.
    pub fn write(&self, addr: u16, value: u8, source: Source) {
        let maybe_handlers = self.handlers.0[addr as usize].split_first();
        if let Some((handler, rest)) = maybe_handlers {
            return handler.write(
                addr,
                value,
                MemoryContext {
                    mmu: self,
                    next: rest,
                    source,
                },
            );
        }
        self.ram_write(addr, value);
    }

    pub fn ram_write(&self, addr: u16, value: u8) {
        match addr {
            // echo ram write
            0xE000..=0xFDFF => self.memory[(addr - 0x2000) as usize].set(value),
            // normal ram write
            _ => self.memory[addr as usize].set(value),
        }
    }

    pub fn bank_at(&self, addr: u16) -> Option<u16> {
        self.handlers.0[addr as usize]
            .iter()
            .find_map(|handler| handler.bank_at(addr))
    }
}
