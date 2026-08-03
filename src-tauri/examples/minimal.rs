//! Minimal OpenRouter speech-to-text — the whole integration in one file.
//!
//! Records 5 seconds from your default microphone, sends it to OpenRouter's
//! transcription endpoint, prints the transcript. No VAD, no streaming, no
//! UI — just the essential API call the full app is built around.
//!
//! Run it:
//! ```sh
//! export OPENROUTER_API_KEY=sk-or-...
//! cargo run --example minimal
//! ```

use std::sync::mpsc;
use std::time::Duration;

use base64::Engine;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const MODEL: &str = "deepgram/nova-3";
const RECORD_SECS: u64 = 5;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("OPENROUTER_API_KEY")
        .map_err(|_| "set OPENROUTER_API_KEY (get one at https://openrouter.ai/keys)")?;

    // ── 1. Record from the default microphone ──────────────────────────
    let device = cpal::default_host()
        .default_input_device()
        .ok_or("no input device")?;
    let config = device.default_input_config()?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;

    println!("recording {RECORD_SECS}s from '{}'…", device.name()?);
    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    let stream = device.build_input_stream(
        &config.into(),
        move |data: &[f32], _| {
            let _ = tx.send(data.to_vec());
        },
        |e| eprintln!("stream error: {e}"),
        None,
    )?;
    stream.play()?;

    let mut samples: Vec<f32> = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(RECORD_SECS);
    while std::time::Instant::now() < deadline {
        if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(100)) {
            // Downmix interleaved frames to mono.
            samples.extend(chunk.chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32));
        }
    }
    drop(stream);

    // ── 2. Encode as WAV (models expect 16-bit PCM; device rate is fine
    //       for a one-shot — the full app resamples to 16 kHz to shrink
    //       uploads, but it's an optimization, not a requirement) ────────
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut wav = std::io::Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut wav, spec)?;
    for s in &samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;

    // ── 3. POST to OpenRouter's transcription endpoint ─────────────────
    println!("transcribing with {MODEL}…");
    let body = serde_json::json!({
        "model": MODEL,
        "input_audio": {
            "data": base64::engine::general_purpose::STANDARD.encode(wav.into_inner()),
            "format": "wav",
        },
    });
    let started = std::time::Instant::now();
    let resp: serde_json::Value = reqwest::blocking::Client::new()
        .post("https://openrouter.ai/api/v1/audio/transcriptions")
        .bearer_auth(&api_key)
        .json(&body)
        .send()?
        .error_for_status()?
        .json()?;

    // ── 4. Done ────────────────────────────────────────────────────────
    println!(
        "\n{:?}  ({} ms)",
        resp["text"].as_str().unwrap_or("<no text>"),
        started.elapsed().as_millis()
    );
    Ok(())
}
