//! OS integration — everything that makes this a *system-wide* dictation
//! tool rather than an API demo.
//!
//! Nothing in this tree talks to OpenRouter. If you're reading this repo to
//! learn the API integration, skip this directory entirely and read
//! `crate::stt` instead; come back here for the desktop-app machinery:
//!
//! - `hotkey`  — device-wide push-to-talk (keyboard poll + raw HID buttons)
//! - `hid`     — macOS IOHID listener (sees mouse side buttons that the
//!               CGEvent layer hides when driver software remaps them)
//! - `tray`    — menu-bar presence + background-service lifecycle
//! - `overlay` — floating dictation pill (transparent click-through window)
//! - `traffic` — macOS titlebar traffic-light repositioning

/// Main-window webview zoom. The UI is laid out in small fixed px sizes;
/// zooming scales it evenly. The native titlebar math in `traffic` uses it.
pub const UI_ZOOM: f64 = 1.15;

pub mod hotkey;
pub mod overlay;
pub mod tray;

#[cfg(target_os = "macos")]
pub mod hid;
#[cfg(target_os = "macos")]
mod hotkey_tap;
#[cfg(target_os = "macos")]
pub mod traffic;
#[cfg(target_os = "macos")]
pub mod paste;
