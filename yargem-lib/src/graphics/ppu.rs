#![allow(dead_code)]
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::convert::Infallible;
use std::pin::pin;
use std::rc::Rc;

use crate::clock::{signal, Fixed, Listener, Raiser, Timeline};
use crate::cpu::interrupt::InterruptRequest;
use crate::graphics::attributes::{BgAttributes, CGBBank, ObjectAttribute, TilePriority};
use crate::graphics::fifo::{BgFIFO, ObjFIFO, Pixel};
use crate::graphics::registers::{ObjSize, PpuMode, PpuRegisters, TileAddressing, TileArea};
use crate::memory::bus::{Bus, BusOwner, BusType};
use crate::memory::mmu::{MemoryContext, MemoryHandler};
use crate::system::PowerOn;
use crate::util::future::Race;
use crate::{is_bit_set, SCREEN_H, SCREEN_W};

const OAM_SCAN_DOTS: u16 = 2;

struct ScannedObject {
    attributes: ObjectAttribute,
    // Index into OAM
    index: usize,
}

impl ScannedObject {
    pub fn into_request(&self, ly: u8, size: ObjSize) -> ObjRequest {
        let flags = self.attributes.flags();
        let height = size.as_height();

        // Row within the whole object, 0..height. Y flip spans the full
        // height, so it must be applied before splitting an 8x16 into halves.
        let mut row = (ly + 16) - self.attributes.y();
        if flags.y_flip() {
            row = height - 1 - row;
        }

        let tile = match size {
            ObjSize::Size8x8 => self.attributes.tile_index(),
            // bit 0 is ignored in 8x16; the row picks the half
            ObjSize::Size8x16 => (self.attributes.tile_index() & 0xFE) | (row >= 8) as u8,
        };

        ObjRequest {
            tile,
            fine_y: row & 7,
            flip_x: flags.x_flip(),
            x: self.attributes.x(),
            palette: flags.dmg_palette() as u8,
            priority: flags.priority(),
            oam_index: self.index as u8,
            bank: flags.bank(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ObjRequest {
    /// Tile index, with the 8x16 half already selected.
    pub tile: u8,
    /// Row inside that tile, 0-7, Y flip already applied.
    pub fine_y: u8,
    pub flip_x: bool,
    /// OAM X (+8 biased). Below 8, the leftmost `8 - x` pixels are clipped.
    pub x: u8,
    /// DMG: 0/1 selecting OBP0/OBP1. CGB: 0-7.
    pub palette: u8,
    pub priority: TilePriority,
    /// CGB priority, and the DMG tie-break on equal X.
    pub oam_index: u8,
    pub bank: CGBBank,
}

#[derive(Default)]
struct LineState {
    pub ly: u8,
    pub bg_fifo: RefCell<BgFIFO>,
    pub obj_fifo: RefCell<ObjFIFO>,
    pub obj_request: Cell<Option<ObjRequest>>, // shifter -> fetcher
    pub window: Cell<bool>,                    // shifter -> fetcher, restart
}

impl LineState {
    pub fn new(ly: u8) -> Self {
        Self {
            ly,
            ..Default::default()
        }
    }
}

pub type Frame = [u8; SCREEN_W * SCREEN_H];

pub struct Ppu {
    registers: RefCell<PpuRegisters>,
    vram: [[Cell<u8>; 0x2000]; 2],

    interrupt_request: InterruptRequest,

    /// OAM lives on the PPU die, not in system RAM. `Cell<u8>` is layout and
    /// cost-identical to `u8`, so OAM DMA can write through the bus handler
    /// while the PPU is mid-scan.
    oam: [Cell<u8>; 160],

    front: RefCell<Box<Frame>>,
    back: RefCell<Box<Frame>>,
    frame_ready: Cell<bool>,

    window_y_reached: Cell<bool>,
    window_ly: Cell<u8>,

    /// The PPU denies the CPU the buses it is using. Arbitration lives here
    /// rather than in the memory handler so that OAM DMA and the PPU's own
    /// fetches, which go through `peek`/`poke`, stay exempt.
    bus_controller: Rc<Bus>,

    is_cgb: bool,

    /// Raised on every LCDC write, so the task sees the LCD switch on or off.
    raiser: Raiser,
    /// Taken by the task when it starts: it is the only consumer.
    listener: Cell<Option<Listener>>,
}

impl Ppu {
    pub fn new(
        interrupt_request: InterruptRequest,
        bus: Rc<Bus>,
        is_cgb: bool,
        power_on: PowerOn,
    ) -> Self {
        let (raiser, listener) = signal();
        Self {
            registers: RefCell::new(PpuRegisters::new(power_on)),
            interrupt_request,
            vram: core::array::from_fn(|_| [const { Cell::new(0) }; 0x2000]),
            oam: [const { Cell::new(0) }; 160],
            front: RefCell::new(Box::new([0u8; SCREEN_W * SCREEN_H])),
            back: RefCell::new(Box::new([0u8; SCREEN_W * SCREEN_H])),
            frame_ready: Cell::new(false),
            window_y_reached: Cell::new(false),
            window_ly: Cell::new(0),
            bus_controller: bus,
            is_cgb,
            raiser,
            listener: Cell::new(Some(listener)),
        }
    }

    pub async fn task(this: Rc<Self>, timeline: Timeline<Fixed>) -> Infallible {
        let mut lcdc = this.listener.take().expect("PPU task spawned twice");
        loop {
            if !this.registers.borrow().lcdc().lcd_ppu_enable {
                this.registers.borrow_mut().set_ly(0);
                this.set_mode(PpuMode::HBlank);
                timeline
                    .after(lcdc.until(|| this.registers.borrow().lcdc().lcd_ppu_enable))
                    .await;
            }

            let lcd_off = pin!(lcdc
                .until(|| !this.registers.borrow().lcdc().lcd_ppu_enable)
                .raw_fut());
            let frame = pin!(this.frame(&timeline));
            // The lcd_off future is polled first so a step due on the same dot
            // does not run with the LCD already off (Race polls left -> right)
            let _ = Race::new(lcd_off, frame).await;
        }
    }

    async fn frame(&self, timeline: &Timeline<Fixed>) {
        self.window_y_reached.set(false);
        self.window_ly.set(0);
        for ly in 0..144 {
            // Mode 2: OAM scan
            // LYC coincidence is its own STAT source, evaluated as LY moves.
            let lyc_lcd = {
                let mut registers = self.registers.borrow_mut();
                registers.set_ly(ly);
                registers.stat().lyc_int_select && registers.stat().lyc_ly_eq
            };
            if lyc_lcd {
                self.interrupt_request.lcd(true);
            }
            self.set_mode(PpuMode::OamScan);
            // async 80 dots
            let objects = self.oam_scan(ly, timeline).await;

            // Mode 3: pixel draw
            self.set_mode(PpuMode::Drawing);
            // latch wy reached for the rest of the frame
            let wy = self.registers.borrow().wy();
            self.window_y_reached
                .set(self.window_y_reached.get() | (ly == wy));
            let dots = self.draw_line(ly, &objects, timeline).await;

            // Mode 0: HBlank
            self.set_mode(PpuMode::HBlank);
            timeline.wait(376u64.saturating_sub(dots)).await;
        }

        // Mode 1: VBlank
        self.set_mode(PpuMode::VBlank);
        self.present_frame();
        self.interrupt_request.vblank(true);
        for ly in 144..154 {
            self.registers.borrow_mut().set_ly(ly);
            timeline.wait(456).await;
            let registers = self.registers.borrow();
            let lyc_lcd = registers.stat().lyc_int_select && registers.stat().lyc_ly_eq;
            if lyc_lcd {
                self.interrupt_request.lcd(true);
            }
        }
    }

    /// A mode transition moves three things at once: the STAT mode bits, the
    /// STAT interrupt line, and bus arbitration. They live together here so
    /// they cannot drift apart — a mode set anywhere else would leave the CPU
    /// locked out of a bus the PPU is no longer using.
    ///
    /// The CPU loses OAM in modes 2 and 3 and VRAM in mode 3, and reads 0xFF
    /// for a denied access. Mode 3 has no STAT interrupt source.
    fn set_mode(&self, mode: PpuMode) {
        let raise = {
            let mut registers = self.registers.borrow_mut();
            registers.set_mode(mode);
            let stat = registers.stat();
            match mode {
                PpuMode::HBlank => stat.mode0_int_select,
                PpuMode::VBlank => stat.mode1_int_select,
                PpuMode::OamScan => stat.mode2_int_select,
                PpuMode::Drawing => false,
            }
        };
        if raise {
            self.interrupt_request.lcd(true);
        }
        self.apply_bus_locks();
    }

    /// Re-derives the locks from the current mode. Also called after an LCDC
    /// write, since disabling the LCD frees both buses mid-mode.
    fn apply_bus_locks(&self) {
        let (oam, video) = {
            let registers = self.registers.borrow();
            if !registers.lcdc().lcd_ppu_enable {
                (false, false)
            } else {
                match registers.stat().ppu_mode {
                    PpuMode::OamScan => (true, false),
                    PpuMode::Drawing => (true, true),
                    PpuMode::HBlank | PpuMode::VBlank => (false, false),
                }
            }
        };
        self.lock(BusType::Oam, oam);
        self.lock(BusType::Video, video);
    }

    fn lock(&self, bus: BusType, held: bool) {
        match held {
            true => self.bus_controller.seize(bus, BusOwner::Ppu, 0xFF),
            false => self.bus_controller.release(bus, BusOwner::Ppu),
        }
    }

    // Will be useful for the rendering passes
    fn object(&self, index: usize) -> ObjectAttribute {
        let base = index * 4;
        ObjectAttribute(u32::from_le_bytes([
            self.oam[base].get(),
            self.oam[base + 1].get(),
            self.oam[base + 2].get(),
            self.oam[base + 3].get(),
        ]))
    }

    fn scanline_mut(&self, ly: u8) -> RefMut<'_, [u8]> {
        let start = ly as usize * SCREEN_W;
        // returns a new RefMut from the passed RefMut mapped with the closure
        // Here it returns a RefMut of a scanline from the back frame buffer
        RefMut::map(self.back.borrow_mut(), |back| {
            &mut back[start..start + SCREEN_W]
        })
    }

    fn present_frame(&self) {
        // Deref through the guards: swapping the `RefMut`s themselves only
        // moves the two temporaries around and leaves both cells untouched.
        std::mem::swap(&mut *self.back.borrow_mut(), &mut *self.front.borrow_mut());
        self.frame_ready.set(true);
    }

    pub fn framebuffer(&self) -> Ref<'_, Frame> {
        // Derefs the inner &Box<Frame> to a Box<Frame>, then derefs that Box<Frame> into a
        // Frame and return a reference to it
        Ref::map(self.front.borrow(), |front| &**front)
    }

    pub fn take_frame_ready(&self) -> bool {
        self.frame_ready.replace(false)
    }

    async fn oam_scan(&self, ly: u8, timeline: &Timeline<Fixed>) -> Vec<ScannedObject> {
        let mut selected = Vec::with_capacity(10);
        for i in 0..40 {
            timeline.wait(2).await;
            if selected.len() >= 10 {
                continue;
            }
            let obj = self.object(i);
            let top = obj.y();
            // if this overflows, object isn't visible anyway (y >= 160)
            let (bottom, overflowed) =
                top.overflowing_add(self.registers.borrow().lcdc().obj_size.as_height());
            // LY's 0 is OBJ.y's 16
            let offset_ly = ly + 16;
            // OBJ has pixel on LY
            if !overflowed && (top..bottom).contains(&offset_ly) {
                selected.push(ScannedObject {
                    attributes: obj,
                    index: i,
                });
            }
        }
        selected
    }

    async fn draw_line(
        &self,
        ly: u8,
        objects: &[ScannedObject],
        timeline: &Timeline<Fixed>,
    ) -> u64 {
        let start = timeline.position();
        let state = LineState::new(ly);

        let fetcher = pin!(self.fetcher(&state, timeline.fork()));
        let shifter = pin!(self.shifter(objects, &state, timeline.clone()));
        // The fetcher never finishes: the line ends with the shifter.
        Race::new(fetcher, shifter).await;

        if state.window.get() {
            self.window_ly.set(self.window_ly.get() + 1);
        }

        timeline.position() - start
    }

    async fn fetcher(&self, state: &LineState, timeline: Timeline<Fixed>) {
        let mut fetcher_x = 0;
        let mut window = false;
        loop {
            if self.take_object_request(state, &timeline).await {
                continue;
            }
            if !window && state.window.get() {
                fetcher_x = 0;
                window = true;
                state.bg_fifo.borrow_mut().clear();
            }

            let (tile, bg_attributes) = self.read_tilemap(window, fetcher_x, state.ly);
            timeline.wait(2).await;
            let fine_y = if window {
                self.window_ly.get() & 7
            } else {
                state.ly.wrapping_add(self.registers.borrow().scy()) & 7
            };
            let bank = bg_attributes
                .as_ref()
                .map_or(0, |attribute| attribute.bank() as u8);
            let (low, high) = self.read_tile_data(tile, fine_y, bank, &timeline).await;

            let (palette, flip_x, bg_priority) =
                bg_attributes.map_or((0, false, TilePriority::None), |attributes| {
                    (
                        attributes.palette(),
                        attributes.flip_x(),
                        attributes.priority(),
                    )
                });

            // Push step: the row is held until the shifter drains the FIFO.
            // Objects are served from inside the wait rather than restarting
            // the fetch, so a completed row is never re-read.
            let pixels = Pixel::from_bytes(low, high, flip_x, palette, 0, bg_priority);
            while !state.bg_fifo.borrow().is_empty() {
                if !self.take_object_request(state, &timeline).await {
                    timeline.wait(1).await;
                }
            }
            // The row came from the background map. If the window started while
            // it was held, it is dropped rather than pushed, and the top of the
            // loop re-fetches the same x from the window map.
            if !window && state.window.get() {
                continue;
            }
            state.bg_fifo.borrow_mut().fill(pixels);

            fetcher_x += 1;
        }
    }

    /// Serves a pending object fetch, if any. The request is cleared on
    /// completion rather than on acceptance: it is what holds the shifter
    /// suspended, so releasing it early would let the shifter pop an OBJ FIFO
    /// the merge has not reached yet. Returns whether a fetch ran.
    async fn take_object_request(&self, state: &LineState, timeline: &Timeline<Fixed>) -> bool {
        let Some(req) = state.obj_request.get() else {
            return false;
        };
        self.fetch_object(req, state, timeline).await;
        state.obj_request.set(None);
        true
    }

    async fn fetch_object(&self, req: ObjRequest, state: &LineState, t: &Timeline<Fixed>) {
        // Objects always use 0x8000 addressing, whatever LCDC bit 4 says.
        let addr = (req.tile as u16 * 16 + req.fine_y as u16 * 2) as usize;
        let bank = if self.is_cgb { req.bank as usize } else { 0 };

        t.wait(2).await; // tile number step, OAM
        let low = self.vram[bank][addr].get();
        t.wait(2).await;
        let high = self.vram[bank][addr + 1].get();
        t.wait(2).await;

        let clip = 8u8.saturating_sub(req.x); // off-screen left edge
        state.obj_fifo.borrow_mut().merge(
            Pixel::from_bytes(
                low,
                high,
                req.flip_x,
                req.palette,
                req.oam_index,
                req.priority,
            ),
            clip,
            self.is_cgb,
        );
    }

    fn read_tilemap(&self, window: bool, fetcher_x: u8, ly: u8) -> (u8, Option<BgAttributes>) {
        let (tile_x, tile_y, base_addr) = self.get_tile(window, fetcher_x, ly);
        let tile_addr = base_addr + (tile_y as u16 * 32) + tile_x as u16;
        let idx = (tile_addr - 0x8000) as usize;
        (
            self.vram[0][idx].get(),
            self.is_cgb.then_some(BgAttributes(self.vram[1][idx].get())),
        )
    }

    async fn read_tile_data(
        &self,
        tile: u8,
        fine_y: u8,
        bank: u8,
        timeline: &Timeline<Fixed>,
    ) -> (u8, u8) {
        let lcdc = self.registers.borrow().lcdc();
        let tile_area = lcdc.bg_window_tiles_area;
        let tile_data_base: u16 = match tile_area {
            TileAddressing::Mode8800 => 0x9000u16.wrapping_add_signed(tile as i8 as i16 * 16),
            TileAddressing::Mode8000 => 0x8000 + (tile as u16 * 16),
        };
        let low_addr = tile_data_base + (fine_y * 2) as u16;
        let high_addr = tile_data_base + (fine_y * 2) as u16 + 1u16;

        let tile_low = self.vram[bank as usize][low_addr as usize - 0x8000].get();
        timeline.wait(2).await;
        let tile_high = self.vram[bank as usize][high_addr as usize - 0x8000].get();
        timeline.wait(2).await;

        (tile_low, tile_high)
    }

    async fn shifter(&self, objects: &[ScannedObject], state: &LineState, t: Timeline<Fixed>) {
        let obj_size = self.registers.borrow().lcdc().obj_size;
        let lcdc_obj_enable = self.registers.borrow().lcdc().obj_enable;
        let mut discard = self.registers.borrow().scx() & 7;

        let mut pending: u16 = (1 << objects.len()) - 1;
        let mut lx = 0usize;
        let mut window = false;
        // WX as it stood when the window took over, so a later write to it can
        // be told from the value that started the window.
        let mut window_wx = 0u8;

        // Warm-up: hardware discards the first fetch, so the first pixel reaches the
        // LCD 12 dots into mode 3.
        t.wait(12).await;

        while lx < 160 {
            // OAM order: at equal X the earlier entry must land first, so the
            // merge rule leaves it on top.
            let hit = lcdc_obj_enable
                .then(|| {
                    (0..objects.len()).find(|&i| {
                        is_bit_set!(pending, i)
                            && objects[i].attributes.x().saturating_sub(8) as usize == lx
                    })
                })
                .flatten();
            if let Some(i) = hit {
                pending &= !(1 << i);
                state
                    .obj_request
                    .set(Some(objects[i].into_request(state.ly, obj_size)));
                while state.obj_request.get().is_some() {
                    t.wait(1).await;
                }
                continue; // another object may start at this same lx
            }

            if !window && self.window_should_start(lx as u8) {
                window = true;
                window_wx = self.registers.borrow().wx();
                state.window.set(true);
                // Both belong to the background fetch being abandoned: the
                // pixels already queued, and the SCX fine scroll, which the
                // window is not subject to. Clearing here rather than in the
                // fetcher is what makes the switch take effect on this dot —
                // the fetcher is parked in its push stall and cannot react
                // until the FIFO drains, which is the drain being cancelled.
                state.bg_fifo.borrow_mut().clear();
                if self.registers.borrow().wx() != 0 {
                    discard = 0;
                }
                continue; // no wait: the stall comes from the cleared BG FIFO
            }

            // WX written mid-scanline, after the window started: when the new
            // value is reached, one colour-0 pixel at the lowest priority is
            // pushed onto the BG FIFO. It shifts the rest of the window right
            // by a pixel rather than replacing one, so the row is untouched and
            // this belongs here and not in the fetcher. Latching the new WX
            // arms it again for a further write.
            if window {
                let wx = self.registers.borrow().wx();
                if wx != window_wx && lx + 7 == wx as usize {
                    let _ = state.bg_fifo.borrow_mut().push(Pixel::default());
                    window_wx = wx;
                }
            }

            let Some(bg) = state.bg_fifo.borrow_mut().pop() else {
                t.wait(1).await;
                continue;
            };
            let obj = state.obj_fifo.borrow_mut().pop().unwrap_or_default();
            if discard > 0 {
                discard -= 1;
            } else {
                self.scanline_mut(state.ly)[lx] = self.mix(bg, obj);
                lx += 1;
            }
            t.wait(1).await;
        }
    }

    fn mix(&self, bg: Pixel, obj: Pixel) -> u8 {
        let registers = self.registers.borrow();
        let lcdc = registers.lcdc();
        let (bg_color, obj_priority) = if self.is_cgb {
            let priority = if lcdc.bg_window_enable {
                match bg.bg_priority {
                    TilePriority::None => obj.bg_priority,
                    TilePriority::BgWindow => TilePriority::BgWindow,
                }
            } else {
                obj.bg_priority
            };
            (bg.color, priority)
        } else {
            let bg_color = if lcdc.bg_window_enable { bg.color } else { 0 };
            (bg_color, obj.bg_priority)
        };
        let obj_palette = if obj.palette == 0 {
            registers.obp0()
        } else {
            registers.obp1()
        };

        let use_obj =
            obj.color != 0 && (matches!(obj_priority, TilePriority::None) || bg_color == 0);
        if use_obj {
            (obj_palette >> (obj.color * 2)) & 3
        } else {
            (registers.bgp() >> (bg_color * 2)) & 3
        }
    }

    fn window_should_start(&self, lx: u8) -> bool {
        self.window_y_reached.get()
            && lx + 7 >= self.registers.borrow().wx()
            && self.registers.borrow().lcdc().window_enable
    }

    fn get_tile(&self, window: bool, fetcher_x: u8, ly: u8) -> (u8, u8, u16) {
        if window {
            let base_addr = matches!(
                self.registers.borrow().lcdc().window_tilemap_area,
                TileArea::High
            )
            .then_some(0x9C00)
            .unwrap_or(0x9800);
            (fetcher_x, self.window_ly.get() / 8, base_addr)
        } else {
            let base_addr = matches!(
                self.registers.borrow().lcdc().bg_tilemap_area,
                TileArea::High
            )
            .then_some(0x9C00)
            .unwrap_or(0x9800);
            let tile_x = ((self.registers.borrow().scx() / 8) + fetcher_x) & 0x1F;
            let tile_y = ly.wrapping_add(self.registers.borrow().scy()) / 8;
            (tile_x, tile_y, base_addr)
        }
    }
}

impl MemoryHandler for Ppu {
    fn read(&self, address: u16, _: MemoryContext<'_>) -> u8 {
        let registers = self.registers.borrow();
        match address {
            0x8000..=0x9FFF => {
                let bank = (registers.vram_bank() & 1) as usize;
                let offset = (address - 0x8000) as usize;
                self.vram[bank][offset].get()
            }
            0xFE00..=0xFE9F => self.oam[(address & 0xFF) as usize].get(),
            _ => registers.read(address),
        }
    }

    fn write(&self, address: u16, value: u8, _: MemoryContext<'_>) {
        match address {
            0x8000..=0x9FFF => {
                let bank = (self.registers.borrow().vram_bank() & 1) as usize;
                let offset = (address - 0x8000) as usize;
                self.vram[bank][offset].set(value);
            }
            0xFE00..=0xFE9F => self.oam[(address & 0xFF) as usize].set(value),
            _ => {
                self.registers.borrow_mut().write(address, value);
                // LCDC bit 7 gates every lock; a write can free both buses.
                if address == 0xFF40 {
                    self.raiser.raise();
                    self.apply_bus_locks();
                }
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Time;
    use crate::cpu::interrupt::InterruptController;
    use crate::memory::mmu::{AddressMap, Mmu, Source};

    /// Drives the PPU alone, with no CPU: VRAM is poked directly, and registers
    /// go through the PPU's own write handler, since an LCDC write is what
    /// wakes a PPU parked with the LCD off.
    struct Harness {
        time: Time,
        ppu: Rc<Ppu>,
        mmu: Mmu,
        _interrupts: Rc<InterruptController>,
    }

    impl Harness {
        fn new() -> Self {
            let mut time = Time::new();
            let interrupts = Rc::new(InterruptController::new());
            let ppu = Rc::new(Ppu::new(
                interrupts.request(),
                Rc::new(Bus::new()),
                false,
                PowerOn::BootRom,
            ));
            time.spawn(|timeline: Timeline<Fixed>| Ppu::task(ppu.clone(), timeline));
            Self {
                time,
                ppu,
                // The handler never reads through the MMU; it only needs one to call.
                mmu: Mmu::new(AddressMap::new()),
                _interrupts: interrupts,
            }
        }

        fn memory_ctx(&self) -> MemoryContext<'_> {
            MemoryContext {
                mmu: &self.mmu,
                next: &[],
                source: Source::Dma,
            }
        }

        fn reg(&self, address: u16, value: u8) {
            let _ = self.ppu.write(address, value, self.memory_ctx());
        }

        fn vram(&self, address: u16, value: u8) {
            self.ppu.vram[0][(address - 0x8000) as usize].set(value);
        }

        /// One full frame is 154 lines of 456 dots.
        fn run_frame(&mut self) {
            for _ in 0..(70224) {
                self.time.tick();
            }
        }

        fn ticks(&mut self, n: usize) {
            for _ in 0..n {
                self.time.tick();
            }
        }
    }

    /// LCDC 0x91: LCD on, BG on, tilemap 0x9800, tile data 0x8000.
    /// BGP 0xE4: colour id 0->0, 1->1, 2->2, 3->3.
    fn basic_bg() -> Harness {
        let h = Harness::new();
        h.reg(0xFF40, 0x91);
        h.reg(0xFF47, 0xE4);
        h.reg(0xFF42, 0); // SCY
        h.reg(0xFF43, 0); // SCX

        // Tile 1, every row = 0b10_01_10_01... via low=0x55, high=0x33:
        //   bit7..0 low  = 0 1 0 1 0 1 0 1
        //   bit7..0 high = 0 0 1 1 0 0 1 1
        //   colour       = 0 1 2 3 0 1 2 3
        for row in 0..8 {
            h.vram(0x8010 + row * 2, 0x55);
            h.vram(0x8010 + row * 2 + 1, 0x33);
        }
        // Whole tilemap points at tile 1.
        for i in 0..0x400 {
            h.vram(0x9800 + i, 1);
        }
        h
    }

    #[test]
    fn stages_read_what_the_harness_wrote() {
        let h = basic_bg();
        let (tile, _) = h.ppu.read_tilemap(false, 0, 0);
        assert_eq!(tile, 1, "tilemap fetch");

        let lcdc = h.ppu.registers.borrow().lcdc();
        assert!(lcdc.lcd_ppu_enable, "LCDC.7");
        assert!(lcdc.bg_window_enable, "LCDC.0");
        assert_eq!(h.ppu.registers.borrow().bgp(), 0xE4, "BGP");
        assert_eq!(h.ppu.vram[0][0x0010].get(), 0x55, "tile 1 row 0 low");

        let px = Pixel::from_bytes(0x55, 0x33, false, 0, 0, TilePriority::None);
        assert_eq!(
            px.iter().map(|p| p.color).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 0, 1, 2, 3],
            "decode"
        );
        assert_eq!(h.ppu.mix(px[1], Pixel::default()), 1, "mix");
    }

    fn oam(h: &Harness, index: usize, y: u8, x: u8, tile: u8, flags: u8) {
        let base = index * 4;
        h.ppu.oam[base].set(y);
        h.ppu.oam[base + 1].set(x);
        h.ppu.oam[base + 2].set(tile);
        h.ppu.oam[base + 3].set(flags);
    }

    #[test]
    fn an_object_suspends_the_shifter_without_deadlocking() {
        let mut h = basic_bg();
        h.reg(0xFF40, 0x93); // + LCDC.1, objects on
        h.reg(0xFF48, 0xE4); // OBP0

        // Tile 2: every pixel colour 1.
        for row in 0..8 {
            h.vram(0x8020 + row * 2, 0xFF);
            h.vram(0x8020 + row * 2 + 1, 0x00);
        }
        // Y=16 puts it on line 0; X=16 puts its left edge at screen x 8.
        oam(&h, 0, 16, 16, 2, 0x00);

        h.run_frame();
        assert!(
            h.ppu.take_frame_ready(),
            "no frame presented: the object fetch deadlocked mode 3"
        );

        let frame = h.ppu.framebuffer();
        assert_eq!(
            frame[8..16].to_vec(),
            vec![1u8; 8],
            "object did not land at screen x 8"
        );
        assert_eq!(
            frame[0..8].to_vec(),
            vec![0, 1, 2, 3, 0, 1, 2, 3],
            "background left of the object was disturbed"
        );
    }

    #[test]
    fn mode3_completes_and_advances_ly() {
        let mut h = basic_bg();
        h.run_frame();
        // If mode 3 ever deadlocked, LY would be parked wherever it stalled.
        assert!(
            h.ppu.take_frame_ready(),
            "no frame was presented in 70224 dots: mode 3 never finished a full field"
        );
    }

    #[test]
    fn renders_the_background_tile_pattern() {
        let mut h = basic_bg();
        h.run_frame();

        let frame = h.ppu.framebuffer();
        let row: Vec<u8> = frame[0..16].to_vec();
        assert_eq!(
            row,
            vec![0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3],
            "first 16 pixels of line 0 do not match the tile pattern"
        );
    }

    #[test]
    fn scx_shifts_the_background_left() {
        let mut h = basic_bg();
        h.reg(0xFF43, 2); // SCX = 2 discards two pixels from the first tile
        h.run_frame();

        let frame = h.ppu.framebuffer();
        assert_eq!(
            frame[0..8].to_vec(),
            vec![2, 3, 0, 1, 2, 3, 0, 1],
            "SCX & 7 discard did not shift the line"
        );
    }

    /// The PPU wakes on the dot after `basic_bg`'s LCDC write, so line N, dot D
    /// is tick 1 + N * 456 + D.
    const LINE_10_MODE_3: usize = 1 + 10 * 456 + 90;

    #[test]
    fn lcd_off_mid_mode3_resets_on_next_dot() {
        let mut h = basic_bg();

        // Ticks to allow run_frame to run past the max number of cycles per frame
        h.ticks(LINE_10_MODE_3);

        assert_eq!(h.ppu.registers.borrow().ly(), 10);
        assert_eq!(h.ppu.registers.borrow().stat().ppu_mode, PpuMode::Drawing);

        // Disable LCD mid-frame
        let mut lcdc = h.ppu.registers.borrow().lcdc();
        lcdc.lcd_ppu_enable = false;

        h.reg(0xFF40, lcdc.into());
        h.ticks(1);

        assert_eq!(h.ppu.registers.borrow().ly(), 0);
        assert_eq!(h.ppu.registers.borrow().stat().ppu_mode, PpuMode::HBlank);

        // Max cycles per frame
        h.run_frame();

        assert!(!h.ppu.take_frame_ready(), "A cancelled frame was presented");
    }

    #[test]
    fn re_enabling_restarts_the_frame_on_the_next_dot() {
        let mut h = basic_bg();
        h.ticks(LINE_10_MODE_3);

        // Disable LCD mid-frame
        let mut lcdc = h.ppu.registers.borrow().lcdc();
        lcdc.lcd_ppu_enable = false;
        h.reg(0xFF40, lcdc.into());

        h.ticks(3);

        // Reenable LCD
        lcdc = h.ppu.registers.borrow().lcdc();
        lcdc.lcd_ppu_enable = true;
        h.reg(0xFF40, lcdc.into());

        // Restarts on the next dot; VBlank lands 144 lines after that.
        h.ticks(144 * 456);
        assert!(!h.ppu.take_frame_ready());
        h.ticks(1);
        assert!(h.ppu.take_frame_ready());
    }
}
