//! Raw HID mouse-button listener (macOS).
//!
//! Why this exists: on this machine, side buttons (Mouse 4/5) never appear
//! through the CGEvent layer — neither CGEventSourceButtonState polling nor
//! a CGEvent tap sees them (verified with standalone probes), most likely
//! because mouse driver software consumes them after the HID layer. IOHID
//! reads input reports straight from the device driver, one level below
//! that, so every physical button is visible.
//!
//! The listener keeps a global pressed-state bitmap; the hotkey poll loop
//! reads it instead of (well, in addition to) CGEventSourceButtonState.
//! Requires the same Input Monitoring permission as everything else.

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const HID_PAGE_BUTTON: u32 = 0x09;

/// Pressed-state bitmap: bit N = mouse button N (CG numbering: 0=left).
static BUTTONS: AtomicU32 = AtomicU32::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Read the current pressed state of button `n` (CG numbering).
pub fn button_down(n: usize) -> bool {
    BUTTONS.load(Ordering::Relaxed) & (1 << n) != 0
}

// ── IOKit / CoreFoundation FFI (minimal surface) ──

type IOHIDManagerRef = *mut c_void;
type IOHIDValueRef = *mut c_void;
type IOHIDElementRef = *mut c_void;
type CFStringRef = *const c_void;
type CFRunLoopRef = *mut c_void;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDManagerCreate(allocator: *const c_void, options: u32) -> IOHIDManagerRef;
    fn IOHIDManagerSetDeviceMatching(manager: IOHIDManagerRef, matching: *const c_void);
    fn IOHIDManagerRegisterInputValueCallback(
        manager: IOHIDManagerRef,
        callback: extern "C" fn(*mut c_void, i32, *mut c_void, IOHIDValueRef),
        context: *mut c_void,
    );
    fn IOHIDManagerScheduleWithRunLoop(
        manager: IOHIDManagerRef,
        run_loop: CFRunLoopRef,
        mode: CFStringRef,
    );
    fn IOHIDManagerOpen(manager: IOHIDManagerRef, options: u32) -> i32;
    fn IOHIDValueGetElement(value: IOHIDValueRef) -> IOHIDElementRef;
    fn IOHIDValueGetIntegerValue(value: IOHIDValueRef) -> isize;
    fn IOHIDElementGetUsagePage(element: IOHIDElementRef) -> u32;
    fn IOHIDElementGetUsage(element: IOHIDElementRef) -> u32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopDefaultMode: CFStringRef;
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopRun();
}

extern "C" fn input_value_cb(
    _context: *mut c_void,
    _result: i32,
    _sender: *mut c_void,
    value: IOHIDValueRef,
) {
    unsafe {
        let element = IOHIDValueGetElement(value);
        if element.is_null() || IOHIDElementGetUsagePage(element) != HID_PAGE_BUTTON {
            return;
        }
        // HID button usage is 1-based (1=left, 2=right, 3=middle, 4/5=side);
        // shift to CG numbering (0-based) to match the rest of the app.
        let usage = IOHIDElementGetUsage(element);
        if usage == 0 || usage > 32 {
            return;
        }
        let bit = 1u32 << (usage - 1);
        let pressed = IOHIDValueGetIntegerValue(value) != 0;
        if pressed {
            BUTTONS.fetch_or(bit, Ordering::Relaxed);
        } else {
            BUTTONS.fetch_and(!bit, Ordering::Relaxed);
        }
    }
}

/// Spawn the HID listener thread (idempotent). Matches all HID devices and
/// filters to Button-page elements in the callback, so mice, trackballs,
/// and gaming side buttons are all covered.
pub fn spawn_listener() {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("hid-buttons".into())
        .spawn(|| unsafe {
            let manager = IOHIDManagerCreate(std::ptr::null(), 0);
            if manager.is_null() {
                eprintln!("[hid] IOHIDManagerCreate failed");
                RUNNING.store(false, Ordering::SeqCst);
                return;
            }
            IOHIDManagerSetDeviceMatching(manager, std::ptr::null()); // all devices
            IOHIDManagerRegisterInputValueCallback(manager, input_value_cb, std::ptr::null_mut());
            IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
            let status = IOHIDManagerOpen(manager, 0);
            if status != 0 {
                eprintln!(
                    "[hid] IOHIDManagerOpen failed ({status:#x}) — grant Input Monitoring \
                     permission in System Settings → Privacy & Security"
                );
            } else {
                eprintln!("[hid] raw HID button listener up");
            }
            CFRunLoopRun();
        })
        .ok();
}
