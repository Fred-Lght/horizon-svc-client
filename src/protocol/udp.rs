// SPDX-License-Identifier: GPL-3.0-or-later
//! The UDP side of SVC compatibility version 20.
//!
//! Client datagram: `0xFF | UUID player | VarInt len | sealed`.
//! Server datagram: `0xFF | VarInt len | sealed`.
//! `sealed` is produced by [`super::crypto::Cipher`]; the plain text starts with a
//! one-byte packet id.

use super::buf::{Malformed, Read, Reader, Writer};

pub const MAGIC: u8 = 0xff;
pub const MAX_BODY: usize = 2048;
pub const MAX_OPUS: usize = 1275;

pub const MIC: u8 = 0x01;
pub const PLAYER_SOUND: u8 = 0x02;
pub const GROUP_SOUND: u8 = 0x03;
pub const LOCATION_SOUND: u8 = 0x04;
pub const AUTHENTICATE: u8 = 0x05;
pub const AUTHENTICATE_ACK: u8 = 0x06;
pub const PING: u8 = 0x07;
pub const KEEP_ALIVE: u8 = 0x08;
pub const CONNECTION_CHECK: u8 = 0x09;
pub const CONNECTION_CHECK_ACK: u8 = 0x0a;

/// A packet the client sends to the voice server.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientPacket {
    Mic {
        data: Vec<u8>,
        sequence: i64,
        whispering: bool,
    },
    Authenticate {
        player: u128,
        secret: [u8; 16],
    },
    Ping {
        id: u128,
        timestamp: i64,
    },
    KeepAlive,
    ConnectionCheck,
}

impl ClientPacket {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new();
        match self {
            Self::Mic {
                data,
                sequence,
                whispering,
            } => {
                out.u8(MIC).bytes(data).i64(*sequence).bool(*whispering);
            }
            Self::Authenticate { player, secret } => {
                // `Secret.toBytes` writes the 16 raw bytes; there is no length prefix.
                out.u8(AUTHENTICATE).uuid(*player);
                out.buf.extend_from_slice(secret);
            }
            Self::Ping { id, timestamp } => {
                out.u8(PING).uuid(*id).i64(*timestamp);
            }
            Self::KeepAlive => {
                out.u8(KEEP_ALIVE);
            }
            Self::ConnectionCheck => {
                out.u8(CONNECTION_CHECK);
            }
        }
        out.into_vec()
    }
}

/// Sound data as it arrives from the server.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerPacket {
    PlayerSound {
        channel: u128,
        sender: u128,
        data: Vec<u8>,
        sequence: i64,
        distance: f32,
        whispering: bool,
        category: Option<String>,
    },
    GroupSound {
        channel: u128,
        sender: u128,
        data: Vec<u8>,
        sequence: i64,
        category: Option<String>,
    },
    LocationSound {
        channel: u128,
        sender: u128,
        location: [f64; 3],
        data: Vec<u8>,
        sequence: i64,
        distance: f32,
        category: Option<String>,
    },
    AuthenticateAck,
    Ping {
        id: u128,
        timestamp: i64,
    },
    KeepAlive,
    ConnectionCheckAck,
    /// A packet id this client does not implement. The caller drops it.
    Unknown(u8),
}

const WHISPER_MASK: u8 = 0b0000_0001;
const HAS_CATEGORY_MASK: u8 = 0b0000_0010;

fn category(r: &mut Reader<'_>, flags: u8) -> Read<Option<String>> {
    if flags & HAS_CATEGORY_MASK != 0 {
        Ok(Some(r.string(16)?))
    } else {
        Ok(None)
    }
}

/// Parse the decrypted body of a server datagram.
pub fn parse_server_packet(plain: &[u8]) -> Read<ServerPacket> {
    let mut r = Reader::new(plain);
    let id = r.u8()?;
    Ok(match id {
        PLAYER_SOUND => {
            let channel = r.uuid()?;
            let sender = r.uuid()?;
            let data = r.bytes(MAX_OPUS)?.to_vec();
            let sequence = r.i64()?;
            let distance = r.f32()?;
            if !distance.is_finite() || distance <= 0.0 {
                return Err(Malformed);
            }
            let flags = r.u8()?;
            ServerPacket::PlayerSound {
                channel,
                sender,
                data,
                sequence,
                distance,
                whispering: flags & WHISPER_MASK != 0,
                category: category(&mut r, flags)?,
            }
        }
        GROUP_SOUND => {
            let channel = r.uuid()?;
            let sender = r.uuid()?;
            let data = r.bytes(MAX_OPUS)?.to_vec();
            let sequence = r.i64()?;
            let flags = r.u8()?;
            ServerPacket::GroupSound {
                channel,
                sender,
                data,
                sequence,
                category: category(&mut r, flags)?,
            }
        }
        LOCATION_SOUND => {
            let channel = r.uuid()?;
            let sender = r.uuid()?;
            let location = [r.f64()?, r.f64()?, r.f64()?];
            if !location.iter().all(|value| value.is_finite()) {
                return Err(Malformed);
            }
            let data = r.bytes(MAX_OPUS)?.to_vec();
            let sequence = r.i64()?;
            let distance = r.f32()?;
            if !distance.is_finite() || distance <= 0.0 {
                return Err(Malformed);
            }
            let flags = r.u8()?;
            ServerPacket::LocationSound {
                channel,
                sender,
                location,
                data,
                sequence,
                distance,
                category: category(&mut r, flags)?,
            }
        }
        AUTHENTICATE_ACK => ServerPacket::AuthenticateAck,
        PING => ServerPacket::Ping {
            id: r.uuid()?,
            timestamp: r.i64()?,
        },
        KEEP_ALIVE => ServerPacket::KeepAlive,
        CONNECTION_CHECK_ACK => ServerPacket::ConnectionCheckAck,
        other => ServerPacket::Unknown(other),
    })
}

/// Wrap an encrypted client packet for the wire.
pub fn client_datagram(player: u128, sealed: &[u8]) -> Vec<u8> {
    let mut out = Writer::new();
    out.u8(MAGIC).uuid(player).bytes(sealed);
    out.into_vec()
}

/// Read a client datagram's player id and encrypted body (used by tests and tools).
#[cfg(test)]
pub fn parse_client_datagram(datagram: &[u8]) -> Read<(u128, &[u8])> {
    let mut r = Reader::new(datagram);
    if r.u8()? != MAGIC {
        return Err(Malformed);
    }
    let player = r.uuid()?;
    Ok((player, r.bytes(MAX_BODY)?))
}

/// Read the encrypted body of a server datagram.
pub fn parse_server_datagram(datagram: &[u8]) -> Read<&[u8]> {
    let mut r = Reader::new(datagram);
    if r.u8()? != MAGIC {
        return Err(Malformed);
    }
    r.bytes(MAX_BODY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mic_layout_matches_svc() {
        let packet = ClientPacket::Mic {
            data: vec![1, 2, 3],
            sequence: 7,
            whispering: true,
        };
        assert_eq!(
            packet.encode(),
            vec![MIC, 3, 1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 7, 1]
        );
    }

    #[test]
    fn authenticate_carries_player_and_secret() {
        let packet = ClientPacket::Authenticate {
            player: 1,
            secret: [9u8; 16],
        };
        let bytes = packet.encode();
        assert_eq!(bytes[0], AUTHENTICATE);
        assert_eq!(bytes.len(), 1 + 16 + 16);
        assert_eq!(&bytes[17..], &[9u8; 16]);
    }

    #[test]
    fn client_datagram_has_magic_player_and_length() {
        let bytes = client_datagram(0x0102, &[3, 4, 5]);
        assert_eq!(bytes[0], MAGIC);
        assert_eq!(bytes[16], 0x02);
        assert_eq!(bytes[16 + 1], 3);
        assert_eq!(&bytes[16 + 2..], &[3, 4, 5]);
    }

    #[test]
    fn server_datagram_has_no_player_id() {
        let mut w = Writer::new();
        w.u8(MAGIC).bytes(&[1, 2, 3, 4]);
        assert_eq!(parse_server_datagram(&w.into_vec()).unwrap(), &[1, 2, 3, 4]);
        assert!(parse_server_datagram(&[0, 0]).is_err());
    }

    fn player_sound() -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(PLAYER_SOUND)
            .uuid(11) // channel
            .uuid(22) // sender
            .bytes(&[0xaa, 0xbb])
            .i64(5)
            .f32(48.0)
            .u8(WHISPER_MASK | HAS_CATEGORY_MASK)
            .string("player");
        w.into_vec()
    }

    #[test]
    fn player_sound_is_read_with_its_flags() {
        assert_eq!(
            parse_server_packet(&player_sound()).unwrap(),
            ServerPacket::PlayerSound {
                channel: 11,
                sender: 22,
                data: vec![0xaa, 0xbb],
                sequence: 5,
                distance: 48.0,
                whispering: true,
                category: Some("player".into()),
            }
        );
    }

    #[test]
    fn location_sound_has_three_doubles() {
        let mut w = Writer::new();
        w.u8(LOCATION_SOUND)
            .uuid(1)
            .uuid(2)
            .f64(1.5)
            .f64(64.0)
            .f64(-3.5)
            .bytes(&[0x01])
            .i64(9)
            .f32(24.0)
            .u8(0);
        match parse_server_packet(&w.into_vec()).unwrap() {
            ServerPacket::LocationSound {
                location, distance, ..
            } => {
                assert_eq!(location, [1.5, 64.0, -3.5]);
                assert_eq!(distance, 24.0);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn a_truncated_sound_packet_is_malformed() {
        let mut bytes = player_sound();
        bytes.pop();
        assert!(parse_server_packet(&bytes).is_err());
    }
}
