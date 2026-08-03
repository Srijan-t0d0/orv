# OpenRouter Voice

Example cross-platform desktop app demonstrating **ultra-low-latency speech-to-text**
with the [OpenRouter API](https://openrouter.ai/docs/guides/overview/multimodal/stt),
built with **Rust + Tauri 2**.

## How it stays fast

OpenRouter's STT endpoint (`POST /api/v1/audio/transcriptions`) is request/response,
so latency is won client-side:

1. **VAD-gated segmentation** — Silero VAD runs on 32 ms frames; a request fires
   ~350 ms after you stop talking, not after a fixed timer.
2. **16 kHz mono WAV** — audio is downsampled before upload (~3x smaller payload).
3. **Pre-warmed connection** — the TLS handshake to `openrouter.ai` is paid at app
   startup and the pool is kept alive, so the first utterance is as fast as the tenth.
4. **Non-blocking dispatch** — transcription requests run on a dedicated Tokio
   runtime; a slow response never stalls audio capture or VAD.
5. **Realtime-safe capture** — the cpal audio callback only downmixes and pushes to
   a lock-free channel.

The UI shows two numbers per utterance: **total** (end-of-speech → text on screen,
what you actually feel) and **api** (HTTP round-trip).

## Pipeline

```
cpal capture ──▶ resample to 16 kHz (rubato) ──▶ Silero VAD segmentation
                                                        │ utterance
                                                        ▼
UI (Tauri events) ◀── transcript + latency ◀── OpenRouter /audio/transcriptions
                                                (reqwest, warm pool, base64 WAV)
```

## Run it

Prereqs: Rust stable, Node 18+, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS.

```sh
export OPENROUTER_API_KEY=sk-or-...   # or paste it in the in-app settings
npm install
npm run dev
```

## Models

A curated fastest-first list is built in (`src-tauri/src/stt/models.rs`) — switch
between them live and compare measured latency:

| Model | Notes |
|---|---|
| `deepgram/nova-3` | Purpose-built low-latency ASR (default) |
| `nvidia/parakeet-tdt-0.6b-v3` | 600M params, extremely fast |
| `mistralai/voxtral-mini-transcribe` | Accuracy/speed balance |
| `qwen/qwen3-asr-flash-2026-02-10` | Ultra-cheap, multilingual |
| `openai/gpt-4o-mini-transcribe` | Cost-efficient OpenAI |
| `openai/whisper-large-v3-turbo` | 99+ languages |

Tip: set the 2-letter language hint (e.g. `en`) to skip auto-detect and shave latency.

## Layout

```
src-tauri/src/
  audio/capture.rs   # cpal input → lock-free channel (realtime-safe)
  audio/vad.rs       # Silero VAD, utterance segmentation + pre-roll/hangover
  audio/encode.rs    # rubato resample → 16 kHz mono, in-memory WAV
  stt/client.rs      # reqwest client, pre-warmed pool, latency measurement
  stt/models.rs      # curated model registry
  pipeline/          # orchestration + Tauri commands/events
  settings.rs        # API key / model / device / language persistence
ui/                  # thin vanilla JS frontend (transcript + latency readout)
```
