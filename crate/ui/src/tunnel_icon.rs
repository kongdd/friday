//! Icons from `docs/autossh-tunnel.ico`: PE embed (`build.rs`) + pre-rasterized RGBA.

use eframe::egui::IconData;
use std::sync::OnceLock;

const WINDOW_ICON_PX: u32 = 64;
#[cfg(any(target_os = "windows", target_os = "macos"))]
const TRAY_ICON_PX: u32 = 32;

static WINDOW_ICON: OnceLock<IconData> = OnceLock::new();
#[cfg(any(target_os = "windows", target_os = "macos"))]
static TRAY_RGBA: OnceLock<Vec<u8>> = OnceLock::new();

/// Title bar / Alt+Tab (via winit `with_icon`).
pub fn window_icon() -> Option<IconData> {
    Some(
        WINDOW_ICON
            .get_or_init(|| IconData {
                rgba: include_bytes!(concat!(env!("OUT_DIR"), "/window_icon.bin")).to_vec(),
                width: WINDOW_ICON_PX,
                height: WINDOW_ICON_PX,
            })
            .clone(),
    )
}

/// Notification area (same artwork as the exe icon).
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn tray_icon() -> anyhow::Result<tray_icon::Icon> {
    let rgba = TRAY_RGBA
        .get_or_init(|| include_bytes!(concat!(env!("OUT_DIR"), "/tray_icon.bin")).to_vec());
    tray_icon::Icon::from_rgba(rgba.clone(), TRAY_ICON_PX, TRAY_ICON_PX)
        .map_err(|error| anyhow::anyhow!("{error:?}"))
}
