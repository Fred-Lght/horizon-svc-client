// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings and Controls bindings owned by the mod, plus the atomics the audio
//! threads read without touching the game.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, Ordering};

use mod_api::{ClientHost, KeyConflictContext, KeyRegistration, Registrar, SettingSpec};

pub const MOD_ID: &str = "svc_client";

pub const KEY_PUSH_TO_TALK: &str = "push_to_talk";
pub const KEY_MUTE: &str = "mute";

pub const MODE_PUSH_TO_TALK: u8 = 0;

/// Register the page in Options > Mods and the two Controls bindings.
pub fn register(r: &mut dyn Registrar) {
    for setting in [
        SettingSpec::toggle("enabled", "svc_client.setting.enabled", true),
        SettingSpec::choice(
            "mode",
            "svc_client.setting.mode",
            [
                "svc_client.mode.push_to_talk",
                "svc_client.mode.voice_activation",
            ],
            MODE_PUSH_TO_TALK as usize,
        ),
        SettingSpec::range(
            "activation_threshold_db",
            "svc_client.setting.activation_threshold_db",
            -80.0,
            0.0,
            1.0,
            -40.0,
        ),
        SettingSpec::range(
            "mic_gain",
            "svc_client.setting.mic_gain",
            0.1,
            3.0,
            0.1,
            1.0,
        ),
        SettingSpec::range("volume", "svc_client.setting.volume", 0.0, 1.0, 0.05, 1.0),
        SettingSpec::toggle("hud", "svc_client.setting.hud", true),
    ] {
        if let Err(error) = r.register_setting(setting) {
            log::warn!("svc_client: setting refused: {error}");
        }
    }

    let mut push_to_talk = KeyRegistration::new(KEY_PUSH_TO_TALK, "key.keyboard.caps.lock")
        .context(KeyConflictContext::InGame);
    push_to_talk.label = "svc_client.key.push_to_talk".to_owned();
    push_to_talk.category = "svc_client.key.category".to_owned();
    r.register_key_mapping(push_to_talk);

    let mut mute =
        KeyRegistration::new(KEY_MUTE, "key.keyboard.j").context(KeyConflictContext::InGame);
    mute.label = "svc_client.key.mute".to_owned();
    mute.category = "svc_client.key.category".to_owned();
    r.register_key_mapping(mute);
}

/// Values shared with the capture and playback threads.
pub struct Controls {
    pub enabled: AtomicBool,
    pub hud: AtomicBool,
    pub mode: AtomicU8,
    pub threshold_db: AtomicU32,
    pub mic_gain: AtomicU32,
    pub volume: AtomicU32,
    pub push_to_talk: AtomicBool,
    pub muted: AtomicBool,
    pub codec: AtomicU8,
    pub max_payload: AtomicU16,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(true),
            hud: AtomicBool::new(true),
            mode: AtomicU8::new(MODE_PUSH_TO_TALK),
            threshold_db: AtomicU32::new((-40.0f32).to_bits()),
            mic_gain: AtomicU32::new(1.0f32.to_bits()),
            volume: AtomicU32::new(1.0f32.to_bits()),
            push_to_talk: AtomicBool::new(false),
            muted: AtomicBool::new(false),
            codec: AtomicU8::new(0),
            max_payload: AtomicU16::new(1275),
        }
    }
}

impl Controls {
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn hud_enabled(&self) -> bool {
        self.hud.load(Ordering::Relaxed)
    }

    pub fn mode(&self) -> u8 {
        self.mode.load(Ordering::Relaxed)
    }

    pub fn threshold_db(&self) -> f32 {
        f32::from_bits(self.threshold_db.load(Ordering::Relaxed))
    }

    pub fn mic_gain(&self) -> f32 {
        f32::from_bits(self.mic_gain.load(Ordering::Relaxed))
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    pub fn push_to_talk(&self) -> bool {
        self.push_to_talk.load(Ordering::Relaxed)
    }

    pub fn muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    pub fn codec(&self) -> u8 {
        self.codec.load(Ordering::Relaxed)
    }

    pub fn set_codec(&self, codec: u8) {
        self.codec.store(codec, Ordering::Relaxed);
    }

    pub fn max_payload(&self) -> usize {
        usize::from(self.max_payload.load(Ordering::Relaxed)).clamp(512, 1275)
    }

    pub fn set_max_payload(&self, mtu: u16) {
        self.max_payload.store(mtu, Ordering::Relaxed);
    }

    pub fn set_push_to_talk(&self, down: bool) {
        self.push_to_talk.store(down, Ordering::Relaxed);
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }
}

/// Read the player's current values once per client tick.
pub fn refresh(host: &dyn ClientHost, controls: &Controls) {
    let enabled = host.setting_bool("enabled").unwrap_or(true);
    let hud = host.setting_bool("hud").unwrap_or(true);
    let mode = host.setting_choice("mode").unwrap_or(0).min(1) as u8;
    let threshold = host
        .setting_f64("activation_threshold_db")
        .unwrap_or(-40.0)
        .clamp(-80.0, 0.0) as f32;
    let mic_gain = host.setting_f64("mic_gain").unwrap_or(1.0).clamp(0.1, 3.0) as f32;
    let volume = host.setting_f64("volume").unwrap_or(1.0).clamp(0.0, 1.0) as f32;

    controls.enabled.store(enabled, Ordering::Relaxed);
    controls.hud.store(hud, Ordering::Relaxed);
    controls.mode.store(mode, Ordering::Relaxed);
    controls
        .threshold_db
        .store(threshold.to_bits(), Ordering::Relaxed);
    controls
        .mic_gain
        .store(mic_gain.to_bits(), Ordering::Relaxed);
    controls.volume.store(volume.to_bits(), Ordering::Relaxed);
}
