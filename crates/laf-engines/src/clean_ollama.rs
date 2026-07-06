//! Optional cleanup tier: a user-run Ollama server on the local machine.
//!
//! PRIVACY: this client is hard-restricted to loopback (127.0.0.1 /
//! localhost / ::1). The constructor rejects anything else, so this code
//! path can never become a cloud call. Inference still happens entirely on
//! the user's device — Ollama is just another local runtime the user may
//! already have models in.

use laf_core::modes::build_system_prompt;
use laf_core::traits::{CleanContext, TextCleaner};
use laf_core::types::{EngineError, EngineResult};
use serde_json::json;
use std::time::Duration;

pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";

pub struct OllamaCleaner {
    base: String,
    model: String,
}

impl OllamaCleaner {
    /// `base` must be a loopback URL; anything else is refused.
    pub fn new(base: &str, model: impl Into<String>) -> EngineResult<Self> {
        let is_loopback = ["http://127.0.0.1", "http://localhost", "http://[::1]"]
            .iter()
            .any(|p| base.starts_with(p));
        if !is_loopback {
            return Err(EngineError::Other(format!(
                "Ollama URL must be loopback (got '{base}') — remote inference is not permitted"
            )));
        }
        Ok(Self { base: base.trim_end_matches('/').to_string(), model: model.into() })
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = model.into();
    }
}

impl TextCleaner for OllamaCleaner {
    fn clean(&self, raw: &str, ctx: &CleanContext) -> EngineResult<String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| EngineError::Cleanup(format!("ollama client: {e}")))?;
        let body = json!({
            "model": self.model,
            "stream": false,
            "options": { "temperature": 0.0 },
            "messages": [
                { "role": "system", "content": build_system_prompt(ctx.mode) },
                { "role": "user", "content": raw },
            ],
        });
        let resp: serde_json::Value = client
            .post(format!("{}/api/chat", self.base))
            .json(&body)
            .send()
            .map_err(|e| EngineError::Cleanup(format!("ollama request: {e}")))?
            .error_for_status()
            .map_err(|e| EngineError::Cleanup(format!("ollama: {e}")))?
            .json()
            .map_err(|e| EngineError::Cleanup(format!("ollama response: {e}")))?;
        let content = resp
            .pointer("/message/content")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if content.is_empty() {
            return Err(EngineError::Cleanup("ollama returned empty output".into()));
        }
        Ok(content)
    }

    fn name(&self) -> &'static str {
        "ollama"
    }

    fn available(&self) -> bool {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            .ok()
            .and_then(|c| c.get(format!("{}/api/version", self.base)).send().ok())
            .is_some_and(|r| r.status().is_success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_non_loopback() {
        assert!(OllamaCleaner::new("http://api.example.com", "m").is_err());
        assert!(OllamaCleaner::new("http://127.0.0.1:11434", "m").is_ok());
        assert!(OllamaCleaner::new("http://localhost:11434", "m").is_ok());
    }
}
