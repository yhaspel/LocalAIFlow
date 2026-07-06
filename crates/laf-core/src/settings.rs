//! Persistent settings, stored as pretty JSON in the platform config dir:
//! * macOS:  `~/Library/Application Support/LocalAIFlow/settings.json`
//! * Linux:  `$XDG_CONFIG_HOME/LocalAIFlow/settings.json` (default `~/.config/…`)

use crate::dictionary::DictEntry;
use crate::types::Mode;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

pub const APP_DIR_NAME: &str = "LocalAIFlow";

/// Hotkey bindings use the W3C `KeyboardEvent.code` names for keys
/// (`KeyD`, `Space`, `F5`, …) joined with `+` to modifiers
/// (`ctrl`, `alt`, `shift`, `super`). Example: `"ctrl+alt+KeyD"`.
/// This is the same grammar the `global-hotkey` crate parses, and we map it
/// ourselves onto evdev keycodes and portal trigger descriptions on Linux.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HotkeyBindings {
    pub dictate_toggle: String,
    /// Hold-to-dictate. Release stops and inserts.
    pub dictate_ptt: String,
    pub ptt_enabled: bool,
    pub read_selection: String,
    pub stop_speech: String,
}

impl Default for HotkeyBindings {
    fn default() -> Self {
        Self {
            dictate_toggle: "ctrl+alt+KeyD".into(),
            dictate_ptt: "ctrl+alt+Space".into(),
            ptt_enabled: true,
            read_selection: "ctrl+alt+KeyR".into(),
            stop_speech: "ctrl+alt+KeyX".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CleanerTier {
    /// Local LLM if its model is installed and loaded, else deterministic.
    #[default]
    Auto,
    Deterministic,
    LocalLlm,
    /// User-run Ollama at 127.0.0.1:11434 (still fully local; opt-in).
    Ollama,
    /// macOS 26+ Apple Foundation Models helper (opt-in; macOS only).
    AppleFm,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SttSettings {
    /// "whisper" (portable default) or "whisperkit" (optional macOS helper).
    pub engine: String,
    pub model_id: String,
    pub threads: usize,
}

impl Default for SttSettings {
    fn default() -> Self {
        Self { engine: "whisper".into(), model_id: "whisper-large-v3-turbo-q5".into(), threads: 0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CleanerSettings {
    pub tier: CleanerTier,
    pub model_id: String,
    /// Model name to request from a local Ollama when tier == Ollama.
    pub ollama_model: String,
}

impl Default for CleanerSettings {
    fn default() -> Self {
        Self {
            tier: CleanerTier::Auto,
            model_id: "qwen2.5-3b-instruct-q4".into(),
            ollama_model: "qwen2.5:3b-instruct".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TtsSettings {
    /// "kokoro" | "piper" | "system"
    pub engine: String,
    pub voice_id: String,
    pub rate: f32,
}

impl Default for TtsSettings {
    fn default() -> Self {
        Self { engine: "kokoro".into(), voice_id: "af_heart".into(), rate: 1.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    pub hotkeys: HotkeyBindings,
    pub default_mode: Mode,
    /// ISO 639-1 code or "auto".
    pub language: String,
    /// None → system default input device.
    pub input_device: Option<String>,
    pub stt: SttSettings,
    pub cleaner: CleanerSettings,
    pub tts: TtsSettings,
    /// Insert each cleaned segment as it finalizes instead of once at the end.
    pub insert_incremental: bool,
    /// Hard network kill-switch: ModelManager refuses ALL downloads.
    pub fully_offline: bool,
    pub launch_at_login: bool,
    pub hud_enabled: bool,
    pub dictionary: Vec<DictEntry>,
    /// Unload idle models after this many seconds (0 = never).
    pub model_idle_unload_secs: u64,
    pub onboarding_done: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            hotkeys: HotkeyBindings::default(),
            default_mode: Mode::Auto,
            language: "auto".into(),
            input_device: None,
            stt: SttSettings::default(),
            cleaner: CleanerSettings::default(),
            tts: TtsSettings::default(),
            insert_incremental: false,
            fully_offline: false,
            launch_at_login: false,
            hud_enabled: true,
            dictionary: Vec::new(),
            model_idle_unload_secs: 300,
            onboarding_done: false,
        }
    }
}

/// App config directory (settings). Created on demand.
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
}

/// App data directory (models live under `<data_dir>/models`).
/// macOS: `~/Library/Application Support/LocalAIFlow`
/// Linux: `$XDG_DATA_HOME/LocalAIFlow` (default `~/.local/share/LocalAIFlow`)
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
}

/// Thread-safe settings store with atomic-rename persistence.
pub struct SettingsStore {
    path: PathBuf,
    inner: RwLock<Settings>,
}

impl SettingsStore {
    pub fn load_default() -> Self {
        Self::load_from(config_dir().join("settings.json"))
    }

    pub fn load_from(path: PathBuf) -> Self {
        let settings = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| match serde_json::from_str::<Settings>(&s) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!("settings.json unreadable ({e}); using defaults");
                    None
                }
            })
            .unwrap_or_default();
        Self { path, inner: RwLock::new(settings) }
    }

    pub fn get(&self) -> Settings {
        self.inner.read().expect("settings lock").clone()
    }

    /// Mutate + persist. Returns the updated snapshot.
    pub fn update<F: FnOnce(&mut Settings)>(&self, f: F) -> Settings {
        let snapshot = {
            let mut guard = self.inner.write().expect("settings lock");
            f(&mut guard);
            guard.clone()
        };
        if let Err(e) = persist(&self.path, &snapshot) {
            tracing::error!("failed to save settings: {e}");
        }
        snapshot
    }

    pub fn replace(&self, new: Settings) -> Settings {
        self.update(|s| *s = new)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn persist(path: &Path, s: &Settings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(s).expect("serialize settings"))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = SettingsStore::load_from(path.clone());
        assert_eq!(store.get(), Settings::default());

        store.update(|s| {
            s.language = "de".into();
            s.fully_offline = true;
        });
        // Reload from disk.
        let store2 = SettingsStore::load_from(path);
        assert_eq!(store2.get().language, "de");
        assert!(store2.get().fully_offline);
    }

    #[test]
    fn tolerates_unknown_and_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"schema_version":1,"language":"fr","future_field":42}"#).unwrap();
        let store = SettingsStore::load_from(path);
        assert_eq!(store.get().language, "fr");
        assert_eq!(store.get().default_mode, Mode::Auto);
    }
}
