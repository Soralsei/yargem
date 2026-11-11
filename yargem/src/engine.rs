use std::{
    fs, io,
    path::PathBuf,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use yargem_lib::{
    debug::protocol::{DebugCommand, DebugEvent},
    input::Button,
    system::{RunResult, Settings, System},
    Frame, SCREEN_H, SCREEN_W,
};

use crate::audio::manager::AudioSink;

const FALLBACK_PACING: f64 = 70224.0 / 4194304.0;

pub enum EngineCommand {
    Button(Button, bool),
    Debug(DebugCommand),
    AudioSink(Option<AudioSink>),
}

pub enum EngineEvent {
    Debug(DebugEvent),
    Error(anyhow::Error),
}

pub struct EngineHandle {
    commands: Option<mpsc::Sender<EngineCommand>>,
    pub events: mpsc::Receiver<EngineEvent>,
    pub frames: triple_buffer::Output<Frame>,
    thread: Option<JoinHandle<()>>,
}

impl EngineHandle {
    pub fn send(&self, cmd: EngineCommand) {
        if let Some(commands) = &self.commands {
            // Err only if the engine thread died; join in Drop surfaces the panic
            let _ = commands.send(cmd);
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        // Disconnecting the channel is the quit signal.
        self.commands = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn spawn_engine(
    save_path: PathBuf,
    boot: Option<Vec<u8>>,
    rom: Vec<u8>,
    settings: Settings,
    request_repaint: Box<dyn Fn() + Send>,
) -> EngineHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (ev_tx, ev_rx) = mpsc::channel();
    let (frame_in, frame_out) = triple_buffer::triple_buffer(&[0; SCREEN_W * SCREEN_H]);

    let thread = thread::Builder::new()
        .name("engine".into())
        .spawn(move || {
            let boot_copy = boot.clone();
            let sys = match System::new(boot_copy, rom.clone(), settings) {
                Ok(s) => s,
                Err(err) => {
                    let _ = ev_tx.send(EngineEvent::Error(
                        anyhow::Error::new(err).context("Failed to load boot rom"),
                    ));
                    System::new(None, rom, settings)
                        .expect("System::new cannot fail without boot rom")
                }
            };
            let engine = Engine {
                // built inside the thread: the Rcs never cross
                sys,
                commands: cmd_rx,
                events: ev_tx,
                frames: frame_in,
                audio: None, // arrives as AudioSink once a device is up
                request_repaint,
                save_path,
            };
            engine.run();
        })
        .expect("failed to spawn engine thread");

    EngineHandle {
        commands: Some(cmd_tx),
        events: ev_rx,
        frames: frame_out,
        thread: Some(thread),
    }
}

struct Engine {
    sys: System,

    commands: mpsc::Receiver<EngineCommand>,
    events: mpsc::Sender<EngineEvent>,
    frames: triple_buffer::Input<Frame>,

    audio: Option<AudioSink>,
    request_repaint: Box<dyn Fn() + Send>,

    save_path: PathBuf,
}

impl Engine {
    fn run(mut self) {
        // Load save file if it exists
        match fs::read(&self.save_path) {
            Ok(save_data) => match self.sys.load_battery_ram(&save_data) {
                Ok(_) => {}
                Err(e) => {
                    let _ = self.events.send(EngineEvent::Error(
                        anyhow::Error::new(e).context("Failed to load save file"),
                    ));
                }
            },
            Err(e) if !matches!(e.kind(), io::ErrorKind::NotFound) => {
                let _ = self.events.send(EngineEvent::Error(
                    anyhow::Error::new(e).context("Failed to load save file"),
                ));
            }
            Err(_) => {}
        }
        while self.drain() {
            // TODO periodically call save_ram if take_ram_dirty
            match self.sys.run_frame() {
                RunResult::Frame => self.frames.write(*self.sys.get_framebuffer()),
                // Nothing can happen without outside action: wait for it.
                RunResult::Idle | RunResult::Paused => {
                    match self.commands.recv() {
                        Ok(cmd) => self.handle(cmd),
                        Err(_) => break,
                    }
                    (self.request_repaint)();
                    continue;
                }
                RunResult::Stopped(reason) => {
                    // partial frame, drawn up to the current LY
                    self.frames.write(*self.sys.get_framebuffer());
                    let regs = self.sys.registers();
                    let _ = self
                        .events
                        .send(EngineEvent::Debug(DebugEvent::Paused { reason, regs }));
                }
            }
            (self.request_repaint)();
            self.pace();
        }

        if self.sys.take_ram_dirty() {
            if let Some(save_data) = self.sys.battery_ram() {
                self.save_ram(save_data);
            }
        }
    }

    fn save_ram(&self, save_data: Vec<u8>) {
        // TODO smarter path handling for saves
        let tmp_path = self.save_path.with_added_extension("tmp");

        let res =
            fs::write(&tmp_path, save_data).and_then(|()| fs::rename(&tmp_path, &self.save_path));

        match res {
            Ok(_) => println!("Saved game to '{}'", self.save_path.display()),
            Err(e) => {
                eprintln!("Failed to save game: {e}");
                // Best effort cleanup
                let _ = fs::remove_file(tmp_path);
            }
        }
    }

    /// returns false on broken commands channel
    fn drain(&mut self) -> bool {
        loop {
            match self.commands.try_recv() {
                Ok(cmd) => self.handle(cmd),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return false,
            }
        }
        true
    }

    fn handle(&mut self, cmd: EngineCommand) {
        match cmd {
            EngineCommand::Button(button, down) => self.sys.set_button(button, down),
            EngineCommand::Debug(cmd) => {
                if let Some(event) = self.sys.debug(cmd) {
                    let _ = self.events.send(EngineEvent::Debug(event));
                }
            }
            EngineCommand::AudioSink(sink) => self.audio = sink,
        }
    }

    fn pace(&mut self) {
        let Some(sink) = &mut self.audio else {
            thread::sleep(Duration::from_secs_f64(FALLBACK_PACING));
            return;
        };
        if !sink.available() {
            self.audio = None; // stream gone; main sends a new sink
            return;
        }
        self.sys.drain_audio(sink.slots(), |sample| {
            sink.push(sample); // max = slots(), cannot overflow
        });
        let lag = sink.overfill();
        if !lag.is_zero() {
            thread::sleep(lag);
        }
    }
}
