// SPDX-License-Identifier: GPL-3.0-or-later
//! Simple Voice Chat 2.6 client for Horizon.
//!
//! Clean-room implementation written from SVC's packet layouts. No code or assets
//! from Simple Voice Chat are copied.

mod audio;
mod hud;
mod protocol;
mod session;
mod settings;
mod state;

use mod_api::{ClientHost, Hooks, Registrar, RegistrationContext};

#[derive(Default)]
struct SvcClient;

#[mod_api::horizon_mod]
impl Hooks for SvcClient {
    fn register(&self, r: &mut dyn Registrar, _: &RegistrationContext) {
        settings::register(r);
    }

    fn client_player_logging_in(
        &self,
        _: &mut dyn ClientHost,
        _: &mut mod_api::ClientPlayerLoggingInEvent,
    ) {
        state::on_join();
    }

    fn client_level_load(&self, _: &mut dyn ClientHost, _: &mut mod_api::ClientLevelLoadEvent) {
        state::reset_locator();
    }

    fn client_player_logging_out(
        &self,
        _: &mut dyn ClientHost,
        _: &mut mod_api::ClientPlayerLoggingOutEvent,
    ) {
        state::on_leave("logged out");
    }

    fn client_disconnecting(&self, _: &dyn ClientHost, _: &mut mod_api::ClientDisconnectingEvent) {
        state::on_leave("disconnected");
    }

    fn client_packet(&self, host: &mut dyn ClientHost, event: &mut mod_api::ClientPacketEvent<'_>) {
        let Some(payload) = &event.packet.custom_payload else {
            return;
        };
        match payload.channel.as_str() {
            protocol::tcp::SECRET => state::on_secret(host, &payload.data),
            protocol::tcp::STATE => match protocol::tcp::parse_state(&payload.data) {
                Ok(state) => log::trace!(
                    "svc_client: state {} disabled={} disconnected={}",
                    state.name,
                    state.disabled,
                    state.disconnected
                ),
                Err(_) => log::debug!("svc_client: malformed voicechat:state"),
            },
            protocol::tcp::STATES => match protocol::tcp::parse_states(&payload.data) {
                Ok(states) => log::trace!("svc_client: {} player states", states.len()),
                Err(_) => log::debug!("svc_client: malformed voicechat:states"),
            },
            protocol::tcp::REMOVE_STATE => log::trace!("svc_client: a player state was removed"),
            other => log::trace!("svc_client: voicechat channel {other} (not used yet)"),
        }
    }

    fn client_tick_post(&self, host: &mut dyn ClientHost, _: &mut mod_api::ClientTickPostEvent) {
        state::tick(host);
        session::tick(host);
    }

    fn gui_render_post(&self, host: &dyn ClientHost, event: &mut mod_api::GuiRenderPostEvent<'_>) {
        hud::draw(host, event);
    }
}
