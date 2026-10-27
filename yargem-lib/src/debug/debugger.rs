use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use crate::{
    cpu::cpu::Cpu,
    debug::{
        breakpoint::Breakpoints,
        protocol::{
            BankAddr, BankRange, DebugCommand, DebugEvent, StopReason, WatchId, WatchpointHit,
        },
        watchpoint::{AccessKind, WatchSet, Watchpoint},
    },
    memory::mmu::{MemoryContext, MemoryHandler, Mmu, Source},
};

pub struct WatchTap {
    set: RefCell<WatchSet>,
    hit: Cell<Option<WatchpointHit>>,
    active: Cell<bool>,
}

impl WatchTap {
    fn new() -> Self {
        Self {
            set: RefCell::new(WatchSet::new()),
            hit: None.into(),
            active: false.into(),
        }
    }

    fn add(&self, id: WatchId, watchpoint: Watchpoint) {
        self.set.borrow_mut().add(id, watchpoint);
    }

    fn remove(&self, id: WatchId) {
        self.set.borrow_mut().remove(id);
    }

    fn set_active(&self, active: bool) {
        self.active.set(active && self.set.borrow().has_watches())
    }

    fn maybe_store_hit(&self, hit: WatchpointHit) {
        if self.hit.get().is_none() {
            self.hit.set(Some(hit))
        }
    }

    fn take_hit(&self) -> Option<WatchpointHit> {
        self.hit.take()
    }
}

impl MemoryHandler for WatchTap {
    fn read(&self, address: u16, ctx: MemoryContext<'_>) -> u8 {
        let set = self.set.borrow();
        let val = ctx.read(address);
        if self.active.get() && set.is_watched(address, AccessKind::Read) {
            let bank = ctx.mmu.bank_at(address);
            if let Some(id) = set.hit(address, bank, AccessKind::Read) {
                self.maybe_store_hit(WatchpointHit {
                    id,
                    addr: address,
                    bank,
                    old: val,
                    new: val,
                })
            }
        }
        val
    }

    fn write(&self, address: u16, value: u8, ctx: MemoryContext<'_>) {
        if self.active.get() && !matches!(ctx.source(), Source::Debugger) {
            let set = self.set.borrow();
            if set.is_watched(address, AccessKind::Write) {
                drop(set);

                let bank = ctx.mmu.bank_at(address);

                let debug = ctx.as_source(Source::Debugger);
                let old = debug.read(address);
                ctx.write(address, value);
                let new = debug.read(address);

                if let Some(id) = self.set.borrow().hit(address, bank, AccessKind::Write) {
                    self.maybe_store_hit(WatchpointHit {
                        id,
                        addr: address,
                        bank,
                        old,
                        new,
                    })
                }

                return;
            }
        }
        ctx.write(address, value);
    }
}

pub enum RunMode {
    Running,
    Paused,
    PauseRequested,
    Step { remaining: u32 },
    StepOver { ret: u16, sp: u16 },
    StepScanline { ly: u8 },
    StepFrame,
}

pub struct Debugger {
    watch_tap: Rc<WatchTap>,
    breakpoints: Breakpoints,
    mode: RunMode,
    attached: bool,
}

impl Debugger {
    pub fn new(start_attached: bool, initial_mode: RunMode) -> Self {
        Self {
            watch_tap: Rc::new(WatchTap::new()),
            breakpoints: Breakpoints::new(),
            mode: initial_mode,
            attached: start_attached,
        }
    }

    pub fn watch_tap(&self) -> Rc<WatchTap> {
        self.watch_tap.clone()
    }

    pub fn attach(&mut self) {
        self.attached = true;
        self.sync_watches();
    }

    pub fn detach(&mut self) {
        self.attached = false;
        self.mode = RunMode::Running;
        self.watch_tap.take_hit(); // stale hit would stop the next attach immediately
        self.sync_watches();
    }

    pub fn add_watch(&mut self, id: WatchId, watchpoint: Watchpoint) {
        self.watch_tap.add(id, watchpoint);
        self.sync_watches();
    }

    pub fn remove_watch(&mut self, id: WatchId) {
        self.watch_tap.remove(id);
        self.sync_watches();
    }

    fn sync_watches(&mut self) {
        self.watch_tap.set_active(self.attached);
    }

    pub fn attached(&self) -> bool {
        self.attached
    }

    pub fn should_stop(&mut self, cpu: &Cpu, mmu: &Mmu) -> Option<StopReason> {
        let reason = if let Some(hit) = self.watch_tap.take_hit() {
            Some(StopReason::Watchpoint(hit))
        } else {
            match &mut self.mode {
                RunMode::Running | RunMode::Paused => None,
                RunMode::Step { remaining } => {
                    *remaining -= 1;
                    (*remaining == 0).then_some(StopReason::Step)
                }
                RunMode::PauseRequested => Some(StopReason::UserPause),
                RunMode::StepOver { ret: _, sp: _ } => todo!("stpe-over not yet implemented"),
                RunMode::StepScanline { ly: _ } => todo!("step-scanline not yet implemented"),
                RunMode::StepFrame => todo!("step-frame not yet implemented"),
            }
        };
        reason
    }

    pub fn should_break(&self, addr: &BankAddr) -> bool {
        self.breakpoints.breaks(&addr)
    }

    pub fn paused(&self) -> bool {
        matches!(self.mode, RunMode::Paused)
    }

    pub fn handle(&mut self, cmd: DebugCommand, pc: u16) -> Option<DebugEvent> {
        let event = match cmd {
            DebugCommand::Pause => {
                self.mode = RunMode::PauseRequested;
                Some(DebugEvent::Paused {
                    reason: StopReason::UserPause,
                    pc,
                })
            }
            DebugCommand::Resume => {
                self.mode = RunMode::Running;
                None
            }
            DebugCommand::StepInstruction => {
                self.mode = RunMode::Step { remaining: 1 };
                None
            }
            DebugCommand::AddBreakpoint(bank_addr) => todo!(),
            DebugCommand::AddWatchpoint {
                id,
                bank_range,
                access,
            } => {
                let watchpoint = Watchpoint::new(bank_range.bank, bank_range.range, access);
                self.add_watch(id, watchpoint);
                None
            }
            DebugCommand::RemoveBreakpoint(bank_addr) => todo!(),
            DebugCommand::RemoveWatchpoint(watch_id) => todo!(),
            DebugCommand::ReadMemory { start, len } => todo!(),
            DebugCommand::WriteMemory { addr, value } => todo!(),
            DebugCommand::ReadRegisters => todo!(),
        };
        event
    }
}
