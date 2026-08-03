//! Floating dictation indicator (Whisper Flow-style).
//!
//! A small transparent, always-on-top, click-through window pinned near the
//! bottom-center of the screen. It appears while the mic is live, pulses
//! while you talk, and shows the speech stream (partials for streaming
//! models, finished utterances otherwise). All content is rendered by
//! overlay.html; this module only positions and shows/hides the window.

use tauri::{AppHandle, LogicalPosition, Manager};

const OVERLAY_W: f64 = 460.0;
const MARGIN_BOTTOM: f64 = 96.0;

fn window(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    app.get_webview_window("overlay")
}

/// Show pinned to bottom-center of the monitor the main window is on
/// (fallback: primary).
pub fn show(app: &AppHandle) {
    let Some(overlay) = window(app) else { return };

    // Click-through: the indicator must never steal clicks from the app
    // under it. Re-asserted on every show in case the OS reset it.
    let _ = overlay.set_ignore_cursor_events(true);

    let monitor = app
        .get_webview_window("main")
        .and_then(|w| w.current_monitor().ok().flatten())
        .or_else(|| overlay.primary_monitor().ok().flatten());

    if let Some(monitor) = monitor {
        let scale = monitor.scale_factor();
        let size = monitor.size().to_logical::<f64>(scale);
        let pos = monitor.position().to_logical::<f64>(scale);
        let height = overlay.outer_size().map(|s| s.to_logical::<f64>(scale).height).unwrap_or(130.0);
        let _ = overlay.set_position(LogicalPosition::new(
            pos.x + (size.width - OVERLAY_W) / 2.0,
            pos.y + size.height - height - MARGIN_BOTTOM,
        ));
    }

    let _ = overlay.show();
}

pub fn hide(app: &AppHandle) {
    if let Some(overlay) = window(app) {
        let _ = overlay.hide();
    }
}
