//! Full-width connections panel — checkbox-list of every connection with
//! inline start/stop, single-click select, and double-click edit.

use eframe::egui::{self, RichText};

use crate::log::{FG_DIM, FG_MUTED, FG_SUCCESS};
use crate::modal::{Modal, state_from_connection};

use super::AutosshApp;

impl AutosshApp {
    pub fn render_connections_panel(&mut self, root: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(root, |ui| {
            if self.config.connections.is_empty() {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("(no connections)").color(FG_MUTED));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Choose Add from the menu to create a connection")
                            .small()
                            .color(FG_DIM),
                    );
                });
                return;
            }

            let mut toggle_at: Option<usize> = None;
            let mut edit_at: Option<usize> = None;
            let supervisor_running = self.supervisor_running();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (i, conn) in self.config.connections.iter().enumerate() {
                        let sel = i == self.selected_connection;
                        let (fill, stroke, accent, muted) = super::card_style(ui, sel);
                        egui::Frame::group(ui.style())
                            .fill(fill)
                            .stroke(stroke)
                            .corner_radius(egui::CornerRadius::same(4))
                            .inner_margin(egui::Margin::symmetric(10, 8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    // checkbox
                                    let mut cb = self.checked_conn.contains(&i);
                                    if ui
                                        .checkbox(&mut cb, "")
                                        .on_hover_text("Select for batch deletion")
                                        .changed()
                                    {
                                        if cb {
                                            self.checked_conn.insert(i);
                                        } else {
                                            self.checked_conn.remove(&i);
                                        }
                                    }
                                    // status dot
                                    let running = supervisor_running && conn.enabled;
                                    ui.colored_label(if running { FG_SUCCESS } else { muted }, "●");
                                    // clickable details (select on click, edit on double-click)
                                    let mut mk = |text: RichText| {
                                        ui.add(egui::Label::new(text).sense(egui::Sense::click()))
                                    };
                                    let r1 = mk(RichText::new(&conn.name).strong());
                                    let destination = match conn.port {
                                        Some(port) => format!("{}:{port}", conn.destination()),
                                        None => conn.destination(),
                                    };
                                    let details = match conn.description.as_deref() {
                                        Some(description) if !description.trim().is_empty() => {
                                            format!("{destination}  ·  {description}")
                                        }
                                        _ => destination,
                                    };
                                    let r2 = mk(RichText::new(details).small().color(muted));
                                    let r3 = mk(RichText::new(format!(
                                        "{} forwards",
                                        conn.forwards.len()
                                    ))
                                    .small()
                                    .color(accent));
                                    let clicked = r1.clicked() || r2.clicked() || r3.clicked();
                                    let dbl = r1.double_clicked()
                                        || r2.double_clicked()
                                        || r3.double_clicked();
                                    if clicked {
                                        self.selected_connection = i;
                                    }
                                    if dbl {
                                        edit_at = Some(i);
                                    }

                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let label =
                                                if running { "■  Stop" } else { "▶  Start" };
                                            if ui.small_button(label).clicked() {
                                                toggle_at = Some(i);
                                            }
                                        },
                                    );
                                });
                            });
                        ui.add_space(4.0);
                    }
                });

            if let Some(i) = toggle_at {
                self.toggle_connection(i);
            }
            if let Some(i) = edit_at {
                let conn = self.config.connections[i].clone();
                self.selected_connection = i;
                self.modal = Modal::EditConnection {
                    idx: i,
                    state: state_from_connection(&conn),
                };
            }
        });
    }

    pub fn delete_checked_connections(&mut self) {
        let mut indices: Vec<usize> = self.checked_conn.iter().copied().collect();
        indices.sort_unstable_by(|a, b| b.cmp(a)); // descending
        let count = indices.len();
        for i in indices {
            self.config.connections.remove(i);
        }
        self.checked_conn.clear();
        self.dirty = true;
        if self.selected_connection >= self.config.connections.len() && self.selected_connection > 0
        {
            self.selected_connection = self.config.connections.len().saturating_sub(1);
        }
        self.flash(format!("deleted {count} connection(s)"));
    }
}
