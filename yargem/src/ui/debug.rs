use yargem_common::Reg16;
use yargem_lib::debug::protocol::{AccessKind, DebugCommand, Flags, StopReason};

use crate::{
    app::DebugState,
    engine::EngineHandle,
    ui::{components::indicator::dot, theme::Theme},
};

pub fn controls_bar(ui: &mut egui::Ui, engine: Option<&EngineHandle>, state: &mut DebugState) {
    let Some(e) = engine else { return };
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!state.paused, egui::Button::new("Pause"))
            .clicked()
        {
            e.send(crate::engine::EngineCommand::Debug(DebugCommand::Pause));
        }
        if ui
            .add_enabled(state.paused, egui::Button::new("Resume"))
            .clicked()
        {
            e.send(crate::engine::EngineCommand::Debug(DebugCommand::Resume));
            state.paused = false;
        }
        if ui
            .add_enabled(state.paused, egui::Button::new("Step"))
            .clicked()
        {
            e.send(crate::engine::EngineCommand::Debug(
                DebugCommand::StepInstruction,
            ));
        }
        if let (Some(regs), Some(reason)) = (state.last_regs.as_ref(), state.last_stop.as_ref()) {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.horizontal(|ui| {
                    let label = match reason {
                        StopReason::Breakpoint(bank_addr) => format!(
                            "Hit breakpoint at bank {}, address 0x{:04X}",
                            bank_addr.bank.unwrap_or(0),
                            bank_addr.address
                        ),
                        StopReason::Watchpoint(hit) => format!(
                            "Watchpoint hit at bank {}, address 0x{:04X}: {} -> {}",
                            hit.bank.unwrap_or(0),
                            hit.addr,
                            hit.old,
                            hit.new
                        ),
                        StopReason::Step | StopReason::UserPause => {
                            format!("Paused at pc 0x{:04X}", regs.pc)
                        }
                    };
                    ui.label(label);
                    ui.add(dot(Theme::of(ui).stop(reason)));
                });
            });
        }
    });
}

#[derive(Default)]
pub struct DebugPanel {
    pub open: bool,
    bp_input: String,
    watch_start: String,
    watch_end: String,
    watch_access: AccessKind,
    error: String, // ex: bad address: $XYZ
}

impl DebugPanel {
    pub fn new() -> Self {
        Self {
            watch_access: AccessKind::Read,
            ..Default::default()
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, _engine: Option<&EngineHandle>, state: &DebugState) {
        egui::Panel::right("dbg_panel").show(ui, |ui| {
            ui.vertical_centered(|ui| {
                egui::CollapsingHeader::new(egui::RichText::new("CPU").strong().size(14.0))
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            registers(state, ui);
                            flags(state, ui);
                        });
                    });

                egui::CollapsingHeader::new("Dissassembly")
                    .default_open(false)
                    .show(ui, |_ui| {});

                egui::CollapsingHeader::new("Breakpoints")
                    .default_open(true)
                    .show(ui, |_ui| {});

                egui::CollapsingHeader::new("Watchpoints")
                    .default_open(true)
                    .show(ui, |_ui| {});
            });
        });
    }
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
        ui.add_space(15.0);
        ui.horizontal(|ui| {
            flag(ui, "Z", flags.zero);
            flag(ui, "N", flags.subtract);
            flag(ui, "H", flags.half_carry);
            flag(ui, "C", flags.carry);
        });
        ui.add_space(15.0);
        ui.label(egui::RichText::new(format!("F = {:02X}", u8::from(&flags))));
    });
}

fn registers(state: &DebugState, ui: &mut egui::Ui) {
    ui.columns(2, |cols| {
        egui::Grid::new("regs_left")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(&mut cols[0], |ui| {
                register(state, ui, Reg16::AF);
                ui.end_row();
                register(state, ui, Reg16::DE);
                ui.end_row();
                register(state, ui, Reg16::SP);
            });

        egui::Grid::new("regs_right")
            .num_columns(2)
            .spacing([10.0, 6.0])
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
    let side = ui.text_style_height(&egui::TextStyle::Body) + 10.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    let painter = ui.painter();
    if is_set {
        painter.rect_filled(rect, 2.0, theme.highlight_bg);
    } else {
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.0, theme.outline),
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
