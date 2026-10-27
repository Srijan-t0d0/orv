//! Suppress selected F13–F20 events without invoking keyboard-layout APIs.
#![cfg(target_os = "macos")]

use device_query::Keycode;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

const KEYS: [Keycode; 8] = [
    Keycode::F13,
    Keycode::F14,
    Keycode::F15,
    Keycode::F16,
    Keycode::F17,
    Keycode::F18,
    Keycode::F19,
    Keycode::F20,
];
const VK: [i64; 8] = [105, 107, 113, 106, 64, 79, 80, 90];
const BINDINGS: [&str; 8] = [
    "key:F13", "key:F14", "key:F15", "key:F16", "key:F17", "key:F18", "key:F19", "key:F20",
];
static SELECTED: AtomicU8 = AtomicU8::new(0);
static DOWN: AtomicU8 = AtomicU8::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);

type Ref = *mut c_void;
type Callback = unsafe extern "C" fn(Ref, u32, Ref, Ref) -> Ref;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(
        location: u32,
        placement: u32,
        options: u32,
        events: u64,
        callback: Callback,
        context: Ref,
    ) -> Ref;
    fn CGEventTapEnable(tap: Ref, enable: bool);
    fn CGEventGetIntegerValueField(event: Ref, field: u32) -> i64;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopDefaultMode: *const c_void;
    fn CFMachPortCreateRunLoopSource(allocator: *const c_void, port: Ref, order: isize) -> Ref;
    fn CFMachPortInvalidate(port: Ref);
    fn CFRunLoopGetCurrent() -> Ref;
    fn CFRunLoopAddSource(run_loop: Ref, source: Ref, mode: *const c_void);
    fn CFRunLoopRemoveSource(run_loop: Ref, source: Ref, mode: *const c_void);
    fn CFRunLoopRun();
    fn CFRelease(value: *const c_void);
}

fn selected_mask(stt: Option<&str>, enter: Option<&str>, capturing: bool) -> u8 {
    if capturing {
        return u8::MAX;
    }
    BINDINGS.iter().enumerate().fold(0, |mask, (i, binding)| {
        mask | if stt == Some(*binding) || enter == Some(*binding) {
            1 << i
        } else {
            0
        }
    })
}

/// Capture selects all eight keys; ordinary bindings select only exact key:F13..F20 strings.
pub fn configure(stt: Option<&str>, enter: Option<&str>, capturing: bool) {
    SELECTED.store(selected_mask(stt, enter, capturing), Ordering::SeqCst);
}

/// Polling must exclude selected keys and consumed presses until their paired release.
pub fn handles(key: Keycode) -> bool {
    ACTIVE.load(Ordering::SeqCst)
        && KEYS
            .iter()
            .position(|k| *k == key)
            .map(|i| {
                (SELECTED.load(Ordering::SeqCst) | DOWN.load(Ordering::SeqCst)) & (1 << i) != 0
            })
            .unwrap_or(false)
}

// A release belongs to us only if we consumed its press, even after unbinding.
fn transition(selected: u8, held: u8, bit: u8, pressed: bool) -> (u8, bool, Option<bool>) {
    if bit == 0 {
        return (held, false, None);
    }
    if held & bit != 0 {
        if pressed {
            (held, true, None)
        } else {
            (held & !bit, true, Some(false))
        }
    } else if pressed && selected & bit != 0 {
        (held | bit, true, Some(true))
    } else {
        (held, false, None)
    }
}

struct Context {
    tap: Ref,
    sender: Sender<(Keycode, bool)>,
}

fn flush(sender: &Sender<(Keycode, bool)>) {
    let held = DOWN.swap(0, Ordering::SeqCst);
    for (i, key) in KEYS.iter().enumerate() {
        if held & (1 << i) != 0 {
            let _ = sender.send((*key, false));
        }
    }
}

// Called only by CoreGraphics with our live run-loop context and event.
unsafe extern "C" fn callback(_proxy: Ref, kind: u32, event: Ref, context: Ref) -> Ref {
    if context.is_null() {
        return event;
    }
    // Context lives on the run-loop thread until the tap has been invalidated.
    let context = unsafe { &*(context as *const Context) };
    if kind == 0xffff_fffe || kind == 0xffff_ffff {
        flush(&context.sender);
        unsafe {
            CGEventTapEnable(context.tap, true);
        }
        return event;
    }
    if (kind != 10 && kind != 11) || event.is_null() {
        return event;
    }
    let vk = unsafe { CGEventGetIntegerValueField(event, 9) };
    let Some(i) = VK.iter().position(|code| *code == vk) else {
        return event;
    };
    let (held, swallow, edge) = transition(
        SELECTED.load(Ordering::SeqCst),
        DOWN.load(Ordering::SeqCst),
        1 << i,
        kind == 10,
    );
    DOWN.store(held, Ordering::SeqCst);
    if let Some(pressed) = edge {
        let _ = context.sender.send((KEYS[i], pressed));
    }
    if swallow {
        null_mut()
    } else {
        event
    }
}

/// Start once. The unbounded queue preserves even down/up pairs between polling ticks.
/// Failure leaves polling in charge; macOS requires Accessibility/Input Monitoring access.
pub fn spawn() -> Receiver<(Keycode, bool)> {
    let (sender, receiver) = mpsc::channel();
    let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
    let result = std::thread::Builder::new().name("hotkey-tap".into()).spawn(move || unsafe {
        let mut context = Context { tap: null_mut(), sender };
        // Session tap, head insertion, default (suppress-enabled) options.
        let tap = CGEventTapCreate(1, 0, 0, (1 << 10) | (1 << 11), callback,
            &mut context as *mut Context as Ref);
        if tap.is_null() {
            ACTIVE.store(false, Ordering::SeqCst);
            eprintln!("[hotkey-tap] CGEventTapCreate failed; check Accessibility/Input Monitoring permissions");
            let _ = ready_sender.send(());
            return;
        }
        context.tap = tap;
        let source = CFMachPortCreateRunLoopSource(null(), tap, 0);
        if source.is_null() {
            CFMachPortInvalidate(tap);
            CFRelease(tap);
            ACTIVE.store(false, Ordering::SeqCst);
            eprintln!("[hotkey-tap] CFMachPortCreateRunLoopSource failed");
            let _ = ready_sender.send(());
            return;
        }
        let run_loop = CFRunLoopGetCurrent();
        CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
        CGEventTapEnable(tap, true);
        ACTIVE.store(true, Ordering::SeqCst);
        eprintln!("[hotkey-tap] function-key suppression active");
        let _ = ready_sender.send(());
        CFRunLoopRun();
        CGEventTapEnable(tap, false);
        CFMachPortInvalidate(tap);
        flush(&context.sender);
        ACTIVE.store(false, Ordering::SeqCst);
        CFRunLoopRemoveSource(run_loop, source, kCFRunLoopDefaultMode);
        CFRelease(source);
        CFRelease(tap);
    });
    if let Err(error) = result {
        ACTIVE.store(false, Ordering::SeqCst);
        eprintln!("[hotkey-tap] thread initialization failed: {error}");
    }
    // Poll only after the tap is ready or has failed. A startup panic or
    // spawn failure drops the sender and unblocks recv as well.
    let _ = ready_receiver.recv();
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_masks() {
        assert_eq!(selected_mask(None, None, false), 0);
        for (i, binding) in BINDINGS.iter().enumerate() {
            assert_eq!(selected_mask(Some(binding), None, false), 1 << i);
        }
        assert_eq!(selected_mask(Some("key:F18"), Some("key:F20"), false), 0xa0);
        assert_eq!(selected_mask(Some("mouse:3"), Some("key:F12"), false), 0);
        assert_eq!(selected_mask(Some("key:F180"), None, false), 0);
        assert_eq!(selected_mask(None, None, true), 0xff);
    }

    #[test]
    fn consumed_pairs_repeats_and_unrelated_events() {
        let bit = 1 << 5;
        let (held, swallow, edge) = transition(bit, 0, bit, true);
        assert_eq!((held, swallow, edge), (bit, true, Some(true)));
        assert_eq!(transition(bit, held, bit, true), (bit, true, None));
        assert_eq!(transition(0, held, bit, true), (bit, true, None));
        assert_eq!(transition(0, held, bit, false), (0, true, Some(false)));
        assert_eq!(transition(bit, 0, bit, false), (0, false, None));
        assert_eq!(transition(0, 0, bit, true), (0, false, None));
        assert_eq!(transition(bit, held, 1, true), (held, false, None));
        assert_eq!(transition(bit, held, 1, false), (held, false, None));
        assert_eq!(transition(0xff, held, 0, true), (held, false, None));
    }
}
