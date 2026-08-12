//! Command-mode interpretation of spoken editing commands.
//!
//! Recognized commands (case-insensitive, matched on word boundaries):
//! punctuation ("period", "comma", "question mark", …), structure
//! ("new line", "new paragraph", "tab"), and editing ("delete that" /
//! "scratch that" — removes the preceding sentence).

/// How a command's replacement attaches to surrounding text.
#[derive(Clone, Copy, PartialEq)]
enum Attach {
    /// Glue to the previous word ("period", "close paren").
    Prev,
    /// Glue to the NEXT word ("open paren", "open quote").
    Next,
    /// Structural whitespace ("new line", "tab").
    Structural,
}

/// (spoken phrase, replacement, attachment) — longest phrases first so
/// "question mark" wins over any hypothetical "question".
const PUNCT_COMMANDS: &[(&str, &str, Attach)] = &[
    ("exclamation point", "!", Attach::Prev),
    ("exclamation mark", "!", Attach::Prev),
    ("question mark", "?", Attach::Prev),
    ("full stop", ".", Attach::Prev),
    ("new paragraph", "\n\n", Attach::Structural),
    ("new line", "\n", Attach::Structural),
    ("open paren", "(", Attach::Next),
    ("close paren", ")", Attach::Prev),
    ("open quote", "\"", Attach::Next),
    ("close quote", "\"", Attach::Prev),
    ("period", ".", Attach::Prev),
    ("comma", ",", Attach::Prev),
    ("colon", ":", Attach::Prev),
    ("semicolon", ";", Attach::Prev),
    ("hyphen", "-", Attach::Prev),
    ("dash", " — ", Attach::Prev),
    ("ellipsis", "…", Attach::Prev),
    ("tab", "\t", Attach::Structural),
];

const DELETE_COMMANDS: &[&str] = &["delete that", "scratch that", "undo that"];

/// Interpret spoken commands in `raw`, producing literal text.
pub fn interpret(raw: &str) -> String {
    // Work token-wise so "period" as a word converts but "periodic" doesn't.
    let lower = raw.to_lowercase();
    let mut tokens: Vec<String> = Vec::new();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let orig_words: Vec<&str> = raw.split_whitespace().collect();

    let mut i = 0usize;
    'outer: while i < words.len() {
        let clean_word = |w: &str| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string();

        // Delete commands (two-word).
        for dc in DELETE_COMMANDS {
            let parts: Vec<&str> = dc.split(' ').collect();
            if i + parts.len() <= words.len()
                && parts.iter().enumerate().all(|(k, p)| clean_word(words[i + k]) == *p)
            {
                delete_last_sentence(&mut tokens);
                i += parts.len();
                continue 'outer;
            }
        }
        // Punctuation commands (1–2 words).
        for (phrase, repl, attach) in PUNCT_COMMANDS {
            let parts: Vec<&str> = phrase.split(' ').collect();
            if i + parts.len() <= words.len()
                && parts.iter().enumerate().all(|(k, p)| clean_word(words[i + k]) == *p)
            {
                attach_punctuation(&mut tokens, repl, *attach);
                i += parts.len();
                continue 'outer;
            }
        }
        tokens.push(orig_words[i].to_string());
        i += 1;
    }

    join_tokens(&tokens)
}

/// Punctuation attaches per its [`Attach`] kind; structural whitespace
/// becomes its own token.
fn attach_punctuation(tokens: &mut Vec<String>, repl: &str, attach: Attach) {
    match attach {
        Attach::Structural => tokens.push(repl.to_string()),
        Attach::Next => tokens.push(format!("\u{0}OPEN\u{0}{repl}")),
        Attach::Prev => match tokens.last_mut() {
            Some(last) if !last.starts_with('\n') && *last != "\t" => last.push_str(repl),
            _ => tokens.push(repl.trim().to_string()),
        },
    }
}

fn delete_last_sentence(tokens: &mut Vec<String>) {
    // Remove tokens backwards until (and excluding) the previous sentence
    // terminator or paragraph break.
    while let Some(last) = tokens.last() {
        let is_boundary = last.ends_with(['.', '!', '?']) || last.starts_with('\n');
        if is_boundary {
            break;
        }
        tokens.pop();
    }
}

fn join_tokens(tokens: &[String]) -> String {
    let mut out = String::new();
    let mut pending_open: Option<String> = None;
    for t in tokens {
        if let Some(rest) = t.strip_prefix("\u{0}OPEN\u{0}") {
            pending_open = Some(rest.to_string());
            continue;
        }
        if t.starts_with('\n') || t == "\t" {
            // Structural whitespace replaces the separating space.
            while out.ends_with(' ') {
                out.pop();
            }
            out.push_str(t);
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') && !out.ends_with('\t') {
            out.push(' ');
        }
        if let Some(open) = pending_open.take() {
            out.push_str(&open);
        }
        out.push_str(t);
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punctuation_words() {
        assert_eq!(
            interpret("send the report today period thanks comma anna"),
            "send the report today. thanks, anna"
        );
        assert_eq!(interpret("really question mark"), "really?");
    }

    #[test]
    fn structural_commands() {
        assert_eq!(interpret("first item new line second item"), "first item\nsecond item");
        assert_eq!(interpret("intro new paragraph body"), "intro\n\nbody");
    }

    #[test]
    fn delete_that_removes_previous_sentence() {
        assert_eq!(interpret("keep this period drop all of that delete that"), "keep this.");
        // Deleting with nothing before is harmless.
        assert_eq!(interpret("scratch that hello"), "hello");
    }

    #[test]
    fn words_containing_commands_are_safe() {
        assert_eq!(interpret("the periodic table"), "the periodic table");
        assert_eq!(interpret("a common cause"), "a common cause");
    }

    #[test]
    fn open_close_pairs() {
        assert_eq!(interpret("open paren nice close paren"), "(nice)");
        assert_eq!(interpret("open quote hello close quote"), "\"hello\"");
    }
}
