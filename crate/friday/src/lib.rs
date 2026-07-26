//! Embedded Friday voice receiver.
//!
//! The listener is intentionally owned by the GUI rather than the SSH
//! supervisor: hiding the window in the Windows tray keeps it alive, while the
//! Start/Stop control can release `127.0.0.1:17322` without affecting tunnels.
//!
//! Module layout:
//!
//! | Module     | Responsibility                                                |
//! |------------|----------------------------------------------------------------|
//! | `player`   | mpv path resolution and platform-correct `Command`.             |
//! | `http`     | Bind the listener, parse `/speak` payloads, dispatch playback.  |
//! | `receiver` | Receiver state and worker lifecycle driven by the GUI.          |
//! | `recorder` | Microphone capture, pause/resume, finish, and local replay.      |
//!
//! Hosts use the exported receiver and recorder controllers; platform details
//! stay crate-private.

mod capture;
mod http;
mod playback;
mod player;
mod receiver;
mod recorder;

pub use player::LISTEN_ADDR;
pub use receiver::{FridayReceiver, FridayState};
pub use recorder::{FridayRecorder, RecordingState};
