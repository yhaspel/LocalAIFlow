//! The dictation/TTS pipeline: a single controller thread owning all engines,
//! driven by commands (from hotkeys / tray / UI) and audio frames, emitting
//! [`UiEvent`]s for the HUD, tray, and settings views.
//!
//! State machine:
//! ```text
//! Idle --Start--> Listening --Stop--> Processing --> Inserting --> Idle
//!   \--ReadSelection--> Speaking --StopSpeech/finished--> Idle
//! Listening --Cancel--> Idle (nothing inserted)
//! ```

use crate::clean::{clean_deterministic, finish_llm_output};
use crate::dictionary::Dictionary;
use crate::metrics::LatencyTracker;
use crate::settings::{CleanerTier, SettingsStore};
use crate::traits::*;
use crate::types::*;
use crossbeam_channel::{select, tick, unbounded, Receiver, Sender};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartSource {
    Toggle,
    PushToTalk,
}

#[derive(Debug)]
pub enum PipelineCmd {
    SetMode(Mode),
    Start(StartSource),
    Stop(StartSource),
    Cancel,
    ReadSelection,
    StopSpeech,
    /// Settings changed on disk; re-read before the next session.
    SettingsChanged,
    Shutdown,
}

/// All engines, moved into the controller thread. The LLM cleaner and TTS
/// fallbacks are optional; everything else is mandatory (real implementations
/// exist for both OSes — see laf-engines and the platform crates).
pub struct Engines {
    pub capture: Box<dyn AudioCapture>,
    pub vad: Box<dyn VoiceActivityDetector>,
    pub stt: Arc<dyn SpeechToText>,
    pub cleaner_det: Arc<dyn TextCleaner>,
    pub cleaner_llm: Option<Arc<dyn TextCleaner>>,
    pub inserter: Arc<dyn TextInserter>,
    pub selection: Arc<dyn SelectionReader>,
    /// Ordered by preference: [kokoro, piper, system] — first available wins,
    /// honoring the engine chosen in settings.
    pub tts_engines: Vec<Arc<dyn SpeechSynthesizer>>,
}

#[derive(Clone)]
pub struct PipelineHandle {
    cmd_tx: Sender<PipelineCmd>,
    mode: Arc<AtomicU8>,
}

impl PipelineHandle {
    pub fn send(&self, cmd: PipelineCmd) {
        if self.cmd_tx.send(cmd).is_err() {
            tracing::error!("pipeline controller is gone");
        }
    }

    pub fn current_mode(&self) -> Mode {
        Mode::ALL[self.mode.load(Ordering::Relaxed) as usize % Mode::ALL.len()]
    }
}

pub fn spawn(
    engines: Engines,
    settings: Arc<SettingsStore>,
    metrics: Arc<LatencyTracker>,
    ui_tx: Sender<UiEvent>,
) -> PipelineHandle {
    let (cmd_tx, cmd_rx) = unbounded::<PipelineCmd>();
    let initial_mode = settings.get().default_mode;
    let mode_atomic = Arc::new(AtomicU8::new(
        Mode::ALL.iter().position(|m| *m == initial_mode).unwrap_or(1) as u8,
    ));
    let handle = PipelineHandle { cmd_tx, mode: mode_atomic.clone() };

    std::thread::Builder::new()
        .name("laf-pipeline".into())
        .spawn(move || {
            Controller {
                engines,
                settings,
                metrics,
                ui_tx,
                cmd_rx,
                mode_atomic,
                session: None,
                audio_rx: None,
                finals: Vec::new(),
                inserted_segments: 0,
                started_at: None,
                stopped_at: None,
                start_source: StartSource::Toggle,
                tts_handle: None,
                last_level_emit: Instant::now(),
                last_activity: Instant::now(),
            }
            .run()
        })
        .expect("spawn pipeline thread");
    handle
}

struct Controller {
    engines: Engines,
    settings: Arc<SettingsStore>,
    metrics: Arc<LatencyTracker>,
    ui_tx: Sender<UiEvent>,
    cmd_rx: Receiver<PipelineCmd>,
    mode_atomic: Arc<AtomicU8>,
    session: Option<Box<dyn SttSession>>,
    audio_rx: Option<Receiver<AudioFrame>>,
    finals: Vec<String>,
    /// In incremental mode: how many finals have already been inserted.
    inserted_segments: usize,
    started_at: Option<Instant>,
    stopped_at: Option<Instant>,
    start_source: StartSource,
    tts_handle: Option<Box<dyn TtsPlayback>>,
    last_level_emit: Instant,
    last_activity: Instant,
}

impl Controller {
    fn mode(&self) -> Mode {
        Mode::ALL[self.mode_atomic.load(Ordering::Relaxed) as usize % Mode::ALL.len()]
    }

    fn emit(&self, ev: UiEvent) {
        let _ = self.ui_tx.send(ev);
    }

    fn set_phase(&self, phase: Phase) {
        self.emit(UiEvent::Phase { phase, mode: self.mode() });
    }

    fn run(mut self) {
        let ticker = tick(Duration::from_millis(50));
        self.set_phase(Phase::Idle);
        loop {
            // Two select shapes because `select!` needs a static arm list and
            // the audio receiver only exists while listening.
            let cmd = if let Some(audio_rx) = self.audio_rx.clone() {
                select! {
                    recv(self.cmd_rx) -> c => c.ok(),
                    recv(audio_rx) -> frame => {
                        if let Ok(frame) = frame { self.on_frame(frame); }
                        None
                    }
                    recv(ticker) -> _ => { self.on_tick(); None }
                }
            } else {
                select! {
                    recv(self.cmd_rx) -> c => c.ok(),
                    recv(ticker) -> _ => { self.on_tick(); None }
                }
            };
            let Some(cmd) = cmd else { continue };
            match cmd {
                PipelineCmd::Shutdown => {
                    self.cancel_dictation();
                    self.stop_speech();
                    return;
                }
                PipelineCmd::SetMode(m) => {
                    let idx = Mode::ALL.iter().position(|x| *x == m).unwrap_or(1) as u8;
                    self.mode_atomic.store(idx, Ordering::Relaxed);
                    self.set_phase(if self.session.is_some() {
                        Phase::Listening
                    } else {
                        Phase::Idle
                    });
                }
                PipelineCmd::Start(src) => self.start_dictation(src),
                PipelineCmd::Stop(src) => {
                    // A PTT release must not stop a toggle-started session and
                    // vice versa (matches Wispr Flow behavior).
                    if self.session.is_some() && self.start_source == src {
                        self.stop_dictation();
                    }
                }
                PipelineCmd::Cancel => self.cancel_dictation(),
                PipelineCmd::ReadSelection => self.read_selection(),
                PipelineCmd::StopSpeech => self.stop_speech(),
                PipelineCmd::SettingsChanged => {
                    let s = self.settings.get();
                    self.engines.capture.select_device(s.input_device.clone());
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Dictation
    // ------------------------------------------------------------------

    fn start_dictation(&mut self, src: StartSource) {
        if self.session.is_some() {
            // Toggle pressed while listening → treat as stop.
            if src == StartSource::Toggle {
                self.stop_dictation();
            }
            return;
        }
        self.stop_speech(); // never dictate over TTS playback
        let s = self.settings.get();
        let dict = Dictionary::new(&s.dictionary);
        let cfg = SttSessionConfig {
            language: s.language.clone(),
            vocabulary_hints: dict.hint_phrases(),
            max_threads: s.stt.threads,
        };
        let t0 = Instant::now();
        let session = match self.engines.stt.start_session(cfg) {
            Ok(sess) => sess,
            Err(e) => {
                self.emit(UiEvent::PipelineError { message: e.to_string() });
                return;
            }
        };
        let (tx, rx) = unbounded::<AudioFrame>();
        match self.engines.capture.start(tx) {
            Ok(device) => tracing::info!("capturing from '{device}'"),
            Err(e) => {
                self.emit(UiEvent::PipelineError { message: e.to_string() });
                return;
            }
        }
        self.metrics.record("stt_session_start", t0.elapsed().as_millis() as u64);
        self.engines.vad.reset();
        self.session = Some(session);
        self.audio_rx = Some(rx);
        self.finals.clear();
        self.inserted_segments = 0;
        self.started_at = Some(Instant::now());
        self.start_source = src;
        self.set_phase(Phase::Listening);
    }

    fn on_frame(&mut self, frame: AudioFrame) {
        if self.last_level_emit.elapsed() >= Duration::from_millis(33) {
            self.emit(UiEvent::Level { rms: frame.rms, peak: frame.peak });
            self.last_level_emit = Instant::now();
        }
        let decision = self.engines.vad.process(&frame.samples);
        if let Some(session) = self.session.as_mut() {
            session.feed(&frame.samples);
            if decision == VadDecision::SpeechEnd {
                session.segment_boundary();
            }
        }
    }

    fn on_tick(&mut self) {
        // Drain STT events.
        let mut incremental_inserts: Vec<String> = Vec::new();
        if let Some(session) = self.session.as_mut() {
            for ev in session.drain_events() {
                match ev {
                    SttEvent::Partial { text } => self.emit(UiEvent::Partial { text }),
                    SttEvent::Final { text, .. } => {
                        if !text.trim().is_empty() {
                            self.finals.push(text.trim().to_string());
                            self.emit(UiEvent::FinalSegment {
                                text: self.finals.last().cloned().unwrap_or_default(),
                            });
                        }
                    }
                    SttEvent::Error { message } => self.emit(UiEvent::PipelineError { message }),
                }
            }
            let s = self.settings.get();
            if s.insert_incremental {
                while self.inserted_segments < self.finals.len() {
                    incremental_inserts.push(self.finals[self.inserted_segments].clone());
                    self.inserted_segments += 1;
                }
            }
        }
        for seg in incremental_inserts {
            self.clean_and_insert(&seg, true);
        }
        // TTS finished?
        if self.tts_handle.as_mut().is_some_and(|h| h.is_finished()) {
            self.tts_handle = None;
            self.emit(UiEvent::TtsStopped);
            if self.session.is_none() {
                self.set_phase(Phase::Idle);
            }
        }
        // Idle model unload.
        let s = self.settings.get();
        if s.model_idle_unload_secs > 0
            && self.session.is_none()
            && self.tts_handle.is_none()
            && self.last_activity.elapsed() > Duration::from_secs(s.model_idle_unload_secs)
        {
            self.engines.stt.unload();
            if let Some(llm) = &self.engines.cleaner_llm {
                llm.unload();
            }
            for t in &self.engines.tts_engines {
                t.unload();
            }
            self.last_activity = Instant::now(); // don't spin unload
        }
    }

    fn stop_dictation(&mut self) {
        let Some(mut session) = self.session.take() else { return };
        self.engines.capture.stop();
        // Feed any audio still sitting in the channel before finalizing, so the
        // tail of the utterance (the last word plus the trailing silence
        // whisper uses to segment) is not clipped. `capture.stop()` has already
        // joined the capture thread, so no further frames can arrive after this
        // drain; the loop terminates once the channel is empty/disconnected.
        if let Some(rx) = self.audio_rx.take() {
            while let Ok(frame) = rx.try_recv() {
                session.feed(&frame.samples);
            }
        }
        self.stopped_at = Some(Instant::now());
        self.set_phase(Phase::Processing);

        let t_final = Instant::now();
        match session.finalize() {
            Ok(events) => {
                for ev in events {
                    if let SttEvent::Final { text, .. } = ev {
                        if !text.trim().is_empty() {
                            self.finals.push(text.trim().to_string());
                        }
                    }
                }
            }
            Err(e) => self.emit(UiEvent::PipelineError { message: e.to_string() }),
        }
        self.metrics.record("stt_finalize", t_final.elapsed().as_millis() as u64);
        self.emit(UiEvent::Latency {
            stage: "stt_finalize".into(),
            ms: t_final.elapsed().as_millis() as u64,
        });

        let s = self.settings.get();
        let remaining: Vec<String> = self.finals.drain(self.inserted_segments..).collect();
        let raw = remaining.join(" ");
        self.finals.clear();
        self.inserted_segments = 0;

        if raw.trim().is_empty() {
            self.set_phase(Phase::Idle);
            self.last_activity = Instant::now();
            return;
        }
        self.clean_and_insert(&raw, false);
        if let Some(t) = self.stopped_at {
            let e2e = t.elapsed().as_millis() as u64;
            self.metrics.record("e2e_stop_to_insert", e2e);
            self.emit(UiEvent::Latency { stage: "e2e_stop_to_insert".into(), ms: e2e });
        }
        let _ = s; // settings snapshot kept for symmetry
        self.set_phase(Phase::Idle);
        self.last_activity = Instant::now();
    }

    fn cancel_dictation(&mut self) {
        if let Some(session) = self.session.take() {
            self.engines.capture.stop();
            self.audio_rx = None;
            // Drop the session WITHOUT finalizing. Cancel must be immediate and
            // discard everything; `finalize()` would force a full (and, on
            // cancel, pointless) decode of all buffered audio before returning,
            // blocking the controller thread for seconds on a long utterance.
            // Dropping hangs up the worker's command channel so it exits at its
            // next `recv` without decoding.
            drop(session);
        }
        self.finals.clear();
        self.inserted_segments = 0;
        self.set_phase(Phase::Idle);
    }

    /// Clean `raw` for the current mode and insert it. `incremental` skips
    /// mode post-formatting that only makes sense on whole utterances.
    fn clean_and_insert(&mut self, raw: &str, incremental: bool) {
        let s = self.settings.get();
        let mode = self.mode();
        let ctx = CleanContext {
            mode,
            language: s.language.clone(),
            dictionary: Dictionary::new(&s.dictionary),
        };

        let t_clean = Instant::now();
        // Raw and Command modes are deterministic by definition; the LLM only
        // adds value on prose modes.
        let use_llm = !matches!(mode, Mode::Raw | Mode::Command)
            && match s.cleaner.tier {
                CleanerTier::Deterministic => false,
                CleanerTier::Auto
                | CleanerTier::LocalLlm
                | CleanerTier::Ollama
                | CleanerTier::AppleFm => true,
            };
        let cleaned = if use_llm {
            match self.engines.cleaner_llm.as_ref().filter(|c| c.available()) {
                Some(llm) => match llm.clean(raw, &ctx) {
                    Ok(out) => finish_llm_output(&out, &ctx),
                    Err(e) => {
                        tracing::warn!("LLM cleaner failed ({e}); falling back to deterministic");
                        clean_deterministic(raw, &ctx)
                    }
                },
                None => clean_deterministic(raw, &ctx),
            }
        } else {
            clean_deterministic(raw, &ctx)
        };
        let clean_ms = t_clean.elapsed().as_millis() as u64;
        self.metrics.record("clean", clean_ms);
        self.emit(UiEvent::Latency { stage: "clean".into(), ms: clean_ms });

        if cleaned.trim().is_empty() {
            return;
        }
        // Incremental segments get a trailing space so consecutive inserts read naturally.
        let payload = if incremental { format!("{cleaned} ") } else { cleaned };

        self.set_phase(Phase::Inserting);
        let t_insert = Instant::now();
        match self.engines.inserter.insert_text(&payload) {
            Ok(report) => {
                self.metrics.record("insert", t_insert.elapsed().as_millis() as u64);
                self.emit(UiEvent::Inserted { report, text: payload });
            }
            Err(e) => {
                self.emit(UiEvent::PipelineError { message: format!("could not insert text: {e}") })
            }
        }
        if incremental {
            self.set_phase(Phase::Listening);
        }
    }

    // ------------------------------------------------------------------
    // TTS
    // ------------------------------------------------------------------

    fn read_selection(&mut self) {
        self.stop_speech();
        let text = match self.engines.selection.read_selection() {
            Ok(Some(t)) if !t.trim().is_empty() => t,
            Ok(_) => {
                self.emit(UiEvent::PipelineError {
                    message: "No text selected in the frontmost app.".into(),
                });
                return;
            }
            Err(e) => {
                self.emit(UiEvent::PipelineError { message: e.to_string() });
                return;
            }
        };
        let s = self.settings.get();
        let opts = TtsOptions { voice_id: s.tts.voice_id.clone(), rate: s.tts.rate };
        let t0 = Instant::now();

        // Prefer the configured engine, then fall through the chain.
        let mut ordered: Vec<&Arc<dyn SpeechSynthesizer>> =
            self.engines.tts_engines.iter().filter(|e| e.info().name == s.tts.engine).collect();
        ordered.extend(self.engines.tts_engines.iter().filter(|e| e.info().name != s.tts.engine));

        let mut last_err: Option<EngineError> = None;
        for engine in ordered {
            match engine.speak(&text, &opts) {
                Ok(handle) => {
                    self.metrics.record("tts_first_dispatch", t0.elapsed().as_millis() as u64);
                    self.tts_handle = Some(handle);
                    self.emit(UiEvent::TtsStarted { chars: text.chars().count() });
                    self.set_phase(Phase::Speaking);
                    self.last_activity = Instant::now();
                    return;
                }
                Err(e) => {
                    tracing::warn!("TTS engine '{}' unavailable: {e}", engine.info().name);
                    last_err = Some(e);
                }
            }
        }
        self.emit(UiEvent::PipelineError {
            message: format!(
                "no TTS engine could speak the selection: {}",
                last_err.map(|e| e.to_string()).unwrap_or_else(|| "none configured".into())
            ),
        });
    }

    fn stop_speech(&mut self) {
        if let Some(mut h) = self.tts_handle.take() {
            h.stop();
            self.emit(UiEvent::TtsStopped);
            if self.session.is_none() {
                self.set_phase(Phase::Idle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clean::DeterministicCleaner;
    use std::sync::Mutex;

    // ---- lightweight fakes for state-machine testing ----
    struct FakeCapture {
        running: bool,
    }
    impl AudioCapture for FakeCapture {
        fn start(&mut self, _sink: Sender<AudioFrame>) -> EngineResult<String> {
            self.running = true;
            Ok("fake-mic".into())
        }
        fn stop(&mut self) {
            self.running = false;
        }
        fn is_running(&self) -> bool {
            self.running
        }
        fn list_devices(&self) -> Vec<String> {
            vec!["fake-mic".into()]
        }
        fn select_device(&mut self, _name: Option<String>) {}
    }

    struct FakeVad;
    impl VoiceActivityDetector for FakeVad {
        fn process(&mut self, _s: &[f32]) -> VadDecision {
            VadDecision::Speech
        }
        fn reset(&mut self) {}
        fn name(&self) -> &'static str {
            "fake"
        }
    }

    struct FakeSttSession {
        fed: usize,
    }
    impl SttSession for FakeSttSession {
        fn feed(&mut self, pcm: &[f32]) {
            self.fed += pcm.len();
        }
        fn segment_boundary(&mut self) {}
        fn drain_events(&mut self) -> Vec<SttEvent> {
            Vec::new()
        }
        fn finalize(&mut self) -> EngineResult<Vec<SttEvent>> {
            Ok(vec![SttEvent::Final { text: "um hello world period".into(), t0_ms: 0, t1_ms: 900 }])
        }
    }

    struct FakeStt;
    impl SpeechToText for FakeStt {
        fn start_session(&self, _cfg: SttSessionConfig) -> EngineResult<Box<dyn SttSession>> {
            Ok(Box::new(FakeSttSession { fed: 0 }))
        }
        fn info(&self) -> EngineInfo {
            EngineInfo { name: "fake-stt", model: None, accelerated: false }
        }
        fn unload(&self) {}
    }

    #[derive(Default)]
    struct RecordingInserter {
        pub texts: Mutex<Vec<String>>,
    }
    impl TextInserter for RecordingInserter {
        fn insert_text(&self, text: &str) -> EngineResult<InsertionReport> {
            self.texts.lock().unwrap().push(text.to_string());
            Ok(InsertionReport {
                method: InsertionMethod::SyntheticKeys { tool: "fake".into() },
                chars: text.chars().count(),
                elapsed_ms: 1,
                fallback_notes: vec![],
            })
        }
    }

    struct NoSelection;
    impl SelectionReader for NoSelection {
        fn read_selection(&self) -> EngineResult<Option<String>> {
            Ok(None)
        }
    }

    fn build() -> (PipelineHandle, Receiver<UiEvent>, Arc<RecordingInserter>, Arc<SettingsStore>) {
        let inserter = Arc::new(RecordingInserter::default());
        let dir = tempfile::tempdir().unwrap();
        let settings = Arc::new(SettingsStore::load_from(dir.path().join("s.json")));
        // Leak tempdir so the settings path stays valid for the test process.
        std::mem::forget(dir);
        let (ui_tx, ui_rx) = unbounded();
        let engines = Engines {
            capture: Box::new(FakeCapture { running: false }),
            vad: Box::new(FakeVad),
            stt: Arc::new(FakeStt),
            cleaner_det: Arc::new(DeterministicCleaner::new()),
            cleaner_llm: None,
            inserter: inserter.clone(),
            selection: Arc::new(NoSelection),
            tts_engines: vec![],
        };
        let handle = spawn(engines, settings.clone(), Arc::new(LatencyTracker::new()), ui_tx);
        (handle, ui_rx, inserter, settings)
    }

    #[test]
    fn full_dictation_round_trip_cleans_and_inserts() {
        let (handle, ui_rx, inserter, _s) = build();
        handle.send(PipelineCmd::Start(StartSource::Toggle));
        std::thread::sleep(Duration::from_millis(120));
        handle.send(PipelineCmd::Stop(StartSource::Toggle));
        // Wait for the Inserted event.
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut inserted = false;
        while Instant::now() < deadline {
            if let Ok(ev) = ui_rx.recv_timeout(Duration::from_millis(200)) {
                if matches!(ev, UiEvent::Inserted { .. }) {
                    inserted = true;
                    break;
                }
            }
        }
        assert!(inserted, "expected an Inserted event");
        let texts = inserter.texts.lock().unwrap();
        assert_eq!(texts.len(), 1);
        // "um" removed, capitalized, terminated ("period" is NOT interpreted
        // outside Command mode).
        assert_eq!(texts[0], "Hello world period.");
        handle.send(PipelineCmd::Shutdown);
    }

    #[test]
    fn ptt_release_does_not_stop_toggle_session() {
        let (handle, ui_rx, inserter, _s) = build();
        handle.send(PipelineCmd::Start(StartSource::Toggle));
        std::thread::sleep(Duration::from_millis(80));
        handle.send(PipelineCmd::Stop(StartSource::PushToTalk)); // must be ignored
        std::thread::sleep(Duration::from_millis(80));
        assert!(inserter.texts.lock().unwrap().is_empty());
        handle.send(PipelineCmd::Cancel);
        // Drain UI events; no Inserted expected.
        while let Ok(ev) = ui_rx.recv_timeout(Duration::from_millis(100)) {
            assert!(!matches!(ev, UiEvent::Inserted { .. }));
        }
        handle.send(PipelineCmd::Shutdown);
    }

    #[test]
    fn command_mode_interprets_spoken_commands() {
        let (handle, ui_rx, inserter, _s) = build();
        handle.send(PipelineCmd::SetMode(Mode::Command));
        handle.send(PipelineCmd::Start(StartSource::Toggle));
        std::thread::sleep(Duration::from_millis(80));
        handle.send(PipelineCmd::Stop(StartSource::Toggle));
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if let Ok(UiEvent::Inserted { .. }) = ui_rx.recv_timeout(Duration::from_millis(200)) {
                break;
            }
        }
        let texts = inserter.texts.lock().unwrap();
        assert_eq!(texts.as_slice(), &["Um hello world.".to_string()]);
        handle.send(PipelineCmd::Shutdown);
    }
}
