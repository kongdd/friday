//! Centre pane: keepalive/retry editor and Save / Start All / Stop All actions.

use eframe::egui::{self, Color32, RichText};

use crate::log::{FG_DIM, FG_ERROR, FG_MUTED, FG_PRIMARY, FG_SUCCESS, FG_WARNING};
use crate::modal::{GlobalGroup, Modal};
use friday::{FridayState, LISTEN_ADDR, RecordingState};

use super::{AutosshApp, RECORD_LEVEL_COUNT};

#[derive(Clone, Copy)]
enum RecorderAction {
    Start,
    Pause,
    Resume,
    Finish,
    Play,
    StopPlayback,
}

fn render_recording_waveform(
    ui: &mut egui::Ui,
    levels: &std::collections::VecDeque<f32>,
    active: bool,
) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::hover());
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
        ui.painter().rect_filled(
            bar,
            width / 2.0,
            if active { FG_PRIMARY } else { FG_WARNING },
        );
    }

    if active {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }
}

impl AutosshApp {
    pub fn render_centre_panel(&mut self, root: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(root, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("💾  Save").clicked() {
                        self.save();
                    }
                    let all_running = self.supervisor_running();
                    let all_label = if all_running {
                        "■  Stop All"
                    } else {
                        "▶  Start All"
                    };
                    if ui.button(all_label).clicked() {
                        if all_running {
                            self.stop_supervisor();
                        } else {
                            self.start_supervisor();
                        }
                    }
                });
            });
            ui.separator();

            // Pack Keepalive/Retry + Friday at the top of the centre column.
            // Do not force a fixed max_height (clips Friday) or bottom-pin Friday
            // (opens a dead band between the two blocks).
            egui::ScrollArea::vertical()
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    self.render_globals(ui);
                    ui.add_space(6.0);
                    self.render_friday(ui);
                });
        });
    }

    fn render_globals(&mut self, ui: &mut egui::Ui) {
        let ka = self.keepalive();
        let r = self.retry();
        let mut sel = self.selected_global.min(5);

        // (display, edit): display keeps the human-readable suffix for the
        // readout; edit is the raw number so the EditGlobal modal can parse it
        // back into a u64. Mixing the two is the bug that turned "30 s" into
        // an un-parseable initial value.
        let keepalive: [(usize, GlobalGroup, (String, String)); 3] = [
            (
                0,
                GlobalGroup::KeepaliveInterval,
                (format!("{} s", ka.interval), ka.interval.to_string()),
            ),
            (
                1,
                GlobalGroup::KeepaliveCount,
                (ka.count_max.to_string(), ka.count_max.to_string()),
            ),
            (
                2,
                GlobalGroup::KeepaliveTimeout,
                (
                    format!("{} s", ka.connect_timeout),
                    ka.connect_timeout.to_string(),
                ),
            ),
        ];
        let retry: [(usize, GlobalGroup, (String, String)); 3] = [
            (
                3,
                GlobalGroup::RetryInitial,
                (
                    format!("{} s", r.initial_seconds),
                    r.initial_seconds.to_string(),
                ),
            ),
            (
                4,
                GlobalGroup::RetryMaximum,
                (
                    format!("{} s", r.maximum_seconds),
                    r.maximum_seconds.to_string(),
                ),
            ),
            (
                5,
                GlobalGroup::RetryStable,
                (
                    format!("{} s", r.stable_seconds),
                    r.stable_seconds.to_string(),
                ),
            ),
        ];

        ui.add_space(4.0);
        ui.columns(2, |cols| {
            for (i, rows) in [&keepalive, &retry].iter().enumerate() {
                cols[i].group(|ui| {
                    ui.add_space(2.0);
                    ui.heading(if i == 0 { "Keepalive" } else { "Retry" });
                    ui.add_space(2.0);
                    for (idx, group, (display, edit)) in rows.iter() {
                        self.render_global_row(
                            ui,
                            *idx,
                            &mut sel,
                            *group,
                            display.clone(),
                            edit.clone(),
                        );
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

    fn render_friday(&mut self, ui: &mut egui::Ui) {
        let state = self.friday.state();
        let player = self.friday.player().map(str::to_owned);
        let receiver_error = self.friday.error().map(str::to_owned);
        let record_state = self.recorder.state();
        let record_error = self.recorder.error().map(str::to_owned);
        let hotkey_error = self.record_hotkey.error().map(str::to_owned);
        let record_file = self
            .recorder
            .path()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned());
        let is_playing = self.recorder.is_playing();
        let elapsed = self.recorder.elapsed().as_secs();
        let (status, color) = match state {
            FridayState::Starting => ("starting", FG_WARNING),
            FridayState::Listening => ("listening", FG_SUCCESS),
            FridayState::Stopping => ("stopping", FG_WARNING),
            FridayState::Stopped => ("stopped", FG_MUTED),
            FridayState::Failed => ("failed", FG_ERROR),
        };
        let (record_status, record_color) = match record_state {
            RecordingState::Idle => ("idle", FG_MUTED),
            RecordingState::Recording => ("recording", FG_ERROR),
            RecordingState::Paused => ("paused", FG_WARNING),
            RecordingState::Finished => ("finished", FG_SUCCESS),
            RecordingState::Failed => ("failed", FG_ERROR),
        };

        let (receiver_action, recorder_action) = ui
            .group(|ui| {
                ui.set_width(ui.available_width());
                let receiver_action = ui
                    .horizontal(|ui| {
                        ui.heading("Friday voice");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let action = match state {
                                FridayState::Stopped | FridayState::Failed => ui
                                    .small_button("▶  Start receiver")
                                    .clicked()
                                    .then_some(true),
                                FridayState::Listening => ui
                                    .small_button("■  Stop receiver")
                                    .clicked()
                                    .then_some(false),
                                FridayState::Starting => {
                                    ui.add_enabled(false, egui::Button::new("Starting…").small());
                                    None
                                }
                                FridayState::Stopping => {
                                    ui.add_enabled(false, egui::Button::new("Stopping…").small());
                                    None
                                }
                            };
                            ui.label(RichText::new(format!("● {status}")).strong().color(color));
                            action
                        })
                        .inner
                    })
                    .inner;
                ui.label(
                    RichText::new(format!("Receiver: http://{LISTEN_ADDR}/speak"))
                        .monospace()
                        .color(FG_PRIMARY),
                );
                if let Some(player) = player {
                    ui.label(
                        RichText::new(format!("player: {player}"))
                            .small()
                            .color(FG_MUTED),
                    );
                }
                if let Some(error) = receiver_error {
                    ui.label(RichText::new(error).small().color(FG_ERROR));
                }

                ui.separator();
                let recorder_action = ui
                    .horizontal(|ui| {
                        ui.label(RichText::new("Microphone").strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let action = match record_state {
                                RecordingState::Idle | RecordingState::Failed => ui
                                    .small_button("●  Record")
                                    .clicked()
                                    .then_some(RecorderAction::Start),
                                RecordingState::Recording => {
                                    if ui.small_button("■  Finish").clicked() {
                                        Some(RecorderAction::Finish)
                                    } else if ui.small_button("Ⅱ  Pause").clicked() {
                                        Some(RecorderAction::Pause)
                                    } else {
                                        None
                                    }
                                }
                                RecordingState::Paused => {
                                    if ui.small_button("■  Finish").clicked() {
                                        Some(RecorderAction::Finish)
                                    } else if ui.small_button("▶  Continue").clicked() {
                                        Some(RecorderAction::Resume)
                                    } else {
                                        None
                                    }
                                }
                                RecordingState::Finished => {
                                    if ui.small_button("●  Record new").clicked() {
                                        Some(RecorderAction::Start)
                                    } else if is_playing
                                        && ui.small_button("■  Stop playback").clicked()
                                    {
                                        Some(RecorderAction::StopPlayback)
                                    } else if !is_playing
                                        && ui.small_button("▶  Playback").clicked()
                                    {
                                        Some(RecorderAction::Play)
                                    } else {
                                        None
                                    }
                                }
                            };
                            ui.label(
                                RichText::new(format!(
                                    "● {record_status}  {:02}:{:02}",
                                    elapsed / 60,
                                    elapsed % 60
                                ))
                                .strong()
                                .color(record_color),
                            );
                            action
                        })
                        .inner
                    })
                    .inner;
                if matches!(
                    record_state,
                    RecordingState::Recording | RecordingState::Paused
                ) {
                    render_recording_waveform(
                        ui,
                        &self.record_levels,
                        record_state == RecordingState::Recording,
                    );
                } else if let Some(file) = record_file {
                    ui.label(
                        RichText::new(format!("Saved: media/{file}"))
                            .small()
                            .monospace()
                            .color(FG_SUCCESS),
                    );
                }
                if let Some(error) = record_error {
                    ui.label(RichText::new(error).small().color(FG_ERROR));
                }
                if let Some(error) = hotkey_error {
                    ui.label(RichText::new(error).small().color(FG_ERROR));
                } else if matches!(record_state, RecordingState::Idle | RecordingState::Failed) {
                    ui.label(
                        RichText::new("F8: record / finish; WAV is saved in media/.")
                            .small()
                            .color(FG_MUTED),
                    );
                }
                (receiver_action, recorder_action)
            })
            .inner;

        match receiver_action {
            Some(true) => self.friday.start(),
            Some(false) => self.friday.stop(),
            None => {}
        }
        let result = match recorder_action {
            Some(RecorderAction::Start) => self.recorder.start(),
            Some(RecorderAction::Pause) => self.recorder.pause(),
            Some(RecorderAction::Resume) => self.recorder.resume(),
            Some(RecorderAction::Finish) => self.recorder.finish(),
            Some(RecorderAction::Play) => self.recorder.play(),
            Some(RecorderAction::StopPlayback) => {
                self.recorder.stop_playback();
                Ok(())
            }
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
        display: String,
        edit: String,
    ) {
        let is_sel = *selected == index;
        let (fill, sw, sc) = if is_sel {
            (Color32::from_rgb(34, 56, 70), 1.0, FG_PRIMARY)
        } else {
            (Color32::from_rgb(24, 28, 34), 0.5, FG_DIM)
        };
        let response = egui::Frame::group(ui.style())
            .fill(fill)
            .stroke(egui::Stroke::new(sw, sc))
            .corner_radius(egui::CornerRadius::same(4))
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical(|ui| {
                    ui.label(RichText::new(group.label()).color(FG_MUTED).small());
                    ui.label(
                        RichText::new(&display)
                            .strong()
                            .color(FG_PRIMARY)
                            .monospace(),
                    );
                });
            });
        let interact = response.response.interact(egui::Sense::click());
        if interact.clicked() {
            *selected = index;
        }
        if interact.double_clicked() {
            *selected = index;
            self.modal = Modal::EditGlobal { group, value: edit };
        }
    }
}
