//! Device-wide hotkey support (keyboard *and* mouse buttons).
//!
//! Implementation: a 25 ms polling loop over `device_query`. Polling (rather
//! than an OS event tap) is deliberate — rdev's CGEventTap callback crashes
//! on modern macOS by calling TSM keyboard-layout APIs off the main queue
//! (dispatch_assert_queue → SIGTRAP). Polled state via CGEventSourceKeyState
//! is thread-safe, and 25 ms is imperceptible for a toggle key.
//!
//! Modes:
//!   - capture: the next key/button press becomes the new binding
//!     (Escape cancels; left/right mouse buttons are refused so the user
//!     can't brick their pointer)
//!   - armed: presses matching the binding toggle STT, system-wide, even
//!     when the app is unfocused
//!
//! macOS note: global input polling requires Input Monitoring permission for
//! the app / terminal that launches it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use device_query::{DeviceQuery, DeviceState, Keycode};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;

const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Mouse buttons use CG-style numbering: 0=left, 1=right, 2=middle, 3+=side.
const MOUSE_BUTTONS: usize = 8;

/// A binding is stored as a stable string, e.g. "key:F9" or "mouse:3".
pub fn encode_key(key: Keycode) -> String {
    format!("key:{key:?}")
}

pub fn encode_button(button: usize) -> String {
    format!("mouse:{button}")
}

/// Human label for the UI ("F9", "Middle click", "Mouse 4").
pub fn label(binding: &str) -> String {
    match binding.split_once(':') {
        Some(("key", k)) => k.to_string(),
        Some(("mouse", "2")) => "Middle click".into(),
        Some(("mouse", b)) => format!("Mouse {}", b.parse::<usize>().map(|n| n + 1).unwrap_or(0)),
        _ => binding.to_string(),
    }
}

/// Poll the pressed state of mouse buttons 0..8, normalized to CG numbering.
///
/// macOS: read from BOTH layers and OR them together:
/// - raw HID (crate::hid) — sees side buttons even when driver software
///   swallows them before the CGEvent layer (verified on this machine:
///   neither CGEventSourceButtonState nor an event tap ever reported 3/4)
/// - CGEventSourceButtonState — covers synthetic/remapped events that only
///   exist at the CG layer
#[cfg(target_os = "macos")]
fn poll_mouse_buttons(_device: &DeviceState) -> [bool; MOUSE_BUTTONS] {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceButtonState(state_id: i32, button: u32) -> bool;
    }
    const COMBINED_SESSION_STATE: i32 = 0;
    let mut out = [false; MOUSE_BUTTONS];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = crate::platform::hid::button_down(i)
            || unsafe { CGEventSourceButtonState(COMBINED_SESSION_STATE, i as u32) };
    }
    out
}

/// Other platforms: device_query's button_pressed is 1-indexed
/// [_, left, right, middle, x1, x2, ...]; shift to CG numbering.
#[cfg(not(target_os = "macos"))]
fn poll_mouse_buttons(device: &DeviceState) -> [bool; MOUSE_BUTTONS] {
    let pressed = device.get_mouse().button_pressed;
    let mut out = [false; MOUSE_BUTTONS];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = pressed.get(i + 1).copied().unwrap_or(false);
    }
    out
}

/// Which action a binding (or a capture) is for.
#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// Start/stop speech-to-text.
    Stt,
    /// Synthesize a real Enter keypress (send dictated messages without
    /// touching the keyboard, e.g. Mouse 4 = talk, Mouse 5 = send).
    Enter,
}

#[derive(Default)]
struct ListenerState {
    stt_binding: Option<String>,
    enter_binding: Option<String>,
    capturing: Option<Target>,
}

static STATE: Mutex<ListenerState> = Mutex::new(ListenerState {
    stt_binding: None,
    enter_binding: None,
    capturing: None,
});
static LISTENER_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize)]
struct HotkeySetEvent {
    target: Target,
    binding: Option<String>,
    label: Option<String>,
}

fn emit_binding(app: &AppHandle, target: Target, binding: Option<&str>) {
    let _ = app.emit(
        "hotkey-set",
        HotkeySetEvent {
            target,
            binding: binding.map(String::from),
            label: binding.map(label),
        },
    );
}

/// Begin capture mode: the next key or mouse press becomes `target`'s binding.
pub fn begin_capture(app: &AppHandle, target: Target) {
    STATE.lock().unwrap().capturing = Some(target);
    let _ = app.emit("hotkey-capturing", target);
}

/// Clear a binding.
pub fn clear(app: &AppHandle, target: Target) {
    {
        let mut st = STATE.lock().unwrap();
        match target {
            Target::Stt => st.stt_binding = None,
            Target::Enter => st.enter_binding = None,
        }
        st.capturing = None;
    }
    persist(app, target, None);
    emit_binding(app, target, None);
}

pub fn set_bindings_from_settings(stt: Option<String>, enter: Option<String>) {
    let mut st = STATE.lock().unwrap();
    st.stt_binding = stt;
    st.enter_binding = enter;
}

fn persist(app: &AppHandle, target: Target, binding: Option<String>) {
    let state = app.state::<AppState>();
    let mut settings = state.settings.lock().unwrap();
    match target {
        Target::Stt => settings.hotkey = binding,
        Target::Enter => settings.enter_hotkey = binding,
    }
    settings.save();
}

/// Synthesize a real Enter keypress into whatever app has focus.
/// macOS: requires the same Accessibility permission the listener needs.
fn press_enter() {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings as EnigoSettings};
    std::thread::spawn(|| {
        if let Ok(mut enigo) = Enigo::new(&EnigoSettings::default()) {
            let _ = enigo.key(Key::Return, Direction::Click);
        }
    });
}

enum Action {
    None,
    CaptureCancelled,
    CaptureRefused,
    Captured(Target, String),
    Press(Target),
    Release(Target),
}

/// Compute what a fresh press means given current mode. Holding the bound
/// input doesn't retrigger because we only act on newly-pressed inputs.
fn on_press(encoded: String) -> Action {
    let mut st = STATE.lock().unwrap();
    if let Some(target) = st.capturing {
        if encoded == "key:Escape" {
            st.capturing = None;
            Action::CaptureCancelled
        } else if encoded == encode_button(0) || encoded == encode_button(1) {
            // Left (0) / right (1) click: refuse, keep capturing.
            Action::CaptureRefused
        } else {
            st.capturing = None;
            match target {
                Target::Stt => {
                    st.stt_binding = Some(encoded.clone());
                    // Same input can't drive both actions.
                    if st.enter_binding.as_deref() == Some(encoded.as_str()) {
                        st.enter_binding = None;
                    }
                }
                Target::Enter => {
                    st.enter_binding = Some(encoded.clone());
                    if st.stt_binding.as_deref() == Some(encoded.as_str()) {
                        st.stt_binding = None;
                    }
                }
            }
            Action::Captured(target, encoded)
        }
    } else if st.stt_binding.as_deref() == Some(encoded.as_str()) {
        Action::Press(Target::Stt)
    } else if st.enter_binding.as_deref() == Some(encoded.as_str()) {
        Action::Press(Target::Enter)
    } else {
        Action::None
    }
}

fn on_release(encoded: String) -> Action {
    let st = STATE.lock().unwrap();
    if st.capturing.is_none() && st.stt_binding.as_deref() == Some(encoded.as_str()) {
        Action::Release(Target::Stt)
    } else {
        Action::None
    }
}

fn dispatch(app: &AppHandle, action: Action) {
    let mode = {
        let state = app.state::<AppState>();
        let settings = state.settings.lock().unwrap();
        settings.hotkey_mode.clone()
    };
    match action {
        // Push-to-talk (default): hold to listen, release to stop.
        // Release also flushes the in-progress utterance for transcription.
        Action::Press(Target::Stt) if mode == "toggle" => {
            eprintln!("[hotkey] STT toggle");
            crate::pipeline::toggle(app)
        }
        Action::Press(Target::Stt) => {
            eprintln!("[hotkey] STT start (push-to-talk)");
            if let Err(e) = crate::pipeline::start_impl(app) {
                eprintln!("[hotkey] STT start failed: {e}");
            }
        }
        Action::Release(Target::Stt) if mode != "toggle" => {
            eprintln!("[hotkey] STT stop (release)");
            crate::pipeline::stop_impl(app);
        }
        Action::Press(Target::Enter) => {
            eprintln!("[hotkey] synthesizing Enter");
            // Message sent → next dictation starts a fresh field; don't
            // prepend a joining space onto it.
            crate::pipeline::reset_continuity();
            press_enter()
        }
        _ => {}
    }
}

/// Spawn the polling listener thread (idempotent).
pub fn spawn_listener(app: AppHandle) {
    if LISTENER_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }

    std::thread::Builder::new()
        .name("global-hotkey".into())
        .spawn(move || {
            let device = DeviceState::new();
            let mut prev_keys: Vec<Keycode> = Vec::new();
            let mut prev_buttons = [false; MOUSE_BUTTONS];
            {
                let st = STATE.lock().unwrap();
                eprintln!(
                    "[hotkey] listener up; stt={:?} enter={:?}",
                    st.stt_binding, st.enter_binding
                );
            }

            loop {
                std::thread::sleep(POLL_INTERVAL);

                let keys = device.get_keys();
                let buttons = poll_mouse_buttons(&device);

                // Edge-detect both directions: presses (down now, up before)
                // and releases (up now, down before — needed for push-to-talk).
                let mut pressed: Vec<String> = keys
                    .iter()
                    .filter(|k| !prev_keys.contains(k))
                    .map(|k| encode_key(*k))
                    .collect();
                let mut released: Vec<String> = prev_keys
                    .iter()
                    .filter(|k| !keys.contains(k))
                    .map(|k| encode_key(*k))
                    .collect();
                for (i, down) in buttons.iter().enumerate() {
                    if *down && !prev_buttons[i] {
                        pressed.push(encode_button(i));
                    } else if !*down && prev_buttons[i] {
                        released.push(encode_button(i));
                    }
                }

                prev_keys = keys;
                prev_buttons = buttons;

                for encoded in pressed {
                    // PRIVACY: never log arbitrary key identities — only
                    // events that match a binding (or capture mode) may be
                    // logged further down. Key state is compared and
                    // discarded; no keystroke history is kept or sent
                    // anywhere. The only data that ever reaches the STT
                    // model is microphone audio.
                    //
                    // Dictation continuity: a physical Enter (message sent)
                    // or a left click (caret probably moved) between
                    // dictations means the next transcript should NOT be
                    // joined onto the previous one with a space.
                    if encoded == "key:Enter"
                        || encoded == "key:NumpadEnter"
                        || encoded == encode_button(0)
                    {
                        crate::pipeline::reset_continuity();
                    }
                    match on_press(encoded) {
                        Action::None => {}
                        Action::CaptureCancelled => {
                            let (stt, enter) = {
                                let st = STATE.lock().unwrap();
                                (st.stt_binding.clone(), st.enter_binding.clone())
                            };
                            emit_binding(&app, Target::Stt, stt.as_deref());
                            emit_binding(&app, Target::Enter, enter.as_deref());
                        }
                        Action::CaptureRefused => {
                            let _ = app.emit("hotkey-capture-refused", ());
                        }
                        Action::Captured(target, binding) => {
                            persist(&app, target, Some(binding.clone()));
                            emit_binding(&app, target, Some(&binding));
                            // The capture may have stolen the other target's
                            // binding; sync both sides.
                            let other = match target {
                                Target::Stt => Target::Enter,
                                Target::Enter => Target::Stt,
                            };
                            let other_binding = {
                                let st = STATE.lock().unwrap();
                                match other {
                                    Target::Stt => st.stt_binding.clone(),
                                    Target::Enter => st.enter_binding.clone(),
                                }
                            };
                            persist(&app, other, other_binding.clone());
                            emit_binding(&app, other, other_binding.as_deref());
                        }
                        action => dispatch(&app, action),
                    }
                }
                for encoded in released {
                    if let Action::Release(t) = on_release(encoded) {
                        dispatch(&app, Action::Release(t));
                    }
                }
            }
        })
        .ok();
}
