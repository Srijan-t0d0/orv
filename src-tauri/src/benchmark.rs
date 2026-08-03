//! In-app model benchmark.
//!
//! The user records a reference clip once (their voice, their mic, their
//! room); `run` then sends that exact clip to every model in the registry
//! and reports per-model round-trip latency plus the transcript, so speed
//! comparisons are empirical rather than editorial.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::audio::{capture, encode, TARGET_SAMPLE_RATE};
use crate::stt::models::{Transport, MODELS};
use crate::AppState;

fn clip_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("openroutervoice").join("benchmark.wav"))
}

/// Duration of the stored clip in seconds, if one exists.
pub fn clip_secs() -> Option<f32> {
    let path = clip_path()?;
    let reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();
    Some(reader.duration() as f32 / spec.sample_rate as f32)
}

pub struct Recorder {
    stop: Arc<AtomicBool>,
}

impl Recorder {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[derive(Clone, Serialize)]
struct BenchEvent {
    state: String, // "recording" | "recorded" | "running" | "result" | "done" | "error"
    detail: Option<String>,
    clip_secs: Option<f32>,
}

#[derive(Clone, Serialize, serde::Deserialize)]
pub struct BenchResult {
    model: String,
    ok: bool,
    api_ms: u64,
    text: String,
}

/// A completed benchmark run, persisted so results survive restarts.
#[derive(Clone, Serialize, serde::Deserialize)]
pub struct BenchRun {
    /// Unix millis when the run finished.
    pub at: u64,
    pub clip_secs: f32,
    pub results: Vec<BenchResult>,
}

fn results_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("openroutervoice").join("benchmark.json"))
}

/// Load the last persisted run, if any.
pub fn load_last_run() -> Option<BenchRun> {
    let raw = fs::read_to_string(results_path()?).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_run(run: &BenchRun) {
    let Some(path) = results_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(run) {
        let _ = fs::write(path, json);
    }
}

fn emit(app: &AppHandle, state: &str, detail: Option<String>, clip: Option<f32>) {
    let _ = app.emit(
        "bench-status",
        BenchEvent {
            state: state.into(),
            detail,
            clip_secs: clip,
        },
    );
}

/// Record the benchmark clip from the configured input device until stopped
/// (30 s hard cap).
pub fn start_recording(app: AppHandle, device: Option<String>) -> Result<Recorder, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();

    std::thread::Builder::new()
        .name("bench-record".into())
        .spawn(move || {
            let (tx, rx) = crossbeam_channel::bounded::<Vec<f32>>(64);
            let handle = match capture::start(device.as_deref(), tx) {
                Ok(h) => h,
                Err(e) => return emit(&app, "error", Some(format!("recording: {e}")), None),
            };
            let mut resampler = match encode::StreamResampler::new(handle.sample_rate) {
                Ok(r) => r,
                Err(e) => return emit(&app, "error", Some(e), None),
            };

            emit(&app, "recording", None, None);
            let mut samples: Vec<f32> = Vec::new();
            let cap = TARGET_SAMPLE_RATE as usize * 30;
            while !stop_thread.load(Ordering::SeqCst) && samples.len() < cap {
                match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(chunk) => samples.extend(resampler.process(&chunk)),
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }
            drop(handle);

            if samples.len() < TARGET_SAMPLE_RATE as usize / 2 {
                return emit(&app, "error", Some("clip too short — record at least half a second".into()), None);
            }
            let Some(path) = clip_path() else {
                return emit(&app, "error", Some("no config dir".into()), None);
            };
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let wav = encode::wav_encode_16k(&samples);
            if let Err(e) = fs::write(&path, wav) {
                return emit(&app, "error", Some(format!("saving clip: {e}")), None);
            }
            let secs = samples.len() as f32 / TARGET_SAMPLE_RATE as f32;
            emit(&app, "recorded", None, Some(secs));
        })
        .map_err(|e| e.to_string())?;

    Ok(Recorder { stop })
}

/// Send the stored clip to the selected models sequentially (one in flight
/// at a time so results aren't skewed by contention). Emits a result per
/// model. `model_ids` empty = all models.
pub fn run(app: AppHandle, model_ids: Vec<String>) -> Result<(), String> {
    let path = clip_path().ok_or("no config dir")?;
    let wav = fs::read(&path).map_err(|_| "no benchmark clip — record one first")?;
    let secs = clip_secs().unwrap_or(0.0);

    let state = app.state::<AppState>();
    let api_key = state.settings.lock().unwrap().api_key.clone();
    if api_key.is_empty() {
        return Err("no credentials".into());
    }
    let stt = state.stt.clone();
    let language = state.settings.lock().unwrap().language.clone();

    std::thread::Builder::new()
        .name("bench-run".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => return emit(&app, "error", Some(e.to_string()), None),
            };

            let selected: Vec<_> = MODELS
                .iter()
                .filter(|m| model_ids.is_empty() || model_ids.iter().any(|id| id == m.id))
                .collect();
            emit(&app, "running", Some(format!("{} models", selected.len())), Some(secs));
            let mut all_results: Vec<BenchResult> = Vec::new();
            for model in selected {
                let result = rt.block_on(async {
                    match model.transport {
                        Transport::Transcription => {
                            stt.transcribe(&api_key, model.id, &wav, secs, language.as_deref(), &[]).await
                        }
                        Transport::ChatStream => {
                            stt.transcribe_streaming(&api_key, model.id, &wav, secs, |_| {}).await
                        }
                    }
                });
                let payload = match result {
                    Ok(t) => BenchResult {
                        model: model.id.to_string(),
                        ok: true,
                        api_ms: t.api_ms,
                        text: t.text.chars().take(120).collect(),
                    },
                    Err(e) => BenchResult {
                        model: model.id.to_string(),
                        ok: false,
                        api_ms: 0,
                        text: e.chars().take(120).collect(),
                    },
                };
                all_results.push(payload.clone());
                let _ = app.emit("bench-result", payload);
            }
            // Durable: results survive restarts and re-hydrate the UI.
            save_run(&BenchRun {
                at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
                clip_secs: secs,
                results: all_results,
            });
            emit(&app, "done", None, Some(secs));
        })
        .map_err(|e| e.to_string())?;

    Ok(())
}
