//! Mode-specific behavior: LLM prompt templates and deterministic
//! post-formatting shared by every cleaner tier.

use crate::types::Mode;

/// Strict, non-chatty system prompt for the LLM cleanup tier.
/// `{MODE}` is replaced by [`mode_style_instruction`].
pub const CLEANUP_SYSTEM_PROMPT: &str = "You are a transcription formatter. Rewrite the user's raw dictated text into clean, correct prose.\nRules:\n- Remove filler words (um, uh, like, you know) and false starts.\n- Fix punctuation, capitalization, and obvious misrecognitions from context.\n- Do NOT add, invent, or answer anything. Only reformat what was said.\n- Preserve meaning and the speaker's wording.\n- Apply the requested style: {MODE}.\nOutput ONLY the formatted text, with no preamble, quotes, or commentary.";

/// Style clause substituted into the system prompt per mode.
pub fn mode_style_instruction(mode: Mode) -> &'static str {
    match mode {
        Mode::Raw => "verbatim — do not change anything",
        Mode::Auto => "natural clean prose",
        Mode::Email => {
            "professional email body: short paragraphs separated by blank lines; keep any greeting on its own line and any sign-off on its own line"
        }
        Mode::Message => "casual chat message: compact, friendly, no formal sign-off",
        Mode::List => "a bullet list: each distinct point on its own line prefixed with '- '",
        Mode::Code => {
            "text destined for a code editor: preserve identifiers, casing and symbols exactly; do not sentence-case; no trailing period"
        }
        Mode::Command => "the literal result after interpreting spoken editing commands",
    }
}

pub fn build_system_prompt(mode: Mode) -> String {
    CLEANUP_SYSTEM_PROMPT.replace("{MODE}", mode_style_instruction(mode))
}

/// Deterministic per-mode shaping applied AFTER any cleaner tier (including
/// the LLM, to guarantee shape even if the model ignores instructions).
pub fn post_format(mode: Mode, text: &str) -> String {
    match mode {
        Mode::List => to_bullets(text),
        Mode::Email => normalize_email_paragraphs(text),
        _ => text.to_string(),
    }
}

/// Split prose into sentence-ish bullets.
fn to_bullets(text: &str) -> String {
    let mut items: Vec<String> = Vec::new();
    // Existing newlines/bullets win; otherwise split on sentence enders.
    let candidates: Vec<&str> = if text.contains('\n') {
        text.lines().collect()
    } else {
        split_sentences(text)
    };
    for c in candidates {
        let trimmed = c.trim().trim_start_matches(['-', '*', '•']).trim();
        if trimmed.is_empty() {
            continue;
        }
        let clean = trimmed.trim_end_matches(['.', ';', ',']).to_string();
        items.push(format!("- {clean}"));
    }
    items.join("\n")
}

/// Ensure greetings and sign-offs sit on their own lines and paragraphs are
/// separated by blank lines.
fn normalize_email_paragraphs(text: &str) -> String {
    let t = text.trim();
    if t.contains("\n\n") {
        return t.to_string();
    }
    // Detect a leading greeting ("Hi X," / "Hello," / "Hey team,") on the
    // first comma boundary.
    let lower = t.to_lowercase();
    let greeting_end = ["hi ", "hello ", "hey ", "dear ", "hi,", "hello,", "hey,"]
        .iter()
        .any(|g| lower.starts_with(g))
        .then(|| t.find(',').map(|i| i + 1))
        .flatten();
    match greeting_end {
        Some(idx) if idx < t.len() => {
            let (greet, rest) = t.split_at(idx);
            format!("{}\n\n{}", greet.trim(), rest.trim())
        }
        _ => t.to_string(),
    }
}

/// Naive but robust sentence splitter (abbreviation-tolerant enough for
/// dictation output).
pub fn split_sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'.' || b == b'!' || b == b'?' {
            // Consume any run of closers, then require whitespace+uppercase-ish
            // or end of text.
            let mut j = i + 1;
            while j < bytes.len() && matches!(bytes[j], b'.' | b'!' | b'?' | b'"' | b'\'' | b')') {
                j += 1;
            }
            let boundary = j >= bytes.len()
                || (bytes[j] == b' '
                    && text[j..].trim_start().chars().next().is_some_and(|c| {
                        c.is_uppercase() || c.is_numeric() || c == '"' || c == '\''
                    }));
            if boundary {
                out.push(text[start..j].trim());
                start = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    if start < text.len() {
        let tail = text[start..].trim();
        if !tail.is_empty() {
            out.push(tail);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_mode_bullets() {
        let out = post_format(Mode::List, "Buy milk. Call Anna about the offsite. Fix the CI.");
        assert_eq!(out, "- Buy milk\n- Call Anna about the offsite\n- Fix the CI");
    }

    #[test]
    fn email_mode_splits_greeting() {
        let out = post_format(Mode::Email, "Hi Sarah, thanks for the update. I'll review it today.");
        assert!(out.starts_with("Hi Sarah,\n\n"));
    }

    #[test]
    fn sentence_split_tolerates_numbers() {
        let s = split_sentences("Version 2.5 shipped. It works.");
        assert_eq!(s, vec!["Version 2.5 shipped.", "It works."]);
    }

    #[test]
    fn prompt_contains_mode() {
        let p = build_system_prompt(Mode::Email);
        assert!(p.contains("professional email body"));
        assert!(!p.contains("{MODE}"));
    }
}
