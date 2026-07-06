//! Kokoro-82M neural TTS via ONNX Runtime (`kokoroxide` crate, MIT/Apache-2.0;
//! model weights Apache-2.0 from onnx-community/Kokoro-82M-v1.0-ONNX).
//!
//! Text is chunked by sentence and synthesized incrementally: the first chunk
//! is usually audible in well under 500 ms on commodity CPUs while the rest
//! generates in the background. espeak-ng provides grapheme→phoneme
//! conversion (system package on both OSes; see the doctor / README).
//!
//! Everything runs in-process; no network is involved at any point.

use crate::audio::{open_playback, PcmControl};
use crate::textseg::tts_chunks;
use laf_kokoro::{load_voice_style, KokoroTTS, TTSConfig, VoiceStyle};
use laf_core::traits::{SpeechSynthesizer, TtsOptions, TtsPlayback};
use laf_core::types::{EngineError, EngineInfo, EngineResult, VoiceInfo};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const KOKORO_RATE: u32 = 24_000;

pub struct KokoroEngine {
    model_path: PathBuf,
    tokenizer_path: PathBuf,
    voices_dir: PathBuf,
    tts: Arc<Mutex<Option<KokoroTTS>>>,
    voice_cache: Mutex<HashMap<String, Arc<VoiceStyle>>>,
}

impl KokoroEngine {
    pub fn new(model_path: PathBuf, tokenizer_path: PathBuf, voices_dir: PathBuf) -> Self {
        Self {
            model_path,
            tokenizer_path,
            voices_dir,
            tts: Arc::new(Mutex::new(None)),
            voice_cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn files_present(&self) -> bool {
        self.model_path.is_file() && self.tokenizer_path.is_file()
    }

    fn ensure_loaded(&self) -> EngineResult<()> {
        let mut guard = self.tts.lock().expect("kokoro lock");
        if guard.is_some() {
            return Ok(());
        }
        if !self.files_present() {
            return Err(EngineError::ModelMissing {
                model_id: "kokoro-v1-q8".into(),
                hint: " (Settings → Models)".into(),
            });
        }
        let t0 = Instant::now();
        let config = TTSConfig::new(
            self.model_path.to_string_lossy().as_ref(),
            self.tokenizer_path.to_string_lossy().as_ref(),
        )
        .with_sample_rate(KOKORO_RATE as i32)
        .with_max_tokens_length(512);
        let tts = KokoroTTS::with_config(config)
            .map_err(|e| EngineError::Tts(format!("failed to load Kokoro: {e}")))?;
        tracing::info!("loaded Kokoro TTS in {} ms", t0.elapsed().as_millis());
        *guard = Some(tts);
        Ok(())
    }

    fn load_voice(&self, voice_id: &str) -> EngineResult<Arc<VoiceStyle>> {
        let mut cache = self.voice_cache.lock().expect("voice cache lock");
        if let Some(v) = cache.get(voice_id) {
            return Ok(v.clone());
        }
        let path = self.voices_dir.join(format!("{voice_id}.bin"));
        let path = if path.is_file() {
            path
        } else {
            // Fall back to any installed voice rather than failing outright.
            first_voice_file(&self.voices_dir).ok_or_else(|| EngineError::ModelMissing {
                model_id: format!("kokoro-voice-{}", voice_id.replace('_', "-")),
                hint: " (Settings → Models)".into(),
            })?
        };
        let style = load_voice_style(path.to_string_lossy().as_ref())
            .map_err(|e| EngineError::Tts(format!("failed to load voice: {e}")))?;
        let style = Arc::new(style);
        cache.insert(voice_id.to_string(), style.clone());
        Ok(style)
    }
}

struct KokoroPlayback {
    control: PcmControl,
    gen_done: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
}

impl TtsPlayback for KokoroPlayback {
    fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        self.control.stop();
    }
    fn is_finished(&self) -> bool {
        self.gen_done.load(Ordering::SeqCst) && self.control.is_finished()
    }
}

impl SpeechSynthesizer for KokoroEngine {
    fn speak(&self, text: &str, opts: &TtsOptions) -> EngineResult<Box<dyn TtsPlayback>> {
        self.ensure_loaded()?;
        let voice = self.load_voice(&opts.voice_id)?;
        let chunks = tts_chunks(text, 60);
        let (writer, control) = open_playback(KOKORO_RATE)?;
        let gen_done = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::new(AtomicBool::new(false));
        let rate = opts.rate.clamp(0.5, 2.0);

        let tts = self.tts.clone();
        let gen_done2 = gen_done.clone();
        let stop2 = stop_flag.clone();
        std::thread::Builder::new()
            .name("laf-kokoro-gen".into())
            .spawn(move || {
                let guard = tts.lock().expect("kokoro lock");
                let Some(tts) = guard.as_ref() else {
                    gen_done2.store(true, Ordering::SeqCst);
                    return;
                };
                for chunk in chunks {
                    if stop2.load(Ordering::SeqCst) {
                        break;
                    }
                    let t0 = Instant::now();
                    match tts.generate_speech(&chunk, &voice, rate) {
                        Ok(audio) => {
                            tracing::debug!(
                                "kokoro chunk ({} chars) in {} ms",
                                chunk.chars().count(),
                                t0.elapsed().as_millis()
                            );
                            writer.push(&audio.samples);
                        }
                        Err(e) => {
                            tracing::error!("kokoro synthesis failed: {e}");
                            break;
                        }
                    }
                }
                writer.finish();
                gen_done2.store(true, Ordering::SeqCst);
            })
            .map_err(|e| EngineError::Tts(format!("spawn kokoro thread: {e}")))?;

        Ok(Box::new(KokoroPlayback { control, gen_done, stop_flag }))
    }

    fn voices(&self) -> Vec<VoiceInfo> {
        let mut out = Vec::new();
        if let Ok(dir) = std::fs::read_dir(&self.voices_dir) {
            for e in dir.filter_map(|e| e.ok()) {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "bin") {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        out.push(VoiceInfo {
                            id: stem.to_string(),
                            label: pretty_voice_label(stem),
                            language: voice_language(stem),
                            engine: "kokoro".into(),
                        });
                    }
                }
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    fn info(&self) -> EngineInfo {
        EngineInfo { name: "kokoro", model: Some("Kokoro-82M v1.0 (ONNX)".into()), accelerated: false }
    }

    fn unload(&self) {
        let mut guard = self.tts.lock().expect("kokoro lock");
        if guard.take().is_some() {
            tracing::info!("unloaded Kokoro TTS");
        }
        self.voice_cache.lock().expect("voice cache lock").clear();
    }
}

fn first_voice_file(dir: &PathBuf) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "bin"))
}

/// Kokoro voice naming convention: `<a|b><f|m>_<name>` (american/british,
/// female/male).
fn pretty_voice_label(stem: &str) -> String {
    let (prefix, name) = stem.split_once('_').unwrap_or(("", stem));
    let mut chars = name.chars();
    let name_cap = chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_else(|| name.to_string());
    let desc = match prefix {
        "af" => " (US, female)",
        "am" => " (US, male)",
        "bf" => " (UK, female)",
        "bm" => " (UK, male)",
        _ => "",
    };
    format!("{name_cap}{desc}")
}

fn voice_language(stem: &str) -> String {
    match stem.chars().next() {
        Some('a') => "en-US".into(),
        Some('b') => "en-GB".into(),
        _ => "en".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_labels() {
        assert_eq!(pretty_voice_label("af_heart"), "Heart (US, female)");
        assert_eq!(pretty_voice_label("bm_george"), "George (UK, male)");
    }
}
