//! Tray / menu-bar icon and menu. The icon doubles as the status indicator
//! (idle / listening / processing / speaking).

use laf_core::pipeline::{PipelineCmd, StartSource};
use laf_core::types::{Mode, Phase};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

const TRAY_ID: &str = "laf-tray";

const ICON_IDLE: &[u8] = include_bytes!("../icons/tray-idle.png");
const ICON_LISTENING: &[u8] = include_bytes!("../icons/tray-listening.png");
const ICON_PROCESSING: &[u8] = include_bytes!("../icons/tray-processing.png");

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = build_menu(app, Mode::Auto)?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(ICON_IDLE)?)
        .icon_as_template(true) // adapts to light/dark menu bar on macOS
        .tooltip("Local AI Flow — idle")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| on_menu_event(app, event.id().as_ref()))
        .build(app)?;
    Ok(())
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, current_mode: Mode) -> tauri::Result<Menu<R>> {
    let start_stop =
        MenuItem::with_id(app, "toggle", "Start / Stop Dictation", true, None::<&str>)?;
    let read = MenuItem::with_id(app, "read", "Read Selection Aloud", true, None::<&str>)?;
    let stop_speech = MenuItem::with_id(app, "stopspeech", "Stop Speaking", true, None::<&str>)?;

    let mode_items: Vec<CheckMenuItem<R>> = Mode::ALL
        .iter()
        .map(|m| {
            CheckMenuItem::with_id(
                app,
                format!("mode:{}", m.id()),
                m.label(),
                true,
                *m == current_mode,
                None::<&str>,
            )
        })
        .collect::<Result<_, _>>()?;
    let mode_refs: Vec<&dyn tauri::menu::IsMenuItem<R>> =
        mode_items.iter().map(|i| i as &dyn tauri::menu::IsMenuItem<R>).collect();
    let modes = Submenu::with_id_and_items(app, "modes", "Mode", true, &mode_refs)?;

    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let doctor = MenuItem::with_id(app, "doctor", "Setup Check (Doctor)…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Local AI Flow", true, None::<&str>)?;

    Menu::with_items(
        app,
        &[
            &start_stop,
            &read,
            &stop_speech,
            &PredefinedMenuItem::separator(app)?,
            &modes,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &doctor,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )
}

fn on_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let Some(state) = app.try_state::<crate::App>() else { return };
    match id {
        "toggle" => state.pipeline.send(PipelineCmd::Start(StartSource::Toggle)),
        "read" => state.pipeline.send(PipelineCmd::ReadSelection),
        "stopspeech" => state.pipeline.send(PipelineCmd::StopSpeech),
        "settings" => {
            if let Some(w) = app.get_webview_window("settings") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }
        "doctor" => {
            if let Some(w) = app.get_webview_window("settings") {
                let _ = w.show();
                let _ = w.set_focus();
                let _ = tauri::Emitter::emit(app, "laf://open-doctor", ());
            }
        }
        "quit" => {
            state.pipeline.send(PipelineCmd::Shutdown);
            app.exit(0);
        }
        other => {
            if let Some(mode_id) = other.strip_prefix("mode:") {
                if let Some(mode) = Mode::from_id(mode_id) {
                    state.pipeline.send(PipelineCmd::SetMode(mode));
                    sync_mode(app, mode);
                }
            }
        }
    }
}

/// Reflect the current mode in the checkable submenu (manual radio group).
pub fn sync_mode<R: Runtime>(app: &AppHandle<R>, mode: Mode) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    // Rebuilding the menu is the simplest reliable cross-platform way to
    // update check states with the tray menu API.
    if let Ok(menu) = build_menu(app, mode) {
        let _ = tray.set_menu(Some(menu));
    }
}

/// Tray icon mirrors the pipeline phase.
pub fn set_phase<R: Runtime>(app: &AppHandle<R>, phase: Phase) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let (bytes, tip) = match phase {
        Phase::Idle => (ICON_IDLE, "Local AI Flow — idle"),
        Phase::Listening => (ICON_LISTENING, "Local AI Flow — listening"),
        Phase::Processing | Phase::Inserting => (ICON_PROCESSING, "Local AI Flow — processing"),
        Phase::Speaking => (ICON_PROCESSING, "Local AI Flow — speaking"),
    };
    if let Ok(img) = Image::from_bytes(bytes) {
        let _ = tray.set_icon(Some(img));
        let _ = tray.set_tooltip(Some(tip));
    }
}
