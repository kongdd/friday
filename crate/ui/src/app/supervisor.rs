//! Supervisor lifecycle and config persistence.

use crate::supervisor::{SupervisorHandle, locate_supervisor};

use super::AutosshApp;

impl AutosshApp {
    pub fn start_supervisor(&mut self) {
        // Starting anything is the cue to watch the log tail.
        self.logs_collapsed = false;
        // A supervisor that exited is stale; discard its handle before a new
        // Start All so the button can recover without restarting the UI.
        if self.supervisor_running() {
            return;
        }
        self.supervisor.take();
        if self.dirty && !self.save() {
            return;
        }
        let Some(binary) = locate_supervisor() else {
            self.flash("cannot find autossh-core binary beside this UI; check PATH");
            return;
        };
        match SupervisorHandle::start(&binary, &self.config_path) {
            Ok(handle) => {
                self.flash(format!("started all: {}", binary.display()));
                self.supervisor = Some(handle);
            }
            Err(error) => {
                self.flash(format!("cannot start all: {error:#}"));
            }
        }
    }

    pub fn stop_supervisor(&mut self) {
        if let Some(handle) = self.supervisor.take() {
            // Graceful shutdown so supervisor can flush closing log lines
            // (SIGTERM → drain → SIGKILL fallback).
            handle.shutdown(&mut self.logs);
            self.flash("stopped all");
        }
    }

    pub fn supervisor_running(&self) -> bool {
        self.supervisor
            .as_ref()
            .is_some_and(SupervisorHandle::alive)
    }

    /// Persist the desired state immediately: the core supervisor watches the
    /// config file and starts/stops just this worker after its next poll.
    pub fn toggle_connection(&mut self, index: usize) {
        let running = self.supervisor_running();
        let Some(connection) = self.config.connections.get_mut(index) else {
            return;
        };
        let was_enabled = connection.enabled;
        let name = connection.name.clone();
        // An enabled connection is only running while its supervisor is alive.
        // Thus an inactive row always means Start, even if it was enabled in a
        // config loaded before the supervisor was launched.
        let start = !running || !was_enabled;
        connection.enabled = start;
        if start {
            self.logs_collapsed = false;
        }

        self.dirty = true;
        if !self.save() {
            return;
        }

        if start {
            self.flash(format!("starting {name}"));
            if !running {
                self.start_supervisor();
            }
        } else {
            self.flash(format!("stopping {name}"));
        }
    }

    pub fn save(&mut self) -> bool {
        let result = (|| -> anyhow::Result<()> {
            let current = super::read_config_snapshot(&self.config_path)?;
            anyhow::ensure!(
                current == self.config_snapshot,
                "configuration changed on disk; reload before saving"
            );
            self.config.save(&self.config_path)?;
            self.config_snapshot = super::read_config_snapshot(&self.config_path)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.dirty = false;
                self.save_error = None;
                true
            }
            Err(error) => {
                let message = format!("Save failed: {error:#}");
                self.save_error = Some(message.clone());
                self.flash(message);
                false
            }
        }
    }
}
