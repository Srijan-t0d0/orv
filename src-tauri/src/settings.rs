//! Persisted user settings (API key, model, device, language hint, hotkey).

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::stt::client::DEFAULT_CLEANUP_MODEL;
use crate::stt::models::DEFAULT_MODEL;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub api_key: String,
    /// How the key was obtained: "manual" | "oauth" | "env" | "".
    pub auth_method: String,
    pub model: String,
    pub device: Option<String>,
    /// Optional ISO-639-1 hint; skipping auto-detect shaves latency.
    pub language: Option<String>,
    /// Global STT binding, e.g. "key:F9" or "mouse:3".
    pub hotkey: Option<String>,
    /// "push_to_talk" (default): listen while held. "toggle": press to
    /// start, press again to stop.
    pub hotkey_mode: String,
    /// Global binding that synthesizes an Enter keypress — bind a mouse
    /// side button to send dictated messages without touching the keyboard.
    pub enter_hotkey: Option<String>,
    /// Type the finished transcript into the focused app (Whisper Flow
    /// behavior). Default on — this is the app's core dictation flow.
    pub type_into_app: bool,
    /// Start with the main window hidden (tray only) so launch never steals
    /// focus. Default on — the app is a background service first.
    pub start_minimized: bool,
    /// Register as a login item.
    pub launch_at_login: bool,
    /// Strip filler words (um, uh, …) from transcripts before typing.
    pub clean_fillers: bool,
    /// User vocabulary terms, merged with the built-in seed list and sent
    /// as model biasing (Deepgram keyterms / Whisper prompt).
    pub vocabulary: Vec<String>,
    /// Smart corrections: run the final transcript through a fast LLM to
    /// fix homophones and tech terms using context. Adds ~300ms + a
    /// fraction of a cent per dictation. Off by default (latency-first).
    pub smart_corrections: bool,
    /// OpenRouter model for the smart-corrections pass.
    pub cleanup_model: String,
}

impl Default for Settings {
    fn default() -> Self {
        let env_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
        Self {
            auth_method: if env_key.is_empty() { String::new() } else { "env".into() },
            api_key: env_key,
            model: DEFAULT_MODEL.to_string(),
            device: None,
            language: None,
            hotkey: None,
            hotkey_mode: "push_to_talk".into(),
            enter_hotkey: None,
            type_into_app: true,
            start_minimized: true,
            launch_at_login: false,
            clean_fillers: true,
            vocabulary: Vec::new(),
            smart_corrections: false,
            cleanup_model: DEFAULT_CLEANUP_MODEL.into(),
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("openroutervoice").join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        let Some(path) = settings_path() else {
            return Self::default();
        };
        let Ok(raw) = fs::read_to_string(&path) else {
            return Self::default();
        };
        let mut settings: Settings = serde_json::from_str(&raw).unwrap_or_default();
        // Env var fills in only when no stored credential exists.
        if settings.api_key.is_empty() {
            if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
                if !key.is_empty() {
                    settings.api_key = key;
                    settings.auth_method = "env".into();
                }
            }
        }
        settings
    }

    pub fn save(&self) {
        let Some(path) = settings_path() else { return };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut to_store = self.clone();
        // Never persist a key that came from the environment.
        if to_store.auth_method == "env" {
            to_store.api_key = String::new();
            to_store.auth_method = String::new();
        }
        if let Ok(json) = serde_json::to_string_pretty(&to_store) {
            let _ = fs::write(path, json);
        }
    }
}
