// SPDX-License-Identifier: GPL-3.0-or-later
//! Process-wide state shared by the game hooks and the voice threads.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::protocol::buf::Writer;
use crate::protocol::tcp;
use crate::settings::{self, Controls, MOD_ID};
use mod_api::{ClientHost, EntityRef, Identifier, SendPacketError, ServerboundPlayPacket};

pub const SPEAKING_HOLD: Duration = Duration::from_millis(700);
const REQUEST_RETRY: Duration = Duration::from_secs(2);
const REQUEST_ATTEMPTS: u8 = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Idle,
    RequestingSecret,
    Authenticating,
    Connected,
    Failed(String),
    Disconnected(String),
}

/// The non-secret half of `voicechat:secret`, for the HUD and logs.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct SecretInfo {
    pub player: u128,
    pub host: String,
    pub port: u16,
    pub mtu: u16,
    pub distance: f64,
    pub keep_alive_ms: u32,
    pub groups: bool,
    pub allow_recording: bool,
}

/// The listener pose used by the 3D mixer.
#[derive(Clone, Copy, Debug)]
pub struct Listener {
    pub position: [f64; 3],
    pub yaw: f32,
}

impl Default for Listener {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            yaw: 0.0,
        }
    }
}

/// Entity ids are assigned by the server; the public API has no client-side
/// UUID lookup (`entity_by_uuid` is server-only and `player(uuid)` only knows the
/// local/LAN player). We find speaking players by walking a bounded id range once
/// per tick while they are missing, then cache the handle.
pub struct Locator {
    known: HashMap<u128, (EntityRef, String)>,
    missing: HashMap<u128, String>,
    next_id: u32,
    retry_at: Instant,
}

const LOCATOR_MAX_ID: u32 = 65_535;
const LOCATOR_CHUNK: u32 = 4_096;

impl Locator {
    fn new() -> Self {
        Self {
            known: HashMap::new(),
            missing: HashMap::new(),
            next_id: 0,
            retry_at: Instant::now(),
        }
    }

    fn reset(&mut self) {
        self.known.clear();
        self.missing.clear();
        self.next_id = 0;
        self.retry_at = Instant::now();
    }

    fn entity_for(
        &mut self,
        uuid: u128,
        name: Option<String>,
        host: &dyn ClientHost,
    ) -> Option<EntityRef> {
        if let Some((entity, known_name)) = self.known.get(&uuid) {
            if Some(known_name.as_str()) == entity_name(*entity, host).as_deref() {
                return Some(*entity);
            }
            self.known.remove(&uuid);
        }
        if let Some(name) = name {
            self.missing.insert(uuid, name);
        }
        None
    }

    fn scan(&mut self, host: &dyn ClientHost) {
        if self.missing.is_empty() || Instant::now() < self.retry_at {
            return;
        }
        let wanted: HashMap<String, u128> = self
            .missing
            .iter()
            .map(|(uuid, name)| (name.clone(), *uuid))
            .collect();
        let end = self
            .next_id
            .saturating_add(LOCATOR_CHUNK)
            .min(LOCATOR_MAX_ID + 1);
        for id in self.next_id..end {
            let Some(entity) = host.entity(id) else {
                continue;
            };
            let Some(name) = entity_name(entity, host) else {
                continue;
            };
            if let Some(uuid) = wanted.get(&name).copied() {
                self.missing.remove(&uuid);
                self.known.insert(uuid, (entity, name));
            }
        }
        self.next_id = if end > LOCATOR_MAX_ID { 0 } else { end };
        if self.next_id == 0 {
            // A full pass found nobody. Do not rescan on every sound packet: a
            // player with a custom name can never be matched this way.
            self.missing.clear();
            self.retry_at = Instant::now() + Duration::from_secs(5);
        }
    }
}

fn entity_name(entity: EntityRef, host: &dyn ClientHost) -> Option<String> {
    entity.name(host)
}

thread_local! {
    /// Only the client tick touches this; keeping it off the shared mutex avoids
    /// holding a lock across a scan of game entity handles.
    static LOCATOR: RefCell<Locator> = RefCell::new(Locator::new());
}

pub struct Shared {
    pub controls: Arc<Controls>,
    pub status: Status,
    pub info: Option<SecretInfo>,
    pub want_secret: bool,
    pub request_attempts: u8,
    pub last_request: Option<Instant>,
    pub session_active: bool,
    pub stop: Option<Arc<AtomicBool>>,
    pub generation: u64,
    pub speaking: HashMap<u128, Instant>,
    pub listener: Listener,
    pub own_mic: Arc<AtomicBool>,
    pub last_enabled: bool,
}

impl Shared {
    fn new() -> Self {
        Self {
            controls: Arc::new(Controls::default()),
            status: Status::Idle,
            info: None,
            want_secret: false,
            request_attempts: 0,
            last_request: None,
            session_active: false,
            stop: None,
            generation: 0,
            speaking: HashMap::new(),
            listener: Listener::default(),
            own_mic: Arc::new(AtomicBool::new(false)),
            last_enabled: true,
        }
    }
}

static SHARED: OnceLock<Mutex<Shared>> = OnceLock::new();

pub fn shared() -> &'static Mutex<Shared> {
    SHARED.get_or_init(|| Mutex::new(Shared::new()))
}

/// Send one plugin message as a vanilla `minecraft:custom_payload`.
///
/// Horizon's raw `play_raw` declarations are a server-side policy: on a client
/// `send_payload_to_server` refuses them with `WrongDirection`. The engine's own
/// test sends `voicechat:request_secret` through this packet path instead.
pub fn send_custom(
    host: &mut dyn ClientHost,
    channel: &str,
    data: &[u8],
) -> Result<(), SendPacketError> {
    let id = Identifier::parse("minecraft:custom_payload").expect("a constant packet id");
    host.send_packet(ServerboundPlayPacket::new(
        id,
        custom_payload_body(channel, data),
    ))
}

/// Vanilla `CustomPayload` body: `Ident` (VarInt length + UTF-8), then the
/// application bytes with no further prefix.
fn custom_payload_body(channel: &str, data: &[u8]) -> Vec<u8> {
    let mut body = Writer::new();
    body.string(channel);
    body.buf.extend_from_slice(data);
    body.into_vec()
}

/// A new connection: ask for a secret as soon as the play phase is up.
pub fn on_join() {
    crate::session::stop("joined a new world");
    let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
    state.want_secret = true;
    state.request_attempts = 0;
    state.last_request = None;
    state.status = Status::RequestingSecret;
    state.info = None;
    state.speaking.clear();
    LOCATOR.with(|locator| locator.borrow_mut().reset());
}

/// A new client level (join or dimension change): old entity handles are stale.
pub fn reset_locator() {
    LOCATOR.with(|locator| locator.borrow_mut().reset());
}

pub fn on_leave(reason: &str) {
    crate::session::stop(reason);
    let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
    state.want_secret = false;
    state.request_attempts = 0;
    state.status = Status::Idle;
    state.info = None;
    state.speaking.clear();
    LOCATOR.with(|locator| locator.borrow_mut().reset());
}

/// Once per client tick: refresh settings and input, then send the request when due.
pub fn tick(host: &mut dyn ClientHost) {
    let controls = {
        let state = shared().lock().unwrap_or_else(|e| e.into_inner());
        state.controls.clone()
    };

    settings::refresh(host, &controls);
    let ptt = host.is_down(MOD_ID, settings::KEY_PUSH_TO_TALK);
    controls.set_push_to_talk(ptt);
    if host.consume_click(MOD_ID, settings::KEY_MUTE) {
        let now = !controls.muted();
        controls.set_muted(now);
        log::info!(
            "svc_client: microphone {}",
            if now { "muted" } else { "unmuted" }
        );
    }

    // SVC's `update_state` tells the server whether voice chat is disabled.
    let report_disabled = {
        let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
        let enabled = controls.enabled();
        let changed = enabled != state.last_enabled;
        state.last_enabled = enabled;
        changed.then_some(!enabled)
    };
    if let Some(disabled) = report_disabled {
        if let Err(error) = send_custom(host, tcp::UPDATE_STATE, &tcp::update_state(disabled)) {
            log::debug!("svc_client: update_state was not sent: {error:?}");
        }
    }

    update_listener(host);
    update_audio_positions(host, &controls);

    let (want, active, attempts, due) = {
        let state = shared().lock().unwrap_or_else(|e| e.into_inner());
        let due = state
            .last_request
            .is_none_or(|last| last.elapsed() >= REQUEST_RETRY);
        (
            state.want_secret,
            state.session_active,
            state.request_attempts,
            due,
        )
    };
    if !want || active || !due {
        return;
    }
    if attempts >= REQUEST_ATTEMPTS {
        let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
        state.want_secret = false;
        state.status = Status::Disconnected("no voice chat on this server".into());
        return;
    }

    let Some(server) = host.client_session().client_server() else {
        return;
    };
    if server.integrated_local {
        // SVC turns its server off in single player. There is nothing to talk to.
        let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
        state.want_secret = false;
        state.status = Status::Idle;
        return;
    }
    let Some(address) = server.address else {
        return;
    };
    if host.local_player().is_none() {
        return;
    }

    match send_custom(
        host,
        tcp::REQUEST_SECRET,
        &tcp::request_secret(tcp::COMPATIBILITY_VERSION),
    ) {
        Ok(()) => {
            let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
            state.request_attempts = state.request_attempts.saturating_add(1);
            state.last_request = Some(Instant::now());
            state.status = Status::RequestingSecret;
            log::info!(
                "svc_client: requested secret from {address} (compatibility {})",
                tcp::COMPATIBILITY_VERSION
            );
        }
        Err(error) => {
            let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
            state.request_attempts = state.request_attempts.saturating_add(1);
            state.last_request = Some(Instant::now());
            log::warn!("svc_client: could not request a secret: {error:?}");
        }
    }
}

/// The server answered `voicechat:request_secret`.
pub fn on_secret(host: &mut dyn ClientHost, data: &[u8]) {
    let secret = match tcp::parse_secret(data) {
        Ok(secret) => secret,
        Err(_) => {
            set_status(Status::Failed("malformed voicechat:secret".into()));
            log::warn!(
                "svc_client: malformed voicechat:secret ({} bytes)",
                data.len()
            );
            return;
        }
    };

    let server = host
        .client_session()
        .client_server()
        .and_then(|server| server.address);
    let Some(joined) = server else {
        set_status(Status::Failed("no server address".into()));
        return;
    };
    if host.local_player().is_none() {
        return;
    }

    let (voice_host, voice_port) = secret.voice_endpoint(&joined);
    let info = SecretInfo {
        player: secret.player,
        host: voice_host.clone(),
        port: voice_port,
        mtu: secret.mtu,
        distance: secret.distance,
        keep_alive_ms: secret.keep_alive_ms,
        groups: secret.groups,
        allow_recording: secret.allow_recording,
    };

    {
        let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
        state.info = Some(info.clone());
        state.want_secret = false;
        state.speaking.clear();
    }

    log::info!(
        "svc_client: secret for {} at {voice_host}:{voice_port} (mtu {}, distance {}, keep-alive {} ms, groups {})",
        crate::protocol::buf::uuid_text(secret.player),
        secret.mtu,
        secret.distance,
        secret.keep_alive_ms,
        secret.groups
    );

    // Tell the server the current "voice chat disabled" setting right away.
    let disabled = !controls().enabled();
    if let Err(error) = send_custom(host, tcp::UPDATE_STATE, &tcp::update_state(disabled)) {
        log::debug!("svc_client: update_state was not sent: {error:?}");
    }
    {
        let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
        state.last_enabled = !disabled;
    }

    crate::session::start(secret, voice_host, voice_port);
}

pub fn set_status(status: Status) {
    let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
    if let Status::Failed(ref reason) = status {
        log::warn!("svc_client: {reason}");
    }
    state.status = status;
}

/// A sound packet arrived for `sender`; the HUD keeps them listed briefly.
pub fn note_speaking(sender: u128) {
    let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
    if state.speaking.len() >= 128 && !state.speaking.contains_key(&sender) {
        if let Some(oldest) = state
            .speaking
            .iter()
            .min_by_key(|(_, at)| **at)
            .map(|(uuid, _)| *uuid)
        {
            state.speaking.remove(&oldest);
        }
    }
    state.speaking.insert(sender, Instant::now());
}

pub fn prune_speaking() {
    let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
    state.speaking.retain(|_, at| at.elapsed() < SPEAKING_HOLD);
}

fn update_listener(host: &mut dyn ClientHost) {
    let Some(camera) = host.camera() else {
        return;
    };
    let listener = Listener {
        position: camera.position,
        yaw: camera.yaw,
    };
    {
        let mut state = shared().lock().unwrap_or_else(|e| e.into_inner());
        state.listener = listener;
    }
    crate::audio::mixer::set_listener(listener);
}

/// Player sounds carry no position on the wire: SVC places them at the sender's
/// eyes, looked up on the client. Do that once per tick for every live source.
fn update_audio_positions(host: &mut dyn ClientHost, controls: &Controls) {
    let volume = if controls.enabled() {
        controls.volume()
    } else {
        0.0
    };
    crate::audio::mixer::set_volume(volume);
    prune_speaking();

    let senders = crate::audio::mixer::active_senders();
    if senders.is_empty() {
        return;
    }

    let tab = tab_names(host);
    let names: Vec<(u128, Option<String>)> = senders
        .iter()
        .map(|sender| (*sender, tab.get(sender).cloned()))
        .collect();

    let mut positions = Vec::with_capacity(senders.len());
    LOCATOR.with(|locator| {
        let mut locator = locator.borrow_mut();
        for (sender, name) in &names {
            if let Some(entity) = locator.entity_for(*sender, name.clone(), host) {
                positions.push((*sender, entity));
            }
        }
        locator.scan(host);
        // A handle found by this tick's scan is used next tick, after validation.
        for (sender, _) in &names {
            if let Some((entity, _)) = locator.known.get(sender) {
                positions.push((*sender, *entity));
            }
        }
    });

    positions.sort_unstable_by_key(|(sender, _)| *sender);
    positions.dedup_by_key(|(sender, _)| *sender);
    for (sender, entity) in positions {
        if let Some(position) = entity.eye_position(host) {
            crate::audio::mixer::set_player_position(sender, position);
        }
    }
}

/// Player names from the Tab list, which works for every player on a client,
/// unlike `player(uuid)`.
pub fn tab_names(host: &dyn ClientHost) -> HashMap<u128, String> {
    host.client_session()
        .client_tab_list()
        .map(|list| {
            list.entries
                .into_iter()
                .map(|entry| (entry.uuid, entry.name))
                .collect()
        })
        .unwrap_or_default()
}

pub fn controls() -> Arc<Controls> {
    shared()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .controls
        .clone()
}

pub fn own_mic() -> Arc<AtomicBool> {
    shared()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .own_mic
        .clone()
}

pub fn is_mic_active() -> bool {
    own_mic().load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_payload_body_is_ident_then_raw_data() {
        let body = custom_payload_body("voicechat:request_secret", &[0, 0, 0, 20]);
        assert_eq!(body[0], 24);
        assert_eq!(&body[1..25], b"voicechat:request_secret");
        assert_eq!(&body[25..], &[0, 0, 0, 20]);
        assert_eq!(body.len(), 29);
    }
}
