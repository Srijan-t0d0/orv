//! macOS insertion: restore the recording's target, paste, then restore the clipboard.
use std::ffi::c_void;
use std::sync::Mutex;
use std::time::Duration;

use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};

static PASTE_LOCK: Mutex<()> = Mutex::new(());

pub fn frontmost_pid() -> Option<i32> {
    unsafe {
        let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        let app: *mut Object = msg_send![workspace, frontmostApplication];
        if app.is_null() {
            return None;
        }
        let pid: i32 = msg_send![app, processIdentifier];
        (pid > 0 && pid as u32 != std::process::id()).then_some(pid)
    }
}

fn restore_clipboard(ours: i64, current: i64) -> bool {
    ours == current
}

pub fn paste_text(
    text: &str,
    target: Option<i32>,
    handle: &tauri::AppHandle,
) -> Result<(), String> {
    let _lock = PASTE_LOCK.lock().map_err(|_| "paste lock poisoned")?;
    let target = target.ok_or("No external input app was focused when recording started")?;
    unsafe {
        let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
        let result = paste_with_pool(text, target, handle);
        let _: () = msg_send![pool, drain];
        result
    }
}

unsafe fn paste_with_pool(
    text: &str,
    target: i32,
    handle: &tauri::AppHandle,
) -> Result<(), String> {
    let app: *mut Object =
        msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: target];
    if app.is_null() {
        return Err("The original input app is no longer running".into());
    }
    if frontmost_pid() != Some(target) {
        let activated: bool = msg_send![app, activateWithOptions: 2u64];
        if !activated {
            return Err("Could not activate the original input app".into());
        }
        for _ in 0..20 {
            if frontmost_pid() == Some(target) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if frontmost_pid() != Some(target) {
            return Err("Original input app did not receive focus; paste cancelled".into());
        }
    }
    eprintln!("[paste] target pid={target}");
    let board: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
    let snapshot: i64 = msg_send![board, changeCount];
    let items: *mut Object = msg_send![board, pasteboardItems];
    // Copy every type (including images/rich text) before clearContents invalidates lazy data.
    let backup: *mut Object = msg_send![class!(NSMutableArray), array];
    let count: usize = if items.is_null() {
        0
    } else {
        msg_send![items, count]
    };
    for i in 0..count {
        let item: *mut Object = msg_send![items, objectAtIndex: i];
        let copy: *mut Object = msg_send![class!(NSPasteboardItem), new];
        let copy: *mut Object = msg_send![copy, autorelease];
        let types: *mut Object = msg_send![item, types];
        let type_count: usize = msg_send![types, count];
        for j in 0..type_count {
            let kind: *mut Object = msg_send![types, objectAtIndex: j];
            let data: *mut Object = msg_send![item, dataForType: kind];
            if data.is_null() {
                return Err("Could not back up clipboard; paste cancelled".into());
            }
            let copied: bool = msg_send![copy, setData: data forType: kind];
            if !copied {
                return Err("Could not back up clipboard item; paste cancelled".into());
            }
        }
        let _: () = msg_send![backup, addObject: copy];
    }
    let value: *mut Object = msg_send![class!(NSString), alloc];
    let value: *mut Object = msg_send![value, initWithBytes: text.as_ptr() as *const c_void length: text.len() encoding: 4usize];
    if value.is_null() {
        return Err("Could not encode transcript for paste".into());
    }
    let kind: *mut Object =
        msg_send![class!(NSString), stringWithUTF8String: b"public.utf8-plain-text\0".as_ptr()];
    let current: i64 = msg_send![board, changeCount];
    if !restore_clipboard(snapshot, current) {
        let _: () = msg_send![value, release];
        return Err("Clipboard changed while backing it up; paste cancelled".into());
    }
    let _: i64 = msg_send![board, clearContents];
    let written: bool = msg_send![board, setString: value forType: kind];
    let _: () = msg_send![value, release];
    let ours: i64 = msg_send![board, changeCount];
    let result = if written {
        send_paste(handle, target, ours)
    } else {
        Err("Could not write transcript to clipboard".into())
    };
    // ponytail: bounded paste-read grace; use AX insertion if a target app reads later.
    if written {
        std::thread::sleep(Duration::from_millis(350));
    }
    let current: i64 = msg_send![board, changeCount];
    if restore_clipboard(ours, current) {
        let _: i64 = msg_send![board, clearContents];
        let restored: bool = msg_send![board, writeObjects: backup];
        if !restored && count > 0 {
            eprintln!("[paste] failed to restore clipboard");
        }
    }
    result
}

type Ref = *mut c_void;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreateKeyboardEvent(source: Ref, key: u16, down: bool) -> Ref;
    fn CGEventSetFlags(event: Ref, flags: u64);
    fn CGEventPost(tap: u32, event: Ref);
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDataGetBytePtr(data: Ref) -> *const u8;
    fn CFRelease(value: *const c_void);
}
#[link(name = "Carbon", kind = "framework")]
extern "C" {
    static kTISPropertyUnicodeKeyLayoutData: Ref;
    fn TISCopyCurrentKeyboardLayoutInputSource() -> Ref;
    fn TISCopyCurrentASCIICapableKeyboardLayoutInputSource() -> Ref;
    fn TISGetInputSourceProperty(source: Ref, key: Ref) -> Ref;
    fn LMGetKbdType() -> u8;
    #[allow(clippy::too_many_arguments)]
    fn UCKeyTranslate(
        layout: *const u8,
        key: u16,
        action: u16,
        modifiers: u32,
        keyboard_type: u32,
        options: u32,
        dead_key_state: *mut u32,
        max_len: usize,
        len: *mut usize,
        chars: *mut u16,
    ) -> i32;
}

/// Keycode that types `ch` with ⌘ held on the active layout (Dvorak, AZERTY, …).
/// Falls back to the last ASCII layout for input methods without layout data.
unsafe fn layout_keycode(ch: u16) -> Option<u16> {
    const ACTION_DISPLAY: u16 = 3;
    const CMD_MODIFIER: u32 = 0x100 >> 8;
    const NO_DEAD_KEYS: u32 = 1;
    for copy in [
        TISCopyCurrentKeyboardLayoutInputSource as unsafe extern "C" fn() -> Ref,
        TISCopyCurrentASCIICapableKeyboardLayoutInputSource,
    ] {
        let source = copy();
        if source.is_null() {
            continue;
        }
        let data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
        let found = (!data.is_null()).then(|| {
            let layout = CFDataGetBytePtr(data);
            (0..128u16).find(|&key| {
                let (mut dead, mut len, mut out) = (0u32, 0usize, [0u16; 4]);
                UCKeyTranslate(layout, key, ACTION_DISPLAY, CMD_MODIFIER, LMGetKbdType() as u32,
                    NO_DEAD_KEYS, &mut dead, out.len(), &mut len, out.as_mut_ptr()) == 0
                    && len == 1
                    && out[0] == ch
            })
        });
        CFRelease(source);
        if let Some(found) = found {
            return found;
        }
    }
    None
}

/// ⌘ rides on the V events themselves (as OpenWhispr does): a separately pressed ⌘
/// registers asynchronously, and a V that beats it types a bare "v".
unsafe fn post_command_v() -> Result<(), String> {
    const SESSION_TAP: u32 = 1;
    const COMMAND: u64 = 1 << 20;
    // Never guess US keycode 9: on other layouts it fires a different shortcut.
    let key = layout_keycode(b'v' as u16).ok_or("No key types V on this keyboard layout")?;
    for down in [true, false] {
        let event = CGEventCreateKeyboardEvent(std::ptr::null_mut(), key, down);
        if event.is_null() {
            return Err("Could not create paste keystroke".into());
        }
        CGEventSetFlags(event, COMMAND);
        CGEventPost(SESSION_TAP, event);
        CFRelease(event);
        if down {
            std::thread::sleep(Duration::from_millis(8));
        }
    }
    Ok(())
}

fn send_paste(
    handle: &tauri::AppHandle,
    target: i32,
    clipboard_version: i64,
) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    // Keyboard-layout (TIS) APIs require the main thread.
    let scheduled = handle.run_on_main_thread(move || {
        let result = (|| -> Result<(), String> {
            let board: *mut Object = unsafe { msg_send![class!(NSPasteboard), generalPasteboard] };
            let current: i64 = unsafe { msg_send![board, changeCount] };
            if frontmost_pid() != Some(target) || current != clipboard_version {
                return Err("Focus or clipboard changed before paste; insertion cancelled".into());
            }
            unsafe { post_command_v() }
        })();
        let _ = tx.send(result);
    });
    scheduled.map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::restore_clipboard;
    #[test]
    fn preserve_newer_user_copy() {
        assert!(restore_clipboard(7, 7));
        assert!(!restore_clipboard(7, 8));
    }
}
