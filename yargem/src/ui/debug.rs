use egui::containers::Sides;
use egui::{
    collapsing_header::CollapsingState, Align, Align2, Button, CentralPanel, CollapsingHeader,
    Frame, Grid, IntoAtoms, Key, KeyboardShortcut, Layout, Margin, Modifiers, Panel, RichText,
    Sense, Stroke, StrokeKind, TextEdit, TextStyle, Ui, WidgetText,
};
use yargem_common::Reg16;
use yargem_lib::debug::protocol::{
    AccessKind, BankAddr, BankRange, DebugCommand, Flags, StopReason,
};

use crate::{
    app::DebugState,
    engine::{EngineCommand, EngineHandle},
    ui::{
        components::{
            badge::{Badge, Dot, Tone},
            segmented::{Segmented, SegmentedEnum},
        },
        style,
        theme::Theme,
    },
};

impl SegmentedEnum for AccessKind {
    const VARIANTS: &'static [Self] = &[AccessKind::Read, AccessKind::Write];

    fn label(&self) -> WidgetText {
        match self {
            AccessKind::Read => "R",
            AccessKind::Write => "W",
        }
        .into()
    }

    fn tooltip(&self) -> Option<WidgetText> {
        Some(format!("{self:?}").into())
    }
}

pub fn controls_bar(ui: &mut Ui, engine: Option<&EngineHandle>, state: &DebugState) {
    let Some(e) = engine else { return };
    ui.horizontal(|ui| {
        ctrl_button(
            ui,
            format!("{} Pause", egui_phosphor::regular::PAUSE),
            !state.paused,
            Some(Key::F6),
            || {
                e.send(EngineCommand::Debug(DebugCommand::Pause));
            },
        );
        ctrl_button(
            ui,
            format!("{} Resume", egui_phosphor::regular::PLAY),
            state.paused,
            Some(Key::F5),
            || {
                e.send(EngineCommand::Debug(DebugCommand::Resume));
            },
        );
        ctrl_button(
            ui,
            format!("{} Step", egui_phosphor::regular::ARROW_LINE_DOWN),
            state.paused,
            Some(Key::F11),
            || {
                e.send(EngineCommand::Debug(DebugCommand::StepInstruction));
            },
        );
        ui.separator();
        ctrl_button(
            ui,
            format!("{} Step scanline", egui_phosphor::regular::LIST),
            state.paused,
            None,
            || {
                e.send(EngineCommand::Debug(DebugCommand::StepScanline));
            },
        );
        ctrl_button(
            ui,
            format!("{} Step frame", egui_phosphor::regular::STOP),
            state.paused,
            Some(Key::F9),
            || {
                e.send(EngineCommand::Debug(DebugCommand::StepFrame));
            },
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if !state.paused {
                ui.label(RichText::new("Running").color(Theme::of(ui).text_weak));
                ui.add(Dot::new(Tone::Custom(Theme::of(ui).running)));
                return;
            }
            if let (Some(regs), Some(reason)) = (state.last_regs.as_ref(), state.last_stop.as_ref())
            {
                ui.horizontal(|ui| {
                    let label = match reason {
                        StopReason::Breakpoint(bank_addr) => format!(
                            "Paused - breakpoint {}${:04X}",
                            bank_addr
                                .bank
                                .map_or("".to_owned(), |b| format!("{:02X}", b) + ":"),
                            bank_addr.address
                        ),
                        StopReason::Watchpoint(hit) => format!(
                            "Paused - watchpoint #{} {} {}${:04X} {} -> {}",
                            hit.id.0,
                            match hit.access {
                                AccessKind::Read => "read",
                                AccessKind::Write => "write",
                            },
                            hit.bank
                                .map_or("".to_owned(), |b| format!("{:02X}", b) + ":"),
                            hit.addr,
                            hit.old,
                            hit.new
                        ),
                        StopReason::Step | StopReason::UserPause => {
                            format!("Paused - step at ${:04X}", regs.pc)
                        }
                    };
                    ui.label(label);
                    ui.add(Dot::new(Tone::Custom(Theme::of(ui).stop(reason))));
                });
            }
        });
    });
}

fn ctrl_button<'a>(
    ui: &mut Ui,
    atoms: impl IntoAtoms<'a>,
    enabled: bool,
    shortcut_key: Option<Key>,
    on_action: impl Fn(),
) {
    let mut button = Button::new(atoms);
    let mut shortcut_pressed = false;
    if let Some(key) = shortcut_key {
        button = button.shortcut_text(key.symbol_or_name());
        shortcut_pressed = ui.input_mut(|input| {
            input.consume_shortcut(&KeyboardShortcut::new(Modifiers::NONE, key))
        });
    }
    let activated = ui.add_enabled(enabled, button).clicked() || shortcut_pressed;
    if activated {
        (on_action)();
    }
}

#[derive(Default)]
pub struct DebugPanel {
    pub open: bool,
    bp_err: Option<anyhow::Error>,
    bp_input: String,
    watch_start: String,
    watch_end: String,
    watch_access: AccessKind,
    wp_err: Option<anyhow::Error>,
}

impl DebugPanel {
    pub fn new() -> Self {
        Self {
            watch_access: AccessKind::Read,
            ..Default::default()
        }
    }

    pub fn show(&mut self, ui: &mut Ui, engine: Option<&EngineHandle>, state: &DebugState) {
        Panel::right("dbg_panel")
            .default_size(340.0)
            .resizable(false)
            .show(ui, |ui| {
                Frame::new()
                    .inner_margin(Margin::symmetric(5, 16))
                    .show(ui, |ui| {
                        Panel::top("dbg_cpu").show(ui, |ui| {
                            cpu_accordion(state, ui);
                            ui.add_space(style::SECTION_GAP);
                        });
                        Panel::bottom("dbg_lists").show(ui, |ui| {
                            self.break_watch_lists(state, ui, engine);
                        });
                        CentralPanel::default().show(ui, |ui| {
                            CollapsingHeader::new("Disassembly")
                                .default_open(false)
                                .show(ui, |ui| {
                                    ui.label("Planned");
                                });
                        });
                    })
            });
    }

    fn break_watch_lists(
        &mut self,
        state: &DebugState,
        ui: &mut Ui,
        engine: Option<&EngineHandle>,
    ) {
        let theme = Theme::of(ui);
        ui.add_space(style::SECTION_GAP);
        let id = ui.make_persistent_id("dbg_breakpoints");
        CollapsingState::load_with_default_open(ui.ctx(), id, true)
            .show_header(ui, |ui| {
                ui.strong("Breakpoints");
                ui.add(Badge::new(
                    RichText::new(&state.breakpoints.len().to_string()).monospace(),
                ));
            })
            .body_unindented(|ui| {
                ui.vertical_centered(|ui| {
                    self.breakpoint_input(engine, &theme, ui);
                    breakpoint_list(ui, &theme, state, engine)
                });
            });

        ui.add_space(style::SECTION_GAP);

        let id = ui.make_persistent_id("dbg_watchpoints");
        CollapsingState::load_with_default_open(ui.ctx(), id, true)
            .show_header(ui, |ui| {
                ui.strong("Watchpoints");
                ui.add(Badge::new(
                    RichText::new(&state.watchpoints.len().to_string()).monospace(),
                ));
            })
            .body_unindented(|ui| {
                ui.vertical_centered(|ui| {
                    self.watchpoint_input(ui, &theme, engine);
                    watchpoint_list(ui, &theme, state, engine);
                });
            });
    }

    fn breakpoint_input(&mut self, engine: Option<&EngineHandle>, theme: &Theme, ui: &mut Ui) {
        ui.vertical(|ui| {
            let (resp, add) = Sides::new().shrink_left().show(
                ui,
                |ui| {
                    ui.add_space(ui.spacing().indent);
                    let input_label = ui.weak("Break at");
                    if self.bp_err.is_some() {
                        ui.visuals_mut().selection.stroke.color = theme.error;
                    }
                    ui.add(
                        TextEdit::singleline(&mut self.bp_input)
                            .desired_width(ui.available_width())
                            .font(TextStyle::Monospace)
                            .hint_text("$200 or <bank>:$4000"),
                    )
                    .labelled_by(input_label.id)
                },
                |ui| ui.button("Add"),
            );

            if resp.changed() {
                self.bp_err = None;
            }
            let entered = resp.lost_focus() && ui.input(|i| i.key_down(Key::Enter));
            if add.clicked() || entered {
                self.add_breakpoint(engine);
                resp.request_focus();
            }

            if let Some(err) = &self.bp_err {
                let input_left = resp.rect.left();
                ui.horizontal(|ui| {
                    ui.add_space(input_left - ui.cursor().left());
                    ui.label(RichText::new(err.to_string()).color(theme.error))
                });
            }
        });
    }

    fn add_breakpoint(&mut self, engine: Option<&EngineHandle>) {
        // Do nothing
        if self.bp_input.is_empty() {
            return;
        }
        match (parse_bank_addr(&self.bp_input), engine) {
            (Ok(bank_addr), Some(engine)) => {
                engine.send(EngineCommand::Debug(DebugCommand::AddBreakpoint(bank_addr)));
                self.bp_input.clear();
            }
            (Err(e), _) => self.bp_err = Some(e),
            _ => {}
        }
    }

    fn watchpoint_input(&mut self, ui: &mut Ui, theme: &Theme, engine: Option<&EngineHandle>) {
        ui.vertical(|ui| {
            let ((resp_start, resp_end), add) = Sides::new().shrink_left().show(
                ui,
                |ui| {
                    ui.add_space(ui.spacing().indent);
                    let input_label = ui.weak("Watch");
                    if self.wp_err.is_some() {
                        ui.visuals_mut().selection.stroke.color = theme.error;
                    }
                    // Both edits share what's left after the label and the "-".
                    let dash_w = WidgetText::from("-")
                        .into_galley(ui, None, f32::INFINITY, TextStyle::Body)
                        .size()
                        .x;
                    let w = ((ui.available_width() - dash_w - 2.0 * ui.spacing().item_spacing.x)
                        / 2.0)
                        .max(40.0);
                    let start = ui
                        .add(
                            TextEdit::singleline(&mut self.watch_start)
                                .desired_width(w)
                                .font(TextStyle::Monospace)
                                .hint_text("$C000"),
                        )
                        .labelled_by(input_label.id);
                    ui.weak("-");
                    let end = ui
                        .add(
                            TextEdit::singleline(&mut self.watch_end)
                                .desired_width(w)
                                .font(TextStyle::Monospace)
                                .hint_text("end"),
                        )
                        .labelled_by(input_label.id);
                    (start, end)
                },
                |ui| {
                    // Right side is right-to-left: first added is rightmost.
                    let add = ui.button("Add");
                    ui.add(Segmented::from_enum(&mut self.watch_access));
                    add
                },
            );

            if resp_start.changed() || resp_end.changed() {
                self.wp_err = None;
            }
            let resp_lost = if resp_end.lost_focus() {
                Some(&resp_end)
            } else {
                resp_start.lost_focus().then_some(&resp_start)
            };
            let entered = resp_lost.is_some() && ui.input(|i| i.key_down(Key::Enter));
            if add.clicked() || entered {
                self.add_watchpoint(engine);
                if let Some(resp) = resp_lost {
                    resp.request_focus();
                }
            }

            if let Some(err) = &self.wp_err {
                let input_left = resp_start.rect.left();
                ui.horizontal(|ui| {
                    ui.add_space(input_left - ui.cursor().left());
                    ui.label(RichText::new(err.to_string()).color(theme.error))
                });
            }
        });
    }

    fn add_watchpoint(&mut self, engine: Option<&EngineHandle>) {
        if self.watch_start.is_empty() {
            return;
        }
        let maybe_end = (!self.watch_end.is_empty())
            .then_some(&self.watch_end)
            .map(|end| parse_hex(end));
        let bank_addr = parse_bank_addr(&self.watch_start);
        match (bank_addr, engine) {
            (Ok(bank_addr), Some(e)) => {
                let end = maybe_end.map_or(Ok(bank_addr.address), |end| end);
                match end {
                    Ok(end) => {
                        if end < bank_addr.address {
                            self.wp_err = Some(anyhow::anyhow!("end be before start"));
                            return;
                        }
                        let bank_range = BankRange {
                            bank: bank_addr.bank,
                            range: (bank_addr.address..=end),
                        };
                        e.send(EngineCommand::Debug(DebugCommand::AddWatchpoint {
                            bank_range,
                            access: self.watch_access,
                        }));
                        self.watch_start.clear();
                        self.watch_end.clear();
                        self.watch_access = AccessKind::Read;
                    }
                    Err(e) => self.wp_err = Some(e),
                }
            }
            (Err(e), _) => self.wp_err = Some(e),
            _ => {}
        }
    }
}

fn breakpoint_list(ui: &mut Ui, theme: &Theme, state: &DebugState, engine: Option<&EngineHandle>) {
    if state.breakpoints.is_empty() {
        return;
    }
    ui.add_space(10.0);
    let stop_row = match &state.last_stop {
        Some(StopReason::Breakpoint(at)) => state.breakpoints.iter().position(|b| b.matches(at)),
        _ => None,
    };
    let (stop_bg, stripe_bg) = (theme.stop_bg, theme.stripe_bg);
    Grid::new("breakpoint_list")
        .num_columns(4)
        .with_row_color(move |row, _| match row {
            0 => None, // header
            r if Some(r - 1) == stop_row => Some(stop_bg),
            r if r % 2 == 0 => Some(stripe_bg),
            _ => None,
        })
        .show(ui, |ui| {
            ui.label("");
            ui.label(RichText::new("bank").weak().small());
            ui.label(RichText::new("address").weak().small());
            ui.label("");
            ui.end_row();
            for (i, breakpoint) in state.breakpoints.iter().enumerate() {
                ui.add(Dot::new(Tone::Error));
                let bank = match breakpoint.bank {
                    Some(bank) => RichText::new(format!("{:02X}", bank)).monospace(),
                    _ => RichText::new("any").weak().monospace(),
                };
                ui.label(bank);
                ui.label(
                    RichText::new(format!("${:04X}", breakpoint.address))
                        .color(theme.text_strong)
                        .monospace(),
                );
                let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
                ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
                    ui.set_min_width(ui.available_width());
                    if ui
                        .add(Button::new(egui_phosphor::regular::X).frame(false))
                        .clicked()
                    {
                        if let Some(engine) = engine {
                            engine.send(EngineCommand::Debug(DebugCommand::RemoveBreakpoint(
                                *breakpoint,
                            )));
                        }
                    }
                    match stop_row {
                        Some(row) if i == row => {
                            ui.add(
                                Badge::new(
                                    RichText::new("stopped here")
                                        .small()
                                        .color(theme.highlight_text),
                                )
                                .tone(Tone::Accent),
                            );
                        }
                        _ => {}
                    }
                });
                ui.end_row();
            }
        });
}

fn watchpoint_list(ui: &mut Ui, theme: &Theme, state: &DebugState, engine: Option<&EngineHandle>) {
    if state.watchpoints.is_empty() {
        return;
    }
    ui.add_space(10.0);
    let stop_row = match &state.last_stop {
        Some(StopReason::Watchpoint(at)) => {
            state.watchpoints.iter().position(|(id, _)| at.id == *id)
        }
        _ => None,
    };
    let (stop_bg, stripe_bg) = (theme.stop_bg, theme.stripe_bg);
    Grid::new("watchpoint_list")
        .num_columns(4)
        .with_row_color(move |row, _| match row {
            r if Some(r) == stop_row => Some(stop_bg),
            r if r % 2 == 1 => Some(stripe_bg),
            _ => None,
        })
        .show(ui, |ui| {
            for (i, (id, watchpoint)) in state.watchpoints.iter().enumerate() {
                let bank_label = match watchpoint.bank() {
                    Some(bank) => format!("{}:", bank),
                    _ => "".to_owned(),
                };
                let range = watchpoint.range();
                let end_label = if range.start() != range.end() {
                    format!("-${:04X}", range.end())
                } else {
                    "".to_owned()
                };
                let range_label =
                    RichText::new(format!("{}${:04X}{}", bank_label, range.start(), end_label))
                        .strong()
                        .monospace();
                ui.label(RichText::new(format!("#{}", id.0)));
                ui.label(range_label);

                // cell 3: access tag (reusing Badge as an outlined pill)
                let access_color = theme.access(watchpoint.access());
                ui.add(
                    Badge::new(
                        RichText::new(watchpoint.access().label().text())
                            .color(access_color)
                            .monospace(),
                    )
                    .outline(true)
                    .color(access_color),
                );

                // cell 4: last column, stretches to the right edge
                let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
                ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
                    ui.set_min_width(ui.available_width());
                    if ui
                        .add(Button::new(egui_phosphor::regular::X).frame(false))
                        .clicked()
                    {
                        if let Some(engine) = engine {
                            engine.send(EngineCommand::Debug(DebugCommand::RemoveWatchpoint(*id)));
                        }
                    }
                    if let (Some(row), Some(StopReason::Watchpoint(at))) =
                        (stop_row, state.last_stop.as_ref())
                    {
                        if i == row {
                            ui.add(
                                Badge::new(
                                    RichText::new(format!("{:02X}->{:02X}", at.old, at.new))
                                        .color(theme.highlight_text),
                                )
                                .tone(Tone::Accent),
                            );
                        }
                    }
                });
                ui.end_row();
            }
        });
}

fn cpu_accordion(state: &DebugState, ui: &mut Ui) {
    CollapsingHeader::new(RichText::new("CPU").strong())
        .default_open(true)
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                registers(state, ui);
                ui.add_space(8.0);
                flags(state, ui);
            });
        });
}

fn flags(state: &DebugState, ui: &mut Ui) {
    ui.horizontal(|ui| {
        let flags = state
            .last_regs
            .as_ref()
            .map_or_else(Flags::default, |regs| regs.f);
        ui.label(
            RichText::new("Flags")
                .color(ui.visuals().weak_text_color())
                .monospace(),
        );
        ui.horizontal(|ui| {
            flag(ui, "Z", flags.zero);
            flag(ui, "N", flags.subtract);
            flag(ui, "H", flags.half_carry);
            flag(ui, "C", flags.carry);
        });
        ui.weak(format!("F = {:02X}", u8::from(&flags)));
    });
}

fn registers(state: &DebugState, ui: &mut Ui) {
    ui.columns(2, |cols| {
        Grid::new("regs_left")
            .num_columns(2)
            .show(&mut cols[0], |ui| {
                register(state, ui, Reg16::AF);
                ui.end_row();
                register(state, ui, Reg16::DE);
                ui.end_row();
                register(state, ui, Reg16::SP);
            });

        Grid::new("regs_right")
            .num_columns(2)
            .show(&mut cols[1], |ui| {
                register(state, ui, Reg16::BC);
                ui.end_row();
                register(state, ui, Reg16::HL);
                ui.end_row();
                register(state, ui, Reg16::PC);
            });
    });
}

fn register(state: &DebugState, ui: &mut Ui, reg: Reg16) {
    let color = ui.visuals().text_color();
    let weak_color = ui.visuals().weak_text_color();
    let reg_label: &str = reg.into();
    ui.label(RichText::new(reg_label).monospace().color(weak_color));
    ui.label(
        RichText::new(format!(
            "{:04X}",
            state
                .last_regs
                .as_ref()
                .map_or(0, |regs| regs.read_u16(reg))
        ))
        .monospace()
        .color(color),
    );
}

fn flag(ui: &mut Ui, label: &str, is_set: bool) {
    let theme = Theme::of(ui);
    let side = ui.spacing().interact_size.y;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), Sense::hover());
    let painter = ui.painter();
    let visuals = ui.visuals().noninteractive();
    if is_set {
        painter.rect_filled(rect, visuals.corner_radius, theme.highlight_bg);
    } else {
        painter.rect_stroke(
            rect,
            visuals.corner_radius,
            Stroke::new(visuals.bg_stroke.width, theme.outline),
            StrokeKind::Inside,
        );
    }
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        TextStyle::Monospace.resolve(ui.style()),
        ui.visuals().text_color(),
    );
}

fn parse_bank_addr(input: &str) -> Result<BankAddr, anyhow::Error> {
    let input = input.trim();
    let (bank, addr) = match input.split_once(":") {
        Some((bank, addr)) => (Some(bank), addr),
        None => (None, input),
    };
    Ok(BankAddr {
        bank: bank.map(parse_hex).transpose()?,
        address: parse_hex(addr)?,
    })
}

fn parse_hex(part: &str) -> anyhow::Result<u16> {
    let part = part.trim();

    let digits = part
        .strip_prefix('$')
        .or_else(|| part.strip_prefix("0x"))
        .unwrap_or(part);

    u16::from_str_radix(digits, 16).map_err(|_| anyhow::anyhow!("Not a hex address: {part}"))
}
