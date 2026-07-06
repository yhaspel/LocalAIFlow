//! macOS global hotkeys via the `global-hotkey` crate (Carbon
//! `RegisterEventHotKey` under the hood).
//!
//! Why this instead of a CGEventTap: Carbon hotkeys deliver BOTH press and
//! release events (`HotKeyState::Pressed`/`Released`), which is exactly what
//! push-to-talk needs — and they work without the Input Monitoring
//! permission an event tap would require. The trade-off is that a hotkey
//! must be a discrete key-combo (no bare-modifier "hold ⌥" gestures); the
//! bindings UI reflects that.
//!
//! NOTE: the manager must be created on the main thread (Carbon requirement);
//! the Tauri shell does this during `setup`.

use crossbeam_channel::Sender;
use laf_core::hotkeys::ParsedBinding;
use laf_core::settings::HotkeyBindings;
use laf_core::traits::HotkeyBackend;
use laf_core::types::{Edge, EngineError, EngineResult, HotkeyAction, HotkeyEvent};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub struct MacHotkeys {
    tx: Sender<HotkeyEvent>,
    manager: Option<global_hotkey::GlobalHotKeyManager>,
    registered: Vec<global_hotkey::hotkey::HotKey>,
    id_map: Arc<Mutex<HashMap<u32, HotkeyAction>>>,
}

impl MacHotkeys {
    pub fn new(tx: Sender<HotkeyEvent>) -> Self {
        Self { tx, manager: None, registered: Vec::new(), id_map: Arc::new(Mutex::new(HashMap::new())) }
    }
}

impl HotkeyBackend for MacHotkeys {
    fn rebind(&mut self, bindings: &HotkeyBindings) -> EngineResult<Vec<String>> {
        use global_hotkey::hotkey::{Code, HotKey, Modifiers};
        use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
        use std::str::FromStr;

        let mut warnings = Vec::new();

        // Lazily create the manager (main thread — see module docs).
        if self.manager.is_none() {
            self.manager = Some(
                GlobalHotKeyManager::new()
                    .map_err(|e| EngineError::Hotkey(format!("hotkey manager: {e}")))?,
            );
            let map = self.id_map.clone();
            let tx = self.tx.clone();
            GlobalHotKeyEvent::set_event_handler(Some(move |ev: GlobalHotKeyEvent| {
                let action = map.lock().expect("id map").get(&ev.id()).copied();
                if let Some(action) = action {
                    let edge =
                        if ev.state() == HotKeyState::Pressed { Edge::Down } else { Edge::Up };
                    let _ = tx.send(HotkeyEvent { action, edge });
                }
            }));
        }
        let manager = self.manager.as_ref().expect("manager just created");

        // Drop previous registrations.
        for hk in self.registered.drain(..) {
            let _ = manager.unregister(hk);
        }
        self.id_map.lock().expect("id map").clear();

        let mut bind = |action: HotkeyAction, spec: &str| {
            let parsed = match ParsedBinding::parse(spec) {
                Ok(p) => p,
                Err(e) => {
                    warnings.push(format!("{action:?}: {e}"));
                    return;
                }
            };
            let mut mods = Modifiers::empty();
            if parsed.ctrl {
                mods |= Modifiers::CONTROL;
            }
            if parsed.alt {
                mods |= Modifiers::ALT;
            }
            if parsed.shift {
                mods |= Modifiers::SHIFT;
            }
            if parsed.meta {
                mods |= Modifiers::META;
            }
            let code = match Code::from_str(&parsed.code) {
                Ok(c) => c,
                Err(_) => {
                    warnings.push(format!("{action:?}: unsupported key '{}'", parsed.code));
                    return;
                }
            };
            let hotkey = HotKey::new(Some(mods), code);
            match manager.register(hotkey) {
                Ok(()) => {
                    self.id_map.lock().expect("id map").insert(hotkey.id(), action);
                    self.registered.push(hotkey);
                }
                Err(e) => warnings.push(format!(
                    "{action:?}: could not register '{spec}' ({e}) — is it taken by another app?"
                )),
            }
        };

        bind(HotkeyAction::DictateToggle, &bindings.dictate_toggle);
        if bindings.ptt_enabled {
            bind(HotkeyAction::DictatePushToTalk, &bindings.dictate_ptt);
        }
        bind(HotkeyAction::ReadSelection, &bindings.read_selection);
        bind(HotkeyAction::StopSpeech, &bindings.stop_speech);
        Ok(warnings)
    }

    fn backend_name(&self) -> &'static str {
        "carbon-hotkeys"
    }
}
