// Local AI Flow HUD — live level waveform + streaming partial transcript.
// Source of truth for dist/hud.js (build: `npm run build` in apps/desktop/ui).

declare global {
  interface Window {
    __TAURI__: {
      event: { listen: (name: string, cb: (e: { payload: UiEvent }) => void) => Promise<unknown> };
      core: { invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T> };
    };
  }
}

type UiEvent =
  | { event: "phase"; phase: string; mode: string }
  | { event: "level"; rms: number; peak: number }
  | { event: "partial"; text: string }
  | { event: "final_segment"; text: string }
  | { event: "inserted"; report: unknown; text: string }
  | { event: string; [k: string]: unknown };

(function () {
  const { listen } = window.__TAURI__.event;

  const canvas = document.getElementById("wave") as HTMLCanvasElement;
  const ctx = canvas.getContext("2d")!;
  const dot = document.getElementById("dot")!;
  const phaseEl = document.getElementById("phase")!;
  const modeEl = document.getElementById("mode")!;
  const textEl = document.getElementById("text")!;

  const BARS = 64;
  const levels: number[] = new Array(BARS).fill(0.02);
  let finals: string[] = [];
  let partial = "";
  let phase = "idle";

  function resize(): void {
    const r = canvas.getBoundingClientRect();
    canvas.width = r.width * devicePixelRatio;
    canvas.height = r.height * devicePixelRatio;
  }
  window.addEventListener("resize", resize);
  resize();

  function draw(): void {
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

  function renderText(): void {
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
    const ev = event.payload as UiEvent;
    switch (ev.event) {
      case "level":
        levels.push((ev as { rms: number }).rms);
        levels.shift();
        break;
      case "partial":
        partial = (ev as { text: string }).text;
        renderText();
        break;
      case "final_segment":
        finals.push((ev as { text: string }).text);
        partial = "";
        renderText();
        break;
      case "phase": {
        const p = ev as { phase: string; mode: string };
        phase = p.phase;
        phaseEl.textContent = p.phase;
        modeEl.textContent = p.mode;
        dot.className =
          "hud-dot " + (p.phase === "listening" ? "listening" : p.phase === "idle" ? "" : "processing");
        if (p.phase === "idle") {
          finals = [];
          partial = "";
          renderText();
          levels.fill(0.02);
        }
        break;
      }
      case "inserted":
        finals = [];
        partial = "";
        renderText();
        break;
    }
  });
})();

export {};
