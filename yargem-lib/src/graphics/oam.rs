#![allow(dead_code)]

use std::{cell::Cell, convert::Infallible, rc::Rc};

use crate::{
    clock::{Cpu, Cycles, Signal, Timeline, M_CYCLE},
    memory::{
        bus::{Bus, BusController, BusOwner},
        mmu::{MemoryHandler, MemoryRead, MemoryWrite, Mmu},
    },
};

#[derive(Clone, Copy)]
enum OAMTransferState {
    Idle,
    InProgress,
}

struct DMAState {
    pub src_addr_reg: Cell<u8>,
    pub requested: Cell<bool>,
}

impl DMAState {
    pub fn new() -> Self {
        Self {
            src_addr_reg: Cell::new(0xFF),
            requested: Cell::new(false),
        }
    }
}

pub struct DMAController {
    bus_controller: Rc<BusController>,
    state: DMAState,
    signal: Signal,
}

impl DMAController {
    pub fn new(bus_controller: Rc<BusController>) -> Self {
        Self {
            bus_controller,
            state: DMAState::new(),
            signal: Signal::new(),
        }
    }

    pub async fn task(
        this: Rc<DMAController>,
        mmu: Rc<Mmu>,
        timeline: Timeline<Cpu>,
    ) -> Infallible {
        'outer: loop {
            timeline
                .after(this.signal.until(|| this.state.requested.take()))
                .await;
            let src = (this.state.src_addr_reg.get() as u16) << 8;
            for offset in 0..0xA0u16 {
                let src_addr = src | offset;
                timeline.wait(M_CYCLE as Cycles).await;
                // resume checkpoint for save states go here
                // ...
                let value = mmu.peek(src_addr);

                // Idempotent
                this.bus_controller.seize(Bus::Oam, BusOwner::Dma, 0xFF);
                this.bus_controller
                    .seize_bus_of(src_addr, BusOwner::Dma, value);

                mmu.poke(0xFE00 | offset, value);

                if this.state.requested.get() {
                    // Restart DMA transfer if transfer enabled in the middle of another transfer
                    this.bus_controller.release_bus_for(src, BusOwner::Dma);
                    continue 'outer;
                }
            }
            this.bus_controller.release_bus_for(src, BusOwner::Dma);
            this.bus_controller.release(Bus::Oam, BusOwner::Dma);
        }
    }
}

impl MemoryHandler for DMAController {
    fn read(&self, _: &Mmu, address: u16) -> MemoryRead {
        if address != 0xFF46 {
            unreachable!("Invalid read in DMAController at address 0x{:04X}", address)
        }
        MemoryRead::Replace(self.state.src_addr_reg.get())
    }

    fn write(&self, _: &Mmu, address: u16, value: u8) -> MemoryWrite {
        if address != 0xFF46 {
            unreachable!(
                "Invalid write in DMAController at address 0x:{:04X}",
                address
            )
        }

        self.state.src_addr_reg.set(value);
        self.state.requested.set(true);
        self.signal.raise();

        MemoryWrite::Block
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Time;
    use crate::memory::mmu::AddressMap;

    const WRAM_PAGE: u8 = 0xC0;
    const VRAM_PAGE: u8 = 0x80;

    /// Drives the DMA alone over plain memory: with nothing else mapped, the MMU
    /// stores every byte, so OAM and both source pages are ordinary RAM.
    struct Harness {
        time: Time,
        mmu: Rc<Mmu>,
        bus: Rc<BusController>,
    }

    impl Harness {
        fn new() -> Self {
            let mut time = Time::new();
            let bus = Rc::new(BusController::new());
            let dma = Rc::new(DMAController::new(bus.clone()));
            let mut map = AddressMap::new();
            map.add((0xFF46, 0xFF46), dma.clone());
            let mmu = Rc::new(Mmu::new(map));
            time.spawn(|t: Timeline<Cpu>| DMAController::task(dma, mmu.clone(), t));
            Self { time, mmu, bus }
        }

        /// Every byte of the page distinct, so a byte from the wrong transfer or
        /// the wrong offset shows up.
        fn fill(&self, page: u8, base: u8) {
            for offset in 0..0xA0u16 {
                let address = (page as u16) << 8 | offset;
                self.mmu.poke(address, base.wrapping_add(offset as u8));
            }
        }

        /// Called between ticks on an M-cycle boundary, which is where the CPU
        /// writes: after the DMA's poll for that cycle.
        fn start(&self, page: u8) {
            self.mmu.poke(0xFF46, page);
        }

        fn m_cycles(&mut self, n: u32) {
            for _ in 0..n * M_CYCLE as u32 {
                self.time.tick();
            }
        }

        fn oam(&self, offset: u16) -> u8 {
            self.mmu.peek(0xFE00 + offset)
        }

        fn locked(&self, address: u16) -> bool {
            self.bus.conflict(address).is_some()
        }

        fn assert_oam_holds(&self, base: u8) {
            for offset in 0..0xA0u16 {
                assert_eq!(
                    self.oam(offset),
                    base.wrapping_add(offset as u8),
                    "OAM byte {offset:#04X}"
                );
            }
        }
    }

    #[test]
    fn fresh_transfer_starts_after_one_setup_cycle() {
        let mut h = Harness::new();
        h.fill(WRAM_PAGE, 0x10);
        h.m_cycles(3);
        h.start(WRAM_PAGE);

        h.m_cycles(1);
        assert!(!h.locked(0xFE00), "OAM locked during the setup cycle");
        assert_eq!(h.oam(0), 0, "copied during the setup cycle");

        h.m_cycles(1);
        assert_eq!(h.oam(0), 0x10);
        assert_eq!(h.oam(1), 0);

        for _ in 1..0x9F {
            assert!(h.locked(0xFE00), "OAM released mid-transfer");
            h.m_cycles(1);
        }
        assert!(h.locked(0xFE00));
        assert_eq!(h.oam(0x9F), 0, "last byte copied early");

        h.m_cycles(1);
        h.assert_oam_holds(0x10);
        assert!(!h.locked(0xFE00), "OAM still held after the transfer");
        assert!(
            !h.locked(0xC000),
            "source bus still held after the transfer"
        );
    }

    #[test]
    fn restart_mid_transfer_keeps_oam_locked() {
        let mut h = Harness::new();
        h.fill(WRAM_PAGE, 0x10);
        h.fill(0xD0, 0x80);
        h.start(WRAM_PAGE);
        // Bytes 0..=10 copied, one per M-cycle from M-cycle 2.
        h.m_cycles(12);
        h.start(0xD0);

        // The new transfer's setup cycle still belongs to the old one.
        h.m_cycles(1);
        assert!(h.locked(0xFE00), "OAM released during the handover");
        assert_eq!(h.oam(11), 0x10 + 11);
        assert_eq!(h.oam(0), 0x10, "new transfer started without a setup cycle");

        h.m_cycles(1);
        assert_eq!(h.oam(0), 0x80);

        for _ in 1..0xA0 {
            assert!(h.locked(0xFE00), "OAM released mid-transfer");
            h.m_cycles(1);
        }
        h.assert_oam_holds(0x80);
        assert!(!h.locked(0xFE00));
    }

    #[test]
    fn restart_during_the_last_byte_keeps_oam_locked() {
        let mut h = Harness::new();
        h.fill(WRAM_PAGE, 0x10);
        h.fill(0xD0, 0x80);
        h.start(WRAM_PAGE);
        // Byte 0x9E lands on M-cycle 160; the last one is due next.
        h.m_cycles(160);
        h.start(0xD0);

        h.m_cycles(1);
        assert_eq!(h.oam(0x9F), 0x10 + 0x9F, "old transfer cut short");
        assert!(h.locked(0xFE00), "OAM released between transfers");

        h.m_cycles(1);
        assert!(h.locked(0xFE00));
        assert_eq!(h.oam(0), 0x80);
    }

    #[test]
    fn restart_onto_another_bus_hands_the_source_bus_over() {
        let mut h = Harness::new();
        h.fill(VRAM_PAGE, 0x20);
        h.fill(WRAM_PAGE, 0x10);
        h.start(VRAM_PAGE);
        h.m_cycles(7);
        assert!(h.locked(0x8000), "VRAM source not held");
        assert!(!h.locked(0xC000));

        h.start(WRAM_PAGE);
        h.m_cycles(2);
        assert!(!h.locked(0x8000), "old source bus still held");
        assert!(h.locked(0xC000), "new source bus not held");

        h.m_cycles(0xA0);
        h.assert_oam_holds(0x10);
        assert!(!h.locked(0x8000));
        assert!(!h.locked(0xC000));
        assert!(!h.locked(0xFE00));
    }
}
