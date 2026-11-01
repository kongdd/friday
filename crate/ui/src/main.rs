//! Native GUI for `autossh-core`: edit the configuration (Connections +
//! Globals) and watch the supervisor runtime log. Backed by `eframe` (egui).
//!
//! Architecture:
//!
//! ```text
//!   ┌────────────────────┐  stderr  ┌────────────────────┐  mpsc  ┌────────────────┐
//!   │ autossh-core (run) ├─────────►│ BufReader::lines    ├───────►│ App.logs Vec   │
//!   └────────────────────┘          └────────────────────┘        └────────────────┘
//! ```
//!
//! The supervisor is spawned once the user clicks "Start supervisor" (or on
//! startup if an attach file exists), and its stderr is colour-coded by
//! severity into the bottom log pane.
//!
//! Release builds on Windows use the `windows` subsystem so no `cmd.exe` flash
//! window appears; debug builds keep the console attached for stderr traces.
//!
//! ## Module layout
//!
//! | Module          | Responsibility                          |
//! |-----------------|------------------------------------------|
//! | `app`           | `AutosshApp` state, rendering, apply     |
//! | `log`           | `Severity` / `LogEntry` / parsing        |
//! | `ssh_config`    | Parse `~/.ssh/config` into host entries  |
//! | `supervisor`    | Spawn child, stream stderr via mpsc      |
//! | `modal`         | Dialog state types and UI functions      |
//! | `hotkey`        | System-wide Friday recording shortcut    |

#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod app;
mod hotkey;
mod log;
mod modal;
mod ssh_config;
mod supervisor;
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod tray;
mod tunnel_icon;

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use eframe::egui::{self, Color32};

use crate::app::AutosshApp;

// ─── entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let config_path = parse_args();
    let app = AutosshApp::load(config_path)?;
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("autossh-core")
        // Compact default: Connections + Keepalive/Friday fit without a tall
        // empty mid band; user can still resize up for long log tails.
        .with_inner_size([680.0, 460.0])
        .with_min_inner_size([560.0, 400.0]);
    if let Some(icon) = tunnel_icon::window_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let native_options = eframe::NativeOptions {
        viewport,
        #[cfg(target_os = "macos")]
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            builder.with_activation_policy(ActivationPolicy::Accessory);
        })),
        ..Default::default()
    };
    eframe::run_native(
        "autossh-ui",
        native_options,
        Box::new(|cc| {
            install_system_fonts(&cc.egui_ctx);
            cc.egui_ctx.set_visuals(visuals());
            enlarge_fonts(&cc.egui_ctx);
            let mut app = app;
            app.install_record_hotkey(&cc.egui_ctx);
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            app.install_tray(&cc.egui_ctx).map_err(|error| {
                Box::<dyn std::error::Error + Send + Sync>::from(format!(
                    "cannot create tray icon: {error:#}"
                ))
            })?;
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe exited with {e:?}"))?;
    Ok(())
}

// ─── CLI argument parsing ─────────────────────────────────────────────────────

fn parse_args() -> PathBuf {
    let mut args = std::env::args().skip(1);
    let mut path = default_path();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" | "--config" => {
                path = PathBuf::from(args.next().expect("missing value for --config"));
            }
            "-h" | "--help" => {
                eprintln!("usage: autossh-ui [--config PATH]");
                std::process::exit(0);
            }
            _ => {}
        }
    }
    path
}

/// Delegates to `autossh_core::default_config_path` so CLI and GUI share the
/// same resolution: `<exe-dir>/config.toml` > `~/.config/autossh/config.toml`.
pub fn default_path() -> PathBuf {
    autossh_core::default_config_path()
}

// ─── fonts and theme ──────────────────────────────────────────────────────────

/// eframe ships portable default fonts rather than using the Windows font
/// fallback chain. Register the Windows system fonts explicitly so that:
///   * UI glyphs (`●`, `✓`, `✎`, `＋`) do not turn into tofu boxes, and
///   * CJK characters in connection names / hosts render correctly without
///     the user having to install a separate font.
#[cfg(target_os = "windows")]
fn install_system_fonts(ctx: &egui::Context) {
    use eframe::egui::{FontData, FontDefinitions, FontFamily};

    let font_dir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts");
    let mut fonts = FontDefinitions::default();

    // (font_name, file, position) — first=true inserts at index 0 (highest priority),
    // first=false appends as a fallback. Symbol + Emoji + a CJK font cover the
    // glyphs that show up in practice; msyh.ttc is present on every Windows 7+
    // install and contains Microsoft YaHei UI plus the UI variant.
    for (name, filename, first) in [
        ("Segoe UI Symbol", "seguisym.ttf", true),
        ("Segoe UI Emoji", "seguiemj.ttf", false),
        ("Microsoft YaHei UI", "msyh.ttc", false),
    ] {
        let Ok(bytes) = std::fs::read(font_dir.join(filename)) else {
            continue;
        };
        fonts
            .font_data
            .insert(name.to_owned(), FontData::from_owned(bytes).into());
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            let family_fonts = fonts.families.entry(family).or_default();
            if first {
                family_fonts.insert(0, name.to_owned());
            } else {
                family_fonts.push(name.to_owned());
            }
        }
    }
    ctx.set_fonts(fonts);
}

/// Add a system CJK font as a fallback without changing the default UI font.
#[cfg(not(target_os = "windows"))]
fn install_system_fonts(ctx: &egui::Context) {
    use eframe::egui::{FontData, FontDefinitions, FontFamily};

    #[cfg(target_os = "macos")]
    let path = Some(PathBuf::from("/System/Library/Fonts/Hiragino Sans GB.ttc"));
    #[cfg(not(target_os = "macos"))]
    let path = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", ":lang=zh-cn"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);

    if let Some(bytes) = path.and_then(|path| std::fs::read(path).ok()) {
        let mut fonts = FontDefinitions::default();
        let name = "system-cjk".to_owned();
        let font = FontData::from_owned(bytes);
        // Hiragino's glyphs sit higher than egui's default Latin font.
        #[cfg(target_os = "macos")]
        let font = font.tweak(egui::epaint::text::FontTweak {
            y_offset_factor: 0.20,
            ..Default::default()
        });
        fonts.font_data.insert(name.clone(), font.into());
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push(name.clone());
        }
        ctx.set_fonts(fonts);
    }
}

/// The compact window uses slightly larger type so labels stay readable.
fn enlarge_fonts(ctx: &egui::Context) {
    const SCALE: f32 = 1.2;
    ctx.style_mut(|style| {
        for font_id in style.text_styles.values_mut() {
            font_id.size *= SCALE;
        }
    });
}

/// Visuals tuned for a palette that keeps colour-coded log badges legible
/// without sinking the whole window into near-black.
fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    let visuals = &mut v;
    visuals.override_text_color = Some(Color32::from_rgb(235, 238, 245));
    visuals.window_corner_radius = egui::CornerRadius::same(6);
    // Softer dark palette so connection entries / globals cards stay legible
    // without sinking the whole window into near-black.
    visuals.window_fill = Color32::from_rgb(37, 42, 51);
    visuals.panel_fill = Color32::from_rgb(37, 42, 51);
    visuals.extreme_bg_color = Color32::from_rgb(27, 31, 38);
    visuals.faint_bg_color = Color32::from_rgb(58, 66, 82);
    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(46, 52, 65);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(60, 68, 88);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(80, 90, 114);
    visuals.widgets.active.bg_fill = Color32::from_rgb(100, 112, 138);
    visuals.selection.bg_fill = Color32::from_rgb(30, 128, 168);
    visuals.selection.stroke = egui::Stroke::new(1.0, Color32::from_rgb(34, 214, 226));
    visuals.hyperlink_color = Color32::from_rgb(34, 214, 226);
    v
}
