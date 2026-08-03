// Floating dictation indicator. Runs in the transparent, click-through
// overlay window; Tauri events are global, so it hears the same stt-*
// stream as the main window.
//
// The waveform is REAL: the pipeline emits stt-level (mic RMS, 0..1) at
// ~16Hz; we keep a scrolling history and draw it as bars, so the display
// moves exactly with your voice.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const pill = document.getElementById("pill");
const textEl = document.getElementById("text");
const canvas = document.getElementById("wave");
const ctx = canvas.getContext("2d");

const BARS = 14;
const levels = new Array(BARS).fill(0);
let target = 0; // most recent mic level
let hideTimer = null;
let active = false;
let raf = null;

/* ── waveform ─────────────────────────────────────────── */

function drawWave() {
  const w = canvas.width, h = canvas.height;
  ctx.clearRect(0, 0, w, h);
  const bw = 3, gap = (w - BARS * bw) / (BARS - 1);
  for (let i = 0; i < BARS; i++) {
    // 0.12 floor keeps a visible idle baseline
    const v = Math.max(0.12, levels[i]);
    const bh = Math.max(2, v * h);
    const x = i * (bw + gap);
    const y = (h - bh) / 2;
    ctx.fillStyle = levels[i] > 0.14 ? "#c8ff00" : "rgba(252,252,254,0.35)";
    ctx.beginPath();
    ctx.roundRect(x, y, bw, bh, 1.5);
    ctx.fill();
  }
}

function tick() {
  // scroll history left, append the newest level (with a little decay
  // easing so single spikes still render as humps)
  levels.shift();
  const prev = levels[levels.length - 1] ?? 0;
  levels.push(Math.max(target, prev * 0.55));
  drawWave();
  raf = active ? requestAnimationFrame(() => setTimeout(tick, 40)) : null;
}

/* ── text ─────────────────────────────────────────────── */

function setText(content, isFinal, isHint) {
  textEl.className = `text${isFinal ? " final" : ""}`;
  textEl.innerHTML = "";
  if (isHint) {
    const span = document.createElement("span");
    span.className = "hint";
    span.textContent = content;
    textEl.appendChild(span);
  } else {
    // <bdi> keeps LTR text rendering correctly inside the RTL-clipped box
    const bdi = document.createElement("bdi");
    bdi.textContent = content;
    textEl.appendChild(bdi);
  }
}

/* ── lifecycle ────────────────────────────────────────── */

function show() {
  clearTimeout(hideTimer);
  if (!active) {
    active = true;
    levels.fill(0);
    tick();
  }
  pill.classList.add("visible");
}

function scheduleHide(delayMs) {
  clearTimeout(hideTimer);
  hideTimer = setTimeout(() => {
    active = false;
    if (raf) cancelAnimationFrame(raf);
    pill.classList.remove("visible");
    setTimeout(() => {
      if (!active) invoke("overlay_hide").catch(() => {});
    }, 260);
  }, delayMs);
}

/* ── events ───────────────────────────────────────────── */

listen("stt-level", (e) => {
  target = e.payload.level;
});

listen("stt-status", (e) => {
  const { state } = e.payload;
  switch (state) {
    case "listening":
      show();
      setText("Listening…", false, true);
      break;
    case "speech":
      show();
      break;
    case "transcribing":
      show();
      target = 0;
      setText("Transcribing…", false, true);
      break;
    case "stopped":
      // Final transcript may still be rendering — let it breathe first.
      scheduleHide(1600);
      break;
    case "error":
      scheduleHide(400);
      break;
  }
});

// Rolling preview of the whole session so far — supersedes previous text.
listen("stt-partial", (e) => {
  show();
  setText(e.payload.text, false, false);
});

// Final transcript: bright flash, then the stopped-fade takes it away.
listen("stt-transcript", (e) => {
  show();
  setText(e.payload.text, true, false);
  scheduleHide(1600);
});
