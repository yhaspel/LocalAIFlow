//! Local-only latency instrumentation. Numbers stay in memory (bounded ring)
//! and are shown in the settings debug panel; nothing is ever written to the
//! network or to analytics of any kind (there are none in this app).

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

const CAP_PER_STAGE: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct StageStats {
    pub stage: String,
    pub count: usize,
    pub last_ms: u64,
    pub p50_ms: u64,
    pub p95_ms: u64,
}

#[derive(Default)]
pub struct LatencyTracker {
    inner: Mutex<Vec<(String, VecDeque<u64>)>>,
}

impl LatencyTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, stage: &str, ms: u64) {
        let mut guard = self.inner.lock().expect("metrics lock");
        if let Some((_, ring)) = guard.iter_mut().find(|(s, _)| s == stage) {
            if ring.len() == CAP_PER_STAGE {
                ring.pop_front();
            }
            ring.push_back(ms);
        } else {
            let mut ring = VecDeque::with_capacity(CAP_PER_STAGE);
            ring.push_back(ms);
            guard.push((stage.to_string(), ring));
        }
        tracing::debug!(target: "laf::latency", stage, ms, "stage timing");
    }

    /// Time a closure and record it.
    pub fn time<T>(&self, stage: &str, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let out = f();
        self.record(stage, start.elapsed().as_millis() as u64);
        out
    }

    pub fn summary(&self) -> Vec<StageStats> {
        let guard = self.inner.lock().expect("metrics lock");
        guard
            .iter()
            .map(|(stage, ring)| {
                let mut sorted: Vec<u64> = ring.iter().copied().collect();
                sorted.sort_unstable();
                let pct = |p: f64| -> u64 {
                    if sorted.is_empty() {
                        return 0;
                    }
                    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
                    sorted[idx]
                };
                StageStats {
                    stage: stage.clone(),
                    count: ring.len(),
                    last_ms: ring.back().copied().unwrap_or(0),
                    p50_ms: pct(0.5),
                    p95_ms: pct(0.95),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_summarizes() {
        let t = LatencyTracker::new();
        for ms in [10, 20, 30, 40, 100] {
            t.record("clean", ms);
        }
        let s = t.summary();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].count, 5);
        assert_eq!(s[0].last_ms, 100);
        assert_eq!(s[0].p50_ms, 30);
    }
}
