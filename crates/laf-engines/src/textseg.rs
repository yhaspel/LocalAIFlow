//! Sentence chunking for streaming TTS: split text into speakable chunks so
//! the first audio starts quickly, merging very short sentences to avoid
//! choppy prosody.

pub fn tts_chunks(text: &str, target_min_chars: usize) -> Vec<String> {
    let sentences = laf_core::modes::split_sentences(text);
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for s in sentences {
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(s);
        if current.chars().count() >= target_min_chars {
            chunks.push(std::mem::take(&mut current));
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() && !text.trim().is_empty() {
        chunks.push(text.trim().to_string());
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_short_sentences() {
        let c = tts_chunks("Hi. Ok. This is a considerably longer sentence for testing.", 20);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0], "Hi. Ok. This is a considerably longer sentence for testing.".split(" This").next().map(|s| s.to_string()).unwrap_or_default().trim());
    }

    #[test]
    fn single_long_text_is_one_chunk() {
        let c = tts_chunks("word ".repeat(10).trim(), 200);
        assert_eq!(c.len(), 1);
    }
}
