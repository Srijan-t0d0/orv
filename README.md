# OpenRouter Voice

**System-wide push-to-talk dictation, powered by the [OpenRouter API](https://openrouter.ai).**
Hold a hotkey anywhere on your desktop, speak, release — your words appear at your
cursor. Built with Rust + Tauri 2 as a reference app for OpenRouter's
[speech-to-text endpoint](https://openrouter.ai/docs/guides/overview/multimodal/stt).

## Where to start reading

| You want… | Read |
|---|---|
| The API call, minimally | [`src-tauri/examples/minimal.rs`](src-tauri/examples/minimal.rs) — mic → WAV → transcript in ~100 lines |
| The full integration, explained | [`docs/INTEGRATION.md`](docs/INTEGRATION.md) — transports, biasing, OAuth, model choice |
| The integration code itself | [`src-tauri/src/stt/`](src-tauri/src/stt/) — client, model registry, vocabulary |
| The desktop-app machinery | [`src-tauri/src/platform/`](src-tauri/src/platform/) — hotkeys, tray, overlay (no API code) |

```sh
# smallest possible taste — record 5s, print the transcript
export OPENROUTER_API_KEY=sk-or-...
cd src-tauri && cargo run --example minimal
```

## Features

- **Push-to-talk anywhere** — device-wide hotkey (keyboard or mouse side button);
  hold to dictate, release to type the transcript into whatever app has focus,
  optional second hotkey that presses Enter to send
- **Rolling live preview** — while you hold, the whole utterance is re-transcribed
  every ~1.2s into a floating overlay pill, so the text self-corrects as context grows
- **16 models, benchmarked** — every dedicated STT model on OpenRouter plus
  streaming-capable audio chat models, with real WER/RTFx/price data and an in-app
  benchmark that ranks them by measured latency on *your* connection and voice
- **Accuracy stack** — vocabulary biasing (Deepgram keyterms / Whisper prompts),
  mis-hearing suggestions, filler-word removal, and an opt-in LLM correction pass
- **Background service** — menu-bar tray, close-to-tray, launch at login,
  starts hidden; the main window is optional
- **OpenRouter OAuth** — "Log in with OpenRouter" (PKCE, loopback callback), or
  paste an API key; credentials never reach the webview

## Architecture

```
src-tauri/src/
├── stt/             ★ the OpenRouter integration (start here)
│   ├── client.rs       one-shot + SSE streaming transports, LLM cleanup, warm pool
│   ├── models.rs       registry: WER/RTFx/price per model, transport routing
│   └── vocab.rs        per-provider vocabulary biasing + suggestion mining
├── audio/             capture (cpal) → resample (rubato) → VAD gate (Silero)
├── pipeline/          session orchestration
│   ├── mod.rs            hold-buffer, rolling re-transcription, dispatch
│   ├── text.rs           filler cleanup, continuity, type-into-app
│   └── commands.rs       Tauri command surface for the UI
├── platform/          OS machinery — zero API code (skippable for API readers)
│   ├── hotkey.rs         device-wide push-to-talk (poll + raw HID)
│   ├── hid.rs            macOS IOHID mouse-button listener
│   ├── tray.rs           menu-bar presence, background lifecycle
│   ├── overlay.rs        floating dictation pill window
│   └── traffic.rs        macOS traffic-light positioning
├── auth.rs            OpenRouter OAuth PKCE (reusable in any native app)
├── settings.rs / history.rs / benchmark.rs   persistence
└── lib.rs             wiring
ui/                    vanilla JS/CSS, no framework, no build step
```

Audio flows one direction: `audio → pipeline → stt → OpenRouter`. The only
network calls in the codebase live in `stt/client.rs` and `auth.rs`.

## Run the app

Prereqs: Rust stable, Node 18+, [Tauri prerequisites](https://tauri.app/start/prerequisites/).

```sh
npm install
npm run dev
```

First run: grant **Microphone** access, plus **Input Monitoring** and
**Accessibility** (System Settings → Privacy & Security) for global hotkeys and
type-into-app. The app starts in the **menu bar** — open the window from the
tray icon, log in (or `export OPENROUTER_API_KEY`), bind a hotkey, dictate.

## Platform support

| | Status |
|---|---|
| macOS | ✅ fully supported (primary target) |
| Windows / Linux | ⚠️ core pipeline is portable; `platform/hid.rs` (side buttons), `platform/traffic.rs`, and overlay behavior need porting — see module docs |

## Privacy

The global input listener is a **matcher, not a recorder**: key/button state is
compared against your two bindings and discarded — no keystroke history exists,
in memory or on disk, and nothing but **microphone audio** (plus model id and
optional vocabulary hints) is ever sent to OpenRouter. Logging deliberately
excludes key identities and transcript contents. Local files: `settings.json`
(preferences), `history.jsonl` (your transcripts, local only), `benchmark.wav`
(your recorded reference clip).
