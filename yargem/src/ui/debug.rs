use yargem_common::Reg16;
use yargem_lib::debug::protocol::{
    AccessKind, BankAddr, BankRange, DebugCommand, Flags, StopReason, Watchpoint,
};

use crate::{
    app::DebugState,
    engine::{EngineCommand, EngineHandle},
    ui::{
        components::{
            badge::{badge, BadgeType},
            indicator::dot,
        },
        style,
        theme::Theme,
    },
};

pub fn controls_bar(ui: &mut egui::Ui, engine: Option<&EngineHandle>, state: &DebugState) {
    let Some(e) = engine else { return };
    ui.horizontal(|ui| {
        ctrl_button(
            ui,
            format!("{} Pause", egui_phosphor::regular::PAUSE),
            !state.paused,
            egui::Key::F6,
            || {
                e.send(EngineCommand::Debug(DebugCommand::Pause));
            },
        );
        ctrl_button(
            ui,
            format!("{} Resume", egui_phosphor::regular::PLAY),
            state.paused,
            egui::Key::F5,
            || {
                e.send(EngineCommand::Debug(DebugCommand::Resume));
            },
        );
        ctrl_button(
            ui,
            format!("{} Step", egui_phosphor::regular::ARROW_LINE_DOWN),
            state.paused,
            egui::Key::F11,
            || {
                e.send(EngineCommand::Debug(DebugCommand::StepInstruction));
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !state.paused {
                ui.label(egui::RichText::new("Running").color(Theme::of(ui).text_weak));
                ui.add(dot(Theme::of(ui).running));
                return;
            }
            if let (Some(regs), Some(reason)) = (state.last_regs.as_ref(), state.last_stop.as_ref())
            {
                ui.horizontal(|ui| {
                    let label = match reason {
                        StopReason::Breakpoint(bank_addr) => format!(
                            "Paused - breakpoint {}:${:04X}",
                            bank_addr
                                .bank
                                .map_or("any".to_owned(), |b| "$".to_owned() + &b.to_string()),
                            bank_addr.address
                        ),
                        StopReason::Watchpoint(hit) => format!(
                            "Paused - watchpoint #{} {}, address {}:${:04X}: {} -> {}",
                            hit.id.0,
                            match hit.access {
                                AccessKind::Read => "read",
                                AccessKind::Write => "write",
                            },
                            hit.bank
                                .map_or("any".to_owned(), |b| "$".to_owned() + &b.to_string()),
                            hit.addr,
                            hit.old,
                            hit.new
                        ),
                        StopReason::Step | StopReason::UserPause => {
                            format!("Paused - step at ${:04X}", regs.pc)
                        }
                    };
                    ui.label(label);
                    ui.add(dot(Theme::of(ui).stop(reason)));
                });
            }
        });
    });
}

fn ctrl_button<'a>(
    ui: &mut egui::Ui,
    atoms: impl egui::IntoAtoms<'a>,
    enabled: bool,
    shortcut_key: egui::Key,
    on_click: impl Fn(),
) {
    if ui
        .add_enabled(
            enabled,
            egui::Button::new(atoms).shortcut_text(shortcut_key.symbol_or_name()),
        )
        .clicked()
        || ui.input_mut(|input| {
            input.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::NONE,
                shortcut_key,
            ))
        })
    {
        (on_click)();
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

    pub fn show(&mut self, ui: &mut egui::Ui, engine: Option<&EngineHandle>, state: &DebugState) {
        egui::Panel::right("dbg_panel")
            .default_size(400.0)
            .min_size(300.0)
            .show(ui, |ui| {
                egui::Panel::top("dbg_cpu").show(ui, |ui| {
                    cpu_accordion(state, ui);
                    ui.add_space(style::SECTION_GAP);
                });
                egui::Panel::bottom("dbg_lists").show(ui, |ui| {
                    self.break_watch_lists(state, ui, engine);
                });
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::CollapsingHeader::new("Disassembly")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.label("Planned");
                        });
                });
            });
    }

    fn break_watch_lists(
        &mut self,
        state: &DebugState,
        ui: &mut egui::Ui,
        engine: Option<&EngineHandle>,
    ) {
        let theme = Theme::of(ui);
        let fill = ui.visuals().code_bg_color;
        ui.add_space(style::SECTION_GAP);
        let id = ui.make_persistent_id("dbg_breakpoints");
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
            .show_header(ui, |ui| {
                ui.strong("Breakpoints");
                badge(
                    ui,
                    egui::RichText::new(&state.breakpoints.len().to_string()).monospace(),
                    BadgeType::Filled(fill),
                );
            })
            .body_unindented(|ui| {
                ui.vertical_centered(|ui| {
                    self.breakpoint_input(engine, &theme, ui);
                    breakpoint_list(ui, &theme, state, engine)
                });
            });

        ui.add_space(style::SECTION_GAP);

        let id = ui.make_persistent_id("dbg_watchpoints");
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
            .show_header(ui, |ui| {
                ui.strong("Watchpoints");
                badge(
                    ui,
                    egui::RichText::new(&state.watchpoints.len().to_string()).monospace(),
                    BadgeType::Filled(fill),
                );
            })
            .body_unindented(|ui| {
                ui.vertical_centered(|ui| {
                    self.watchpoint_input(ui, &theme, engine);
                    watchpoint_list(ui, &theme, state, engine);
                });
            });
    }

    fn breakpoint_input(
        &mut self,
        engine: Option<&EngineHandle>,
        theme: &Theme,
        ui: &mut egui::Ui,
    ) {
        ui.vertical(|ui| {
            let input_left = ui
                .horizontal(|ui| {
                    ui.add_space(ui.spacing().indent);
                    let input_label = ui.weak("Break at");

                    if self.bp_err.is_some() {
                        ui.visuals_mut().selection.stroke.color = theme.error;
                    }
                    let resp = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.bp_input)
                                .desired_width(150.0)
                                .font(egui::TextStyle::Monospace)
                                .hint_text("$200 or <bank>:$4000"),
                        )
                        .labelled_by(input_label.id);
                    if resp.changed() {
                        self.bp_err = None;
                    }

                    let entered = resp.lost_focus() && ui.input(|i| i.key_down(egui::Key::Enter));
                    let add = ui.button("Add");

                    if add.clicked() || entered {
                        self.add_breakpoint(engine);
                        resp.request_focus();
                    }

                    resp.rect.left()
                })
                .inner;

            if let Some(err) = &self.bp_err {
                ui.horizontal(|ui| {
                    ui.add_space(input_left - ui.cursor().left());
                    ui.label(egui::RichText::new(err.to_string()).color(theme.error))
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

    fn watchpoint_input(
        &mut self,
        ui: &mut egui::Ui,
        theme: &Theme,
        engine: Option<&EngineHandle>,
    ) {
        ui.vertical(|ui| {
            let input_left = ui
                .horizontal(|ui| {
                    ui.add_space(ui.spacing().indent);
                    let input_label = ui.weak("Watch");

                    if self.wp_err.is_some() {
                        ui.visuals_mut().selection.stroke.color = theme.error;
                    }
                    let resp_start = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.watch_start)
                                .desired_width(60.0)
                                .font(egui::TextStyle::Monospace)
                                .hint_text("$C000"),
                        )
                        .labelled_by(input_label.id);

                    // ui.weak(egui::RichText::new("\u{2014}"));
                    ui.weak("-");

                    let resp_end = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.watch_end)
                                .desired_width(60.0)
                                .font(egui::TextStyle::Monospace)
                                .hint_text("end"),
                        )
                        .labelled_by(input_label.id);

                    if resp_start.changed() || resp_start.changed() {
                        self.wp_err = None;
                    }

                    let mut resp_lost = resp_start.lost_focus().then_some(&resp_start);
                    if resp_end.lost_focus() {
                        resp_lost = Some(&resp_end);
                    };

                    let entered = resp_lost.is_some() && ui.input(|i| i.key_down(egui::Key::Enter));
                    let add = ui.button("Add");
                    if add.clicked() || entered {
                        self.add_watchpoint(engine);
                        resp_lost.map(|resp| resp.request_focus());
                    }
                    resp_start.rect.left()
                })
                .inner;

            if let Some(err) = &self.wp_err {
                ui.horizontal(|ui| {
                    ui.add_space(input_left - ui.cursor().left());
                    ui.label(egui::RichText::new(err.to_string()).color(theme.error))
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

fn breakpoint_list(
    ui: &mut egui::Ui,
    theme: &Theme,
    state: &DebugState,
    engine: Option<&EngineHandle>,
) {
    if state.breakpoints.is_empty() {
        return;
    }
    let stop_row = match &state.last_stop {
        Some(StopReason::Breakpoint(at)) => state.breakpoints.iter().position(|b| b.matches(at)),
        _ => None,
    };
    let (stop_bg, stripe_bg) = (theme.stop_bg, theme.stripe_bg);
    egui::Grid::new("breakpoint_list")
        .num_columns(4)
        .with_row_color(move |row, _| match row {
            0 => None, // header
            r if Some(r - 1) == stop_row => Some(stop_bg),
            r if r % 2 == 0 => Some(stripe_bg),
            _ => None,
        })
        .show(ui, |ui| {
            ui.label("");
            ui.label(egui::RichText::new("bank").weak().small());
            ui.label(egui::RichText::new("address").weak().small());
            ui.end_row();
            for (i, breakpoint) in state.breakpoints.iter().enumerate() {
                ui.add(dot(theme.breakpoint));
                let bank = match breakpoint.bank {
                    Some(bank) => egui::RichText::new(format!("{:02X}", bank)).monospace(),
                    _ => egui::RichText::new("any").weak().monospace(),
                };
                ui.label(bank);
                ui.label(
                    egui::RichText::new(format!("${:04X}", breakpoint.address))
                        .color(theme.text_strong)
                        .monospace(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.set_min_width(ui.available_width());
                    if ui
                        .add(egui::Button::new(egui_phosphor::regular::X).frame(false))
                        .clicked()
                    {
                        if let Some(engine) = engine {
                            engine.send(EngineCommand::Debug(DebugCommand::RemoveBreakpoint(
                                *breakpoint,
                            )));
                        }
                    }
                    match stop_row {
                        Some(row) if i == row => badge(
                            ui,
                            egui::RichText::new("stopped here")
                                .small()
                                .color(theme.highlight_text),
                            BadgeType::Filled(theme.highlight_bg),
                        ),
                        _ => {}
                    }
                });
                ui.end_row();
            }
        });
}

fn watchpoint_list(
    ui: &mut egui::Ui,
    theme: &Theme,
    state: &DebugState,
    engine: Option<&EngineHandle>,
) {
}

fn cpu_accordion(state: &DebugState, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new(egui::RichText::new("CPU").strong())
        .default_open(true)
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                registers(state, ui);
                flags(state, ui);
            });
        });
}

fn flags(state: &DebugState, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let flags = state
            .last_regs
            .as_ref()
            .map_or_else(Flags::default, |regs| regs.f);
        ui.label(
            egui::RichText::new("Flags")
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

fn registers(state: &DebugState, ui: &mut egui::Ui) {
    ui.columns(2, |cols| {
        egui::Grid::new("regs_left")
            .num_columns(2)
            .show(&mut cols[0], |ui| {
                register(state, ui, Reg16::AF);
                ui.end_row();
                register(state, ui, Reg16::DE);
                ui.end_row();
                register(state, ui, Reg16::SP);
            });

        egui::Grid::new("regs_right")
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

fn register(state: &DebugState, ui: &mut egui::Ui, reg: Reg16) {
    let color = ui.visuals().text_color();
    let weak_color = ui.visuals().weak_text_color();
    let reg_label: &str = reg.into();
    ui.label(egui::RichText::new(reg_label).monospace().color(weak_color));
    ui.label(
        egui::RichText::new(format!(
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

fn flag(ui: &mut egui::Ui, label: &str, is_set: bool) {
    let theme = Theme::of(ui);
    let side = ui.spacing().interact_size.y;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    let painter = ui.painter();
    let visuals = ui.visuals().noninteractive();
    if is_set {
        painter.rect_filled(rect, visuals.corner_radius, theme.highlight_bg);
    } else {
        painter.rect_stroke(
            rect,
            visuals.corner_radius,
            egui::Stroke::new(visuals.bg_stroke.width, theme.outline),
            egui::StrokeKind::Inside,
        );
    }
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::TextStyle::Monospace.resolve(ui.style()),
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
