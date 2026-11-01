//! Main toolbar: connection imports, direct audio controls, and global actions.

use eframe::egui::{self, RichText};

use crate::log::{FG_ERROR, FG_MUTED, FG_PRIMARY, FG_SUCCESS, FG_WARNING};
use crate::modal::{GlobalGroup, Modal};
use friday::{FridayState, LISTEN_ADDR, RecordingState};

use super::icons::{self, Icon};
use super::{AutosshApp, RECORD_LEVEL_COUNT};

#[derive(Clone, Copy)]
enum RecorderAction {
    Start,
    Pause,
    Resume,
    Finish,
    Play,
    StopPlayback,
    OpenFolder,
}

fn audio_status_label(
    ui: &mut egui::Ui,
    text: &str,
    color: egui::Color32,
    width: f32,
) -> egui::Response {
    // Centre the text so a fixed slot leaves symmetric, barely visible margins.
    ui.allocate_ui_with_layout(
        egui::vec2(width, 22.0),
        egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
        |ui| ui.label(RichText::new(text).small().color(color)),
    )
    .inner
    .on_hover_text(text)
}

fn receiver_status(state: FridayState) -> (&'static str, egui::Color32) {
    match state {
        FridayState::Starting => ("Starting", FG_WARNING),
        FridayState::Listening => ("Listening", FG_SUCCESS),
        FridayState::Stopping => ("Stopping", FG_WARNING),
        FridayState::Stopped => ("Stopped", FG_MUTED),
        FridayState::Failed => ("Failed", FG_ERROR),
    }
}

fn recorder_status(state: RecordingState) -> (&'static str, egui::Color32) {
    match state {
        RecordingState::Idle => ("Idle", FG_MUTED),
        RecordingState::Recording => ("Recording", FG_ERROR),
        RecordingState::Paused => ("Paused", FG_WARNING),
        RecordingState::Finished => ("Saved", FG_SUCCESS),
        RecordingState::Failed => ("Failed", FG_ERROR),
    }
}

fn open_recording_folder() -> Result<(), String> {
    let path = friday::recording_dir()?;
    let program = if cfg!(target_os = "windows") {
        "explorer.exe"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = std::process::Command::new(program)
        .arg(&path)
        .spawn()
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let _waiter = std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn render_recording_waveform(
    ui: &mut egui::Ui,
    levels: &std::collections::VecDeque<f32>,
    state: RecordingState,
) {
    let color = match state {
        RecordingState::Recording => FG_PRIMARY,
        RecordingState::Paused => FG_WARNING,
        _ => FG_MUTED,
    };
    // Compact fixed width: the waveform must not stretch the toolbar row.
    let (rect, _) = ui.allocate_exact_size(egui::vec2(160.0, 18.0), egui::Sense::hover());
    let bars = RECORD_LEVEL_COUNT;
    let gap = 2.0;
    let width = (rect.width() - gap * (bars - 1) as f32) / bars as f32;
    let leading = bars.saturating_sub(levels.len());

    for index in 0..bars {
        let level = index
            .checked_sub(leading)
            .and_then(|index| levels.get(index))
            .copied()
            .unwrap_or_default();
        let height = 2.0 + 14.0 * level;
        let x = rect.left() + index as f32 * (width + gap);
        let bar = egui::Rect::from_center_size(
            egui::pos2(x + width / 2.0, rect.center().y),
            egui::vec2(width.max(1.0), height),
        );
        ui.painter().rect_filled(bar, width / 2.0, color);
    }

    if state == RecordingState::Recording {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }
}

impl AutosshApp {
    pub fn render_toolbar(&mut self, root: &mut egui::Ui) {
        egui::Panel::top("toolbar").show_inside(root, |ui| {
            // Compact spacing keeps the two toolbar rows thin.
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 2.0);
            ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);
            // The panel contributes 2px; match the 5px below the menu row.
            ui.add_space(3.0);
            egui::MenuBar::new().ui(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                // ASCII labels avoid missing-glyph boxes in the Windows font fallback.
                if ui.button("Add").clicked() {
                    self.modal = Modal::Add(crate::modal::AddDialogState::default());
                }
                ui.menu_button("Config", |ui| {
                    if ui.button("Open configuration").clicked() {
                        #[cfg(target_os = "macos")]
                        let mut command =
                            std::process::Command::new("/Applications/Zed.app/Contents/MacOS/cli");
                        #[cfg(not(target_os = "macos"))]
                        let mut command = std::process::Command::new("zed");
                        match command.arg(&self.config_path).spawn() {
                            Ok(mut child) => {
                                std::thread::spawn(move || {
                                    let _ = child.wait();
                                });
                            }
                            Err(error) => self.flash(format!("Cannot open configuration: {error}")),
                        }
                        ui.close();
                    }
                    if ui.button("Import configuration…").clicked() {
                        self.modal = Modal::Import(crate::modal::ImportDialogState {
                            path_input: crate::default_path().display().to_string(),
                            ..crate::modal::ImportDialogState::default()
                        });
                        ui.close();
                    }
                    if ui.button("Import SSH hosts…").clicked() {
                        self.modal = Modal::ImportSsh(crate::modal::SshImportState {
                            source_path: crate::ssh_config::default_ssh_config_path(),
                            ..crate::modal::SshImportState::default()
                        });
                        ui.close();
                    }
                });
                if ui.button("Setting").clicked() {
                    self.settings_open = true;
                }
                let selected = self.checked_conn.len();
                if selected > 0
                    && ui
                        .button(RichText::new(format!("Delete ({selected})")).color(FG_ERROR))
                        .clicked()
                {
                    self.delete_checked_connections();
                }
            });
            super::row_separator(ui);
            self.render_audio_status(ui);
        });

        let mut open = self.settings_open;
        // Escape dismisses the settings window without touching the config.
        if open
            && root
                .ctx()
                .input(|input| input.key_pressed(egui::Key::Escape))
        {
            open = false;
        }
        if open {
            egui::Window::new("Settings")
                .open(&mut open)
                .default_width(500.0)
                .show(root.ctx(), |ui| self.render_globals(ui));
        }
        self.settings_open = open;
    }

    fn render_globals(&mut self, ui: &mut egui::Ui) {
        let ka = self.keepalive();
        let r = self.retry();
        let mut sel = self.selected_global.min(5);

        // (display, edit): display keeps the human-readable suffix for the
        // readout; edit is the raw number so the EditGlobal modal can parse it
        // back into a u64. Mixing the two is the bug that turned "30 s" into
        // an un-parseable initial value.
        let seconds = |value: u64| (format!("{value} s"), value.to_string());
        let rows = [
            (0, GlobalGroup::KeepaliveInterval, seconds(ka.interval)),
            (
                1,
                GlobalGroup::KeepaliveCount,
                (ka.count_max.to_string(), ka.count_max.to_string()),
            ),
            (
                2,
                GlobalGroup::KeepaliveTimeout,
                seconds(ka.connect_timeout),
            ),
            (3, GlobalGroup::RetryInitial, seconds(r.initial_seconds)),
            (4, GlobalGroup::RetryMaximum, seconds(r.maximum_seconds)),
            (5, GlobalGroup::RetryStable, seconds(r.stable_seconds)),
        ];

        ui.add_space(4.0);
        ui.columns(2, |cols| {
            for (column, title) in [(0, "Keepalive"), (1, "Retry")] {
                cols[column].group(|ui| {
                    ui.add_space(2.0);
                    ui.heading(title);
                    ui.add_space(2.0);
                    for (idx, group, (display, edit)) in rows.iter().skip(column * 3).take(3) {
                        self.render_global_row(ui, *idx, &mut sel, *group, display, edit);
                    }
                });
            }
        });
        ui.add_space(2.0);
        ui.label(
            RichText::new("shared by every connection; click to highlight, double-click to edit")
                .small()
                .color(FG_MUTED),
        );
        self.selected_global = sel;
    }

    fn render_audio_status(&mut self, ui: &mut egui::Ui) {
        let mut action = None;
        // Never wrap: the waveform must stay on the same row as its controls.
        ui.horizontal(|ui| {
            self.render_receiver_controls(ui);
            let (status, color) = receiver_status(self.friday.state());
            // Reserve every slot, including idle controls, so transitions never move the waveform.
            audio_status_label(ui, status, color, 60.0);
            audio_status_label(ui, LISTEN_ADDR, FG_MUTED, 96.0)
                .on_hover_text(format!("http://{LISTEN_ADDR}/speak"));
            ui.separator();
            action = self.render_recorder_controls(ui).or(action);
            let state = self.recorder.state();
            let (status, color) = recorder_status(state);
            let elapsed = self.recorder.elapsed().as_secs();
            let playing = self.recorder.is_playing();
            let status = if playing { "Playing" } else { status };
            // One fixed-width slot keeps the row compact and stable across states.
            audio_status_label(
                ui,
                &format!("{status} {:02}:{:02}", elapsed / 60, elapsed % 60),
                color,
                88.0,
            );
            render_recording_waveform(ui, &self.record_levels, state);
            if icons::icon_button(
                ui,
                if playing { Icon::Stop } else { Icon::Play },
                state == RecordingState::Finished,
                if playing {
                    "Stop playback"
                } else {
                    "Play latest recording"
                },
            )
            .clicked()
            {
                action = Some(if playing {
                    RecorderAction::StopPlayback
                } else {
                    RecorderAction::Play
                });
            }
            if icons::icon_button(ui, Icon::Folder, true, "Open recording folder").clicked() {
                action = Some(RecorderAction::OpenFolder);
            }
            if state == RecordingState::Finished
                && let Some(path) = self.recorder.path()
                && let Some(name) = path.file_name()
            {
                ui.label(
                    RichText::new(name.to_string_lossy())
                        .small()
                        .color(FG_SUCCESS),
                )
                .on_hover_text(path.display().to_string());
            }
        });
        self.apply_recorder_action(action);
        for (source, error) in [
            ("Receiver", self.friday.error()),
            ("Microphone", self.recorder.error()),
            ("F8", self.record_hotkey.error()),
        ] {
            if let Some(error) = error {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("{source}: {error}"))
                            .small()
                            .color(FG_ERROR),
                    );
                });
            }
        }
    }

    fn render_receiver_controls(&mut self, ui: &mut egui::Ui) {
        let state = self.friday.state();
        let label = match state {
            FridayState::Stopped | FridayState::Failed => "Start receiver",
            FridayState::Listening => "Stop receiver",
            FridayState::Starting => "Starting…",
            FridayState::Stopping => "Stopping…",
        };
        if icons::icon_button(
            ui,
            Icon::Speaker,
            !matches!(state, FridayState::Starting | FridayState::Stopping),
            label,
        )
        .clicked()
        {
            if state == FridayState::Listening {
                self.friday.stop();
            } else {
                self.friday.start();
            }
        }
    }

    /// Returns the requested recorder action; the caller applies it once.
    fn render_recorder_controls(&self, ui: &mut egui::Ui) -> Option<RecorderAction> {
        let state = self.recorder.state();
        let active = self.recorder.is_active();
        let mut action = None;
        if icons::icon_button(
            ui,
            Icon::Microphone,
            true,
            if active {
                "Finish recording (F8)"
            } else {
                "Start recording (F8)"
            },
        )
        .clicked()
        {
            action = Some(if active {
                RecorderAction::Finish
            } else {
                RecorderAction::Start
            });
        }
        let paused = state == RecordingState::Paused;
        if icons::icon_button(
            ui,
            if paused { Icon::Play } else { Icon::Pause },
            active,
            if paused {
                "Resume recording"
            } else {
                "Pause recording"
            },
        )
        .clicked()
        {
            action = Some(if paused {
                RecorderAction::Resume
            } else {
                RecorderAction::Pause
            });
        }
        action
    }

    fn apply_recorder_action(&mut self, action: Option<RecorderAction>) {
        let result = match action {
            Some(RecorderAction::Start) => self.recorder.start(),
            Some(RecorderAction::Pause) => self.recorder.pause(),
            Some(RecorderAction::Resume) => self.recorder.resume(),
            Some(RecorderAction::Finish) => self.recorder.finish(),
            Some(RecorderAction::Play) => self.recorder.play(),
            Some(RecorderAction::StopPlayback) => {
                self.recorder.stop_playback();
                Ok(())
            }
            Some(RecorderAction::OpenFolder) => open_recording_folder(),
            None => Ok(()),
        };
        if let Err(error) = result {
            self.flash(error);
        }
    }

    fn render_global_row(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        selected: &mut usize,
        group: GlobalGroup,
        display: &str,
        edit: &str,
    ) {
        let is_sel = *selected == index;
        let (fill, stroke, accent, muted) = super::card_style(ui, is_sel);
        let response = egui::Frame::group(ui.style())
            .fill(fill)
            .stroke(stroke)
            .corner_radius(egui::CornerRadius::same(4))
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical(|ui| {
                    ui.label(RichText::new(group.label()).color(muted).small());
                    ui.label(RichText::new(display).strong().color(accent).monospace());
                });
            });
        let interact = response.response.interact(egui::Sense::click());
        if interact.clicked() {
            *selected = index;
        }
        if interact.double_clicked() {
            *selected = index;
            self.modal = Modal::EditGlobal {
                group,
                value: edit.to_owned(),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_icons_follow_waveform_and_playback_starts_disabled() {
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal(|ui| {
                    render_recording_waveform(ui, &Default::default(), RecordingState::Idle);
                    let waveform_end = ui.cursor().left();
                    let playback =
                        icons::icon_button(ui, Icon::Play, false, "Play latest recording");
                    let folder =
                        icons::icon_button(ui, Icon::Folder, true, "Open recording folder");
                    assert!(!playback.enabled());
                    assert!(folder.enabled());
                    assert!(playback.rect.left() >= waveform_end);
                    assert!(folder.rect.left() >= playback.rect.right());
                });
            });
        });
    }

    #[test]
    fn audio_icons_and_status_slots_keep_fixed_widths() {
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for (icon, enabled, tooltip) in [
                    (Icon::Speaker, true, "Start receiver"),
                    (Icon::Speaker, false, "Starting…"),
                    (Icon::Speaker, true, "Stop receiver"),
                    (Icon::Microphone, true, "Start recording (F8)"),
                    (Icon::Microphone, true, "Finish recording (F8)"),
                    (Icon::Pause, false, "Pause recording"),
                    (Icon::Play, true, "Resume recording"),
                ] {
                    let response = icons::icon_button(ui, icon, enabled, tooltip);
                    assert_eq!(response.rect.size(), egui::vec2(24.0, 22.0));
                }
                for text in [
                    "Idle",
                    "Recording",
                    "Paused",
                    "Saved",
                    "Playing",
                    "Starting",
                    "Listening",
                    "Failed",
                ] {
                    let response = audio_status_label(ui, text, FG_MUTED, 60.0);
                    assert_eq!(response.rect.width(), 60.0);
                }
                for text in ["Idle 00:00", "Recording 00:07", "Playing 59:59"] {
                    let response = audio_status_label(ui, text, FG_MUTED, 88.0);
                    assert_eq!(response.rect.width(), 88.0);
                }
            });
        });
    }

    #[test]
    fn receiver_feedback_covers_success_failure_and_transitions() {
        for (state, expected) in [
            (FridayState::Stopped, ("Stopped", FG_MUTED)),
            (FridayState::Starting, ("Starting", FG_WARNING)),
            (FridayState::Listening, ("Listening", FG_SUCCESS)),
            (FridayState::Stopping, ("Stopping", FG_WARNING)),
            (FridayState::Failed, ("Failed", FG_ERROR)),
        ] {
            assert_eq!(receiver_status(state), expected);
        }
    }

    fn waveform_heights(state: RecordingState, levels: &[f32]) -> Vec<f32> {
        let ctx = egui::Context::default();
        let levels = levels.iter().copied().collect();
        let output = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_recording_waveform(ui, &levels, state);
            });
        });
        let color = match state {
            RecordingState::Recording => FG_PRIMARY,
            RecordingState::Paused => FG_WARNING,
            _ => FG_MUTED,
        };
        output
            .shapes
            .into_iter()
            .filter_map(|shape| match shape.shape {
                egui::Shape::Rect(rect) if rect.fill == color => Some(rect.rect.height()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn waveform_renders_live_levels_without_opening_a_menu() {
        let heights = waveform_heights(RecordingState::Recording, &[0.0, 0.5, 1.0]);
        assert_eq!(heights.len(), RECORD_LEVEL_COUNT);
        assert!(heights[RECORD_LEVEL_COUNT - 1] > heights[RECORD_LEVEL_COUNT - 2]);
        assert!(heights[RECORD_LEVEL_COUNT - 2] > heights[0]);
    }

    #[test]
    fn paused_waveform_preserves_levels_and_idle_shows_silence() {
        let levels = [0.2, 0.8, 0.4];
        assert_eq!(
            waveform_heights(RecordingState::Paused, &levels),
            waveform_heights(RecordingState::Recording, &levels),
        );
        let idle = waveform_heights(RecordingState::Idle, &[]);
        assert_eq!(idle.len(), RECORD_LEVEL_COUNT);
        assert!(idle.iter().all(|&height| height == 2.0));
    }
}
