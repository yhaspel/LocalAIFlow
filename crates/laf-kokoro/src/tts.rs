//! Kokoro ONNX session. Ported from kokoroxide v0.1.5 `kokoro/tts.rs`
//! (MIT/Apache-2.0): the ort 1.16 Environment/CowArray/Value plumbing is
//! replaced with the ort 2.x `Session` + `Tensor` API; WAV export dropped
//! (Local AI Flow streams PCM straight to the audio device).

use crate::ipa_tokenizer::EspeakIpaTokenizer;
use crate::voice::VoiceStyle;
use crate::{KokoroError, Result};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use std::sync::Mutex;

pub struct TTSConfig {
    pub model_path: String,
    pub tokenizer_path: String,
    pub max_length: usize,
    pub sample_rate: u32,
    pub intra_threads: usize,
}

impl TTSConfig {
    pub fn new(model_path: &str, tokenizer_path: &str) -> Self {
        TTSConfig {
            model_path: model_path.to_string(),
            tokenizer_path: tokenizer_path.to_string(),
            max_length: 512,
            sample_rate: 24_000,
            intra_threads: 0,
        }
    }

    pub fn with_max_tokens_length(mut self, max_length: usize) -> Self {
        self.max_length = max_length;
        self
    }

    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = sample_rate;
        self
    }

    pub fn with_intra_threads(mut self, n: usize) -> Self {
        self.intra_threads = n;
        self
    }
}

pub struct GeneratedAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub duration_seconds: f32,
}

pub struct KokoroTTS {
    /// ort 2.x `Session::run` takes `&mut self` (I/O binding reuse), so the
    /// session sits behind a mutex; synthesis is serialized anyway.
    session: Mutex<Session>,
    tokenizer: EspeakIpaTokenizer,
    sample_rate: u32,
}

impl KokoroTTS {
    pub fn new(model_path: &str, tokenizer_path: &str) -> Result<Self> {
        Self::with_config(TTSConfig::new(model_path, tokenizer_path))
    }

    pub fn with_config(config: TTSConfig) -> Result<Self> {
        let TTSConfig { model_path, tokenizer_path, max_length, sample_rate, intra_threads } =
            config;

        let mut builder = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?;
        if intra_threads > 0 {
            builder = builder.with_intra_threads(intra_threads)?;
        }
        let session = builder.commit_from_file(&model_path)?;

        let tokenizer_content = std::fs::read_to_string(&tokenizer_path)?;
        let tokenizer_json: serde_json::Value = serde_json::from_str(&tokenizer_content)
            .map_err(|e| KokoroError::Tokenizer(format!("tokenizer.json parse: {e}")))?;
        let vocab_obj = tokenizer_json["model"]["vocab"]
            .as_object()
            .ok_or_else(|| KokoroError::Tokenizer("no vocab in tokenizer.json".into()))?;
        let mut vocab = std::collections::HashMap::new();
        for (token, id) in vocab_obj {
            vocab.insert(token.clone(), id.as_i64().unwrap_or(0));
        }

        let tokenizer = EspeakIpaTokenizer::new(vocab)?.with_model_max_length(max_length);
        Ok(KokoroTTS { session: Mutex::new(session), tokenizer, sample_rate })
    }

    pub fn generate_speech(
        &self,
        text: &str,
        voice_style: &VoiceStyle,
        speed: f32,
    ) -> Result<GeneratedAudio> {
        let tokens = self.tokenizer.encode(text, None)?;
        self.generate_from_tokens(&tokens, voice_style, speed)
    }

    pub fn generate_speech_from_phonemes(
        &self,
        phonemes: &str,
        voice_style: &VoiceStyle,
        speed: f32,
    ) -> Result<GeneratedAudio> {
        let tokens = self.tokenizer.encode_phonemes(phonemes, None)?;
        self.generate_from_tokens(&tokens, voice_style, speed)
    }

    pub fn generate_from_tokens(
        &self,
        tokens: &[i64],
        voice_style: &VoiceStyle,
        speed: f32,
    ) -> Result<GeneratedAudio> {
        // Style vector is indexed by token length (reference implementation
        // semantics preserved from kokoroxide).
        let style_vector = voice_style.get_style_vector_for_token_length(tokens.len(), 256);

        let input_ids = Tensor::from_array(([1usize, tokens.len()], tokens.to_vec()))?;
        let style = Tensor::from_array(([1usize, 256usize], style_vector))?;
        let speed_t = Tensor::from_array(([1usize], vec![speed]))?;

        let mut session = self.session.lock().expect("kokoro session lock");
        // The Kokoro export takes (input_ids, style, speed) in this order.
        let outputs = session.run(ort::inputs![input_ids, style, speed_t])?;

        let (_, data) = outputs[0].try_extract_tensor::<f32>()?;
        let samples = data.to_vec();
        let duration_seconds = samples.len() as f32 / self.sample_rate as f32;
        Ok(GeneratedAudio { samples, sample_rate: self.sample_rate, duration_seconds })
    }
}
