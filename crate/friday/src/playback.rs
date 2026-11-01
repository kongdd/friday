//! Track in-flight mpv children so Stop or the next message cuts playback immediately.

use std::{
    path::PathBuf,
    process::Child,
    sync::{Arc, Mutex},
};

pub(super) struct PlaybackRegistry {
    inner: Mutex<PlaybackState>,
}

#[derive(Default)]
struct PlaybackState {
    generation: u64,
    players: Vec<(Child, PathBuf)>,
}

impl PlaybackRegistry {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(PlaybackState::default()),
        })
    }

    /// Interrupt current speech as soon as the next message arrives.
    pub(super) fn begin(&self) -> u64 {
        let Ok(mut state) = self.inner.lock() else {
            return 0;
        };
        state.generation = state.generation.wrapping_add(1);
        kill_players(&mut state.players);
        state.generation
    }

    pub(super) fn register(&self, generation: u64, mut child: Child, path: PathBuf) {
        if let Ok(mut state) = self.inner.lock()
            && generation == state.generation
        {
            state.players.push((child, path));
            return;
        }
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(path);
    }

    /// Drop finished players and remove their temp MP3 files.
    pub(super) fn reap_finished(&self) {
        let Ok(mut state) = self.inner.lock() else {
            return;
        };
        state
            .players
            .retain_mut(|(child, path)| match child.try_wait() {
                Ok(Some(_)) => {
                    let _ = std::fs::remove_file(path);
                    false
                }
                Ok(None) => true,
                Err(_) => {
                    let _ = std::fs::remove_file(path);
                    false
                }
            });
    }

    /// Kill active mpv instances and invalidate requests still being decoded.
    pub(super) fn kill_all(&self) {
        let Ok(mut state) = self.inner.lock() else {
            return;
        };
        state.generation = state.generation.wrapping_add(1);
        kill_players(&mut state.players);
    }
}

fn kill_players(players: &mut Vec<(Child, PathBuf)>) {
    for (mut child, path) in players.drain(..) {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_message_invalidates_previous_playback() {
        let playback = PlaybackRegistry::new();
        let previous = playback.begin();
        let current = playback.begin();

        assert_ne!(previous, current);
        assert_eq!(playback.inner.lock().unwrap().generation, current);
    }
}
