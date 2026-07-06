//! Kokoro-82M ONNX text-to-speech.
//!
//! Vendored from `kokoroxide` v0.1.5 by Dhruv Chaudhary
//! (https://github.com/dhruv304c2/kokoroxide, MIT OR Apache-2.0) and ported
//! from the yanked `ort` 1.16 line to `ort` 2.x. Pipeline:
//!
//! ```text
//! text --espeak-ng--> IPA --misaki mapping--> phonemes --vocab--> token ids
//!      --ONNX (input_ids, style, speed)--> 24 kHz f32 samples
//! ```

mod g2p;
mod ipa_tokenizer;
mod tts;
mod voice;

pub use g2p::EspeakG2P;
pub use ipa_tokenizer::EspeakIpaTokenizer;
pub use tts::{GeneratedAudio, KokoroTTS, TTSConfig};
pub use voice::{load_voice_style, VoiceStyle};

#[derive(Debug, thiserror::Error)]
pub enum KokoroError {
    #[error("espeak-ng: {0}")]
    Espeak(String),
    #[error("tokenizer: {0}")]
    Tokenizer(String),
    #[error("onnx: {0}")]
    Onnx(#[from] ort::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, KokoroError>;
