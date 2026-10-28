# Integrating OpenRouter speech-to-text

This is the guide to the ~800 lines of this repo that talk to OpenRouter —
what each call does, why it's shaped that way, and what to copy into your own
app. Everything referenced lives in [`src-tauri/src/stt/`](../src-tauri/src/stt/)
and [`src-tauri/src/auth.rs`](../src-tauri/src/auth.rs).

The absolute minimum is [`examples/minimal.rs`](../src-tauri/examples/minimal.rs):
record → WAV → one POST → transcript. Everything below is what production adds
on top.

## 1. The transcription call

`POST https://openrouter.ai/api/v1/audio/transcriptions` with base64 WAV:

```jsonc
{
  "model": "deepgram/nova-3",
  "input_audio": { "data": "<base64 wav>", "format": "wav" },
  "language": "en"           // optional ISO-639-1 hint — skips auto-detect
}
```

Response: `{ "text": "...", "usage": { … } }`. See `SttClient::transcribe`
in [`client.rs`](../src-tauri/src/stt/client.rs).

Three production details that matter more than they look:

- **Pre-warm the connection** (`SttClient::prewarm`). A cold TLS handshake to
  openrouter.ai can cost more than the transcription itself. Fire a throwaway
  `GET /models?limit=1` at startup and keep the pool alive
  (`pool_idle_timeout` 300s, `tcp_keepalive` 30s) — the first real request
  then behaves like the tenth.
- **Downsample before upload.** Models are trained on 16 kHz mono; sending
  your device's 48 kHz stereo triples upload time for zero accuracy gain.
- **Attribution headers** (`HTTP-Referer`, `X-Title`) identify your app on
  OpenRouter rankings.

## 2. Two transports: one-shot vs streaming

`/audio/transcriptions` is request/response — no partial results. For live
token-by-token text, audio-capable *chat* models accept audio via
`/chat/completions` with `stream: true`:

```jsonc
{
  "model": "mistralai/voxtral-small-24b-2507",
  "stream": true,
  "messages": [{
    "role": "user",
    "content": [
      { "type": "text", "text": "Transcribe this audio verbatim. Output only the transcript." },
      { "type": "input_audio", "input_audio": { "data": "<base64 wav>", "format": "wav" } }
    ]
  }]
}
```

Parse the SSE stream (`data: {json}` lines, `choices[0].delta.content`) —
see `SttClient::transcribe_streaming`. The registry
([`models.rs`](../src-tauri/src/stt/models.rs)) tags every model with its
`Transport` so the pipeline routes automatically.

**When to use which:** for short utterances, dedicated STT models return the
whole transcript faster than a chat model streams its first tokens. Streaming
wins on long-form audio where first-token latency beats total latency.

## 3. Model choice (with real numbers)

The registry carries published WER (Open ASR Leaderboard / Artificial
Analysis), RTFx, and price normalized to $/hour of audio — OpenRouter lists
STT as $/M "audio tokens" but token units differ per provider, so raw prices
aren't comparable; per-hour is.

The short version of the trade-off space:

| Pick | If you want |
|---|---|
| `deepgram/nova-3` | realtime dictation default (5.2% WER, streaming-first serving) |
| `nvidia/parakeet-tdt-0.6b-v3` | raw speed (3330x realtime) at $0.09/hr |
| `openai/whisper-large-v3` | near-best accuracy (4.1% WER) at $0.09/hr |
| `openai/gpt-4o-transcribe` | best published WER (4.0%), latency be damned |
| `openai/whisper-large-v3-turbo` | cheapest ($0.04/hr via Groq) |

But the honest answer is **benchmark on your own audio** — this app ships an
in-app benchmark (record a clip → run it through every model → rank by
measured latency) precisely because network path, clip length, and voice
change the ranking. On our test machine, `x-ai/grok-stt-1.0` beat everything.

## 4. Vocabulary and accent hints (the accuracy lever)

STT models mangle technical terms ("PyPI" → "pie pie") and accented speech.
Decoder-side biasing does not currently reach models through OpenRouter's
`/audio/transcriptions` (tested 2026-10-28): every STT model tried rejected
the documented `keyterms` field with a 400, and `prompt` or provider options
(`provider.options.groq.prompt`, Deepgram `keyterm`) were accepted but did not
change the output. So hints go only into prompts this app writes
([`vocab.rs`](../src-tauri/src/stt/vocab.rs) `hints`): the speaker's accent
plus the vocabulary, kept under ~800 chars.

In increasing power:
1. deterministic post-fixes (this repo strips filler words in
   [`pipeline/text.rs`](../src-tauri/src/pipeline/text.rs))
2. chat-transport models (audio via `/chat/completions`) get the hints in
   their transcription instruction, which they do follow
3. an LLM cleanup pass — `SttClient::smart_correct` sends the transcript to
   a configurable model (default `google/gemini-3.1-flash-lite`) with a
   cleanup prompt adapted from OpenWhispr's, the hints, a 6s timeout, and a
   length sanity check so failure falls back to the raw transcript. The
   only fix for homophones (eye/I) and accent mis-hearings that needs
   sentence context.

## 5. OAuth PKCE ("Log in with OpenRouter")

Native apps shouldn't make users copy API keys. The PKCE flow
([`auth.rs`](../src-tauri/src/auth.rs)) is ~150 lines and fully reusable:

1. Bind a loopback listener on `127.0.0.1:<random>` (bind *first*, so you
   know the callback port).
2. Open the browser at
   `https://openrouter.ai/auth?callback_url=<loopback>&code_challenge=<S256(verifier)>&code_challenge_method=S256`.
3. The redirect lands on your listener with `?code=…`.
4. Exchange it: `POST /api/v1/auth/keys` with `{ code, code_verifier,
   code_challenge_method }` → `{ "key": "sk-or-…" }` — a user-controlled,
   app-scoped key.

Keep the verifier and the received key out of any webview/renderer process;
this app holds both exclusively in Rust.

## 6. Latency checklist

What made this app feel instant, in order of impact:

1. Warm connection pool (§1) — saves a full handshake on the first utterance
2. Push-to-talk release as the end-of-speech signal — no VAD hangover wait
3. 16 kHz mono WAV — ~3x smaller upload
4. `language` hint — skips auto-detect
5. Fast model for dictation (Nova-3 class), accuracy models reserved for
   long-form
6. Rolling preview (re-transcribe the growing buffer every ~1.2s) so the user
   reads text *while* speaking — perceived latency beats actual latency
