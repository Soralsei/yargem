use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use crate::{
    cpu::cpu::Cpu,
    debug::{
        breakpoint::Breakpoints,
        protocol::{
            BankAddr, DebugCommand, DebugEvent, RegisterSnapshot, StopReason, WatchId,
            WatchpointHit,
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

    fn get(&self) -> Vec<(WatchId, Watchpoint)> {
        self.set.borrow().iter().map(Clone::clone).collect()
    }
}

impl MemoryHandler for WatchTap {
    fn read(&self, address: u16, ctx: MemoryContext<'_>) -> u8 {
        let val = ctx.read(address);
        if self.active.get() && matches!(ctx.source(), Source::Cpu) {
            let set = self.set.borrow();
            if set.is_watched(address, AccessKind::Read) {
                let bank = ctx.mmu.bank_at(address);
                if let Some(id) = set.hit(address, bank, AccessKind::Read) {
                    self.maybe_store_hit(WatchpointHit {
                        id,
                        access: AccessKind::Read,
                        addr: address,
                        bank,
                        old: val,
                        new: val,
                    })
                }
            }
        }
        val
    }

    fn write(&self, address: u16, value: u8, ctx: MemoryContext<'_>) {
        if self.active.get() && matches!(ctx.source(), Source::Cpu) {
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
                        access: AccessKind::Write,
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
    watch_counter: u32,
}

impl Debugger {
    pub fn new(start_attached: bool, initial_mode: RunMode) -> Self {
        Self {
            watch_tap: Rc::new(WatchTap::new()),
            breakpoints: Breakpoints::new(),
            mode: initial_mode,
            attached: start_attached,
            watch_counter: 0,
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

    fn add_watch(&mut self, watchpoint: Watchpoint) {
        self.watch_counter += 1;
        self.watch_tap.add(WatchId(self.watch_counter), watchpoint);
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

    pub fn should_stop(&mut self, addr: &BankAddr) -> Option<StopReason> {
        let reason = if let Some(hit) = self.watch_tap.take_hit() {
            Some(StopReason::Watchpoint(hit))
        } else if self.breakpoints.breaks(addr) {
            Some(StopReason::Breakpoint(*addr))
        } else {
            match &mut self.mode {
                RunMode::Running | RunMode::Paused => None,
                RunMode::Step { remaining } => {
                    *remaining -= 1;
                    (*remaining == 0).then_some(StopReason::Step)
                }
                RunMode::PauseRequested => Some(StopReason::UserPause),
                RunMode::StepOver { ret: _, sp: _ } => todo!("step-over not yet implemented"),
                RunMode::StepScanline { ly: _ } => todo!("step-scanline not yet implemented"),
                RunMode::StepFrame => todo!("step-frame not yet implemented"),
            }
        };
        if reason.is_some() {
            self.mode = RunMode::Paused;
        }
        reason
    }

    pub fn pause_idle(&mut self) -> Option<StopReason> {
        match self.mode {
            RunMode::PauseRequested => {
                self.mode = RunMode::Paused;
                return Some(StopReason::UserPause);
            }
            RunMode::Step { remaining } if remaining == 1 => {
                self.mode = RunMode::Paused;
                return Some(StopReason::Step);
            }
            _ => {}
        }
        None
    }

    pub fn paused(&self) -> bool {
        matches!(self.mode, RunMode::Paused)
    }

    pub fn handle(&mut self, cmd: DebugCommand, cpu: &Cpu, mmu: &Mmu) -> Option<DebugEvent> {
        match cmd {
            DebugCommand::ReadMemory { start, len } => {
                let bytes: Vec<u8> = (start..(start + len))
                    .map(|addr| mmu.read(addr, Source::Debugger))
                    .collect();
                return Some(DebugEvent::Memory { start, bytes });
            }
            DebugCommand::WriteMemory { addr, value } => mmu.write(addr, value, Source::Debugger),

            DebugCommand::ReadRegisters => {
                return Some(DebugEvent::Registers(RegisterSnapshot(
                    *cpu.registers.borrow(),
                )))
            }
            DebugCommand::Attach => {
                self.attach();
                return Some(DebugEvent::Attached);
            }
            DebugCommand::Detach => {
                self.detach();
                return Some(DebugEvent::Detached);
            }
            DebugCommand::Pause => {
                if !self.paused() {
                    self.mode = RunMode::PauseRequested;
                }
            }
            DebugCommand::Resume => {
                self.mode = RunMode::Running;
            }
            DebugCommand::StepInstruction => {
                self.mode = RunMode::Step { remaining: 1 };
            }
            DebugCommand::AddBreakpoint(_bank_addr) => {
                self.breakpoints.add(_bank_addr);
                return Some(DebugEvent::Breakpoints(self.breakpoints.get()));
            }

            DebugCommand::AddWatchpoint { bank_range, access } => {
                let watchpoint = Watchpoint::new(bank_range.bank, bank_range.range, access);
                self.add_watch(watchpoint);
                return Some(DebugEvent::Watchpoints(self.watch_tap.get()));
            }
            DebugCommand::RemoveBreakpoint(bank_addr) => {
                self.breakpoints.remove(bank_addr);
                return Some(DebugEvent::Breakpoints(self.breakpoints.get()));
            }
            DebugCommand::RemoveWatchpoint(id) => {
                self.remove_watch(id);
                return Some(DebugEvent::Watchpoints(self.watch_tap.get()));
            }
        };
        None
    }
}

#[cfg(test)]
mod test {
    use std::{fs, io};

    use crate::{
        debug::{
            protocol::{
                BankAddr, BankRange, DebugCommand, DebugEvent, StopReason, WatchId, WatchpointHit,
            },
            watchpoint::AccessKind,
        },
        system::{RunResult, Settings, System},
    };

    struct Harness {
        system: System,
    }

    impl Harness {
        fn new(rom_path: &str) -> Result<Self, io::Error> {
            let settings = Settings {
                debugger_attached: true,
                ..Default::default()
            };
            let rom = fs::read(rom_path)?;
            Ok(Self {
                system: System::new(None, rom, settings),
            })
        }

        fn debug(&mut self, cmd: DebugCommand) -> Option<DebugEvent> {
            self.system.debug(cmd)
        }

        fn run_until_end(&mut self) -> RunResult {
            for _ in 0..600 {
                match self.system.run_frame() {
                    RunResult::Frame => {}
                    run => {
                        eprintln!("{:?}", self.system.cpu.registers.borrow());
                        return run;
                    }
                };
            }
            panic!("ROM neither stopped nor idled within 600 frames");
        }
    }

    fn check_hit_watchpoint(run: &RunResult, expected: &WatchpointHit) {
        match run {
            RunResult::Stopped(stop_reason) => {
                let StopReason::Watchpoint(hit) = stop_reason else {
                    assert!(
                        false,
                        "emulator stopped for reason other than watchpoint : {:?}",
                        stop_reason
                    );
                    return;
                };
                assert_eq!(hit.addr, expected.addr);
                assert_eq!(hit.id, expected.id);
                assert_eq!(hit.old, expected.old);
                assert_eq!(hit.new, expected.new);
            }
            other => panic!("unexpected run result : {:?}", other),
        }
    }

    #[test]
    fn watchpoint_read_normal() {
        let mut h = Harness::new("../tests/watchpoint-test.gb").expect("failed to open rom");
        let _ = h.debug(DebugCommand::AddWatchpoint {
            bank_range: BankRange {
                bank: None,
                range: 0xC001..=0xC001,
            },
            access: AccessKind::Read,
        });

        let run = h.run_until_end();
        let expected = WatchpointHit {
            id: WatchId(1),
            access: AccessKind::Read,
            addr: 0xC001,
            bank: None,
            old: 123,
            new: 123,
        };
        check_hit_watchpoint(&run, &expected);
    }

    #[test]
    fn watchpoint_write_normal() {
        let mut h = Harness::new("../tests/watchpoint-test.gb").expect("failed to open rom");
        let _ = h.debug(DebugCommand::AddWatchpoint {
            bank_range: BankRange {
                bank: None,
                range: 0xC002..=0xC002,
            },
            access: AccessKind::Write,
        });

        let run = h.run_until_end();
        let expected = WatchpointHit {
            id: WatchId(1),
            access: AccessKind::Write,
            addr: 0xC002,
            bank: None,
            old: 0,
            new: 234,
        };

        check_hit_watchpoint(&run, &expected);
    }

    #[test]
    fn watchpoint_read_pop() {
        let mut h = Harness::new("../tests/watchpoint-test.gb").expect("failed to read ROM");
        let _ = h.debug(DebugCommand::AddWatchpoint {
            bank_range: BankRange {
                bank: None,
                range: 0xC003..=0xC003,
            },
            access: AccessKind::Read,
        });

        let run = h.run_until_end();
        let expected = WatchpointHit {
            id: WatchId(1),
            access: AccessKind::Read,
            addr: 0xC003,
            bank: None,
            old: 0x0A,
            new: 0x0A,
        };

        check_hit_watchpoint(&run, &expected);
    }

    #[test]
    fn watchpoint_write_push() {
        let mut h = Harness::new("../tests/watchpoint-test.gb").expect("failed to read ROM");
        let _ = h.debug(DebugCommand::AddWatchpoint {
            bank_range: BankRange {
                bank: None,
                range: 0xC004..=0xC004,
            },
            access: AccessKind::Write,
        });

        let run = h.run_until_end();
        let expected = WatchpointHit {
            id: WatchId(1),
            access: AccessKind::Write,
            addr: 0xC004,
            bank: None,
            old: 0x00,
            new: 0xB0,
        };

        check_hit_watchpoint(&run, &expected);
    }

    #[test]
    fn breakpoint_break_n() {
        let mut h = Harness::new("../tests/breakpoint-test.gb").expect("failed to open ROM");

        let addr = BankAddr {
            bank: None,
            address: 0x212,
        };
        let _ = h.debug(DebugCommand::AddBreakpoint(addr));

        let mut break_count = 0;
        for _ in 0..20 {
            match h.run_until_end() {
                RunResult::Stopped(StopReason::Breakpoint(bank_addr)) => {
                    assert_eq!(bank_addr, addr);
                    break_count += 1;
                    h.debug(DebugCommand::Resume);
                }
                RunResult::Idle => break,
                other => panic!("unexpected run result : {:?}", other),
            }
        }

        assert_eq!(break_count, 10);
    }
}
