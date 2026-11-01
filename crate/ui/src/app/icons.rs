//! Hand-painted icons.
//!
//! Platform font fallbacks render some symbols as tofu boxes, so every glyph
//! used by the toolbar is drawn with the painter instead of a font character.
//! The palette (speaker, microphone, pause, play, stop, folder, chevrons) is
//! shared by the audio row and the collapsible Logs heading.

use eframe::egui;

#[derive(Clone, Copy)]
pub enum Icon {
    Speaker,
    Microphone,
    Pause,
    Play,
    Stop,
    Folder,
    ChevronDown,
    ChevronRight,
}

/// Draw `icon` centred on `center`.
fn paint(painter: &egui::Painter, center: egui::Pos2, stroke: egui::Stroke, icon: Icon) {
    let poly = |points: &[(f32, f32)]| {
        points
            .iter()
            .map(|&(x, y)| center + egui::vec2(x, y))
            .collect::<Vec<_>>()
    };
    match icon {
        Icon::Speaker => {
            painter.add(egui::Shape::closed_line(
                poly(&[
                    (-7.0, -3.0),
                    (-4.0, -3.0),
                    (0.0, -7.0),
                    (0.0, 7.0),
                    (-4.0, 3.0),
                    (-7.0, 3.0),
                ]),
                stroke,
            ));
            painter.add(egui::Shape::line(
                poly(&[(3.0, -5.0), (6.0, -2.0), (6.0, 2.0), (3.0, 5.0)]),
                stroke,
            ));
        }
        Icon::Microphone => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center + egui::vec2(0.0, -3.0), egui::vec2(6.0, 10.0)),
                3.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.add(egui::Shape::line(
                poly(&[
                    (-6.0, -2.0),
                    (-6.0, 2.0),
                    (-3.0, 5.0),
                    (3.0, 5.0),
                    (6.0, 2.0),
                    (6.0, -2.0),
                ]),
                stroke,
            ));
            painter.line_segment(
                [center + egui::vec2(0.0, 5.0), center + egui::vec2(0.0, 9.0)],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-4.0, 9.0),
                    center + egui::vec2(4.0, 9.0),
                ],
                stroke,
            );
        }
        Icon::Pause => {
            for x in [-3.0, 3.0] {
                painter.rect_filled(
                    egui::Rect::from_center_size(
                        center + egui::vec2(x, 0.0),
                        egui::vec2(3.0, 12.0),
                    ),
                    0.5,
                    stroke.color,
                );
            }
        }
        Icon::Play => {
            painter.add(egui::Shape::convex_polygon(
                poly(&[(-4.0, -6.0), (6.0, 0.0), (-4.0, 6.0)]),
                stroke.color,
                egui::Stroke::NONE,
            ));
        }
        Icon::Stop => {
            painter.rect_filled(
                egui::Rect::from_center_size(center, egui::vec2(10.0, 10.0)),
                1.0,
                stroke.color,
            );
        }
        Icon::Folder => {
            painter.add(egui::Shape::closed_line(
                poly(&[
                    (-7.0, -5.0),
                    (-2.0, -5.0),
                    (0.0, -3.0),
                    (7.0, -3.0),
                    (7.0, 6.0),
                    (-7.0, 6.0),
                ]),
                stroke,
            ));
        }
        Icon::ChevronDown => {
            painter.add(egui::Shape::convex_polygon(
                poly(&[(-6.0, -3.0), (6.0, -3.0), (0.0, 5.0)]),
                stroke.color,
                egui::Stroke::NONE,
            ));
        }
        Icon::ChevronRight => {
            painter.add(egui::Shape::convex_polygon(
                poly(&[(-3.0, -6.0), (-3.0, 6.0), (5.0, 0.0)]),
                stroke.color,
                egui::Stroke::NONE,
            ));
        }
    }
}

/// Fixed-size (24x22) icon button for the toolbar rows.
pub fn icon_button(ui: &mut egui::Ui, icon: Icon, enabled: bool, tooltip: &str) -> egui::Response {
    let response = ui
        .add_enabled_ui(enabled, |ui| {
            ui.add_sized([24.0, 22.0], egui::Button::new(""))
        })
        .inner;
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tooltip));
    let mut stroke = ui.style().interact(&response).fg_stroke;
    if !response.enabled() {
        stroke.color = ui.visuals().weak_text_color();
    }
    paint(ui.painter(), response.rect.center(), stroke, icon);
    response.on_hover_text(tooltip)
}

/// Triangle toggle for the Logs heading, sized to the heading line so the
/// marker stays on the text centre line.
pub fn collapse_button(ui: &mut egui::Ui, collapsed: bool) -> egui::Response {
    let height = ui.text_style_height(&egui::TextStyle::Heading);
    let response = ui.add(
        egui::Button::new("")
            .min_size(egui::vec2(18.0, height))
            .frame(false),
    );
    let icon = if collapsed {
        Icon::ChevronRight
    } else {
        Icon::ChevronDown
    };
    paint(
        ui.painter(),
        response.rect.center(),
        ui.style().interact(&response).fg_stroke,
        icon,
    );
    response
}
