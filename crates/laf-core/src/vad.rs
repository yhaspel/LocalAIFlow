//! Voice activity detection.
//!
//! Default: an adaptive energy gate with hysteresis and hangover. It tracks a
//! noise floor with an exponential moving average and opens when short-term
//! RMS exceeds the floor by a ratio. Deliberately simple, zero-dependency and
//! deterministic — and swappable behind [`VoiceActivityDetector`] (a Silero
//! ONNX implementation can slot in without touching the pipeline).

use crate::traits::{VadDecision, VoiceActivityDetector};
use crate::types::STT_SAMPLE_RATE;

pub struct EnergyVad {
    /// EMA of noise RMS while in silence.
    noise_floor: f32,
    /// Open when rms > noise_floor * open_ratio (plus absolute floor).
    open_ratio: f32,
    close_ratio: f32,
    abs_floor: f32,
    /// Speech must persist this long to open (debounce), ms.
    min_speech_ms: u32,
    /// Silence must persist this long to close (hangover), ms.
    hangover_ms: u32,
    in_speech: bool,
    speech_run_ms: f32,
    silence_run_ms: f32,
}

impl Default for EnergyVad {
    fn default() -> Self {
        Self {
            noise_floor: 0.003,
            open_ratio: 3.0,
            close_ratio: 1.8,
            abs_floor: 0.0045,
            min_speech_ms: 60,
            hangover_ms: 550,
            in_speech: false,
            speech_run_ms: 0.0,
            silence_run_ms: 0.0,
        }
    }
}

impl EnergyVad {
    pub fn new() -> Self {
        Self::default()
    }

    fn frame_ms(samples: usize) -> f32 {
        samples as f32 * 1000.0 / STT_SAMPLE_RATE as f32
    }
}

impl VoiceActivityDetector for EnergyVad {
    fn process(&mut self, samples: &[f32]) -> VadDecision {
        if samples.is_empty() {
            return if self.in_speech { VadDecision::Speech } else { VadDecision::Silence };
        }
        let ms = Self::frame_ms(samples.len());
        let rms = {
            let sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
            (sq / samples.len() as f64).sqrt() as f32
        };

        let open_thresh = (self.noise_floor * self.open_ratio).max(self.abs_floor);
        let close_thresh = (self.noise_floor * self.close_ratio).max(self.abs_floor * 0.8);

        if self.in_speech {
            if rms < close_thresh {
                self.silence_run_ms += ms;
                if self.silence_run_ms >= self.hangover_ms as f32 {
                    self.in_speech = false;
                    self.speech_run_ms = 0.0;
                    self.silence_run_ms = 0.0;
                    // Update floor with what is clearly silence now.
                    self.noise_floor = 0.95 * self.noise_floor + 0.05 * rms.max(1e-5);
                    return VadDecision::SpeechEnd;
                }
            } else {
                self.silence_run_ms = 0.0;
            }
            VadDecision::Speech
        } else {
            if rms > open_thresh {
                self.speech_run_ms += ms;
                if self.speech_run_ms >= self.min_speech_ms as f32 {
                    self.in_speech = true;
                    self.silence_run_ms = 0.0;
                    return VadDecision::Speech;
                }
            } else {
                self.speech_run_ms = 0.0;
                // Only adapt the floor during silence, slowly.
                self.noise_floor = 0.98 * self.noise_floor + 0.02 * rms.max(1e-5);
            }
            VadDecision::Silence
        }
    }

    fn reset(&mut self) {
        let floor = self.noise_floor;
        *self = Self { noise_floor: floor, ..Self::default() };
    }

    fn name(&self) -> &'static str {
        "energy-gate"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(level: f32, total_ms: u32) -> Vec<Vec<f32>> {
        // 20 ms frames at 16 kHz = 320 samples.
        let n = (total_ms / 20).max(1);
        (0..n)
            .map(|i| {
                (0..320).map(|j| level * ((i * 320 + j) as f32 * 0.3).sin()).collect::<Vec<f32>>()
            })
            .collect()
    }

    #[test]
    fn opens_on_speech_and_closes_after_hangover() {
        let mut vad = EnergyVad::new();
        // Quiet lead-in.
        for f in frames(0.001, 400) {
            assert_eq!(vad.process(&f), VadDecision::Silence);
        }
        // Loud speech.
        let mut opened = false;
        for f in frames(0.2, 400) {
            if vad.process(&f) == VadDecision::Speech {
                opened = true;
            }
        }
        assert!(opened, "vad should open on loud input");
        // Silence again: expect exactly one SpeechEnd.
        let mut ends = 0;
        for f in frames(0.001, 1200) {
            if vad.process(&f) == VadDecision::SpeechEnd {
                ends += 1;
            }
        }
        assert_eq!(ends, 1);
    }

    #[test]
    fn ignores_short_blips() {
        let mut vad = EnergyVad::new();
        for f in frames(0.001, 400) {
            vad.process(&f);
        }
        // 20ms blip (below min_speech_ms=60) must not open the gate.
        let blip = frames(0.3, 20);
        for f in blip {
            assert_ne!(vad.process(&f), VadDecision::SpeechEnd);
        }
        for f in frames(0.001, 200) {
            assert_eq!(vad.process(&f), VadDecision::Silence);
        }
    }
}
