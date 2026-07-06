//! Tauri IPC commands for the Settings/HUD webviews. All data is local; the
//! only command that can touch the network is `model_download` (explicit
//! user action, pinned URLs, disabled in Fully-Offline mode and absent in
//! offline builds).

use crate::App;
use laf_core::pipeline::{PipelineCmd, StartSource};
use laf_core::settings::Settings;
use laf_core::types::Mode;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};

#[tauri::command]
pub fn settings_get(state: State<'_, App>) -> Settings {
    state.settings.get()
}

#[tauri::command]
pub fn settings_set(app: AppHandle, state: State<'_, App>, settings: Settings) -> Result<Vec<String>, String> {
    let previous = state.settings.get();
    let new = state.settings.replace(settings);
    state.mm.set_offline(new.fully_offline);
    state.pipeline.send(PipelineCmd::SettingsChanged);

    // Launch-at-login via the autostart plugin (LaunchAgent on macOS,
    // XDG autostart .desktop on Linux).
    if previous.launch_at_login != new.launch_at_login {
        use tauri_plugin_autostart::ManagerExt;
        let autolaunch = app.autolaunch();
        let r = if new.launch_at_login { autolaunch.enable() } else { autolaunch.disable() };
        if let Err(e) = r {
            tracing::warn!("autostart update failed: {e}");
        }
    }

    // Rebind hotkeys on the main thread (macOS requirement).
    let mut warnings: Vec<String> = Vec::new();
    if previous.hotkeys != new.hotkeys {
        let hotkeys = state.hotkeys.clone();
        let bindings = new.hotkeys.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = app.run_on_main_thread(move || {
            let result = hotkeys
                .lock()
                .expect("hotkeys lock")
                .rebind(&bindings)
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(w)) => warnings = w,
            Ok(Err(e)) => warnings.push(format!("hotkeys: {e}")),
            Err(_) => warnings.push("hotkeys: rebind timed out".into()),
        }
    }
    crate::tray::sync_mode(&app, state.pipeline.current_mode());
    Ok(warnings)
}

#[tauri::command]
pub fn modes_list() -> Vec<serde_json::Value> {
    Mode::ALL.iter().map(|m| json!({ "id": m.id(), "label": m.label() })).collect()
}

#[tauri::command]
pub fn set_mode(app: AppHandle, state: State<'_, App>, mode: String) -> Result<(), String> {
    let mode = Mode::from_id(&mode).ok_or_else(|| format!("unknown mode '{mode}'"))?;
    state.pipeline.send(PipelineCmd::SetMode(mode));
    crate::tray::sync_mode(&app, mode);
    Ok(())
}

#[tauri::command]
pub fn dictation_start(state: State<'_, App>) {
    state.pipeline.send(PipelineCmd::Start(StartSource::Toggle));
}

#[tauri::command]
pub fn dictation_stop(state: State<'_, App>) {
    state.pipeline.send(PipelineCmd::Stop(StartSource::Toggle));
}

#[tauri::command]
pub fn dictation_cancel(state: State<'_, App>) {
    state.pipeline.send(PipelineCmd::Cancel);
}

#[tauri::command]
pub fn tts_read_selection(state: State<'_, App>) {
    state.pipeline.send(PipelineCmd::ReadSelection);
}

#[tauri::command]
pub fn tts_stop(state: State<'_, App>) {
    state.pipeline.send(PipelineCmd::StopSpeech);
}

#[tauri::command]
pub fn doctor_report() -> laf_core::doctor::DoctorReport {
    crate::platform_doctor()
}

#[tauri::command]
pub fn models_list(state: State<'_, App>) -> Vec<laf_core::models::ModelStatus> {
    state.mm.list()
}

/// Explicit, user-triggered model download — the app's single network path.
#[cfg(feature = "online")]
#[tauri::command]
pub async fn model_download(app: AppHandle, id: String) -> Result<(), String> {
    let state = app.state::<App>();
    if state.settings.get().fully_offline {
        return Err("Fully Offline mode is enabled — downloads are disabled.".into());
    }
    let mm = state.mm.clone();
    let emitter = app.clone();
    let model_id = id.clone();
    let last_emit = std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let result = mm
        .download(&id, move |done, total| {
            let mut guard = last_emit.lock().expect("emit throttle");
            if guard.elapsed().as_millis() >= 150 || done == total {
                *guard = std::time::Instant::now();
                let _ = emitter.emit(
                    "laf://ui",
                    laf_core::types::UiEvent::ModelDownload {
                        model_id: model_id.clone(),
                        downloaded: done,
                        total,
                        done: false,
                        error: None,
                    },
                );
            }
        })
        .await;
    let ev = match &result {
        Ok(_) => laf_core::types::UiEvent::ModelDownload {
            model_id: id.clone(),
            downloaded: 0,
            total: 0,
            done: true,
            error: None,
        },
        Err(e) => laf_core::types::UiEvent::ModelDownload {
            model_id: id.clone(),
            downloaded: 0,
            total: 0,
            done: true,
            error: Some(e.to_string()),
        },
    };
    let _ = app.emit("laf://ui", ev);
    result.map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(not(feature = "online"))]
#[tauri::command]
pub async fn model_download(_app: AppHandle, _id: String) -> Result<(), String> {
    Err("this build has no network code (offline build) — place model files in the models folder manually".into())
}

#[tauri::command]
pub fn model_delete(state: State<'_, App>, id: String) -> Result<(), String> {
    state.mm.delete(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn model_verify(state: State<'_, App>, id: String) -> Result<bool, String> {
    state.mm.verify(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn voices_list(state: State<'_, App>) -> Vec<laf_core::types::VoiceInfo> {
    // Aggregate voices from every TTS engine the pipeline knows about is not
    // directly reachable here; rebuild lightweight engine views instead.
    let mut out: Vec<laf_core::types::VoiceInfo> = Vec::new();
    #[cfg(feature = "tts-kokoro")]
    {
        use laf_core::traits::SpeechSynthesizer as _;
        out.extend(crate::dynamic::DynamicKokoro::new(state.mm.clone()).voices());
    }
    {
        use laf_core::traits::SpeechSynthesizer as _;
        out.extend(
            laf_engines::tts_piper::PiperTts::new(state.mm.user_root().join("piper")).voices(),
        );
        out.extend(laf_engines::tts_system::SystemTts::new().voices());
    }
    out
}

#[tauri::command]
pub fn input_devices() -> Vec<String> {
    use laf_core::traits::AudioCapture as _;
    laf_engines::audio::CpalCapture::new().list_devices()
}

#[tauri::command]
pub fn latency_summary(state: State<'_, App>) -> Vec<laf_core::metrics::StageStats> {
    state.metrics.summary()
}

#[tauri::command]
pub fn permissions_status() -> serde_json::Value {
    #[cfg(target_os = "macos")]
    {
        json!({
            "platform": "macos",
            "accessibility": laf_platform_macos::permissions::accessibility_trusted(),
            "input_monitoring": laf_platform_macos::permissions::input_monitoring_granted(),
        })
    }
    #[cfg(target_os = "linux")]
    {
        json!({ "platform": "linux" }) // the doctor covers Linux capabilities
    }
}

#[tauri::command]
pub fn permission_request(kind: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use laf_platform_macos::permissions::{self, Pane};
        match kind.as_str() {
            "accessibility" => {
                permissions::request_accessibility(true);
                permissions::open_settings_pane(Pane::Accessibility);
            }
            "microphone" => permissions::open_settings_pane(Pane::Microphone),
            "input_monitoring" => {
                permissions::request_input_monitoring();
                permissions::open_settings_pane(Pane::InputMonitoring);
            }
            other => return Err(format!("unknown permission '{other}'")),
        }
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        let _ = kind;
        Err("on Linux, run the Setup check (doctor) for exact instructions".into())
    }
}

#[tauri::command]
pub fn app_info(state: State<'_, App>) -> serde_json::Value {
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "models_dir": state.mm.user_root().display().to_string(),
        "config_dir": laf_core::settings::config_dir().display().to_string(),
        "offline_build": cfg!(not(feature = "online")),
    })
}
