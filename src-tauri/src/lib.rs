mod audio;
mod auth;
mod benchmark;
mod history;
mod platform;
mod pipeline;
mod settings;
mod stt;

use std::sync::Mutex;

use pipeline::PipelineHandle;
use settings::Settings;
use stt::client::SttClient;
use tauri::Manager;

/// Shared app state managed by Tauri.
pub struct AppState {
    pub pipeline: Mutex<Option<PipelineHandle>>,
    pub stt: SttClient,
    pub settings: Mutex<Settings>,
    pub bench_recorder: Mutex<Option<benchmark::Recorder>>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let settings = Settings::load();
    let stt = SttClient::new();

    // Pre-warm the TLS connection to OpenRouter as early as possible: a cold
    // handshake can cost more wall-clock time than the transcription itself.
    stt.prewarm();

    // Restore the saved hotkey bindings before the listener starts.
    platform::hotkey::set_bindings_from_settings(settings.hotkey.clone(), settings.enter_hotkey.clone());

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(AppState {
            pipeline: Mutex::new(None),
            stt,
            settings: Mutex::new(settings),
            bench_recorder: Mutex::new(None),
        })
        .setup(|app| {
            // Device-wide hotkey listener (keyboard + mouse) for the app's
            // lifetime. On macOS this needs Input Monitoring permission.
            // The raw HID listener sees mouse side buttons that driver
            // software hides from the CGEvent layer.
            #[cfg(target_os = "macos")]
            platform::hid::spawn_listener();
            platform::hotkey::spawn_listener(app.handle().clone());
            // Center the macOS traffic lights in the custom header band.
            #[cfg(target_os = "macos")]
            platform::traffic::setup(app.handle());
            // Menu-bar presence — the app is primarily a background service.
            platform::tray::setup(app.handle())?;

            let settings = app.state::<AppState>().settings.lock().unwrap().clone();

            // Background-first: start hidden in the tray so launch (or login)
            // never steals focus from whatever the user is doing.
            if settings.start_minimized {
                platform::tray::hide_main_window(app.handle());
            }

            // Sync the login-item registration with the setting.
            {
                use tauri_plugin_autostart::ManagerExt;
                let autostart = app.autolaunch();
                let registered = autostart.is_enabled().unwrap_or(false);
                if settings.launch_at_login && !registered {
                    let _ = autostart.enable();
                } else if !settings.launch_at_login && registered {
                    let _ = autostart.disable();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Close-to-tray: the red button hides the window; the service
            // (hotkeys, overlay, pipeline) keeps running. Quit via the tray.
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    platform::tray::hide_main_window(window.app_handle());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            pipeline::commands::start_listening,
            pipeline::commands::stop_listening,
            pipeline::commands::list_models,
            pipeline::commands::get_settings,
            pipeline::commands::set_settings,
            pipeline::commands::set_api_key,
            pipeline::commands::clear_credentials,
            pipeline::commands::login_with_openrouter,
            pipeline::commands::bind_hotkey,
            pipeline::commands::clear_hotkey,
            pipeline::commands::list_input_devices,
            pipeline::commands::bench_record_start,
            pipeline::commands::bench_record_stop,
            pipeline::commands::bench_run,
            pipeline::commands::bench_clip_info,
            pipeline::commands::overlay_hide,
            pipeline::commands::get_history,
            pipeline::commands::get_last_benchmark,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
