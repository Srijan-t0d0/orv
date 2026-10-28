# OpenRouter Voice

**System-wide push-to-talk dictation, powered by the [OpenRouter API](https://openrouter.ai).**
Hold a hotkey anywhere on your desktop, speak, release — your words appear at your
cursor. Built with Rust + Tauri 2 as a reference app for OpenRouter's
[speech-to-text endpoint](https://openrouter.ai/docs/guides/overview/multimodal/stt).

## This fork

Personal macOS fork of [OpenRouterLabs/orv](https://github.com/OpenRouterLabs/orv).
The upstream is a reference app; this fork adds fixes tested in everyday dictation:

- Remember the input application's PID before recording and restore its focus before insertion.
- Paste with Cmd+V instead of Unicode keyboard-event text injection. Back up all clipboard
  item types and restore them unless the clipboard changed during the paste.
- Check the running process's Accessibility access and report insertion failures in logs.
- Consume bound F13–F20 hotkeys on macOS so mouse-emitted function keys do not type
  private-use glyphs into the focused app. Capture also consumes those keys. Ordinary
  keys and mouse buttons retain polling behavior; they are not suppressed.
- Include a native macOS icon so `npm run build -- --bundles app` works without local overrides.

Settings live at `~/Library/Application Support/openroutervoice/settings.json` on macOS.
Quit the app before editing and restart afterward. That file can contain your API key;
keep it, recordings, history, and logs out of Git. Never commit credentials.

Local ad-hoc builds change their signing identity when rebuilt. macOS may require
re-granting Accessibility and Input Monitoring, followed by fully quitting and reopening
this app. Closing its window only hides it. The hotkey listener does not currently recover
from a missing startup permission without a restart.

Verification: `cd src-tauri && cargo test --lib`, a macOS app-bundle build, and a manual
focused-input dictation test. The clipboard restore uses a bounded 350 ms grace period;
slow clipboard consumers may need a longer interval. Other platforms retain upstream's
text-injection path and have not been tested by this fork.

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
- **Live model catalog, benchmarked** — every dedicated STT model OpenRouter lists
  (fetched at startup) plus streaming-capable audio chat models, with real
  WER/RTFx/price data for the curated ones and an in-app
  benchmark that ranks them by measured latency on *your* connection and voice
- **Accuracy stack** — mis-hearing suggestions, filler-word removal, and an opt-in
  LLM cleanup pass (configurable model) that knows your vocabulary and accent
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
│   └── vocab.rs        vocabulary/accent prompt hints + suggestion mining
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

## License & attribution

MIT — see [LICENSE](LICENSE). Voice activity detection uses the
[Silero VAD](https://github.com/snakers4/silero-vad) v5 model (MIT), bundled
via the [`voice_activity_detector`](https://github.com/nkeenan38/voice_activity_detector)
crate (MIT); release builds redistribute the model weights.

## Privacy

The global input listener is a **matcher, not a recorder**: key/button state is
compared against your two bindings and discarded — no keystroke history exists,
in memory or on disk, and nothing but **microphone audio** (plus model id and
optional vocabulary hints) is ever sent to OpenRouter. Logging deliberately
excludes key identities and transcript contents. Local files: `settings.json`
(preferences), `history.jsonl` (your transcripts, local only), `benchmark.wav`
(your recorded reference clip).
