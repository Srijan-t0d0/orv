//! Transcript post-processing: everything between "the model returned text"
//! and "text lands at the user's cursor".
//!
//! - filler removal (um/uh/hmm) with separator tidy-up
//! - dictation continuity (joining space between consecutive dictations)
//! - insertion into the recording's target app (clipboard paste on macOS)

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Dictation continuity: consecutive transcripts typed into the same field
/// need a joining space ("…last one." + "And this…" → "…last one. And this…").
/// Tracks the final char of the last typed transcript; cleared when Enter is
/// pressed / synthesized or a click happens (caret moved), or when stale.
static LAST_TYPED: Mutex<Option<(char, Instant)>> = Mutex::new(None);
/// After this long, assume the user moved on and don't auto-join.
const CONTINUITY_TTL: Duration = Duration::from_secs(90);

/// Called by the hotkey module on Enter (physical or synthesized) and left
/// click — the next dictation starts fresh, no joining space.
pub fn reset_continuity() {
    *LAST_TYPED.lock().unwrap() = None;
}

/// Strip filler words and tidy the leftovers. STT models emit "Um," / "uh"
/// as real tokens; standalone fillers are removed, then doubled separators
/// are collapsed. Word-boundary-safe: "umbrella" is never touched.
pub fn clean_fillers(text: &str) -> String {
    const FILLERS: &[&str] = &["um", "uh", "erm", "uhm", "umm", "uhh", "hmm", "hm", "mhm"];

    // Tokenize on whitespace; a token is a filler if its alphabetic core
    // (letters only, case-insensitive) matches the list.
    let mut kept: Vec<&str> = Vec::new();
    let mut removed = 0usize;
    for token in text.split_whitespace() {
        let core: String = token
            .chars()
            .filter(|c| c.is_alphabetic())
            .flat_map(|c| c.to_lowercase())
            .collect();
        if FILLERS.contains(&core.as_str()) {
            removed += 1;
            continue;
        }
        kept.push(token);
    }
    if removed == 0 {
        return text.to_string();
    }

    let mut out = kept.join(" ");
    // Collapse separators stranded by removals: ", ," / ",," → ","
    while out.contains(", ,") {
        out = out.replace(", ,", ",");
    }
    while out.contains(",,") {
        out = out.replace(",,", ",");
    }
    while out.contains("  ") {
        out = out.replace("  ", " ");
    }
    let out = out.trim_start_matches([',', ' ']).to_string();
    eprintln!("[clean] removed {removed} filler token(s)");
    out
}

/// Check the current process, rather than trusting the System Settings toggle.
pub fn input_access_allowed() -> bool {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn AXIsProcessTrusted() -> bool;
        }
        unsafe { AXIsProcessTrusted() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Insert `text` into the recording's target application. macOS restores the
/// target's focus and uses clipboard paste; other platforms use enigo text entry.
/// Needs Accessibility permission on macOS.
///
/// Consecutive dictations join naturally: if the previous transcript ended
/// without trailing whitespace and this one starts with a word character,
/// a single space is prepended.
///
/// The returned receiver fires once the text has been sent to the app; it
/// disconnects without firing if insertion fails.
pub fn type_text(
    text: &str,
    target_pid: Option<i32>,
    app: tauri::AppHandle,
) -> std::sync::mpsc::Receiver<()> {
    #[cfg(not(target_os = "macos"))]
    use enigo::{Enigo, Keyboard, Settings as EnigoSettings};

    let (sent, sent_rx) = std::sync::mpsc::channel();
    if !input_access_allowed() {
        eprintln!("[type] BLOCKED: this running process does not have Accessibility access");
        return sent_rx;
    }
    let mut text = text.to_string();
    {
        let mut last = LAST_TYPED.lock().unwrap();
        if let Some((last_char, at)) = *last {
            let fresh = at.elapsed() < CONTINUITY_TTL;
            let needs_space = fresh
                && !last_char.is_whitespace()
                && text.chars().next().is_some_and(|c| !c.is_whitespace());
            if needs_space {
                eprintln!("[type] joining onto previous dictation with a space");
                text.insert(0, ' ');
            }
        }
        if let Some(end) = text.chars().last() {
            *last = Some((end, Instant::now()));
        }
    }

    #[cfg(target_os = "macos")]
    std::thread::spawn(move || {
        match crate::platform::paste::paste_text(&text, target_pid, &app, sent) {
            Ok(()) => eprintln!("[paste] submitted {} chars", text.chars().count()),
            Err(e) => eprintln!("[paste] failed: {e}"),
        }
    });
    #[cfg(not(target_os = "macos"))]
    std::thread::spawn(move || match Enigo::new(&EnigoSettings::default()) {
        Ok(mut enigo) => {
            if let Err(e) = enigo.text(&text) {
                eprintln!("[type] failed to type transcript: {e}");
            } else {
                let _ = sent.send(());
                eprintln!("[type] submitted {} chars as keyboard events (delivery not verified)", text.chars().count());
            }
        }
        Err(e) => eprintln!(
            "[type] enigo init failed: {e} — grant Accessibility permission in \
             System Settings → Privacy & Security"
        ),
    });
    sent_rx
}
