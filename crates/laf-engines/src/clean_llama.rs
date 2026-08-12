//! LLM cleanup tier on llama.cpp via `llama-cpp-2` (pinned =0.1.146; the
//! crate tracks llama.cpp closely, so the generation loop below mirrors the
//! upstream `simple` example exactly).
//!
//! The model is a small local instruct GGUF (default: Qwen2.5-3B-Instruct
//! Q4_K_M). Prompting uses the model's own chat template read from GGUF
//! metadata, falling back to ChatML (which Qwen2.5 uses natively).

use encoding_rs::UTF_8;
use laf_core::modes::build_system_prompt;
use laf_core::traits::{CleanContext, TextCleaner};
use laf_core::types::{EngineError, EngineResult};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

fn backend() -> EngineResult<&'static LlamaBackend> {
    static BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();
    let r = BACKEND.get_or_init(|| {
        llama_cpp_2::send_logs_to_tracing(
            llama_cpp_2::LogOptions::default().with_logs_enabled(false),
        );
        LlamaBackend::init().map_err(|e| e.to_string())
    });
    r.as_ref().map_err(|e| EngineError::Cleanup(format!("llama backend init: {e}")))
}

pub struct LlamaCleaner {
    model_path: PathBuf,
    model_label: String,
    n_ctx: u32,
    model: Mutex<Option<LlamaModel>>,
}

impl LlamaCleaner {
    pub fn new(model_path: PathBuf, model_label: impl Into<String>) -> Self {
        Self { model_path, model_label: model_label.into(), n_ctx: 4096, model: Mutex::new(None) }
    }

    fn generate(&self, system: &str, user: &str) -> EngineResult<String> {
        let backend = backend()?;
        let mut guard = self.model.lock().expect("llama model lock");
        if guard.is_none() {
            let t0 = Instant::now();
            let params = LlamaModelParams::default();
            let model = LlamaModel::load_from_file(backend, &self.model_path, &params)
                .map_err(|e| EngineError::Cleanup(format!("load GGUF model: {e}")))?;
            tracing::info!(
                "loaded cleanup model '{}' in {} ms",
                self.model_label,
                t0.elapsed().as_millis()
            );
            *guard = Some(model);
        }
        let model = guard.as_ref().expect("model just loaded");

        // Build the prompt with the model's own chat template when possible.
        let messages = vec![
            LlamaChatMessage::new("system".into(), system.into())
                .map_err(|e| EngineError::Cleanup(format!("chat message: {e}")))?,
            LlamaChatMessage::new("user".into(), user.into())
                .map_err(|e| EngineError::Cleanup(format!("chat message: {e}")))?,
        ];
        let prompt = match model
            .chat_template(None)
            .map_err(|e| tracing::debug!("no chat template in GGUF: {e}"))
            .ok()
            .and_then(|tmpl| model.apply_chat_template(&tmpl, &messages, true).ok())
        {
            Some(p) => p,
            // ChatML fallback (native for Qwen2.5).
            None => format!(
                "<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n"
            ),
        };

        let mut ctx = model
            .new_context(
                backend,
                LlamaContextParams::default()
                    .with_n_ctx(NonZeroU32::new(self.n_ctx))
                    .with_n_threads(effective_threads())
                    .with_n_threads_batch(effective_threads()),
            )
            .map_err(|e| EngineError::Cleanup(format!("llama context: {e}")))?;

        let tokens = model
            .str_to_token(&prompt, AddBos::Never)
            .map_err(|e| EngineError::Cleanup(format!("tokenize: {e}")))?;
        if tokens.len() as u32 + 16 > self.n_ctx {
            return Err(EngineError::Cleanup(format!(
                "dictation too long for the cleanup model context ({} tokens)",
                tokens.len()
            )));
        }
        // Output budget. Cleanup output is roughly the SAME length as the
        // input (fillers trimmed; punctuation/capitalization and per-mode
        // formatting add a little), so the cap must be ≳ the input length or a
        // long dictation gets truncated mid-sentence. Allow ~1.25× input plus a
        // margin — above normal cleanup output, still comfortably under "double
        // the input" to bound runaway generation, and always within the context
        // window. (Was `tokens.len() / 2 + 96`, i.e. HALF the input, which
        // silently cut the tail off any dictation longer than ~190 tokens.)
        let max_out =
            (tokens.len() + tokens.len() / 4 + 96).min((self.n_ctx as usize) - tokens.len() - 8);

        let mut batch = LlamaBatch::new(self.n_ctx as usize, 1);
        let last_index = tokens.len() as i32 - 1;
        for (i, token) in (0i32..).zip(tokens.into_iter()) {
            batch
                .add(token, i, &[0], i == last_index)
                .map_err(|e| EngineError::Cleanup(format!("batch add: {e}")))?;
        }
        ctx.decode(&mut batch).map_err(|e| EngineError::Cleanup(format!("prompt eval: {e}")))?;

        // Deterministic sampling: formatting is a transformation, not a
        // creative task.
        let mut sampler = LlamaSampler::chain_simple([LlamaSampler::greedy()]);
        let mut decoder = UTF_8.new_decoder();
        let mut out = String::new();
        let mut n_cur = batch.n_tokens();
        for _ in 0..max_out {
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(token);
            if model.is_eog_token(token) {
                break;
            }
            match model.token_to_piece(token, &mut decoder, false, None) {
                Ok(piece) => out.push_str(&piece),
                Err(e) => {
                    tracing::debug!("token_to_piece: {e}");
                }
            }
            batch.clear();
            batch
                .add(token, n_cur, &[0], true)
                .map_err(|e| EngineError::Cleanup(format!("batch add: {e}")))?;
            n_cur += 1;
            ctx.decode(&mut batch).map_err(|e| EngineError::Cleanup(format!("decode: {e}")))?;
        }
        Ok(out.trim().to_string())
    }
}

impl TextCleaner for LlamaCleaner {
    fn clean(&self, raw: &str, ctx: &CleanContext) -> EngineResult<String> {
        let system = build_system_prompt(ctx.mode);
        let out = self.generate(&system, raw)?;
        if out.is_empty() {
            return Err(EngineError::Cleanup("model returned empty output".into()));
        }
        Ok(out)
    }

    fn name(&self) -> &'static str {
        "llama.cpp"
    }

    fn available(&self) -> bool {
        self.model_path.is_file()
    }

    fn unload(&self) {
        let mut guard = self.model.lock().expect("llama model lock");
        if guard.take().is_some() {
            tracing::info!("unloaded cleanup model '{}'", self.model_label);
        }
    }
}

fn effective_threads() -> i32 {
    std::thread::available_parallelism().map(|n| n.get().saturating_sub(1)).unwrap_or(4).clamp(1, 8)
        as i32
}
