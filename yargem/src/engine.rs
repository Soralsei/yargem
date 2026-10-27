use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use yargem_lib::{
    debug::protocol::{DebugCommand, DebugEvent},
    input::Button,
    system::{RunResult, Settings, System},
    Frame, SAMPLE_RATE, SCREEN_H, SCREEN_W,
};

use crate::audio::manager::{AudioSink, Sample};

const FALLBACK_PACING: f64 = 70224.0 / 4194304.0;

pub enum EngineCommand {
    Button(Button, bool),
    Debug(DebugCommand),
    AudioSink(Option<AudioSink>),
}

pub struct EngineHandle {
    commands: Option<mpsc::Sender<EngineCommand>>,
    pub events: mpsc::Receiver<DebugEvent>,
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

pub fn spawn_engine(boot: Option<Vec<u8>>, rom: Vec<u8>, settings: Settings) -> EngineHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (ev_tx, ev_rx) = mpsc::channel();
    let (frame_in, frame_out) = triple_buffer::triple_buffer(&[0u8; SCREEN_W * SCREEN_H]);

    let thread = thread::Builder::new()
        .name("engine".into())
        .spawn(move || {
            Engine {
                // built inside the thread: the Rcs never cross
                sys: System::new(boot, rom, settings),
                commands: cmd_rx,
                events: ev_tx,
                frames: frame_in,
                audio: None, // arrives as AudioSink once a device is up
            }
            .run()
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
    events: mpsc::Sender<DebugEvent>,
    frames: triple_buffer::Input<Frame>,
    audio: Option<AudioSink>,
}

impl Engine {
    fn run(mut self) {
        loop {
            loop {
                match self.commands.try_recv() {
                    Ok(cmd) => self.handle(cmd),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            }
            if self.sys.paused() {
                // Blocks at zero CPU; the handle's drop disconnects and wakes it.
                match self.commands.recv() {
                    Ok(cmd) => self.handle(cmd),
                    Err(_) => return,
                }
                continue;
            }
            match self.sys.run_frame() {
                RunResult::Frame => self.frames.write(*self.sys.get_framebuffer()),
                // Nothing can happen without outside action: wait for it.
                RunResult::Idle => match self.commands.recv() {
                    Ok(cmd) => self.handle(cmd),
                    Err(_) => return,
                },
                RunResult::Stopped(reason) => {
                    // partial frame, drawn up to the current LY
                    self.frames.write(*self.sys.get_framebuffer());
                    let pc = self.sys.pc();
                    let _ = self.events.send(DebugEvent::Paused { reason, pc });
                }
            }
            self.pace();
        }
    }

    fn handle(&mut self, cmd: EngineCommand) {
        match cmd {
            EngineCommand::Button(button, down) => self.sys.set_button(button, down),
            EngineCommand::Debug(cmd) => {
                if let Some(event) = self.sys.debug(cmd) {
                    let _ = self.events.send(event);
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
