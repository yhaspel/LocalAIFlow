//! Small streaming resampler: arbitrary input rate/channel-count → 16 kHz
//! mono f32, suitable for speech. Channel averaging, a windowed-sinc FIR
//! anti-aliasing low-pass (when downsampling), then linear interpolation.
//! Stateful across chunks so it can sit directly in the audio callback path.

use crate::types::STT_SAMPLE_RATE;

pub struct Resampler {
    in_rate: u32,
    channels: u16,
    /// FIR taps for the anti-aliasing filter (identity when upsampling).
    taps: Vec<f32>,
    /// Carry-over of unfiltered mono samples (last taps-1 for convolution).
    hist: Vec<f32>,
    /// Fractional read position into the filtered stream.
    pos: f64,
    step: f64,
    /// Filtered samples not yet consumed by the interpolator.
    pending: Vec<f32>,
}

impl Resampler {
    pub fn new(in_rate: u32, channels: u16) -> Self {
        let step = in_rate as f64 / STT_SAMPLE_RATE as f64;
        let taps = if in_rate > STT_SAMPLE_RATE {
            // Cutoff a bit under Nyquist of the OUTPUT rate.
            design_lowpass(in_rate as f32, 0.45 * STT_SAMPLE_RATE as f32, 33)
        } else {
            vec![1.0]
        };
        let hist = vec![0.0; taps.len().saturating_sub(1)];
        Self { in_rate, channels, taps, hist, pos: 0.0, step, pending: Vec::new() }
    }

    pub fn in_rate(&self) -> u32 {
        self.in_rate
    }

    /// Push interleaved input samples; returns freshly produced 16 kHz mono.
    pub fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
        // 1. Downmix to mono.
        let ch = self.channels.max(1) as usize;
        let mono: Vec<f32> = interleaved
            .chunks_exact(ch)
            .map(|frame| frame.iter().sum::<f32>() / ch as f32)
            .collect();

        // 2. FIR low-pass (streaming convolution with history).
        let filtered: Vec<f32> = if self.taps.len() == 1 {
            mono
        } else {
            let mut joined = Vec::with_capacity(self.hist.len() + mono.len());
            joined.extend_from_slice(&self.hist);
            joined.extend_from_slice(&mono);
            let t = self.taps.len();
            let out: Vec<f32> = joined
                .windows(t)
                .map(|w| w.iter().zip(&self.taps).map(|(a, b)| a * b).sum())
                .collect();
            // Save the tail as history for the next chunk.
            let keep = t - 1;
            if joined.len() >= keep {
                self.hist = joined[joined.len() - keep..].to_vec();
            }
            out
        };

        // 3. Linear-interpolation rate conversion.
        self.pending.extend_from_slice(&filtered);
        let mut out = Vec::with_capacity((filtered.len() as f64 / self.step) as usize + 2);
        while (self.pos as usize) + 1 < self.pending.len() {
            let i = self.pos as usize;
            let frac = (self.pos - i as f64) as f32;
            out.push(self.pending[i] * (1.0 - frac) + self.pending[i + 1] * frac);
            self.pos += self.step;
        }
        // Drop consumed samples, keep the fractional remainder anchored.
        let consumed = self.pos as usize;
        if consumed > 0 && consumed <= self.pending.len() {
            self.pending.drain(..consumed);
            self.pos -= consumed as f64;
        }
        out
    }
}

/// Hamming-windowed sinc low-pass.
fn design_lowpass(sample_rate: f32, cutoff: f32, taps: usize) -> Vec<f32> {
    let taps = if taps % 2 == 0 { taps + 1 } else { taps };
    let fc = cutoff / sample_rate;
    let m = (taps - 1) as f32;
    let mut h: Vec<f32> = (0..taps)
        .map(|i| {
            let x = i as f32 - m / 2.0;
            let sinc = if x.abs() < f32::EPSILON {
                2.0 * fc
            } else {
                (2.0 * std::f32::consts::PI * fc * x).sin() / (std::f32::consts::PI * x)
            };
            let window = 0.54 - 0.46 * (2.0 * std::f32::consts::PI * i as f32 / m).cos();
            sinc * window
        })
        .collect();
    let sum: f32 = h.iter().sum();
    for v in &mut h {
        *v /= sum;
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_expected_sample_count_48k() {
        let mut r = Resampler::new(48_000, 2);
        let mut total_out = 0usize;
        // 1 second of stereo 48k in 10ms chunks.
        for _ in 0..100 {
            let chunk: Vec<f32> = (0..960).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
            total_out += r.process(&chunk).len();
        }
        // ~16000 out (edge effects allowed).
        assert!((15_800..=16_050).contains(&total_out), "got {total_out}");
    }

    #[test]
    fn preserves_tone_level() {
        // A 440 Hz tone at 48k should come through ~unattenuated at 16k.
        let mut r = Resampler::new(48_000, 1);
        let input: Vec<f32> =
            (0..48_000).map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin()).collect();
        let out = r.process(&input);
        let rms_in = (input.iter().map(|s| s * s).sum::<f32>() / input.len() as f32).sqrt();
        let rms_out = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!((rms_out - rms_in).abs() < 0.05, "in {rms_in} out {rms_out}");
    }

    #[test]
    fn passthrough_at_16k() {
        let mut r = Resampler::new(16_000, 1);
        let input: Vec<f32> = (0..1600).map(|i| i as f32 / 1600.0).collect();
        let out = r.process(&input);
        assert!((out.len() as i64 - 1600).abs() <= 2);
    }
}
