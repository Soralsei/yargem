use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use crate::{
    cpu::cpu::Cpu,
    debug::{
        breakpoint::Breakpoints,
        protocol::{BankAddr, DebugCommand, DebugEvent, StopReason, WatchId, WatchpointHit},
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
        let val = ctx.read(address);
        if self.active.get() && matches!(ctx.source(), Source::Cpu) {
            let set = self.set.borrow();
            if set.is_watched(address, AccessKind::Read) {
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

    fn add_watch(&mut self, id: WatchId, watchpoint: Watchpoint) {
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
            DebugCommand::AddBreakpoint(_bank_addr) => {
                self.breakpoints.add(_bank_addr);
                None
            }
            DebugCommand::AddWatchpoint {
                id,
                bank_range,
                access,
            } => {
                let watchpoint = Watchpoint::new(bank_range.bank, bank_range.range, access);
                self.add_watch(id, watchpoint);
                None
            }
            DebugCommand::RemoveBreakpoint(_bank_addr) => todo!(),
            DebugCommand::RemoveWatchpoint(id) => {
                self.remove_watch(id);
                None
            }
            DebugCommand::ReadMemory { start: _, len: _ } => todo!(),
            DebugCommand::WriteMemory { addr: _, value: _ } => todo!(),
            DebugCommand::ReadRegisters => todo!(),
        };
        event
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

    fn check_run_result(run: &RunResult, expected: &WatchpointHit) {
        match run {
            RunResult::Idle => panic!("watchpoint not hit, returned Idle"),
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
            RunResult::Frame => unreachable!(),
        }
    }

    #[test]
    fn watchpoint_read_normal() {
        let mut h = Harness::new("../tests/watchpoint-test.gb").expect("failed to open rom");
        let evt = h.debug(DebugCommand::AddWatchpoint {
            id: WatchId(0u32),
            bank_range: BankRange {
                bank: None,
                range: 0xC001..=0xC001,
            },
            access: AccessKind::Read,
        });

        assert!(evt.is_none(), "debug event is not none");

        let run = h.run_until_end();
        let expected = WatchpointHit {
            id: WatchId(0u32),
            addr: 0xC001,
            bank: None,
            old: 123,
            new: 123,
        };
        check_run_result(&run, &expected);
    }

    #[test]
    fn watchpoint_write_normal() {
        let mut h = Harness::new("../tests/watchpoint-test.gb").expect("failed to open rom");
        let evt = h.debug(DebugCommand::AddWatchpoint {
            id: WatchId(0u32),
            bank_range: BankRange {
                bank: None,
                range: 0xC002..=0xC002,
            },
            access: AccessKind::Write,
        });

        assert!(evt.is_none(), "debug event is not none");

        let run = h.run_until_end();
        let expected = WatchpointHit {
            id: WatchId(0u32),
            addr: 0xC002,
            bank: None,
            old: 0,
            new: 234,
        };

        check_run_result(&run, &expected);
    }

    #[test]
    fn watchpoint_read_pop() {
        let mut h = Harness::new("../tests/watchpoint-test.gb").expect("failed to read ROM");
        let evt = h.debug(DebugCommand::AddWatchpoint {
            id: WatchId(0u32),
            bank_range: BankRange {
                bank: None,
                range: 0xC003..=0xC003,
            },
            access: AccessKind::Read,
        });

        assert!(evt.is_none(), "debug event is not none");

        let run = h.run_until_end();
        let expected = WatchpointHit {
            id: WatchId(0),
            addr: 0xC003,
            bank: None,
            old: 0x0A,
            new: 0x0A,
        };

        check_run_result(&run, &expected);
    }

    #[test]
    fn breakpoint_break_n() {
        let mut h = Harness::new("../tests/breakpoint-test.gb").expect("failed to open ROM");

        let addr = BankAddr {
            bank: None,
            address: 0x212,
        };
        let evt = h.debug(DebugCommand::AddBreakpoint(addr));

        assert!(evt.is_none(), "debug event is not none");

        let mut break_count = 0;
        for _ in 0..10 {
            if let RunResult::Stopped(reason) = h.run_until_end() {
                match reason {
                    StopReason::Breakpoint(bank_addr) => {
                        assert_eq!(bank_addr, addr);
                        break_count += 1;
                    }
                    _ => continue,
                }
            } else {
                continue;
            }
        }

        assert_eq!(break_count, 10);
    }
}
