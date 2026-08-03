//! Transcript post-processing: everything between "the model returned text"
//! and "text lands at the user's cursor".
//!
//! - filler removal (um/uh/hmm) with separator tidy-up
//! - dictation continuity (joining space between consecutive dictations)
//! - typing into the focused app (enigo key synthesis)

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

/// Synthesize `text` into the focused application. Uses enigo's text entry
/// (macOS: CGEvent keyboard events with a unicode payload), which types into
/// any field that accepts keystrokes. Needs Accessibility permission.
///
/// Consecutive dictations join naturally: if the previous transcript ended
/// without trailing whitespace and this one starts with a word character,
/// a single space is prepended.
pub fn type_text(text: &str) {
    use enigo::{Enigo, Keyboard, Settings as EnigoSettings};

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

    std::thread::spawn(move || match Enigo::new(&EnigoSettings::default()) {
        Ok(mut enigo) => {
            if let Err(e) = enigo.text(&text) {
                eprintln!("[type] failed to type transcript: {e}");
            } else {
                eprintln!("[type] typed {} chars", text.chars().count());
            }
        }
        Err(e) => eprintln!(
            "[type] enigo init failed: {e} — grant Accessibility permission in \
             System Settings → Privacy & Security"
        ),
    });
}
