# Simple Voice Chat for Horizon

A native Horizon mod (`.hmod`) that speaks the **Simple Voice Chat 2.6** protocol
(compatibility version 20): proximity voice over UDP, Opus 48 kHz mono, push-to-talk,
voice activation and a small speaking HUD.

This is a clean-room client written from SVC's packet layouts. It contains no code or
assets from Simple Voice Chat and is not affiliated with or endorsed by Max Henkel.

## What works

- `voicechat:request_secret` / `voicechat:secret` over the game connection.
- AES-128-GCM UDP handshake: `Authenticate -> AuthenticateAck -> ConnectionCheck ->
  ConnectionCheckAck`, keep-alives and ping replies.
- Microphone capture through cpal, Opus encoding in 20 ms / 960-sample frames.
- Opus decoding and a stereo 3D mixer with SVC's distance and panning formulas.
- Push-to-talk (Caps Lock), mute (J), voice activation with threshold and hangover.
- HUD with connection state, microphone indicator and the players currently talking.
- Player positions come from a bounded entity-id scan plus the Tab list, because the
  public client API has no UUID-to-entity lookup (`entity_by_uuid` is server-only).
- Settings under **Options > Mods > Simple Voice Chat for Horizon**.

Groups, whispers and the 2.7 / compatibility 21 protocol are not implemented yet.

## Requirements

- Horizon pinned to `0a875da91dbb0fdcb7ba8c42c11341aa229af51a` (Nightly 2026-11-20).
- A working microphone and output device for the audio parts.
- The launcher builds against its pinned Rust 1.99.1 and the signed SDK for that build.
- Build-time native tools: CMake (the `opus` crate vendors libopus) and, on Linux,
  ALSA headers (`libasound2-dev`) plus pkg-config for cpal.

## Build

The launcher is the supported path:

```sh
horizon-launcher native-build . --instance "My Instance" --trust-native-code
```

For a local source check, the pinned SDK must be in `.horizon/sdk/api`:

```sh
cargo check
cargo test
cargo build --release
```

Registry dependencies are pinned in `Cargo.lock`. In a fresh checkout, run
`horizon-launcher native-lock .` before a launcher build if the launcher asks for it.

Nothing is deployed automatically.

## Protocol notes

- UDP client datagram: `0xFF | player UUID | VarInt len | IV(12) | AES-GCM`.
- UDP server datagram: `0xFF | VarInt len | IV(12) | AES-GCM`.
- Opus: 48 kHz, mono, 20 ms, at most 1275 bytes; libopus is compiled into the mod
  (no system `libopus` at run time).
- The voice port and host come from `voicechat:secret`, with `voice_host` parsed the
  same way SVC's `InitializationData` does.

## Layout

```
src/protocol/   SVC wire format, AES-GCM
src/session.rs  UDP thread, handshake, keep-alive
src/audio/      cpal capture/playback, Opus, 3D mixer
src/state.rs    shared state, request retries, speaking list
src/hud.rs      DrawContext overlay
```

## License

GNU General Public License v3.0 **or later** (`GPL-3.0-or-later`).
The full text is in `LICENSE`; third-party notices are in `NOTICE.md`.
