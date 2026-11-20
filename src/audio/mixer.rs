// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-sender Opus decoders, jitter queues and the stereo mix.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use opus::{Channels, Decoder};

use super::spatial;
use crate::state::Listener;

const SAMPLE_RATE: u32 = 48_000;
const FRAME_SAMPLES: usize = 960;
const MAX_BUFFERED: usize = SAMPLE_RATE as usize * 2;
const SOURCE_IDLE: Duration = Duration::from_secs(2);
const MAX_SOURCES: usize = 64;

struct Source {
    decoder: Decoder,
    samples: VecDeque<i16>,
    position: Option<[f64; 3]>,
    /// A location or group sound carries its own position; a player sound follows
    /// the entity and is refreshed by the client tick.
    fixed_position: bool,
    max_distance: f32,
    last_packet: Instant,
    last_sequence: Option<i64>,
}

impl Source {
    fn new() -> Option<Self> {
        match Decoder::new(SAMPLE_RATE, Channels::Mono) {
            Ok(decoder) => Some(Self {
                decoder,
                samples: VecDeque::with_capacity(FRAME_SAMPLES * 4),
                position: None,
                fixed_position: false,
                max_distance: 48.0,
                last_packet: Instant::now(),
                last_sequence: None,
            }),
            Err(error) => {
                log::error!("svc_client: could not create an Opus decoder: {error}");
                None
            }
        }
    }
}

pub struct Mixer {
    listener: Listener,
    volume: f32,
    sources: HashMap<u128, Source>,
}

impl Mixer {
    fn new() -> Self {
        Self {
            listener: Listener::default(),
            volume: 1.0,
            sources: HashMap::new(),
        }
    }

    pub fn set_listener(&mut self, listener: Listener) {
        self.listener = listener;
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    pub fn set_player_position(&mut self, sender: u128, position: [f64; 3]) {
        if let Some(source) = self.sources.get_mut(&sender)
            && !source.fixed_position
        {
            source.position = Some(position);
        }
    }

    pub fn active_senders(&self) -> Vec<u128> {
        self.sources.keys().copied().collect()
    }

    fn push(
        &mut self,
        sender: u128,
        sequence: i64,
        data: &[u8],
        position: Option<([f64; 3], f32)>,
        group: bool,
    ) {
        if data.is_empty() {
            return;
        }
        if !self.sources.contains_key(&sender) {
            if self.sources.len() >= MAX_SOURCES {
                log::debug!("svc_client: too many voice sources, dropping {sender}");
                return;
            }
            let Some(source) = Source::new() else {
                return;
            };
            self.sources.insert(sender, source);
        }
        let source = self.sources.get_mut(&sender).expect("source was inserted");
        source.last_packet = Instant::now();

        if let Some(last) = source.last_sequence {
            if sequence <= last {
                return;
            }
            let gap = (sequence - last).clamp(1, 3) as usize;
            if gap > 1 {
                // A small concealment: silence for at most two missing frames.
                for _ in 0..(gap - 1) * FRAME_SAMPLES {
                    source.samples.push_back(0);
                }
            }
        }
        source.last_sequence = Some(sequence);

        let mut pcm = [0i16; FRAME_SAMPLES];
        match source.decoder.decode(data, &mut pcm, false) {
            Ok(samples) => {
                for sample in &pcm[..samples.min(FRAME_SAMPLES)] {
                    source.samples.push_back(*sample);
                }
            }
            Err(error) => {
                log::debug!("svc_client: Opus decode failed: {error}");
                return;
            }
        }

        match position {
            Some((at, distance)) => {
                source.position = Some(at);
                source.max_distance = if distance.is_finite() && distance > 0.0 {
                    distance
                } else {
                    48.0
                };
                source.fixed_position = true;
            }
            None if group => {
                source.position = None;
                source.fixed_position = true;
            }
            None => {
                source.fixed_position = false;
            }
        }

        while source.samples.len() > MAX_BUFFERED {
            source.samples.pop_front();
        }
    }

    /// Mix `frames` stereo frames into `out` (`frames * 2` samples).
    pub fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let Some(frames) = out.len().checked_div(2) else {
            return;
        };
        if frames == 0 {
            return;
        }

        for source in self.sources.values_mut() {
            let gains = match source.position {
                Some(position) => {
                    let distance = distance(self.listener.position, position);
                    if distance > f64::from(source.max_distance) + 1.0 {
                        continue;
                    }
                    let gain = spatial::distance_gain(distance, source.max_distance) * self.volume;
                    let (left, right) =
                        spatial::gains(self.listener, position, source.max_distance);
                    (gain * left, gain * right)
                }
                None => (self.volume, self.volume),
            };

            for frame in 0..frames {
                let Some(sample) = source.samples.pop_front() else {
                    break;
                };
                let sample = f32::from(sample) / 32768.0;
                out[frame * 2] += sample * gains.0;
                out[frame * 2 + 1] += sample * gains.1;
            }
        }

        for sample in out.iter_mut() {
            *sample = sample.clamp(-1.0, 1.0);
        }

        self.sources.retain(|_, source| {
            !source.samples.is_empty() || source.last_packet.elapsed() < SOURCE_IDLE
        });
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

static MIXER: OnceLock<Arc<Mutex<Mixer>>> = OnceLock::new();

pub fn mixer() -> Arc<Mutex<Mixer>> {
    MIXER
        .get_or_init(|| Arc::new(Mutex::new(Mixer::new())))
        .clone()
}

/// Called by the UDP thread for every sound packet.
pub fn push_encoded(
    sender: u128,
    sequence: i64,
    data: &[u8],
    position: Option<([f64; 3], f32)>,
    group: bool,
) {
    let mixer = mixer();
    let mut mixer = mixer.lock().unwrap_or_else(|e| e.into_inner());
    mixer.push(sender, sequence, data, position, group);
}

pub fn set_listener(listener: Listener) {
    let mixer = mixer();
    mixer
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_listener(listener);
}

pub fn set_volume(volume: f32) {
    let mixer = mixer();
    mixer
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_volume(volume);
}

pub fn set_player_position(sender: u128, position: [f64; 3]) {
    let mixer = mixer();
    mixer
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_player_position(sender, position);
}

pub fn active_senders() -> Vec<u128> {
    let mixer = mixer();
    mixer
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .active_senders()
}

pub fn clear() {
    let mixer = mixer();
    mixer
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .sources
        .clear();
}
