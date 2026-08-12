//! Text cleanup tiers.
//!
//! Tier 0 (this module, always available, zero latency): deterministic
//! filler-word removal, punctuation + capitalization heuristics, spoken
//! command interpretation, and dictionary substitution. Powers Raw mode and
//! is the guaranteed fallback when no LLM is installed.
//!
//! Tier 1 (laf-engines, optional): a small local instruct model via
//! llama.cpp, or a user-run local Ollama. Tier 2 (macOS 26+, optional):
//! Apple Foundation Models through a helper process. All tiers implement
//! [`TextCleaner`]; the pipeline picks the best available one and *always*
//! finishes with dictionary substitution + deterministic mode post-format.

pub mod commands;
pub mod deterministic;

use crate::traits::{CleanContext, TextCleaner};
use crate::types::{EngineResult, Mode};

/// The always-available deterministic cleaner (tier 0).
#[derive(Default)]
pub struct DeterministicCleaner;

impl DeterministicCleaner {
    pub fn new() -> Self {
        Self
    }
}

impl TextCleaner for DeterministicCleaner {
    fn clean(&self, raw: &str, ctx: &CleanContext) -> EngineResult<String> {
        Ok(clean_deterministic(raw, ctx))
    }

    fn name(&self) -> &'static str {
        "deterministic"
    }

    fn available(&self) -> bool {
        true
    }
}

/// Full deterministic path, also used to post-process LLM output shape.
pub fn clean_deterministic(raw: &str, ctx: &CleanContext) -> String {
    let text = match ctx.mode {
        // Raw: verbatim except whitespace normalization + dictionary.
        Mode::Raw => deterministic::normalize_whitespace(raw),
        Mode::Command => {
            let interpreted = commands::interpret(raw);
            deterministic::punctuate_and_capitalize(&interpreted)
        }
        Mode::Code => {
            // Identifiers must survive: only strip fillers + normalize spaces.
            let no_fillers = deterministic::remove_fillers(raw);
            deterministic::normalize_whitespace(&no_fillers)
        }
        _ => {
            let no_fillers = deterministic::remove_fillers(raw);
            let no_repeats = deterministic::collapse_false_starts(&no_fillers);
            deterministic::punctuate_and_capitalize(&no_repeats)
        }
    };
    let with_dict = ctx.dictionary.apply(&text);
    crate::modes::post_format(ctx.mode, &with_dict)
}

/// Apply the same finishing pass to LLM output so every tier converges on the
/// identical contract (dictionary always wins; mode shape is guaranteed).
pub fn finish_llm_output(llm_out: &str, ctx: &CleanContext) -> String {
    let trimmed = strip_llm_wrapping(llm_out);
    let with_dict = ctx.dictionary.apply(&trimmed);
    crate::modes::post_format(ctx.mode, &with_dict)
}

/// Models occasionally wrap output in quotes or add a label despite the
/// prompt. Strip the obvious cases only — never touch inner content.
fn strip_llm_wrapping(s: &str) -> String {
    let mut t = s.trim();
    for prefix in ["Formatted text:", "Output:", "Cleaned text:", "Result:"] {
        if t.len() > prefix.len() && t[..prefix.len()].eq_ignore_ascii_case(prefix) {
            t = t[prefix.len()..].trim_start();
        }
    }
    if t.len() >= 2 {
        let b = t.as_bytes();
        if (b[0] == b'"' && b[t.len() - 1] == b'"') || (b[0] == b'\'' && b[t.len() - 1] == b'\'') {
            let inner = &t[1..t.len() - 1];
            // Only unwrap if the interior has no matching quote (i.e. the
            // pair is wrapping, not content).
            if !inner.contains(b[0] as char) {
                t = inner;
            }
        }
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::Dictionary;

    fn ctx(mode: Mode) -> CleanContext {
        CleanContext { mode, language: "en".into(), dictionary: Dictionary::default() }
    }

    #[test]
    fn auto_mode_end_to_end() {
        let c = DeterministicCleaner::new();
        let out = c
            .clean("um so i think we should uh ship the the feature on friday", &ctx(Mode::Auto))
            .unwrap();
        // Leading discourse-marker "so" is stripped like other fillers.
        assert_eq!(out, "I think we should ship the feature on Friday.");
    }

    #[test]
    fn raw_mode_is_verbatim() {
        let c = DeterministicCleaner::new();
        let out = c.clean("um so i think  we should", &ctx(Mode::Raw)).unwrap();
        assert_eq!(out, "um so i think we should");
    }

    #[test]
    fn strip_wrapping_quotes_and_labels() {
        assert_eq!(strip_llm_wrapping("\"Hello there.\""), "Hello there.");
        assert_eq!(strip_llm_wrapping("Output: Hello."), "Hello.");
        // Interior quotes are preserved.
        assert_eq!(strip_llm_wrapping("\"a\" and \"b\""), "\"a\" and \"b\"");
    }
}
