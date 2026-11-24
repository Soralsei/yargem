// TODO

use std::fmt::Write;
use std::ops::Range;

use egui::{CentralPanel, Context, Frame, ScrollArea, TextStyle, Ui, ViewportBuilder, ViewportId};
use yargem_lib::debug::protocol::DebugCommand;

use crate::{
    app::DebugState,
    engine::{EngineCommand, EngineHandle},
    ui::theme::Theme,
};

struct MemCache {
    start: u16,
    bytes: Vec<u8>,
    pending: Option<Range<usize>>,
    in_flight: bool,
}

pub struct MemoryViewer<
    const FETCH_SIZE: usize = 1024,
    const MARGIN: usize = 4096,
    const TOTAL_BYTES: usize = 0x10000,
    const BYTES_PER_ROW: usize = 16,
> {
    pub open: bool,
    cache: MemCache,
    goto_input: String,
}

impl<
        const FETCH_SIZE: usize,
        const MARGIN: usize,
        const TOTAL_BYTES: usize,
        const BYTES_PER_ROW: usize,
    > MemoryViewer<FETCH_SIZE, MARGIN, TOTAL_BYTES, BYTES_PER_ROW>
{
    pub fn new() -> Self {
        Self {
            open: false,
            cache: MemCache {
                start: 0,
                bytes: Vec::new(),
                in_flight: false,
                pending: None,
            },
            goto_input: "".to_owned(),
        }
    }

    pub fn set_cache_data(&mut self, start: u16, bytes: Vec<u8>, engine: Option<&EngineHandle>) {
        self.cache.start = start;
        self.cache.bytes = bytes;
        self.cache.in_flight = false;
        if let (Some(pending), Some(engine)) = (&self.cache.pending, engine) {
            engine.send(EngineCommand::Debug(DebugCommand::ReadMemory {
                start: pending.start as u16,
                len: pending.len() as u16,
            }));
            self.cache.in_flight = true;
        }
    }

    pub fn show(
        &mut self,
        ctx: &Context,
        engine: Option<&EngineHandle>,
        state: Option<&DebugState>,
    ) {
        if self.open {
            ctx.show_viewport_immediate(
                ViewportId::from_hash_of("memory_viewer"),
                ViewportBuilder::default()
                    .with_title("Memory viewer")
                    .with_inner_size([400.0, 300.0]),
                |ui, _class| {
                    // Handle the OS window's close ('X') button
                    if ctx.input(|i| i.viewport().close_requested()) {
                        self.open = false;
                        return;
                    }
                    self.ctrl_header(ui);
                    self.mem_area(ui, engine, state);
                },
            )
        }
    }

    fn ctrl_header(&mut self, ui: &mut Ui) {}

    fn mem_area(&mut self, ui: &mut Ui, engine: Option<&EngineHandle>, state: Option<&DebugState>) {
        if self.cache.bytes.len() == 0 && !self.cache.in_flight {
            if let Some(engine) = engine {
                engine.send(EngineCommand::Debug(DebugCommand::ReadMemory {
                    start: 0,
                    len: (FETCH_SIZE + MARGIN) as u16,
                }));
                self.cache.in_flight = true;
            }
        }
        let total_rows: usize = const { TOTAL_BYTES / BYTES_PER_ROW };
        let row_height = ui.text_style_height(&TextStyle::Monospace);

        let mut line_buf = String::with_capacity(80);

        CentralPanel::default()
            .frame(Frame::new().fill(Theme::of(ui).code_bg).outer_margin(16.0))
            .show(ui, |ui| {
                ScrollArea::vertical().show_rows(ui, row_height, total_rows, |ui, row_range| {
                    let vis_start = row_range.start * BYTES_PER_ROW;
                    let vis_end = (row_range.end * BYTES_PER_ROW).min(TOTAL_BYTES);

                    let cache_start = self.cache.start as usize;
                    let cache_end = cache_start + self.cache.bytes.len();

                    let near_top =
                        vis_start < cache_start.saturating_add(MARGIN) && cache_start > 0;
                    let near_bottom =
                        vis_end > cache_end.saturating_sub(MARGIN) && cache_end < TOTAL_BYTES;

                    if near_top || near_bottom {
                        let center = (vis_start + vis_end) / 2;
                        let req_start = center.saturating_sub(FETCH_SIZE / 2) & !0xF;
                        let req_end = (req_start + FETCH_SIZE).min(TOTAL_BYTES);

                        if !self.cache.in_flight {
                            if let Some(engine) = engine {
                                let _ =
                                    engine.send(EngineCommand::Debug(DebugCommand::ReadMemory {
                                        start: req_start as u16,
                                        len: (req_end - req_start) as u16,
                                    }));
                                self.cache.in_flight = true;
                            }
                        } else {
                            // Overwrite any older queued request so you only ever fetch the latest position
                            self.cache.pending = Some(req_start..req_end);
                        }
                    }
                    for row in row_range {
                        line_buf.clear();
                        let offset = row * 16;

                        if !(cache_start..cache_end).contains(&offset) {
                            continue;
                        }

                        let chunk = &self.cache.bytes[(offset - cache_start)..(cache_end - offset)];

                        let _ = write!(line_buf, "{:08X}\t\t", offset);
                        for i in 0..16 {
                            if let Some(b) = chunk.get(i) {
                                let _ = write!(line_buf, "{:02X}\t", b);
                            } else {
                                line_buf.push_str("??\t");
                            }
                        }
                        line_buf.push_str("\t");
                        for &b in chunk {
                            line_buf.push(if b.is_ascii_graphic() { b as char } else { '.' });
                        }
                        ui.monospace(&line_buf);
                    }
                })
            });
    }
}
