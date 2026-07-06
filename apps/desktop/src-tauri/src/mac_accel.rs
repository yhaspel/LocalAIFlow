//! M6 — optional macOS-native acceleration tier (Apple Silicon / macOS 26+).
//!
//! Design: tiny helper executables built from the Swift sources in
//! `platform/macos-helpers/` (not built by default; see that directory's
//! README). At runtime we probe for them next to the app binary or in the
//! app bundle's `Contents/MacOS/`; when absent, everything transparently
//! uses the portable engines (whisper.cpp Metal / llama.cpp) — the helpers
//! are a performance option, never a requirement.
//!
//! * `laf-whisperkit-helper` — WhisperKit (ANE-accelerated Whisper) or, on
//!   macOS 26+, SpeechAnalyzer/SpeechTranscriber. Streams JSON lines
//!   {"partial":…} / {"final":…} for 16 kHz f32 PCM on stdin.
//! * `laf-applefm-helper` — Apple Foundation Models (LanguageModelSession)
//!   for the cleanup step: reads {"system":…,"user":…} JSON on stdin,
//!   writes the formatted text to stdout.
//!
//! Everything stays on-device; the helpers use Apple's on-device models only.

#![cfg(target_os = "macos")]

use laf_core::traits::CleanContext;
use laf_core::types::{EngineError, EngineResult};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn helper_path(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate = dir.join(name);
    candidate.is_file().then_some(candidate)
}

pub fn whisperkit_available() -> bool {
    helper_path("laf-whisperkit-helper").is_some()
}

// ---------------------------------------------------------------------------
// WhisperKit subprocess STT (optional accel tier behind the same trait)
// ---------------------------------------------------------------------------

use crossbeam_channel::{unbounded, Receiver};
use laf_core::traits::{SpeechToText, SttSession, SttSessionConfig};
use laf_core::types::{EngineInfo, SttEvent};
use std::io::{BufRead, BufReader};
use std::process::Child;

pub struct WhisperKitStt {
    helper: PathBuf,
}

impl WhisperKitStt {
    /// Some(engine) only when the helper binary is installed.
    pub fn detect() -> Option<Self> {
        helper_path("laf-whisperkit-helper").map(|helper| Self { helper })
    }
}

pub struct WhisperKitSession {
    child: Option<Child>,
    stdin: Option<std::process::ChildStdin>,
    events: Receiver<SttEvent>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl SpeechToText for WhisperKitStt {
    fn start_session(&self, _cfg: SttSessionConfig) -> EngineResult<Box<dyn SttSession>> {
        let mut child = Command::new(&self.helper)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| EngineError::Stt(format!("whisperkit helper spawn: {e}")))?;
        let stdin = child.stdin.take();
        let stdout =
            child.stdout.take().ok_or_else(|| EngineError::Stt("helper stdout".into()))?;
        let (tx, rx) = unbounded::<SttEvent>();
        let reader = std::thread::Builder::new()
            .name("laf-whisperkit-read".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                    if let Some(t) = v.get("partial").and_then(|x| x.as_str()) {
                        let _ = tx.send(SttEvent::Partial { text: t.to_string() });
                    } else if let Some(t) = v.get("final").and_then(|x| x.as_str()) {
                        let _ = tx.send(SttEvent::Final {
                            text: t.to_string(),
                            t0_ms: 0,
                            t1_ms: 0,
                        });
                    }
                }
            })
            .map_err(|e| EngineError::Stt(format!("spawn reader: {e}")))?;
        Ok(Box::new(WhisperKitSession { child: Some(child), stdin, events: rx, reader: Some(reader) }))
    }

    fn info(&self) -> EngineInfo {
        EngineInfo { name: "whisperkit", model: None, accelerated: true }
    }

    fn unload(&self) {}
}

impl SttSession for WhisperKitSession {
    fn feed(&mut self, pcm: &[f32]) {
        if let Some(stdin) = self.stdin.as_mut() {
            let bytes: Vec<u8> = pcm.iter().flat_map(|f| f.to_le_bytes()).collect();
            if stdin.write_all(&bytes).is_err() {
                self.stdin = None; // helper died; finalize will surface it
            }
        }
    }

    fn segment_boundary(&mut self) {
        // The helper re-transcribes a rolling window and emits one final on
        // EOF; intra-utterance boundaries are a no-op for this tier.
    }

    fn drain_events(&mut self) -> Vec<SttEvent> {
        self.events.try_iter().collect()
    }

    fn finalize(&mut self) -> EngineResult<Vec<SttEvent>> {
        self.stdin.take(); // EOF → helper transcribes the whole buffer and exits
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
        Ok(self.events.try_iter().collect())
    }
}

impl Drop for WhisperKitSession {
    fn drop(&mut self) {
        self.stdin.take();
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

pub fn applefm_available() -> bool {
    helper_path("laf-applefm-helper").is_some()
}

/// Run the Apple Foundation Models cleanup helper if installed.
/// Returns Ok(None) when the helper is absent (caller falls back).
pub fn apple_fm_clean(raw: &str, ctx: &CleanContext) -> EngineResult<Option<String>> {
    let Some(helper) = helper_path("laf-applefm-helper") else {
        return Ok(None);
    };
    let payload = serde_json::json!({
        "system": laf_core::modes::build_system_prompt(ctx.mode),
        "user": raw,
    });
    let mut child = Command::new(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| EngineError::Cleanup(format!("applefm helper spawn: {e}")))?;
    child
        .stdin
        .take()
        .ok_or_else(|| EngineError::Cleanup("applefm helper stdin".into()))?
        .write_all(payload.to_string().as_bytes())
        .map_err(|e| EngineError::Cleanup(format!("applefm helper write: {e}")))?;
    let out = child
        .wait_with_output()
        .map_err(|e| EngineError::Cleanup(format!("applefm helper: {e}")))?;
    if !out.status.success() {
        return Err(EngineError::Cleanup(format!(
            "applefm helper exited with {:?}",
            out.status.code()
        )));
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        return Err(EngineError::Cleanup("applefm helper returned nothing".into()));
    }
    Ok(Some(text))
}
