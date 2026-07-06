// Local AI Flow HUD — live level waveform + streaming partial transcript.
// Compiled from ../src/hud.ts (types stripped); keep the two in sync.
"use strict";
(function () {
  const { listen } = window.__TAURI__.event;

  const canvas = document.getElementById("wave");
  const ctx = canvas.getContext("2d");
  const dot = document.getElementById("dot");
  const phaseEl = document.getElementById("phase");
  const modeEl = document.getElementById("mode");
  const textEl = document.getElementById("text");

  const BARS = 64;
  const levels = new Array(BARS).fill(0.02);
  let finals = [];
  let partial = "";
  let phase = "idle";

  function resize() {
    const r = canvas.getBoundingClientRect();
    canvas.width = r.width * devicePixelRatio;
    canvas.height = r.height * devicePixelRatio;
  }
  window.addEventListener("resize", resize);
  resize();

  function draw() {
    const w = canvas.width, h = canvas.height;
    ctx.clearRect(0, 0, w, h);
    const bw = w / BARS;
    for (let i = 0; i < BARS; i++) {
      const v = Math.min(1, levels[i] * 6);
      const bh = Math.max(2 * devicePixelRatio, v * h);
      const x = i * bw;
      const grad = ctx.createLinearGradient(0, h, 0, 0);
      grad.addColorStop(0, "#4f8cff");
      grad.addColorStop(1, "#7fd0ff");
      ctx.fillStyle = phase === "listening" ? grad : "#3a4653";
      ctx.fillRect(x + bw * 0.15, (h - bh) / 2, bw * 0.7, bh);
    }
    requestAnimationFrame(draw);
  }
  requestAnimationFrame(draw);

  function renderText() {
    const tail = finals.slice(-2).join(" ");
    textEl.innerHTML = "";
    const done = document.createElement("span");
    done.textContent = tail ? tail + " " : "";
    const part = document.createElement("span");
    part.className = "partial";
    part.textContent = partial;
    textEl.appendChild(done);
    textEl.appendChild(part);
  }

  listen("laf://ui", (event) => {
    const ev = event.payload;
    switch (ev.event) {
      case "level":
        levels.push(ev.rms);
        levels.shift();
        break;
      case "partial":
        partial = ev.text;
        renderText();
        break;
      case "final_segment":
        finals.push(ev.text);
        partial = "";
        renderText();
        break;
      case "phase":
        phase = ev.phase;
        phaseEl.textContent = ev.phase;
        modeEl.textContent = ev.mode;
        dot.className =
          "hud-dot " + (ev.phase === "listening" ? "listening" : ev.phase === "idle" ? "" : "processing");
        if (ev.phase === "idle") {
          finals = [];
          partial = "";
          renderText();
          levels.fill(0.02);
        }
        break;
      case "inserted":
        finals = [];
        partial = "";
        renderText();
        break;
    }
  });
})();
