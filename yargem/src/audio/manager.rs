use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::Context;
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, Host, Stream, StreamConfig,
};
use yargem_lib::SAMPLE_RATE;

use crate::audio::resampler::{LinearResampler, StereoFrame, StereoResampler};

pub type Sample = [f32; 2];

const FRAME_BURST: usize = 70224 / 64; // source frames per run_frame
const JITTER_MARGIN: usize = FRAME_BURST; // raise on crackles
const FALLBACK_CALLBACK_FRAMES: usize = 1024; // until the first callback reports
const CAPACITY: usize = 8192;

pub struct AudioSink {
    samples: rtrb::Producer<Sample>,
    waterline: Arc<AtomicUsize>,
}

impl AudioSink {
    /// How far the ring is ahead of the waterline, as playback time.
    pub fn overfill(&self) -> Duration {
        let fill = self.samples.buffer().capacity() - self.samples.slots();
        let excess = fill.saturating_sub(self.waterline.load(Ordering::Relaxed));
        Duration::from_secs_f64(excess as f64 / SAMPLE_RATE as f64)
    }

    pub fn push(&mut self, sample: Sample) {
        let _ = self.samples.push(sample);
    }

    pub fn slots(&self) -> usize {
        self.samples.slots()
    }

    pub fn available(&self) -> bool {
        !self.samples.is_abandoned()
    }
}

fn audio_waterline(callback_frames: usize, device_rate: u32) -> usize {
    let callback_burst =
        (callback_frames as f64 * SAMPLE_RATE as f64 / device_rate as f64).ceil() as usize;
    (callback_burst + FRAME_BURST + JITTER_MARGIN).min(CAPACITY - FRAME_BURST)
}

pub struct AudioManager {
    host: Host,
    out_stream: Option<Stream>,
    failed: Arc<AtomicBool>,
}

impl AudioManager {
    pub fn new() -> Self {
        let host = cpal::default_host();
        Self {
            host,
            out_stream: None,
            failed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn default_device(&self) -> Option<Device> {
        self.host.default_output_device()
    }

    /// Set from the audio thread when the stream dies (unplugged device...).
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    pub fn devices(&self) -> Result<cpal::Devices, cpal::Error> {
        self.host.devices()
    }

    pub fn open(&mut self, device: Option<Device>) -> anyhow::Result<Option<AudioSink>> {
        self.out_stream = None; // dropping stops it, and its consumer goes with it
        self.failed = Arc::new(AtomicBool::new(false));
        let Some(device) = device else {
            return Ok(None);
        };

        let supported = device.default_output_config()?;
        let channels = supported.channels() as usize;
        let rate = supported.sample_rate();

        let (producer, mut consumer) = rtrb::RingBuffer::<Sample>::new(CAPACITY);
        let waterline = Arc::new(AtomicUsize::new(audio_waterline(
            FALLBACK_CALLBACK_FRAMES,
            rate,
        )));
        let cb_waterline = waterline.clone();
        let failed = self.failed.clone();
        let mut resampler = LinearResampler::new(SAMPLE_RATE, rate);

        let stream = device
            .build_output_stream(
                supported.into(),
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    cb_waterline.fetch_max(
                        audio_waterline(data.len() / channels, rate),
                        Ordering::Relaxed,
                    );
                    for frame in data.chunks_exact_mut(channels) {
                        let s = resampler.resample(|| {
                            consumer
                                .pop()
                                .ok()
                                .map(|[left, right]| StereoFrame { left, right })
                        });
                        match frame {
                            [mono] => *mono = (s.left + s.right) * 0.5,
                            // cpal zero-fills the buffer, extra channels stay silent
                            [left, right, ..] => (*left, *right) = (s.left, s.right),
                            [] => {}
                        }
                    }
                },
                move |_err| failed.store(true, Ordering::Relaxed),
                None,
            )
            .context("Failed to build output stream")?;
        stream.play().context("Failed to start output stream")?;
        self.out_stream = Some(stream);
        Ok(Some(AudioSink {
            samples: producer,
            waterline,
        }))
    }
}
