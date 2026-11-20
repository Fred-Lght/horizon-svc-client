# Third-party notices

This mod is licensed under the GNU General Public License v3.0 or later (see `LICENSE`).
It depends on crates whose own, GPL-compatible licences apply:

- **libopus** — Xiph.Org Foundation, BSD-3-Clause. Vendored and statically linked by
  `opusic-sys` (feature `bundled`) through the `opus` crate.
- **cpal** — Apache-2.0 OR MIT.
- **opus** (Rust bindings) — MIT OR Apache-2.0.
- **aes-gcm**, **aead**, **aes** — Apache-2.0 OR MIT.
- **rand** — Apache-2.0 OR MIT.
- **serde**, **log** and their transitive dependencies — their respective licences.

The complete dependency graph and exact versions are in `Cargo.lock`. This mod contains
no code or assets from Simple Voice Chat; it implements the wire format from its public
packet layouts.
