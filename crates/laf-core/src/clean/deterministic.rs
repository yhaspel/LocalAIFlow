//! Deterministic cleanup primitives: filler removal, false-start collapsing,
//! whitespace normalization, punctuation + capitalization heuristics.
//! English-centric filler list with a few common cross-language fillers; the
//! LLM tier handles other languages more gracefully.

use regex::Regex;
use std::sync::OnceLock;

fn fillers_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Standalone fillers. "like" and "so" are only removed in positions
        // where they are near-certainly fillers (start of utterance for "so";
        // "like" surrounded by commas/pauses) to avoid mangling meaning.
        Regex::new(
            r"(?ix)
            (?: \b(?:um+|uh+|uhm+|erm+|ah+m|hmm+|mmm+)\b
              | \byou\s+know\b
              | \bi\s+mean\b(?:\s*,)?
              | ,\s*like\s*,
            )",
        )
        .unwrap()
    })
}

fn leading_so_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^\s*(?:so|okay|ok|well|right)[,\s]+").unwrap())
}

fn fragment_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "I wa- I want" — a word cut off with a dash, followed by a restart.
    RE.get_or_init(|| Regex::new(r"\b\w{1,8}-\s+").unwrap())
}

pub fn remove_fillers(text: &str) -> String {
    let mut out = fillers_re().replace_all(text, " ").into_owned();
    out = fragment_re().replace_all(&out, "").into_owned();
    out = leading_so_re().replace(&out, "").into_owned();
    normalize_whitespace(&out)
}

/// Collapse immediate word repetitions ("the the" → "the"), a classic STT
/// stutter artifact. Case-insensitive, keeps the first occurrence.
pub fn collapse_false_starts(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<&str> = Vec::with_capacity(words.len());
    for w in words {
        let strip = |s: &str| s.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        if let Some(last) = out.last() {
            if !strip(w).is_empty() && strip(last) == strip(w) {
                continue;
            }
        }
        out.push(w);
    }
    out.join(" ")
}

pub fn normalize_whitespace(text: &str) -> String {
    // Preserve intentional newlines; collapse runs of spaces/tabs.
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        lines.push(line.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    let joined = lines.join("\n");
    // Trim outer whitespace but keep interior newlines.
    joined.trim().to_string()
}

/// Weekdays/months that STT often leaves lowercase.
const PROPER_WORDS: &[&str] = &[
    "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday", "january",
    "february", "march", "april", "june", "july", "august", "september", "october", "november",
    "december",
];

/// Punctuation + capitalization pass:
/// * normalize whitespace and spacing around punctuation,
/// * capitalize sentence starts and standalone "i",
/// * capitalize common proper words (weekdays/months),
/// * ensure the text ends with a sentence terminator.
pub fn punctuate_and_capitalize(text: &str) -> String {
    let mut t = normalize_whitespace(text);
    if t.is_empty() {
        return t;
    }

    // Space normalization around punctuation: "hello , world" -> "hello, world".
    static SPACE_BEFORE: OnceLock<Regex> = OnceLock::new();
    let sb = SPACE_BEFORE.get_or_init(|| Regex::new(r"\s+([,.;:!?])").unwrap());
    t = sb.replace_all(&t, "$1").into_owned();
    static SPACE_AFTER: OnceLock<Regex> = OnceLock::new();
    let sa = SPACE_AFTER.get_or_init(|| Regex::new(r"([,.;:!?])([^\s\d.\n])").unwrap());
    t = sa.replace_all(&t, "$1 $2").into_owned();

    // Standalone "i" -> "I".
    static LONE_I: OnceLock<Regex> = OnceLock::new();
    let li = LONE_I.get_or_init(|| Regex::new(r"\bi\b").unwrap());
    t = li.replace_all(&t, "I").into_owned();
    static I_CONTRACT: OnceLock<Regex> = OnceLock::new();
    let ic = I_CONTRACT.get_or_init(|| Regex::new(r"\bi'(m|ll|ve|d)\b").unwrap());
    t = ic.replace_all(&t, "I'$1").into_owned();

    // Proper words.
    for w in PROPER_WORDS {
        let re = Regex::new(&format!(r"(?i)\b{w}\b")).unwrap();
        let cap = {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        };
        t = re.replace_all(&t, cap.as_str()).into_owned();
    }

    // Capitalize after sentence enders and at start (per line).
    t = capitalize_sentence_starts(&t);

    // Final terminator (skip if the text ends with any terminator or looks
    // like a list/code-ish line).
    let last = t.chars().last().unwrap_or(' ');
    if !matches!(last, '.' | '!' | '?' | ':' | ';' | '\n') {
        t.push('.');
    }
    t
}

fn capitalize_sentence_starts(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut capitalize_next = true;
    for c in text.chars() {
        if capitalize_next && c.is_alphabetic() {
            result.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(c);
            match c {
                '.' | '!' | '?' | '\n' => capitalize_next = true,
                c if c.is_alphanumeric() => capitalize_next = false,
                _ => {}
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_common_fillers() {
        assert_eq!(
            remove_fillers("um i think uh we should you know try it"),
            "i think we should try it"
        );
    }

    #[test]
    fn removes_comma_like_but_keeps_verb_like() {
        assert_eq!(remove_fillers("it was, like, huge"), "it was huge");
        assert_eq!(remove_fillers("i like pizza"), "i like pizza");
    }

    #[test]
    fn collapses_repeats() {
        assert_eq!(collapse_false_starts("we we should ship the the thing"), "we should ship the thing");
    }

    #[test]
    fn removes_cutoff_fragments() {
        assert_eq!(remove_fillers("i wa- i want this"), "i i want this");
        // (repeat collapse then removes the doubled "i")
        assert_eq!(collapse_false_starts(&remove_fillers("i wa- i want this")), "i want this");
    }

    #[test]
    fn punctuation_and_caps() {
        assert_eq!(
            punctuate_and_capitalize("hello world how are you"),
            "Hello world how are you."
        );
        assert_eq!(punctuate_and_capitalize("this works. it really does"), "This works. It really does.");
        assert_eq!(punctuate_and_capitalize("i'm sure i can"), "I'm sure I can.");
        assert_eq!(punctuate_and_capitalize("see you friday"), "See you Friday.");
        assert_eq!(punctuate_and_capitalize("wait , what ?"), "Wait, what?");
    }

    #[test]
    fn preserves_existing_terminator_and_newlines() {
        assert_eq!(punctuate_and_capitalize("done!"), "Done!");
        assert_eq!(punctuate_and_capitalize("a\nb"), "A\nB.");
    }
}
