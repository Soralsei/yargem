use std::cell::{Cell, RefCell};

use bitvec::array::BitArray;
use slab::Slab;

use crate::{
    debug::{breakpoint::Breakpoint, protocol::WatchpointHit, watchpoint::Watchpoint},
    memory::mmu::{MemoryHandler, MemoryRead, MemoryWrite, Mmu},
};

struct DebuggerState {
    watchpoints: Slab<Watchpoint>,
    breakpoints: Slab<Breakpoint>,
    watched_map: BitArray<[u64; 1024]>,

    was_hit: bool,
}

impl DebuggerState {
    pub fn new() -> Self {
        Self {
            watchpoints: Slab::with_capacity(30),
            breakpoints: Slab::with_capacity(30),
            watched_map: BitArray::ZERO,
            was_hit: false,
        }
    }

    fn watchpoint_hit(&mut self, addr: u16, bank: Option<u16>) -> bool {
        let hit = self
            .watchpoints
            .iter()
            .any(|(_, watchpoint)| watchpoint.hit(addr, bank));
        self.was_hit |= hit;
        hit
    }
}

pub struct Debugger {
    state: RefCell<DebuggerState>,
    enabled: Cell<bool>,
}

impl Debugger {
    pub fn new() -> Self {
        Self {
            state: RefCell::new(DebuggerState::new()),
            enabled: false.into(),
        }
    }

    // pub fn take_pending_watch_hit(&self) -> Option<WatchpointHit> {
    //     let was_hit = std::mem::take(&mut self.state.borrow_mut().was_hit);
    //     if !was_hit {
    //         return None
    //     }
    //     Some(WatchpointHit {
    //
    //     })
    // }

    // pub fn watch(&self, watchpoint_req: W)
}

impl MemoryHandler for Debugger {
    fn read(&self, mmu: &Mmu, address: u16) -> MemoryRead {
        if self.enabled.get() {
            let mut state = self.state.borrow_mut();
            let is_watched = unsafe { *state.watched_map.get_unchecked(address as usize) };
            if is_watched {
                let bank = mmu.bank_at(address);
                state.watchpoint_hit(address, bank);
            }
        }
        MemoryRead::Pass
    }

    fn write(&self, _: &Mmu, address: u16, value: u8) -> MemoryWrite {
        todo!()
    }
}
