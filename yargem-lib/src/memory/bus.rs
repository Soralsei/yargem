use std::{cell::Cell, rc::Rc};

use crate::memory::mmu::Mmu;

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum Bus {
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

pub struct BusController {
    owners: [Cell<u8>; 3],   // indexed by Bus, mask of BusOwner
    conflict: [Cell<u8>; 3], // value the CPU sees while held
}

impl BusController {
    pub fn new() -> Self {
        Self {
            owners: [const { Cell::new(0) }; 3],
            conflict: [const { Cell::new(0xFF) }; 3],
        }
    }

    pub fn conflict(&self, address: u16) -> Option<u8> {
        let bus = BusController::bus_for(address)? as usize;
        (self.owners[bus].get() != 0).then(|| self.conflict[bus].get())
    }

    fn bus_for(address: u16) -> Option<Bus> {
        match address {
            0xFE00..=0xFE9F => Some(Bus::Oam),
            0x8000..=0x9FFF => Some(Bus::Video),
            0x0000..=0x7FFF | 0xA000..=0xFDFF => Some(Bus::External),
            _ => None,
        }
    }

    pub fn seize(&self, bus: Bus, owner: BusOwner, conflict_value: u8) {
        self.owners[bus as usize].update(|mask| mask | owner as u8);
        self.conflict[bus as usize].set(conflict_value);
    }

    pub fn seize_bus_of(&self, address: u16, owner: BusOwner, conflict_value: u8) {
        if let Some(bus) = BusController::bus_for(address) {
            self.seize(bus, owner, conflict_value);
        }
    }

    pub fn release(&self, bus: Bus, owner: BusOwner) {
        self.owners[bus as usize].update(|mask| mask & !(owner as u8));
    }

    pub fn release_bus_for(&self, address: u16, owner: BusOwner) {
        if let Some(bus) = BusController::bus_for(address) {
            self.release(bus, owner);
        }
    }
}

/// The CPU's view of the bus. A bus access here costs a machine cycle and loses
/// to whoever else is driving that bus.
///
/// Clocked devices hold the `Rc<Mmu>` directly and use `peek`/`poke`, which is
/// what keeps them out of their own arbitration: OAM DMA must not be blocked by
/// the OAM lock it holds itself. `Mmu` deliberately has no ticking accessor, so
/// taking the arbitrated path by accident is not possible.
pub struct CpuBus {
    mmu: Rc<Mmu>,
    bus_controller: Rc<BusController>,
}

impl CpuBus {
    pub fn new(mmu: Rc<Mmu>, bus_controller: Rc<BusController>) -> Self {
        Self {
            mmu,
            bus_controller,
        }
    }

    pub fn read(&self, address: u16) -> u8 {
        match self.bus_controller.conflict(address) {
            // Whoever owns the bus is driving it; the CPU sees their value.
            Some(conflict) => conflict,
            None => self.mmu.read(address),
        }
    }

    pub fn write(&self, address: u16, value: u8) {
        if self.bus_controller.conflict(address).is_none() {
            self.mmu.write(address, value);
        }
    }

    /// Inspect without arbitration, for the places the
    /// CPU reads state rather than driving the bus (STOP checking IE/IF/JOYP).
    pub fn peek(&self, address: u16) -> u8 {
        self.mmu.read(address)
    }

    /// Write without arbitration, for the places
    /// the CPU alters state rather than driving the bus (STOP resetting DIV).
    pub fn poke(&self, address: u16, value: u8) {
        self.mmu.write(address, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VRAM: u16 = 0x8000;
    const OAM: u16 = 0xFE00;

    #[test]
    fn a_held_bus_reads_the_conflict_value() {
        let bus = BusController::new();
        assert_eq!(bus.conflict(VRAM), None);

        bus.seize(Bus::Video, BusOwner::Ppu, 0xFF);
        assert_eq!(bus.conflict(VRAM), Some(0xFF));

        bus.release(Bus::Video, BusOwner::Ppu);
        assert_eq!(bus.conflict(VRAM), None);
    }

    #[test]
    fn releasing_one_owner_leaves_the_other_holding() {
        let bus = BusController::new();
        // OAM DMA holds OAM across a whole transfer; the PPU takes and drops it
        // every scanline. The PPU's release must not open the bus under the DMA.
        bus.seize(Bus::Oam, BusOwner::Dma, 0xFF);
        bus.seize(Bus::Oam, BusOwner::Ppu, 0xFF);

        bus.release(Bus::Oam, BusOwner::Ppu);
        assert_eq!(bus.conflict(OAM), Some(0xFF), "DMA still holds OAM");

        bus.release(Bus::Oam, BusOwner::Dma);
        assert_eq!(bus.conflict(OAM), None);
    }

    #[test]
    fn seizing_twice_needs_only_one_release() {
        let bus = BusController::new();
        // The DMA re-seizes every M-cycle of its transfer.
        bus.seize(Bus::Oam, BusOwner::Dma, 0xFF);
        bus.seize(Bus::Oam, BusOwner::Dma, 0xFF);
        bus.release(Bus::Oam, BusOwner::Dma);
        assert_eq!(bus.conflict(OAM), None, "ownership is a mask, not a count");
    }

    #[test]
    fn registers_are_never_arbitrated() {
        let bus = BusController::new();
        bus.seize(Bus::Video, BusOwner::Ppu, 0xFF);
        bus.seize(Bus::Oam, BusOwner::Ppu, 0xFF);
        assert_eq!(
            bus.conflict(0xFF40),
            None,
            "LCDC is not on an arbitrated bus"
        );
        assert_eq!(bus.conflict(0xFF80), None, "HRAM stays reachable");
    }
}
