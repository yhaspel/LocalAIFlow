//! Last-resort TTS via the operating system's built-in synthesizer.
//! Always local; used when no neural model is installed (e.g. Fully Offline
//! without bundled weights).
//!
//! * macOS: `/usr/bin/say` (AVSpeechSynthesizer voices; `-r` = words/min).
//! * Linux: `spd-say` (speech-dispatcher) preferred, else `espeak-ng`.

use laf_core::traits::{SpeechSynthesizer, TtsOptions, TtsPlayback};
use laf_core::types::{EngineError, EngineInfo, EngineResult, VoiceInfo};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

pub struct SystemTts;

impl SystemTts {
    pub fn new() -> Self {
        Self
    }

    fn find(bin: &str) -> Option<std::path::PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| p.is_file())
    }
}

impl Default for SystemTts {
    fn default() -> Self {
        Self::new()
    }
}

struct ChildPlayback {
    child: Arc<Mutex<Option<Child>>>,
    /// Extra cleanup on stop (e.g. `spd-say -S` cancels queued speech).
    on_stop: Option<fn()>,
}

impl TtsPlayback for ChildPlayback {
    fn stop(&mut self) {
        if let Some(mut child) = self.child.lock().expect("tts child lock").take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(f) = self.on_stop.take() {
            f();
        }
    }

    fn is_finished(&self) -> bool {
        let mut guard = self.child.lock().expect("tts child lock");
        match guard.as_mut() {
            None => true,
            Some(child) => match child.try_wait() {
                Ok(Some(_)) => {
                    *guard = None;
                    true
                }
                Ok(None) => false,
                Err(_) => true,
            },
        }
    }
}

fn spd_cancel() {
    let _ = Command::new("spd-say").arg("-S").status();
}

impl SpeechSynthesizer for SystemTts {
    fn speak(&self, text: &str, opts: &TtsOptions) -> EngineResult<Box<dyn TtsPlayback>> {
        #[cfg(target_os = "macos")]
        {
            // `say` default rate ≈ 175 wpm; scale by the requested factor.
            let wpm = (175.0 * opts.rate).clamp(90.0, 450.0) as i32;
            let mut cmd = Command::new("/usr/bin/say");
            cmd.arg("-r").arg(wpm.to_string());
            // Voice ids for the system engine are the `say -v` names.
            if !opts.voice_id.is_empty() && opts.voice_id != "system-default" {
                cmd.arg("-v").arg(&opts.voice_id);
            }
            // `--` ends option parsing so a selection beginning with '-' can't
            // be misread as a flag (e.g. `say -o` / `espeak-ng -w` write files).
            cmd.arg("--")
                .arg(text)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let child =
                cmd.spawn().map_err(|e| EngineError::Tts(format!("failed to run `say`: {e}")))?;
            return Ok(Box::new(ChildPlayback {
                child: Arc::new(Mutex::new(Some(child))),
                on_stop: None,
            }));
        }
        #[cfg(target_os = "linux")]
        {
            if Self::find("spd-say").is_some() {
                // spd-say rate: -100..100 (0 = normal). Map 0.5x..2x roughly.
                let rate = (((opts.rate - 1.0) * 100.0).clamp(-100.0, 100.0)) as i32;
                let mut cmd = Command::new("spd-say");
                cmd.arg("-r").arg(rate.to_string());
                // keep the child alive while speaking so is_finished works
                cmd.arg("--wait");
                // `--` ends option parsing so a selection beginning with '-' can't
                // be misread as a flag (e.g. `say -o` / `espeak-ng -w` write files).
                cmd.arg("--")
                    .arg(text)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                let child = cmd
                    .spawn()
                    .map_err(|e| EngineError::Tts(format!("failed to run spd-say: {e}")))?;
                return Ok(Box::new(ChildPlayback {
                    child: Arc::new(Mutex::new(Some(child))),
                    on_stop: Some(spd_cancel),
                }));
            }
            if Self::find("espeak-ng").is_some() {
                let wpm = (175.0 * opts.rate).clamp(80.0, 450.0) as i32;
                let mut cmd = Command::new("espeak-ng");
                cmd.arg("-s").arg(wpm.to_string());
                // `--` ends option parsing so a selection beginning with '-' can't
                // be misread as a flag (e.g. `say -o` / `espeak-ng -w` write files).
                cmd.arg("--")
                    .arg(text)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                let child = cmd
                    .spawn()
                    .map_err(|e| EngineError::Tts(format!("failed to run espeak-ng: {e}")))?;
                return Ok(Box::new(ChildPlayback {
                    child: Arc::new(Mutex::new(Some(child))),
                    on_stop: None,
                }));
            }
            return Err(EngineError::Tts(
                "no system synthesizer found (install speech-dispatcher or espeak-ng)".into(),
            ));
        }
        #[allow(unreachable_code)]
        Err(EngineError::Unsupported("system TTS: unsupported platform".into()))
    }

    fn voices(&self) -> Vec<VoiceInfo> {
        vec![VoiceInfo {
            id: "system-default".into(),
            label: "System default voice".into(),
            language: "system".into(),
            engine: "system".into(),
        }]
    }

    fn info(&self) -> EngineInfo {
        EngineInfo { name: "system", model: None, accelerated: false }
    }
}
