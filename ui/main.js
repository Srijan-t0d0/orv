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
const settingsPanel = $("settings");
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

function addUtterance(ev) {
  const div = utteranceDiv(ev.id);
  div.classList.remove("partial");
  const slow = ev.total_ms > 1200 ? " slow" : "";
  div.querySelector(".text").textContent = ev.text;
  div.querySelector(".meta").innerHTML = `
      <span class="latency${slow}">${ev.total_ms} ms</span>
      <span>api ${ev.api_ms} ms</span>
      <span>${ev.audio_secs.toFixed(1)}s audio</span>
      <span>${ev.model}</span>`;
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
  modelNameEl.textContent = m ? m.name : currentModel;
  const live = liveLatency.get(currentModel);
  modelLatEl.classList.toggle("hidden", !live);
  if (live) modelLatEl.textContent = `${live} ms`;
}

function openModelMenu() {
  renderModelMenu();
  modelMenu.style.top = `${modelSelect.getBoundingClientRect().bottom + 6}px`;
  modelMenu.classList.remove("hidden");
  modelSelect.classList.add("open");
}

function closeModelMenu() {
  modelMenu.classList.add("hidden");
  modelSelect.classList.remove("open");
}

modelSelect.addEventListener("click", () =>
  modelMenu.classList.contains("hidden") ? openModelMenu() : closeModelMenu());
document.addEventListener("click", (e) => {
  if (!modelMenu.contains(e.target) && !modelSelect.contains(e.target)) closeModelMenu();
});

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
  if (s.model) currentModel = s.model;
  renderModelBar();
  deviceSel.value = s.device ?? "";
  languageInput.value = s.language ?? "";
  if (!s.has_credentials) settingsPanel.classList.remove("hidden");
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
    },
  });
  renderCredentials(s);
}

/* ── init ─────────────────────────────────────────────── */

async function init() {
  const [modelList, devices] = await Promise.all([
    invoke("list_models"),
    invoke("list_input_devices"),
  ]);
  models = modelList;

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

$("settingsBtn").addEventListener("click", () => settingsPanel.classList.toggle("hidden"));

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

for (const el of [deviceSel, languageInput, typeIntoApp, launchAtLogin, startMinimized]) {
  el.addEventListener("change", patchSettings);
}

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
      settingsPanel.classList.add("hidden");
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
let benchRecording = false;
let benchRows = [];

function setClip(secs) {
  const has = typeof secs === "number" && secs > 0;
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
  benchResults.innerHTML = models
    .map((m) => `
      <div class="bench-row pending" data-id="${m.id}">
        <span class="rank">·</span>
        <span class="bname">${m.name}</span>
        <span class="bbar" style="width:0"></span>
        <span class="bms">…</span>
      </div>`)
    .join("");
  benchResults.classList.remove("hidden");
  try {
    await invoke("bench_run");
  } catch (e) {
    addError(String(e));
  }
});

function renderBenchRanking() {
  const done = benchRows.filter((r) => r.ok).sort((a, b) => a.api_ms - b.api_ms);
  const maxMs = Math.max(...done.map((r) => r.api_ms), 1);
  // Reorder: completed (ranked fastest first), then pending, then failures.
  const failed = benchRows.filter((r) => !r.ok);
  const doneIds = new Set(benchRows.map((r) => r.model));
  const pending = models.filter((m) => !doneIds.has(m.id));

  benchResults.innerHTML = [
    ...done.map((r, i) => {
      const m = models.find((x) => x.id === r.model);
      return `
        <div class="bench-row${i === 0 ? " best" : ""}" data-id="${r.model}">
          <span class="rank">${i + 1}</span>
          <span class="bname">${m?.name ?? r.model}</span>
          <span class="bbar" style="width:${Math.round((r.api_ms / maxMs) * 120)}px"></span>
          <span class="bms">${r.api_ms} ms</span>
        </div>`;
    }),
    ...pending.map((m) => `
      <div class="bench-row pending" data-id="${m.id}">
        <span class="rank">·</span><span class="bname">${m.name}</span>
        <span class="bbar" style="width:0"></span><span class="bms">…</span>
      </div>`),
    ...failed.map((r) => {
      const m = models.find((x) => x.id === r.model);
      return `
        <div class="bench-row failed" data-id="${r.model}" title="${r.text.replaceAll('"', "&quot;")}">
          <span class="rank">✕</span><span class="bname">${m?.name ?? r.model}</span>
          <span class="bbar" style="width:0"></span><span class="bms">failed</span>
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

init().catch((e) => addError(String(e)));
