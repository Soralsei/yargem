use std::cell::Ref;
use std::rc::Rc;

use crate::audio::apu::Apu;
use crate::clock::{CpuClock, Cycles, Time};
use crate::graphics::oam::DMAController;
use crate::graphics::ppu::{Frame, Ppu};
use crate::input::{Button, JoypadHandler};
use crate::memory::bus::{BusController, CpuBus};
use crate::memory::mmu::AddressMap;
use crate::timer::Timer;

use super::memory::mbc::Mbc;
use super::memory::mmu::MemoryHandler;

use super::cpu::cpu::Cpu;
use super::cpu::interrupt::InterruptController;
#[cfg(feature = "blaarg")]
use super::debug::blaarg_spy::BlaargSpy;
use super::memory::mmu::Mmu;
use super::memory::serial::{LogSink, Serial};

const CYCLES_PER_FRAME: Cycles = 70224; // 154 lines * 456 T-cycles

/// KEY1 (0xFF4D). Both meaningful bits — current speed and the armed switch —
/// are clock state, so the handler is a pure view onto `Clock`.
struct SpeedSwitch(Rc<CpuClock>);

impl MemoryHandler for SpeedSwitch {
    fn read(&self, _address: u16, _next: crate::memory::mmu::Next<'_>) -> u8 {
        self.0.key1()
    }

    fn write(&self, _address: u16, value: u8, _next: crate::memory::mmu::Next<'_>) {
        self.0.arm(value & 1 != 0);
    }
}

struct Unmapped;

impl MemoryHandler for Unmapped {
    fn read(&self, _address: u16, _next: crate::memory::mmu::Next<'_>) -> u8 {
        0xFF
    }
}

pub struct System {
    cpu: Rc<Cpu>,
    irq: Rc<InterruptController>,
    ppu: Rc<Ppu>,
    apu: Rc<Apu>,
    time: Time,
    joypad: Rc<JoypadHandler>,
}

impl System {
    pub fn new(boot_rom: Option<Vec<u8>>, rom: Vec<u8>, is_cgb_override: bool) -> Self {
        let mut time = Time::new();

        let mbc = Rc::new(Mbc::new(boot_rom, rom));
        let is_cgb = mbc.cartridge().is_cgb_only() || is_cgb_override;

        let interrupt_controller = Rc::new(InterruptController::new());
        let serial = Rc::new(Serial::new(interrupt_controller.request(), LogSink));
        let timer = Rc::new(Timer::new(
            interrupt_controller.request(),
            time.cpu.clone(),
            is_cgb,
        ));
        let joypad = Rc::new(JoypadHandler::new(interrupt_controller.request()));

        let bus_controller = Rc::new(BusController::new());
        let dma = Rc::new(DMAController::new(bus_controller.clone()));

        // The bus is shared: the CPU drives it, and clocked devices that move
        // bytes themselves (OAM DMA, the PPU fetcher) need a handle to it too.
        let cpu = Rc::new(Cpu::new(is_cgb));
        let ppu = Rc::new(Ppu::new(
            interrupt_controller.request(),
            bus_controller.clone(),
            is_cgb,
        ));
        let apu = Rc::new(Apu::new(time.fixed.clone(), is_cgb));

        let mut map = AddressMap::new();

        #[cfg(feature = "blaarg")]
        {
            println!("Added blaarg debug feature");
            mmu.add_handler((0xA000, 0xBFFF), Rc::new(BlaargSpy));
        }

        map.add((0x0000, 0x7FFF), mbc.clone());
        map.add((0xFF50, 0xFF50), mbc.clone());
        map.add((0xA000, 0xBFFF), mbc.clone());

        // PPU VRAM
        map.add((0x8000, 0x9FFF), ppu.clone());
        // OAM
        map.add((0xFE00, 0xFE9F), ppu.clone());
        // Ppu registers other than OAM DMA
        map.add((0xFF40, 0xFF45), ppu.clone());
        // OAM DMA register
        map.add((0xFF46, 0xFF46), dma.clone());
        // Ppu rest of registers
        map.add((0xFF47, 0xFF4B), ppu.clone());

        map.add((0xFF00, 0xFF00), joypad.clone());

        map.add((0xFF01, 0xFF02), serial.clone());
        map.add((0xFF04, 0xFF07), timer.clone());

        map.add((0xFF0F, 0xFF0F), interrupt_controller.clone());
        map.add((0xFF10, 0xFF3F), apu.clone());
        if is_cgb {
            map.add((0xFF4D, 0xFF4D), Rc::new(SpeedSwitch(time.cpu.clone())));
            // TODO: map other IO registers in the CGB
        } else {
            let unmapped = Rc::new(Unmapped);
            map.add((0xFF4D, 0xFF4D), unmapped.clone()); // KEY1
            map.add((0xFF4F, 0xFF4F), unmapped.clone()); // VBK
            map.add((0xFF51, 0xFF55), unmapped.clone()); // HDMA1-5
            map.add((0xFF68, 0xFF6B), unmapped.clone()); // BCPS/BCPD/OCPS/OCPD
            map.add((0xFF70, 0xFF70), unmapped);
        }
        map.add((0xFFFF, 0xFFFF), interrupt_controller.clone());
        let mmu = Rc::new(Mmu::new(map));

        time.spawn(|timeline| Ppu::task(ppu.clone(), timeline));
        time.spawn(|timeline| Serial::task(serial.clone(), timeline));
        time.spawn(|timeline| DMAController::task(dma.clone(), mmu.clone(), timeline));
        time.spawn(|timeline| Timer::task(timer.clone(), timeline));

        time.spawn(|()| Apu::frame_sequencer_task(apu.clone(), timer.div_apu()));
        // time.spawn(|timeline| Apu::generator_task(apu.clone(), timeline));

        let cpu_clock = time.cpu.clone();
        let fixed_clock = time.fixed.clone();

        time.spawn(|(t_cpu, t_fixed)| {
            Cpu::task(
                cpu.clone(),
                interrupt_controller.clone(),
                CpuBus::new(mmu.clone(), bus_controller.clone()),
                t_cpu,
                t_fixed,
                cpu_clock,
                fixed_clock,
            )
        });

        Self {
            cpu,
            irq: interrupt_controller,
            ppu,
            apu,
            time,
            joypad,
        }
    }

    pub fn step(&mut self) {
        let mark = self.cpu.instr_count();
        while self.cpu.instr_count() == mark && !self.cpu_parked() {
            self.time.advance(CYCLES_PER_FRAME);
        }
    }

    pub fn run_frame(&mut self) {
        const BUDGET: Cycles = 2 * CYCLES_PER_FRAME;
        let mut done = 0;
        while done < BUDGET {
            let advanced = self.time.advance(CYCLES_PER_FRAME);
            // Both clocks are stalled, no point in running frames
            // nothing can happen on the system without outside action
            // or a frame can be displayed
            if advanced == 0 || self.ppu.take_frame_ready() {
                break;
            }
            done += advanced;
        }
    }

    pub fn get_framebuffer(&self) -> Ref<'_, Frame> {
        self.ppu.framebuffer()
    }

    pub fn drain_audio(&self, max: usize, sink: impl FnMut([f32; 2])) {
        self.apu.drain_samples(max, sink);
    }

    pub fn set_button(&mut self, btn: Button, down: bool) {
        self.joypad.set(btn, down);
    }

    pub fn cpu_parked(&self) -> bool {
        self.cpu.wedged()
            || self.time.stopped()
            || (self.cpu.halted() && self.irq.enabled() & 0x1F == 0)
    }
}
