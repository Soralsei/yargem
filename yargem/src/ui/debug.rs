use yargem_lib::debug::protocol::{DebugCommand, StopReason};

use crate::{app::DebugState, engine::EngineHandle};

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
        if let Some(pc) = state.last_pc {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("Paused at ${:04X}", pc));
                    if let Some(StopReason::Breakpoint(_addr)) = state.last_stop { /* TODO */ }
                });
            });
        }
    });
}

pub struct DebugPanel {
    open: bool,
}
