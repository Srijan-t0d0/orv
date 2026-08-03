//! macOS traffic-light positioning.
//!
//! The `trafficLightPosition` window config isn't reliably applied with our
//! Overlay titlebar, so we move the buttons via AppKit directly: grow the
//! titlebar container to the header's height (AppKit vertically centers the
//! buttons within it) and pin the close button at `x`, preserving the native
//! 20px pitch. Must be re-applied whenever AppKit rebuilds the titlebar
//! (resize, fullscreen transitions, theme changes).

#![cfg(target_os = "macos")]

use objc::runtime::Object;
use objc::{msg_send, sel, sel_impl};
use tauri::{Manager, WebviewWindow, WindowEvent};

/// Header band height in ui/style.css — the lights center within this.
const HEADER_HEIGHT: f64 = 44.0;
const INSET_X: f64 = 14.0;

#[repr(C)]
#[derive(Clone, Copy)]
struct NSPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct NSSize {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct NSRect {
    origin: NSPoint,
    size: NSSize,
}

fn apply(window: &WebviewWindow) {
    let Ok(ns_window) = window.ns_window() else { return };
    let ns_window = ns_window as *mut Object;

    unsafe {
        // 0 = close, 1 = miniaturize, 2 = zoom
        let close: *mut Object = msg_send![ns_window, standardWindowButton: 0u64];
        let mini: *mut Object = msg_send![ns_window, standardWindowButton: 1u64];
        let zoom: *mut Object = msg_send![ns_window, standardWindowButton: 2u64];
        if close.is_null() || mini.is_null() || zoom.is_null() {
            return;
        }

        // titlebar container = button.superview.superview
        let title_bar: *mut Object = msg_send![close, superview];
        let title_bar: *mut Object = msg_send![title_bar, superview];
        if title_bar.is_null() {
            return;
        }

        let content_view: *mut Object = msg_send![ns_window, contentView];
        let content_frame: NSRect = msg_send![content_view, frame];

        // Grow the (top-anchored) titlebar container to the header height;
        // AppKit centers the buttons vertically inside it.
        let mut tb_frame: NSRect = msg_send![title_bar, frame];
        tb_frame.size.height = HEADER_HEIGHT;
        tb_frame.origin.y = content_frame.size.height - HEADER_HEIGHT;
        let _: () = msg_send![title_bar, setFrame: tb_frame];

        // Keep the native horizontal pitch, pin close at INSET_X, and center
        // each button vertically ourselves — resizing the container does NOT
        // recenter its subviews.
        let close_frame: NSRect = msg_send![close, frame];
        let mini_frame: NSRect = msg_send![mini, frame];
        let pitch = mini_frame.origin.x - close_frame.origin.x;
        for (i, button) in [close, mini, zoom].into_iter().enumerate() {
            let mut frame: NSRect = msg_send![button, frame];
            frame.origin.x = INSET_X + i as f64 * pitch;
            frame.origin.y = (HEADER_HEIGHT - frame.size.height) / 2.0;
            let _: () = msg_send![button, setFrameOrigin: frame.origin];
        }
    }
}

/// Position now and keep positioned across titlebar rebuilds.
pub fn setup(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };

    apply(&window);

    let window_for_events = window.clone();
    window.on_window_event(move |event| {
        if matches!(
            event,
            WindowEvent::Resized(_) | WindowEvent::Focused(_) | WindowEvent::ThemeChanged(_)
        ) {
            apply(&window_for_events);
        }
    });
}
