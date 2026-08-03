//! Menu-bar presence + background-service lifecycle.
//!
//! The app lives primarily in the menu bar: closing the main window hides it
//! (the process, hotkeys, and overlay keep running), and on macOS the Dock
//! icon disappears while hidden (Accessory activation policy) and returns
//! when the window is reopened. Quit is explicit, from the tray menu.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager};

const STATUS_ID: &str = "status";
const TOGGLE_ID: &str = "toggle";
const OPEN_ID: &str = "open";
const QUIT_ID: &str = "quit";

/// Handle for updating the tray from pipeline events.
pub struct Tray {
    icon: TrayIcon,
    status: MenuItem<tauri::Wry>,
    toggle: MenuItem<tauri::Wry>,
}

// MenuItem/TrayIcon are managed on the main thread; we only touch them via
// Tauri's main-thread event loop.
unsafe impl Send for Tray {}
unsafe impl Sync for Tray {}

impl Tray {
    /// Reflect pipeline state in the menu ("idle" / "listening" / …).
    pub fn set_status(&self, state: &str) {
        let label = match state {
            "listening" => "● Listening",
            "speech" => "● Hearing speech…",
            "transcribing" => "● Transcribing…",
            "error" => "▲ Error — open window",
            _ => "○ Idle",
        };
        let _ = self.status.set_text(label);
        let _ = self
            .toggle
            .set_text(if state == "stopped" || state == "error" {
                "Start listening"
            } else {
                "Stop listening"
            });
        let _ = self.icon.set_tooltip(Some(format!("OpenRouter Voice — {label}")));
    }
}

pub fn show_main_window(app: &AppHandle) {
    // Back to a regular app (Dock icon) while the window is visible.
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn hide_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    // Background service: drop out of the Dock/App Switcher while hidden.
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, STATUS_ID, "○ Idle", false, None::<&str>)?;
    let toggle = MenuItem::with_id(app, TOGGLE_ID, "Start listening", true, None::<&str>)?;
    let open = MenuItem::with_id(app, OPEN_ID, "Open OpenRouter Voice", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT_ID, "Quit", true, Some("Cmd+Q"))?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &toggle,
            &open,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    // Dedicated template icon (black + alpha): macOS recolors it for
    // light/dark menu bars. The full-color app icon would render as a blob.
    let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
    let icon = TrayIconBuilder::with_id("main-tray")
        .icon(tray_icon)
        .icon_as_template(true) // adapts to light/dark menu bar on macOS
        .tooltip("OpenRouter Voice")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            TOGGLE_ID => crate::pipeline::toggle(app),
            OPEN_ID => show_main_window(app),
            QUIT_ID => app.exit(0),
            _ => {}
        })
        .build(app)?;

    app.manage(Tray {
        icon,
        status,
        toggle,
    });
    Ok(())
}
