use yargem_common::Reg16;
use yargem_lib::debug::protocol::{AccessKind, DebugCommand, StopReason};

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
        if let (Some(pc), Some(reason)) = (state.last_pc, state.last_stop.as_ref()) {
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
                            format!("Paused at pc 0x{:04X}", pc)
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
                egui::CollapsingHeader::new("CPU")
                    .default_open(true)
                    .show(ui, |ui| {
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
                    });

                egui::CollapsingHeader::new("Dissassembly")
                    .default_open(true)
                    .show(ui, |ui| {});

                egui::CollapsingHeader::new("Breakpoints")
                    .default_open(true)
                    .show(ui, |ui| {});

                egui::CollapsingHeader::new("Watchpoints")
                    .default_open(true)
                    .show(ui, |ui| {});
            });
        });
    }
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
