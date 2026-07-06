//! Model manager: the ONLY code in Local AI Flow that may touch the network,
//! and it does so exclusively for explicit, user-initiated model downloads
//! from Hugging Face. Checksums and sizes are pinned below; downloads are
//! streamed to `<file>.part`, SHA-256-verified, then renamed into place.
//!
//! "Fully Offline" is enforced twice:
//! * runtime: [`ModelManager::set_offline`] makes `download` return
//!   [`EngineError::OfflineMode`];
//! * build time: compiling `laf-core` without the `online` feature removes
//!   the download code (and the reqwest dependency) entirely.
//!
//! Bundled models: a secondary read-only directory (e.g. inside the app
//! bundle / AppImage) is consulted when a model is not present in the user
//! data dir, so a "Fully Offline" install with pre-bundled weights works
//! without any first-run download.

use crate::settings::data_dir;
use crate::types::{EngineError, EngineResult};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    Stt,
    Cleaner,
    TtsModel,
    TtsTokenizer,
    TtsVoice,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: ModelKind,
    /// Direct download URL (Hugging Face `resolve` endpoint).
    pub url: &'static str,
    /// Path relative to `<data>/models/`.
    pub filename: &'static str,
    /// Pinned SHA-256 (lowercase hex). `None` only for tiny non-LFS text
    /// assets whose hash is recorded on first download instead.
    pub sha256: Option<&'static str>,
    pub size_bytes: Option<u64>,
    pub license: &'static str,
    /// Rough guidance shown in the UI.
    pub note: &'static str,
}

/// Pinned registry. SHA-256 values were read from the Hugging Face LFS
/// pointers on 2026-07-06.
pub const REGISTRY: &[ModelSpec] = &[
    // ---- Whisper (ggerganov/whisper.cpp conversions; models MIT) ----
    ModelSpec {
        id: "whisper-large-v3-turbo-q5",
        label: "Whisper large-v3-turbo (Q5_0) — recommended",
        kind: ModelKind::Stt,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
        filename: "whisper/ggml-large-v3-turbo-q5_0.bin",
        sha256: Some("394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2"),
        size_bytes: Some(574_041_195),
        license: "MIT",
        note: "Best accuracy/latency balance; ~1.1 GB RAM. Multilingual.",
    },
    ModelSpec {
        id: "whisper-small-q5",
        label: "Whisper small (Q5_1)",
        kind: ModelKind::Stt,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin",
        filename: "whisper/ggml-small-q5_1.bin",
        sha256: Some("ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb"),
        size_bytes: Some(190_085_487),
        license: "MIT",
        note: "Good accuracy on mid-range machines. Multilingual.",
    },
    ModelSpec {
        id: "whisper-base-q5",
        label: "Whisper base (Q5_1)",
        kind: ModelKind::Stt,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base-q5_1.bin",
        filename: "whisper/ggml-base-q5_1.bin",
        sha256: Some("422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898"),
        size_bytes: Some(59_707_625),
        license: "MIT",
        note: "Low-end machines; noticeably lower accuracy.",
    },
    ModelSpec {
        id: "whisper-tiny-q5",
        label: "Whisper tiny (Q5_1)",
        kind: ModelKind::Stt,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny-q5_1.bin",
        filename: "whisper/ggml-tiny-q5_1.bin",
        sha256: Some("818710568da3ca15689e31a743197b520007872ff9576237bda97bd1b469c3d7"),
        size_bytes: Some(32_152_673),
        license: "MIT",
        note: "Fastest; testing / very constrained hardware.",
    },
    // ---- Cleanup LLMs (GGUF) ----
    ModelSpec {
        id: "qwen2.5-3b-instruct-q4",
        label: "Qwen2.5-3B-Instruct (Q4_K_M) — recommended cleaner",
        kind: ModelKind::Cleaner,
        url: "https://huggingface.co/Qwen/Qwen2.5-3B-Instruct-GGUF/resolve/main/qwen2.5-3b-instruct-q4_k_m.gguf",
        filename: "llm/qwen2.5-3b-instruct-q4_k_m.gguf",
        sha256: Some("626b4a6678b86442240e33df819e00132d3ba7dddfe1cdc4fbb18e0a9615c62d"),
        size_bytes: Some(2_104_932_768),
        license: "Qwen Research License (review before commercial use)",
        note: "~2.6 GB RAM. Best formatting quality of the small models.",
    },
    ModelSpec {
        id: "qwen2.5-1.5b-instruct-q4",
        label: "Qwen2.5-1.5B-Instruct (Q4_K_M) — light cleaner",
        kind: ModelKind::Cleaner,
        url: "https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/qwen2.5-1.5b-instruct-q4_k_m.gguf",
        filename: "llm/qwen2.5-1.5b-instruct-q4_k_m.gguf",
        sha256: Some("6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e"),
        size_bytes: Some(1_117_320_736),
        license: "Apache-2.0",
        note: "Low-end machines; Apache-2.0 licensed.",
    },
    // ---- Kokoro TTS (Apache-2.0) ----
    ModelSpec {
        id: "kokoro-v1-q8",
        label: "Kokoro-82M v1.0 (ONNX, quantized)",
        kind: ModelKind::TtsModel,
        url: "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main/onnx/model_quantized.onnx",
        filename: "kokoro/model_quantized.onnx",
        sha256: Some("fbae9257e1e05ffc727e951ef9b9c98418e6d79f1c9b6b13bd59f5c9028a1478"),
        size_bytes: Some(92_361_116),
        license: "Apache-2.0",
        note: "24 kHz neural voice, faster than real time on CPU.",
    },
    ModelSpec {
        id: "kokoro-tokenizer",
        label: "Kokoro tokenizer",
        kind: ModelKind::TtsTokenizer,
        url: "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main/tokenizer.json",
        filename: "kokoro/tokenizer.json",
        sha256: None, // small non-LFS asset; hash recorded on first download
        size_bytes: None,
        license: "Apache-2.0",
        note: "Required by the Kokoro engine.",
    },
    ModelSpec {
        id: "kokoro-voice-af-heart",
        label: "Voice: af_heart (US English, female)",
        kind: ModelKind::TtsVoice,
        url: "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main/voices/af_heart.bin",
        filename: "kokoro/voices/af_heart.bin",
        sha256: Some("d583ccff3cdca2f7fae535cb998ac07e9fcb90f09737b9a41fa2734ec44a8f0b"),
        size_bytes: Some(522_240),
        license: "Apache-2.0",
        note: "Default voice.",
    },
    ModelSpec {
        id: "kokoro-voice-bf-emma",
        label: "Voice: bf_emma (British English, female)",
        kind: ModelKind::TtsVoice,
        url: "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main/voices/bf_emma.bin",
        filename: "kokoro/voices/bf_emma.bin",
        sha256: Some("669fe0647f9dd04fcab92f1439a40eeb4c8b4ab1f82e4996fe3d918ce4a63b73"),
        size_bytes: Some(522_240),
        license: "Apache-2.0",
        note: "Alternative voice.",
    },
];

/// The set a fresh install needs for the recommended experience.
pub const RECOMMENDED_SET: &[&str] = &[
    "whisper-large-v3-turbo-q5",
    "qwen2.5-3b-instruct-q4",
    "kokoro-v1-q8",
    "kokoro-tokenizer",
    "kokoro-voice-af-heart",
];

/// Everything the Kokoro engine needs on disk.
pub const KOKORO_GROUP: &[&str] = &["kokoro-v1-q8", "kokoro-tokenizer", "kokoro-voice-af-heart"];

pub fn spec(id: &str) -> Option<&'static ModelSpec> {
    REGISTRY.iter().find(|m| m.id == id)
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
    pub spec: ModelSpec,
    pub installed: bool,
    pub bundled: bool,
    pub path: Option<PathBuf>,
    pub bytes_on_disk: Option<u64>,
}

pub struct ModelManager {
    user_root: PathBuf,
    /// Read-only directory of pre-bundled models (inside app bundle).
    bundled_root: RwLock<Option<PathBuf>>,
    offline: AtomicBool,
}

impl ModelManager {
    pub fn new() -> Self {
        Self::with_root(data_dir().join("models"))
    }

    pub fn with_root(user_root: PathBuf) -> Self {
        Self { user_root, bundled_root: RwLock::new(None), offline: AtomicBool::new(false) }
    }

    pub fn set_bundled_root(&self, dir: Option<PathBuf>) {
        *self.bundled_root.write().expect("bundled_root lock") = dir;
    }

    pub fn set_offline(&self, offline: bool) {
        self.offline.store(offline, Ordering::SeqCst);
    }

    pub fn is_offline(&self) -> bool {
        self.offline.load(Ordering::SeqCst)
    }

    pub fn user_root(&self) -> &Path {
        &self.user_root
    }

    /// Resolve an installed model file: user dir first, then bundled dir.
    pub fn resolve(&self, id: &str) -> Option<PathBuf> {
        let spec = spec(id)?;
        let user = self.user_root.join(spec.filename);
        if user.is_file() {
            return Some(user);
        }
        let bundled = self.bundled_root.read().expect("bundled_root lock");
        if let Some(root) = bundled.as_ref() {
            let p = root.join(spec.filename);
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }

    /// Like [`resolve`](Self::resolve) but with an actionable error.
    pub fn require(&self, id: &str) -> EngineResult<PathBuf> {
        self.resolve(id).ok_or_else(|| EngineError::ModelMissing {
            model_id: id.to_string(),
            hint: if self.is_offline() {
                " (Fully Offline mode is on: place the file in the models folder or use a build with bundled models)".into()
            } else {
                " (open Settings → Models to download it)".into()
            },
        })
    }

    pub fn list(&self) -> Vec<ModelStatus> {
        REGISTRY
            .iter()
            .map(|m| {
                let user_path = self.user_root.join(m.filename);
                let bundled_path = self
                    .bundled_root
                    .read()
                    .expect("bundled_root lock")
                    .as_ref()
                    .map(|r| r.join(m.filename))
                    .filter(|p| p.is_file());
                let path = if user_path.is_file() {
                    Some(user_path.clone())
                } else {
                    bundled_path.clone()
                };
                ModelStatus {
                    spec: m.clone(),
                    installed: path.is_some(),
                    bundled: bundled_path.is_some() && !user_path.is_file(),
                    bytes_on_disk: path.as_ref().and_then(|p| p.metadata().ok()).map(|md| md.len()),
                    path,
                }
            })
            .collect()
    }

    /// Delete a model from the *user* dir (bundled files are read-only).
    pub fn delete(&self, id: &str) -> EngineResult<()> {
        let spec = spec(id).ok_or_else(|| EngineError::Other(format!("unknown model '{id}'")))?;
        let path = self.user_root.join(spec.filename);
        if path.is_file() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// Verify size + SHA-256 of an installed model against the registry.
    pub fn verify(&self, id: &str) -> EngineResult<bool> {
        let spec = spec(id).ok_or_else(|| EngineError::Other(format!("unknown model '{id}'")))?;
        let Some(path) = self.resolve(id) else { return Ok(false) };
        if let Some(expected_size) = spec.size_bytes {
            if path.metadata()?.len() != expected_size {
                return Ok(false);
            }
        }
        if let Some(expected_sha) = spec.sha256 {
            let actual = sha256_file(&path)?;
            return Ok(actual == expected_sha);
        }
        Ok(true)
    }
}

impl Default for ModelManager {
    fn default() -> Self {
        Self::new()
    }
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

// --------------------------------------------------------------------------
// Download support — the single network code path in the application.
// Compiled only with the `online` feature; refuses to run in offline mode.
// --------------------------------------------------------------------------
#[cfg(feature = "online")]
mod download {
    use super::*;
    use futures_util::StreamExt;
    use sha2::{Digest, Sha256};

    impl ModelManager {
        /// Download `id` with streaming SHA-256 verification.
        /// `progress(downloaded_bytes, total_bytes)` is called periodically.
        ///
        /// NETWORK ACCESS JUSTIFICATION: this is the one-time, user-triggered
        /// model-asset download described in the privacy policy (README
        /// "Privacy"). No other code in this workspace opens a socket to the
        /// public internet.
        pub async fn download(
            &self,
            id: &str,
            progress: impl Fn(u64, u64) + Send + Sync + 'static,
        ) -> EngineResult<PathBuf> {
            if self.is_offline() {
                return Err(EngineError::OfflineMode);
            }
            let spec =
                spec(id).ok_or_else(|| EngineError::Other(format!("unknown model '{id}'")))?;
            let dest = self.user_root.join(spec.filename);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let part = dest.with_extension("part");

            let client = reqwest::Client::builder()
                .user_agent(concat!("LocalAIFlow/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|e| EngineError::Other(format!("http client: {e}")))?;
            let resp = client
                .get(spec.url)
                .send()
                .await
                .map_err(|e| EngineError::Other(format!("download failed: {e}")))?
                .error_for_status()
                .map_err(|e| EngineError::Other(format!("download failed: {e}")))?;

            let total = resp.content_length().or(spec.size_bytes).unwrap_or(0);
            let mut hasher = Sha256::new();
            let mut written: u64 = 0;
            {
                use tokio::io::AsyncWriteExt;
                let mut file = tokio::fs::File::create(&part).await?;
                let mut stream = resp.bytes_stream();
                while let Some(chunk) = stream.next().await {
                    let chunk =
                        chunk.map_err(|e| EngineError::Other(format!("download stream: {e}")))?;
                    hasher.update(&chunk);
                    file.write_all(&chunk).await?;
                    written += chunk.len() as u64;
                    progress(written, total);
                }
                file.flush().await?;
            }

            if let Some(expected_size) = spec.size_bytes {
                if written != expected_size {
                    let _ = std::fs::remove_file(&part);
                    return Err(EngineError::Other(format!(
                        "size mismatch for {id}: got {written}, expected {expected_size}"
                    )));
                }
            }
            let actual_sha = hex::encode(hasher.finalize());
            if let Some(expected_sha) = spec.sha256 {
                if actual_sha != expected_sha {
                    let _ = std::fs::remove_file(&part);
                    return Err(EngineError::Other(format!(
                        "checksum mismatch for {id}: file corrupted or tampered (got {actual_sha})"
                    )));
                }
            } else {
                // Record first-seen hash next to the file for future audits.
                let _ = std::fs::write(dest.with_extension("sha256"), &actual_sha);
            }
            std::fs::rename(&part, &dest)?;
            progress(written, written.max(total));
            Ok(dest)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_ids_unique_and_paths_relative() {
        let mut seen = std::collections::HashSet::new();
        for m in REGISTRY {
            assert!(seen.insert(m.id), "duplicate model id {}", m.id);
            assert!(!m.filename.starts_with('/'), "absolute path in {}", m.id);
            assert!(!m.filename.contains(".."), "path traversal in {}", m.id);
            assert!(m.url.starts_with("https://huggingface.co/"), "unexpected host for {}", m.id);
        }
        for id in RECOMMENDED_SET.iter().chain(KOKORO_GROUP) {
            assert!(spec(id).is_some(), "unknown id {id} in a group");
        }
    }

    #[test]
    fn resolve_prefers_user_over_bundled_and_verifies() {
        let user = tempfile::tempdir().unwrap();
        let bundled = tempfile::tempdir().unwrap();
        let mm = ModelManager::with_root(user.path().to_path_buf());
        mm.set_bundled_root(Some(bundled.path().to_path_buf()));

        let spec = spec("kokoro-voice-af-heart").unwrap();
        let bundled_file = bundled.path().join(spec.filename);
        std::fs::create_dir_all(bundled_file.parent().unwrap()).unwrap();
        std::fs::write(&bundled_file, b"bundled-bytes").unwrap();
        assert_eq!(mm.resolve("kokoro-voice-af-heart").unwrap(), bundled_file);
        assert!(mm.list().iter().any(|s| s.spec.id == "kokoro-voice-af-heart" && s.bundled));

        let user_file = user.path().join(spec.filename);
        std::fs::create_dir_all(user_file.parent().unwrap()).unwrap();
        std::fs::write(&user_file, b"user-bytes").unwrap();
        assert_eq!(mm.resolve("kokoro-voice-af-heart").unwrap(), user_file);

        // verify() fails on wrong size/hash.
        assert!(!mm.verify("kokoro-voice-af-heart").unwrap());
    }

    #[test]
    fn missing_model_error_is_actionable() {
        let dir = tempfile::tempdir().unwrap();
        let mm = ModelManager::with_root(dir.path().to_path_buf());
        mm.set_offline(true);
        let err = mm.require("whisper-tiny-q5").unwrap_err();
        assert!(err.to_string().contains("Fully Offline"));
    }

    #[test]
    fn sha256_matches_known_vector() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
