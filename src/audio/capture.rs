// SPDX-License-Identifier: GPL-3.0-or-later
//! cpal microphone capture: downmix to mono, resample to 48 kHz, gate, Opus.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use opus::{Application, Channels, Encoder};

use super::MicFrame;
use crate::settings::MODE_PUSH_TO_TALK;
use crate::state;

const FRAME_SAMPLES: usize = 960;
const MAX_OPUS: usize = 1275;
const HANGOVER: Duration = Duration::from_millis(250);
const RING_LIMIT: usize = 48_000;

pub fn start() {
    let spawned = thread::Builder::new().name("svc-mic".to_owned()).spawn(run);
    if let Err(error) = spawned {
        log::error!("svc_client: could not start the microphone thread: {error}");
    }
}

fn run() {
    let host = cpal::default_host();
    let Some(device) = host.default_input_device() else {
        log::warn!("svc_client: no default input device; push-to-talk will send nothing");
        return;
    };
    let config = match device.default_input_config() {
        Ok(config) => config,
        Err(error) => {
            log::warn!("svc_client: no default input config: {error}");
            return;
        }
    };
    let rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let format = config.sample_format();
    log::info!(
        "svc_client: input device {:?}, {rate} Hz, {channels} ch, {format:?}",
        device.name().unwrap_or_default()
    );

    let ring: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::with_capacity(16_384)));
    let err_fn = |error: cpal::StreamError| log::warn!("svc_client: input stream error: {error}");

    let stream_config = config.config();
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &stream_config, Arc::clone(&ring), err_fn),
        SampleFormat::I16 => build::<i16>(&device, &stream_config, Arc::clone(&ring), err_fn),
        SampleFormat::I32 => build::<i32>(&device, &stream_config, Arc::clone(&ring), err_fn),
        SampleFormat::U16 => build::<u16>(&device, &stream_config, Arc::clone(&ring), err_fn),
        other => {
            log::warn!("svc_client: unsupported input sample format {other:?}");
            return;
        }
    };
    let stream = match stream {
        Ok(stream) => stream,
        Err(error) => {
            log::warn!("svc_client: could not build the input stream: {error}");
            return;
        }
    };
    if let Err(error) = stream.play() {
        log::warn!("svc_client: could not start microphone capture: {error}");
        return;
    }

    let controls = state::controls();
    let mut encoder_codec = controls.codec();
    let mut encoder = match Encoder::new(48_000, Channels::Mono, opus_application(encoder_codec)) {
        Ok(encoder) => encoder,
        Err(error) => {
            log::error!("svc_client: could not create the Opus encoder: {error}");
            return;
        }
    };

    let own_mic = state::own_mic();
    let mut resampler = MonoResampler::new(f64::from(rate), f64::from(48_000));
    let mut samples: Vec<f32> = Vec::with_capacity(FRAME_SAMPLES * 2);
    let mut sequence = 0i64;
    let mut sending = false;
    let mut hangover_until = Instant::now();

    loop {
        thread::sleep(Duration::from_millis(10));

        if controls.codec() != encoder_codec {
            match Encoder::new(48_000, Channels::Mono, opus_application(controls.codec())) {
                Ok(next) => {
                    encoder = next;
                    encoder_codec = controls.codec();
                    sending = false;
                }
                Err(error) => log::warn!("svc_client: could not change the Opus mode: {error}"),
            }
        }

        let raw = {
            let mut ring = ring.lock().unwrap_or_else(|e| e.into_inner());
            if ring.len() > RING_LIMIT {
                let drop = ring.len() - RING_LIMIT;
                ring.drain(0..drop);
            }
            ring.drain(..).collect::<Vec<_>>()
        };
        if raw.is_empty() {
            continue;
        }
        samples.extend(resampler.push(&raw));

        while samples.len() >= FRAME_SAMPLES {
            let frame: Vec<f32> = samples.drain(..FRAME_SAMPLES).collect();
            let peak = frame.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
            let level_db = if peak <= f32::EPSILON {
                -127.0
            } else {
                20.0 * peak.log10()
            };

            let voice = level_db > controls.threshold_db();
            if voice {
                hangover_until = Instant::now() + HANGOVER;
            }
            let gate = controls.enabled()
                && !controls.muted()
                && if controls.mode() == MODE_PUSH_TO_TALK {
                    controls.push_to_talk()
                } else {
                    voice || Instant::now() < hangover_until
                };

            let sender = crate::session::mic_sender();
            if !gate {
                own_mic.store(false, std::sync::atomic::Ordering::Relaxed);
                if sending {
                    if let Some(sender) = &sender {
                        let _ = sender.send(MicFrame {
                            data: Vec::new(),
                            sequence,
                            whispering: false,
                        });
                        sequence = sequence.wrapping_add(1);
                    }
                    let _ = encoder.reset_state();
                    sending = false;
                }
                continue;
            }

            let Some(sender) = sender else {
                own_mic.store(false, std::sync::atomic::Ordering::Relaxed);
                if sending {
                    let _ = encoder.reset_state();
                    sending = false;
                }
                continue;
            };

            let gain = controls.mic_gain();
            let pcm: Vec<i16> = frame
                .iter()
                .map(|sample| {
                    let value = (sample * gain * 32767.0).clamp(-32768.0, 32767.0);
                    value as i16
                })
                .collect();
            let mut encoded = [0u8; MAX_OPUS];
            match encoder.encode(&pcm, &mut encoded[..controls.max_payload()]) {
                Ok(len) => {
                    if sender
                        .send(MicFrame {
                            data: encoded[..len].to_vec(),
                            sequence,
                            whispering: false,
                        })
                        .is_ok()
                    {
                        sequence = sequence.wrapping_add(1);
                        sending = true;
                        own_mic.store(true, std::sync::atomic::Ordering::Relaxed);
                    } else {
                        own_mic.store(false, std::sync::atomic::Ordering::Relaxed);
                        let _ = encoder.reset_state();
                        sending = false;
                    }
                }
                Err(error) => {
                    log::debug!("svc_client: Opus encode failed: {error}");
                    own_mic.store(false, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
    }
}

/// Build a stream that downmixes every callback block to mono `f32`.
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    ring: Arc<Mutex<VecDeque<f32>>>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample + ToMono,
{
    let channels = config.channels as usize;
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let mut ring = ring.lock().unwrap_or_else(|e| e.into_inner());
            for frame in data.chunks(channels.max(1)) {
                let mut sum = 0.0f32;
                for sample in frame {
                    sum += sample.to_mono();
                }
                ring.push_back(sum / frame.len().max(1) as f32);
            }
        },
        err_fn,
        None,
    )
}

trait ToMono: Copy {
    fn to_mono(self) -> f32;
}

impl ToMono for f32 {
    fn to_mono(self) -> f32 {
        self.clamp(-1.0, 1.0)
    }
}

impl ToMono for i16 {
    fn to_mono(self) -> f32 {
        f32::from(self) / 32768.0
    }
}

impl ToMono for i32 {
    fn to_mono(self) -> f32 {
        self as f32 / 2_147_483_648.0
    }
}

impl ToMono for u16 {
    fn to_mono(self) -> f32 {
        (f32::from(self) - 32768.0) / 32768.0
    }
}

/// Linear interpolation from the device rate to 48 kHz.
struct MonoResampler {
    buffer: Vec<f32>,
    position: f64,
    ratio: f64,
}

impl MonoResampler {
    fn new(input_rate: f64, output_rate: f64) -> Self {
        Self {
            buffer: Vec::new(),
            position: 0.0,
            ratio: input_rate / output_rate,
        }
    }

    fn push(&mut self, input: &[f32]) -> Vec<f32> {
        if (self.ratio - 1.0).abs() < f64::EPSILON {
            return input.to_vec();
        }
        self.buffer.extend_from_slice(input);
        let mut out = Vec::with_capacity(
            ((self.buffer.len() as f64 / self.ratio) as usize).saturating_add(2),
        );
        while (self.position as usize + 1) < self.buffer.len() {
            let index = self.position as usize;
            let frac = (self.position - index as f64) as f32;
            out.push(lerp(self.buffer[index], self.buffer[index + 1], frac));
            self.position += self.ratio;
        }
        let consumed = self.position as usize;
        if consumed > 0 {
            let drop = consumed.min(self.buffer.len());
            self.buffer.drain(0..drop);
            self.position -= drop as f64;
        }
        out
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn opus_application(codec: u8) -> Application {
    match codec {
        1 => Application::Audio,
        2 => Application::LowDelay,
        _ => Application::Voip,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_keeps_the_sample_count_close() {
        let mut resampler = MonoResampler::new(44_100.0, 48_000.0);
        let input = vec![0.25f32; 441];
        let out = resampler.push(&input);
        assert!((out.len() as i32 - 480).abs() <= 2, "{}", out.len());
    }

    #[test]
    fn resampler_bypasses_48k() {
        let mut resampler = MonoResampler::new(48_000.0, 48_000.0);
        let input = vec![0.5f32; 960];
        assert_eq!(resampler.push(&input), input);
    }

    #[test]
    fn downmix_converts_common_formats() {
        assert!((0i16.to_mono()).abs() < f32::EPSILON);
        assert!((32767i16.to_mono() - 0.999_97).abs() < 1e-4);
        assert!((u16::MAX.to_mono() - 0.999_97).abs() < 1e-4);
    }
}
