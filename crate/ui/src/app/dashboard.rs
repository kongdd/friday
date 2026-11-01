//! Bottom status bar: save state on the left, counters and supervisor actions on the right.

use eframe::egui::{self, RichText};

use crate::log::{FG_ERROR, FG_SUCCESS, FG_WARNING};

use super::AutosshApp;

impl AutosshApp {
    pub fn render_dashboard(&mut self, root: &mut egui::Ui) {
        egui::Panel::bottom("dashboard").show_inside(root, |ui| {
            let muted = ui.visuals().weak_text_color();
            let (status, color) = match self.supervisor.as_ref() {
                None => ("Stopped", muted),
                Some(handle) if handle.alive() => ("Running", FG_SUCCESS),
                Some(_) => ("Exited", FG_ERROR),
            };
            ui.horizontal(|ui| {
                // Config path plus save state fill the bottom-left corner.
                let path = self.config_path.display().to_string();
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "{path} · {}",
                            if self.dirty { "Unsaved" } else { "Saved" }
                        ))
                        .small()
                        .color(if self.dirty {
                            FG_WARNING
                        } else {
                            muted
                        }),
                    )
                    .truncate(),
                )
                .on_hover_text(&path);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(6.0);
                    let running = self.supervisor_running();
                    let label = if running { "Stop all" } else { "Start all" };
                    if ui.button(label).clicked() {
                        if running {
                            self.stop_supervisor();
                        } else {
                            self.start_supervisor();
                        }
                    }
                    ui.label(RichText::new(format!("SSH: {status}")).small().color(color));
                    ui.separator();
                    ui.label(
                        RichText::new(format!("logs: {}", self.logs.len()))
                            .small()
                            .color(muted),
                    );
                    ui.separator();
                    ui.label(
                        RichText::new(format!("conns: {}", self.config.connections.len()))
                            .small()
                            .color(muted),
                    );
                    if let Some((text, _)) = self.msg.as_ref() {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(text).small().color(muted))
                                    .truncate(),
                            )
                            .on_hover_text(self.config_path.display().to_string());
                        });
                    }
                });
            });
        });
    }
}
