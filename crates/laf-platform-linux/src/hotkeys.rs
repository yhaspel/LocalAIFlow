//! Linux global hotkeys, selected at runtime:
//!
//! * **X11**: `global-hotkey` crate (XGrabKey under the hood) — reliable
//!   press *and* release events, so push-to-talk works natively.
//! * **Wayland, preferred**: the `org.freedesktop.portal.GlobalShortcuts`
//!   XDG portal (KDE ships it; GNOME since 48). Its Activated/Deactivated
//!   signals map directly to press/release → push-to-talk works. The desktop
//!   may show a shortcuts dialog on first bind; users can re-map there.
//! * **Wayland, fallback**: raw evdev on /dev/input/event* (requires the
//!   user to be in the `input` group). Compositor-independent and gives
//!   exact press/release edges.
//!
//! `rebind` tears down the current backend and builds the best available one
//! for the new bindings, returning human-readable warnings for anything that
//! could not be grabbed.

use crate::session::{detect_session, evdev_readable, SessionType};
use crossbeam_channel::Sender;
use laf_core::hotkeys::ParsedBinding;
use laf_core::settings::HotkeyBindings;
use laf_core::traits::HotkeyBackend;
use laf_core::types::{Edge, EngineError, EngineResult, HotkeyAction, HotkeyEvent};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct LinuxHotkeys {
    session: SessionType,
    tx: Sender<HotkeyEvent>,
    backend: Backend,
}

/// The payloads are RAII guards: their `Drop` impls unregister grabs / stop
/// portal sessions / join evdev threads. They're held, not read.
#[allow(dead_code)]
enum Backend {
    None,
    X11(X11Backend),
    Portal(PortalBackend),
    Evdev(EvdevBackend),
}

impl LinuxHotkeys {
    pub fn new(tx: Sender<HotkeyEvent>) -> Self {
        Self { session: detect_session(), tx, backend: Backend::None }
    }
}

impl HotkeyBackend for LinuxHotkeys {
    fn rebind(&mut self, bindings: &HotkeyBindings) -> EngineResult<Vec<String>> {
        // Tear down whatever is active.
        self.backend = Backend::None;

        let mut warnings = Vec::new();
        let parsed = parse_bindings(bindings, &mut warnings);
        if parsed.is_empty() {
            return Ok(warnings);
        }

        match self.session {
            SessionType::X11 => match X11Backend::start(&parsed, self.tx.clone(), &mut warnings) {
                Ok(b) => self.backend = Backend::X11(b),
                Err(e) => warnings.push(format!("X11 hotkeys unavailable: {e}")),
            },
            SessionType::Wayland | SessionType::Unknown => {
                match PortalBackend::start(&parsed, self.tx.clone()) {
                    Ok(b) => {
                        self.backend = Backend::Portal(b);
                        warnings.push(
                            "Using the GlobalShortcuts portal — your desktop may show a \
                             confirmation dialog once."
                                .into(),
                        );
                    }
                    Err(portal_err) => {
                        if evdev_readable() {
                            match EvdevBackend::start(&parsed, self.tx.clone()) {
                                Ok(b) => {
                                    self.backend = Backend::Evdev(b);
                                    warnings.push(format!(
                                        "GlobalShortcuts portal unavailable ({portal_err}); \
                                         using raw evdev input instead."
                                    ));
                                }
                                Err(e) => {
                                    return Err(EngineError::Hotkey(format!(
                                        "no hotkey backend available: portal: {portal_err}; evdev: {e}"
                                    )))
                                }
                            }
                        } else {
                            return Err(EngineError::Hotkey(format!(
                                "no hotkey backend: portal unavailable ({portal_err}) and \
                                 /dev/input is not readable — add your user to the 'input' \
                                 group: sudo usermod -aG input $USER (then re-login)"
                            )));
                        }
                    }
                }
            }
        }
        Ok(warnings)
    }

    fn backend_name(&self) -> &'static str {
        match self.backend {
            Backend::None => "none",
            Backend::X11(_) => "x11-grab",
            Backend::Portal(_) => "xdg-portal",
            Backend::Evdev(_) => "evdev",
        }
    }
}

pub(crate) fn parse_bindings(
    bindings: &HotkeyBindings,
    warnings: &mut Vec<String>,
) -> Vec<(HotkeyAction, ParsedBinding)> {
    let mut out = Vec::new();
    let mut add = |action: HotkeyAction, s: &str| match ParsedBinding::parse(s) {
        Ok(b) => out.push((action, b)),
        Err(e) => warnings.push(format!("{action:?}: {e}")),
    };
    add(HotkeyAction::DictateToggle, &bindings.dictate_toggle);
    if bindings.ptt_enabled {
        add(HotkeyAction::DictatePushToTalk, &bindings.dictate_ptt);
    }
    add(HotkeyAction::ReadSelection, &bindings.read_selection);
    add(HotkeyAction::StopSpeech, &bindings.stop_speech);
    out
}

// ---------------------------------------------------------------------------
// X11: global-hotkey crate (XGrabKey)
// ---------------------------------------------------------------------------

struct X11Backend {
    manager: global_hotkey::GlobalHotKeyManager,
    registered: Vec<global_hotkey::hotkey::HotKey>,
}

impl X11Backend {
    fn start(
        parsed: &[(HotkeyAction, ParsedBinding)],
        tx: Sender<HotkeyEvent>,
        warnings: &mut Vec<String>,
    ) -> EngineResult<Self> {
        use global_hotkey::hotkey::{Code, HotKey, Modifiers};
        use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
        use std::str::FromStr;

        let manager = GlobalHotKeyManager::new()
            .map_err(|e| EngineError::Hotkey(format!("hotkey manager: {e}")))?;
        let mut registered = Vec::new();
        let id_map: Arc<Mutex<HashMap<u32, HotkeyAction>>> = Arc::new(Mutex::new(HashMap::new()));

        for (action, b) in parsed {
            let mut mods = Modifiers::empty();
            if b.ctrl {
                mods |= Modifiers::CONTROL;
            }
            if b.alt {
                mods |= Modifiers::ALT;
            }
            if b.shift {
                mods |= Modifiers::SHIFT;
            }
            if b.meta {
                mods |= Modifiers::SUPER;
            }
            let code = match Code::from_str(&b.code) {
                Ok(c) => c,
                Err(_) => {
                    warnings.push(format!("{action:?}: unsupported key code '{}'", b.code));
                    continue;
                }
            };
            let hotkey = HotKey::new(Some(mods), code);
            match manager.register(hotkey) {
                Ok(()) => {
                    id_map.lock().expect("id map").insert(hotkey.id(), *action);
                    registered.push(hotkey);
                }
                Err(e) => warnings.push(format!("{action:?}: grab failed ({e})")),
            }
        }

        let map = id_map.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |ev: GlobalHotKeyEvent| {
            let action = map.lock().expect("id map").get(&ev.id()).copied();
            if let Some(action) = action {
                let edge =
                    if ev.state() == HotKeyState::Pressed { Edge::Down } else { Edge::Up };
                let _ = tx.send(HotkeyEvent { action, edge });
            }
        }));
        Ok(Self { manager, registered })
    }
}

impl Drop for X11Backend {
    fn drop(&mut self) {
        for hk in self.registered.drain(..) {
            let _ = self.manager.unregister(hk);
        }
        global_hotkey::GlobalHotKeyEvent::set_event_handler(
            None::<fn(global_hotkey::GlobalHotKeyEvent)>,
        );
    }
}

// ---------------------------------------------------------------------------
// Wayland: org.freedesktop.portal.GlobalShortcuts via ashpd
// ---------------------------------------------------------------------------

struct PortalBackend {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PortalBackend {
    fn start(
        parsed: &[(HotkeyAction, ParsedBinding)],
        tx: Sender<HotkeyEvent>,
    ) -> EngineResult<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let shortcuts: Vec<(HotkeyAction, String, String)> = parsed
            .iter()
            .map(|(a, b)| (*a, portal_id(*a).to_string(), b.portal_trigger()))
            .collect();

        // Probe synchronously so callers get a real error if the portal is
        // missing (then they can fall back to evdev).
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| EngineError::Hotkey(format!("tokio: {e}")))?;
        let available = rt.block_on(crate::session::portal_global_shortcuts_version());
        if available.is_none() {
            return Err(EngineError::Hotkey("portal not present".into()));
        }

        let thread = std::thread::Builder::new()
            .name("laf-portal-hotkeys".into())
            .spawn(move || {
                rt.block_on(async move {
                    if let Err(e) = portal_loop(shortcuts, tx, stop2).await {
                        tracing::warn!("GlobalShortcuts portal loop ended: {e}");
                    }
                });
            })
            .map_err(|e| EngineError::Hotkey(format!("spawn portal thread: {e}")))?;
        Ok(Self { stop, thread: Some(thread) })
    }
}

impl Drop for PortalBackend {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // The loop polls the stop flag between stream items with a timeout,
        // so the thread exits promptly.
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn portal_id(action: HotkeyAction) -> &'static str {
    match action {
        HotkeyAction::DictateToggle => "dictate-toggle",
        HotkeyAction::DictatePushToTalk => "dictate-ptt",
        HotkeyAction::ReadSelection => "read-selection",
        HotkeyAction::StopSpeech => "stop-speech",
    }
}

fn action_for_portal_id(id: &str) -> Option<HotkeyAction> {
    match id {
        "dictate-toggle" => Some(HotkeyAction::DictateToggle),
        "dictate-ptt" => Some(HotkeyAction::DictatePushToTalk),
        "read-selection" => Some(HotkeyAction::ReadSelection),
        "stop-speech" => Some(HotkeyAction::StopSpeech),
        _ => None,
    }
}

async fn portal_loop(
    shortcuts: Vec<(HotkeyAction, String, String)>,
    tx: Sender<HotkeyEvent>,
    stop: Arc<AtomicBool>,
) -> ashpd::Result<()> {
    use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
    use ashpd::desktop::CreateSessionOptions;
    use futures_util::StreamExt;

    let gs = GlobalShortcuts::new().await?;
    let session = gs.create_session(CreateSessionOptions::default()).await?;

    let new_shortcuts: Vec<NewShortcut> = shortcuts
        .iter()
        .map(|(action, id, trigger)| {
            NewShortcut::new(id.clone(), describe(*action)).preferred_trigger(trigger.as_str())
        })
        .collect();
    let request = gs
        .bind_shortcuts(&session, &new_shortcuts, None, BindShortcutsOptions::default())
        .await?;
    // Wait for the user/portal to confirm the binding.
    let _ = request.response();

    let mut activated = gs.receive_activated().await?;
    let mut deactivated = gs.receive_deactivated().await?;

    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        tokio::select! {
            ev = activated.next() => {
                if let Some(ev) = ev {
                    if let Some(action) = action_for_portal_id(ev.shortcut_id()) {
                        let _ = tx.send(HotkeyEvent { action, edge: Edge::Down });
                    }
                } else { break; }
            }
            ev = deactivated.next() => {
                if let Some(ev) = ev {
                    if let Some(action) = action_for_portal_id(ev.shortcut_id()) {
                        let _ = tx.send(HotkeyEvent { action, edge: Edge::Up });
                    }
                } else { break; }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(300)) => {}
        }
    }
    let _ = session.close().await;
    Ok(())
}

fn describe(action: HotkeyAction) -> &'static str {
    match action {
        HotkeyAction::DictateToggle => "Local AI Flow: toggle dictation",
        HotkeyAction::DictatePushToTalk => "Local AI Flow: push-to-talk dictation",
        HotkeyAction::ReadSelection => "Local AI Flow: read selection aloud",
        HotkeyAction::StopSpeech => "Local AI Flow: stop speaking",
    }
}

// ---------------------------------------------------------------------------
// Wayland fallback: raw evdev
// ---------------------------------------------------------------------------

struct EvdevBackend {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl EvdevBackend {
    fn start(
        parsed: &[(HotkeyAction, ParsedBinding)],
        tx: Sender<HotkeyEvent>,
    ) -> EngineResult<Self> {
        let combos: Vec<(HotkeyAction, EvdevCombo)> = parsed
            .iter()
            .filter_map(|(a, b)| evdev_combo(b).map(|c| (*a, c)))
            .collect();
        if combos.is_empty() {
            return Err(EngineError::Hotkey("no binding could be mapped to evdev keys".into()));
        }
        let mut devices: Vec<evdev::Device> = Vec::new();
        if let Ok(dir) = std::fs::read_dir("/dev/input") {
            for entry in dir.filter_map(|e| e.ok()) {
                let path = entry.path();
                if !path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("event")) {
                    continue;
                }
                if let Ok(dev) = evdev::Device::open(&path) {
                    let is_keyboard = dev
                        .supported_keys()
                        .is_some_and(|keys| keys.contains(evdev::KeyCode::KEY_A));
                    if is_keyboard {
                        devices.push(dev);
                    }
                }
            }
        }
        if devices.is_empty() {
            return Err(EngineError::Hotkey(
                "no readable keyboard under /dev/input (join the 'input' group)".into(),
            ));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("laf-evdev-hotkeys".into())
            .spawn(move || evdev_loop(devices, combos, tx, stop2))
            .map_err(|e| EngineError::Hotkey(format!("spawn evdev thread: {e}")))?;
        Ok(Self { stop, thread: Some(thread) })
    }
}

impl Drop for EvdevBackend {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[derive(Clone, Copy)]
struct EvdevCombo {
    ctrl: bool,
    alt: bool,
    shift: bool,
    meta: bool,
    key: evdev::KeyCode,
}

fn evdev_combo(b: &ParsedBinding) -> Option<EvdevCombo> {
    Some(EvdevCombo {
        ctrl: b.ctrl,
        alt: b.alt,
        shift: b.shift,
        meta: b.meta,
        key: code_to_evdev(&b.code)?,
    })
}

fn code_to_evdev(code: &str) -> Option<evdev::KeyCode> {
    use evdev::KeyCode as Key;
    if let Some(l) = code.strip_prefix("Key") {
        let c = l.chars().next()?;
        return Some(match c {
            'A' => Key::KEY_A, 'B' => Key::KEY_B, 'C' => Key::KEY_C, 'D' => Key::KEY_D,
            'E' => Key::KEY_E, 'F' => Key::KEY_F, 'G' => Key::KEY_G, 'H' => Key::KEY_H,
            'I' => Key::KEY_I, 'J' => Key::KEY_J, 'K' => Key::KEY_K, 'L' => Key::KEY_L,
            'M' => Key::KEY_M, 'N' => Key::KEY_N, 'O' => Key::KEY_O, 'P' => Key::KEY_P,
            'Q' => Key::KEY_Q, 'R' => Key::KEY_R, 'S' => Key::KEY_S, 'T' => Key::KEY_T,
            'U' => Key::KEY_U, 'V' => Key::KEY_V, 'W' => Key::KEY_W, 'X' => Key::KEY_X,
            'Y' => Key::KEY_Y, 'Z' => Key::KEY_Z,
            _ => return None,
        });
    }
    if let Some(d) = code.strip_prefix("Digit") {
        return Some(match d {
            "0" => Key::KEY_0, "1" => Key::KEY_1, "2" => Key::KEY_2, "3" => Key::KEY_3,
            "4" => Key::KEY_4, "5" => Key::KEY_5, "6" => Key::KEY_6, "7" => Key::KEY_7,
            "8" => Key::KEY_8, "9" => Key::KEY_9,
            _ => return None,
        });
    }
    if let Some(f) = code.strip_prefix('F') {
        if let Ok(n) = f.parse::<u8>() {
            let keys = [
                Key::KEY_F1, Key::KEY_F2, Key::KEY_F3, Key::KEY_F4, Key::KEY_F5, Key::KEY_F6,
                Key::KEY_F7, Key::KEY_F8, Key::KEY_F9, Key::KEY_F10, Key::KEY_F11, Key::KEY_F12,
            ];
            return keys.get((n as usize).checked_sub(1)?).copied();
        }
    }
    Some(match code {
        "Space" => Key::KEY_SPACE,
        "Enter" => Key::KEY_ENTER,
        "Tab" => Key::KEY_TAB,
        "Escape" => Key::KEY_ESC,
        "Backquote" => Key::KEY_GRAVE,
        "Minus" => Key::KEY_MINUS,
        "Equal" => Key::KEY_EQUAL,
        _ => return None,
    })
}

fn evdev_loop(
    mut devices: Vec<evdev::Device>,
    combos: Vec<(HotkeyAction, EvdevCombo)>,
    tx: Sender<HotkeyEvent>,
    stop: Arc<AtomicBool>,
) {
    use evdev::KeyCode as Key;
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut meta = false;
    // Actions whose key is currently held (to emit matching Up edges).
    let mut held: HashMap<Key, HotkeyAction> = HashMap::new();

    // Poll the devices with a timeout so the stop flag is honored.
    let mut fds: Vec<libc::pollfd> = devices
        .iter()
        .map(|d| libc::pollfd {
            fd: std::os::fd::AsRawFd::as_raw_fd(d),
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();

    while !stop.load(Ordering::SeqCst) {
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 200) };
        if n <= 0 {
            continue;
        }
        for (i, pfd) in fds.iter_mut().enumerate() {
            if pfd.revents & libc::POLLIN == 0 {
                continue;
            }
            pfd.revents = 0;
            let Ok(events) = devices[i].fetch_events() else { continue };
            for ev in events {
                let evdev::EventSummary::Key(_, key, value) = ev.destructure() else { continue };
                // value: 1 down, 0 up, 2 repeat
                match key {
                    Key::KEY_LEFTCTRL | Key::KEY_RIGHTCTRL => ctrl = value != 0,
                    Key::KEY_LEFTALT | Key::KEY_RIGHTALT => alt = value != 0,
                    Key::KEY_LEFTSHIFT | Key::KEY_RIGHTSHIFT => shift = value != 0,
                    Key::KEY_LEFTMETA | Key::KEY_RIGHTMETA => meta = value != 0,
                    _ => {}
                }
                if value == 1 {
                    for (action, combo) in &combos {
                        if combo.key == key
                            && combo.ctrl == ctrl
                            && combo.alt == alt
                            && combo.shift == shift
                            && combo.meta == meta
                        {
                            held.insert(key, *action);
                            let _ = tx.send(HotkeyEvent { action: *action, edge: Edge::Down });
                        }
                    }
                } else if value == 0 {
                    if let Some(action) = held.remove(&key) {
                        let _ = tx.send(HotkeyEvent { action, edge: Edge::Up });
                    }
                }
            }
        }
    }
}
