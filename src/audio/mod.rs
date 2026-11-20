// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio capture, Opus and the 3D mixer.

use std::sync::OnceLock;
use std::sync::mpsc::Sender;

pub mod capture;
pub mod mixer;
pub mod playback;
pub mod spatial;

static STARTED: OnceLock<()> = OnceLock::new();

/// Start playback and capture once, on the first voice session.
pub fn start() {
    STARTED.get_or_init(|| {
        playback::start();
        capture::start();
    });
}

/// One encoded 20 ms frame from the microphone thread to the UDP thread.
#[derive(Clone, Debug)]
pub struct MicFrame {
    pub data: Vec<u8>,
    pub sequence: i64,
    pub whispering: bool,
}

pub type MicSender = Sender<MicFrame>;
