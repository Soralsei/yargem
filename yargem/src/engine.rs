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

pub type Sample = [f32; 2];

pub enum EngineCommand {
    Button(Button, bool),
    Debug(DebugCommand),
    /// Fresh ring after an output device (re)start. `None`: no device.
    AudioSink(Option<rtrb::Producer<Sample>>),
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

pub fn spawn_engine(
    boot: Option<Vec<u8>>,
    rom: Vec<u8>,
    settings: Settings,
    waterline: Arc<AtomicUsize>,
) -> EngineHandle {
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
                waterline,
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
    audio: Option<rtrb::Producer<Sample>>,
    waterline: Arc<AtomicUsize>,
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
        let Some(audio) = &mut self.audio else {
            // No device: nothing drains samples, so pace by wall clock instead.
            return;
        };
        self.sys.drain_audio(audio.slots(), |sample| {
            let _ = audio.push(sample); // max = slots(), cannot overflow
        });
        let fill = audio.buffer().capacity() - audio.slots();
        let waterline = self.waterline.load(Ordering::Relaxed);
        if fill > waterline {
            let excess = (fill - waterline) as f64 / SAMPLE_RATE as f64;
            thread::sleep(Duration::from_secs_f64(excess));
        }
    }
}
