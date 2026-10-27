use std::{cell::Cell, rc::Rc};

use crate::memory::mmu::{MemoryContext, MemoryHandler, Mmu, Source};

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum BusType {
    External = 0,
    Video = 1,
    Oam = 2,
}

/// Who is driving a bus. A bus can be held by more than one at once — OAM DMA
/// holds OAM across a whole transfer, which spans several PPU mode changes —
/// so ownership is a mask and the bus unlocks only once every holder is gone.
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum BusOwner {
    Dma = 1 << 0,
    Ppu = 1 << 1,
}

pub struct Bus {
    owners: [Cell<u8>; 3],   // indexed by Bus, mask of BusOwner
    conflict: [Cell<u8>; 3], // value the CPU sees while held
}

impl Bus {
    pub fn new() -> Self {
        Self {
            owners: [const { Cell::new(0) }; 3],
            conflict: [const { Cell::new(0xFF) }; 3],
        }
    }

    pub fn conflict(&self, address: u16) -> Option<u8> {
        let bus = Bus::bus_for(address)? as usize;
        (self.owners[bus].get() != 0).then(|| self.conflict[bus].get())
    }

    fn bus_for(address: u16) -> Option<BusType> {
        match address {
            0xFE00..=0xFE9F => Some(BusType::Oam),
            0x8000..=0x9FFF => Some(BusType::Video),
            0x0000..=0x7FFF | 0xA000..=0xFDFF => Some(BusType::External),
            _ => None,
        }
    }

    pub fn seize(&self, bus: BusType, owner: BusOwner, conflict_value: u8) {
        self.owners[bus as usize].update(|mask| mask | owner as u8);
        self.conflict[bus as usize].set(conflict_value);
    }

    pub fn seize_bus_of(&self, address: u16, owner: BusOwner, conflict_value: u8) {
        if let Some(bus) = Bus::bus_for(address) {
            self.seize(bus, owner, conflict_value);
        }
    }

    pub fn release(&self, bus: BusType, owner: BusOwner) {
        self.owners[bus as usize].update(|mask| mask & !(owner as u8));
    }

    pub fn release_bus_for(&self, address: u16, owner: BusOwner) {
        if let Some(bus) = Bus::bus_for(address) {
            self.release(bus, owner);
        }
    }
}

impl MemoryHandler for Bus {
    fn read(&self, address: u16, ctx: MemoryContext<'_>) -> u8 {
        match (ctx.source(), self.conflict(address)) {
            (Source::Cpu, Some(conflict)) => conflict,
            _ => ctx.read(address),
        }
    }

    fn write(&self, address: u16, value: u8, ctx: MemoryContext<'_>) {
        match (ctx.source(), self.conflict(address)) {
            (Source::Cpu, Some(_)) => {}
            _ => ctx.write(address, value),
        }
    }
}

/// Cpu's view of the memory bus
pub struct CpuBus(pub Rc<Mmu>);

impl CpuBus {
    pub fn read(&self, address: u16) -> u8 {
        self.0.read(address, Source::Cpu)
    }

    pub fn write(&self, address: u16, value: u8) {
        self.0.write(address, value, Source::Cpu);
    }

    // Not arbitered by the Bus (ignores conflicts)
    pub fn peek(&self, address: u16) -> u8 {
        self.0.read(address, Source::Internal)
    }

    // Not arbitered by the bus (ignores conflicts)
    pub fn poke(&self, address: u16, value: u8) {
        self.0.write(address, value, Source::Internal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VRAM: u16 = 0x8000;
    const OAM: u16 = 0xFE00;

    #[test]
    fn a_held_bus_reads_the_conflict_value() {
        let bus = Bus::new();
        assert_eq!(bus.conflict(VRAM), None);

        bus.seize(BusType::Video, BusOwner::Ppu, 0xFF);
        assert_eq!(bus.conflict(VRAM), Some(0xFF));

        bus.release(BusType::Video, BusOwner::Ppu);
        assert_eq!(bus.conflict(VRAM), None);
    }

    #[test]
    fn releasing_one_owner_leaves_the_other_holding() {
        let bus = Bus::new();
        // OAM DMA holds OAM across a whole transfer; the PPU takes and drops it
        // every scanline. The PPU's release must not open the bus under the DMA.
        bus.seize(BusType::Oam, BusOwner::Dma, 0xFF);
        bus.seize(BusType::Oam, BusOwner::Ppu, 0xFF);

        bus.release(BusType::Oam, BusOwner::Ppu);
        assert_eq!(bus.conflict(OAM), Some(0xFF), "DMA still holds OAM");

        bus.release(BusType::Oam, BusOwner::Dma);
        assert_eq!(bus.conflict(OAM), None);
    }

    #[test]
    fn seizing_twice_needs_only_one_release() {
        let bus = Bus::new();
        // The DMA re-seizes every M-cycle of its transfer.
        bus.seize(BusType::Oam, BusOwner::Dma, 0xFF);
        bus.seize(BusType::Oam, BusOwner::Dma, 0xFF);
        bus.release(BusType::Oam, BusOwner::Dma);
        assert_eq!(bus.conflict(OAM), None, "ownership is a mask, not a count");
    }

    #[test]
    fn registers_are_never_arbitrated() {
        let bus = Bus::new();
        bus.seize(BusType::Video, BusOwner::Ppu, 0xFF);
        bus.seize(BusType::Oam, BusOwner::Ppu, 0xFF);
        assert_eq!(
            bus.conflict(0xFF40),
            None,
            "LCDC is not on an arbitrated bus"
        );
        assert_eq!(bus.conflict(0xFF80), None, "HRAM stays reachable");
    }
}
