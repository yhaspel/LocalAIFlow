//! cpal-based audio: microphone capture (CoreAudio on macOS, ALSA/PipeWire on
//! Linux) resampled to 16 kHz mono, plus a small PCM playback sink for TTS.
//!
//! cpal `Stream`s are not `Send`, so each capture/playback owns a dedicated
//! thread that builds the stream, keeps it alive, and tears it down on a stop
//! signal. The public structs stay `Send` and satisfy the core traits.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{bounded, Sender};
use laf_core::resample::Resampler;
use laf_core::traits::AudioCapture;
use laf_core::types::{AudioFrame, EngineError, EngineResult};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// ~64 ms frames keep HUD levels lively and STT feeding smooth.
const FRAME_SAMPLES_16K: usize = 1024;

pub struct CpalCapture {
    device_name: Option<String>,
    stop_tx: Option<Sender<()>>,
    worker: Option<std::thread::JoinHandle<()>>,
    running: Arc<AtomicBool>,
}

impl CpalCapture {
    pub fn new() -> Self {
        Self { device_name: None, stop_tx: None, worker: None, running: Arc::new(AtomicBool::new(false)) }
    }
}

impl Default for CpalCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioCapture for CpalCapture {
    fn start(&mut self, sink: Sender<AudioFrame>) -> EngineResult<String> {
        if self.is_running() {
            self.stop();
        }
        let (stop_tx, stop_rx) = bounded::<()>(1);
        let (ready_tx, ready_rx) = bounded::<Result<String, String>>(1);
        let wanted = self.device_name.clone();
        let running = self.running.clone();

        let worker = std::thread::Builder::new()
            .name("laf-capture".into())
            .spawn(move || {
                let host = cpal::default_host();
                let device = match wanted
                    .as_deref()
                    .and_then(|w| {
                        host.input_devices().ok().and_then(|mut it| {
                            it.find(|d| d.description().is_ok_and(|desc| desc.to_string() == w))
                        })
                    })
                    .or_else(|| host.default_input_device())
                {
                    Some(d) => d,
                    None => {
                        let _ = ready_tx.send(Err(
                            "no microphone available (check OS audio settings / permission)".into(),
                        ));
                        return;
                    }
                };
                let name = device
                    .description()
                    .map(|d| d.to_string())
                    .unwrap_or_else(|_| "unknown input".into());
                let supported = match device.default_input_config() {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("no default input config: {e}")));
                        return;
                    }
                };
                let sample_format = supported.sample_format();
                let stream_cfg: cpal::StreamConfig = supported.config();
                let resampler =
                    Arc::new(Mutex::new(Resampler::new(stream_cfg.sample_rate, stream_cfg.channels)));
                let acc: Arc<Mutex<Vec<f32>>> =
                    Arc::new(Mutex::new(Vec::with_capacity(FRAME_SAMPLES_16K * 2)));

                // Closure is Clone: captures only Arc + Sender.
                let deliver = {
                    let acc = acc.clone();
                    move |mono: Vec<f32>| {
                        let mut acc = acc.lock().expect("acc lock");
                        acc.extend_from_slice(&mono);
                        while acc.len() >= FRAME_SAMPLES_16K {
                            let frame: Vec<f32> = acc.drain(..FRAME_SAMPLES_16K).collect();
                            let _ = sink.send(AudioFrame::from_samples(frame));
                        }
                    }
                };
                let err_fn = |e| tracing::error!("input stream error: {e}");

                macro_rules! build_stream {
                    ($t:ty, $conv:expr) => {{
                        let deliver = deliver.clone();
                        let resampler = resampler.clone();
                        device.build_input_stream(
                            stream_cfg.clone(),
                            move |data: &[$t], _: &cpal::InputCallbackInfo| {
                                let floats: Vec<f32> = data.iter().map($conv).collect();
                                let mono =
                                    resampler.lock().expect("resampler lock").process(&floats);
                                if !mono.is_empty() {
                                    deliver(mono);
                                }
                            },
                            err_fn,
                            None,
                        )
                    }};
                }

                let stream = match sample_format {
                    cpal::SampleFormat::F32 => build_stream!(f32, |s: &f32| *s),
                    cpal::SampleFormat::I16 => build_stream!(i16, |s: &i16| *s as f32 / 32768.0),
                    cpal::SampleFormat::U16 => {
                        build_stream!(u16, |s: &u16| (*s as f32 - 32768.0) / 32768.0)
                    }
                    cpal::SampleFormat::I32 => {
                        build_stream!(i32, |s: &i32| *s as f32 / 2_147_483_648.0)
                    }
                    other => {
                        let _ = ready_tx.send(Err(format!("unsupported sample format {other:?}")));
                        return;
                    }
                };
                let stream = match stream {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("failed to open input stream: {e}")));
                        return;
                    }
                };
                if let Err(e) = stream.play() {
                    let _ = ready_tx.send(Err(format!("failed to start input stream: {e}")));
                    return;
                }
                running.store(true, Ordering::SeqCst);
                let _ = ready_tx.send(Ok(name));
                let _ = stop_rx.recv(); // park until stop / sender dropped
                running.store(false, Ordering::SeqCst);
                drop(stream);
            })
            .map_err(|e| EngineError::Audio(format!("spawn capture thread: {e}")))?;

        match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(name)) => {
                self.stop_tx = Some(stop_tx);
                self.worker = Some(worker);
                Ok(name)
            }
            Ok(Err(msg)) => Err(EngineError::Audio(msg)),
            Err(_) => Err(EngineError::Audio("audio device did not start in time".into())),
        }
    }

    fn stop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn list_devices(&self) -> Vec<String> {
        let host = cpal::default_host();
        let mut out = Vec::new();
        if let Some(d) = host.default_input_device() {
            if let Ok(desc) = d.description() {
                out.push(desc.to_string());
            }
        }
        if let Ok(devices) = host.input_devices() {
            for d in devices {
                if let Ok(desc) = d.description() {
                    let n = desc.to_string();
                    if !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
        }
        out
    }

    fn select_device(&mut self, name: Option<String>) {
        self.device_name = name;
    }
}

impl Drop for CpalCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// Playback sink for TTS
// ---------------------------------------------------------------------------

/// Producer half: push mono f32 at `src_rate`, then call `finish()`.
#[derive(Clone)]
pub struct PcmWriter {
    queue: Arc<Mutex<VecDeque<f32>>>,
    done: Arc<AtomicBool>,
}

impl PcmWriter {
    pub fn push(&self, samples: &[f32]) {
        self.queue.lock().expect("pcm queue").extend(samples.iter().copied());
    }
    pub fn finish(&self) {
        self.done.store(true, Ordering::SeqCst);
    }
}

/// Control half — stop / finished semantics for TTS playback handles.
pub struct PcmControl {
    stop_tx: Option<Sender<()>>,
    worker: Option<std::thread::JoinHandle<()>>,
    queue: Arc<Mutex<VecDeque<f32>>>,
    done: Arc<AtomicBool>,
    drained: Arc<AtomicBool>,
}

impl PcmControl {
    pub fn stop(&mut self) {
        self.queue.lock().expect("pcm queue").clear();
        self.done.store(true, Ordering::SeqCst);
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }

    pub fn is_finished(&self) -> bool {
        self.drained.load(Ordering::SeqCst)
    }
}

impl Drop for PcmControl {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Open the default output device; mono input at `src_rate` is linearly
/// resampled to the device rate and fanned out to all channels (fine for
/// speech).
pub fn open_playback(src_rate: u32) -> EngineResult<(PcmWriter, PcmControl)> {
    let queue: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
    let done = Arc::new(AtomicBool::new(false));
    let drained = Arc::new(AtomicBool::new(false));
    let (stop_tx, stop_rx) = bounded::<()>(1);
    let (ready_tx, ready_rx) = bounded::<Result<(), String>>(1);

    let worker = {
        let queue = queue.clone();
        let done = done.clone();
        let drained = drained.clone();
        std::thread::Builder::new()
            .name("laf-playback".into())
            .spawn(move || {
                let host = cpal::default_host();
                let Some(device) = host.default_output_device() else {
                    let _ = ready_tx.send(Err("no audio output device".into()));
                    return;
                };
                let supported = match device.default_output_config() {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("no output config: {e}")));
                        return;
                    }
                };
                let stream_cfg: cpal::StreamConfig = supported.config();
                let out_rate = stream_cfg.sample_rate as f64;
                let channels = stream_cfg.channels as usize;
                let step = src_rate as f64 / out_rate;
                let mut pos = 0.0f64;

                let cb_queue = queue.clone();
                let cb_done = done.clone();
                let cb_drained = drained.clone();
                let err_fn = |e| tracing::error!("output stream error: {e}");
                let stream = device.build_output_stream(
                    stream_cfg,
                    move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        let mut queue = cb_queue.lock().expect("pcm queue");
                        for frame in out.chunks_mut(channels) {
                            let i = pos as usize;
                            let sample = if i + 1 < queue.len() {
                                let frac = (pos - i as f64) as f32;
                                let v = queue[i] * (1.0 - frac) + queue[i + 1] * frac;
                                pos += step;
                                v
                            } else {
                                0.0
                            };
                            for c in frame.iter_mut() {
                                *c = sample;
                            }
                        }
                        // Discard consumed samples once per callback.
                        let consumed = pos as usize;
                        if consumed > 0 {
                            let n = consumed.min(queue.len());
                            queue.drain(..n);
                            pos -= n as f64;
                        }
                        if queue.len() < 2 && cb_done.load(Ordering::SeqCst) {
                            cb_drained.store(true, Ordering::SeqCst);
                        }
                    },
                    err_fn,
                    None,
                );
                let stream = match stream {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("failed to open output stream: {e}")));
                        return;
                    }
                };
                if let Err(e) = stream.play() {
                    let _ = ready_tx.send(Err(format!("failed to start output stream: {e}")));
                    return;
                }
                let _ = ready_tx.send(Ok(()));
                let _ = stop_rx.recv();
                drained.store(true, Ordering::SeqCst);
                drop(stream);
            })
            .map_err(|e| EngineError::Audio(format!("spawn playback thread: {e}")))?
    };

    match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(())) => Ok((
            PcmWriter { queue: queue.clone(), done: done.clone() },
            PcmControl { stop_tx: Some(stop_tx), worker: Some(worker), queue, done, drained },
        )),
        Ok(Err(msg)) => Err(EngineError::Audio(msg)),
        Err(_) => Err(EngineError::Audio("audio output did not start in time".into())),
    }
}
