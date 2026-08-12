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
        // Parse the URL and check its HOST, not a string prefix: a prefix match
        // like `starts_with("http://127.0.0.1")` is trivially bypassed by
        // `http://127.0.0.1@evil.com` (userinfo) or `http://127.0.0.1.evil.com`,
        // both of which resolve to a remote host. Only localhost and the
        // loopback IP ranges (127.0.0.0/8, ::1) are accepted.
        let url = reqwest::Url::parse(base)
            .map_err(|e| EngineError::Other(format!("invalid Ollama URL '{base}': {e}")))?;
        let host = url.host_str().unwrap_or("");
        let host_unbracketed =
            host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
        let ip_loopback = host_unbracketed
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
        let is_loopback = matches!(url.scheme(), "http" | "https")
            && (host_unbracketed.eq_ignore_ascii_case("localhost") || ip_loopback);
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
        assert!(OllamaCleaner::new("http://[::1]:11434", "m").is_ok());
        // 127.0.0.0/8 is all loopback.
        assert!(OllamaCleaner::new("http://127.1.2.3:11434", "m").is_ok());
        // Prefix-match bypasses that resolve to a REMOTE host must be refused.
        assert!(OllamaCleaner::new("http://127.0.0.1@evil.com", "m").is_err());
        assert!(OllamaCleaner::new("http://127.0.0.1.evil.com", "m").is_err());
        assert!(OllamaCleaner::new("http://localhost.evil.com", "m").is_err());
    }
}
