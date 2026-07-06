//! Portable engines shared by macOS and Linux.
//!
//! Feature map (see Cargo.toml): `audio` (cpal capture/playback),
//! `stt-whisper` (whisper.cpp), `llm-llama` (llama.cpp cleanup),
//! `llm-ollama` (local Ollama), `tts-kokoro` (Kokoro-82M ONNX),
//! `tts-piper` (Piper subprocess), `tts-system` (`say` / speech-dispatcher).
//!
//! Nothing in this crate performs network I/O except the Ollama client,
//! which is hard-restricted to loopback addresses (it talks to a server the
//! user runs on their own machine — still 100% on-device inference).

#[cfg(feature = "audio")]
pub mod audio;

#[cfg(feature = "stt-whisper")]
pub mod stt_whisper;

#[cfg(feature = "llm-llama")]
pub mod clean_llama;

#[cfg(feature = "llm-ollama")]
pub mod clean_ollama;

#[cfg(feature = "tts-kokoro")]
pub mod tts_kokoro;

#[cfg(feature = "tts-piper")]
pub mod tts_piper;

#[cfg(feature = "tts-system")]
pub mod tts_system;

pub mod textseg;
