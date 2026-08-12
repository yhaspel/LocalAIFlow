//! Local AI Flow — Tauri v2 shell.
//!
//! A background menu-bar/tray agent: no Dock icon (macOS LSUIElement +
//! Accessory activation policy), no taskbar entry. The tray menu, global
//! hotkeys, HUD, and Settings window all drive one shared dictation/TTS
//! pipeline (laf-core) over channels.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // no-op on our targets; silences a tauri template lint

mod commands;
mod dynamic;
#[cfg(target_os = "macos")]
mod mac_accel;
mod tray;

use crossbeam_channel::unbounded;
use laf_core::metrics::LatencyTracker;
use laf_core::models::ModelManager;
use laf_core::pipeline::{self, Engines, PipelineCmd, PipelineHandle, StartSource};
use laf_core::settings::SettingsStore;
use laf_core::traits::{HotkeyBackend, SelectionReader, SpeechSynthesizer, TextInserter};
use laf_core::types::{Edge, HotkeyAction, Phase, UiEvent};
use laf_core::vad::EnergyVad;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

pub struct App {
    pub settings: Arc<SettingsStore>,
    pub mm: Arc<ModelManager>,
    pub metrics: Arc<LatencyTracker>,
    pub pipeline: PipelineHandle,
    /// Hotkey backend: rebind must run on the main thread on macOS, so
    /// commands dispatch through `run_on_main_thread`.
    pub hotkeys: Arc<Mutex<Box<dyn HotkeyBackend>>>,
}

fn main() {
    // `local-ai-flow --doctor` prints the environment report and exits —
    // usable over SSH / before any GUI exists.
    if std::env::args().any(|a| a == "--doctor") {
        let report = platform_doctor();
        println!("{}", report.to_terminal());
        std::process::exit(match report.worst() {
            laf_core::doctor::CheckStatus::Fail => 2,
            laf_core::doctor::CheckStatus::Warn => 1,
            laf_core::doctor::CheckStatus::Ok => 0,
        });
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,laf=debug".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("settings") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            commands::settings_get,
            commands::settings_set,
            commands::modes_list,
            commands::set_mode,
            commands::dictation_start,
            commands::dictation_stop,
            commands::dictation_cancel,
            commands::tts_read_selection,
            commands::tts_stop,
            commands::doctor_report,
            commands::models_list,
            commands::model_download,
            commands::model_delete,
            commands::model_verify,
            commands::voices_list,
            commands::input_devices,
            commands::latency_summary,
            commands::permissions_status,
            commands::permission_request,
            commands::app_info,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let settings = Arc::new(SettingsStore::load_default());
            let metrics = Arc::new(LatencyTracker::new());
            let mm = Arc::new(ModelManager::new());
            mm.set_offline(settings.get().fully_offline);
            // Bundled models (Fully-Offline installs): <resources>/models
            if let Ok(res) = app.path().resource_dir() {
                let bundled = res.join("models");
                if bundled.is_dir() {
                    mm.set_bundled_root(Some(bundled));
                }
            }

            let (ui_tx, ui_rx) = unbounded::<UiEvent>();
            let engines = build_engines(&settings, &mm);
            let pipeline = pipeline::spawn(engines, settings.clone(), metrics.clone(), ui_tx);

            // ---- global hotkeys (created on the main thread) ----
            let (hk_tx, hk_rx) = unbounded();
            let mut backend = platform_hotkeys(hk_tx);
            match backend.rebind(&settings.get().hotkeys) {
                Ok(warnings) => {
                    for w in warnings {
                        tracing::warn!("hotkeys: {w}");
                    }
                }
                Err(e) => tracing::error!("hotkeys unavailable: {e}"),
            }
            tracing::info!("hotkey backend: {}", backend.backend_name());

            let state = App {
                settings: settings.clone(),
                mm,
                metrics,
                pipeline: pipeline.clone(),
                hotkeys: Arc::new(Mutex::new(backend)),
            };
            app.manage(state);

            tray::build(app.handle())?;

            // ---- hotkey events → pipeline commands ----
            {
                let pipeline = pipeline.clone();
                std::thread::Builder::new().name("laf-hotkey-map".into()).spawn(move || {
                    for ev in hk_rx {
                        match (ev.action, ev.edge) {
                            (HotkeyAction::DictateToggle, Edge::Down) => {
                                pipeline.send(PipelineCmd::Start(StartSource::Toggle))
                            }
                            (HotkeyAction::DictatePushToTalk, Edge::Down) => {
                                pipeline.send(PipelineCmd::Start(StartSource::PushToTalk))
                            }
                            (HotkeyAction::DictatePushToTalk, Edge::Up) => {
                                pipeline.send(PipelineCmd::Stop(StartSource::PushToTalk))
                            }
                            (HotkeyAction::ReadSelection, Edge::Down) => {
                                pipeline.send(PipelineCmd::ReadSelection)
                            }
                            (HotkeyAction::StopSpeech, Edge::Down) => {
                                pipeline.send(PipelineCmd::StopSpeech)
                            }
                            _ => {}
                        }
                    }
                })?;
            }

            // ---- pipeline UI events → webviews + tray + HUD visibility ----
            {
                let handle = app.handle().clone();
                std::thread::Builder::new().name("laf-ui-forward".into()).spawn(move || {
                    for ev in ui_rx {
                        if let UiEvent::Phase { phase, .. } = &ev {
                            tray::set_phase(&handle, *phase);
                            update_hud_visibility(&handle, *phase);
                        }
                        // One event stream for every window; payloads are
                        // small and local.
                        let _ = handle.emit("laf://ui", &ev);
                    }
                })?;
            }

            // First run → open Settings (onboarding section shows there).
            if !settings.get().onboarding_done {
                if let Some(w) = app.get_webview_window("settings") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the settings window hides it — the agent lives in the tray.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "settings" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Local AI Flow");
}

/// Assemble the engine set for this OS. Every entry is a real, working
/// implementation on both macOS and Linux (see laf-engines + platform crates).
fn build_engines(settings: &Arc<SettingsStore>, mm: &Arc<ModelManager>) -> Engines {
    use laf_core::traits::AudioCapture as _;
    let mut capture = laf_engines::audio::CpalCapture::new();
    capture.select_device(settings.get().input_device.clone());

    let stt = Arc::new(dynamic::DynamicStt::new(settings.clone(), mm.clone()));
    let cleaner_llm: Option<Arc<dyn laf_core::traits::TextCleaner>> =
        Some(Arc::new(dynamic::DynamicCleaner::new(settings.clone(), mm.clone())));

    let mut tts_engines: Vec<Arc<dyn SpeechSynthesizer>> = Vec::new();
    #[cfg(feature = "tts-kokoro")]
    tts_engines.push(Arc::new(dynamic::DynamicKokoro::new(mm.clone())));
    tts_engines.push(Arc::new(laf_engines::tts_piper::PiperTts::new(mm.user_root().join("piper"))));
    tts_engines.push(Arc::new(laf_engines::tts_system::SystemTts::new()));

    let inserter: Arc<dyn TextInserter> = platform_inserter();
    let selection: Arc<dyn SelectionReader> = platform_selection();

    Engines {
        capture: Box::new(capture),
        vad: Box::new(EnergyVad::new()),
        stt,
        cleaner_det: Arc::new(laf_core::clean::DeterministicCleaner::new()),
        cleaner_llm,
        inserter,
        selection,
        tts_engines,
    }
}

// ---- per-OS constructors (both sides fully implemented) -------------------

#[cfg(target_os = "linux")]
fn platform_inserter() -> Arc<dyn TextInserter> {
    Arc::new(laf_platform_linux::LinuxInserter::new().expect("linux inserter init"))
}
#[cfg(target_os = "macos")]
fn platform_inserter() -> Arc<dyn TextInserter> {
    Arc::new(laf_platform_macos::MacInserter::new())
}

#[cfg(target_os = "linux")]
fn platform_selection() -> Arc<dyn SelectionReader> {
    Arc::new(laf_platform_linux::LinuxSelectionReader::new())
}
#[cfg(target_os = "macos")]
fn platform_selection() -> Arc<dyn SelectionReader> {
    Arc::new(laf_platform_macos::MacSelectionReader::new())
}

#[cfg(target_os = "linux")]
fn platform_hotkeys(
    tx: crossbeam_channel::Sender<laf_core::types::HotkeyEvent>,
) -> Box<dyn HotkeyBackend> {
    Box::new(laf_platform_linux::LinuxHotkeys::new(tx))
}
#[cfg(target_os = "macos")]
fn platform_hotkeys(
    tx: crossbeam_channel::Sender<laf_core::types::HotkeyEvent>,
) -> Box<dyn HotkeyBackend> {
    Box::new(laf_platform_macos::MacHotkeys::new(tx))
}

#[cfg(target_os = "linux")]
pub fn platform_doctor() -> laf_core::doctor::DoctorReport {
    laf_platform_linux::doctor()
}
#[cfg(target_os = "macos")]
pub fn platform_doctor() -> laf_core::doctor::DoctorReport {
    laf_platform_macos::doctor()
}

/// Show the HUD (bottom-center of the monitor) while listening/processing.
fn update_hud_visibility(app: &AppHandle, phase: Phase) {
    let Some(hud) = app.get_webview_window("hud") else { return };
    match phase {
        Phase::Listening | Phase::Processing | Phase::Inserting => {
            if let Ok(Some(monitor)) = hud.current_monitor().or_else(|_| hud.primary_monitor()) {
                let msize = monitor.size();
                let scale = monitor.scale_factor();
                let (w, h) = ((440.0 * scale) as i32, (132.0 * scale) as i32);
                let x = monitor.position().x + ((msize.width as i32 - w) / 2).max(0);
                let y = monitor.position().y + msize.height as i32 - h - (56.0 * scale) as i32;
                let _ = hud.set_position(tauri::PhysicalPosition::new(x, y));
            }
            let _ = hud.show();
        }
        Phase::Idle | Phase::Speaking => {
            let _ = hud.hide();
        }
    }
}
