// Local AI Flow — Settings app. Compiled from ../src/app.ts (types stripped).
"use strict";
(function () {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  const $ = (id) => document.getElementById(id);
  let settings = null;
  let models = [];
  const RECOMMENDED = [
    "whisper-large-v3-turbo-q5",
    "qwen2.5-3b-instruct-q4",
    "kokoro-v1-q8",
    "kokoro-tokenizer",
    "kokoro-voice-af-heart",
  ];

  // ---------- tabs ----------
  document.querySelectorAll("nav button[data-tab]").forEach((b) =>
    b.addEventListener("click", () => showTab(b.dataset.tab))
  );
  function showTab(name) {
    document.querySelectorAll("nav button[data-tab]").forEach((b) =>
      b.classList.toggle("active", b.dataset.tab === name)
    );
    document.querySelectorAll("section.tab").forEach((s) =>
      s.classList.toggle("visible", s.id === "tab-" + name)
    );
    if (name === "doctor") refreshDoctor();
    if (name === "debug") refreshLatency();
  }
  listen("laf://open-doctor", () => showTab("doctor"));

  function toast(msg, ms = 3200) {
    const t = $("toast");
    t.textContent = msg;
    t.classList.add("show");
    setTimeout(() => t.classList.remove("show"), ms);
  }

  // ---------- load ----------
  async function load() {
    settings = await invoke("settings_get");
    const modes = await invoke("modes_list");
    $("s-mode").innerHTML = modes
      .map((m) => `<option value="${m.id}">${m.label}</option>`)
      .join("");
    $("s-mode").value = settings.default_mode;
    $("s-language").value = settings.language;
    $("s-cleaner").value = settings.cleaner.tier;
    $("s-incremental").checked = settings.insert_incremental;
    $("s-hud").checked = settings.hud_enabled;
    $("s-autostart").checked = settings.launch_at_login;
    $("s-offline").checked = settings.fully_offline;
    $("s-unload").value = settings.model_idle_unload_secs;

    const devices = await invoke("input_devices");
    $("s-device").innerHTML =
      `<option value="">System default</option>` +
      devices.map((d) => `<option value="${esc(d)}">${esc(d)}</option>`).join("");
    $("s-device").value = settings.input_device || "";

    $("hk-toggle").value = settings.hotkeys.dictate_toggle;
    $("hk-ptt").value = settings.hotkeys.dictate_ptt;
    $("hk-ptt-on").checked = settings.hotkeys.ptt_enabled;
    $("hk-read").value = settings.hotkeys.read_selection;
    $("hk-stop").value = settings.hotkeys.stop_speech;

    $("tts-engine").value = settings.tts.engine;
    $("tts-rate").value = settings.tts.rate;
    $("tts-rate-val").textContent = Number(settings.tts.rate).toFixed(2) + "×";

    renderDictionary();
    await refreshModels();
    await refreshVoices();

    $("onboarding").style.display = settings.onboarding_done ? "none" : "block";
    if (!settings.onboarding_done) renderPermissions();

    const info = await invoke("app_info");
    $("appinfo").textContent =
      `v${info.version} · models: ${info.models_dir} · config: ${info.config_dir}` +
      (info.offline_build ? " · OFFLINE BUILD (no network code compiled in)" : "");
  }

  function esc(s) {
    return String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
  }

  // ---------- collect + save ----------
  function collect() {
    settings.default_mode = $("s-mode").value;
    settings.language = $("s-language").value;
    settings.input_device = $("s-device").value || null;
    settings.cleaner.tier = $("s-cleaner").value;
    settings.stt.model_id = $("s-sttmodel").value || settings.stt.model_id;
    settings.cleaner.model_id = $("s-llmmodel").value || settings.cleaner.model_id;
    settings.insert_incremental = $("s-incremental").checked;
    settings.hud_enabled = $("s-hud").checked;
    settings.launch_at_login = $("s-autostart").checked;
    settings.fully_offline = $("s-offline").checked;
    settings.model_idle_unload_secs = Number($("s-unload").value) || 0;
    settings.hotkeys.dictate_toggle = $("hk-toggle").value;
    settings.hotkeys.dictate_ptt = $("hk-ptt").value;
    settings.hotkeys.ptt_enabled = $("hk-ptt-on").checked;
    settings.hotkeys.read_selection = $("hk-read").value;
    settings.hotkeys.stop_speech = $("hk-stop").value;
    settings.tts.engine = $("tts-engine").value;
    settings.tts.voice_id = $("tts-voice").value || settings.tts.voice_id;
    settings.tts.rate = Number($("tts-rate").value);
    settings.dictionary = readDictionary();
  }

  async function save() {
    collect();
    try {
      const warnings = await invoke("settings_set", { settings });
      $("hk-warnings").textContent = warnings.join("\n");
      $("save-status").textContent = "Saved ✓";
      setTimeout(() => ($("save-status").textContent = ""), 2000);
      if (warnings.length) toast("Saved with warnings:\n" + warnings.join("\n"), 5000);
    } catch (e) {
      toast("Save failed: " + e);
    }
  }
  $("save").addEventListener("click", save);
  $("dictate-now").addEventListener("click", () => invoke("dictation_start"));
  $("tts-rate").addEventListener("input", () => {
    $("tts-rate-val").textContent = Number($("tts-rate").value).toFixed(2) + "×";
  });
  $("tts-test").addEventListener("click", async () => {
    collect();
    await invoke("settings_set", { settings });
    invoke("tts_read_selection");
  });

  // ---------- hotkey capture ----------
  document.querySelectorAll(".hotkey-input").forEach((input) => {
    input.addEventListener("click", () => {
      input.classList.add("recording");
      input.value = "press keys…";
      const onKey = (e) => {
        e.preventDefault();
        if (["ControlLeft","ControlRight","AltLeft","AltRight","ShiftLeft","ShiftRight","MetaLeft","MetaRight"].includes(e.code)) return;
        const parts = [];
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
  function renderDictionary() {
    const tbody = $("dict-table").querySelector("tbody");
    tbody.innerHTML = "";
    for (const e of settings.dictionary) addDictRow(e.from, e.to, e.match_case);
  }
  function addDictRow(from = "", to = "", matchCase = false) {
    const tbody = $("dict-table").querySelector("tbody");
    const tr = document.createElement("tr");
    tr.innerHTML =
      `<td><input class="d-from" value="${esc(from)}" placeholder="cuber netties"/></td>` +
      `<td><input class="d-to" value="${esc(to)}" placeholder="Kubernetes"/></td>` +
      `<td><input type="checkbox" class="d-case" ${matchCase ? "checked" : ""}/></td>` +
      `<td><button class="danger d-del">✕</button></td>`;
    tr.querySelector(".d-del").addEventListener("click", () => tr.remove());
    tbody.appendChild(tr);
  }
  $("dict-add").addEventListener("click", () => addDictRow());
  function readDictionary() {
    return Array.from($("dict-table").querySelectorAll("tbody tr"))
      .map((tr) => ({
        from: tr.querySelector(".d-from").value.trim(),
        to: tr.querySelector(".d-to").value.trim(),
        match_case: tr.querySelector(".d-case").checked,
      }))
      .filter((e) => e.from && e.to);
  }

  // ---------- models ----------
  async function refreshModels() {
    models = await invoke("models_list");
    const bykind = (k) => models.filter((m) => m.spec.kind === k);
    $("s-sttmodel").innerHTML = bykind("stt")
      .map((m) => `<option value="${m.spec.id}" ${m.installed ? "" : "disabled"}>${esc(m.spec.label)}${m.installed ? "" : " (not installed)"}</option>`)
      .join("");
    $("s-sttmodel").value = settings.stt.model_id;
    $("s-llmmodel").innerHTML = bykind("cleaner")
      .map((m) => `<option value="${m.spec.id}" ${m.installed ? "" : "disabled"}>${esc(m.spec.label)}${m.installed ? "" : " (not installed)"}</option>`)
      .join("");
    $("s-llmmodel").value = settings.cleaner.model_id;

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
      const dl = row.querySelector(".m-dl");
      if (dl) dl.addEventListener("click", () => download(m.spec.id, row));
      const del = row.querySelector(".m-del");
      if (del)
        del.addEventListener("click", async () => {
          await invoke("model_delete", { id: m.spec.id });
          refreshModels();
        });
      const ver = row.querySelector(".m-verify");
      if (ver)
        ver.addEventListener("click", async () => {
          ver.textContent = "…";
          const ok = await invoke("model_verify", { id: m.spec.id });
          ver.textContent = ok ? "Verified ✓" : "CORRUPT ✕";
          if (!ok) toast("Checksum mismatch — delete and re-download this model.");
        });
      list.appendChild(row);
    }
  }

  async function download(id, row) {
    const bar = row ? row.querySelector(".progress") : null;
    if (bar) bar.style.display = "block";
    try {
      await invoke("model_download", { id });
    } catch (e) {
      toast("Download failed: " + e, 6000);
    }
    await refreshModels();
    await refreshVoices();
  }

  listen("laf://ui", (event) => {
    const ev = event.payload;
    if (ev.event === "model_download") {
      const row = document.querySelector(`[data-id="${ev.model_id}"]`);
      if (row) {
        const bar = row.querySelector(".progress");
        bar.style.display = "block";
        const pct = ev.total > 0 ? (100 * ev.downloaded) / ev.total : 0;
        bar.firstElementChild.style.width = pct.toFixed(1) + "%";
      }
    } else if (ev.event === "pipeline_error") {
      toast(ev.message, 5000);
    } else if (ev.event === "inserted") {
      $("save-status").textContent = `inserted ${ev.report.chars} chars via ${ev.report.method.method}`;
      setTimeout(() => ($("save-status").textContent = ""), 4000);
    }
    logEvent(ev);
  });

  // ---------- voices ----------
  async function refreshVoices() {
    const voices = await invoke("voices_list");
    const engine = $("tts-engine").value;
    const filtered = voices.filter((v) => v.engine === engine);
    const use = filtered.length ? filtered : voices;
    $("tts-voice").innerHTML = use
      .map((v) => `<option value="${esc(v.id)}">${esc(v.label)} — ${esc(v.engine)}</option>`)
      .join("");
    $("tts-voice").value = settings.tts.voice_id;
  }
  $("tts-engine").addEventListener("change", refreshVoices);

  // ---------- doctor ----------
  async function refreshDoctor() {
    const report = await invoke("doctor_report");
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
  $("doctor-refresh").addEventListener("click", refreshDoctor);

  // ---------- debug ----------
  async function refreshLatency() {
    const stats = await invoke("latency_summary");
    const tbody = $("lat-table").querySelector("tbody");
    tbody.innerHTML = stats
      .map(
        (s) =>
          `<tr><td>${esc(s.stage)}</td><td>${s.count}</td><td>${s.last_ms} ms</td><td>${s.p50_ms} ms</td><td>${s.p95_ms} ms</td></tr>`
      )
      .join("");
  }
  $("lat-refresh").addEventListener("click", refreshLatency);
  const evlog = [];
  function logEvent(ev) {
    if (ev.event === "level") return;
    evlog.push(`${new Date().toLocaleTimeString()} ${JSON.stringify(ev)}`);
    if (evlog.length > 120) evlog.shift();
    const el = $("evlog");
    el.textContent = evlog.join("\n");
    el.scrollTop = el.scrollHeight;
  }

  // ---------- onboarding ----------
  async function renderPermissions() {
    const box = $("ob-permissions");
    const st = await invoke("permissions_status");
    box.innerHTML = "";
    if (st.platform === "macos") {
      box.innerHTML =
        row_perm("Accessibility (insert text into other apps)", st.accessibility, "accessibility") +
        row_perm("Microphone (asked automatically on first dictation)", true, "microphone");
    } else {
      box.innerHTML = `<p class="sub">Linux capabilities are listed under <b>Setup Check</b> — open it to see exactly what your session supports and how to enable the rest.</p>`;
    }
    box.querySelectorAll("button[data-perm]").forEach((b) =>
      b.addEventListener("click", async () => {
        await invoke("permission_request", { kind: b.dataset.perm });
        setTimeout(renderPermissions, 1500);
      })
    );
  }
  function row_perm(label, ok, kind) {
    return (
      `<div class="row"><div class="info"><b>${label}</b></div>` +
      (ok
        ? `<span class="badge ok">ok</span>`
        : `<button data-perm="${kind}" class="primary">Grant…</button>`) +
      `</div>`
    );
  }
  $("ob-download").addEventListener("click", async () => {
    for (const id of RECOMMENDED) {
      const m = models.find((x) => x.spec.id === id);
      if (m && !m.installed) await download(id, document.querySelector(`[data-id="${id}"]`));
    }
    toast("Recommended models ready.");
  });
  $("ob-offline").addEventListener("click", async () => {
    settings.fully_offline = true;
    $("s-offline").checked = true;
    await save();
    toast("Fully Offline enabled — the app will never touch the network.");
  });
  $("ob-done").addEventListener("click", async () => {
    settings.onboarding_done = true;
    await save();
    $("onboarding").style.display = "none";
  });

  load().catch((e) => toast("Failed to load settings: " + e));
})();
