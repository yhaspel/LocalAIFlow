//! Streaming speech-to-text on whisper.cpp via `whisper-rs` 0.16.
//!
//! whisper.cpp has no incremental decoder, so streaming is implemented the
//! canonical way (as in whisper.cpp's own `stream` example): keep the audio
//! of the current VAD segment in a buffer and re-run `full()` over it at an
//! adaptive cadence for live partials; when the pipeline signals a segment
//! boundary (speech → silence), decode once more and emit a `Final`.
//!
//! Acceleration: build with `whisper-metal` / `whisper-coreml` on macOS and
//! `whisper-cuda` / `whisper-vulkan` on Linux; plain CPU works everywhere.

use crossbeam_channel::{unbounded, Receiver, Sender};
use laf_core::traits::{SpeechToText, SttSession, SttSessionConfig};
use laf_core::types::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Force a segment boundary if speech runs longer than this (bounds latency
/// and decode cost; whisper's window is 30 s).
const MAX_SEGMENT_SECS: f32 = 28.0;
/// Don't bother decoding less than this much audio.
const MIN_DECODE_SECS: f32 = 0.4;

pub struct WhisperEngine {
    model_path: PathBuf,
    model_label: String,
    ctx: Mutex<Option<Arc<WhisperContext>>>,
}

impl WhisperEngine {
    pub fn new(model_path: PathBuf, model_label: impl Into<String>) -> Self {
        Self { model_path, model_label: model_label.into(), ctx: Mutex::new(None) }
    }

    fn ensure_ctx(&self) -> EngineResult<Arc<WhisperContext>> {
        let mut guard = self.ctx.lock().expect("whisper ctx lock");
        if let Some(ctx) = guard.as_ref() {
            return Ok(ctx.clone());
        }
        let path = self
            .model_path
            .to_str()
            .ok_or_else(|| EngineError::Stt("model path is not valid UTF-8".into()))?;
        let t0 = Instant::now();
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|e| EngineError::Stt(format!("failed to load whisper model: {e}")))?;
        tracing::info!(
            "loaded whisper model '{}' in {} ms",
            self.model_label,
            t0.elapsed().as_millis()
        );
        let ctx = Arc::new(ctx);
        *guard = Some(ctx.clone());
        Ok(ctx)
    }
}

enum WorkerMsg {
    Audio(Vec<f32>),
    Boundary,
    Flush,
}

pub struct WhisperSession {
    tx: Option<Sender<WorkerMsg>>,
    events_rx: Receiver<SttEvent>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl SpeechToText for WhisperEngine {
    fn start_session(&self, cfg: SttSessionConfig) -> EngineResult<Box<dyn SttSession>> {
        let ctx = self.ensure_ctx()?;
        let (tx, rx) = unbounded::<WorkerMsg>();
        let (ev_tx, ev_rx) = unbounded::<SttEvent>();
        let worker = std::thread::Builder::new()
            .name("laf-whisper".into())
            .spawn(move || worker_loop(ctx, cfg, rx, ev_tx))
            .map_err(|e| EngineError::Stt(format!("spawn whisper worker: {e}")))?;
        Ok(Box::new(WhisperSession { tx: Some(tx), events_rx: ev_rx, worker: Some(worker) }))
    }

    fn info(&self) -> EngineInfo {
        EngineInfo {
            name: "whisper",
            model: Some(self.model_label.clone()),
            // Metal is enabled per-target in Cargo.toml (macOS builds always
            // have it); these cfgs cover the opt-in accelerators.
            accelerated: cfg!(target_os = "macos")
                || cfg!(any(
                    feature = "whisper-coreml",
                    feature = "whisper-cuda",
                    feature = "whisper-vulkan"
                )),
        }
    }

    fn unload(&self) {
        let mut guard = self.ctx.lock().expect("whisper ctx lock");
        if guard.take().is_some() {
            tracing::info!("unloaded whisper model '{}'", self.model_label);
        }
    }
}

impl SttSession for WhisperSession {
    fn feed(&mut self, pcm: &[f32]) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(WorkerMsg::Audio(pcm.to_vec()));
        }
    }

    fn segment_boundary(&mut self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(WorkerMsg::Boundary);
        }
    }

    fn drain_events(&mut self) -> Vec<SttEvent> {
        self.events_rx.try_iter().collect()
    }

    fn finalize(&mut self) -> EngineResult<Vec<SttEvent>> {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(WorkerMsg::Flush);
            drop(tx); // hang up so the worker exits after the flush
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        Ok(self.events_rx.try_iter().collect())
    }
}

impl Drop for WhisperSession {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

fn worker_loop(
    ctx: Arc<WhisperContext>,
    cfg: SttSessionConfig,
    rx: Receiver<WorkerMsg>,
    ev_tx: Sender<SttEvent>,
) {
    let mut state = match ctx.create_state() {
        Ok(s) => s,
        Err(e) => {
            let _ = ev_tx.send(SttEvent::Error { message: format!("whisper state: {e}") });
            return;
        }
    };
    let threads = effective_threads(cfg.max_threads);
    let lang: Option<String> =
        if cfg.language == "auto" { Some("auto".to_string()) } else { Some(cfg.language.clone()) };
    let hints = if cfg.vocabulary_hints.is_empty() {
        None
    } else {
        // whisper.cpp treats the initial prompt as prior context, biasing the
        // decoder toward this vocabulary — the supported way to pass hints.
        Some(cfg.vocabulary_hints.join(", "))
    };

    let mut buffer: Vec<f32> = Vec::with_capacity(STT_SAMPLE_RATE as usize * 30);
    let mut segment_offset_ms: u64 = 0;
    let mut decoded_upto: usize = 0; // samples decoded in the last partial
    let mut last_decode_cost_ms: u64 = 200;
    let mut last_decode_at = Instant::now();

    let decode = |state: &mut whisper_rs::WhisperState,
                  buf: &[f32],
                  lang: &Option<String>,
                  hints: &Option<String>|
     -> Result<String, String> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(threads);
        if let Some(l) = lang.as_deref() {
            params.set_language(Some(l));
        }
        params.set_translate(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_no_context(true);
        params.set_suppress_blank(true);
        if let Some(h) = hints.as_deref() {
            params.set_initial_prompt(h);
        }
        state.full(params, buf).map_err(|e| format!("whisper full(): {e}"))?;
        let mut text = String::new();
        for i in 0..state.full_n_segments() {
            if let Some(seg) = state.get_segment(i) {
                if let Ok(t) = seg.to_str_lossy() {
                    text.push_str(&t);
                }
            }
        }
        Ok(text.trim().to_string())
    };

    let finalize_segment =
        |state: &mut whisper_rs::WhisperState, buffer: &mut Vec<f32>, offset: &mut u64, decoded_upto: &mut usize| {
            if (buffer.len() as f32) < MIN_DECODE_SECS * STT_SAMPLE_RATE as f32 {
                *offset += (buffer.len() as u64 * 1000) / STT_SAMPLE_RATE as u64;
                buffer.clear();
                *decoded_upto = 0;
                return;
            }
            let dur_ms = (buffer.len() as u64 * 1000) / STT_SAMPLE_RATE as u64;
            match decode(state, buffer, &lang, &hints) {
                Ok(text) if !text.is_empty() => {
                    let _ = ev_tx.send(SttEvent::Final {
                        text,
                        t0_ms: *offset,
                        t1_ms: *offset + dur_ms,
                    });
                }
                Ok(_) => {}
                Err(e) => {
                    let _ = ev_tx.send(SttEvent::Error { message: e });
                }
            }
            *offset += dur_ms;
            buffer.clear();
            *decoded_upto = 0;
        };

    loop {
        let msg = match rx.recv() {
            Ok(m) => m,
            Err(_) => break, // pipeline hung up without flush — just exit
        };
        match msg {
            WorkerMsg::Audio(pcm) => {
                buffer.extend_from_slice(&pcm);
                if buffer.len() as f32 > MAX_SEGMENT_SECS * STT_SAMPLE_RATE as f32 {
                    finalize_segment(&mut state, &mut buffer, &mut segment_offset_ms, &mut decoded_upto);
                    continue;
                }
                // Adaptive partial cadence: at least MIN_DECODE new audio and
                // at least the cost of the previous decode between runs.
                let new_samples = buffer.len().saturating_sub(decoded_upto);
                let min_gap_ms = last_decode_cost_ms.clamp(300, 1500);
                let enough_new = new_samples as f32 >= 0.5 * STT_SAMPLE_RATE as f32;
                let long_enough = buffer.len() as f32 >= MIN_DECODE_SECS * STT_SAMPLE_RATE as f32;
                if long_enough && enough_new && last_decode_at.elapsed().as_millis() as u64 >= min_gap_ms {
                    let t0 = Instant::now();
                    match decode(&mut state, &buffer, &lang, &hints) {
                        Ok(text) => {
                            if !text.is_empty() {
                                let _ = ev_tx.send(SttEvent::Partial { text });
                            }
                        }
                        Err(e) => {
                            let _ = ev_tx.send(SttEvent::Error { message: e });
                        }
                    }
                    last_decode_cost_ms = t0.elapsed().as_millis() as u64;
                    last_decode_at = Instant::now();
                    decoded_upto = buffer.len();
                }
            }
            WorkerMsg::Boundary => {
                finalize_segment(&mut state, &mut buffer, &mut segment_offset_ms, &mut decoded_upto);
            }
            WorkerMsg::Flush => {
                finalize_segment(&mut state, &mut buffer, &mut segment_offset_ms, &mut decoded_upto);
                break;
            }
        }
    }
}

fn effective_threads(requested: usize) -> i32 {
    let n = if requested == 0 {
        std::thread::available_parallelism().map(|n| n.get().saturating_sub(1)).unwrap_or(4)
    } else {
        requested
    };
    n.clamp(1, 8) as i32
}
