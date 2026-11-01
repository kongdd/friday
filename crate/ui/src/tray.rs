//! System-tray integration.
//!
//! Tray callbacks may run while the native window is hidden. They therefore
//! wake egui explicitly after forwarding a small command to the app thread.

use std::sync::mpsc::{self, Receiver};

use crate::tunnel_icon;
use eframe::egui;
use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

const MENU_SHOW: &str = "autossh-show";
const MENU_RELOAD: &str = "autossh-reload";
const MENU_EXIT: &str = "autossh-exit";

#[derive(Clone, Copy, Debug)]
pub enum TrayCommand {
    Show,
    Reload,
    Exit,
    #[cfg(target_os = "macos")]
    Menu,
}

pub struct AppTray {
    // The icon disappears when the final TrayIcon handle is dropped.
    _icon: TrayIcon,
    #[cfg(target_os = "macos")]
    menu: Menu,
    commands: Receiver<TrayCommand>,
}

impl AppTray {
    pub fn new(ctx: &egui::Context) -> anyhow::Result<Self> {
        let show = MenuItem::with_id(MENU_SHOW, "Open", true, None);
        let reload = MenuItem::with_id(MENU_RELOAD, "Reload", true, None);
        let separator = PredefinedMenuItem::separator();
        let exit = MenuItem::with_id(MENU_EXIT, "Exit", true, None);
        let menu = Menu::with_items(&[&show, &reload, &separator, &exit])?;

        let (sender, commands) = mpsc::channel();

        let menu_sender = sender.clone();
        let menu_ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let command = match event.id.as_ref() {
                MENU_SHOW => Some(TrayCommand::Show),
                MENU_RELOAD => Some(TrayCommand::Reload),
                MENU_EXIT => Some(TrayCommand::Exit),
                _ => None,
            };
            if let Some(command) = command {
                let _ = menu_sender.send(command);
                menu_ctx.request_repaint();
            }
        }));

        let tray_ctx = ctx.clone();
        #[cfg(target_os = "macos")]
        let last_click = std::sync::Mutex::new(None::<std::time::Instant>);
        tray_icon::TrayIconEvent::set_event_handler(Some(
            move |event: tray_icon::TrayIconEvent| {
                use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
                #[cfg(target_os = "macos")]
                if matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Right,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                ) {
                    let _ = sender.send(TrayCommand::Menu);
                    tray_ctx.request_repaint();
                    return;
                }
                let left_click = matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                );
                // Windows does not receive double-click events from tray-icon's
                // window class; keep its existing single-click behavior.
                #[cfg(target_os = "windows")]
                let show_window = left_click;
                // macOS emits individual clicks, not DoubleClick events.
                #[cfg(target_os = "macos")]
                let show_window = left_click && {
                    let mut last = last_click.lock().unwrap();
                    let now = std::time::Instant::now();
                    let double =
                        last.is_some_and(|time| now.duration_since(time).as_millis() <= 500);
                    *last = if double { None } else { Some(now) };
                    double
                };
                if show_window {
                    let _ = sender.send(TrayCommand::Show);
                    tray_ctx.request_repaint();
                }
            },
        ));

        let builder = TrayIconBuilder::new()
            .with_tooltip("autossh-core")
            .with_icon(tunnel_icon::tray_icon()?)
            .with_menu_on_left_click(false);
        #[cfg(target_os = "windows")]
        let builder = builder.with_menu(Box::new(menu));
        let icon = builder.build()?;

        Ok(Self {
            _icon: icon,
            #[cfg(target_os = "macos")]
            menu,
            commands,
        })
    }

    #[cfg(target_os = "macos")]
    pub fn show_menu(&mut self) {
        // An attached NSStatusItem menu intercepts left clicks, so attach it
        // only while presenting the right-click menu.
        self._icon.set_menu(Some(Box::new(self.menu.clone())));
        self._icon.show_menu();
        self._icon.set_menu(None);
    }

    pub fn try_recv(&self) -> Option<TrayCommand> {
        self.commands.try_recv().ok()
    }
}
