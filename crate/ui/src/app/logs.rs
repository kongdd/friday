//! Bottom tail-style log stream with severity badges.

use eframe::egui::{self, RichText};

use crate::log::{FG_DIM, FG_MUTED, LOG_BUFFER_LIMIT, format_unix_ts};

use super::AutosshApp;
use super::icons::{self, Icon};

impl AutosshApp {
    pub fn render_logs_panel(&mut self, root: &mut egui::Ui) {
        let collapsed = self.logs_collapsed;
        let mut panel = egui::Panel::bottom("logs");
        if collapsed {
            // Collapsed: keep only the heading row.
            panel = panel.exact_height(30.0);
        } else {
            // Compact by default; drag the splitter when a long tail is needed.
            panel = panel
                .resizable(true)
                .default_size(210.0)
                .size_range(160.0..=360.0);
        }
        panel.show_inside(root, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                if !collapsed {
                    ui.add_space(3.0);
                }
                let mut heading_row = |ui: &mut egui::Ui| {
                    ui.add_space(8.0);
                    let marker = icons::collapse_button(ui, collapsed).on_hover_text(if collapsed {
                        "Expand logs"
                    } else {
                        "Collapse logs"
                    });
                    let heading = ui.add(
                        egui::Label::new(RichText::new("Logs").heading())
                            .sense(egui::Sense::click()),
                    );
                    if marker.clicked() || heading.clicked() {
                        self.logs_collapsed = !collapsed;
                    }
                    if collapsed {
                        if !self.logs.is_empty() {
                            ui.label(
                                RichText::new(format!("{} lines", self.logs.len()))
                                    .small()
                                    .color(FG_MUTED),
                            );
                        }
                        return;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("clear").clicked() {
                            self.logs.clear();
                        }
                        ui.separator();
                        if ui.checkbox(&mut self.log_scroll.follow, "follow").changed()
                            && self.log_scroll.follow
                        {
                            self.log_scroll.offset_from_bottom = 0;
                        }
                    });
                };
                if collapsed {
                    ui.allocate_ui_with_layout(
                        ui.available_size(),
                        egui::Layout::left_to_right(egui::Align::Center),
                        &mut heading_row,
                    );
                    return;
                }
                ui.horizontal(heading_row);
                super::row_separator(ui);
                egui::ScrollArea::vertical()
                    .stick_to_bottom(self.log_scroll.follow)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.logs.is_empty() {
                            ui.add_space(20.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new(
                                        "no log lines yet — start the supervisor to see its output here",
                                    )
                                    .small()
                                    .color(FG_MUTED),
                                );
                            });
                            return;
                        }
                        let total = self.logs.len();
                        let viewport_h = ui.available_height().max(1.0) as usize;
                        let start = if self.log_scroll.follow || total <= viewport_h {
                            total.saturating_sub(viewport_h)
                        } else {
                            let bottom = total.saturating_sub(self.log_scroll.offset_from_bottom);
                            bottom.saturating_sub(viewport_h)
                        };
                        if self.logs.len() > LOG_BUFFER_LIMIT - 100 {
                            ui.label(
                                RichText::new(format!(
                                    " … {} earlier lines dropped … ",
                                    self.logs.len()
                                ))
                                .small()
                                .color(FG_DIM),
                            );
                        }
                        for entry in &self.logs[start..total] {
                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                let time = entry
                                    .ts_secs
                                    .map(format_unix_ts)
                                    .unwrap_or_else(|| "        ".to_string());
                                ui.label(
                                    RichText::new(format!("{time} "))
                                        .monospace()
                                        .color(ui.visuals().weak_text_color()),
                                );
                                // severity badge
                                let (badge_text, badge_bg) = (
                                    format!(" {} ", entry.severity.label()),
                                    entry.severity.badge(),
                                );
                                let badge_fg = entry.severity.foreground();
                                let event_color = entry.event_color();
                                egui::Frame::group(ui.style())
                                    .fill(badge_bg)
                                    .corner_radius(egui::CornerRadius::same(3))
                                    .inner_margin(egui::Margin::same(0))
                                    .show(ui, |ui| {
                                        ui.label(
                                            RichText::new(badge_text)
                                                .strong()
                                                .monospace()
                                                .color(badge_fg),
                                        );
                                    });
                                ui.add_space(4.0);
                                if let Some(conn) = &entry.connection {
                                    ui.label(
                                        RichText::new(conn.clone())
                                            .strong()
                                            .color(event_color),
                                    );
                                    ui.label(
                                        RichText::new(format!(": {}", entry.message))
                                            .color(event_color),
                                    );
                                } else {
                                    ui.label(RichText::new(&entry.message).color(event_color));
                                }
                            });
                        }
                    });
            });
    }
}
