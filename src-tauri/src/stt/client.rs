//! OpenRouter speech-to-text client.
//!
//! POST /api/v1/audio/transcriptions with base64 WAV. The client keeps a
//! warm connection pool: the TLS handshake to openrouter.ai is paid once at
//! startup instead of on the first utterance.

use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};

const BASE_URL: &str = "https://openrouter.ai/api/v1";
const REFERER: &str = "https://github.com/OpenRouterLabs/orv";
const TITLE: &str = "OpenRouter Voice";

#[derive(Clone)]
pub struct SttClient {
    http: reqwest::Client,
}

#[derive(Serialize)]
struct InputAudio<'a> {
    data: &'a str,
    format: &'a str,
}

#[derive(Serialize)]
struct TranscriptionRequest<'a> {
    model: &'a str,
    input_audio: InputAudio<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a str>,
    /// Decoder biasing for Whisper/GPT-4o-style models: a vocabulary
    /// sentence nudges them toward the right spellings of technical terms.
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<String>,
    /// Provider passthrough — Deepgram's keyterm biasing rides here.
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

#[derive(Clone, Serialize)]
pub struct Transcription {
    pub text: String,
    /// Request sent → first response byte (first text delta when streaming).
    pub ttfb_ms: u64,
    /// Round-trip time of the HTTP request, in milliseconds.
    pub api_ms: u64,
    /// Seconds of audio that were transcribed.
    pub audio_secs: f32,
    pub model: String,
}

/// The prompt for chat-transport transcription. Kept terse so models don't
/// editorialize.
const CHAT_TRANSCRIBE_PROMPT: &str =
    "Transcribe this audio verbatim. Output only the transcript text, nothing else.";

/// Model for the "Smart corrections" LLM cleanup pass: cheapest of the
/// current Gemini Flash Lite line — ~a hundredth of a cent per dictation.
pub const CLEANUP_MODEL: &str = "google/gemini-3.1-flash-lite";

/// A smart-corrections result with the same timing split as `Transcription`.
pub struct Correction {
    pub text: String,
    pub ttfb_ms: u64,
    pub api_ms: u64,
}

/// Parse an SSE chat stream (`data: {json}` lines ending in `data: [DONE]`),
/// calling `on_delta` per text fragment; returns the full text.
async fn read_sse(mut resp: reqwest::Response, mut on_delta: impl FnMut(&str)) -> Result<String, String> {
    let mut full = String::new();
    let mut buf = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("stream read failed: {e}"))?
    {
        buf.extend_from_slice(&chunk);
        while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            let Some(payload) = line.strip_prefix("data: ") else {
                continue;
            };
            if payload == "[DONE]" {
                buf.clear();
                break;
            }
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(payload) {
                if let Some(delta) = json["choices"][0]["delta"]["content"].as_str() {
                    if !delta.is_empty() {
                        full.push_str(delta);
                        on_delta(delta);
                    }
                }
            }
        }
    }
    Ok(full)
}

impl SttClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .pool_idle_timeout(Duration::from_secs(300))
            .pool_max_idle_per_host(4)
            .tcp_keepalive(Duration::from_secs(30))
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client");
        Self { http }
    }

    /// Fire-and-forget request that establishes the TLS connection so the
    /// first real transcription doesn't pay the handshake.
    pub fn prewarm(&self) {
        let http = self.http.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("prewarm runtime");
            rt.block_on(async {
                let _ = http.get(format!("{BASE_URL}/models?limit=1")).send().await;
            });
        });
    }

    pub async fn transcribe(
        &self,
        api_key: &str,
        model: &str,
        wav_bytes: &[u8],
        audio_secs: f32,
        language: Option<&str>,
        vocab: &[String],
    ) -> Result<Transcription, String> {
        let data = base64::engine::general_purpose::STANDARD.encode(wav_bytes);

        // Vocabulary biasing, in whichever dialect the model understands.
        let (prompt, provider) = if vocab.is_empty() {
            (None, None)
        } else if model.starts_with("deepgram/") {
            (
                None,
                Some(serde_json::json!({
                    "options": { "deepgram": { "keyterm": super::vocab::keyterms(vocab) } }
                })),
            )
        } else {
            (Some(super::vocab::biasing_prompt(vocab)), None)
        };

        let body = TranscriptionRequest {
            model,
            input_audio: InputAudio {
                data: &data,
                format: "wav",
            },
            language,
            prompt,
            provider,
        };

        let started = Instant::now();
        let resp = self
            .http
            .post(format!("{BASE_URL}/audio/transcriptions"))
            .bearer_auth(api_key)
            .header("HTTP-Referer", REFERER)
            .header("X-Title", TITLE)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        // Non-streaming: headers arrive once the transcript is ready.
        let ttfb_ms = started.elapsed().as_millis() as u64;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("OpenRouter error {status}: {text}"));
        }

        let parsed: TranscriptionResponse = resp
            .json()
            .await
            .map_err(|e| format!("bad response: {e}"))?;

        Ok(Transcription {
            text: parsed.text,
            ttfb_ms,
            api_ms: started.elapsed().as_millis() as u64,
            audio_secs,
            model: model.to_string(),
        })
    }

    /// Streaming transcription via /chat/completions (SSE). Calls `on_delta`
    /// with each text fragment as it arrives; returns the final transcript.
    pub async fn transcribe_streaming(
        &self,
        api_key: &str,
        model: &str,
        wav_bytes: &[u8],
        audio_secs: f32,
        mut on_delta: impl FnMut(&str),
    ) -> Result<Transcription, String> {
        let data = base64::engine::general_purpose::STANDARD.encode(wav_bytes);
        let body = serde_json::json!({
            "model": model,
            "stream": true,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": CHAT_TRANSCRIBE_PROMPT },
                    { "type": "input_audio", "input_audio": { "data": data, "format": "wav" } }
                ]
            }]
        });

        let started = Instant::now();
        let resp = self
            .http
            .post(format!("{BASE_URL}/chat/completions"))
            .bearer_auth(api_key)
            .header("HTTP-Referer", REFERER)
            .header("X-Title", TITLE)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("OpenRouter error {status}: {text}"));
        }

        let mut ttfb_ms = None;
        let full = read_sse(resp, |delta| {
            ttfb_ms.get_or_insert(started.elapsed().as_millis() as u64);
            on_delta(delta);
        })
        .await?;
        let api_ms = started.elapsed().as_millis() as u64;

        Ok(Transcription {
            text: full,
            ttfb_ms: ttfb_ms.unwrap_or(api_ms),
            api_ms,
            audio_secs,
            model: model.to_string(),
        })
    }

    /// "Smart corrections": context-aware cleanup of a finished transcript
    /// by a fast LLM. Fixes what decoder biasing can't — homophones
    /// (eye/I), OpenAI-vs-OpenAPI-class confusions — using the sentence's
    /// meaning. Returns the original text unchanged on any error, so this
    /// can never make a dictation worse than the raw transcript.
    pub async fn smart_correct(
        &self,
        api_key: &str,
        transcript: &str,
        vocab: &[String],
    ) -> Result<Correction, String> {
        let vocab_line = if vocab.is_empty() {
            String::new()
        } else {
            format!("\nKnown vocabulary: {}.", super::vocab::merged_terms(vocab).join(", "))
        };
        let body = serde_json::json!({
            "model": CLEANUP_MODEL,
            "messages": [
                {
                    "role": "system",
                    "content": format!(
                        "You fix speech-to-text transcription errors. Correct misheard \
                         technical terms, homophones, and casing using sentence context. \
                         Do NOT rephrase, summarize, or change the meaning or wording \
                         beyond fixing recognition errors. Preserve the speaker's exact \
                         phrasing. Output ONLY the corrected transcript.{vocab_line}"
                    )
                },
                { "role": "user", "content": transcript }
            ],
            "temperature": 0.0,
            // Streamed only so time-to-first-token is measurable.
            "stream": true
        });

        let started = Instant::now();
        let resp = self
            .http
            .post(format!("{BASE_URL}/chat/completions"))
            .bearer_auth(api_key)
            .header("HTTP-Referer", REFERER)
            .header("X-Title", TITLE)
            .timeout(Duration::from_secs(6)) // cleanup must never stall dictation
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("cleanup request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("cleanup error {}", resp.status()));
        }
        let mut ttfb_ms = None;
        let corrected = read_sse(resp, |_| {
            ttfb_ms.get_or_insert(started.elapsed().as_millis() as u64);
        })
        .await
        .map_err(|e| format!("bad cleanup response: {e}"))?
        .trim()
        .to_string();
        let api_ms = started.elapsed().as_millis() as u64;

        // Sanity guard: an empty or wildly different-length answer means the
        // model editorialized — keep the raw transcript instead.
        let ratio = corrected.len() as f32 / transcript.len().max(1) as f32;
        if corrected.is_empty() || !(0.5..=1.6).contains(&ratio) {
            return Err("cleanup output failed sanity check".into());
        }
        Ok(Correction {
            text: corrected,
            ttfb_ms: ttfb_ms.unwrap_or(api_ms),
            api_ms,
        })
    }
}
