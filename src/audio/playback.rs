// SPDX-License-Identifier: GPL-3.0-or-later
//! cpal output stream: pulls 48 kHz stereo from the mixer and resamples to the
//! device's rate.

use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SampleRate, SizedSample};

use super::mixer::{self, Mixer};

const MIX_RATE: f64 = 48_000.0;

pub fn start() {
    let spawned = thread::Builder::new()
        .name("svc-output".to_owned())
        .spawn(run);
    if let Err(error) = spawned {
        log::error!("svc_client: could not start the output thread: {error}");
    }
}

fn run() {
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        log::warn!("svc_client: no default output device; voice playback is off");
        return;
    };
    let config = match device.default_output_config() {
        Ok(config) => config,
        Err(error) => {
            log::warn!("svc_client: no default output config: {error}");
            return;
        }
    };

    let rate = config.sample_rate();
    let channels = config.channels() as usize;
    let format = config.sample_format();
    log::info!(
        "svc_client: output device {:?}, {rate} Hz, {channels} ch, {format:?}",
        device
            .description()
            .map(|description| description.name().to_owned())
            .unwrap_or_default()
    );

    let state = Arc::new(Mutex::new(OutputState::new(
        mixer::mixer(),
        f64::from(MIX_RATE) / f64::from(rate),
    )));

    let err_fn = |error: cpal::Error| log::warn!("svc_client: output stream error: {error}");
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, config.into(), state, err_fn),
        SampleFormat::I16 => build::<i16>(&device, config.into(), state, err_fn),
        SampleFormat::I32 => build::<i32>(&device, config.into(), state, err_fn),
        SampleFormat::U16 => build::<u16>(&device, config.into(), state, err_fn),
        other => {
            log::warn!("svc_client: unsupported output sample format {other:?}");
            return;
        }
    };

    let stream = match stream {
        Ok(stream) => stream,
        Err(error) => {
            log::warn!("svc_client: could not build the output stream: {error}");
            return;
        }
    };
    if let Err(error) = stream.play() {
        log::warn!("svc_client: could not start output: {error}");
        return;
    }

    // The stream owns the callback; keep this thread (and the stream) alive for
    // the life of the game.
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    state: Arc<Mutex<OutputState>>,
    err_fn: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
            state.write(data, channels);
        },
        err_fn,
        None,
    )
}

struct OutputState {
    mixer: Arc<Mutex<Mixer>>,
    /// Interleaved 48 kHz stereo frames not yet consumed.
    scratch: Vec<f32>,
    /// Fractional read position in `scratch`, in frames.
    position: f64,
    /// Input frames consumed per output frame (48 kHz / device rate).
    ratio: f64,
}

impl OutputState {
    fn new(mixer: Arc<Mutex<Mixer>>, ratio: f64) -> Self {
        Self {
            mixer,
            scratch: Vec::new(),
            position: 0.0,
            ratio,
        }
    }

    fn write<T>(&mut self, data: &mut [T], channels: usize)
    where
        T: SizedSample + FromSample<f32>,
    {
        if channels == 0 {
            return;
        }
        let frames = data.len() / channels;
        if frames == 0 {
            return;
        }

        let needed = (frames as f64 * self.ratio).ceil() as usize + 2;
        while self.scratch.len() / 2 < needed {
            let mut block = [0.0f32; 960 * 2];
            self.mixer
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .render(&mut block);
            self.scratch.extend_from_slice(&block);
        }

        for frame in 0..frames {
            let index = self.position.floor() as usize;
            let frac = (self.position - index as f64) as f32;
            let left = lerp(self.scratch[index * 2], self.scratch[(index + 1) * 2], frac);
            let right = lerp(
                self.scratch[index * 2 + 1],
                self.scratch[(index + 1) * 2 + 1],
                frac,
            );
            self.position += self.ratio;

            let base = frame * channels;
            if channels == 1 {
                data[base] = T::from_sample((left + right) * 0.5);
            } else {
                data[base] = T::from_sample(left);
                data[base + 1] = T::from_sample(right);
                for extra in 2..channels {
                    data[base + extra] = T::from_sample(0.0);
                }
            }
        }

        let consumed = self.position.floor() as usize;
        if consumed > 0 {
            let samples = (consumed * 2).min(self.scratch.len());
            self.scratch.drain(0..samples);
            self.position -= consumed as f64;
        }
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[allow(dead_code)]
fn _sample_rate_is_u32(rate: SampleRate) -> u32 {
    rate
}
