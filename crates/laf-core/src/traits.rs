//! Engine trait definitions — the contract between the shared core and the
//! per-OS / per-engine implementations.
//!
//! Design notes:
//! * Traits are synchronous and channel-based rather than async. Every engine
//!   that needs concurrency owns a worker thread internally (whisper decode,
//!   audio callbacks, TTS playback); the pipeline talks to them through
//!   `crossbeam_channel`. This keeps the traits object-safe and trivially
//!   implementable from C-callback contexts (CoreAudio, ALSA) where an async
//!   runtime is not available.
//! * All trait objects are `Send` so the pipeline controller thread can own
//!   them; engines that are cheap to share are also `Sync`.

use crate::dictionary::Dictionary;
use crate::settings::HotkeyBindings;
use crate::types::*;
use crossbeam_channel::Sender;

/// Microphone capture. Implementations deliver 16 kHz mono f32 frames of
/// roughly 50–100 ms regardless of the hardware format (they resample).
pub trait AudioCapture: Send {
    /// Begin capturing; frames flow into `sink` until [`stop`](Self::stop).
    /// Returns the human-readable device name actually opened.
    fn start(&mut self, sink: Sender<AudioFrame>) -> EngineResult<String>;
    fn stop(&mut self);
    fn is_running(&self) -> bool;
    /// Input device names, for the settings UI. Empty when enumeration is
    /// unsupported; first entry is the default device.
    fn list_devices(&self) -> Vec<String>;
    /// Select an input device by name (None → system default). Takes effect
    /// on the next `start`.
    fn select_device(&mut self, name: Option<String>);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadDecision {
    Silence,
    Speech,
    /// Speech just ended (trailing hangover elapsed) — segment boundary.
    SpeechEnd,
}

/// Speech/silence segmentation. Swappable: energy gate by default, Silero
/// ONNX optionally.
pub trait VoiceActivityDetector: Send {
    /// Feed one frame (any length) of 16 kHz mono audio.
    fn process(&mut self, samples: &[f32]) -> VadDecision;
    fn reset(&mut self);
    fn name(&self) -> &'static str;
}

#[derive(Debug, Clone)]
pub struct SttSessionConfig {
    /// BCP-47-ish two-letter code, or "auto".
    pub language: String,
    /// Words/phrases from the custom dictionary, passed as vocabulary hints
    /// (whisper.cpp: initial_prompt) where the engine supports it.
    pub vocabulary_hints: Vec<String>,
    /// Prefer translating nothing; transcribe verbatim.
    pub max_threads: usize,
}

/// A speech-to-text engine (factory). Sessions are single dictation
/// utterances; the engine may keep the model resident between sessions.
pub trait SpeechToText: Send + Sync {
    fn start_session(&self, cfg: SttSessionConfig) -> EngineResult<Box<dyn SttSession>>;
    fn info(&self) -> EngineInfo;
    /// Drop model weights to reclaim memory (idle unload). Next session
    /// reloads transparently.
    fn unload(&self);
}

/// A live streaming transcription session. `feed` is called from the audio
/// thread and must not block; decoding happens on the session's own worker.
///
/// Segmentation contract: the *pipeline* owns the VAD and calls
/// [`segment_boundary`](Self::segment_boundary) when speech ends; the session
/// then turns its current window into a `Final` event. This keeps VAD
/// swappable without every STT engine reimplementing segmentation.
pub trait SttSession: Send {
    fn feed(&mut self, pcm_16k_mono: &[f32]);
    /// VAD detected end-of-speech: finalize the current segment.
    fn segment_boundary(&mut self);
    /// Non-blocking: everything the decoder produced since the last call.
    fn drain_events(&mut self) -> Vec<SttEvent>;
    /// Flush remaining audio and return any last events (the pipeline
    /// accumulates `Final` texts itself and joins them into the transcript).
    fn finalize(&mut self) -> EngineResult<Vec<SttEvent>>;
}

#[derive(Debug, Clone)]
pub struct CleanContext {
    pub mode: Mode,
    pub language: String,
    pub dictionary: Dictionary,
}

/// Transcript → clean text. Implementations: deterministic (always available),
/// llama.cpp local LLM, Ollama-local, and (macOS 26+, optional) Apple
/// Foundation Models via helper.
pub trait TextCleaner: Send + Sync {
    fn clean(&self, raw: &str, ctx: &CleanContext) -> EngineResult<String>;
    fn name(&self) -> &'static str;
    /// True if this cleaner is ready right now (model loaded / reachable).
    fn available(&self) -> bool;
    fn unload(&self) {}
}

/// Insert text into the focused field of the frontmost application.
/// Implementations must try their platform's chain in order (least invasive
/// first) and report what worked.
pub trait TextInserter: Send + Sync {
    fn insert_text(&self, text: &str) -> EngineResult<InsertionReport>;
}

/// Read the currently selected text in the frontmost application (for TTS).
pub trait SelectionReader: Send + Sync {
    fn read_selection(&self) -> EngineResult<Option<String>>;
}

#[derive(Debug, Clone)]
pub struct TtsOptions {
    pub voice_id: String,
    /// 1.0 = normal speed.
    pub rate: f32,
}

/// Handle to in-flight speech playback.
pub trait TtsPlayback: Send {
    fn stop(&mut self);
    fn is_finished(&self) -> bool;
}

pub trait SpeechSynthesizer: Send + Sync {
    /// Speak `text`, streaming sentence-by-sentence; returns immediately with
    /// a playback handle.
    fn speak(&self, text: &str, opts: &TtsOptions) -> EngineResult<Box<dyn TtsPlayback>>;
    fn voices(&self) -> Vec<VoiceInfo>;
    fn info(&self) -> EngineInfo;
    fn unload(&self) {}
}

/// Global hotkey backend. Emits [`HotkeyEvent`]s on the channel passed at
/// construction time (see platform crates). `rebind` applies new bindings and
/// returns warnings for keys that could not be grabbed in this environment
/// (e.g. Wayland without portal or evdev access).
pub trait HotkeyBackend: Send {
    fn rebind(&mut self, bindings: &HotkeyBindings) -> EngineResult<Vec<String>>;
    fn backend_name(&self) -> &'static str;
}
