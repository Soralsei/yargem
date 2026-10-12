use std::sync::Arc;

use anyhow::Context;
use cpal::{
    traits::{DeviceTrait, HostTrait},
    Device, Host, Stream, StreamConfig,
};
use egui::mutex::Mutex;
use yargem_lib::SAMPLE_RATE;

const FRAME_BURST: usize = 70224 / 64; // source frames per run_frame
const JITTER_MARGIN: usize = FRAME_BURST; // raise on crackles
const FALLBACK_CALLBACK_FRAMES: usize = 1024; // until the first callback reports
const CAPACITY: usize = 8192;

pub struct AudioManager {
    host: Host,
    out_device: Arc<Mutex<Option<Device>>>,
    out_stream: Option<Stream>,
}

impl AudioManager {
    pub fn new() -> Self {
        let host = cpal::default_host();
        let out_device = host.default_output_device();
        Self {
            host,
            out_device: Arc::new(Mutex::new(out_device)),
            out_stream: None,
        }
    }

    pub fn devices(&self) -> Result<cpal::Devices, cpal::Error> {
        self.host.devices()
    }

    pub fn set_out_device(&mut self, device: Option<cpal::Device>) {
        self.out_stream = None;
        let mut out_device = self.out_device.lock();
        *out_device = device;
    }

    fn audio_waterline(callback_frames: usize, device_rate: u32) -> usize {
        let callback_burst =
            ((callback_frames * SAMPLE_RATE as usize) as f64 / device_rate as f64).ceil() as usize;
        (callback_burst + FRAME_BURST + JITTER_MARGIN).min(CAPACITY - FRAME_BURST)
    }

    pub fn start_stream(
        &mut self,
        config: StreamConfig,
        error_cb: impl FnMut(cpal::Error) + std::marker::Send + 'static,
    ) -> anyhow::Result<()> {
        if let Some(device) = &*self.out_device.lock() {
            self.out_stream = Some(
                device
                    .build_output_stream(
                        config,
                        move |_data: &mut [f32], _: &cpal::OutputCallbackInfo| {},
                        error_cb,
                        None,
                    )
                    .context("Failed to initialize out stream")?,
            );
        }
        Ok(())
    }
}
