# TODO

## Debugger (engine side)

- **Breakpoint/watchpoint listing pushed by the engine**: the engine owns the sets, so the GUI shouldn't keep its own copy.
  - `Debugger` allocates `WatchId`s from a counter; drop `id` from `AddWatchpoint`.
  - Every Add/Remove answers with the full current list: `DebugEvent::Breakpoints(Vec<BankAddr>)` / `DebugEvent::Watchpoints(Vec<(WatchId, Watchpoint)>)`. Whole list, not deltas.
  - The GUI's `DebugState` only caches these events; the debugger window takes `&DebugState` and only sends commands.
  - No `List*` command until something can attach to an already-running session.
  - Needs `iter()` on `Breakpoints` and `WatchSet`, `Clone` + `bank()` on `Watchpoint`, and test updates (Add* no longer returns `None`; take the id from the event).
- **`Resumed` event**: the protocol has no event for leaving the paused state, so the GUI sets `paused = false` itself on Resume as a stopgap. Add `DebugEvent::Resumed`, sent on Resume and when a long step mode (step-frame) starts, so `DebugState` is only written from events. Attach/Detach (also tracked by the GUI itself) can follow the same pattern later.
- **ReadMemory / WriteMemory**: `System::debug` returns `None` for both. Read through `Source::Debugger` so watchpoints don't fire.
- **Step modes**: `StepOver`, `StepScanline`, `StepFrame` are `todo!()` in `Debugger::should_stop`, and there are no commands for them yet.
- **Step while parked**: `StepInstruction` with the CPU in HALT (IE=0), STOP or wedged never reaches a boundary. Decide whether `stop_parked` should also stop `Step`.
- **`Debugger::handle` swallows unknown commands** (`_ => {}`). Make the match exhaustive once `System::debug` and `Debugger::handle` agree on who owns which command.

## Frontend (GUI)

- **Debugger window**: `debugger_panel` is still an empty right panel. Move it to an `egui::Window` struct holding only input buffers (breakpoint address, watch range, access kind, parse error). It shows registers and breakpoint/watchpoint lists from `DebugState`.
- **Run bar**: show the stop reason (breakpoint address, watchpoint hit details), not just the pc (`ui/debug.rs` TODO). Add keyboard shortcuts for pause/step/resume.
- **Memory viewer**: `ui/memory.rs` and `memory_window` are stubs. Hex view with `ScrollArea::show_rows`, request only the visible range with `ReadMemory`, cache the reply, add a go-to address and later a bank selector.
- **Input manager**: `input/input_map.rs` and `input/manager.rs` are empty. Configurable keymap; send `Button` commands only on change (currently 8 commands per repaint).
- **Palette in settings**: `DMG` is hardcoded in `ui/screen.rs`. Goes in the persisted `Config`.
- **Screen with no ROM**: `Screen::ui` re-uploads a black texture on every repaint when no engine is loaded. Upload once (on Close ROM).
- **ROM load errors**: `fs::read` failure is swallowed (`Err(e) => {}`). Show it to the user.
- **Boot ROM config**: the minifb version took a boot ROM path as `args[2]`; the GUI always passes `None`.
  - File picker per slot: DMG and CGB. Reject at pick time unless the size is 0x100 (DMG) or 0x900 (CGB); `Mbc` indexes `boot_rom[address]` unchecked, so a short file panics on the engine thread.
  - Store the paths, not the bytes, and re-read on each ROM load. Missing file: show a notice, fall back to post-boot.
  - Engine side: `System::new` takes both slots (`BootRoms { dmg, cgb }`) and picks by the model it already decides (`is_cgb_only() || is_cgb_override`), so the frontend doesn't duplicate that rule.
- **Persisted config**: one `#[derive(Serialize, Deserialize)] Config` in eframe storage (`eframe::get_value` in `App::new`, `eframe::set_value` in `App::save`, key `eframe::APP_KEY`). Holds the boot ROM paths now; palette, keymap and last ROM directory later. Needs `features = ["persistence"]` on eframe.
## Debugger roadmap (from `DEBUGGER.md`, "Order of work")

- Ring buffer of the last N `(bank, pc, opcode, registers)`, dumped on stop.
- Disassembly: emit `size` and structured operands from codegen, render at runtime, disassembly panel with follow-PC toggle.
- Tile, tilemap, OAM and palette viewers; PPU state panel (LCDC/STAT decoded, LY, scroll, window).
- Rewind, only alongside savestates.
- `DEBUGGER.md` itself is stale: it says "not implemented" and describes per-range tap registration and a `suspended` flag, while the code uses a whole-space `WatchTap` with an `active` flag and `Source`-based filtering. Update or trim it.

## Emulator core

- MBC2, MBC3, MBC5, MBC6, MBC7 (`memory/mbc.rs`), plus unknown cartridge types currently `unimplemented!()`.
- CGB IO registers beyond KEY1 (`system.rs`).
- APU VIN mixing (`audio/apu.rs`).
