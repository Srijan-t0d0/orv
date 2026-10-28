pub mod commands;
pub mod text;

pub use text::reset_continuity;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::audio::{capture, encode, vad, TARGET_SAMPLE_RATE};
use crate::stt::client::SttClient;
use crate::stt::models::{current, Transport};
use crate::AppState;

/// Monotonic id per listening session.
static UTTERANCE_SEQ: AtomicU64 = AtomicU64::new(1);

/// Cap the recording (2 min) so a stuck key can't grow it unbounded.
const MAX_HOLD_SAMPLES: usize = TARGET_SAMPLE_RATE as usize * 120;
/// Overlay level-meter update cadence.
const LEVEL_INTERVAL: Duration = Duration::from_millis(60);

/// Running pipeline; dropping/stopping tears everything down.
pub struct PipelineHandle {
    stop: Arc<AtomicBool>,
    /// When the user released the hotkey (or clicked stop): t=0 of the timeline.
    stopped_at: Arc<OnceLock<Instant>>,
}

impl PipelineHandle {
    pub fn stop(&self) {
        let _ = self.stopped_at.set(Instant::now());
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[derive(Clone, Serialize)]
struct TranscriptEvent {
    id: u64,
    text: String,
    /// Hotkey release → text inserted (or ready, when not typing), in ms.
    total_ms: u64,
    finish_ms: u64,
    prep_ms: u64,
    /// HTTP round-trip portion.
    api_ms: u64,
    stt_ttfb_ms: u64,
    insert_ms: Option<u64>,
    audio_secs: f32,
    model: String,
    llm: Option<crate::history::LlmStats>,
}

/// Microphone level for the overlay waveform (0.0–1.0).
#[derive(Clone, Serialize)]
struct LevelEvent {
    level: f32,
}

/// A likely mis-heard term found in a transcript, suggested for the
/// user's vocabulary.
#[derive(Clone, Serialize)]
struct VocabSuggestion {
    heard: String,
    term: String,
}

#[derive(Clone, Serialize)]
struct StatusEvent {
    state: String, // "listening" | "speech" | "transcribing" | "stopped" | "error"
    detail: Option<String>,
}

fn emit_status(app: &AppHandle, state: &str, detail: Option<String>) {
    // The floating indicator tracks the pipeline lifecycle: visible from
    // the moment listening starts; overlay.js requests hide after showing
    // the final transcript (or immediately on stop with nothing pending).
    match state {
        "listening" | "speech" | "transcribing" => crate::platform::overlay::show(app),
        _ => {}
    }
    // Mirror state in the menu-bar tray (the app is usually windowless).
    if let Some(tray) = app.try_state::<crate::platform::tray::Tray>() {
        tray.set_status(state);
    }
    let _ = app.emit(
        "stt-status",
        StatusEvent {
            state: state.into(),
            detail,
        },
    );
}

pub struct PipelineConfig {
    pub api_key: String,
    pub model: String,
    pub device: Option<String>,
    pub language: Option<String>,
    pub vocabulary: Vec<String>,
    /// Accent + vocabulary text for prompts (see `vocab::hints`).
    pub hints: String,
    pub target_pid: Option<i32>,
}

/// Start listening using current settings. Callable from any thread
/// (Tauri command or the global hotkey listener).
pub fn start_impl(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut pipeline = state.pipeline.lock().unwrap();
    if pipeline.is_some() {
        eprintln!("[pipeline] start ignored: already listening");
        return Err("already listening".into());
    }
    eprintln!("[pipeline] starting…");

    let settings = state.settings.lock().unwrap().clone();
    if settings.api_key.is_empty() {
        emit_status(
            app,
            "error",
            Some("no credentials — log in with OpenRouter or paste an API key in settings".into()),
        );
        return Err("no API key".into());
    }

    #[cfg(target_os = "macos")]
    let target_pid = crate::platform::paste::frontmost_pid();
    #[cfg(not(target_os = "macos"))]
    let target_pid = None;

    let handle = start(
        app.clone(),
        state.stt.clone(),
        PipelineConfig {
            api_key: settings.api_key,
            model: settings.model,
            device: settings.device,
            language: settings.language,
            hints: crate::stt::vocab::hints(&settings.vocabulary, &settings.accent),
            vocabulary: settings.vocabulary,
            target_pid,
        },
    )?;
    *pipeline = Some(handle);
    Ok(())
}

/// Stop listening if active. Returns true if a pipeline was stopped.
pub fn stop_impl(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let mut pipeline = state.pipeline.lock().unwrap();
    if let Some(handle) = pipeline.take() {
        handle.stop();
        true
    } else {
        false
    }
}

/// Toggle listening — used by the device-wide hotkey.
pub fn toggle(app: &AppHandle) {
    if !stop_impl(app) {
        let _ = start_impl(app);
    }
}

/// Spawn the capture → transcribe pipeline.
///
/// One listening session = ONE transcript (Whisper Flow model):
/// - all audio from start to stop accumulates in a single buffer
/// - on stop, one pass over the full buffer produces the transcript that is
///   typed/emitted exactly once. There is no live preview: re-sending the
///   growing buffer while recording billed ~4.5x the spoken audio.
/// VAD is used only to detect whether any speech happened at all and for
/// status ("speech" vs silence), never to split audio.
pub fn start(app: AppHandle, stt: SttClient, cfg: PipelineConfig) -> Result<PipelineHandle, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let stopped_at = Arc::new(OnceLock::new());
    let stopped_at_thread = stopped_at.clone();

    let (audio_tx, audio_rx) = crossbeam_channel::bounded::<Vec<f32>>(64);

    // Tokio runtime dedicated to in-flight transcription requests, so a slow
    // response never blocks audio processing.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    std::thread::Builder::new()
        .name("stt-pipeline".into())
        .spawn(move || {
            // Capture must be created on this thread (cpal streams are not Send
            // on all platforms) and lives until the loop exits.
            let capture_handle = match capture::start(cfg.device.as_deref(), audio_tx) {
                Ok(h) => h,
                Err(e) => {
                    emit_status(&app, "error", Some(format!("audio capture: {e}")));
                    return;
                }
            };

            let mut resampler = match encode::StreamResampler::new(capture_handle.sample_rate) {
                Ok(r) => r,
                Err(e) => {
                    emit_status(&app, "error", Some(format!("resampler: {e}")));
                    return;
                }
            };

            let mut speech_gate = match vad::Segmenter::new() {
                Ok(s) => s,
                Err(e) => {
                    emit_status(&app, "error", Some(format!("VAD init: {e}")));
                    return;
                }
            };

            eprintln!(
                "[pipeline] listening on '{}' @ {} Hz (model {})",
                capture_handle.device_name,
                resampler.input_rate(),
                cfg.model
            );
            emit_status(
                &app,
                "listening",
                Some(format!(
                    "{} @ {} Hz",
                    capture_handle.device_name,
                    resampler.input_rate()
                )),
            );

            let session_id = UTTERANCE_SEQ.fetch_add(1, Ordering::Relaxed);
            let mut hold_buffer: Vec<f32> = Vec::new();
            let mut heard_speech = false;
            let mut last_level = Instant::now();

            loop {
                if stop_thread.load(Ordering::SeqCst) {
                    break;
                }

                let chunk = match audio_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(c) => c,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                };

                let samples_16k = resampler.process(&chunk);
                if samples_16k.is_empty() {
                    continue;
                }

                // Level meter for the overlay waveform (RMS over this chunk).
                if last_level.elapsed() >= LEVEL_INTERVAL {
                    last_level = Instant::now();
                    let rms = (samples_16k.iter().map(|s| s * s).sum::<f32>()
                        / samples_16k.len() as f32)
                        .sqrt();
                    // Perceptual-ish scaling: speech RMS is typically 0.02–0.2.
                    let level = (rms * 8.0).min(1.0);
                    let _ = app.emit("stt-level", LevelEvent { level });
                }

                // Speech gate: status + "did anything worth transcribing happen".
                for event in speech_gate.push(&samples_16k) {
                    if let vad::VadEvent::SpeechStart = event {
                        if !heard_speech {
                            eprintln!("[pipeline] VAD: speech detected");
                        }
                        heard_speech = true;
                        emit_status(&app, "speech", None);
                    }
                }

                if hold_buffer.len() < MAX_HOLD_SAMPLES {
                    hold_buffer.extend_from_slice(&samples_16k);
                }
            }

            drop(capture_handle);
            let audio_secs = hold_buffer.len() as f32 / TARGET_SAMPLE_RATE as f32;
            eprintln!("[pipeline] stopped after {audio_secs:.2}s (speech: {heard_speech})");

            // Final authoritative pass over the entire session.
            if heard_speech && hold_buffer.len() > TARGET_SAMPLE_RATE as usize / 5 {
                emit_status(&app, "transcribing", None);
                let end_of_speech = Instant::now();
                let released = stopped_at_thread.get().copied().unwrap_or(end_of_speech);
                let finish_ms = end_of_speech.duration_since(released).as_millis() as u64;
                let wav = encode::wav_encode_16k(&hold_buffer);
                eprintln!(
                    "[stt] #{session_id} FINAL pass ({audio_secs:.2}s, {} KB) to {}",
                    wav.len() / 1024,
                    cfg.model
                );
                let prep_ms = end_of_speech.elapsed().as_millis() as u64;
                let result = rt.block_on(transcribe_any(
                    &stt,
                    &cfg.api_key,
                    &cfg.model,
                    &wav,
                    audio_secs,
                    cfg.language.as_deref(),
                    &cfg.hints,
                ));
                match result {
                    Ok(t) => {
                        let mut text = t.text.trim().to_string();
                        let (do_clean, do_smart, cleanup_model) = {
                            let state = app.state::<AppState>();
                            let s = state.settings.lock().unwrap();
                            (s.clean_fillers, s.smart_corrections, s.cleanup_model.clone())
                        };
                        if do_clean {
                            text = text::clean_fillers(&text);
                        }
                        // Smart corrections: context-aware LLM fixup for
                        // homophones and tech terms. Falls back to the raw
                        // transcript on any failure or timeout.
                        let mut llm = None;
                        if do_smart && !text.is_empty() {
                            let started = Instant::now();
                            let mut stats = crate::history::LlmStats {
                                model: cleanup_model.clone(),
                                ttfb_ms: None,
                                ms: 0,
                                error: None,
                            };
                            match rt.block_on(stt.smart_correct(&cfg.api_key, &cleanup_model, &text, &cfg.hints)) {
                                Ok(c) => {
                                    stats.ttfb_ms = Some(c.ttfb_ms);
                                    stats.ms = c.api_ms;
                                    text = c.text;
                                }
                                Err(e) => {
                                    eprintln!("[smart] skipped: {e}");
                                    stats.ms = started.elapsed().as_millis() as u64;
                                    stats.error = Some(e);
                                }
                            }
                            llm = Some(stats);
                        }
                        // PRIVACY: log timing/length only, never transcript
                        // content — dictations must not accumulate in logs.
                        eprintln!(
                            "[stats] #{session_id} stt {} ttfb {} ms / {} ms | llm {} ({} chars)",
                            t.model,
                            t.ttfb_ms,
                            t.api_ms,
                            llm.as_ref().map_or("off".into(), |l| format!(
                                "ttfb {} ms / {} ms{}",
                                l.ttfb_ms.map_or("-".into(), |v| v.to_string()),
                                l.ms,
                                if l.error.is_some() { " (failed)" } else { "" }
                            )),
                            text.chars().count()
                        );
                        if text.is_empty() {
                            eprintln!("[stt] #{session_id} EMPTY final transcript");
                        } else {
                            // Mine the transcript for probable mis-hearings
                            // and surface them as vocabulary suggestions.
                            let suggestions = crate::stt::vocab::suggest_terms(&text, &cfg.vocabulary);
                            if !suggestions.is_empty() {
                                eprintln!("[vocab] {} suggestion(s) from this transcript", suggestions.len());
                                let payload: Vec<VocabSuggestion> = suggestions
                                    .into_iter()
                                    .map(|(heard, term)| VocabSuggestion { heard, term })
                                    .collect();
                                let _ = app.emit("vocab-suggestions", payload);
                            }
                            let type_into_app =
                                app.state::<AppState>().settings.lock().unwrap().type_into_app;
                            // Wait only until the keystroke is sent, not for the
                            // clipboard-restore grace that follows it.
                            let mut insert_ms = None;
                            if type_into_app {
                                eprintln!("[stt] #{session_id} typing transcript into focused app");
                                let started = Instant::now();
                                let sent = text::type_text(&text, cfg.target_pid, app.clone());
                                if sent.recv_timeout(Duration::from_secs(3)).is_ok() {
                                    insert_ms = Some(started.elapsed().as_millis() as u64);
                                }
                            }
                            let total_ms = released.elapsed().as_millis() as u64;
                            // Durable history: survives restarts.
                            crate::history::append(&crate::history::HistoryEntry {
                                at: std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_millis() as u64)
                                    .unwrap_or(0),
                                text: text.clone(),
                                total_ms,
                                finish_ms: Some(finish_ms),
                                prep_ms: Some(prep_ms),
                                api_ms: t.api_ms,
                                stt_ttfb_ms: Some(t.ttfb_ms),
                                insert_ms,
                                audio_secs: t.audio_secs,
                                model: t.model.clone(),
                                llm: llm.clone(),
                            });
                            let _ = app.emit(
                                "stt-transcript",
                                TranscriptEvent {
                                    id: session_id,
                                    text,
                                    total_ms,
                                    finish_ms,
                                    prep_ms,
                                    api_ms: t.api_ms,
                                    stt_ttfb_ms: t.ttfb_ms,
                                    insert_ms,
                                    audio_secs: t.audio_secs,
                                    model: t.model,
                                    llm,
                                },
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!("[stt] #{session_id} final FAILED: {e}");
                        emit_status(&app, "error", Some(e));
                    }
                }
            } else {
                eprintln!("[pipeline] no speech in session — nothing to transcribe");
            }

            rt.shutdown_timeout(Duration::from_secs(10));
            emit_status(&app, "stopped", None);
        })
        .map_err(|e| e.to_string())?;

    Ok(PipelineHandle { stop, stopped_at })
}

/// Transcribe via whichever transport the model uses. Rolling preview
/// ignores streaming deltas (each pass's full text is the preview).
async fn transcribe_any(
    stt: &SttClient,
    api_key: &str,
    model: &str,
    wav: &[u8],
    audio_secs: f32,
    language: Option<&str>,
    hints: &str,
) -> Result<crate::stt::client::Transcription, String> {
    let streaming = current()
        .iter()
        .find(|m| m.id == model)
        .map(|m| m.transport == Transport::ChatStream)
        .unwrap_or(false);
    if streaming {
        stt.transcribe_streaming(api_key, model, wav, audio_secs, hints, |_| {}).await
    } else {
        stt.transcribe(api_key, model, wav, audio_secs, language).await
    }
}
