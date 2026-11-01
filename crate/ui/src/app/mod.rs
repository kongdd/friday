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
pub mod icons;
pub mod logs;
pub mod modal_host;
pub mod supervisor;

// ─── app state ─────────────────────────────────────────────────────────────────

const RECORD_LEVEL_COUNT: usize = 40;

fn read_config_snapshot(path: &std::path::Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Full-width, one-physical-pixel separator with balanced row spacing.
fn row_separator(ui: &mut egui::Ui) {
    let pixel = ui.ctx().pixels_per_point().recip();
    let (gap, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 6.0 + pixel),
        egui::Sense::hover(),
    );
    let y = (gap.center().y / pixel).floor() * pixel + pixel / 2.0;
    ui.painter().hline(
        ui.clip_rect().x_range(),
        y,
        egui::Stroke::new(pixel, egui::Color32::from_gray(230)),
    );
}

fn card_style(
    ui: &egui::Ui,
    selected: bool,
) -> (egui::Color32, egui::Stroke, egui::Color32, egui::Color32) {
    use crate::log::{FG_DIM, FG_MUTED, FG_PRIMARY};
    use egui::Color32;

    let dark = ui.visuals().dark_mode;
    let accent = if dark {
        FG_PRIMARY
    } else {
        Color32::from_rgb(0, 110, 120)
    };
    let muted = if dark {
        FG_MUTED
    } else {
        Color32::from_rgb(95, 100, 110)
    };
    let (fill, width, border) = match (dark, selected) {
        (true, true) => (Color32::from_rgb(34, 56, 70), 1.0, accent),
        (true, false) => (Color32::from_rgb(24, 28, 34), 0.5, FG_DIM),
        (false, true) => (Color32::from_rgb(220, 240, 242), 1.0, accent),
        (false, false) => (Color32::from_rgb(245, 246, 248), 0.5, muted),
    };
    (fill, egui::Stroke::new(width, border), accent, muted)
}

pub struct AutosshApp {
    pub config_path: PathBuf,
    pub config: Config,
    pub dirty: bool,
    config_snapshot: Option<Vec<u8>>,
    save_error: Option<String>,
    reload_confirm: bool,
    exit_confirm: bool,
    force_exit: bool,
    pub selected_connection: usize,
    pub selected_global: usize,
    settings_open: bool,

    pub supervisor: Option<SupervisorHandle>,
    pub friday: FridayReceiver,
    pub recorder: FridayRecorder,
    record_levels: VecDeque<f32>,
    record_hotkey: RecordingHotkey,
    pub logs: Vec<crate::log::LogEntry>,
    log_scroll: LogScroll,
    /// Logs start collapsed to keep the window compact; starting a connection
    /// or Start all expands them so supervisor output is immediately visible.
    logs_collapsed: bool,

    modal: Modal,
    msg: Option<(String, Instant)>,

    /// Tracks which connections are checked for batch delete.
    checked_conn: HashSet<usize>,

    /// Kept alive for the lifetime of the app; dropping it removes the icon.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    tray: Option<crate::tray::AppTray>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    hidden_in_tray: bool,
    #[cfg(target_os = "macos")]
    exiting: bool,
}

impl AutosshApp {
    pub fn load(config_path: PathBuf) -> anyhow::Result<Self> {
        let config = if config_path.exists() {
            Config::load(&config_path)?
        } else {
            Config::default()
        };
        let config_snapshot = read_config_snapshot(&config_path)?;
        Ok(Self {
            config_path,
            config,
            dirty: false,
            config_snapshot,
            save_error: None,
            reload_confirm: false,
            exit_confirm: false,
            force_exit: false,
            selected_connection: 0,
            selected_global: 0,
            settings_open: false,
            supervisor: None,
            friday: FridayReceiver::new(),
            recorder: FridayRecorder::new(),
            record_levels: VecDeque::with_capacity(RECORD_LEVEL_COUNT),
            record_hotkey: RecordingHotkey::new(),
            logs: Vec::new(),
            log_scroll: LogScroll::default(),
            logs_collapsed: true,
            modal: Modal::None,
            msg: None,
            checked_conn: HashSet::new(),
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            tray: None,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            hidden_in_tray: false,
            #[cfg(target_os = "macos")]
            exiting: false,
        })
    }

    pub fn install_record_hotkey(&mut self, ctx: &egui::Context) {
        self.record_hotkey.install(ctx);
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub fn install_tray(&mut self, ctx: &egui::Context) -> anyhow::Result<()> {
        self.tray = Some(crate::tray::AppTray::new(ctx)?);
        Ok(())
    }

    // ─── config helpers (used by centre + supervisor + modal_host) ──

    pub fn keepalive(&self) -> KeepaliveConfig {
        self.config.keepalive.clone()
    }

    pub fn retry(&self) -> RetryConfig {
        self.config.retry.clone()
    }

    fn reload_config(&mut self) -> Result<(), String> {
        if !matches!(self.modal, Modal::None) {
            return Err("Close the open dialog before reloading".into());
        }
        let config = Config::load(&self.config_path).map_err(|error| format!("{error:#}"))?;
        let snapshot =
            read_config_snapshot(&self.config_path).map_err(|error| error.to_string())?;
        self.config = config;
        self.config_snapshot = snapshot;
        self.dirty = false;
        self.save_error = None;
        self.selected_connection = 0;
        self.checked_conn.clear();
        Ok(())
    }

    fn request_reload(&mut self) {
        if self.dirty {
            self.reload_confirm = true;
        } else if let Err(error) = self.reload_config() {
            self.flash(format!("Reload failed: {error}"));
        } else {
            self.flash("Configuration reloaded");
        }
    }

    fn autosave_if_changed(&mut self, previous: &Config) {
        if self.dirty && self.config != *previous {
            self.save();
        }
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
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        self.update_tray(ctx);
        #[cfg(target_os = "macos")]
        let closing = self.exiting;
        #[cfg(not(target_os = "macos"))]
        let closing = true;
        if closing
            && self.dirty
            && !self.force_exit
            && ctx.input(|input| input.viewport().close_requested())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.exit_confirm = true;
        }
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
        #[cfg(target_os = "macos")]
        if self.hidden_in_tray {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
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
        let previous = self.config.clone();
        self.render_modal(ui.ctx());
        self.render_dashboard(ui);
        self.render_toolbar(ui);
        self.render_logs_panel(ui);
        self.render_connections_panel(ui);
        self.autosave_if_changed(&previous);
        if self.exit_confirm {
            egui::Window::new("Unsaved changes")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label("Saving failed. Exit and discard changes?");
                    ui.horizontal(|ui| {
                        if ui.button("Exit anyway").clicked() {
                            self.force_exit = true;
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.button("Cancel").clicked() {
                            self.exit_confirm = false;
                            #[cfg(target_os = "macos")]
                            {
                                self.exiting = false;
                            }
                        }
                    });
                });
        }
        if self.reload_confirm {
            egui::Window::new("Discard unsaved changes?")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label("Reloading discards unsaved changes.");
                    ui.horizontal(|ui| {
                        if ui.button("Reload").clicked() {
                            self.reload_confirm = false;
                            if let Err(error) = self.reload_config() {
                                self.flash(format!("Reload failed: {error}"));
                            } else {
                                self.flash("Configuration reloaded");
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.reload_confirm = false;
                        }
                    });
                });
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Drop the supervisor now, rather than relying on process teardown.
        // Its Drop implementation also terminates all spawned SSH descendants.
        self.supervisor.take();
        self.friday.stop();
        // Failed saves must not overwrite externally modified configuration on exit.
    }
}

// ─── Windows tray auto-hide ────────────────────────────────────────────────────

impl AutosshApp {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn update_tray(&mut self, ctx: &egui::Context) {
        use crate::tray::TrayCommand;

        let command = self.tray.as_ref().and_then(crate::tray::AppTray::try_recv);

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
            Some(TrayCommand::Reload) => {
                self.request_reload();
                if self.reload_confirm
                    || self
                        .msg
                        .as_ref()
                        .is_some_and(|(msg, _)| msg.starts_with("Reload failed"))
                {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.hidden_in_tray = false;
                }
                return;
            }
            #[cfg(target_os = "macos")]
            Some(TrayCommand::Menu) => {
                if let Some(tray) = &mut self.tray {
                    tray.show_menu();
                }
                // A menu action may arrive during the nested native event loop.
                ctx.request_repaint();
                return;
            }
            Some(TrayCommand::Exit) => {
                #[cfg(target_os = "macos")]
                {
                    self.exiting = true;
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            None => {}
        }

        #[cfg(target_os = "macos")]
        if !self.exiting && ctx.input(|input| input.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.hidden_in_tray = true;
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

#[cfg(test)]
mod reload_tests {
    use super::*;

    #[test]
    fn autosave_detects_external_changes_and_preserves_edits() {
        let path =
            std::env::temp_dir().join(format!("autossh-autosave-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut app = AutosshApp::load(path.clone()).unwrap();
        let previous = app.config.clone();
        app.config.keepalive.interval = 42;
        app.dirty = true;
        app.autosave_if_changed(&previous);
        assert!(!app.dirty);
        assert_eq!(Config::load(&path).unwrap().keepalive.interval, 42);

        let external = "[keepalive]\ninterval = 77\n";
        std::fs::write(&path, external).unwrap();
        let previous = app.config.clone();
        app.config.keepalive.interval = 99;
        app.dirty = true;
        app.autosave_if_changed(&previous);
        assert!(app.dirty);
        assert!(app.save_error.is_some());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
        assert_eq!(app.config.keepalive.interval, 99);
        app.request_reload();
        assert!(app.reload_confirm);
        app.reload_config().unwrap();
        assert_eq!(app.config.keepalive.interval, 77);
        assert!(!app.dirty);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn reload_preserves_unsaved_changes_and_rejects_invalid_files() {
        let path = std::env::temp_dir().join(format!("autossh-reload-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            "[keepalive]\ninterval = 123\n[[connections]]\nname = 'demo'\n[[connections.forwards]]\nmode = 'local'\nforward = '8080'\n",
        )
        .unwrap();
        let mut app = AutosshApp::load(path.clone()).unwrap();
        app.config.keepalive.interval = 30;
        app.checked_conn.insert(0);
        app.reload_config().unwrap();
        assert_eq!(app.config.keepalive.interval, 123);
        assert!(app.checked_conn.is_empty());

        app.dirty = true;
        app.request_reload();
        assert!(app.reload_confirm);
        assert!(app.dirty);
        app.dirty = false;
        std::fs::write(&path, "invalid = [").unwrap();
        assert!(app.reload_config().is_err());
        assert_eq!(app.config.keepalive.interval, 123);
        std::fs::remove_file(path).unwrap();
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
