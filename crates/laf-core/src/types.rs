//! Shared value types used across the core, engines, platforms, and UI layer.

use serde::{Deserialize, Serialize};

/// Whisper (and our whole STT path) consumes 16 kHz mono f32 PCM.
pub const STT_SAMPLE_RATE: u32 = 16_000;

/// One chunk of captured audio, already converted to 16 kHz mono f32.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub samples: Vec<f32>,
    /// Root-mean-square level of this chunk (0.0..~1.0), for the HUD meter.
    pub rms: f32,
    pub peak: f32,
}

impl AudioFrame {
    pub fn from_samples(samples: Vec<f32>) -> Self {
        let (mut sq, mut peak) = (0.0f64, 0.0f32);
        for &s in &samples {
            sq += (s as f64) * (s as f64);
            peak = peak.max(s.abs());
        }
        let rms = if samples.is_empty() { 0.0 } else { (sq / samples.len() as f64).sqrt() as f32 };
        Self { samples, rms, peak }
    }
}

/// Events emitted by a live STT session.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SttEvent {
    /// Rolling hypothesis for audio that is still being spoken. Replaces the
    /// previous partial wholesale (not appended).
    Partial {
        text: String,
    },
    /// A segment the engine considers final (speech followed by silence).
    Final {
        text: String,
        t0_ms: u64,
        t1_ms: u64,
    },
    Error {
        message: String,
    },
}

/// Formatting modes, mirroring Wispr Flow's behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Verbatim transcript; dictionary substitution only.
    Raw,
    /// Default cleanup: fillers removed, punctuation/capitalization fixed.
    #[default]
    Auto,
    /// Auto + paragraph-oriented prose, greetings/sign-offs kept on own lines.
    Email,
    /// Auto + casual, compact (chat/message style).
    Message,
    /// Sentences/pauses become bullet list items.
    List,
    /// Minimal touch-ups for identifiers; no sentence-case enforcement.
    Code,
    /// Spoken editing commands are interpreted ("new line", "delete that", …).
    Command,
}

impl Mode {
    pub const ALL: [Mode; 7] =
        [Mode::Raw, Mode::Auto, Mode::Email, Mode::Message, Mode::List, Mode::Code, Mode::Command];

    pub fn label(&self) -> &'static str {
        match self {
            Mode::Raw => "Raw",
            Mode::Auto => "Auto",
            Mode::Email => "Email",
            Mode::Message => "Message",
            Mode::List => "List",
            Mode::Code => "Code",
            Mode::Command => "Command",
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Mode::Raw => "raw",
            Mode::Auto => "auto",
            Mode::Email => "email",
            Mode::Message => "message",
            Mode::List => "list",
            Mode::Code => "code",
            Mode::Command => "command",
        }
    }

    pub fn from_id(id: &str) -> Option<Mode> {
        Mode::ALL.iter().copied().find(|m| m.id() == id)
    }
}

/// How a piece of text ultimately reached the focused application.
/// Ordered by preference; the platform inserters walk this chain and report
/// which rung actually worked so the UI/doctor can surface it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum InsertionMethod {
    /// macOS: focused AXUIElement accepted a direct value/selected-text write.
    AxDirect,
    /// Linux: AT-SPI2 EditableText::InsertText on the focused widget.
    AtspiEditableText,
    /// Synthetic keystrokes (CGEvent unicode on macOS; wtype / ydotool /
    /// XTEST on Linux — `tool` says which).
    SyntheticKeys { tool: String },
    /// Clipboard set + synthetic paste chord + clipboard restore.
    ClipboardPaste { paste_tool: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct InsertionReport {
    pub method: InsertionMethod,
    pub chars: usize,
    pub elapsed_ms: u64,
    /// Human-readable notes from rungs that were tried and skipped.
    pub fallback_notes: Vec<String>,
}

/// Dictation pipeline phase, mirrored by the tray icon and HUD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Listening,
    Processing,
    Inserting,
    Speaking,
}

/// Everything the UI (HUD, settings, tray) needs to render live state.
/// Serialized directly to the webviews via Tauri events.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum UiEvent {
    Phase {
        phase: Phase,
        mode: Mode,
    },
    Level {
        rms: f32,
        peak: f32,
    },
    Partial {
        text: String,
    },
    FinalSegment {
        text: String,
    },
    Inserted {
        report: InsertionReport,
        text: String,
    },
    TtsStarted {
        chars: usize,
    },
    TtsStopped,
    PipelineError {
        message: String,
    },
    Latency {
        stage: String,
        ms: u64,
    },
    ModelDownload {
        model_id: String,
        downloaded: u64,
        total: u64,
        done: bool,
        error: Option<String>,
    },
}

/// Hotkey-triggerable actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyAction {
    DictateToggle,
    DictatePushToTalk,
    ReadSelection,
    StopSpeech,
}

/// Press/release edge for hotkey events. Toggle actions only use `Down`;
/// push-to-talk needs both edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy)]
pub struct HotkeyEvent {
    pub action: HotkeyAction,
    pub edge: Edge,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceInfo {
    pub id: String,
    pub label: String,
    pub language: String,
    pub engine: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineInfo {
    pub name: &'static str,
    pub model: Option<String>,
    pub accelerated: bool,
}

/// Unified error type crossing engine/platform boundaries.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("audio device error: {0}")]
    Audio(String),
    #[error("speech-to-text error: {0}")]
    Stt(String),
    #[error("text cleanup error: {0}")]
    Cleanup(String),
    #[error("text insertion failed: {0}")]
    Insertion(String),
    #[error("speech synthesis error: {0}")]
    Tts(String),
    #[error("hotkey error: {0}")]
    Hotkey(String),
    #[error("model '{model_id}' is not installed{hint}")]
    ModelMissing { model_id: String, hint: String },
    #[error("operation requires network but Fully Offline mode is enabled")]
    OfflineMode,
    #[error("missing permission: {0}")]
    Permission(String),
    #[error("unsupported in this environment: {0}")]
    Unsupported(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

pub type EngineResult<T> = Result<T, EngineError>;
