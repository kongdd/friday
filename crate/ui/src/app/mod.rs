//! Application state and the `eframe::App` glue.
//!
//! `AutosshApp` holds the configuration, the supervisor handle, the log buffer,
//! the modal state machine, and the Windows tray icon. Work is split across
//! the two phases provided by [`eframe::App`]:
//!
//! 1. [`eframe::App::logic`] mutates state and handles tray commands. It also
//!    runs while the native window is hidden, which keeps Show and Exit usable.
//! 2. [`eframe::App::ui`] renders the dashboard, panels, logs, and modal.

use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    time::{Duration, Instant},
};

use autossh_core::{Config, KeepaliveConfig, RetryConfig};
use eframe::egui;

use crate::hotkey::RecordingHotkey;
use crate::log::{LOG_BUFFER_LIMIT, LogScroll};
use crate::modal::Modal;
use crate::supervisor::SupervisorHandle;
use friday::{FridayReceiver, FridayRecorder};

pub mod centre;
pub mod connections;
pub mod dashboard;
pub mod logs;
pub mod modal_host;
pub mod supervisor;

// ─── app state ─────────────────────────────────────────────────────────────────

const RECORD_LEVEL_COUNT: usize = 40;

pub struct AutosshApp {
    pub config_path: PathBuf,
    pub config: Config,
    pub dirty: bool,
    pub selected_connection: usize,
    pub selected_global: usize,

    pub supervisor: Option<SupervisorHandle>,
    pub friday: FridayReceiver,
    pub recorder: FridayRecorder,
    record_levels: VecDeque<f32>,
    record_hotkey: RecordingHotkey,
    pub logs: Vec<crate::log::LogEntry>,
    log_scroll: LogScroll,

    modal: Modal,
    msg: Option<(String, Instant)>,

    /// Tracks which connections are checked for batch delete.
    checked_conn: HashSet<usize>,

    /// Kept alive for the lifetime of the app; dropping it removes the icon.
    #[cfg(target_os = "windows")]
    windows_tray: Option<crate::tray::WindowsTray>,
    #[cfg(target_os = "windows")]
    hidden_in_tray: bool,
}

impl AutosshApp {
    pub fn load(config_path: PathBuf) -> anyhow::Result<Self> {
        let config = if config_path.exists() {
            Config::load(&config_path)?
        } else {
            Config::default()
        };
        Ok(Self {
            config_path,
            config,
            dirty: false,
            selected_connection: 0,
            selected_global: 0,
            supervisor: None,
            friday: FridayReceiver::new(),
            recorder: FridayRecorder::new(),
            record_levels: VecDeque::with_capacity(RECORD_LEVEL_COUNT),
            record_hotkey: RecordingHotkey::new(),
            logs: Vec::new(),
            log_scroll: LogScroll::default(),
            modal: Modal::None,
            msg: None,
            checked_conn: HashSet::new(),
            #[cfg(target_os = "windows")]
            windows_tray: None,
            #[cfg(target_os = "windows")]
            hidden_in_tray: false,
        })
    }

    pub fn install_record_hotkey(&mut self, ctx: &egui::Context) {
        self.record_hotkey.install(ctx);
    }

    #[cfg(target_os = "windows")]
    pub fn install_windows_tray(&mut self, ctx: &egui::Context) -> anyhow::Result<()> {
        self.windows_tray = Some(crate::tray::WindowsTray::new(ctx)?);
        Ok(())
    }

    // ─── config helpers (used by centre + supervisor + modal_host) ──

    pub fn keepalive(&self) -> KeepaliveConfig {
        self.config.keepalive.clone()
    }

    pub fn retry(&self) -> RetryConfig {
        self.config.retry.clone()
    }

    pub fn apply_globals(&mut self, ka: &KeepaliveConfig, r: &RetryConfig) {
        self.config.keepalive = ka.clone();
        self.config.retry = r.clone();
        self.dirty = true;
    }

    // ─── toast message ──

    pub fn flash(&mut self, text: impl Into<String>) {
        self.msg = Some((text.into(), Instant::now()));
    }

    fn prune_msg(&mut self) {
        if let Some((_, t)) = &self.msg
            && t.elapsed() > Duration::from_secs(4)
        {
            self.msg = None;
        }
    }

    fn toggle_recording(&mut self) {
        let result = if self.recorder.is_active() {
            self.recorder.finish()
        } else {
            self.recorder.start()
        };
        if let Err(error) = result {
            self.flash(error);
        }
    }
}

// ─── eframe entry ──────────────────────────────────────────────────────────────

impl eframe::App for AutosshApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "windows")]
        self.update_windows_tray(ctx);
        self.poll_supervisor();
        self.friday.poll();
        if self.record_hotkey.pressed() {
            self.toggle_recording();
        }
        self.recorder.poll();
        if self.recorder.state() == friday::RecordingState::Recording {
            let raw = self.recorder.level();
            let previous = self.record_levels.back().copied().unwrap_or_default();
            let level = if raw >= previous {
                raw * 0.7 + previous * 0.3
            } else {
                raw * 0.25 + previous * 0.75
            };
            if self.record_levels.len() == RECORD_LEVEL_COUNT {
                self.record_levels.pop_front();
            }
            self.record_levels.push_back(level);
        } else if !self.recorder.is_active() {
            self.record_levels.clear();
        }
        self.prune_msg();
        if self.supervisor.is_some()
            || self.friday.is_active()
            || self.recorder.is_active()
            || self.recorder.is_playing()
        {
            // Keep background state flowing even while the window is hidden.
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render_modal(ui.ctx());
        self.render_dashboard(ui);
        self.render_logs_panel(ui);
        self.render_connections_panel(ui);
        self.render_centre_panel(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Drop the supervisor now, rather than relying on process teardown.
        // Its Drop implementation also terminates all spawned SSH descendants.
        self.supervisor.take();
        self.friday.stop();
        // Best-effort save so the user does not silently lose edits.
        if self.dirty {
            let _ = self.config.save(&self.config_path);
        }
    }
}

// ─── Windows tray auto-hide ────────────────────────────────────────────────────

impl AutosshApp {
    #[cfg(target_os = "windows")]
    fn update_windows_tray(&mut self, ctx: &egui::Context) {
        use crate::tray::TrayCommand;

        let command = self
            .windows_tray
            .as_ref()
            .and_then(crate::tray::WindowsTray::try_recv);

        match command {
            Some(TrayCommand::Show) => {
                // Restore before focusing: Windows ignores Focus for hidden or
                // minimized windows.
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                self.hidden_in_tray = false;
                return;
            }
            Some(TrayCommand::Exit) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            None => {}
        }

        // A minimized window would normally remain in the taskbar. Hide it
        // completely instead; the tray icon remains available for restoring it.
        let minimized = ctx.input(|input| input.viewport().minimized.unwrap_or(false));
        if minimized && !self.hidden_in_tray {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            self.hidden_in_tray = true;
        }
    }
}

// ─── log buffer trimming (kept here because it owns `logs`/`log_scroll`) ─────

impl AutosshApp {
    fn poll_supervisor(&mut self) {
        let Some(handle) = self.supervisor.as_ref() else {
            return;
        };
        let mut entries = Vec::new();
        handle.drain(&mut entries);
        self.logs
            .extend(entries.into_iter().filter(crate::log::is_displayable));
        if self.logs.len() > LOG_BUFFER_LIMIT {
            let excess = self.logs.len() - (LOG_BUFFER_LIMIT - 100);
            self.logs.drain(..excess);
        }
    }
}
