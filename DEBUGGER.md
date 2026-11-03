# Design note: the debugger

Status: **not implemented**. This records a design worked out in discussion so it
can be picked up later. Nothing here is in the tree yet, beyond the pieces noted
as already present.

It spans three crates — the engine in `backend`, the frontend in `gb-emu`, the
disassembly tables in `codegen` — so it sits at the root rather than in any one
of them.

## Why

The MBC1 SRAM bug is the argument. `Mbc1::has_ram` returned `self.ram.is_empty()`
instead of `!self.ram.is_empty()`, so every cart-RAM access took the "no chip"
path: reads answered `0xFF`, writes vanished. Tetris and Super Mario Land are
ROM-only and never noticed. Super Mario Land 2 puts its stack at `0xA8FF` and its
main work variables in cart RAM:

```
0205: 31 ff a8     LD SP, 0xA8FF        ; stack lives in cart RAM
0208: 3e 0a        LD A, 0x0A
020A: ea 00 00     LD (0x0000), A       ; enable cart RAM
...
022E: 3e 93        LD A, 0x93
0230: ea 7e a2     LD (0xA27E), A       ; state -> cart RAM
0268: fa 0e a2     LD A, (0xA20E)       ; reads back 0xFF
```

Writes vanished, so every `ret` returned to garbage and the game re-entered its
init forever. Blank screen, no crash, no log line.

Finding it took building a headless runner, dumping 6.8 million trace lines
through `#[cfg(feature = "debug")]`, and aggregating PC frequencies to notice
that the clear loop at `0x0215` had run 1,612,800 times — 16128 iterations times
100 restarts. A watchpoint on `0xA000..=0xBFFF` would have answered the same
question in seconds.

That is the whole case. The current tooling is a `println!` per instruction and
`tail`, and it does not scale to the PPU work that is next.

## Frontend

### egui, in-process

`eframe` + `egui` replaces minifb in `gb-emu/src/main.rs`. The framebuffer
becomes a `TextureHandle`; debug panels are egui widgets in the same process; the
debugger engine is a method call on `&mut System`. No channel, no serialisation,
no second process.

minifb gives a pixel buffer and nothing else, so there is nothing to lose. egui
gives docked panels, scrollable tables and per-frame texture upload for one
dependency and a couple of minutes of compile time.

Immediate mode is the real reason. There is no UI state to keep in step with
emulator state — each repaint reads `System` directly. Every retained-mode
toolkit would have you mirror registers, breakpoints and memory into widget
state and then keep the mirror honest.

### Alternatives, and why not

`gdbstub` with a custom `Arch` for SM83 gets breakpoints, watchpoints, stepping
and register/memory inspection with **no UI code at all** — the frontend is gdb,
lldb or VS Code. It would have caught the SML2 bug.

It buys nothing for the work actually queued: no VRAM tile viewer, no OAM table,
no PPU state, no palette view. It is complementary, not competing. Worth adding
later; not worth doing first.

A separate process with a web frontend over a socket is strictly more work for a
capability — remote attach — that nobody has asked for. Skip.

## The engine/frontend boundary

Do not pick a transport. Pick a message enum, and the transport becomes
swappable:

```rust
// backend/src/debug/protocol.rs
pub enum DebugCommand {
    Pause, Resume, StepInstruction, StepScanline, StepFrame,
    AddBreakpoint(BankAddr), RemoveBreakpoint(BankAddr),
    AddWatchpoint { range: (u16, u16), on_read: bool, on_write: bool },
    ReadMemory { start: u16, len: u16 },
    WriteMemory { addr: u16, value: u8 },
}

pub enum DebugEvent {
    Paused { reason: StopReason, pc: u16 },
    Memory { start: u16, bytes: Vec<u8> },
    Registers(RegisterSnapshot),
}

pub enum StopReason {
    Breakpoint(BankAddr),
    Watchpoint { addr: u16, old: u8, new: u8, pc: u16 },
    Step,
    UserPause,
}
```

- In-process: `fn handle(&mut self, cmd: DebugCommand) -> Vec<DebugEvent>`.
- Threaded: `std::sync::mpsc` both ways.
- Remote: `#[derive(Serialize, Deserialize)]` and length-prefix it.

Same enums throughout. The in-process path costs a `Vec` allocation per command
and is the one to build.

### `System` is `!Send`

`Rc<Mmu>`, `Rc<Ppu>`, `Rc<Clock>`, `Rc<InterruptController>`, `Rc<JoypadHandler>`
and `RefCell` throughout. That does not block threading it — you can run the core
on its own thread, provided you *construct* it inside that thread so the `Rc`s
never cross. Commands and events cross; those are plain data and `Send`.

Do not thread it until something demands it. The single-threaded path is simpler
and the emulator is nowhere near a frame budget.

## Watchpoints are nearly free

`Mmu` already has the hook. `peek` iterates handlers in insertion order and
returns on the first `Replace`; `poke` iterates and stops at the first non-`Pass`.
So a handler registered *before* the MBC sees every access and stays transparent
by answering `Pass`:

```rust
impl MemoryHandler for WatchTap {
    fn read(&self, _mmu: &Mmu, addr: u16) -> MemoryRead {
        if self.armed_read(addr) { self.hit.set(Some(Access::Read(addr))); }
        MemoryRead::Pass
    }
    fn write(&self, mmu: &Mmu, addr: u16, value: u8) -> MemoryWrite {
        if self.armed_write(addr) {
            self.hit.set(Some(Access::Write { addr, old: mmu.peek(addr), new: value }));
        }
        MemoryWrite::Pass
    }
}
```

`BlaargSpy` already establishes the pattern — under the `blaarg` feature it is
registered at `(0xA000, 0xBFFF)` *before* `mbc`, in `System::new`.

Register each tap over **its watched range only**, not the whole address space.
The range is known when the watchpoint is created, so the tap costs nothing at
any address nobody is watching.

One limit: a handler cannot halt mid-instruction. It sets a flag; `System::step`
checks it after the instruction returns. "Break *after* the write that touched
`0xA27E`" is the standard semantics anyway, and it is what you want — you get to
see the completed effect.

Capturing `old` costs a re-entrant `mmu.peek` from inside dispatch. That is safe
— `peek` holds a shared borrow of the handler table and nested shared borrows are
fine, which is exactly the case `Mmu::peek`'s comment already documents — but it
does mean `old` is only meaningful for addresses the tap can read back.

The tap must also be suspendable, because the debugger reads memory through the
same path it is watching. The hex viewer and the disassembly panel peek on every
repaint, and without a guard each one trips every watchpoint on screen at 60 Hz:

```rust
pub struct WatchTap { suspended: Cell<bool>, /* ... */ }
```

The UI sets it around its own peeks. This is the same re-entrancy the `old`
capture above relies on, seen from the other side.

## Attaching and detaching

The tap should not exist while nobody is debugging. Rather than a compile-time
feature or a permanent registration, `System` carries a request the run loop
honours at a step boundary:

```rust
pub fn request_debug_attach(&self, on: bool) { self.debug_request.set(Some(on)); }
```

The boundary is not a stylistic choice. `Mmu::peek` holds `handlers.borrow()`
across handler dispatch, and `add_handler`'s comment already records that
mutating the table from inside dispatch is the one thing that breaks. At a step
boundary no borrow is outstanding, so `handlers.borrow_mut()` is safe. Anywhere
else and it is a `RefCell` panic waiting for a game that writes to a watched
address.

### `remove_handler` needs identity, not a range

`add_handler` takes an `Rc<dyn MemoryHandler>` and pushes it. Removal by
`Rc::ptr_eq` works but drags in fat-pointer comparison semantics that changed in
Rust 1.76. Hand out a token instead:

```rust
pub struct HandlerId(u32);

pub fn add_handler(&self, range: (u16, u16), h: Rc<dyn MemoryHandler>) -> HandlerId;
pub fn remove_handler(&self, id: HandlerId);
```

Store the id alongside each `Rc` in the slot. Removal is then independent of the
range it was registered over, which matters because the caller detaching is not
always the caller that attached.

Cost: `remove_handler` over a wide range is a `retain` per address slot — 65536
of them for a whole-space handler. Irrelevant once per attach; do not put it
anywhere per-frame.

### Re-attaching will silently stop working

This is the part that bites. `add_handler` **pushes to the end** of the slot, and
dispatch order is registration order. The cartridge answers `Replace` for
`0xA000..=0xBFFF` reads and `Block` for writes, so a tap registered after it is
never reached — `peek` has already returned and `poke` has already stopped.

First attach works, because `System::new` registers debug handlers before `mbc`.
Detach and re-attach, and the tap lands behind the cartridge and goes quiet. No
error, no panic: watchpoints on cart RAM simply stop firing, which is exactly the
region the SML2 bug lived in.

So dynamic attach needs a front insertion, not just a removal:

```rust
pub fn add_handler_front(&self, range: (u16, u16), h: Rc<dyn MemoryHandler>) -> HandlerId;
```

Debug handlers always take the front. If more than debug handlers ever need
ordering, replace insertion order with an explicit priority — but not before.

### Detach must clear latent state

Pending hit flags, the paused flag, and any half-consumed step request. A stale
hit flag surviving a detach means the next attach stops immediately, at an
address with nothing wrong with it.

Attach latency of one frame is fine; honouring the request in `run_frame` rather
than per-`step` is enough. Breaking is the thing that must be per-step.

### What attach does *not* cover

The instruction-level overhead is not handler-based and needs its own flag: the
trace bitmap store, the ring buffer push, and the PC check below. Those live in
`System::step`, not in the handler table, so they are gated by a `bool` rather
than by registration.

## PC breakpoints

Not a handler, and not a field on `Cpu`. `System::step` already sequences
control flow and is the one place with both the PC and the bank in scope:

```rust
pub fn step(&mut self) -> usize {
    if self.debugger.breaks_at(self.cpu.pc(), self.current_rom_bank()) {
        self.paused = true;
        return 0;
    }
    let mut elapsed = self.cpu.execute_instruction();
    elapsed += self.cpu.handle_interrupts(&self.interrupt_controller);
    elapsed
}
```

Before the fetch, so the PC has not moved yet. `Cpu` needs only a `pc()`
accessor; no debugger state on it at all.

A `HashSet` lookup per instruction is measurable. Gate it on
`!breakpoints.is_empty()`, which predicts perfectly when nothing is armed.

### Why not a whole-space breakpoint handler

Tempting, since watchpoints are handlers and the handler table is already there:
register a tap over `0x0000..=0xFFFF` and treat a read of a breakpointed address
as an instruction fetch. It does not work, and the reasons are worth recording
because the first two *can* be fixed and the rest cannot.

`MemoryHandler::read(&self, mmu, address)` carries no origin and no reason, and
the two cases needing separation arrive identically:

```
cpu.fetch(Reg8::Z)        -> bus.read(pc)   -> CpuBus::read -> Mmu::peek(addr)
cpu.load(Reg8::Z, addr)   -> bus.read(addr) -> CpuBus::read -> Mmu::peek(addr)
```

**Operand bytes** — fixable. An operand fetch is a `fetch` like any other, so
`jp $0150` reads `0x0151` and `0x0152` too. Restricting the frontend to set
breakpoints only on instruction boundaries, which the trace bitmap already knows,
removes this.

**The debugger's own reads** — fixable, by the `suspended` guard above.

**Data reads of code addresses** — not fixable. An instruction boundary is still
an ordinary byte. Every GB game copies its OAM DMA routine into HRAM at boot,
because the CPU cannot touch ROM during a transfer; that copy reads real
instruction bytes as data. Bank-switch trampolines do the same, and a
self-checksumming game reads the entire ROM in one burst. The cheapest filter is
to compare the tap's address against the PC — which *is* the PC check, reached
via a whole-space handler and a dyn dispatch on every access in the system.

Two more that do not move. A handler sees a `u16`, and a breakpoint needs
`(bank, addr)`. And a whole-space registration taxes every access in the machine
— genuinely small, well under a percent — but it is the wrong shape: watchpoints
are a bus question and belong on the bus; breakpoints are a control-flow question
and belong where control flow is sequenced.

### The PPU is a task, and that constrains stepping

`Ppu::task` runs under `clock.spawn(Domain::Fixed, ...)`. Breaking inside it is
not a matter of setting a flag — see `backend/ASYNC_CLOCK.md`, which works
through why a suspended task cannot be snapshotted at an arbitrary await.

Consequence: step-instruction and step-frame are straightforward, step-scanline
is straightforward (break when `LY` changes), but "break inside the pixel
fetcher" is not available and should not be promised in the UI. Inspecting PPU
state at instruction boundaries covers the WX and palette work.

### If the CPU becomes a task too

`ASYNC_CLOCK.md` §"The CPU as a task" puts the CPU last, with
`codegen/DISPATCH.md`. If that lands, `System::step` survives — it stops being
"run one instruction" and becomes "advance the clock" — but a tick boundary is
no longer an instruction boundary. Mid-instruction the PC has already walked past
the opcode and operand bytes, so checking `pc()` on an arbitrary tick fires on
operand addresses: the false positive "Why not a whole-space breakpoint handler"
above exists to avoid.

Keep `step`'s meaning instead of its implementation. Have the CPU task bump a
counter at its loop top and define a step as "advance until the counter moves":

```rust
// in the cpu task
loop {
    state.instr.set(state.instr.get() + 1);
    let opcode = fetch(&bus).await;
    // ...
}

// System
pub fn step(&mut self) -> usize {
    let mark = self.cpu.instr_count();
    while self.cpu.instr_count() == mark { self.clock.tick(1); }
    // ...
}
```

The loop condition *is* the boundary flag, so there is no second piece of state
to keep honest. The breakpoint check keeps the home it has today.

Polling order makes this correct without extra care: the tick that finishes an
instruction resumes the task, runs it through the loop top, and re-suspends at
the next `fetch(&bus).await`. By the time `step` returns, the counter has moved
and the PC is the next instruction's address.

That counter earns its keep three times: step granularity here, sequence numbers
for the ring buffer below, and a resume point for `ASYNC_CLOCK.md`'s replay —
a better one than a cycle count, which collides across a speed switch.

Two things that design already guarantees and this leans on. `PC` is shared
state, not a local across an await, so `pc()` stays readable from outside. And
"the top of its loop *is* a natural boundary" — written there for savestate
resume descriptors, the same boundary the debugger needs.

Breaking also gets simpler, not harder. Today it means not calling
`execute_instruction`; with the inversion it means not ticking, which freezes
every device at once. That is what stopping a real machine does, and it removes
the question of what the PPU is doing while you are stopped.

## Disassembly

### What is missing today

`Instruction` carries `mnemonic: &'static str`, and that string is frozen at
codegen time with `d8`/`d16`/`r8` left in it. The trace prints `ld a, d8`, never
`ld a, $93`. `emit_mnemonic` in `codegen/src/generator.rs` does the join:

```rust
let args: Vec<String> = operands.iter().map(|o| o.to_mnemonic()).collect();
mnemonic.push_str(&args.join(", "));
```

The fix is not to template the string and substitute at runtime. It is to **move
the join to runtime** so operands can be resolved against real bytes.

### `size` is already parsed and thrown away

`codegen::types::Instruction` has `size: usize`, deserialised from
`instructions.yml`. `InstructionTemplate` never emits it. One template field.

Watch the convention: CB entries carry `size: 2`, counting the prefix byte, but
the fetch loop consumes `0xCB` before dispatch. Pick one meaning and assert it in
the generator rather than discovering the mismatch in the disassembly view.

### Emit operands, not a template

```rust
// backend/src/cpu/instructions.rs (generated)
pub enum Operand {
    R8(Reg8), R16(Reg16), Cond(Condition),
    Mem(&'static Operand), HighMem(&'static Operand),
    Imm8, Imm16, Rel8, Bit(u8), Vector(u8),
}

pub struct Instruction {
    pub cycles: Cycles,
    pub operator: &'static str,          // "ld"
    pub operands: &'static [Operand],
    pub size: u8,
}
```

`Operand::Mem(&Operand::R16(Reg16::HL))` is valid in const position — rvalue
static promotion. The runtime enum mirrors `codegen::types::Operand` one to one,
so the generator work is a third emitter trait beside `MnemonicEmitter` and
`RustEmitter`, in the same shape as the two that exist.

No `execute` field: under `codegen/DISPATCH.md` the executable half of the
generated file is an `async fn` with a `match`, not a table of function pointers.
That is the better shape here anyway — with nothing but data left, `Instruction`
is fully const and the lookup becomes `UNPREFIXED[op]` rather than a match
returning by value, which is what a panel disassembling a few hundred
instructions per repaint wants. Note the operands stay in the yaml's vocabulary:
`(hl)` and `d8` render as `(hl)` and `$93` regardless of the `W`/`Z` latches
execution lowers them to.

### Rendering

```rust
fn render(op: &Operand, bytes: &[u8], pc: u16, size: u8) -> String {
    match op {
        Operand::Imm8  => format!("${:02X}", bytes[1]),
        Operand::Imm16 => format!("${:04X}", u16::from_le_bytes([bytes[1], bytes[2]])),
        // resolve the target -- a raw offset is useless in a debugger
        Operand::Rel8  => format!("${:04X}", pc.wrapping_add(size as u16)
                                               .wrapping_add(bytes[1] as i8 as u16)),
        Operand::HighMem(inner) => match **inner {
            Operand::Imm8 => io_name(bytes[1])
                .map(|n| format!("({n})"))
                .unwrap_or_else(|| format!("($FF{:02X})", bytes[1])),
            _ => format!("({})", render(inner, bytes, pc, size)),
        },
        // ...
    }
}
```

Two things earn their keep disproportionately.

`Rel8` must render the **resolved target**, not the offset. Nobody reads `jr nz,
-8`; everybody reads `jr nz, $032D`.

`io_name` is roughly eighty lines of table and turns `ld ($FF40), a` into
`ld (rLCDC), a`. Best readability-per-line in the whole debugger, and it makes
the PPU register writes you are about to debug self-documenting.

### A linear sweep over the ROM will desync

This is the part that bites, and it is why "one pass over the ROM to build an
address/mnemonic table" does not work.

GB ROMs interleave code and data with no markers — jump tables, tile data, text,
and the header sitting at `0x0104`. A linear pass hits a data byte, decodes it as
a two- or three-byte opcode, and every instruction after it is garbage until the
stream accidentally resyncs. SML2's bank 0 is full of this.

**Trace bitmap — build this one.** Every PC actually fetched is, by definition,
an instruction boundary. One store per instruction:

```rust
self.visited[bank as usize][pc as usize] = true;
```

Ground truth, no heuristics, no desync. It also solves the problem a live
disassembly view otherwise cannot solve: scrolling *backwards* from the current
PC is formally impossible on a variable-length ISA, and trivial once boundaries
are known.

**Recursive descent** — seed from `0x0100`, the RST vectors `0x00`–`0x38` and the
interrupt vectors `0x40`/`0x48`/`0x50`/`0x58`/`0x60`; follow `jp`/`jr`/`call`;
stop at `ret`/`reti`/unconditional `jp`. Useful for inspecting a cold ROM before
running it. Dies on `jp (hl)` and jump tables, both common on the GB.

**Linear sweep** as a gap-filler only, and rendered visibly uncertain.

### Bank it

`(bank, address)` is the key, not `address`. Nothing currently exposes the
current bank. Add to `Mbc1`:

```rust
fn current_rom_bank(&self) -> u8 {
    let b1 = Mbc1::maybe_one_bank(self.rom_bank_number) & self.get_bank_num_mask() as u8;
    if self.rom_needs_extended_banking() {
        ((self.ram_bank_number & 0b11) << 5) | b1
    } else {
        b1
    }
}
```

That is the same expression as the `0x4000..=0x7FFF` read arm. Factor it out and
have the read arm call it, or the two will drift. Then display addresses
BGB-style: `01:4A3F`.

### Do not pre-disassemble

No eager full-ROM table. Disassemble the visible window on demand — a few hundred
instructions per repaint is free, and it stays correct when the bank changes
underneath you. Cache per `(bank, addr)` only if it ever shows up in a profile.

## Layout

Two window states: normal, and debug with a resizable right-hand column carrying
registers/flags, disassembly and breakpoints. This is where BGB, mGBA and SameBoy
all landed, and it maps onto egui's panel system directly.

```rust
egui::TopBottomPanel::top("menu").show(ctx, |ui| { /* File Savestate Debug ... */ });

if debug_mode {
    egui::SidePanel::right("debug").resizable(true).default_width(340.0).show(ctx, |ui| {
        // fixed-ish height, ~10 lines of content
        egui::TopBottomPanel::top("regs").show_inside(ui, |ui| { registers(ui, &snap); });
        // short list, user-resizable
        egui::TopBottomPanel::bottom("bp").resizable(true).show_inside(ui, |ui| { breakpoints(ui); });
        // absorbs whatever is left
        egui::CentralPanel::default().show_inside(ui, |ui| { disassembly(ui, &snap); });
    });
}

egui::CentralPanel::default().show(ctx, |ui| { game_texture(ui, &tex); });
```

The ordering matters. Disassembly goes in the inner `CentralPanel`, not a third
fixed split — registers are fixed-size content and breakpoints are a short list,
so an even three-way split squeezes the one panel that needs the room.

### Two things that look wrong if skipped

`TextureOptions::NEAREST` on the game texture. The default is linear and it turns
GB pixel art into soup.

Letterbox, never stretch. When the side panel takes width, preserve 160:144 and
centre. Integer-scale where it fits:

```rust
let scale = (avail.x / 160.0).min(avail.y / 144.0).floor().max(1.0);
```

Non-integer scaling with nearest filtering gives unevenly wide pixels, which
shimmers whenever anything moves horizontally.

### The column runs out of room

Registers, disassembly and breakpoints cover CPU debugging. They do not cover the
work actually in progress — the WX ≤ 7 quirks and CGB palettes. That needs:

- memory hex viewer, with a bank selector
- VRAM tile viewer and tilemap viewer, with the `9800`/`9C00` toggle
- OAM table, plus a sprite-bounds overlay on the game texture
- PPU state: LCDC and STAT decoded to named bits, `LY`, `SCX`/`SCY`, `WX`/`WY`,
  current mode, dot within line
- palettes: BGP/OBP0/OBP1, later BCPD/OCPD

Eight-odd panels do not fit one fixed column. A tab bar in the side panel is a
`ui.selectable_value` over an enum — do that first. `egui_dock` gives draggable,
splittable BGB-style tabs and is worth it once the count justifies it.

The PPU viewers want to be *wide*, not tall; a tilemap is 256×256. Letting panels
pop out into `egui::ViewportBuilder` child windows — real OS windows — means
debugging a scanline bug on a second monitor while the game runs on the first.

### Step controls belong in a toolbar

Pause, step-instruction, step-over, step-scanline, step-frame, run. These get hit
thousands of times a session; a menu dive per step is miserable. Toolbar row under
the menu bar, plus keyboard shortcuts. Step-scanline specifically, given the PPU
work.

### Two behaviours to get right early

A **follow-PC toggle** on the disassembly panel. Without it the view yanks back
to PC on every step and nothing around a breakpoint can be read.

**Repaint pacing.** While running, `ctx.request_repaint()` each frame. While
paused, do not — let egui idle at zero CPU while staying interactive. This means
the main loop stops being `while open { run_frame(); draw(); }` and becomes
`if !paused { run_frame(); }` inside the egui update callback.

### The window mode and the attach state are different things

The panel layout is a UI concern; attach is an engine concern. Keep them
separate — the ring buffer is worth running with the panels closed, and closing
the panels is not a reason to drop breakpoints.

Given attach, normal mode is free by construction rather than by argument: no
handler registered, no per-step flag set, nothing to predict. That is what the
attach machinery buys, and it is why this does not need a separate debug build.

## Relationship to `backend/ASYNC_CLOCK.md`

Two points of contact.

The savestate scheme there — rolling frame snapshot plus deterministic replay —
is the same machinery a debugger needs for **rewind**, which is the single
highest-value debugger feature after breakpoints. Stepping backwards from a
watchpoint hit beats stepping forwards to it from a guess. If rewind is wanted,
build it on that design rather than a second one.

The determinism requirements listed there — no `HashMap` iteration in the tick
path, no wall clock, no uninitialised reads, stable task poll order — are a
precondition for the debugger being trustworthy at all, not just for replay. A
breakpoint that fires at a different cycle on each run is worse than no
breakpoint.

## Order of work

1. Ring buffer of the last N `(bank, pc, opcode, registers)`, dumped on stop.
   No UI, no egui, no protocol. This alone replaces the 6.8-million-line trace
   that found the SML2 bug, and it is useful the day it lands.
2. `size` and structured `operands` through codegen; the `render` function and
   the `io_name` table. Still no UI — it makes the ring-buffer dump readable.
3. Swap minifb for `eframe`. Framebuffer as a texture, keep the existing keymap.
   Nearest filtering and letterboxing from the start.
4. `Mmu::add_handler_front` and `remove_handler` with a `HandlerId`, then the
   attach request honoured in `run_frame`. Front insertion is not optional — see
   "Re-attaching will silently stop working". Nothing uses it yet; landing it
   first keeps the watchpoint commit from carrying two unrelated changes.
5. Watchpoints as a `MemoryHandler` tap over the watched range, with the
   `suspended` guard, plus the stop flag in `System::step`. This is the piece
   that pays for the SML2 class of bug.
6. PC breakpoints in `System::step` and the disassembly panel, on the trace
   bitmap.
6. Tile, tilemap, OAM and palette viewers — these pay off directly against the
   unfinished PPU work.
7. Rewind, only alongside savestates, and only on the `ASYNC_CLOCK.md` design.
