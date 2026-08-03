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

#[derive(Clone, Serialize)]
struct BenchResult {
    model: String,
    ok: bool,
    api_ms: u64,
    text: String,
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

/// Send the stored clip to every model sequentially (one in flight at a
/// time so results aren't skewed by contention). Emits a result per model.
pub fn run(app: AppHandle) -> Result<(), String> {
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

            emit(&app, "running", Some(format!("{} models", MODELS.len())), Some(secs));
            for model in MODELS {
                let result = rt.block_on(async {
                    match model.transport {
                        Transport::Transcription => {
                            stt.transcribe(&api_key, model.id, &wav, secs, language.as_deref()).await
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
                let _ = app.emit("bench-result", payload);
            }
            emit(&app, "done", None, Some(secs));
        })
        .map_err(|e| e.to_string())?;

    Ok(())
}
