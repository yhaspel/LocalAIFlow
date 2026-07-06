// Local AI Flow — Settings app (TypeScript source of truth for dist/app.js;
// build with `npm run build`, typecheck with `npm run typecheck`).
//
// The committed dist/app.js is this file with types stripped so the repo
// builds without a Node toolchain.

interface Tauri {
  core: { invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T> };
  event: { listen: (name: string, cb: (e: { payload: AnyEvent }) => void) => Promise<unknown> };
}
declare global {
  interface Window { __TAURI__: Tauri }
}

interface DictEntry { from: string; to: string; match_case: boolean }
interface Settings {
  schema_version: number;
  hotkeys: {
    dictate_toggle: string; dictate_ptt: string; ptt_enabled: boolean;
    read_selection: string; stop_speech: string;
  };
  default_mode: string;
  language: string;
  input_device: string | null;
  stt: { engine: string; model_id: string; threads: number };
  cleaner: { tier: string; model_id: string; ollama_model: string };
  tts: { engine: string; voice_id: string; rate: number };
  insert_incremental: boolean;
  fully_offline: boolean;
  launch_at_login: boolean;
  hud_enabled: boolean;
  dictionary: DictEntry[];
  model_idle_unload_secs: number;
  onboarding_done: boolean;
}
interface ModelStatus {
  spec: { id: string; label: string; kind: string; note: string; license: string; size_bytes: number | null };
  installed: boolean; bundled: boolean; path: string | null; bytes_on_disk: number | null;
}
interface VoiceInfo { id: string; label: string; language: string; engine: string }
interface DoctorReport {
  platform: string; session: string;
  checks: { id: string; label: string; status: "ok" | "warn" | "fail"; detail: string; fix_hint: string }[];
}
interface StageStats { stage: string; count: number; last_ms: number; p50_ms: number; p95_ms: number }
type AnyEvent = { event: string; [k: string]: unknown };

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = <T extends HTMLElement = HTMLElement>(id: string): T => document.getElementById(id) as T;
let settings: Settings;
let models: ModelStatus[] = [];
const RECOMMENDED = [
  "whisper-large-v3-turbo-q5",
  "qwen2.5-3b-instruct-q4",
  "kokoro-v1-q8",
  "kokoro-tokenizer",
  "kokoro-voice-af-heart",
];

// ---------- tabs ----------
document.querySelectorAll<HTMLButtonElement>("nav button[data-tab]").forEach((b) =>
  b.addEventListener("click", () => showTab(b.dataset.tab!))
);
function showTab(name: string): void {
  document.querySelectorAll<HTMLButtonElement>("nav button[data-tab]").forEach((b) =>
    b.classList.toggle("active", b.dataset.tab === name)
  );
  document.querySelectorAll("section.tab").forEach((s) =>
    s.classList.toggle("visible", s.id === "tab-" + name)
  );
  if (name === "doctor") void refreshDoctor();
  if (name === "debug") void refreshLatency();
}
void listen("laf://open-doctor", () => showTab("doctor"));

function toast(msg: string, ms = 3200): void {
  const t = $("toast");
  t.textContent = msg;
  t.classList.add("show");
  setTimeout(() => t.classList.remove("show"), ms);
}

// ---------- load ----------
async function load(): Promise<void> {
  settings = await invoke<Settings>("settings_get");
  const modes = await invoke<{ id: string; label: string }[]>("modes_list");
  $<HTMLSelectElement>("s-mode").innerHTML = modes.map((m) => `<option value="${m.id}">${m.label}</option>`).join("");
  $<HTMLSelectElement>("s-mode").value = settings.default_mode;
  $<HTMLSelectElement>("s-language").value = settings.language;
  $<HTMLSelectElement>("s-cleaner").value = settings.cleaner.tier;
  $<HTMLInputElement>("s-incremental").checked = settings.insert_incremental;
  $<HTMLInputElement>("s-hud").checked = settings.hud_enabled;
  $<HTMLInputElement>("s-autostart").checked = settings.launch_at_login;
  $<HTMLInputElement>("s-offline").checked = settings.fully_offline;
  $<HTMLInputElement>("s-unload").value = String(settings.model_idle_unload_secs);

  const devices = await invoke<string[]>("input_devices");
  $<HTMLSelectElement>("s-device").innerHTML =
    `<option value="">System default</option>` +
    devices.map((d) => `<option value="${esc(d)}">${esc(d)}</option>`).join("");
  $<HTMLSelectElement>("s-device").value = settings.input_device ?? "";

  $<HTMLInputElement>("hk-toggle").value = settings.hotkeys.dictate_toggle;
  $<HTMLInputElement>("hk-ptt").value = settings.hotkeys.dictate_ptt;
  $<HTMLInputElement>("hk-ptt-on").checked = settings.hotkeys.ptt_enabled;
  $<HTMLInputElement>("hk-read").value = settings.hotkeys.read_selection;
  $<HTMLInputElement>("hk-stop").value = settings.hotkeys.stop_speech;

  $<HTMLSelectElement>("tts-engine").value = settings.tts.engine;
  $<HTMLInputElement>("tts-rate").value = String(settings.tts.rate);
  $("tts-rate-val").textContent = settings.tts.rate.toFixed(2) + "×";

  renderDictionary();
  await refreshModels();
  await refreshVoices();

  $("onboarding").style.display = settings.onboarding_done ? "none" : "block";
  if (!settings.onboarding_done) void renderPermissions();

  const info = await invoke<{ version: string; models_dir: string; config_dir: string; offline_build: boolean }>("app_info");
  $("appinfo").textContent =
    `v${info.version} · models: ${info.models_dir} · config: ${info.config_dir}` +
    (info.offline_build ? " · OFFLINE BUILD (no network code compiled in)" : "");
}

function esc(s: unknown): string {
  return String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]!));
}

// ---------- collect + save ----------
function collect(): void {
  settings.default_mode = $<HTMLSelectElement>("s-mode").value;
  settings.language = $<HTMLSelectElement>("s-language").value;
  settings.input_device = $<HTMLSelectElement>("s-device").value || null;
  settings.cleaner.tier = $<HTMLSelectElement>("s-cleaner").value;
  settings.stt.model_id = $<HTMLSelectElement>("s-sttmodel").value || settings.stt.model_id;
  settings.cleaner.model_id = $<HTMLSelectElement>("s-llmmodel").value || settings.cleaner.model_id;
  settings.insert_incremental = $<HTMLInputElement>("s-incremental").checked;
  settings.hud_enabled = $<HTMLInputElement>("s-hud").checked;
  settings.launch_at_login = $<HTMLInputElement>("s-autostart").checked;
  settings.fully_offline = $<HTMLInputElement>("s-offline").checked;
  settings.model_idle_unload_secs = Number($<HTMLInputElement>("s-unload").value) || 0;
  settings.hotkeys.dictate_toggle = $<HTMLInputElement>("hk-toggle").value;
  settings.hotkeys.dictate_ptt = $<HTMLInputElement>("hk-ptt").value;
  settings.hotkeys.ptt_enabled = $<HTMLInputElement>("hk-ptt-on").checked;
  settings.hotkeys.read_selection = $<HTMLInputElement>("hk-read").value;
  settings.hotkeys.stop_speech = $<HTMLInputElement>("hk-stop").value;
  settings.tts.engine = $<HTMLSelectElement>("tts-engine").value;
  settings.tts.voice_id = $<HTMLSelectElement>("tts-voice").value || settings.tts.voice_id;
  settings.tts.rate = Number($<HTMLInputElement>("tts-rate").value);
  settings.dictionary = readDictionary();
}

async function save(): Promise<void> {
  collect();
  try {
    const warnings = await invoke<string[]>("settings_set", { settings });
    $("hk-warnings").textContent = warnings.join("\n");
    $("save-status").textContent = "Saved ✓";
    setTimeout(() => ($("save-status").textContent = ""), 2000);
    if (warnings.length) toast("Saved with warnings:\n" + warnings.join("\n"), 5000);
  } catch (e) {
    toast("Save failed: " + e);
  }
}
$("save").addEventListener("click", () => void save());
$("dictate-now").addEventListener("click", () => void invoke("dictation_start"));
$("tts-rate").addEventListener("input", () => {
  $("tts-rate-val").textContent = Number($<HTMLInputElement>("tts-rate").value).toFixed(2) + "×";
});
$("tts-test").addEventListener("click", async () => {
  collect();
  await invoke("settings_set", { settings });
  void invoke("tts_read_selection");
});

// ---------- hotkey capture ----------
document.querySelectorAll<HTMLInputElement>(".hotkey-input").forEach((input) => {
  input.addEventListener("click", () => {
    input.classList.add("recording");
    input.value = "press keys…";
    const onKey = (e: KeyboardEvent): void => {
      e.preventDefault();
      if (["ControlLeft","ControlRight","AltLeft","AltRight","ShiftLeft","ShiftRight","MetaLeft","MetaRight"].includes(e.code)) return;
      const parts: string[] = [];
      if (e.ctrlKey) parts.push("ctrl");
      if (e.altKey) parts.push("alt");
      if (e.shiftKey) parts.push("shift");
      if (e.metaKey) parts.push("super");
      parts.push(e.code);
      input.value = parts.join("+");
      input.classList.remove("recording");
      window.removeEventListener("keydown", onKey, true);
    };
    window.addEventListener("keydown", onKey, true);
  });
});

// ---------- dictionary ----------
function renderDictionary(): void {
  const tbody = $("dict-table").querySelector("tbody")!;
  tbody.innerHTML = "";
  for (const e of settings.dictionary) addDictRow(e.from, e.to, e.match_case);
}
function addDictRow(from = "", to = "", matchCase = false): void {
  const tbody = $("dict-table").querySelector("tbody")!;
  const tr = document.createElement("tr");
  tr.innerHTML =
    `<td><input class="d-from" value="${esc(from)}" placeholder="cuber netties"/></td>` +
    `<td><input class="d-to" value="${esc(to)}" placeholder="Kubernetes"/></td>` +
    `<td><input type="checkbox" class="d-case" ${matchCase ? "checked" : ""}/></td>` +
    `<td><button class="danger d-del">✕</button></td>`;
  tr.querySelector(".d-del")!.addEventListener("click", () => tr.remove());
  tbody.appendChild(tr);
}
$("dict-add").addEventListener("click", () => addDictRow());
function readDictionary(): DictEntry[] {
  return Array.from($("dict-table").querySelectorAll<HTMLTableRowElement>("tbody tr"))
    .map((tr) => ({
      from: tr.querySelector<HTMLInputElement>(".d-from")!.value.trim(),
      to: tr.querySelector<HTMLInputElement>(".d-to")!.value.trim(),
      match_case: tr.querySelector<HTMLInputElement>(".d-case")!.checked,
    }))
    .filter((e) => e.from && e.to);
}

// ---------- models ----------
async function refreshModels(): Promise<void> {
  models = await invoke<ModelStatus[]>("models_list");
  const bykind = (k: string) => models.filter((m) => m.spec.kind === k);
  $<HTMLSelectElement>("s-sttmodel").innerHTML = bykind("stt")
    .map((m) => `<option value="${m.spec.id}" ${m.installed ? "" : "disabled"}>${esc(m.spec.label)}${m.installed ? "" : " (not installed)"}</option>`)
    .join("");
  $<HTMLSelectElement>("s-sttmodel").value = settings.stt.model_id;
  $<HTMLSelectElement>("s-llmmodel").innerHTML = bykind("cleaner")
    .map((m) => `<option value="${m.spec.id}" ${m.installed ? "" : "disabled"}>${esc(m.spec.label)}${m.installed ? "" : " (not installed)"}</option>`)
    .join("");
  $<HTMLSelectElement>("s-llmmodel").value = settings.cleaner.model_id;

  const list = $("models-list");
  list.innerHTML = "";
  for (const m of models) {
    const row = document.createElement("div");
    row.className = "row";
    const size = m.spec.size_bytes ? (m.spec.size_bytes / 1e6).toFixed(0) + " MB" : "";
    const status = m.installed
      ? `<span class="badge ok">${m.bundled ? "bundled" : "installed"}</span>`
      : `<span class="badge warn">not installed</span>`;
    row.innerHTML =
      `<div class="info"><b>${esc(m.spec.label)} ${status}</b>` +
      `<span>${esc(m.spec.note)} · ${size} · ${esc(m.spec.license)}</span>` +
      `<div class="progress" style="display:none;margin-top:6px"><div></div></div></div>` +
      `<div style="display:flex;gap:6px">` +
      (m.installed
        ? `<button class="m-verify">Verify</button>` + (m.bundled ? "" : `<button class="danger m-del">Delete</button>`)
        : `<button class="primary m-dl">Download</button>`) +
      `</div>`;
    row.dataset.id = m.spec.id;
    row.querySelector(".m-dl")?.addEventListener("click", () => void download(m.spec.id, row));
    row.querySelector(".m-del")?.addEventListener("click", async () => {
      await invoke("model_delete", { id: m.spec.id });
      void refreshModels();
    });
    const ver = row.querySelector<HTMLButtonElement>(".m-verify");
    ver?.addEventListener("click", async () => {
      ver.textContent = "…";
      const ok = await invoke<boolean>("model_verify", { id: m.spec.id });
      ver.textContent = ok ? "Verified ✓" : "CORRUPT ✕";
      if (!ok) toast("Checksum mismatch — delete and re-download this model.");
    });
    list.appendChild(row);
  }
}

async function download(id: string, row: HTMLElement | null): Promise<void> {
  const bar = row?.querySelector<HTMLElement>(".progress");
  if (bar) bar.style.display = "block";
  try {
    await invoke("model_download", { id });
  } catch (e) {
    toast("Download failed: " + e, 6000);
  }
  await refreshModels();
  await refreshVoices();
}

void listen("laf://ui", (event) => {
  const ev = event.payload;
  if (ev.event === "model_download") {
    const row = document.querySelector<HTMLElement>(`[data-id="${ev.model_id as string}"]`);
    if (row) {
      const bar = row.querySelector<HTMLElement>(".progress")!;
      bar.style.display = "block";
      const total = ev.total as number;
      const pct = total > 0 ? (100 * (ev.downloaded as number)) / total : 0;
      (bar.firstElementChild as HTMLElement).style.width = pct.toFixed(1) + "%";
    }
  } else if (ev.event === "pipeline_error") {
    toast(ev.message as string, 5000);
  } else if (ev.event === "inserted") {
    const report = ev.report as { chars: number; method: { method: string } };
    $("save-status").textContent = `inserted ${report.chars} chars via ${report.method.method}`;
    setTimeout(() => ($("save-status").textContent = ""), 4000);
  }
  logEvent(ev);
});

// ---------- voices ----------
async function refreshVoices(): Promise<void> {
  const voices = await invoke<VoiceInfo[]>("voices_list");
  const engine = $<HTMLSelectElement>("tts-engine").value;
  const filtered = voices.filter((v) => v.engine === engine);
  const use = filtered.length ? filtered : voices;
  $<HTMLSelectElement>("tts-voice").innerHTML = use
    .map((v) => `<option value="${esc(v.id)}">${esc(v.label)} — ${esc(v.engine)}</option>`)
    .join("");
  $<HTMLSelectElement>("tts-voice").value = settings.tts.voice_id;
}
$("tts-engine").addEventListener("change", () => void refreshVoices());

// ---------- doctor ----------
async function refreshDoctor(): Promise<void> {
  const report = await invoke<DoctorReport>("doctor_report");
  const list = $("doctor-list");
  list.innerHTML = `<p class="sub">platform: ${esc(report.platform)} · session: ${esc(report.session)}</p>`;
  for (const c of report.checks) {
    const row = document.createElement("div");
    row.className = "row";
    row.innerHTML =
      `<div class="info"><b>${esc(c.label)} <span class="badge ${c.status}">${c.status}</span></b>` +
      `<span>${esc(c.detail)}${c.fix_hint ? "<br/>fix: <span class='mono'>" + esc(c.fix_hint) + "</span>" : ""}</span></div>`;
    list.appendChild(row);
  }
}
$("doctor-refresh").addEventListener("click", () => void refreshDoctor());

// ---------- debug ----------
async function refreshLatency(): Promise<void> {
  const stats = await invoke<StageStats[]>("latency_summary");
  const tbody = $("lat-table").querySelector("tbody")!;
  tbody.innerHTML = stats
    .map((s) => `<tr><td>${esc(s.stage)}</td><td>${s.count}</td><td>${s.last_ms} ms</td><td>${s.p50_ms} ms</td><td>${s.p95_ms} ms</td></tr>`)
    .join("");
}
$("lat-refresh").addEventListener("click", () => void refreshLatency());
const evlog: string[] = [];
function logEvent(ev: AnyEvent): void {
  if (ev.event === "level") return;
  evlog.push(`${new Date().toLocaleTimeString()} ${JSON.stringify(ev)}`);
  if (evlog.length > 120) evlog.shift();
  const el = $("evlog");
  el.textContent = evlog.join("\n");
  el.scrollTop = el.scrollHeight;
}

// ---------- onboarding ----------
async function renderPermissions(): Promise<void> {
  const box = $("ob-permissions");
  const st = await invoke<{ platform: string; accessibility?: boolean }>("permissions_status");
  box.innerHTML = "";
  if (st.platform === "macos") {
    box.innerHTML =
      rowPerm("Accessibility (insert text into other apps)", !!st.accessibility, "accessibility") +
      rowPerm("Microphone (asked automatically on first dictation)", true, "microphone");
  } else {
    box.innerHTML = `<p class="sub">Linux capabilities are listed under <b>Setup Check</b> — open it to see exactly what your session supports and how to enable the rest.</p>`;
  }
  box.querySelectorAll<HTMLButtonElement>("button[data-perm]").forEach((b) =>
    b.addEventListener("click", async () => {
      await invoke("permission_request", { kind: b.dataset.perm });
      setTimeout(() => void renderPermissions(), 1500);
    })
  );
}
function rowPerm(label: string, ok: boolean, kind: string): string {
  return (
    `<div class="row"><div class="info"><b>${label}</b></div>` +
    (ok ? `<span class="badge ok">ok</span>` : `<button data-perm="${kind}" class="primary">Grant…</button>`) +
    `</div>`
  );
}
$("ob-download").addEventListener("click", async () => {
  for (const id of RECOMMENDED) {
    const m = models.find((x) => x.spec.id === id);
    if (m && !m.installed) await download(id, document.querySelector<HTMLElement>(`[data-id="${id}"]`));
  }
  toast("Recommended models ready.");
});
$("ob-offline").addEventListener("click", async () => {
  settings.fully_offline = true;
  $<HTMLInputElement>("s-offline").checked = true;
  await save();
  toast("Fully Offline enabled — the app will never touch the network.");
});
$("ob-done").addEventListener("click", async () => {
  settings.onboarding_done = true;
  await save();
  $("onboarding").style.display = "none";
});

void load().catch((e) => toast("Failed to load settings: " + e));

export {};
