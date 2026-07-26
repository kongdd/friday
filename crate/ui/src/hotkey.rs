//! Global recording hotkey owned by the native UI event loop.

use std::sync::mpsc::{self, Receiver};

use eframe::egui;
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey},
};

pub struct RecordingHotkey {
    _manager: Option<GlobalHotKeyManager>,
    events: Option<Receiver<HotKeyState>>,
    error: Option<String>,
    down: bool,
}

impl RecordingHotkey {
    pub fn new() -> Self {
        Self {
            _manager: None,
            events: None,
            error: None,
            down: false,
        }
    }

    /// Register after eframe has created its native event loop. The callback
    /// wakes egui so F8 also works while the window is hidden or unfocused.
    pub fn install(&mut self, ctx: &egui::Context) {
        let hotkey = HotKey::new(None, Code::F8);
        let manager = match GlobalHotKeyManager::new().and_then(|manager| {
            manager.register(hotkey)?;
            Ok(manager)
        }) {
            Ok(manager) => manager,
            Err(error) => {
                self.error = Some(format!("cannot register F8 hotkey: {error}"));
                return;
            }
        };

        let (events_tx, events_rx) = mpsc::channel();
        let ctx = ctx.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.id == hotkey.id() {
                let _ = events_tx.send(event.state);
                ctx.request_repaint();
            }
        }));
        self._manager = Some(manager);
        self.events = Some(events_rx);
        self.error = None;
    }

    pub fn pressed(&mut self) -> bool {
        let mut pressed = false;
        loop {
            let state = self
                .events
                .as_ref()
                .and_then(|events| events.try_recv().ok());
            let Some(state) = state else {
                break;
            };
            pressed |= self.accept(state);
        }
        pressed
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn accept(&mut self, state: HotKeyState) -> bool {
        match state {
            HotKeyState::Pressed if !self.down => {
                self.down = true;
                true
            }
            HotKeyState::Released => {
                self.down = false;
                false
            }
            _ => false,
        }
    }
}

impl Default for RecordingHotkey {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggers_once_until_f8_is_released() {
        let mut hotkey = RecordingHotkey::new();
        assert!(hotkey.accept(HotKeyState::Pressed));
        assert!(!hotkey.accept(HotKeyState::Pressed));
        assert!(!hotkey.accept(HotKeyState::Released));
        assert!(hotkey.accept(HotKeyState::Pressed));
    }
}
