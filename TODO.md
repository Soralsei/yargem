# TODO

## Debugger (engine side)

- [X] **Breakpoint/watchpoint listing pushed by the engine**: the engine owns the sets, so the GUI shouldn't keep its own copy.
  - [X] `Debugger` allocates `WatchId`s from a counter; drop `id` from `AddWatchpoint`.
  - [X] Every Add/Remove answers with the full current list: `DebugEvent::Breakpoints(Vec<BankAddr>)` / `DebugEvent::Watchpoints(Vec<(WatchId, Watchpoint)>)`. Whole list, not deltas.
  - [X] The GUI's `DebugState` only caches these events; the debugger window takes `&DebugState` and only sends commands.
  - [X] No `List*` command until something can attach to an already-running session.
  - [X] Needs `iter()` on `Breakpoints` and `WatchSet`, `Clone` + `bank()` on `Watchpoint`, and test updates (Add* no longer returns `None`; take the id from the event).
- [X] **`Resumed` event**: the protocol has no event for leaving the paused state, so the GUI sets `paused = false` itself on Resume as a stopgap. Add `DebugEvent::Resumed`, sent on Resume and when a long step mode (step-frame) starts, so `DebugState` is only written from events. Attach/Detach (also tracked by the GUI itself) can follow the same pattern later.
- [X] **ReadMemory / WriteMemory**: `System::debug` returns `None` for both. Read through `Source::Debugger` so watchpoints don't fire.
- [X] **Step modes**: `StepOver`, `StepScanline`, `StepFrame` are `todo!()` in `Debugger::should_stop`, and there are no commands for them yet.
- [X] **Step while parked**: `StepInstruction` with the CPU in HALT (IE=0), STOP or wedged never reaches a boundary. Decide whether `stop_parked` should also stop `Step`.

## Frontend (GUI)

- [ ] **Debugger window**: `debugger_panel` is still an empty right panel. Move it to an `egui::Window` struct holding only input buffers (breakpoint address, watch range, access kind, parse error). It shows registers and breakpoint/watchpoint lists from `DebugState`.
- [X] **Run bar**: show the stop reason (breakpoint address, watchpoint hit details), not just the pc (`ui/debug.rs` TODO). Add keyboard shortcuts for pause/step/resume.
- **Memory viewer**: `ui/memory.rs` and `memory_window` are stubs. Hex view with `ScrollArea::show_rows`, request only the visible range with `ReadMemory`, cache the reply, add a go-to address and later a bank selector.
- [ ] **Input manager**: `input/input_map.rs` and `input/manager.rs` are empty. Configurable keymap; send `Button` commands only on change (currently 8 commands per repaint).
- **Palette in settings**: `DMG` is hardcoded in `ui/screen.rs`. Goes in the persisted `Config`.
- [ ] **Screen with no ROM**: `Screen::ui` re-uploads a black texture on every repaint when no engine is loaded. Upload once (on Close ROM).
- [ ] **ROM load errors**: `fs::read` failure is swallowed (`Err(e) => {}`). Show it to the user.
- [ ] **Boot ROM config**: the minifb version took a boot ROM path as `args[2]`; the GUI always passes `None`.
  - [ ] File picker per slot: DMG and CGB. Reject at pick time unless the size is 0x100 (DMG) or 0x900 (CGB); `Mbc` indexes `boot_rom[address]` unchecked, so a short file panics on the engine thread.
  - [ ] Store the paths, not the bytes, and re-read on each ROM load. Missing file: show a notice, fall back to post-boot.
  - [ ] Engine side: `System::new` takes both slots (`BootRoms { dmg, cgb }`) and picks by the model it already decides (`is_cgb_only() || is_cgb_override`), so the frontend doesn't duplicate that rule.

- [ ] **Persisted config**: one `#[derive(Serialize, Deserialize)] Config` in eframe storage (`eframe::get_value` in `App::new`, `eframe::set_value` in `App::save`, key `eframe::APP_KEY`). Holds the boot ROM paths now; palette, keymap and last ROM directory later. Needs `features = ["persistence"]` on eframe.

## Debugger roadmap (from `DEBUGGER.md`, "Order of work")

- [ ] Ring buffer of the last N `(bank, pc, opcode, registers)`, dumped on stop.
- [ ] Disassembly: emit `size` and structured operands from codegen, render at runtime, disassembly panel with follow-PC toggle.
- [ ] Tile, tilemap, OAM and palette viewers; PPU state panel (LCDC/STAT decoded, LY, scroll, window).
- [ ] Rewind, only alongside savestates.
- [ ] `DEBUGGER.md` itself is stale: it says "not implemented" and describes per-range tap registration and a `suspended` flag, while the code uses a whole-space `WatchTap` with an `active` flag and `Source`-based filtering. Update or trim it.

## Emulator core

- [ ] MBC2, MBC3, MBC5, MBC6, MBC7 (`memory/mbc.rs`), plus unknown cartridge types currently `unimplemented!()`.
  - [X] MBC2
  - [ ] MBC3
  - [ ] MBC5
  - [ ] MBC6
  - [ ] MBC7

- [ ] CGB IO registers beyond KEY1 (`system.rs`).
- [ ] CGB PPU color handling and banking
- [ ] APU VIN mixing (`audio/apu.rs`).

### HDMA and same-dot wakes (in order)

The executor change is checked against the baseline, and HDMA needs the executor change to be exact in double speed.

- [ ] **Determinism baseline**: headless runner over `gb-test-roms/` (tetris, both SMLs, the blargg ROMs), about 1000 frames each, one framebuffer hash per frame saved to a file. A change that claims to keep timing must reproduce every hash, and the first frame that differs shows where to look.
  - [ ] Same runner as a `cargo test` for blargg. The ROMs use two runtimes, and each ROM reports through exactly one of them (checked against the binaries):
    - **$A000**: dmg_sound, cgb_sound, mem_timing-2, oam_bug, halt_bug, interrupt_time. Reads $80 while running, then the result code (0 = passed), with text from $A004. This is what `debug/blaarg_spy.rs` decodes.
    - **Serial only**: cpu_instrs (combined and individual), instr_timing, mem_timing (v1). Text ending in `Passed` or `Failed`.
    - Watch both and stop on whichever reports first. Serial needs a sink that captures output: `System::new` hardcodes `LogSink`, so the runner needs a way to pass its own. cgb_sound probably needs CGB mode (`is_cgb_override`).
- [ ] **Executor: same-dot wakes** (`clock.rs`): a task woken by a task spawned after it is only polled on the next dot. A two-way handshake (HDMA park, then release) therefore always loses a dot on one leg, which is one M-cycle late in double speed.
  - [ ] `Executor::poll`: a second pass over the tasks still `notified` after the first, in spawn order. Only one extra pass, so the work per dot stays bounded (a wake raised during pass 2 still waits a dot). Rewrite the doc comment above `poll`.
  - [ ] Keep today's timing: `Timeline::after_write` = `after` plus one grid step (an M-cycle on `Cpu`, a dot on `Fixed`), only if the event wasn't already true. Use it at the four sites woken by CPU register writes: `oam.rs:60`, `serial.rs:86`, `timer.rs:109`, `ppu.rs:172` (LCD on). LCD off (`ppu.rs:177`) needs nothing. The "only if it waited" condition stops an OAM DMA restart, which loops back with the request already set, from gaining a cycle.
  - [ ] Unit harnesses that write between ticks (e.g. `oam.rs` `Harness::start`) will be one step late. Make them write from a task spawned last, like the real CPU.
  - [ ] Every baseline hash must be identical.
- [ ] **CPU task arguments**: bundle `t`, `t_fixed`, `clock`, `fixed_clock` into a `CpuTime` with a `FromTask` impl in `clock.rs` (`FromTask::build` takes `&Rc<CpuClock>` / `&Rc<FixedClock>`). Unpack it at the top of `Cpu::task` so `CpuTask`'s fields stay the same. This removes the `cpu_clock` / `fixed_clock` lets in `system.rs`.
- [ ] **CGB VRAM DMA, FF51–FF55** (`memory/hdma.rs`). Reference: SameBoy `Core/memory.c` (`GB_hdma_run`, the `GB_IO_HDMA5` read/write cases), `Core/sm83_cpu.c` (HDMA runs right after the opcode fetch), `Core/display.c` (HBlank trigger).
  - [ ] Registers: FF51/52 source `& 0xFFF0`, FF53/54 dest `0x8000 | (addr & 0x1FF0)`, all four read 0xFF. They are live counters advanced by the copy, and FF55 writes don't reload them. FF55 read: `(active ? 0 : 0x80) | remaining`, where `remaining` = blocks − 1 and wraps to 0x7F when done (0xFF for free).
  - [ ] FF55 write: set `remaining` first. Then, if an HBlank transfer is active and bit 7 = 0, cancel it (no GDMA starts). Otherwise bit 7 picks GDMA (0) or HBlank (1). Arming HBlank while STAT is in mode 0 (LCD off included) copies one block right away. On cancel, Pan Docs says FF55 keeps the remaining count, but SameBoy overwrites it with the written value. Follow SameBoy.
  - [ ] Own task on `Timeline<Fixed>`: 2 dots of startup, then 2 dots per byte (34 dots per HBlank block, `2 + 32n` for a GDMA). The CPU resumes on its M-cycle grid, which gives SameBoy's 36 (single) / 34 (double) dot totals without checking the speed.
  - [ ] CPU stop: `Hdma::hold()` / `hlda()`, checked in `CpuTask::execute` right after the opcode fetch (not after the 0xCB second fetch). The CPU waits on a listener taken from `Hdma` inside `Cpu::task` until released. `hlda()` raises the listener the HDMA task already waits on. `Cpu::task` takes `Rc<Hdma>`.
  - [ ] PPU: entering HBlank with LY < 144 wakes the HDMA task (`set_mode`, `ppu.rs:239`).
  - [ ] Map FF51–55 to `Hdma` in CGB mode in `system.rs` and spawn its task.
  - [ ] Edge cases, can wait: no block for an HBlank while the CPU is halted (needs a read of `Cpu::halted()`); waking from HALT in mode 0, when HALT was entered outside mode 0, copies one block; turning the LCD off with a transfer armed copies one block.
  - [ ] rgbasm tests, with the expected values taken from running the same ROM in SameBoy: FF55 reads after a GDMA, after an HBlank transfer completes, and after a cancel; GDMA length measured with DIV in both speeds; HBlank blocks per frame, by watching FF55 count down across LY.
