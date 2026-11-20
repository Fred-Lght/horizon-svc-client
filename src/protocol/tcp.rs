// SPDX-License-Identifier: GPL-3.0-or-later
//! The `voicechat:*` plugin channels carried by the game connection.
//!
//! Wire layout verified against Simple Voice Chat `2.6.x/26.3`
//! (`common/src/main/java/de/maxhenkel/voicechat/net/`) and the clean-room Rust
//! relay in `horizon-mods/native/voicechat-server/mixins/voicechat_server/relay/src/proto.rs`.

use super::buf::{Malformed, Read, Reader, Writer};

/// Compatibility version this client speaks. Stable SVC 2.6.28 reports 20.
pub const COMPATIBILITY_VERSION: i32 = 20;

pub const REQUEST_SECRET: &str = "voicechat:request_secret";
pub const SECRET: &str = "voicechat:secret";
pub const UPDATE_STATE: &str = "voicechat:update_state";
pub const STATE: &str = "voicechat:state";
pub const STATES: &str = "voicechat:states";
pub const REMOVE_STATE: &str = "voicechat:remove_state";

/// The Opus application the server asks for (`ServerConfig.Codec`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Voip,
    Audio,
    RestrictedLowDelay,
}

impl Codec {
    pub fn wire(self) -> u8 {
        match self {
            Self::Voip => 0,
            Self::Audio => 1,
            Self::RestrictedLowDelay => 2,
        }
    }

    fn from_wire(value: u8) -> Read<Self> {
        match value {
            0 => Ok(Self::Voip),
            1 => Ok(Self::Audio),
            2 => Ok(Self::RestrictedLowDelay),
            _ => Err(Malformed),
        }
    }
}

/// `voicechat:secret`: everything needed to open the UDP connection.
#[derive(Clone)]
pub struct Secret {
    pub key: [u8; 16],
    pub port: u16,
    pub player: u128,
    pub codec: Codec,
    pub mtu: u16,
    pub distance: f64,
    pub keep_alive_ms: u32,
    pub groups: bool,
    pub voice_host: String,
    pub allow_recording: bool,
}

impl Secret {
    /// Where to send UDP, given the address the player joined.
    ///
    /// `voice_host` follows SVC's `InitializationData.parseAddress`: empty means
    /// the joined host and the secret's port; a bare number is a port; a
    /// `host` or `host:port` replaces the host and optionally the port.
    pub fn voice_endpoint(&self, joined_address: &str) -> (String, u16) {
        let joined_host = host_of(joined_address);
        let default_port = if self.port == 0 {
            port_of(joined_address).unwrap_or(24_454)
        } else {
            self.port
        };
        let trimmed = self.voice_host.trim();
        if trimmed.is_empty() {
            return (joined_host, default_port);
        }
        if let Ok(port) = trimmed.parse::<u16>() {
            if port != 0 {
                return (joined_host, port);
            }
        }
        if let Ok(address) = trimmed.parse::<std::net::SocketAddr>() {
            return (address.ip().to_string(), address.port());
        }
        if let Some((host, port)) = trimmed.rsplit_once(':') {
            if !host.is_empty()
                && !host.contains(':')
                && let Ok(port) = port.parse::<u16>()
                && port != 0
            {
                return (host.to_owned(), port);
            }
        }
        (trimmed.to_owned(), default_port)
    }
}

/// The port part of `host:port`, `[v6]:port`, or `None`.
pub fn port_of(address: &str) -> Option<u16> {
    let address = address.trim();
    if let Ok(socket) = address.parse::<std::net::SocketAddr>() {
        return Some(socket.port());
    }
    let (host, port) = address.rsplit_once(':')?;
    if host.is_empty() || host.contains(':') {
        return None;
    }
    port.parse::<u16>().ok().filter(|port| *port != 0)
}

/// The host part of `host:port`, `[v6]:port` or a bare host.
pub fn host_of(address: &str) -> String {
    let address = address.trim();
    if let Ok(socket) = address.parse::<std::net::SocketAddr>() {
        return socket.ip().to_string();
    }
    if let Some((host, port)) = address.rsplit_once(':')
        && !host.is_empty()
        && !host.contains(':')
        && port.parse::<u16>().is_ok()
    {
        return host.to_owned();
    }
    address.to_owned()
}

pub fn request_secret(version: i32) -> Vec<u8> {
    let mut out = Writer::new();
    out.i32(version);
    out.into_vec()
}

pub fn parse_secret(data: &[u8]) -> Read<Secret> {
    let mut r = Reader::new(data);
    let key = {
        let mut key = [0u8; 16];
        for byte in &mut key {
            *byte = r.u8()?;
        }
        key
    };
    // -1 and 0 mean "the game server's own port"; `voice_endpoint` fills it in.
    let port = match r.i32()? {
        -1 | 0 => 0,
        value @ 1..=65_535 => value as u16,
        _ => return Err(Malformed),
    };
    let player = r.uuid()?;
    let codec = Codec::from_wire(r.u8()?)?;
    let mtu = r.i32()?.clamp(512, 2048) as u16;
    let distance = r.f64()?;
    if !distance.is_finite() || distance <= 0.0 {
        return Err(Malformed);
    }
    let keep_alive_ms = r.i32()?;
    if keep_alive_ms <= 0 {
        return Err(Malformed);
    }
    let groups = r.bool()?;
    let voice_host = r.string(32_767)?;
    let allow_recording = r.bool()?;
    Ok(Secret {
        key,
        port,
        player,
        codec,
        mtu,
        distance,
        keep_alive_ms: keep_alive_ms as u32,
        groups,
        voice_host,
        allow_recording,
    })
}

/// `voicechat:update_state` (compatibility 20): one boolean.
pub fn update_state(disabled: bool) -> Vec<u8> {
    let mut out = Writer::new();
    out.bool(disabled);
    out.into_vec()
}

/// A player's voice-chat state as compatibility 20 encodes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerState {
    pub uuid: u128,
    pub name: String,
    pub disabled: bool,
    pub disconnected: bool,
    pub group: Option<u128>,
}

impl PlayerState {
    fn read(r: &mut Reader<'_>) -> Read<Self> {
        Ok(Self {
            disabled: r.bool()?,
            disconnected: r.bool()?,
            uuid: r.uuid()?,
            name: r.string(256)?,
            group: if r.bool()? { Some(r.uuid()?) } else { None },
        })
    }

    #[cfg(test)]
    fn write(&self, w: &mut Writer) {
        w.bool(self.disabled)
            .bool(self.disconnected)
            .uuid(self.uuid)
            .string(&self.name)
            .bool(self.group.is_some());
        if let Some(group) = self.group {
            w.uuid(group);
        }
    }
}

pub fn parse_state(data: &[u8]) -> Read<PlayerState> {
    PlayerState::read(&mut Reader::new(data))
}

#[cfg(test)]
pub fn encode_state(state: &PlayerState) -> Vec<u8> {
    let mut out = Writer::new();
    state.write(&mut out);
    out.into_vec()
}

pub fn parse_states(data: &[u8]) -> Read<Vec<PlayerState>> {
    let mut r = Reader::new(data);
    let count = r.i32()?;
    // A hostile or corrupt packet must not make us allocate without bound.
    if count < 0 || count > 512 {
        return Err(Malformed);
    }
    let mut states = Vec::with_capacity(count as usize);
    for _ in 0..count {
        states.push(PlayerState::read(&mut r)?);
    }
    Ok(states)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret_bytes() -> Vec<u8> {
        let mut w = Writer::new();
        w.buf.extend_from_slice(&[7u8; 16]); // secret has no length prefix
        w.i32(24_454)
            .uuid(0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00)
            .u8(0) // VOIP
            .i32(1_275)
            .f64(48.0)
            .i32(1_000)
            .bool(true)
            .string("voice.example.org:24455")
            .bool(false);
        w.into_vec()
    }

    #[test]
    fn secret_is_read_field_by_field() {
        let secret = parse_secret(&secret_bytes()).unwrap();
        assert_eq!(secret.key, [7u8; 16]);
        assert_eq!(secret.port, 24_454);
        assert_eq!(secret.codec, Codec::Voip);
        assert_eq!(secret.mtu, 1_275);
        assert_eq!(secret.distance, 48.0);
        assert_eq!(secret.keep_alive_ms, 1_000);
        assert!(secret.groups);
        assert_eq!(secret.voice_host, "voice.example.org:24455");
        assert!(!secret.allow_recording);
    }

    #[test]
    fn voice_host_overrides_host_and_port() {
        let secret = parse_secret(&secret_bytes()).unwrap();
        assert_eq!(
            secret.voice_endpoint("mc.example.org:25565"),
            ("voice.example.org".to_owned(), 24_455)
        );
    }

    #[test]
    fn empty_voice_host_uses_the_joined_address() {
        let mut w = Writer::new();
        w.buf.extend_from_slice(&[0u8; 16]);
        w.i32(24_454)
            .uuid(0)
            .u8(0)
            .i32(1_275)
            .f64(48.0)
            .i32(1_000)
            .bool(false)
            .string("")
            .bool(false);
        let secret = parse_secret(&w.into_vec()).unwrap();
        assert_eq!(
            secret.voice_endpoint("mc.example.org:25565"),
            ("mc.example.org".to_owned(), 24_454)
        );
    }

    #[test]
    fn a_bare_port_in_voice_host_keeps_the_joined_host() {
        let mut w = Writer::new();
        w.buf.extend_from_slice(&[0u8; 16]);
        w.i32(24_454)
            .uuid(0)
            .u8(0)
            .i32(1_275)
            .f64(48.0)
            .i32(1_000)
            .bool(false)
            .string("25000")
            .bool(false);
        let secret = parse_secret(&w.into_vec()).unwrap();
        assert_eq!(
            secret.voice_endpoint("mc.example.org:25565"),
            ("mc.example.org".to_owned(), 25_000)
        );
    }

    #[test]
    fn a_port_of_minus_one_uses_the_joined_port() {
        let mut w = Writer::new();
        w.buf.extend_from_slice(&[0u8; 16]);
        w.i32(-1)
            .uuid(0)
            .u8(0)
            .i32(1_275)
            .f64(48.0)
            .i32(1_000)
            .bool(false)
            .string("")
            .bool(false);
        let secret = parse_secret(&w.into_vec()).unwrap();
        assert_eq!(
            secret.voice_endpoint("mc.example.org:25565"),
            ("mc.example.org".to_owned(), 25_565)
        );
    }

    #[test]
    fn a_broken_secret_is_rejected() {
        assert!(parse_secret(&[]).is_err());
        let mut bytes = secret_bytes();
        bytes.truncate(bytes.len() - 1);
        assert!(parse_secret(&bytes).is_err());
    }

    #[test]
    fn request_secret_is_one_big_endian_int() {
        assert_eq!(request_secret(20), vec![0, 0, 0, 20]);
    }

    #[test]
    fn state_roundtrip() {
        let state = PlayerState {
            uuid: 42,
            name: "Steve".into(),
            disabled: false,
            disconnected: true,
            group: Some(9),
        };
        assert_eq!(parse_state(&encode_state(&state)).unwrap(), state);
    }

    #[test]
    fn states_reject_an_absurd_count() {
        let mut w = Writer::new();
        w.i32(1_000_000);
        assert!(parse_states(&w.into_vec()).is_err());
    }
}
