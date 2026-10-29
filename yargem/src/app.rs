#![allow(dead_code, unused_variables)]
use std::fs;

use egui::Key;
use egui_file_dialog::FileDialog;
use yargem_lib::{
    debug::protocol::{DebugCommand, DebugEvent, RegisterSnapshot, StopReason},
    input::Button,
    system::Settings,
};

use crate::{
    audio::manager::AudioManager,
    engine::{spawn_engine, EngineCommand, EngineHandle},
    ui::{debug::controls_bar, Screen},
};

// TODO extract input handling to `input` module
const KEYMAP: [(Key, Button); 8] = [
    (Key::Z, Button::A),
    (Key::X, Button::B),
    (Key::Backspace, Button::Select),
    (Key::Enter, Button::Start),
    (Key::ArrowRight, Button::Right),
    (Key::ArrowLeft, Button::Left),
    (Key::ArrowUp, Button::Up),
    (Key::ArrowDown, Button::Down),
];

#[derive(Default)]
pub struct DebugState {
    pub paused: bool,
    pub last_stop: Option<StopReason>,
    pub last_pc: Option<u16>,
    pub last_regs: Option<RegisterSnapshot>,
}

pub struct App {
    audio: AudioManager,
    file_dialog: FileDialog,
    screen: Screen,

    engine: Option<EngineHandle>,
    debug_state: Option<DebugState>,

    show_debugger: bool,
    show_memory: bool,
}

impl App {
    pub fn new(ctx: &eframe::CreationContext) -> Self {
        Self {
            audio: AudioManager::new(),
            screen: Screen::new(&ctx.egui_ctx),
            file_dialog: FileDialog::default(),
            engine: None,
            debug_state: None,
            show_debugger: false,
            show_memory: false,
        }
    }

    fn top_panel_menu(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                self.file_menu(ui);
                self.debug_menu(ui);
            });
        });
    }

    fn debug_menu(&mut self, ui: &mut egui::Ui) {
        ui.add_enabled_ui(self.engine.is_some(), |ui| {
            ui.menu_button("Debug", |ui| {
                if ui
                    .add_enabled(self.debug_state.is_none(), egui::Button::new("Attach"))
                    .clicked()
                {
                    self.engine
                        .as_ref()
                        .expect("engine is not none")
                        .send(EngineCommand::Debug(DebugCommand::Attach));
                    self.debug_state = Some(DebugState::default());
                }
                if ui
                    .add_enabled(
                        self.debug_state.as_ref().is_some(),
                        egui::Button::new("Detach"),
                    )
                    .clicked()
                {
                    self.engine
                        .as_ref()
                        .expect("engine is not none")
                        .send(EngineCommand::Debug(DebugCommand::Detach));
                    self.debug_state = None;
                }
                ui.separator();
                if ui
                    .add_enabled(
                        self.debug_state.as_ref().is_some_and(|state| !state.paused),
                        egui::Button::new("Pause"),
                    )
                    .clicked()
                {
                    self.engine
                        .as_ref()
                        .expect("engine is not none")
                        .send(EngineCommand::Debug(DebugCommand::Pause));
                }
                if ui
                    .add_enabled(
                        self.debug_state.as_ref().is_some_and(|state| state.paused),
                        egui::Button::new("Resume"),
                    )
                    .clicked()
                {
                    self.engine
                        .as_ref()
                        .expect("engine is not none")
                        .send(EngineCommand::Debug(DebugCommand::Resume));
                    self.debug_state
                        .as_mut()
                        .expect("debug state is not None")
                        .paused = false;
                }
            })
        });
    }

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("File", |ui| {
            if ui.button("Open ROM").clicked() {
                self.file_dialog.pick_file();
            }

            if ui
                .add_enabled(self.engine.is_some(), egui::Button::new("Close ROM"))
                .clicked()
            {
                self.engine.take();
            }

            ui.separator();

            if ui.button("Quit").clicked() {
                ui.send_viewport_cmd(egui::ViewportCommand::Close)
            }
        });
    }

    fn debugger_panel(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        egui::Panel::right("debugger").show(ui, |ui| {});
    }

    fn memory_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("Memory")
            .open(&mut self.show_memory)
            .show(ctx, |ui| {
                // TODO
            });
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let Some(engine) = &self.engine else { return };
        while let Ok(ev) = engine.events.try_recv() {
            match ev {
                DebugEvent::Paused { reason, pc } => {
                    if let Some(state) = &mut self.debug_state {
                        println!("paused");
                        state.paused = true;
                        state.last_pc = Some(pc);
                        state.last_stop = Some(reason);
                    }
                }
                DebugEvent::Memory { start, bytes } => {}
                DebugEvent::Registers(register_snapshot) => {
                    if let Some(state) = &mut self.debug_state {
                        state.last_regs = Some(register_snapshot);
                    }
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.top_panel_menu(ui, frame);
        if self.show_debugger {
            self.debugger_panel(ui, frame);
        }
        if self.show_memory {
            self.memory_window(ui);
        }
        if let Some(debug_state) = &mut self.debug_state {
            egui::Panel::top("controls").show(ui, |ui| {
                controls_bar(ui, self.engine.as_ref(), debug_state);
            });
        }

        self.file_dialog.update(ui);

        if let Some(path) = self.file_dialog.take_picked() {
            match fs::read(path) {
                Ok(rom) => {
                    let ctx = ui.clone();
                    let engine = spawn_engine(
                        None,
                        rom,
                        Settings::default(),
                        Box::new(move || ctx.request_repaint()),
                    );

                    match self.audio.open(self.audio.default_device()) {
                        Ok(sink) => engine.send(EngineCommand::AudioSink(sink)),
                        Err(e) => eprintln!("failed to open audio device : {}", e),
                    }
                    self.engine = Some(engine);
                }
                Err(e) => {}
            }
        }

        if !ui.ctx().egui_wants_keyboard_input() {
            // TODO implement generic input manager and mapping
            // for now, just hardcode the mapping
            if let Some(engine) = self.engine.as_mut() {
                KEYMAP.iter().for_each(|(key, button)| {
                    engine.send(EngineCommand::Button(
                        *button,
                        ui.input(|i| i.key_down(*key)),
                    ));
                });
            }
        }

        egui::CentralPanel::default().show(ui, |ui| {
            self.screen.ui(ui, self.engine.as_mut());
        });
    }
}
