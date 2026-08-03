//! Custom vocabulary: seed terms + user terms + correction suggestions.
//!
//! Three layers work together:
//! 1. SEED_TERMS — a comprehensive base list of tech/AI terms that STT
//!    models chronically mangle ("pie pie" for PyPI). Always active.
//! 2. User terms (settings) — anything the user adds.
//! 3. Model-side biasing — the merged list is sent with every request:
//!    Deepgram takes per-term `keyterm` params; Whisper/GPT-4o take a
//!    `prompt` string that biases the decoder toward these spellings.
//!
//! The suggestion engine (`suggest_terms`) mines finished transcripts for
//! likely mis-hearings (spaced-out acronyms, phonetic tells like "pie pie")
//! and proposes vocabulary additions the user can accept with one click.

use std::collections::HashSet;

/// Terms STT models get wrong out of the box. Grouped for maintainability;
/// flattened at runtime. Keep each term in its canonical written form.
pub const SEED_TERMS: &[&str] = &[
    // Package registries / tooling
    "PyPI", "npm", "pnpm", "crates.io", "Homebrew", "Dockerfile", "Docker",
    "Kubernetes", "kubectl", "Terraform", "GitHub", "GitLab", "git", "CI/CD",
    "ESLint", "Vite", "webpack", "cargo", "rustc", "venv", "pip",
    // Languages / frameworks
    "TypeScript", "JavaScript", "Python", "Rust", "Golang", "Swift",
    "Kotlin", "C++", "C#", "SQL", "GraphQL", "React", "Next.js", "Vue",
    "Svelte", "Tauri", "Electron", "Node.js", "Deno", "Bun", "FastAPI",
    "Django", "Flask", "tokio", "async/await",
    // AI / ML
    "OpenAI", "OpenRouter", "OpenAPI", "Anthropic", "Claude", "GPT-4o",
    "ChatGPT", "Gemini", "LLaMA", "Mistral", "Whisper", "Deepgram",
    "Hugging Face", "LLM", "RAG", "fine-tuning", "embeddings", "tokenizer",
    "inference", "transformer", "PyTorch", "TensorFlow", "CUDA", "ONNX",
    "speech-to-text", "text-to-speech", "STT", "TTS", "VAD",
    // Protocols / formats / infra
    "API", "REST", "gRPC", "WebSocket", "HTTP", "HTTPS", "OAuth", "JWT",
    "JSON", "YAML", "TOML", "regex", "UUID", "SSH", "TLS", "DNS", "CDN",
    "SDK", "CLI", "UI", "UX", "IDE", "PR", "repo", "localhost", "backend",
    "frontend", "middleware", "microservices", "serverless", "PostgreSQL",
    "MySQL", "SQLite", "Redis", "MongoDB", "Supabase", "Firebase",
    // Platforms / products
    "macOS", "iOS", "Linux", "Ubuntu", "AWS", "GCP", "Azure", "Vercel",
    "Cloudflare", "Netlify", "VS Code", "Xcode", "Figma", "Slack",
    "Discord", "Notion", "Linear", "Jira", "Zoom",
];

/// Merge seed + user terms, deduped case-insensitively, user terms first
/// (they're most relevant to this user's speech).
pub fn merged_terms(user_terms: &[String]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for term in user_terms
        .iter()
        .map(|s| s.as_str())
        .chain(SEED_TERMS.iter().copied())
    {
        let term = term.trim();
        if term.is_empty() {
            continue;
        }
        if seen.insert(term.to_lowercase()) {
            out.push(term.to_string());
        }
    }
    out
}

/// Whisper/GPT-4o style biasing prompt: a natural-ish sentence embedding
/// the vocabulary. Kept under ~800 chars — long prompts dilute biasing.
pub fn biasing_prompt(user_terms: &[String]) -> String {
    let terms = merged_terms(user_terms);
    let mut list = String::new();
    for term in &terms {
        if list.len() + term.len() > 760 {
            break;
        }
        if !list.is_empty() {
            list.push_str(", ");
        }
        list.push_str(term);
    }
    format!("Technical vocabulary that may appear: {list}.")
}

/// Deepgram keyterm list (their limit is generous; cap for URL sanity).
pub fn keyterms(user_terms: &[String]) -> Vec<String> {
    merged_terms(user_terms).into_iter().take(100).collect()
}

// ── Suggestion engine ──────────────────────────────────────────────────

/// Phonetic tells: what a mangled term looks like in a transcript → what
/// the user probably said. Only fires on exact lowercase phrase match.
const PHONETIC_TELLS: &[(&str, &str)] = &[
    ("pie pie", "PyPI"),
    ("pie p i", "PyPI"),
    ("py pi", "PyPI"),
    ("open a i", "OpenAI"),
    ("open ai", "OpenAI"),
    ("open api", "OpenAPI"),
    ("open router", "OpenRouter"),
    ("get hub", "GitHub"),
    ("git hub", "GitHub"),
    ("no js", "Node.js"),
    ("node js", "Node.js"),
    ("next js", "Next.js"),
    ("view js", "Vue"),
    ("react js", "React"),
    ("type script", "TypeScript"),
    ("java script", "JavaScript"),
    ("coober netties", "Kubernetes"),
    ("kuber nettis", "Kubernetes"),
    ("cube control", "kubectl"),
    ("post gress", "PostgreSQL"),
    ("post grass", "PostgreSQL"),
    ("my sequel", "MySQL"),
    ("sequel light", "SQLite"),
    ("tory", "Tauri"),
    ("torii", "Tauri"),
    ("rust sea", "rustc"),
    ("web pack", "webpack"),
    ("versal", "Vercel"),
    ("dark mode", "dark mode"), // sentinel: common but correct — never suggest
];

/// Scan a finished transcript for likely mis-hearings and return suggested
/// vocabulary additions as (heard, suggestion) pairs. Deduped against terms
/// already in the vocabulary.
pub fn suggest_terms(transcript: &str, user_terms: &[String]) -> Vec<(String, String)> {
    let lower = transcript.to_lowercase();
    let known: HashSet<String> = merged_terms(user_terms)
        .iter()
        .map(|t| t.to_lowercase())
        .collect();

    let mut suggestions: Vec<(String, String)> = Vec::new();
    let mut suggested: HashSet<String> = HashSet::new();

    // 1. Phonetic tells.
    for (heard, term) in PHONETIC_TELLS {
        if heard == term {
            continue; // sentinel rows
        }
        if lower.contains(heard) && !suggested.contains(*term) {
            // Suggest even if the term is already in the vocabulary: a tell
            // in the transcript means the model misheard it anyway, and the
            // suggestion doubles as visibility that the miss happened.
            suggestions.push((heard.to_string(), term.to_string()));
            suggested.insert(term.to_string());
        }
    }

    // 2. Spelled-out acronyms: "a p i" → "API" (3+ single letters in a row).
    let words: Vec<&str> = lower.split_whitespace().collect();
    let mut run: Vec<&str> = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let is_single = w.len() == 1 && w.chars().all(|c| c.is_ascii_alphabetic());
        if is_single {
            run.push(w);
        }
        if !is_single || i == words.len() - 1 {
            if run.len() >= 3 {
                let acronym: String = run.iter().flat_map(|s| s.chars()).collect::<String>().to_uppercase();
                if !known.contains(&acronym.to_lowercase()) && !suggested.contains(&acronym) {
                    suggestions.push((run.join(" "), acronym.clone()));
                    suggested.insert(acronym);
                }
            }
            run.clear();
        }
    }

    suggestions
}
