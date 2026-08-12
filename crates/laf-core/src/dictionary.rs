//! Custom dictionary: user-maintained names/jargon/spellings applied as a
//! normalization pass after cleanup, and fed to the STT engine as vocabulary
//! hints where supported.

use regex::{escape, Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DictEntry {
    /// What the STT engine tends to produce ("cuber netties").
    pub from: String,
    /// What the user wants ("Kubernetes").
    pub to: String,
    /// If false (default), `from` matches case-insensitively and the
    /// replacement adapts: a capitalized/uppercased match keeps that shape.
    #[serde(default)]
    pub match_case: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Dictionary {
    entries: Vec<(DictEntry, Regex)>,
}

impl Dictionary {
    pub fn new(entries: &[DictEntry]) -> Self {
        let compiled = entries
            .iter()
            .filter(|e| !e.from.trim().is_empty())
            .filter_map(|e| {
                // Word-ish boundaries: don't fire inside larger words, but do
                // allow multi-word phrases. `\b` only works next to word
                // characters, so only guard edges that ARE word characters
                // (entries like "c++" need an unguarded right edge).
                let trimmed = e.from.trim();
                let left =
                    if trimmed.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        r"\b"
                    } else {
                        ""
                    };
                let right =
                    if trimmed.chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        r"\b"
                    } else {
                        ""
                    };
                let pattern = format!("{left}{}{right}", escape(trimmed));
                RegexBuilder::new(&pattern)
                    .case_insensitive(!e.match_case)
                    .build()
                    .ok()
                    .map(|re| (e.clone(), re))
            })
            .collect();
        Self { entries: compiled }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Phrases handed to the STT engine as vocabulary hints.
    pub fn hint_phrases(&self) -> Vec<String> {
        self.entries.iter().map(|(e, _)| e.to.clone()).collect()
    }

    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (entry, re) in &self.entries {
            out = re
                .replace_all(&out, |caps: &regex::Captures| {
                    let matched = caps.get(0).map(|m| m.as_str()).unwrap_or("");
                    adapt_case(matched, &entry.to, entry.match_case)
                })
                .into_owned();
        }
        out
    }
}

/// Preserve the *shape* of the matched text when the entry is
/// case-insensitive: "kubernetes" -> to as-is; "Kubernetes" -> capitalized;
/// "KUBERNETES" -> uppercased (only when the match is all-caps and longer
/// than one char).
fn adapt_case(matched: &str, replacement: &str, match_case: bool) -> String {
    if match_case {
        return replacement.to_string();
    }
    let letters: Vec<char> = matched.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() > 1 && letters.iter().all(|c| c.is_uppercase()) {
        return replacement.to_uppercase();
    }
    if letters.first().is_some_and(|c| c.is_uppercase())
        && !replacement.chars().next().is_some_and(|c| c.is_uppercase())
    {
        let mut chars = replacement.chars();
        return match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        };
    }
    replacement.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(pairs: &[(&str, &str)]) -> Dictionary {
        let entries: Vec<DictEntry> = pairs
            .iter()
            .map(|(f, t)| DictEntry { from: f.to_string(), to: t.to_string(), match_case: false })
            .collect();
        Dictionary::new(&entries)
    }

    #[test]
    fn replaces_word_with_boundaries() {
        let d = dict(&[("cuber netties", "Kubernetes")]);
        assert_eq!(d.apply("we deploy on cuber netties today"), "we deploy on Kubernetes today");
    }

    #[test]
    fn does_not_fire_inside_words() {
        let d = dict(&[("cat", "dog")]);
        assert_eq!(d.apply("concatenate the cat file"), "concatenate the dog file");
    }

    #[test]
    fn case_adaptation() {
        let d = dict(&[("acme corp", "AcmeCorp")]);
        assert_eq!(d.apply("Acme corp shipped"), "AcmeCorp shipped");
        let d2 = dict(&[("sql", "SQL")]);
        assert_eq!(d2.apply("the sql query"), "the SQL query");
    }

    #[test]
    fn case_sensitive_entry() {
        let entries = vec![DictEntry { from: "Jon".into(), to: "John".into(), match_case: true }];
        let d = Dictionary::new(&entries);
        assert_eq!(d.apply("Jon met jon"), "John met jon");
    }

    #[test]
    fn special_chars_escaped() {
        let d = dict(&[("c++", "C++")]);
        assert_eq!(d.apply("i like c++ a lot"), "i like C++ a lot");
    }
}
