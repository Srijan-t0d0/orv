//! Tauri commands exposed to the frontend.

use tauri::{AppHandle, State};

use crate::audio::capture;
use crate::hotkey;
use crate::settings::Settings;
use crate::stt::models::{SttModel, MODELS};
use crate::AppState;

#[tauri::command]
pub fn start_listening(app: AppHandle) -> Result<(), String> {
    super::start_impl(&app)
}

#[tauri::command]
pub fn stop_listening(app: AppHandle) -> Result<(), String> {
    super::stop_impl(&app);
    Ok(())
}

#[tauri::command]
pub fn list_models() -> Vec<SttModel> {
    MODELS.to_vec()
}

#[tauri::command]
pub fn list_input_devices() -> Vec<String> {
    capture::list_input_devices()
}

/// Settings as sent to the webview: the raw key never leaves the backend.
#[derive(serde::Serialize)]
pub struct UiSettings {
    pub has_credentials: bool,
    pub auth_method: String,
    pub key_preview: String,
    pub model: String,
    pub device: Option<String>,
    pub language: Option<String>,
    pub hotkey: Option<String>,
    pub hotkey_label: Option<String>,
    pub hotkey_mode: String,
    pub enter_hotkey: Option<String>,
    pub enter_hotkey_label: Option<String>,
    pub type_into_app: bool,
    pub start_minimized: bool,
    pub launch_at_login: bool,
    pub clean_fillers: bool,
    pub vocabulary: Vec<String>,
    /// Built-in seed terms (read-only, shown for reference).
    pub seed_count: usize,
    pub smart_corrections: bool,
}

/// Mutable subset the webview is allowed to change.
#[derive(serde::Deserialize)]
pub struct UiSettingsPatch {
    pub model: Option<String>,
    pub device: Option<Option<String>>,
    pub language: Option<Option<String>>,
    pub hotkey_mode: Option<String>,
    pub type_into_app: Option<bool>,
    pub start_minimized: Option<bool>,
    pub launch_at_login: Option<bool>,
    pub clean_fillers: Option<bool>,
    pub vocabulary: Option<Vec<String>>,
    pub smart_corrections: Option<bool>,
}

fn to_ui(settings: &Settings) -> UiSettings {
    let key_preview = if settings.api_key.len() > 12 {
        format!(
            "{}…{}",
            &settings.api_key[..8],
            &settings.api_key[settings.api_key.len() - 4..]
        )
    } else if !settings.api_key.is_empty() {
        "•••".into()
    } else {
        String::new()
    };
    UiSettings {
        has_credentials: !settings.api_key.is_empty(),
        auth_method: settings.auth_method.clone(),
        key_preview,
        model: settings.model.clone(),
        device: settings.device.clone(),
        language: settings.language.clone(),
        hotkey: settings.hotkey.clone(),
        hotkey_label: settings.hotkey.as_deref().map(hotkey::label),
        hotkey_mode: settings.hotkey_mode.clone(),
        enter_hotkey: settings.enter_hotkey.clone(),
        enter_hotkey_label: settings.enter_hotkey.as_deref().map(hotkey::label),
        type_into_app: settings.type_into_app,
        start_minimized: settings.start_minimized,
        launch_at_login: settings.launch_at_login,
        clean_fillers: settings.clean_fillers,
        vocabulary: settings.vocabulary.clone(),
        seed_count: crate::stt::vocab::SEED_TERMS.len(),
        smart_corrections: settings.smart_corrections,
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> UiSettings {
    to_ui(&state.settings.lock().unwrap())
}

#[tauri::command]
pub fn set_settings(app: AppHandle, state: State<'_, AppState>, patch: UiSettingsPatch) -> UiSettings {
    let mut settings = state.settings.lock().unwrap();
    if let Some(model) = patch.model {
        settings.model = model;
    }
    if let Some(device) = patch.device {
        settings.device = device.filter(|d| !d.is_empty());
    }
    if let Some(language) = patch.language {
        settings.language = language.filter(|l| !l.is_empty());
    }
    if let Some(mode) = patch.hotkey_mode {
        if mode == "push_to_talk" || mode == "toggle" {
            settings.hotkey_mode = mode;
        }
    }
    if let Some(type_into) = patch.type_into_app {
        settings.type_into_app = type_into;
    }
    if let Some(minimized) = patch.start_minimized {
        settings.start_minimized = minimized;
    }
    if let Some(clean) = patch.clean_fillers {
        settings.clean_fillers = clean;
    }
    if let Some(vocab) = patch.vocabulary {
        settings.vocabulary = vocab
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
    }
    if let Some(smart) = patch.smart_corrections {
        settings.smart_corrections = smart;
    }
    if let Some(login) = patch.launch_at_login {
        settings.launch_at_login = login;
        use tauri_plugin_autostart::ManagerExt;
        let autostart = app.autolaunch();
        let _ = if login { autostart.enable() } else { autostart.disable() };
    }
    settings.save();
    to_ui(&settings)
}

#[tauri::command]
pub fn set_api_key(state: State<'_, AppState>, key: String) -> Result<UiSettings, String> {
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err("empty key".into());
    }
    let mut settings = state.settings.lock().unwrap();
    settings.api_key = key;
    settings.auth_method = "manual".into();
    settings.save();
    Ok(to_ui(&settings))
}

#[tauri::command]
pub fn clear_credentials(state: State<'_, AppState>) -> UiSettings {
    let mut settings = state.settings.lock().unwrap();
    settings.api_key = String::new();
    settings.auth_method = String::new();
    settings.save();
    to_ui(&settings)
}

#[tauri::command]
pub fn login_with_openrouter(app: AppHandle) -> Result<(), String> {
    crate::auth::begin_login(app)
}

#[tauri::command]
pub fn bind_hotkey(app: AppHandle, target: Option<String>) {
    let target = match target.as_deref() {
        Some("enter") => hotkey::Target::Enter,
        _ => hotkey::Target::Stt,
    };
    hotkey::begin_capture(&app, target);
}

#[tauri::command]
pub fn bench_record_start(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    // Don't fight the live pipeline for the microphone.
    if state.pipeline.lock().unwrap().is_some() {
        return Err("stop listening before recording a benchmark clip".into());
    }
    let device = state.settings.lock().unwrap().device.clone();
    let recorder = crate::benchmark::start_recording(app, device)?;
    *state.bench_recorder.lock().unwrap() = Some(recorder);
    Ok(())
}

#[tauri::command]
pub fn bench_record_stop(state: State<'_, AppState>) {
    if let Some(recorder) = state.bench_recorder.lock().unwrap().take() {
        recorder.stop();
    }
}

#[tauri::command]
pub fn bench_run(app: AppHandle, models: Option<Vec<String>>) -> Result<(), String> {
    crate::benchmark::run(app, models.unwrap_or_default())
}

#[tauri::command]
pub fn bench_clip_info() -> Option<f32> {
    crate::benchmark::clip_secs()
}

/// Called by overlay.js once its fade-out animation finishes.
#[tauri::command]
pub fn overlay_hide(app: AppHandle) {
    crate::overlay::hide(&app);
}

/// Recent transcript history (oldest first) for rendering on startup.
#[tauri::command]
pub fn get_history() -> Vec<crate::history::HistoryEntry> {
    crate::history::load_recent()
}

/// Last completed benchmark run, if any (results survive restarts).
#[tauri::command]
pub fn get_last_benchmark() -> Option<crate::benchmark::BenchRun> {
    crate::benchmark::load_last_run()
}

#[tauri::command]
pub fn clear_hotkey(app: AppHandle, target: Option<String>) {
    let target = match target.as_deref() {
        Some("enter") => hotkey::Target::Enter,
        _ => hotkey::Target::Stt,
    };
    hotkey::clear(&app, target);
}
