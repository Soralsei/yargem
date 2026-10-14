use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::OutputCallbackInfo;
use minifb::{Key, Scale, ScaleMode, Window, WindowOptions};
use yargem::audio::resampler::{LinearResampler, StereoFrame, StereoResampler};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::Duration;

use yargem_lib::input::Button;
use yargem_lib::system::System;
use yargem_lib::{SAMPLE_RATE, SCREEN_H, SCREEN_W};

const DMG: [u32; 4] = [0xFFE0F8D0, 0xFF88C070, 0xFF346856, 0xFF081820];

const KEYMAP: [(Key, Button); 8] = [
    (Key::Z, Button::A),
    (Key::X, Button::B),
    (Key::Backspace, Button::Select),
    (Key::Enter, Button::Start),
    (Key::Right, Button::Right),
    (Key::Left, Button::Left),
    (Key::Up, Button::Up),
    (Key::Down, Button::Down),
];

const FRAME_BURST: usize = 70224 / 64; // source frames per run_frame
const JITTER_MARGIN: usize = FRAME_BURST; // raise on crackles
const FALLBACK_CALLBACK_FRAMES: usize = 1024; // until the first callback reports
const CAPACITY: usize = 8192;

#[derive(Clone)]
struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    #[inline]
    pub fn cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub fn new() -> (Canceller, Self) {
        let this = Self(Arc::new(AtomicBool::new(false)));
        (
            Canceller {
                token: this.clone(),
            },
            this,
        )
    }
}

#[derive(Clone)]
struct Canceller {
    token: CancelToken,
}

impl Canceller {
    #[inline]
    pub fn cancel(&self) {
        self.token.0.store(true, Ordering::Release);
    }
}

fn audio_waterline(callback_frames: usize, device_rate: u32) -> usize {
    let callback_burst =
        (callback_frames as f64 * SAMPLE_RATE as f64 / device_rate as f64).ceil() as usize;
    (callback_burst + FRAME_BURST + JITTER_MARGIN).min(CAPACITY - FRAME_BURST)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let rom = std::fs::read(&args[1]).expect("rom");
    let boot = args.get(2).and_then(|p| std::fs::read(p).ok());

    let (input_tx, input_rx) = mpsc::channel::<(Button, bool)>();
    let (mut frame_in, mut frame_out) = triple_buffer::triple_buffer(&[0u8; SCREEN_W * SCREEN_H]);
    let (mut audio_tx, mut audio_rx) = rtrb::RingBuffer::<[f32; 2]>::new(8192);
    let (canceller, token) = CancelToken::new();

    let mut win = Window::new(
        "yargem",
        SCREEN_W,
        SCREEN_H,
        WindowOptions {
            scale: Scale::X4,
            scale_mode: ScaleMode::AspectRatioStretch,
            resize: true,
            ..Default::default()
        },
    )
    .expect("window creation shouldn't fail");
    win.set_target_fps(60);

    let audio_host = cpal::default_host();
    let audio_device = audio_host
        .default_output_device()
        .expect("No output device");

    let callback_frames: Arc<OnceLock<usize>> = Arc::new(OnceLock::new());

    let stream_config = audio_device
        .default_output_config()
        .expect("Failed to get default stream output config");
    let cb_frames = callback_frames.clone();

    let mut resampler = LinearResampler::new(SAMPLE_RATE, stream_config.sample_rate());

    let stream = audio_device
        .build_output_stream(
            stream_config.into(),
            move |data: &mut [f32], _: &OutputCallbackInfo| {
                let channels = stream_config.channels() as usize;
                let _ = cb_frames.set(data.len() / channels);

                for frame in data.chunks_exact_mut(channels) {
                    let resampled = resampler.resample(|| {
                        audio_rx.pop().ok().map(|s| StereoFrame {
                            left: s[0],
                            right: s[1],
                        })
                    });

                    match frame {
                        [mono] => *mono = (resampled.left + resampled.right) * 0.5,
                        [left, right, ..] => {
                            *left = resampled.left;
                            *right = resampled.right;
                            // buffer is 0.0 initialized by cpal, no need to zero-out remaining
                            // channels
                        }
                        [] => {}
                    }
                }
            },
            move |_err| {},
            None,
        )
        .expect("Failed to create output stream");

    let engine_thread = std::thread::Builder::new()
        .name("engine".into())
        .spawn(move || {
            let mut sys = System::new(boot, rom, false);
            loop {
                if token.cancelled() {
                    break;
                }
                for (b, down) in input_rx.try_iter() {
                    sys.set_button(b, down);
                }
                sys.run_frame();
                frame_in.write(*sys.get_framebuffer());

                sys.drain_audio(audio_tx.slots(), |sample| {
                    audio_tx
                        .push(sample)
                        .expect("Draining Engine audio into rtrb::RingBuffer should never run past the buffer's size");
                });

                let fill = CAPACITY - audio_tx.slots();
                let waterline = audio_waterline(
                    *callback_frames.get().unwrap_or(&FALLBACK_CALLBACK_FRAMES),
                    stream_config.sample_rate(),);
                if fill > waterline {
                    let excess = (fill - waterline) as f64 / SAMPLE_RATE as f64;
                    std::thread::sleep(Duration::from_secs_f64(excess));
                }
            }
        }).expect("failed to spawn engine thread");

    stream.play().expect("Failed to start output stream");

    let mut buf = [0u32; SCREEN_W * SCREEN_H];

    while win.is_open() && !win.is_key_down(Key::Escape) {
        KEYMAP.iter().for_each(|(key, button)| {
            match input_tx.send((*button, win.is_key_down(*key))) {
                Ok(()) => {}
                Err(e) => eprintln!("Failed to send key to engine thread: {}", e),
            }
        });
        let frame = frame_out.read();

        for (px, &i) in buf.iter_mut().zip(frame.iter()) {
            *px = DMG[(i & 3) as usize];
        }
        win.update_with_buffer(&buf, SCREEN_W, SCREEN_H).unwrap();
    }
    canceller.cancel();
    engine_thread.join().expect("Failed to join engine thread");
}
