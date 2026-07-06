//! Settings-aware engine wrappers.
//!
//! The pipeline holds `Arc<dyn Trait>` engines for its whole lifetime; these
//! wrappers re-resolve model files from `SettingsStore` + `ModelManager` on
//! every session/speak/clean call, transparently (re)loading inner engines
//! when the user switches models in Settings — no app restart, no pipeline
//! teardown.

use laf_core::models::ModelManager;
use laf_core::settings::{CleanerTier, SettingsStore};
use laf_core::traits::*;
use laf_core::types::*;
#[cfg(any(feature = "stt-whisper", feature = "llm-llama", feature = "tts-kokoro"))]
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(any(feature = "stt-whisper", feature = "llm-llama", feature = "tts-kokoro"))]
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Speech-to-text
// ---------------------------------------------------------------------------

pub struct DynamicStt {
    settings: Arc<SettingsStore>,
    mm: Arc<ModelManager>,
    #[cfg(feature = "stt-whisper")]
    inner: Mutex<Option<(PathBuf, Arc<laf_engines::stt_whisper::WhisperEngine>)>>,
}

impl DynamicStt {
    pub fn new(settings: Arc<SettingsStore>, mm: Arc<ModelManager>) -> Self {
        Self {
            settings,
            mm,
            #[cfg(feature = "stt-whisper")]
            inner: Mutex::new(None),
        }
    }
}

impl SpeechToText for DynamicStt {
    #[cfg(feature = "stt-whisper")]
    fn start_session(&self, cfg: SttSessionConfig) -> EngineResult<Box<dyn SttSession>> {
        let s = self.settings.get();
        // Optional macOS acceleration tier (M6): route to the WhisperKit
        // helper when the user selected it AND the helper is installed;
        // otherwise fall through to portable whisper.cpp.
        #[cfg(target_os = "macos")]
        if s.stt.engine == "whisperkit" {
            match crate::mac_accel::WhisperKitStt::detect() {
                Some(engine) => return engine.start_session(cfg),
                None => tracing::warn!(
                    "whisperkit engine selected but helper not installed — using whisper.cpp \
                     (see platform/macos-helpers/README.md)"
                ),
            }
        }
        let path = self.mm.require(&s.stt.model_id)?;
        let mut guard = self.inner.lock().expect("stt lock");
        let rebuild = guard.as_ref().map(|(p, _)| p != &path).unwrap_or(true);
        if rebuild {
            *guard = Some((
                path.clone(),
                Arc::new(laf_engines::stt_whisper::WhisperEngine::new(path, s.stt.model_id.clone())),
            ));
        }
        guard.as_ref().expect("just built").1.start_session(cfg)
    }

    #[cfg(not(feature = "stt-whisper"))]
    fn start_session(&self, _cfg: SttSessionConfig) -> EngineResult<Box<dyn SttSession>> {
        let _ = (&self.settings, &self.mm);
        Err(EngineError::Unsupported(
            "this binary was built without the whisper STT engine (feature stt-whisper)".into(),
        ))
    }

    fn info(&self) -> EngineInfo {
        #[cfg(feature = "stt-whisper")]
        {
            if let Some((_, e)) = self.inner.lock().expect("stt lock").as_ref() {
                return e.info();
            }
        }
        EngineInfo { name: "whisper", model: Some(self.settings.get().stt.model_id), accelerated: false }
    }

    fn unload(&self) {
        #[cfg(feature = "stt-whisper")]
        if let Some((_, e)) = self.inner.lock().expect("stt lock").as_ref() {
            e.unload();
        }
    }
}

// ---------------------------------------------------------------------------
// LLM cleaner (llama.cpp or local Ollama, per settings tier)
// ---------------------------------------------------------------------------

pub struct DynamicCleaner {
    settings: Arc<SettingsStore>,
    mm: Arc<ModelManager>,
    #[cfg(feature = "llm-llama")]
    llama: Mutex<Option<(PathBuf, Arc<laf_engines::clean_llama::LlamaCleaner>)>>,
}

impl DynamicCleaner {
    pub fn new(settings: Arc<SettingsStore>, mm: Arc<ModelManager>) -> Self {
        Self {
            settings,
            mm,
            #[cfg(feature = "llm-llama")]
            llama: Mutex::new(None),
        }
    }

    #[cfg(feature = "llm-llama")]
    fn llama_for_current_model(
        &self,
    ) -> EngineResult<Arc<laf_engines::clean_llama::LlamaCleaner>> {
        let s = self.settings.get();
        let path = self.mm.require(&s.cleaner.model_id)?;
        let mut guard = self.llama.lock().expect("llama lock");
        let rebuild = guard.as_ref().map(|(p, _)| p != &path).unwrap_or(true);
        if rebuild {
            *guard = Some((
                path.clone(),
                Arc::new(laf_engines::clean_llama::LlamaCleaner::new(path, s.cleaner.model_id.clone())),
            ));
        }
        Ok(guard.as_ref().expect("just built").1.clone())
    }

    fn ollama(&self) -> EngineResult<laf_engines::clean_ollama::OllamaCleaner> {
        let s = self.settings.get();
        laf_engines::clean_ollama::OllamaCleaner::new(
            laf_engines::clean_ollama::DEFAULT_OLLAMA_URL,
            s.cleaner.ollama_model,
        )
    }
}

impl TextCleaner for DynamicCleaner {
    fn clean(&self, raw: &str, ctx: &CleanContext) -> EngineResult<String> {
        match self.settings.get().cleaner.tier {
            CleanerTier::Ollama => self.ollama()?.clean(raw, ctx),
            CleanerTier::AppleFm => {
                #[cfg(target_os = "macos")]
                {
                    // Optional macOS 26+ helper (M6): used when present.
                    if let Some(out) = crate::mac_accel::apple_fm_clean(raw, ctx)? {
                        return Ok(out);
                    }
                }
                // Not available → treat as LocalLlm.
                #[cfg(feature = "llm-llama")]
                return self.llama_for_current_model()?.clean(raw, ctx);
                #[cfg(not(feature = "llm-llama"))]
                Err(EngineError::Cleanup("no LLM cleaner in this build".into()))
            }
            _ => {
                #[cfg(feature = "llm-llama")]
                return self.llama_for_current_model()?.clean(raw, ctx);
                #[cfg(not(feature = "llm-llama"))]
                Err(EngineError::Cleanup("no LLM cleaner in this build".into()))
            }
        }
    }

    fn name(&self) -> &'static str {
        "llm-auto"
    }

    fn available(&self) -> bool {
        let s = self.settings.get();
        match s.cleaner.tier {
            CleanerTier::Deterministic => false,
            CleanerTier::Ollama => self.ollama().map(|o| o.available()).unwrap_or(false),
            _ => {
                #[cfg(feature = "llm-llama")]
                return self.mm.resolve(&s.cleaner.model_id).is_some();
                #[cfg(not(feature = "llm-llama"))]
                false
            }
        }
    }

    fn unload(&self) {
        #[cfg(feature = "llm-llama")]
        if let Some((_, e)) = self.llama.lock().expect("llama lock").as_ref() {
            e.unload();
        }
    }
}

// ---------------------------------------------------------------------------
// Kokoro TTS (model paths resolved per call)
// ---------------------------------------------------------------------------

#[cfg(feature = "tts-kokoro")]
pub struct DynamicKokoro {
    mm: Arc<ModelManager>,
    inner: Mutex<Option<((PathBuf, PathBuf), Arc<laf_engines::tts_kokoro::KokoroEngine>)>>,
}

#[cfg(feature = "tts-kokoro")]
impl DynamicKokoro {
    pub fn new(mm: Arc<ModelManager>) -> Self {
        Self { mm, inner: Mutex::new(None) }
    }

    fn engine(&self) -> EngineResult<Arc<laf_engines::tts_kokoro::KokoroEngine>> {
        let model = self.mm.require("kokoro-v1-q8")?;
        let tokenizer = self.mm.require("kokoro-tokenizer")?;
        // The voices dir is wherever the default voice resolved (user dir
        // wins over bundled), falling back to the user dir layout.
        let voices_dir = self
            .mm
            .resolve("kokoro-voice-af-heart")
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| self.mm.user_root().join("kokoro/voices"));
        let key = (model.clone(), tokenizer.clone());
        let mut guard = self.inner.lock().expect("kokoro lock");
        let rebuild = guard.as_ref().map(|(k, _)| k != &key).unwrap_or(true);
        if rebuild {
            *guard = Some((
                key,
                Arc::new(laf_engines::tts_kokoro::KokoroEngine::new(model, tokenizer, voices_dir)),
            ));
        }
        Ok(guard.as_ref().expect("just built").1.clone())
    }
}

#[cfg(feature = "tts-kokoro")]
impl SpeechSynthesizer for DynamicKokoro {
    fn speak(&self, text: &str, opts: &TtsOptions) -> EngineResult<Box<dyn TtsPlayback>> {
        self.engine()?.speak(text, opts)
    }
    fn voices(&self) -> Vec<VoiceInfo> {
        self.engine().map(|e| e.voices()).unwrap_or_default()
    }
    fn info(&self) -> EngineInfo {
        EngineInfo { name: "kokoro", model: Some("Kokoro-82M v1.0 (ONNX)".into()), accelerated: false }
    }
    fn unload(&self) {
        if let Some((_, e)) = self.inner.lock().expect("kokoro lock").as_ref() {
            e.unload();
        }
    }
}
