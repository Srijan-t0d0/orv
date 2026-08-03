//! OpenRouter speech-to-text client.
//!
//! POST /api/v1/audio/transcriptions with base64 WAV. The client keeps a
//! warm connection pool: the TLS handshake to openrouter.ai is paid once at
//! startup instead of on the first utterance.

use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};

const BASE_URL: &str = "https://openrouter.ai/api/v1";
const REFERER: &str = "https://github.com/openrouter-examples/openroutervoice";
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
}

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

#[derive(Clone, Serialize)]
pub struct Transcription {
    pub text: String,
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
    ) -> Result<Transcription, String> {
        let data = base64::engine::general_purpose::STANDARD.encode(wav_bytes);
        let body = TranscriptionRequest {
            model,
            input_audio: InputAudio {
                data: &data,
                format: "wav",
            },
            language,
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

        // Parse the SSE stream: lines of `data: {json}` ending in `data: [DONE]`.
        let mut full = String::new();
        let mut buf = Vec::new();
        let mut stream = resp;
        while let Some(chunk) = stream
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

        Ok(Transcription {
            text: full,
            api_ms: started.elapsed().as_millis() as u64,
            audio_secs,
            model: model.to_string(),
        })
    }
}
