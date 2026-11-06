#![allow(dead_code, unused_variables)]
use std::fs;

use egui::Key;
use egui_file_dialog::FileDialog;
use yargem_lib::{
    debug::protocol::{
        BankAddr, DebugCommand, DebugEvent, RegisterSnapshot, StopReason, WatchId, Watchpoint,
    },
    input::Button,
    system::Settings,
};

use crate::{
    audio::manager::AudioManager,
    engine::{spawn_engine, EngineCommand, EngineHandle},
    ui::{
        debug::{controls_bar, DebugPanel},
        style, Screen,
    },
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
    pub attached: bool,
    pub last_stop: Option<StopReason>,
    pub last_regs: Option<RegisterSnapshot>,
    pub watchpoints: Vec<(WatchId, Watchpoint)>,
    pub breakpoints: Vec<BankAddr>,
}

impl DebugState {
    pub fn new(
        regs: RegisterSnapshot,
        breaks: Vec<BankAddr>,
        watches: Vec<(WatchId, Watchpoint)>,
    ) -> Self {
        Self {
            last_regs: Some(regs),
            watchpoints: watches,
            breakpoints: breaks,
            ..Default::default()
        }
    }
}

pub struct App {
    audio: AudioManager,

    file_dialog: FileDialog,
    screen: Screen,
    debug_panel: DebugPanel,

    engine: Option<EngineHandle>,
    debug_state: Option<DebugState>,
}

impl App {
    pub fn new(ctx: &eframe::CreationContext) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

        ctx.egui_ctx.set_fonts(fonts);
        style::install(&ctx.egui_ctx);

        Self {
            audio: AudioManager::new(),
            screen: Screen::new(&ctx.egui_ctx),
            file_dialog: FileDialog::default(),
            debug_panel: DebugPanel::new(),
            engine: None,
            debug_state: None,
        }
    }

    fn top_panel_menu(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                self.file_menu(ui, frame);
                self.debug_menu(ui);
            });
        });
    }

    fn debug_menu(&mut self, ui: &mut egui::Ui) {
        ui.add_enabled_ui(self.engine.is_some(), |ui| {
            ui.menu_button("Debug", |ui| {
                self.attach_menu(ui);
                self.detach_menu(ui);

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
                }
            })
        });
    }

    fn detach_menu(&mut self, ui: &mut egui::Ui) {
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
        }
    }

    fn attach_menu(&mut self, ui: &mut egui::Ui) {
        if ui
            .add_enabled(self.debug_state.is_none(), egui::Button::new("Attach"))
            .clicked()
        {
            self.engine
                .as_ref()
                .expect("engine is not none")
                .send(EngineCommand::Debug(DebugCommand::Attach));
        }
    }

    fn file_menu(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.menu_button("File", |ui| {
            if ui.button("Open ROM").clicked() {
                self.file_dialog.pick_file();
            }

            if ui
                .add_enabled(self.engine.is_some(), egui::Button::new("Close ROM"))
                .clicked()
            {
                self.engine = None;
                self.debug_state = None;
            }

            ui.separator();

            if ui.button("Quit").clicked() {
                ui.send_viewport_cmd(egui::ViewportCommand::Close)
            }
        });
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let Some(engine) = &self.engine else { return };
        while let Ok(ev) = engine.events.try_recv() {
            match ev {
                DebugEvent::Paused { reason, regs } => {
                    if let Some(state) = &mut self.debug_state {
                        state.paused = true;
                        state.last_regs = Some(regs);
                        state.last_stop = Some(reason);
                    }
                }
                DebugEvent::Resumed => {
                    if let Some(state) = &mut self.debug_state {
                        state.paused = false;
                    }
                }
                DebugEvent::Memory { start, bytes } => {}
                DebugEvent::Registers(register_snapshot) => {
                    if let Some(state) = &mut self.debug_state {
                        state.last_regs = Some(register_snapshot);
                    }
                }
                DebugEvent::Attached {
                    regs,
                    breaks,
                    watches,
                } => {
                    match &mut self.debug_state {
                        Some(state) => {
                            state.last_regs = Some(regs);
                            state.breakpoints = breaks;
                            state.watchpoints = watches;
                        }
                        None => self.debug_state = Some(DebugState::new(regs, breaks, watches)),
                    }
                    self.debug_panel.open = true;
                }
                DebugEvent::Detached => {
                    self.debug_state = None;
                    self.debug_panel.open = false;
                }
                DebugEvent::Watchpoints(watches) => {
                    if let Some(state) = &mut self.debug_state {
                        state.watchpoints = watches;
                    }
                }
                DebugEvent::Breakpoints(breakpoints) => {
                    if let Some(state) = &mut self.debug_state {
                        state.breakpoints = breakpoints;
                    }
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.top_panel_menu(ui, frame);

        if let Some(debug_state) = &mut self.debug_state {
            egui::Panel::top("controls").show(ui, |ui| {
                controls_bar(ui, self.engine.as_ref(), debug_state);
            });
            self.debug_panel.show(ui, self.engine.as_ref(), debug_state);
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
