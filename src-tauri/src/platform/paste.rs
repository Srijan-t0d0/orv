//! macOS insertion: restore the recording's target, paste, then restore the clipboard.
use std::ffi::c_void;
use std::sync::Mutex;
use std::time::Duration;

use enigo::{Direction, Enigo, Key, Keyboard, Settings};
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

fn send_paste(
    handle: &tauri::AppHandle,
    target: i32,
    clipboard_version: i64,
) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    // Enigo resolves the logical V using macOS keyboard-layout APIs; those require the main thread.
    let scheduled = handle.run_on_main_thread(move || {
        let result = (|| -> Result<(), String> {
            let mut keyboard = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
            let board: *mut Object = unsafe { msg_send![class!(NSPasteboard), generalPasteboard] };
            let current: i64 = unsafe { msg_send![board, changeCount] };
            if frontmost_pid() != Some(target) || current != clipboard_version {
                return Err("Focus or clipboard changed before paste; insertion cancelled".into());
            }
            let result = keyboard
                .key(Key::Meta, Direction::Press)
                .and_then(|_| keyboard.key(Key::Unicode('v'), Direction::Click))
                .map_err(|e| e.to_string());
            let released = keyboard
                .key(Key::Meta, Direction::Release)
                .map_err(|e| e.to_string());
            result.and(released)
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
