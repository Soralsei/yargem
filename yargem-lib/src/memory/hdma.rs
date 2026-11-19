use std::{cell::Cell, convert::Infallible, pin::pin, rc::Rc};

use crate::{
    clock::{Fixed, Timeline},
    cpu::hold::HoldRequest,
    executor::{edge, EdgeListener, EdgeRaiser},
    is_bit_set,
    memory::mmu::{MemoryContext, MemoryHandler, Mmu, Source},
    util::{
        bit_operations::{bytes_to_word, word_to_bytes},
        future::{Either, Race},
    },
};

#[derive(Debug, Clone, Copy, PartialEq)]
enum TransferState {
    Idle,
    General,
    HBlank,
}

#[derive(Debug, Clone, Copy)]
struct DmaControl(u8);

impl DmaControl {
    fn dma_type(&self) -> TransferState {
        if is_bit_set!(self.0, 7) {
            TransferState::HBlank
        } else {
            TransferState::General
        }
    }

    fn len(&self) -> u8 {
        self.0 & 0x7F
    }
}

struct DmaState {
    source_hi: Cell<u8>, // HDMA1
    source_lo: Cell<u8>, // HDMA2
    dest_hi: Cell<u8>,   // HDMA3
    dest_lo: Cell<u8>,   // HDMA4
    remaining: Cell<u8>,
    transfer_state: Cell<TransferState>,
}

impl DmaState {
    fn new() -> Self {
        Self {
            source_hi: Cell::new(0x00),
            source_lo: Cell::new(0x00),
            dest_hi: Cell::new(0x80),
            dest_lo: Cell::new(0x00),
            remaining: Cell::new(0x7F),
            transfer_state: Cell::new(TransferState::Idle),
        }
    }

    fn source(&self) -> u16 {
        bytes_to_word(self.source_hi.get(), self.source_lo.get())
    }

    fn dest(&self) -> u16 {
        bytes_to_word(self.dest_hi.get(), self.dest_lo.get())
    }

    fn set_src(&self, src: u16) {
        let (hi, lo) = word_to_bytes(src);
        self.source_lo.set(lo);
        self.source_hi.set(hi);
    }

    fn set_dst(&self, dst: u16) {
        let (hi, lo) = word_to_bytes(dst);
        self.dest_lo.set(lo);
        self.dest_hi.set(hi);
    }
}

pub struct DMAController {
    state: DmaState,
    // Taken by the task on spawn
    transfer_listener: Cell<Option<EdgeListener>>,
    transfer_write: EdgeRaiser,
}

impl DMAController {
    pub fn new() -> Self {
        let (raiser, listener) = edge();
        Self {
            state: DmaState::new(),
            transfer_listener: Cell::new(Some(listener)),
            transfer_write: raiser,
        }
    }

    pub async fn task(
        this: Rc<Self>,
        mmu: Rc<Mmu>,
        mut hold_req: HoldRequest,
        mut mode0_listener: EdgeListener,
        t_fixed: Timeline<Fixed>,
    ) -> Infallible {
        let mut transfer_listener = this
            .transfer_listener
            .take()
            .expect("DmaController::task spawned twice");
        loop {
            while this.state.transfer_state.get() == TransferState::Idle {
                t_fixed.after(transfer_listener.next()).await;
            }

            if this.state.transfer_state.get() == TransferState::General {
                t_fixed.after(hold_req.until_granted()).await;
            }

            // Replace with trace level logging at some point
            eprintln!(
                "starting DMA {:#?}, src: ${:04X}, dst: ${:04X}, num blocks: {}",
                this.state.transfer_state.get(),
                this.state.source(),
                this.state.dest(),
                this.state.remaining.get(),
            );
            if this.state.dest() == 0x9C00 {
                let vbk = mmu.read(0xFF4F, Source::Debugger);
                eprintln!("VBK at $9C00 : ${vbk:02X}");
            }

            'transfer: while this.state.remaining.get() != 0x7F {
                if this.state.transfer_state.get() == TransferState::HBlank {
                    let which = this
                        .mode0_cancel_race(&mut mode0_listener, &t_fixed, &mut transfer_listener)
                        .await;
                    match which {
                        Either::Left(_) => {}
                        Either::Right(_) => {
                            //Transfer was cancelled by the CPU (or overwritten before mode0)
                            break 'transfer;
                        }
                    }
                    t_fixed.after(hold_req.until_granted()).await;
                }

                let mut src = this.state.source();
                let mut dst = this.state.dest();
                for _ in 0..16 {
                    t_fixed.wait(2).await;

                    let (new_src, new_dst) = match this.transfer_byte(&mmu, src, dst) {
                        Some(value) => value,
                        // cancel active transfer on src/dst overflow
                        None => break 'transfer,
                    };
                    src = new_src;
                    dst = new_dst;
                }
                if this.state.transfer_state.get() == TransferState::HBlank {
                    hold_req.release();
                }
                this.state.set_src(src);
                this.state.set_dst(dst);

                this.state
                    .remaining
                    .set(this.state.remaining.get().wrapping_sub(1) & 0x7F);
            }

            this.state.transfer_state.set(TransferState::Idle);
            hold_req.release();
        }
    }

    async fn mode0_cancel_race(
        &self,
        mode0_listener: &mut EdgeListener,
        t_fixed: &Timeline<Fixed>,
        transfer_listener: &mut EdgeListener,
    ) -> Either<(), ()> {
        let mode0_fut = pin!(t_fixed.after(mode0_listener.next()));
        let cancel_fut = pin!(t_fixed.after(transfer_listener.next()));
        let which = Race::new(mode0_fut, cancel_fut).await;
        which
    }

    fn transfer_byte(&self, mmu: &Rc<Mmu>, src: u16, dst: u16) -> Option<(u16, u16)> {
        let byte = mmu.read(src, Source::Dma);
        mmu.write(dst, byte, Source::Dma);

        let (new_src, src_overflowed) = src.overflowing_add(1);
        let new_dst = dst + 1;

        if src_overflowed || new_dst >= 0xA000 {
            self.state.transfer_state.set(TransferState::Idle);
            self.state.remaining.set(0x7F);
            return None;
        }

        Some((new_src, new_dst))
    }
}

impl MemoryHandler for DMAController {
    fn read(&self, address: u16, _: MemoryContext<'_>) -> u8 {
        match address {
            0xFF51..=0xFF54 => 0xFF,
            0xFF55 => {
                (if self.state.transfer_state.get() != TransferState::Idle {
                    0
                } else {
                    0x80
                }) | self.state.remaining.get()
            }
            _ => unreachable!("invalid read in HdmaController at 0x{address:04X}"),
        }
    }

    fn write(&self, address: u16, value: u8, _: MemoryContext<'_>) {
        match address {
            0xFF51 => self.state.source_hi.set(value),
            0xFF52 => self.state.source_lo.set(value & 0xF0),
            0xFF53 => self.state.dest_hi.set(0x80 | (value & 0x1F)),
            0xFF54 => self.state.dest_lo.set(value & 0xF0),
            0xFF55 => {
                let value = DmaControl(value);

                self.state.remaining.set(value.len());

                self.transfer_write.raise();

                if self.state.transfer_state.get() == TransferState::HBlank
                    && value.dma_type() == TransferState::General
                {
                    self.state.transfer_state.set(TransferState::Idle);
                    return;
                }

                self.state.transfer_state.set(value.dma_type());
            }
            _ => unreachable!("invalid read in HdmaController at 0x{address:04X}"),
        };
    }
}
