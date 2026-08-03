//! Registry of every dedicated STT model on OpenRouter (all 13 as of
//! 2026-08), plus audio-capable chat models that support streaming.
//!
//! Stats are grounded in real data:
//! - `wer`: word error rate from the Open ASR Leaderboard (short-form
//!   English) and Artificial Analysis STT benchmarks. Lower is better.
//!   "—" where no public benchmark exists.
//! - `rtfx`: real-time factor (seconds of audio processed per second) from
//!   the Open ASR Leaderboard / provider benchmarks, where published.
//! - `price`: normalized to $ per hour of audio. NOTE: OpenRouter lists STT
//!   prices as $/M "audio tokens", but token units differ per provider
//!   (Deepgram ≈ minutes, Groq ≈ hours, Qwen ≈ seconds), so raw $/M numbers
//!   are not comparable — the per-hour figure is.
//! - `speed`/`quality` (1–5) drive the picker's meter bars and are DERIVED
//!   from rtfx/wer: quality 5 = WER ≤4.5%, 4 = ≤6%, 3 = ≤8% or unbenchmarked
//!   mid-tier; speed 5 = streaming-first/RTFx >300, down to 2 for big LLM
//!   decoders. The UI additionally shows live measured latency, which is the
//!   ground truth for speed on your network.

use serde::Serialize;

/// How audio reaches the model.
#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    /// POST /audio/transcriptions — one shot, full text back.
    Transcription,
    /// POST /chat/completions with input_audio + stream:true — the
    /// transcript streams in token-by-token (partial results in the UI).
    ChatStream,
}

#[derive(Clone, Serialize)]
pub struct SttModel {
    pub id: &'static str,
    pub name: &'static str,
    pub transport: Transport,
    /// Provider serving this model on OpenRouter.
    pub provider: &'static str,
    /// 1–5 meter, derived from rtfx/architecture (see module docs).
    pub speed: u8,
    /// 1–5 meter, derived from wer (see module docs).
    pub quality: u8,
    /// Published word error rate, or "—".
    pub wer: &'static str,
    /// Published real-time factor, or "—".
    pub rtfx: &'static str,
    /// Normalized cost per hour of audio.
    pub price: &'static str,
    pub languages: &'static str,
    pub note: &'static str,
    /// Curated "suggested" flag — pinned group at the top of the picker.
    pub suggested: bool,
}

pub const MODELS: &[SttModel] = &[
    // ── Suggested: the realtime-dictation sweet spot ──
    SttModel {
        id: "deepgram/nova-3",
        name: "Deepgram Nova-3",
        transport: Transport::Transcription,
        provider: "Deepgram",
        speed: 5,
        quality: 4,
        wer: "5.2%",
        rtfx: "~330",
        price: "$0.26/hr",
        languages: "Multilingual",
        note: "Streaming-first ASR built for realtime; the latency benchmark to beat.",
        suggested: true,
    },
    SttModel {
        id: "nvidia/parakeet-tdt-0.6b-v3",
        name: "NVIDIA Parakeet TDT 0.6B v3",
        transport: Transport::Transcription,
        provider: "Together",
        speed: 5,
        quality: 3,
        wer: "6.3%",
        rtfx: "3330",
        price: "$0.09/hr",
        languages: "25 languages",
        note: "Fastest model on the Open ASR leaderboard by an order of magnitude.",
        suggested: true,
    },
    SttModel {
        id: "openai/whisper-large-v3-turbo",
        name: "Whisper Large V3 Turbo",
        transport: Transport::Transcription,
        provider: "Groq",
        speed: 5,
        quality: 3,
        wer: "7.8%",
        rtfx: "~216",
        price: "$0.04/hr",
        languages: "99+ languages",
        note: "Cheapest per hour on OpenRouter; Groq LPU serving keeps it fast.",
        suggested: true,
    },
    // ── Accuracy tier ──
    SttModel {
        id: "openai/gpt-4o-transcribe",
        name: "GPT-4o Transcribe",
        transport: Transport::Transcription,
        provider: "OpenAI",
        speed: 2,
        quality: 5,
        wer: "4.0%",
        rtfx: "—",
        price: "~$0.36/hr",
        languages: "Multilingual",
        note: "Best published WER of any API model; LLM decoder adds latency.",
        suggested: false,
    },
    SttModel {
        id: "openai/whisper-large-v3",
        name: "Whisper Large V3",
        transport: Transport::Transcription,
        provider: "Together / Groq",
        speed: 3,
        quality: 5,
        wer: "4.1%",
        rtfx: "146",
        price: "$0.09/hr",
        languages: "99+ languages",
        note: "Near-GPT-4o accuracy at a tenth of the effective price.",
        suggested: false,
    },
    SttModel {
        id: "openai/gpt-4o-mini-transcribe",
        name: "GPT-4o Mini Transcribe",
        transport: Transport::Transcription,
        provider: "OpenAI",
        speed: 3,
        quality: 4,
        wer: "~5%",
        rtfx: "—",
        price: "~$0.18/hr",
        languages: "Multilingual",
        note: "Robust to accents and jargon; solid accuracy-per-dollar.",
        suggested: false,
    },
    SttModel {
        id: "microsoft/mai-transcribe-1.5",
        name: "Microsoft MAI-Transcribe 1.5",
        transport: Transport::Transcription,
        provider: "Azure",
        speed: 3,
        quality: 4,
        wer: "—",
        rtfx: "—",
        price: "$0.36/hr",
        languages: "43 languages",
        note: "Tuned for captions, calls, and subtitling workloads.",
        suggested: false,
    },
    SttModel {
        id: "google/chirp-3",
        name: "Google Chirp 3",
        transport: Transport::Transcription,
        provider: "Google Vertex",
        speed: 3,
        quality: 4,
        wer: "—",
        rtfx: "—",
        price: "$0.96/hr",
        languages: "24 GA + 77 preview",
        note: "USM-family ASR; strong multilingual, priciest per hour here.",
        suggested: false,
    },
    SttModel {
        id: "mistralai/voxtral-mini-transcribe",
        name: "Voxtral Mini Transcribe",
        transport: Transport::Transcription,
        provider: "Mistral",
        speed: 4,
        quality: 4,
        wer: "~6%",
        rtfx: "—",
        price: "$0.18/hr",
        languages: "Multilingual",
        note: "Voxtral-family accuracy through the fast one-shot endpoint.",
        suggested: false,
    },
    // ── Budget / specialty tier ──
    SttModel {
        id: "qwen/qwen3-asr-flash-2026-02-10",
        name: "Qwen3 ASR Flash",
        transport: Transport::Transcription,
        provider: "Alibaba",
        speed: 4,
        quality: 3,
        wer: "—",
        rtfx: "—",
        price: "$0.13/hr",
        languages: "11 languages",
        note: "Strong zh/en, handles noisy audio and song vocals.",
        suggested: false,
    },
    SttModel {
        id: "fish-audio/transcribe-1",
        name: "Fish Audio Transcribe 1",
        transport: Transport::Transcription,
        provider: "Fish Audio",
        speed: 3,
        quality: 3,
        wer: "—",
        rtfx: "—",
        price: "$0.36/hr",
        languages: "Auto-detect",
        note: "Word-level timestamps when alignment details are requested.",
        suggested: false,
    },
    SttModel {
        id: "x-ai/grok-stt-1.0",
        name: "xAI Grok STT 1.0",
        transport: Transport::Transcription,
        provider: "xAI",
        speed: 3,
        quality: 3,
        wer: "—",
        rtfx: "—",
        price: "$0.10/hr",
        languages: "Multilingual",
        note: "Word timestamps, diarization, and multichannel audio.",
        suggested: false,
    },
    SttModel {
        id: "openai/whisper-1",
        name: "Whisper 1 (v2)",
        transport: Transport::Transcription,
        provider: "OpenAI",
        speed: 2,
        quality: 3,
        wer: "~8%",
        rtfx: "—",
        price: "$0.36/hr",
        languages: "50+ languages",
        note: "The classic whisper-1 API; superseded by V3 on every axis.",
        suggested: false,
    },
    // ── Streaming (chat-completions transport) ──
    // /audio/transcriptions is one-shot; these accept audio via
    // /chat/completions with stream:true so partial text renders live.
    SttModel {
        id: "mistralai/voxtral-small-24b-2507",
        name: "Voxtral Small 24B (streaming)",
        transport: Transport::ChatStream,
        provider: "Mistral",
        speed: 3,
        quality: 4,
        wer: "6.6%",
        rtfx: "54",
        price: "$0.36/hr",
        languages: "Multilingual",
        note: "Best multilingual WER in its class; transcript streams token-by-token.",
        suggested: false,
    },
    SttModel {
        id: "google/gemini-2.5-flash-lite",
        name: "Gemini 2.5 Flash Lite (streaming)",
        transport: Transport::ChatStream,
        provider: "Google",
        speed: 4,
        quality: 3,
        wer: "—",
        rtfx: "—",
        price: "~$0.11/hr",
        languages: "Multilingual",
        note: "Cheap streaming transcription via Gemini audio input.",
        suggested: false,
    },
];

pub const DEFAULT_MODEL: &str = "deepgram/nova-3";
