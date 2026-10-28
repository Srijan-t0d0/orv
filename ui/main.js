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

// Per-stage timing: "stt 180/640 ms" = latency (first byte) / duration.
function stageSpans(e) {
  const ttfb = (v) => (v == null ? "–" : v);
  let html = `<span title="speech-to-text latency / duration">stt ${ttfb(e.stt_ttfb_ms)}/${e.api_ms} ms</span>`;
  if (e.llm) {
    const failed = e.llm.error ? " slow" : "";
    html += `<span class="${failed}" title="${e.llm.model}${e.llm.error ? ` — ${e.llm.error.replace(/"/g, "&quot;")}` : ""}">llm ${ttfb(e.llm.ttfb_ms)}/${e.llm.ms} ms${e.llm.error ? " ✕" : ""}</span>`;
  }
  return html;
}

function addUtterance(ev) {
  const div = utteranceDiv(ev.id);
  div.classList.remove("partial");
  const slow = ev.total_ms > 1200 ? " slow" : "";
  div.querySelector(".text").textContent = ev.text;
  div.querySelector(".meta").innerHTML = `
      <span class="latency${slow}">${ev.total_ms} ms</span>
      ${stageSpans(ev)}
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
      <div class="meta">
        <span>${when}</span>
        <span class="latency">${h.total_ms} ms</span>
        ${stageSpans(h)}
        <span>${h.audio_secs.toFixed(1)}s audio</span>
        <span>${h.model}</span>
      </div>`;
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

for (const el of [deviceSel, languageInput, typeIntoApp, launchAtLogin, startMinimized, cleanFillers, smartCorrections, vocabInput]) {
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
