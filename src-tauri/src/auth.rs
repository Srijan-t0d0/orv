//! "Log in with OpenRouter" — OAuth PKCE flow for native apps.
//!
//! 1. Spin up a one-shot loopback HTTP listener on 127.0.0.1:<random port>.
//! 2. Open the system browser at openrouter.ai/auth with an S256 code
//!    challenge and the loopback URL as callback.
//! 3. The browser redirects back with ?code=...; exchange it at
//!    POST /api/v1/auth/keys for a user-controlled API key.
//! 4. Store the key in settings and notify the UI.
//!
//! The code verifier never leaves the process and the received key never
//! reaches the webview.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;

use base64::Engine;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

use crate::AppState;

const AUTH_URL: &str = "https://openrouter.ai/auth";
const EXCHANGE_URL: &str = "https://openrouter.ai/api/v1/auth/keys";

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[derive(Deserialize)]
struct ExchangeResponse {
    key: String,
}

fn emit_auth(app: &AppHandle, state: &str, detail: Option<String>) {
    #[derive(serde::Serialize, Clone)]
    struct AuthEvent {
        state: String,
        detail: Option<String>,
    }
    let _ = app.emit(
        "auth-status",
        AuthEvent {
            state: state.into(),
            detail,
        },
    );
}

/// Kick off the login flow; progress is reported via `auth-status` events
/// ("pending" → "success" | "error").
pub fn begin_login(app: AppHandle) -> Result<(), String> {
    // Bind first so we know the callback port before opening the browser.
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|e| format!("loopback bind failed: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .port();

    // PKCE verifier + S256 challenge.
    let mut verifier_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut verifier_bytes);
    let verifier = b64url(&verifier_bytes);
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));

    let callback = format!("http://127.0.0.1:{port}/callback");
    let url = format!(
        "{AUTH_URL}?callback_url={}&code_challenge={challenge}&code_challenge_method=S256",
        urlencode(&callback)
    );

    open::that_detached(&url).map_err(|e| format!("could not open browser: {e}"))?;
    emit_auth(&app, "pending", Some("waiting for browser authorization…".into()));

    std::thread::Builder::new()
        .name("openrouter-oauth".into())
        .spawn(move || {
            // Give the user 5 minutes to finish in the browser.
            listener
                .set_nonblocking(false)
                .ok();
            let code = wait_for_code(&listener, Duration::from_secs(300));

            match code {
                Ok(code) => match exchange(&code, &verifier) {
                    Ok(key) => {
                        let state = app.state::<AppState>();
                        let mut settings = state.settings.lock().unwrap();
                        settings.api_key = key;
                        settings.auth_method = "oauth".into();
                        settings.save();
                        drop(settings);
                        emit_auth(&app, "success", None);
                    }
                    Err(e) => emit_auth(&app, "error", Some(e)),
                },
                Err(e) => emit_auth(&app, "error", Some(e)),
            }
        })
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Accept loopback connections until one carries ?code=..., then respond
/// with a small "you can close this tab" page.
fn wait_for_code(listener: &TcpListener, timeout: Duration) -> Result<String, String> {
    let deadline = std::time::Instant::now() + timeout;
    for stream in listener.incoming() {
        if std::time::Instant::now() > deadline {
            return Err("login timed out".into());
        }
        let Ok(mut stream) = stream else { continue };
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();

        let mut request_line = String::new();
        if BufReader::new(&stream).read_line(&mut request_line).is_err() {
            continue;
        }
        // e.g. "GET /callback?code=abc HTTP/1.1"
        let path = request_line.split_whitespace().nth(1).unwrap_or("");
        let code = path
            .split_once("code=")
            .map(|(_, rest)| rest.split('&').next().unwrap_or("").to_string());

        let (status, body) = if code.as_deref().is_some_and(|c| !c.is_empty()) {
            (
                "200 OK",
                "<html><body style=\"font-family:sans-serif;background:#0e1116;color:#e6e9ef;\
                 display:flex;align-items:center;justify-content:center;height:100vh\">\
                 <div><h2>✓ Logged in to OpenRouter Voice</h2>\
                 <p>You can close this tab and return to the app.</p></div></body></html>",
            )
        } else {
            ("400 Bad Request", "<html><body>Missing code.</body></html>")
        };
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.flush();

        if let Some(code) = code {
            if !code.is_empty() {
                return Ok(code);
            }
        }
    }
    Err("listener closed".into())
}

/// POST the code + verifier to OpenRouter, get a user-controlled API key.
fn exchange(code: &str, verifier: &str) -> Result<String, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        let resp = reqwest::Client::new()
            .post(EXCHANGE_URL)
            .json(&serde_json::json!({
                "code": code,
                "code_verifier": verifier,
                "code_challenge_method": "S256",
            }))
            .send()
            .await
            .map_err(|e| format!("exchange request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("exchange failed ({status}): {text}"));
        }
        let parsed: ExchangeResponse = resp
            .json()
            .await
            .map_err(|e| format!("bad exchange response: {e}"))?;
        Ok(parsed.key)
    })
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
