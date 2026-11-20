// SPDX-License-Identifier: GPL-3.0-or-later
//! The in-game overlay: connection status, microphone indicator and the players
//! currently talking.

use mod_api::{ClientHost, DrawContext, GuiRenderPostEvent};

use crate::state::{self, Status};

const PANEL_WIDTH: f32 = 210.0;
const LINE: f32 = 11.0;
const PAD: f32 = 5.0;

pub fn draw(host: &dyn ClientHost, event: &mut GuiRenderPostEvent<'_>) {
    if event.gui_hidden {
        return;
    }
    let controls = state::controls();
    if !controls.hud_enabled() {
        return;
    }

    state::prune_speaking();
    let (status, info, mut speaking) = {
        let shared = state::shared().lock().unwrap_or_else(|e| e.into_inner());
        (
            shared.status.clone(),
            shared.info.clone(),
            shared
                .speaking
                .iter()
                .map(|(uuid, at)| (*uuid, *at))
                .collect::<Vec<_>>(),
        )
    };
    if matches!(status, Status::Idle) && info.is_none() {
        return;
    }

    speaking.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
    speaking.truncate(5);
    let tab = state::tab_names(host);

    let detail = detail_line(&status, info.as_ref());
    let rows = 2.0 + speaking.len() as f32;
    let height = PAD * 2.0 + rows * LINE + 4.0;
    let [screen_width, screen_height] = event.batch.size();
    let x = 8.0;
    let y = (screen_height - height - 8.0).max(8.0);
    let _ = screen_width;

    event.batch.fill([x, y, PANEL_WIDTH, height], 0x9000_0000);
    event
        .batch
        .text("SVC", [x + PAD, y + PAD], 0xFFFF_FFFF, true);

    let (dot, color) = status_style(&status);
    event.batch.fill(
        [x + PAD, y + PAD + LINE + 2.0, 5.0, 5.0],
        if dot { color } else { 0xFF60_6060 },
    );
    event
        .batch
        .text(&detail, [x + PAD + 8.0, y + PAD + LINE], color, true);

    let (mic_color, mic_text) = if controls.muted() || !controls.enabled() {
        (0xFFFF_5555, "mic off")
    } else if state::is_mic_active() {
        (0xFF55_FF55, "mic on")
    } else if controls.mode() == crate::settings::MODE_PUSH_TO_TALK {
        (0xFFAA_AA55, "push to talk")
    } else {
        (0xFF88_88_88, "voice activation")
    };
    event
        .batch
        .fill([x + PAD, y + PAD + LINE * 2.0 + 2.0, 5.0, 5.0], mic_color);
    event.batch.text(
        mic_text,
        [x + PAD + 8.0, y + PAD + LINE * 2.0],
        mic_color,
        true,
    );

    let mut row = 0.0;
    for (uuid, _) in &speaking {
        let name = tab
            .get(uuid)
            .cloned()
            .or_else(|| {
                host.player(*uuid)
                    .and_then(|player| player.entity().name(host))
            })
            .unwrap_or_else(|| short_uuid(*uuid));
        event.batch.text(
            &truncate(&name, 26),
            [x + PAD + 8.0, y + PAD + LINE * (3.0 + row)],
            0xFF55_FF55,
            true,
        );
        row += 1.0;
    }
}

fn detail_line(status: &Status, info: Option<&crate::state::SecretInfo>) -> String {
    match status {
        Status::Idle => "idle".to_owned(),
        Status::RequestingSecret => "asking the server for a secret".to_owned(),
        Status::Authenticating => "authenticating".to_owned(),
        Status::Connected => match info {
            Some(info) => format!(
                "connected  {}:{}  {:.0}m",
                info.host, info.port, info.distance
            ),
            None => "connected".to_owned(),
        },
        Status::Failed(reason) => format!("failed: {}", truncate(reason, 40)),
        Status::Disconnected(reason) => format!("disconnected: {}", truncate(reason, 32)),
    }
}

fn status_style(status: &Status) -> (bool, u32) {
    match status {
        Status::Connected => (true, 0xFF55_FF55),
        Status::RequestingSecret | Status::Authenticating => (true, 0xFFFF_CC55),
        Status::Failed(_) => (true, 0xFFFF_5555),
        Status::Disconnected(_) => (true, 0xFFFF_8855),
        Status::Idle => (false, 0xFF88_8888),
    }
}

fn short_uuid(uuid: u128) -> String {
    let text = crate::protocol::buf::uuid_text(uuid);
    text.chars().take(8).collect()
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Kept because `DrawContext` is object-safe but its text measurement is useful
/// once the overlay grows.
#[allow(dead_code)]
fn text_width(draw: &dyn DrawContext, text: &str) -> f32 {
    draw.text_width(text)
}
