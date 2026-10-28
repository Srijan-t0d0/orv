const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const appWindow = window.__TAURI__.window.getCurrentWindow();

// Window dragging: don't rely solely on data-tauri-drag-region (its
// injected handler only fires when the mousedown target is the exact
// element carrying the attribute). Drive startDragging() ourselves from
// anywhere in the header that isn't an interactive control.
const headerEl = document.querySelector("header");
headerEl.addEventListener("mousedown", (e) => {
  if (e.button !== 0) return;
  if (e.target.closest("button, select, input, a")) return;
  if (e.detail === 2) {
    appWindow.toggleMaximize();
  } else {
    appWindow.startDragging();
  }
});

const $ = (id) => document.getElementById(id);
const statusEl = $("status");
const statusText = $("statusText");
const toggleBtn = $("toggle");
const heroHint = $("heroHint");
const hotkeyHint = $("hotkeyHint");
const hotkeyKbd = $("hotkeyKbd");
const modelSelect = $("modelSelect");
const modelNameEl = $("modelName");
const modelLatEl = $("modelLat");
const modelMenu = $("modelMenu");
const deviceSel = $("device");
const languageInput = $("language");
const transcriptEl = $("transcript");
const viewMain = $("viewMain");
const viewSettings = $("viewSettings");
const gearIcon = $("gearIcon");
const backIcon = $("backIcon");
const credStateEl = $("credState");
const apiKeyInput = $("apiKey");
const loginBtn = $("loginBtn");
const logoutBtn = $("logoutBtn");
const hotkeyBtn = $("hotkeyBtn");
const hotkeyLabel = $("hotkeyLabel");
const hotkeyClear = $("hotkeyClear");
const enterHotkeyBtn = $("enterHotkeyBtn");
const enterHotkeyLabel = $("enterHotkeyLabel");
const enterHotkeyClear = $("enterHotkeyClear");
const typeIntoApp = $("typeIntoApp");
const launchAtLogin = $("launchAtLogin");
const startMinimized = $("startMinimized");
const cleanFillers = $("cleanFillers");
const smartCorrections = $("smartCorrections");
const cleanupSelect = $("cleanupSelect");
const cleanupMenu = $("cleanupMenu");
const cleanupSearch = $("cleanupSearch");
const DEFAULT_CLEANUP_MODEL = "google/gemini-3.1-flash-lite";
let cleanupModels = [];
let cleanupModel = DEFAULT_CLEANUP_MODEL;
const accentInput = $("accent");
const vocabInput = $("vocabInput");
const vocabSuggestions = $("vocabSuggestions");
const sugChips = $("sugChips");
const seedCount = $("seedCount");

let listening = false;
let models = [];
let currentModel = "";
let hotkeyMode = "push_to_talk";
const modeSwitch = $("modeSwitch");
// Best measured end-to-end latency per model id, this session.
const liveLatency = new Map();

/* ── status / listening state ─────────────────────────── */

function setStatus(state, detail) {
  statusText.textContent = detail ? `${state} — ${detail}` : state;
  statusEl.className = `status ${state}`;
}

function setListeningUi(on) {
  listening = on;
  toggleBtn.classList.toggle("live", on);
  heroHint.textContent = on ? "Listening — click to stop" : "Click to start listening";
}

/* ── transcript ───────────────────────────────────────── */

function clearPlaceholder() {
  transcriptEl.querySelector(".placeholder")?.remove();
}

// Streaming models emit stt-partial before the final stt-transcript;
// both carry the same utterance id so the partial block upgrades in place.
function utteranceDiv(id) {
  let div = transcriptEl.querySelector(`[data-utt="${id}"]`);
  if (!div) {
    clearPlaceholder();
    div = document.createElement("div");
    div.className = "utterance";
    div.dataset.utt = id;
    div.innerHTML = `<div class="text"></div><div class="meta"></div>`;
    transcriptEl.appendChild(div);
  }
  return div;
}

function showPartial(ev) {
  const div = utteranceDiv(ev.id);
  div.classList.add("partial");
  div.querySelector(".text").textContent = ev.text;
  div.querySelector(".meta").innerHTML = `<span class="streaming">streaming…</span>`;
  transcriptEl.scrollTop = transcriptEl.scrollHeight;
}

// Seconds read faster than thousands of milliseconds: "2.4s", "0.45s".
const secs = (ms) => `${(ms / 1000).toFixed(ms < 1000 ? 2 : 1)}s`;
const attr = (s) => String(s).replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");

// The meta line under a transcript: the wait you felt, then where it went.
// Each label's tooltip says what the number measures.
function timingHtml(e, when) {
  const slow = e.total_ms > 1200 ? " slow" : "";
  const sttTip = `Speech-to-text (${e.model}) turning your ${e.audio_secs.toFixed(1)}s recording into text` +
    (e.stt_ttfb_ms != null ? `; the server started replying after ${secs(e.stt_ttfb_ms)}` : "");
  let cleanup = "";
  if (e.llm) {
    const tip = e.llm.error
      ? `Smart cleanup failed, so the raw transcript was used: ${e.llm.error}`
      : `Smart cleanup (${e.llm.model}) fixing the transcript` +
        (e.llm.ttfb_ms != null ? `; its first words arrived after ${secs(e.llm.ttfb_ms)}` : "");
    cleanup = `<span class="stage${e.llm.error ? " failed" : ""}" title="${attr(tip)}">` +
      `Cleanup <b>${secs(e.llm.ms)}</b>${e.llm.error ? " · failed" : ""}</span>`;
  }
  return `
      ${when ? `<span>${when}</span>` : ""}
      <span class="latency${slow}" title="From releasing the hotkey until the text was ready to insert">Total <b>${secs(e.total_ms)}</b></span>
      <span class="stage" title="${attr(sttTip)}">Speech-to-text <b>${secs(e.api_ms)}</b></span>
      ${cleanup}
      <span title="Length of your recording">${e.audio_secs.toFixed(1)}s of audio</span>
      <span class="model">${attr(e.model)}</span>`;
}

function addUtterance(ev) {
  const div = utteranceDiv(ev.id);
  div.classList.remove("partial");
  div.querySelector(".text").textContent = ev.text;
  div.querySelector(".meta").innerHTML = timingHtml(ev);
  transcriptEl.scrollTop = transcriptEl.scrollHeight;

  const best = liveLatency.get(ev.model);
  if (!best || ev.total_ms < best) liveLatency.set(ev.model, ev.total_ms);
  renderModelBar();
}

function addError(msg) {
  clearPlaceholder();
  const div = document.createElement("div");
  div.className = "error-line";
  div.textContent = msg;
  transcriptEl.appendChild(div);
  transcriptEl.scrollTop = transcriptEl.scrollHeight;
}

/* ── provider logos ───────────────────────────────────── */

// Bundled in icons/providers/, keyed by model-id author ("google/…").
const PROVIDER_ICONS = {
  amazon: "png", anthropic: "svg", assemblyai: "png", "bytedance-seed": "png",
  cohere: "png", deepgram: "svg", deepseek: "png", elevenlabs: "png",
  "fish-audio": "png", google: "svg", inception: "svg", liquid: "png",
  meta: "png", "meta-llama": "png", microsoft: "svg", minimax: "png",
  mistralai: "png", moonshotai: "png", nousresearch: "png", nvidia: "png",
  openai: "svg", perplexity: "svg", qwen: "png", stepfun: "png",
  tencent: "png", "x-ai": "png", xiaomi: "png", "z-ai": "png",
};

function providerIcon(author, label) {
  const ext = PROVIDER_ICONS[author];
  if (!ext) return `<span class="prov-icon mono">${attr((label || author || "?")[0].toUpperCase())}</span>`;
  const file = author === "meta-llama" ? "meta" : author;
  return `<img class="prov-icon" src="icons/providers/${file}.${ext}" alt="">`;
}

/* ── model picker (custom dropdown) ───────────────────── */

function meterHtml(kind, value) {
  const segs = Array.from({ length: 5 }, (_, i) =>
    `<span class="meter-seg${i < value ? " on" : ""}"></span>`).join("");
  return `
    <div class="meter ${kind}">
      <span class="meter-label">${kind.toUpperCase()}</span>
      <div class="meter-track">${segs}</div>
    </div>`;
}

function rowHtml(m) {
  const live = liveLatency.get(m.id);
  const tip = `${m.languages} · via ${m.provider}` +
    (m.rtfx !== "—" ? ` · ${m.rtfx}x realtime` : "");
  return `
    <div class="model-row${m.id === currentModel ? " selected" : ""}" data-id="${m.id}" title="${tip}">
      ${providerIcon(m.id.split("/")[0], m.provider)}
      <div class="row-main">
        <span class="row-name">${m.name}</span>
        <span class="row-note">${m.note}</span>
      </div>
      ${meterHtml("speed", m.speed)}
      ${meterHtml("quality", m.quality)}
      <div class="row-stat">
        <span class="stat-label">WER</span>
        <span class="stat-val${m.wer === "—" ? " none" : ""}">${m.wer}</span>
      </div>
      <div class="row-price">
        <span class="price">${m.price}</span>
        <span class="lat${live ? "" : " none"}">${live ? live + " ms" : "—"}</span>
      </div>
    </div>`;
}

function renderModelMenu() {
  const suggested = models.filter((m) => m.suggested);
  const rest = models.filter((m) => !m.suggested);
  modelMenu.innerHTML =
    `<div class="menu-section">SUGGESTED</div>` +
    suggested.map(rowHtml).join("") +
    `<div class="menu-section">ALL MODELS</div>` +
    rest.map(rowHtml).join("");
  for (const row of modelMenu.querySelectorAll(".model-row")) {
    row.addEventListener("click", () => {
      currentModel = row.dataset.id;
      closeModelMenu();
      renderModelBar();
      patchSettings();
    });
  }
}

function renderModelBar() {
  const m = models.find((x) => x.id === currentModel);
  $("modelIcon").innerHTML = providerIcon(currentModel.split("/")[0], m && m.provider);
  modelNameEl.textContent = m ? m.name : currentModel;
  const live = liveLatency.get(currentModel);
  modelLatEl.classList.toggle("hidden", !live);
  if (live) modelLatEl.textContent = `${live} ms`;
}

// The menu renders inline in the sidebar directly under the picker.
function openModelMenu() {
  renderModelMenu();
  modelMenu.classList.remove("hidden");
  modelSelect.classList.add("open");
}

function closeModelMenu() {
  modelMenu.classList.add("hidden");
  modelSelect.classList.remove("open");
}

modelSelect.addEventListener("click", () =>
  modelMenu.classList.contains("hidden") ? openModelMenu() : closeModelMenu());

/* ── cleanup-model picker ─────────────────────────────── */

// Fast, cheap, non-reasoning picks; dropped if OpenRouter retires them.
const SUGGESTED_CLEANUP = [
  "google/gemini-3.1-flash-lite", "openai/gpt-4.1-nano",
  "mistralai/mistral-small-3.2-24b-instruct", "meta-llama/llama-3.3-70b-instruct",
  "anthropic/claude-haiku-4.5",
];

// Cents, plus a third decimal only when it carries information ($0.075).
const usd = (v) => {
  const r = Math.round(v * 1000) / 1000;
  return `$${r.toFixed(Math.round(r * 1000) % 10 ? 3 : 2)}`;
};
const ctx = (n) => (n >= 1e6 ? `${+(n / 1e6).toFixed(1)}M` : `${Math.round(n / 1e3)}K`);

function cleanupPriceHtml(m) {
  if (m.input_per_m == null || m.output_per_m == null) return `<span class="price">varies</span>`;
  if (m.input_per_m === 0 && m.output_per_m === 0) return `<span class="price free">Free</span>`;
  return `<span class="price">in ${usd(m.input_per_m)}</span><span class="price">out ${usd(m.output_per_m)}</span>`;
}

function cleanupRowHtml(m) {
  const tags = [m.provider, `${ctx(m.context)} context`].filter(Boolean).join(" · ");
  return `
    <div class="model-row${m.id === cleanupModel ? " selected" : ""}" data-id="${attr(m.id)}" title="${attr(m.description)}">
      ${providerIcon(m.author, m.provider)}
      <div class="row-main">
        <span class="row-name">${attr(m.name)}${m.reasoning_required ? ` <span class="tag" title="Always reasons before answering, which makes cleanup slower">reasoning</span>` : ""}</span>
        <span class="row-note">${attr(tags)} · ${attr(m.id)}</span>
      </div>
      <div class="row-price wide" title="Dollars per million input / output tokens">${cleanupPriceHtml(m)}</div>
    </div>`;
}

function renderCleanupBar() {
  const m = cleanupModels.find((x) => x.id === cleanupModel);
  $("cleanupIcon").innerHTML = providerIcon(m ? m.author : cleanupModel.split("/")[0], m && m.provider);
  $("cleanupName").textContent = m ? m.name : cleanupModel;
  const price = $("cleanupPrice");
  const priced = m && m.input_per_m != null && m.output_per_m != null;
  price.classList.toggle("hidden", !priced);
  if (priced) price.textContent = `${usd(m.input_per_m)} / ${usd(m.output_per_m)} per M`;
}

function renderCleanupMenu() {
  const q = cleanupSearch.value.trim().toLowerCase();
  const match = (m) => !q || `${m.name} ${m.provider} ${m.id}`.toLowerCase().includes(q);
  const suggested = SUGGESTED_CLEANUP.map((id) => cleanupModels.find((m) => m.id === id)).filter((m) => m && match(m));
  const rest = cleanupModels.filter((m) => !SUGGESTED_CLEANUP.includes(m.id) && match(m));
  // Any OpenRouter id works, even one missing from the list.
  const custom = q.includes("/") && !cleanupModels.some((m) => m.id === q)
    ? `<div class="model-row" data-id="${attr(cleanupSearch.value.trim())}"><div class="row-main"><span class="row-name">Use “${attr(cleanupSearch.value.trim())}”</span><span class="row-note">Custom model id</span></div></div>`
    : "";
  $("cleanupRows").innerHTML = custom +
    (suggested.length ? `<div class="menu-section">SUGGESTED · FAST AND CHEAP</div>` + suggested.map(cleanupRowHtml).join("") : "") +
    (rest.length ? `<div class="menu-section">ALL MODELS · ${rest.length}</div>` + rest.map(cleanupRowHtml).join("") : "") +
    (!custom && !suggested.length && !rest.length ? `<div class="menu-section">NO MATCHES</div>` : "");
}

function openCleanupMenu() {
  cleanupSearch.value = "";
  renderCleanupMenu();
  cleanupMenu.classList.remove("hidden");
  cleanupSelect.classList.add("open");
  cleanupSearch.focus();
  cleanupMenu.querySelector(".model-row.selected")?.scrollIntoView({ block: "nearest" });
}

function closeCleanupMenu() {
  cleanupMenu.classList.add("hidden");
  cleanupSelect.classList.remove("open");
}

cleanupSelect.addEventListener("click", () =>
  cleanupMenu.classList.contains("hidden") ? openCleanupMenu() : closeCleanupMenu());
cleanupSearch.addEventListener("input", renderCleanupMenu);
cleanupSearch.addEventListener("keydown", (e) => { if (e.key === "Escape") closeCleanupMenu(); });
$("cleanupRows").addEventListener("click", (e) => {
  const row = e.target.closest(".model-row");
  if (!row) return;
  cleanupModel = row.dataset.id;
  closeCleanupMenu();
  renderCleanupBar();
  patchSettings();
});

/* ── view switching (main ⇄ settings) ─────────────────── */

function setView(settings) {
  viewMain.classList.toggle("on", !settings);
  viewSettings.classList.toggle("on", settings);
  gearIcon.classList.toggle("hidden", settings);
  backIcon.classList.toggle("hidden", !settings);
}

$("settingsBtn").addEventListener("click", () =>
  setView(!viewSettings.classList.contains("on")));

for (const item of document.querySelectorAll(".nav-item")) {
  item.addEventListener("click", () => {
    document.querySelectorAll(".nav-item").forEach((n) => n.classList.toggle("on", n === item));
    document.querySelectorAll(".panel-page").forEach((p) =>
      p.classList.toggle("on", p.dataset.panel === item.dataset.panel));
  });
}

function openSidebarPanel(panel) {
  setView(true);
  document.querySelector(`.nav-item[data-panel="${panel}"]`)?.click();
}

/* ── credentials & settings ───────────────────────────── */

function renderCredentials(s) {
  const method = { oauth: "OpenRouter login", manual: "API key", env: "environment variable" }[s.auth_method];
  credStateEl.innerHTML = s.has_credentials
    ? `<span class="ok">✓ Connected</span> via ${method} <code>${s.key_preview}</code>`
    : `Not connected — log in or paste a key to start transcribing.`;
  logoutBtn.classList.toggle("hidden", !s.has_credentials);
  loginBtn.classList.toggle("hidden", s.has_credentials && s.auth_method === "oauth");
}

function renderHotkey(label) {
  hotkeyBtn.classList.remove("capturing");
  hotkeyBtn.classList.toggle("set", !!label);
  hotkeyLabel.textContent = label || "None";
  hotkeyClear.classList.toggle("hidden", !label);
  hotkeyHint.classList.toggle("hidden", !label);
  if (label) {
    hotkeyKbd.textContent = label;
    hotkeyHint.childNodes[0].textContent = hotkeyMode === "toggle" ? "or press " : "or hold ";
  }
}

function renderEnterHotkey(label) {
  enterHotkeyBtn.classList.remove("capturing");
  enterHotkeyBtn.classList.toggle("set", !!label);
  enterHotkeyLabel.textContent = label || "None";
  enterHotkeyClear.classList.toggle("hidden", !label);
}

function renderMode() {
  for (const b of modeSwitch.querySelectorAll("button")) {
    b.classList.toggle("on", b.dataset.mode === hotkeyMode);
  }
}

function applySettings(s) {
  renderCredentials(s);
  hotkeyMode = s.hotkey_mode || "push_to_talk";
  renderMode();
  renderHotkey(s.hotkey_label);
  renderEnterHotkey(s.enter_hotkey_label);
  typeIntoApp.checked = s.type_into_app;
  launchAtLogin.checked = s.launch_at_login;
  startMinimized.checked = s.start_minimized;
  cleanFillers.checked = s.clean_fillers;
  smartCorrections.checked = s.smart_corrections;
  cleanupModel = s.cleanup_model;
  renderCleanupBar();
  accentInput.value = s.accent;
  vocabInput.value = (s.vocabulary || []).join(", ");
  seedCount.textContent = s.seed_count;
  if (s.model) currentModel = s.model;
  renderModelBar();
  deviceSel.value = s.device ?? "";
  languageInput.value = s.language ?? "";
  if (!s.has_credentials) openSidebarPanel("credentials");
}

async function patchSettings() {
  const s = await invoke("set_settings", {
    patch: {
      model: currentModel,
      device: deviceSel.value || null,
      language: languageInput.value.trim().toLowerCase() || null,
      hotkey_mode: hotkeyMode,
      type_into_app: typeIntoApp.checked,
      launch_at_login: launchAtLogin.checked,
      start_minimized: startMinimized.checked,
      clean_fillers: cleanFillers.checked,
      vocabulary: vocabInput.value.split(",").map((t) => t.trim()).filter(Boolean),
      smart_corrections: smartCorrections.checked,
      cleanup_model: cleanupModel,
      accent: accentInput.value.trim(),
    },
  });
  renderCredentials(s);
}

/* ── init ─────────────────────────────────────────────── */

function renderHistory(entries) {
  if (!entries.length) return;
  clearPlaceholder();
  for (const h of entries) {
    const div = document.createElement("div");
    div.className = "utterance history";
    const when = new Date(h.at).toLocaleString([], {
      month: "short", day: "numeric", hour: "2-digit", minute: "2-digit",
    });
    div.innerHTML = `
      <div class="text"></div>
      <div class="meta">${timingHtml(h, when)}</div>`;
    div.querySelector(".text").textContent = h.text;
    transcriptEl.appendChild(div);
  }
  transcriptEl.scrollTop = transcriptEl.scrollHeight;
}

async function init() {
  const [modelList, devices, history] = await Promise.all([
    invoke("list_models"),
    invoke("list_input_devices"),
    invoke("get_history").catch(() => []),
  ]);
  models = modelList;
  renderHistory(history);
  invoke("list_cleanup_models").then((list) => {
    cleanupModels = list;
    renderCleanupBar();
  }).catch(() => {});
  renderBenchModelList();

  deviceSel.innerHTML =
    `<option value="">Default input</option>` +
    devices.map((d) => `<option>${d}</option>`).join("");

  applySettings(await invoke("get_settings"));
}

/* ── wiring ───────────────────────────────────────────── */

toggleBtn.addEventListener("click", async () => {
  try {
    if (!listening) {
      await patchSettings();
      await invoke("start_listening");
      setListeningUi(true);
    } else {
      await invoke("stop_listening");
      setListeningUi(false);
    }
  } catch (e) {
    addError(String(e));
  }
});


loginBtn.addEventListener("click", async () => {
  try {
    await invoke("login_with_openrouter");
  } catch (e) {
    addError(String(e));
  }
});

$("saveKey").addEventListener("click", async () => {
  const key = apiKeyInput.value.trim();
  if (!key) return;
  try {
    renderCredentials(await invoke("set_api_key", { key }));
    apiKeyInput.value = "";
  } catch (e) {
    addError(String(e));
  }
});

logoutBtn.addEventListener("click", async () => {
  renderCredentials(await invoke("clear_credentials"));
});

hotkeyBtn.addEventListener("click", () => {
  hotkeyBtn.blur(); // so Enter/Space bind instead of re-clicking
  invoke("bind_hotkey", { target: "stt" });
});
hotkeyClear.addEventListener("click", () => invoke("clear_hotkey", { target: "stt" }));

enterHotkeyBtn.addEventListener("click", () => {
  enterHotkeyBtn.blur();
  invoke("bind_hotkey", { target: "enter" });
});
enterHotkeyClear.addEventListener("click", () => invoke("clear_hotkey", { target: "enter" }));

modeSwitch.addEventListener("click", (e) => {
  const mode = e.target.dataset?.mode;
  if (!mode || mode === hotkeyMode) return;
  hotkeyMode = mode;
  renderMode();
  renderHotkey(hotkeyLabel.textContent === "None" ? null : hotkeyLabel.textContent);
  patchSettings();
});

for (const el of [deviceSel, languageInput, typeIntoApp, launchAtLogin, startMinimized, cleanFillers, smartCorrections, accentInput, vocabInput]) {
  el.addEventListener("change", patchSettings);
}

/* ── vocabulary suggestions ───────────────────────────── */

// Suggestions accumulate across dictations (deduped) until accepted.
const pendingSuggestions = new Map(); // term → heard

function renderSuggestions() {
  vocabSuggestions.classList.toggle("hidden", pendingSuggestions.size === 0);
  sugChips.innerHTML = "";
  for (const [term, heard] of pendingSuggestions) {
    const chip = document.createElement("button");
    chip.className = "sug-chip";
    chip.title = `Heard as “${heard}” — click to add “${term}” to your vocabulary`;
    chip.innerHTML = `+ ${term} <span class="heard">(${heard})</span>`;
    chip.addEventListener("click", () => {
      const terms = vocabInput.value.split(",").map((t) => t.trim()).filter(Boolean);
      if (!terms.some((t) => t.toLowerCase() === term.toLowerCase())) {
        terms.push(term);
        vocabInput.value = terms.join(", ");
        patchSettings();
      }
      pendingSuggestions.delete(term);
      renderSuggestions();
    });
    sugChips.appendChild(chip);
  }
}

listen("vocab-suggestions", (e) => {
  const existing = new Set(
    vocabInput.value.split(",").map((t) => t.trim().toLowerCase()).filter(Boolean)
  );
  for (const { heard, term } of e.payload) {
    if (!existing.has(term.toLowerCase())) pendingSuggestions.set(term, heard);
  }
  renderSuggestions();
  // Discoverability without stealing focus: badge the gear button.
  $("settingsBtn").classList.toggle("has-suggestions", pendingSuggestions.size > 0);
});

/* ── backend events ───────────────────────────────────── */

listen("stt-status", (e) => {
  const { state, detail } = e.payload;
  setStatus(state, detail);
  if (state === "error" && detail) addError(detail);
  if (["listening", "speech", "transcribing"].includes(state)) setListeningUi(true);
  if (state === "stopped") setListeningUi(false);
});

listen("stt-transcript", (e) => addUtterance(e.payload));
listen("stt-partial", (e) => showPartial(e.payload));

let capturingTarget = "stt";
listen("hotkey-capturing", (e) => {
  capturingTarget = e.payload === "enter" ? "enter" : "stt";
  const [btn, label] = capturingTarget === "enter"
    ? [enterHotkeyBtn, enterHotkeyLabel] : [hotkeyBtn, hotkeyLabel];
  btn.classList.add("capturing");
  label.textContent = "press a key…";
});
listen("hotkey-capture-refused", () => {
  const label = capturingTarget === "enter" ? enterHotkeyLabel : hotkeyLabel;
  label.textContent = "not that one…";
});
listen("hotkey-set", (e) => {
  const { target, label } = e.payload;
  if (target === "enter") renderEnterHotkey(label);
  else renderHotkey(label);
});
listen("hotkey-error", (e) => addError(e.payload));

listen("auth-status", async (e) => {
  const { state, detail } = e.payload;
  if (state === "pending") {
    loginBtn.textContent = "Waiting for browser…";
    loginBtn.disabled = true;
  } else {
    loginBtn.textContent = "Log in with OpenRouter";
    loginBtn.disabled = false;
    if (state === "success") {
      applySettings(await invoke("get_settings"));
      setView(false);
      setStatus("stopped", "logged in — ready");
    } else if (state === "error") {
      addError(`login: ${detail}`);
    }
  }
});

/* ── benchmark ─────────────────────────────────────────── */

const benchRecord = $("benchRecord");
const benchRun = $("benchRun");
const benchClipInfo = $("benchClipInfo");
const benchResults = $("benchResults");
const benchModelList = $("benchModelList");
const benchModelCount = $("benchModelCount");
let benchRecording = false;
let benchRows = [];
let benchClipSecs = 0;

// Which models are included in a run (persisted locally; default: all).
let benchSelection = new Set(
  JSON.parse(localStorage.getItem("benchSelection") || "[]")
);

function benchSelectedIds() {
  return models.filter((m) => benchSelection.size === 0 || benchSelection.has(m.id)).map((m) => m.id);
}

function renderBenchModelList() {
  benchModelList.innerHTML = "";
  for (const m of models) {
    const included = benchSelection.size === 0 || benchSelection.has(m.id);
    const label = document.createElement("label");
    label.className = "check-row";
    label.innerHTML = `<input type="checkbox" ${included ? "checked" : ""} data-id="${m.id}" /> ${m.name}`;
    label.querySelector("input").addEventListener("change", () => {
      // Materialize "all" into an explicit set on the first change.
      if (benchSelection.size === 0) models.forEach((x) => benchSelection.add(x.id));
      const cb = label.querySelector("input");
      if (cb.checked) benchSelection.add(m.id);
      else benchSelection.delete(m.id);
      localStorage.setItem("benchSelection", JSON.stringify([...benchSelection]));
      updateBenchCount();
    });
    benchModelList.appendChild(label);
  }
  updateBenchCount();
}

function updateBenchCount() {
  const n = benchSelectedIds().length;
  benchModelCount.textContent = `(${n}/${models.length})`;
  benchRun.disabled = benchRun.disabled || n === 0;
}

function setClip(secs) {
  const has = typeof secs === "number" && secs > 0;
  if (has) benchClipSecs = secs;
  benchClipInfo.textContent = has ? `clip: ${secs.toFixed(1)}s` : "no clip yet";
  benchRun.disabled = !has;
}

benchRecord.addEventListener("click", async () => {
  try {
    if (!benchRecording) {
      await invoke("bench_record_start");
      benchRecording = true;
      benchRecord.textContent = "■ Stop";
      benchRecord.classList.add("recording");
    } else {
      await invoke("bench_record_stop");
    }
  } catch (e) {
    addError(String(e));
  }
});

benchRun.addEventListener("click", async () => {
  benchRows = [];
  const ids = benchSelectedIds();
  benchResults.innerHTML = ids
    .map((id) => {
      const m = models.find((x) => x.id === id);
      return `
      <div class="bench-row pending" data-id="${id}">
        <span class="rank">·</span>
        <span class="bname">${m?.name ?? id}</span>
        <span class="bbar" style="width:0"></span>
        <span class="bms">…</span>
      </div>`;
    })
    .join("");
  benchResults.classList.remove("hidden");
  try {
    await invoke("bench_run", { models: ids });
  } catch (e) {
    addError(String(e));
  }
});

// ms/s = milliseconds of processing per second of audio — clip-length
// independent, so runs with different clips stay comparable.
function msPerSec(r) {
  return benchClipSecs > 0 ? Math.round(r.api_ms / benchClipSecs) : null;
}

function renderBenchRanking() {
  const done = benchRows.filter((r) => r.ok).sort((a, b) => a.api_ms - b.api_ms);
  const maxMs = Math.max(...done.map((r) => r.api_ms), 1);
  // Reorder: completed (ranked fastest first), then pending, then failures.
  const failed = benchRows.filter((r) => !r.ok);
  const doneIds = new Set(benchRows.map((r) => r.model));
  const pending = benchSelectedIds().filter((id) => !doneIds.has(id));

  benchResults.innerHTML = [
    ...done.map((r, i) => {
      const m = models.find((x) => x.id === r.model);
      const norm = msPerSec(r);
      return `
        <div class="bench-row${i === 0 ? " best" : ""}" data-id="${r.model}">
          <span class="rank">${i + 1}</span>
          <span class="bname">${m?.name ?? r.model}</span>
          <span class="bbar" style="width:${Math.round((r.api_ms / maxMs) * 90)}px"></span>
          <span class="bnorm">${norm !== null ? norm + " ms/s" : ""}</span>
          <span class="bms">${r.api_ms} ms</span>
        </div>`;
    }),
    ...pending.map((id) => {
      const m = models.find((x) => x.id === id);
      return `
      <div class="bench-row pending" data-id="${id}">
        <span class="rank">·</span><span class="bname">${m?.name ?? id}</span>
        <span class="bbar" style="width:0"></span><span class="bnorm"></span><span class="bms">…</span>
      </div>`;
    }),
    ...failed.map((r) => {
      const m = models.find((x) => x.id === r.model);
      return `
        <div class="bench-row failed" data-id="${r.model}" title="${r.text.replaceAll('"', "&quot;")}">
          <span class="rank">✕</span><span class="bname">${m?.name ?? r.model}</span>
          <span class="bbar" style="width:0"></span><span class="bnorm"></span><span class="bms">failed</span>
        </div>`;
    }),
  ].join("");
}

listen("bench-status", (e) => {
  const { state, detail, clip_secs } = e.payload;
  if (state === "recording") {
    benchClipInfo.textContent = "recording… speak now";
  } else if (state === "recorded") {
    benchRecording = false;
    benchRecord.textContent = "● Record clip";
    benchRecord.classList.remove("recording");
    setClip(clip_secs);
  } else if (state === "running") {
    benchRun.disabled = true;
    benchClipInfo.textContent = `running ${detail} on ${clip_secs?.toFixed(1)}s clip…`;
  } else if (state === "done") {
    benchRun.disabled = false;
    setClip(clip_secs);
  } else if (state === "error") {
    benchRecording = false;
    benchRecord.textContent = "● Record clip";
    benchRecord.classList.remove("recording");
    if (detail) addError(`benchmark: ${detail}`);
  }
});

listen("bench-result", (e) => {
  const r = e.payload;
  benchRows.push(r);
  renderBenchRanking();
  // Benchmark latencies are real measurements — feed the picker too.
  if (r.ok) {
    const best = liveLatency.get(r.model);
    if (!best || r.api_ms < best) liveLatency.set(r.model, r.api_ms);
    renderModelBar();
  }
});

invoke("bench_clip_info").then(setClip).catch(() => {});

// Re-hydrate the last benchmark run: ranked results reappear after restart
// and their measured latencies feed the model picker.
invoke("get_last_benchmark")
  .then((run) => {
    if (!run || !run.results?.length) return;
    benchRows = run.results;
    benchClipSecs = run.clip_secs || 0;
    for (const r of run.results) {
      if (r.ok) {
        const best = liveLatency.get(r.model);
        if (!best || r.api_ms < best) liveLatency.set(r.model, r.api_ms);
      }
    }
    renderBenchRanking();
    benchResults.classList.remove("hidden");
    renderModelBar();
  })
  .catch(() => {});

init().catch((e) => addError(String(e)));
