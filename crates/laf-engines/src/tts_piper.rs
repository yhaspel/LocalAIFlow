//! Piper TTS fallback (MIT-licensed neural TTS, https://github.com/rhasspy/piper).
//!
//! Runs the user-installed `piper` binary as a subprocess with `--output-raw`
//! (s16le mono at the voice's native rate, usually 22050 Hz) and streams the
//! audio straight into the cpal playback sink. Fully local. The voice model
//! is any `*.onnx` the user drops into `<models>/piper/` (with its `.json`
//! config next to it).

use crate::audio::{open_playback, PcmControl};
use laf_core::traits::{SpeechSynthesizer, TtsOptions, TtsPlayback};
use laf_core::types::{EngineError, EngineInfo, EngineResult, VoiceInfo};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct PiperTts {
    piper_bin: Option<PathBuf>,
    voices_dir: PathBuf,
}

impl PiperTts {
    pub fn new(voices_dir: PathBuf) -> Self {
        Self { piper_bin: find_in_path("piper"), voices_dir }
    }

    fn default_voice(&self) -> Option<PathBuf> {
        std::fs::read_dir(&self.voices_dir)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "onnx"))
    }

    fn voice_path(&self, voice_id: &str) -> Option<PathBuf> {
        if voice_id.is_empty() || !voice_id.starts_with("piper:") {
            return self.default_voice();
        }
        let name = voice_id.trim_start_matches("piper:");
        let p = self.voices_dir.join(format!("{name}.onnx"));
        p.is_file().then_some(p).or_else(|| self.default_voice())
    }

    /// Sample rate from the voice's JSON config (piper convention:
    /// `<voice>.onnx.json`, `audio.sample_rate`).
    fn voice_rate(voice: &Path) -> u32 {
        let cfg = voice.with_extension("onnx.json");
        std::fs::read_to_string(cfg)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.pointer("/audio/sample_rate").and_then(|r| r.as_u64()))
            .map(|r| r as u32)
            .unwrap_or(22_050)
    }
}

struct PiperPlayback {
    control: PcmControl,
    child_done: Arc<AtomicBool>,
    /// Shared with the reader thread: whoever wins takes the child and reaps it
    /// (reader on normal EOF, `stop` on cancel) so it never lingers as a zombie.
    child: Arc<Mutex<Option<Child>>>,
}

impl TtsPlayback for PiperPlayback {
    fn stop(&mut self) {
        if let Some(mut c) = self.child.lock().expect("piper child lock").take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.control.stop();
    }
    fn is_finished(&self) -> bool {
        self.child_done.load(Ordering::SeqCst) && self.control.is_finished()
    }
}

impl SpeechSynthesizer for PiperTts {
    fn speak(&self, text: &str, opts: &TtsOptions) -> EngineResult<Box<dyn TtsPlayback>> {
        let piper = self
            .piper_bin
            .clone()
            .ok_or_else(|| EngineError::Tts("piper binary not found on PATH".into()))?;
        let voice = self.voice_path(&opts.voice_id).ok_or_else(|| {
            EngineError::Tts(format!("no piper voice model in {}", self.voices_dir.display()))
        })?;
        let rate = Self::voice_rate(&voice);
        // Piper's length_scale is inverse speed.
        let length_scale = (1.0 / opts.rate.clamp(0.5, 2.0)).to_string();

        let mut child = Command::new(piper)
            .arg("--model")
            .arg(&voice)
            .arg("--length_scale")
            .arg(length_scale)
            .arg("--output-raw")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| EngineError::Tts(format!("failed to start piper: {e}")))?;

        let mut stdin = child.stdin.take().ok_or_else(|| EngineError::Tts("piper stdin".into()))?;
        let mut stdout =
            child.stdout.take().ok_or_else(|| EngineError::Tts("piper stdout".into()))?;
        let (writer, control) = open_playback(rate)?;

        // Feed piper's stdin on its OWN thread. piper starts emitting raw PCM to
        // stdout as soon as it has a sentence; writing a large selection
        // synchronously here could fill piper's stdout pipe (piper then blocks
        // writing it and stops reading stdin) before we finish writing stdin —
        // a classic pipe deadlock. Dropping stdin at the end signals EOF so
        // piper synthesizes the tail and exits.
        let text_owned = text.to_string();
        std::thread::Builder::new()
            .name("laf-piper-write".into())
            .spawn(move || {
                use std::io::Write;
                let _ = stdin.write_all(text_owned.as_bytes()).and_then(|_| stdin.write_all(b"\n"));
                // stdin dropped here → EOF to piper.
            })
            .map_err(|e| EngineError::Tts(format!("spawn piper writer: {e}")))?;

        let child = Arc::new(Mutex::new(Some(child)));
        let child_done = Arc::new(AtomicBool::new(false));
        let done2 = child_done.clone();
        let child_for_reader = child.clone();
        std::thread::Builder::new()
            .name("laf-piper-read".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match stdout.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let samples: Vec<f32> = buf[..n]
                                .chunks_exact(2)
                                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
                                .collect();
                            writer.push(&samples);
                        }
                    }
                }
                writer.finish();
                // stdout EOF means piper is exiting: reap it so a completed read
                // doesn't leave a zombie process (this is a long-running agent).
                if let Some(mut c) = child_for_reader.lock().expect("piper child lock").take() {
                    let _ = c.wait();
                }
                done2.store(true, Ordering::SeqCst);
            })
            .map_err(|e| EngineError::Tts(format!("spawn piper reader: {e}")))?;

        Ok(Box::new(PiperPlayback { control, child_done, child }))
    }

    fn voices(&self) -> Vec<VoiceInfo> {
        let mut out = Vec::new();
        if let Ok(dir) = std::fs::read_dir(&self.voices_dir) {
            for e in dir.filter_map(|e| e.ok()) {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "onnx") {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        out.push(VoiceInfo {
                            id: format!("piper:{stem}"),
                            label: format!("Piper — {stem}"),
                            language: "per-voice".into(),
                            engine: "piper".into(),
                        });
                    }
                }
            }
        }
        out
    }

    fn info(&self) -> EngineInfo {
        EngineInfo { name: "piper", model: None, accelerated: false }
    }
}

fn find_in_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| p.is_file())
}
