//! espeak IPA → Misaki phonemes → Kokoro token ids. Verbatim from
//! kokoroxide v0.1.5 (MIT/Apache-2.0); only the error type and debug prints
//! were adapted. The Misaki mapping table mirrors the reference Python
//! implementation (`FROM_ESPEAKS`, longest-first).

use super::g2p::EspeakG2P;
use crate::{KokoroError, Result};
use std::collections::HashMap;

pub struct EspeakIpaTokenizer {
    vocab: HashMap<String, i64>,
    bos_id: i64,
    eos_id: i64,
    model_max_length: usize,
    g2p: EspeakG2P,
    max_token_chars: usize,
}

impl EspeakIpaTokenizer {
    pub fn new(vocab: HashMap<String, i64>) -> Result<Self> {
        let bos_id = *vocab
            .get("$")
            .ok_or_else(|| KokoroError::Tokenizer("BOS token '$' not found in vocab".into()))?;
        let eos_id = bos_id;
        let g2p = EspeakG2P::new()?;
        let max_token_chars = Self::max_token_chars(&vocab);
        Ok(Self { vocab, bos_id, eos_id, model_max_length: 512, g2p, max_token_chars })
    }

    pub fn with_model_max_length(mut self, max_length: usize) -> Self {
        self.model_max_length = max_length;
        self
    }

    /// Convert espeak IPA to Misaki phonemes (Kokoro's expected notation).
    fn espeak_ipa_to_misaki(&self, ipa: &str) -> String {
        let mut result = ipa.replace('\u{0361}', "^");

        // FROM_ESPEAKS, sorted longest-first (order matters).
        let from_espeaks = [
            ("ʔˌn\u{0329}", "tᵊn"),
            ("a^ɪ", "I"),
            ("a^ʊ", "W"),
            ("d^ʒ", "ʤ"),
            ("e^ɪ", "A"),
            ("t^ʃ", "ʧ"),
            ("ɔ^ɪ", "Y"),
            ("ə^l", "ᵊl"),
            ("ʔn", "tᵊn"),
            ("ɚ", "əɹ"),
            ("ʲO", "jO"),
            ("ʲQ", "jQ"),
            ("\u{0303}", ""),
            ("e", "A"),
            ("r", "ɹ"),
            ("x", "k"),
            ("ç", "k"),
            ("ɐ", "ə"),
            ("ɬ", "l"),
            ("ʔ", "t"),
            ("ʲ", ""),
        ];
        for (old, new) in from_espeaks {
            result = result.replace(old, new);
        }

        // Syllabic consonants: (\S)U+0329 → ᵊ\1.
        let mut chars: Vec<char> = result.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if i + 1 < chars.len() && chars[i + 1] == '\u{0329}' {
                let consonant = chars[i];
                chars[i] = 'ᵊ';
                chars[i + 1] = consonant;
                i += 2;
            } else {
                i += 1;
            }
        }
        result = chars.into_iter().collect();
        result = result.replace('\u{0329}', "");

        // American English (british = false) adjustments.
        result = result.replace("o^ʊ", "O");
        result = result.replace("ɜːɹ", "ɜɹ");
        result = result.replace("ɜː", "ɜɹ");
        result = result.replace("ɪə", "iə");
        result = result.replace('ː', "");
        result = result.replace('^', "");
        result
    }

    fn text_to_ipa(&self, text: &str) -> Result<String> {
        let ipa = self.g2p.text_to_ipa(text)?;
        let misaki = self.espeak_ipa_to_misaki(&ipa);
        if std::env::var("DEBUG_PHONEMES").is_ok() {
            eprintln!("text: {text:?}\nespeak IPA: {ipa:?}\nmisaki: {misaki:?}");
        }
        Ok(misaki)
    }

    fn max_token_chars(vocab: &HashMap<String, i64>) -> usize {
        vocab.keys().map(|k| k.chars().count()).max().unwrap_or(1)
    }

    /// Greedy longest-match tokenization against the phoneme vocab.
    pub fn tokenize_longest(&self, ipa: &str) -> Vec<i64> {
        let mut ids = Vec::with_capacity(ipa.len());
        let chars: Vec<char> = ipa.chars().collect();
        let mut i = 0;
        let max_len = self.max_token_chars;
        while i < chars.len() {
            let mut matched = false;
            let limit = max_len.min(chars.len() - i);
            for l in (1..=limit).rev() {
                let cand: String = chars[i..i + l].iter().collect();
                if let Some(&id) = self.vocab.get(&cand) {
                    ids.push(id);
                    i += l;
                    matched = true;
                    break;
                }
            }
            if !matched {
                if !chars[i].is_whitespace() {
                    tracing_or_eprintln(&format!("unknown phoneme token {:?}", chars[i]));
                }
                i += 1;
            }
        }
        ids
    }

    pub fn encode_phonemes(&self, phonemes: &str, max_length: Option<usize>) -> Result<Vec<i64>> {
        let max_len = max_length.unwrap_or(self.model_max_length);
        let mut tokens = Vec::with_capacity(phonemes.len() + 2);
        tokens.push(self.bos_id);
        let mut inner = self.tokenize_longest(phonemes);
        tokens.append(&mut inner);
        tokens.push(self.eos_id);
        Ok(truncate_keeping_bos_eos(tokens, max_len, self.bos_id, self.eos_id))
    }

    pub fn encode(&self, text: &str, max_length: Option<usize>) -> Result<Vec<i64>> {
        let max_len = max_length.unwrap_or(self.model_max_length);
        let mut tokens = Vec::with_capacity(text.len() + 2);
        tokens.push(self.bos_id);
        let ipa_text = self.text_to_ipa(text)?;
        let mut inner = self.tokenize_longest(&ipa_text);
        tokens.append(&mut inner);
        tokens.push(self.eos_id);
        Ok(truncate_keeping_bos_eos(tokens, max_len, self.bos_id, self.eos_id))
    }
}

fn truncate_keeping_bos_eos(tokens: Vec<i64>, max_len: usize, bos: i64, eos: i64) -> Vec<i64> {
    if tokens.len() <= max_len {
        return tokens;
    }
    let keep_inner = max_len.saturating_sub(2);
    let mut truncated = Vec::with_capacity(max_len);
    truncated.push(bos);
    truncated.extend_from_slice(&tokens[1..1 + keep_inner]);
    truncated.push(eos);
    truncated
}

fn tracing_or_eprintln(msg: &str) {
    // laf-kokoro deliberately avoids a tracing dependency; the parent app
    // captures stderr into its local log.
    eprintln!("laf-kokoro: {msg}");
}
