// SPDX-License-Identifier: GPL-3.0-or-later
//! The UDP voice session: authentication, keep-alives and sound packets.

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::audio::MicFrame;
use crate::protocol::crypto::Cipher;
use crate::protocol::tcp::Secret;
use crate::protocol::udp::{self, ClientPacket, ServerPacket};
use crate::state::{self, Status};

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
static MIC_TX: OnceLock<Mutex<Option<(u64, crate::audio::MicSender)>>> = OnceLock::new();

fn mic_slot() -> &'static Mutex<Option<(u64, crate::audio::MicSender)>> {
    MIC_TX.get_or_init(|| Mutex::new(None))
}

/// The capture thread sends frames here when a session is live.
pub fn mic_sender() -> Option<crate::audio::MicSender> {
    mic_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|(_, sender)| sender.clone())
}

pub fn start(secret: Secret, host: String, port: u16) {
    let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<MicFrame>();

    {
        let mut shared = state::shared().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = shared.stop.take() {
            old.store(true, Ordering::Relaxed);
        }
        shared.generation = generation;
        shared.session_active = true;
        shared.stop = Some(Arc::clone(&stop));
        shared.status = Status::Authenticating;
    }
    *mic_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some((generation, tx));
    state::controls().set_codec(secret.codec.wire());
    state::controls().set_max_payload(secret.mtu);
    crate::audio::start();

    let spawned = thread::Builder::new()
        .name("svc-udp".to_owned())
        .spawn(move || run(generation, secret, host, port, stop, rx));

    if let Err(error) = spawned {
        let mut shared = state::shared().lock().unwrap_or_else(|e| e.into_inner());
        shared.session_active = false;
        shared.generation = 0;
        shared.stop = None;
        shared.status = Status::Failed(format!("could not start UDP thread: {error}"));
        drop(shared);
        *mic_slot().lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Stop the current session. Safe on every disconnect path.
pub fn stop(reason: &str) {
    let mut shared = state::shared().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(stop) = shared.stop.take() {
        stop.store(true, Ordering::Relaxed);
    }
    shared.session_active = false;
    shared.generation = 0;
    shared.info = None;
    shared
        .own_mic
        .store(false, std::sync::atomic::Ordering::Relaxed);
    if shared.status != Status::Idle {
        shared.status = Status::Disconnected(reason.to_owned());
    }
    drop(shared);

    *mic_slot().lock().unwrap_or_else(|e| e.into_inner()) = None;
    crate::audio::mixer::clear();
}

fn clear_mic(generation: u64) {
    let mut slot = mic_slot().lock().unwrap_or_else(|e| e.into_inner());
    if slot.as_ref().is_some_and(|(owner, _)| *owner == generation) {
        *slot = None;
    }
}

/// A session that never reached its loop: mark it failed and let the next join retry.
fn abort(generation: u64, message: String) {
    if is_current(generation) {
        let mut shared = state::shared().lock().unwrap_or_else(|e| e.into_inner());
        shared.session_active = false;
        shared.stop = None;
        shared.generation = 0;
        shared
            .own_mic
            .store(false, std::sync::atomic::Ordering::Relaxed);
        shared.status = Status::Failed(message);
    }
    clear_mic(generation);
}

fn is_current(generation: u64) -> bool {
    state::shared()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .generation
        == generation
}

fn run(
    generation: u64,
    secret: Secret,
    host: String,
    port: u16,
    stop: Arc<AtomicBool>,
    mic_rx: Receiver<MicFrame>,
) {
    let cipher = Cipher::new(&secret.key);
    let keep_alive = Duration::from_millis(u64::from(secret.keep_alive_ms.clamp(100, 60_000)));
    let timeout = keep_alive * 10;

    let target = match (host.as_str(), port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut addresses| addresses.next())
    {
        Some(address) => address,
        None => {
            abort(
                generation,
                format!("could not resolve the voice host {host}:{port}"),
            );
            return;
        }
    };
    let bind = if target.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    };
    let socket = match UdpSocket::bind(bind).and_then(|socket| {
        socket.connect(target)?;
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;
        Ok(socket)
    }) {
        Ok(socket) => socket,
        Err(error) => {
            abort(
                generation,
                format!("UDP connect to {target} failed: {error}"),
            );
            return;
        }
    };

    let peer: Option<SocketAddr> = socket.peer_addr().ok();
    log::info!(
        "svc_client: UDP socket opened towards {host}:{port} ({:?})",
        peer
    );

    let started = Instant::now();
    let mut authenticated = false;
    let mut connected = false;
    let mut last_send = Instant::now() - Duration::from_secs(1);
    let mut last_rx = Instant::now();
    let mut buffer = [0u8; 4096];

    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();

        if !connected && now.duration_since(last_send) >= Duration::from_millis(1000) {
            let packet = if authenticated {
                ClientPacket::ConnectionCheck
            } else {
                ClientPacket::Authenticate {
                    player: secret.player,
                    secret: secret.key,
                }
            };
            if let Err(error) = send(&socket, &cipher, secret.player, &packet) {
                log::warn!("svc_client: UDP send failed: {error}");
            }
            last_send = now;
        }

        if !connected && now.duration_since(started) > Duration::from_secs(15) {
            if is_current(generation) {
                state::set_status(Status::Failed(
                    "voice server did not answer the handshake".into(),
                ));
            }
            break;
        }

        if connected && now.duration_since(last_rx) > timeout {
            if is_current(generation) {
                state::set_status(Status::Failed(format!(
                    "voice server timed out after {} ms without a keep-alive",
                    timeout.as_millis()
                )));
                crate::audio::mixer::clear();
            }
            break;
        }

        loop {
            match mic_rx.try_recv() {
                Ok(frame) => {
                    if connected
                        && let Err(error) = send(
                            &socket,
                            &cipher,
                            secret.player,
                            &ClientPacket::Mic {
                                data: frame.data,
                                sequence: frame.sequence,
                                whispering: frame.whispering,
                            },
                        )
                    {
                        log::warn!("svc_client: microphone packet failed: {error}");
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        let len = match socket.recv(&mut buffer) {
            Ok(len) => len,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                // A connected UDP socket reports ICMP port-unreachable this way;
                // keep retrying until the handshake deadline.
                continue;
            }
            Err(error) if stop.load(Ordering::Relaxed) => {
                let _ = error;
                break;
            }
            Err(error) => {
                if is_current(generation) {
                    state::set_status(Status::Failed(format!("UDP receive failed: {error}")));
                }
                break;
            }
        };

        let sealed = match udp::parse_server_datagram(&buffer[..len]) {
            Ok(sealed) => sealed,
            Err(_) => continue,
        };
        let plain = match cipher.open(sealed) {
            Ok(plain) => plain,
            Err(_) => {
                log::debug!("svc_client: dropped a datagram that did not authenticate");
                continue;
            }
        };
        let packet = match udp::parse_server_packet(&plain) {
            Ok(packet) => packet,
            Err(_) => continue,
        };
        last_rx = Instant::now();

        match packet {
            ServerPacket::AuthenticateAck => {
                if !authenticated {
                    authenticated = true;
                    last_send = Instant::now() - Duration::from_secs(1);
                    log::info!("svc_client: server acknowledged authentication");
                }
            }
            ServerPacket::ConnectionCheckAck => {
                if authenticated && !connected {
                    connected = true;
                    if is_current(generation) {
                        state::set_status(Status::Connected);
                    }
                    log::info!("svc_client: connected to the voice server at {host}:{port}");
                }
            }
            ServerPacket::KeepAlive => {
                let _ = send(&socket, &cipher, secret.player, &ClientPacket::KeepAlive);
            }
            ServerPacket::Ping { id, timestamp } => {
                let _ = send(
                    &socket,
                    &cipher,
                    secret.player,
                    &ClientPacket::Ping { id, timestamp },
                );
            }
            ServerPacket::PlayerSound {
                sender,
                data,
                sequence,
                ..
            } => {
                state::note_speaking(sender);
                crate::audio::mixer::push_encoded(sender, sequence, &data, None, false);
            }
            ServerPacket::GroupSound {
                sender,
                data,
                sequence,
                ..
            } => {
                state::note_speaking(sender);
                crate::audio::mixer::push_encoded(sender, sequence, &data, None, true);
            }
            ServerPacket::LocationSound {
                sender,
                location,
                data,
                sequence,
                distance,
                ..
            } => {
                state::note_speaking(sender);
                crate::audio::mixer::push_encoded(
                    sender,
                    sequence,
                    &data,
                    Some((location, distance)),
                    false,
                );
            }
            ServerPacket::Unknown(id) => {
                log::debug!("svc_client: unknown voice packet 0x{id:02x}");
            }
        }
    }

    clear_mic(generation);

    if is_current(generation) {
        let mut shared = state::shared().lock().unwrap_or_else(|e| e.into_inner());
        shared.session_active = false;
        shared.stop = None;
        shared
            .own_mic
            .store(false, std::sync::atomic::Ordering::Relaxed);
        if !matches!(shared.status, Status::Failed(_)) {
            shared.status = Status::Disconnected("voice connection closed".into());
        }
    }
}

fn send(
    socket: &UdpSocket,
    cipher: &Cipher,
    player: u128,
    packet: &ClientPacket,
) -> std::io::Result<()> {
    let plain = packet.encode();
    let sealed = cipher
        .seal(&plain)
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    let datagram = udp::client_datagram(player, &sealed);
    socket.send(&datagram)?;
    Ok(())
}

/// Called from the client tick; reserved for jitter-buffer upkeep.
pub fn tick(_host: &mut dyn mod_api::ClientHost) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::buf::Writer;
    use crate::protocol::tcp::Codec;
    use std::net::SocketAddr;

    fn server_send(socket: &UdpSocket, cipher: &Cipher, peer: SocketAddr, packet: &ServerPacket) {
        let mut plain = Vec::new();
        match packet {
            ServerPacket::AuthenticateAck => plain.push(udp::AUTHENTICATE_ACK),
            ServerPacket::ConnectionCheckAck => plain.push(udp::CONNECTION_CHECK_ACK),
            ServerPacket::KeepAlive => plain.push(udp::KEEP_ALIVE),
            _ => unreachable!(),
        }
        let sealed = cipher.seal(&plain).unwrap();
        let mut out = Writer::new();
        out.u8(udp::MAGIC).bytes(&sealed);
        socket.send_to(&out.buf, peer).unwrap();
    }

    #[test]
    fn a_mock_voice_server_sees_the_v20_handshake() {
        let server = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let port = server.local_addr().unwrap().port();
        let key = [42u8; 16];
        let player = 0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10;
        let secret = Secret {
            key,
            port,
            player,
            codec: Codec::Voip,
            mtu: 1275,
            distance: 48.0,
            keep_alive_ms: 1000,
            groups: true,
            voice_host: String::new(),
            allow_recording: false,
        };

        let stop = Arc::new(AtomicBool::new(false));
        let (_tx, rx) = mpsc::channel::<MicFrame>();
        let client_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            run(9999, secret, "127.0.0.1".into(), port, client_stop, rx);
        });

        let cipher = Cipher::new(&key);
        let mut buffer = [0u8; 4096];

        // Authenticate: magic + player UUID + VarInt + AES-GCM, body type 5,
        // then the player UUID and the 16 raw secret bytes.
        let (len, peer) = server.recv_from(&mut buffer).unwrap();
        let (who, sealed) = udp::parse_client_datagram(&buffer[..len]).unwrap();
        assert_eq!(who, player);
        let plain = cipher.open(sealed).unwrap();
        assert_eq!(plain[0], udp::AUTHENTICATE);
        assert_eq!(&plain[1..17], &player.to_be_bytes());
        assert_eq!(&plain[17..], &key);
        server_send(&server, &cipher, peer, &ServerPacket::AuthenticateAck);

        // ConnectionCheck: only the type byte.
        let (len, _) = server.recv_from(&mut buffer).unwrap();
        let (_, sealed) = udp::parse_client_datagram(&buffer[..len]).unwrap();
        assert_eq!(cipher.open(sealed).unwrap(), vec![udp::CONNECTION_CHECK]);
        server_send(&server, &cipher, peer, &ServerPacket::ConnectionCheckAck);

        // Keep-alive from the server is echoed by the client.
        server_send(&server, &cipher, peer, &ServerPacket::KeepAlive);
        let (len, _) = server.recv_from(&mut buffer).unwrap();
        let (_, sealed) = udp::parse_client_datagram(&buffer[..len]).unwrap();
        assert_eq!(cipher.open(sealed).unwrap(), vec![udp::KEEP_ALIVE]);

        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }
}
