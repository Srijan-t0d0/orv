// Floating dictation indicator. Runs in the transparent, click-through
// overlay window; Tauri events are global, so it hears the same stt-*
// stream as the main window.
//
// The waveform is REAL: the pipeline emits stt-level (mic RMS, 0..1) at
// ~16Hz. Bars stay IN PLACE (no scrolling) and breathe with the level —
// same feel as the animated logo in the dashboard header: each bar has its
// own phase/weight, all driven by the live mic level.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const pill = document.getElementById("pill");
const textEl = document.getElementById("text");
const canvas = document.getElementById("wave");
const ctx = canvas.getContext("2d");

const BARS = 5;
// Static per-bar character, like the logo's staggered bars.
const WEIGHT = [0.55, 1.0, 0.75, 0.9, 0.6];
const PHASE = [0.0, 1.3, 2.6, 3.9, 5.2];

let level = 0;        // latest mic RMS (0..1), decays between events
let smooth = 0;       // eased level for fluid motion
let active = false;
let hideTimer = null;
let raf = null;

/* ── waveform ─────────────────────────────────────────── */

function drawWave(now) {
  const w = canvas.width, h = canvas.height;
  ctx.clearRect(0, 0, w, h);

  // Ease toward the live level; decay so bars settle when you pause.
  smooth += (level - smooth) * 0.25;
  level *= 0.94;

  const bw = 4;
  const gap = (w - BARS * bw) / (BARS - 1);
  const t = now / 1000;
  for (let i = 0; i < BARS; i++) {
    // Idle: gentle logo-like sway. Speaking: level drives the amplitude,
    // per-bar sine phase keeps them dancing out of step.
    const idle = 0.16 + 0.06 * Math.sin(t * 2.2 + PHASE[i]);
    const drive = smooth * (0.45 + 0.55 * Math.abs(Math.sin(t * 7 + PHASE[i]))) * WEIGHT[i];
    const v = Math.min(1, idle + drive);
    const bh = Math.max(3, v * h);
    const x = i * (bw + gap);
    const y = (h - bh) / 2; // grow from the vertical center
    ctx.fillStyle = smooth > 0.06 ? "#c8ff00" : "rgba(252,252,254,0.4)";
    ctx.beginPath();
    ctx.roundRect(x, y, bw, bh, 2);
    ctx.fill();
  }

  if (active) raf = requestAnimationFrame(drawWave);
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
    smooth = 0;
    raf = requestAnimationFrame(drawWave);
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
  level = Math.max(level, e.payload.level);
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

// Final transcript: bright flash, then the stopped-fade takes it away.
listen("stt-transcript", (e) => {
  show();
  setText(e.payload.text, true, false);
  scheduleHide(1600);
});
