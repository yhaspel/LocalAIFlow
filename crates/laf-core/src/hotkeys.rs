//! Platform-agnostic parsing of hotkey binding strings.
//!
//! Grammar: `[mod+]*<Code>` where mod ∈ {ctrl, alt, shift, super/cmd/meta}
//! and `<Code>` is a W3C `KeyboardEvent.code` name (`KeyD`, `Digit1`,
//! `Space`, `F5`, `Backquote`, …). This mirrors what the `global-hotkey`
//! crate accepts, and we translate to evdev / portal formats on Linux.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParsedBinding {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Cmd on macOS, Super/Meta on Linux.
    pub meta: bool,
    /// W3C code name, canonical casing (e.g. "KeyD").
    pub code: String,
}

impl ParsedBinding {
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut b = ParsedBinding { ctrl: false, alt: false, shift: false, meta: false, code: String::new() };
        for part in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => b.ctrl = true,
                "alt" | "option" => b.alt = true,
                "shift" => b.shift = true,
                "super" | "cmd" | "command" | "meta" | "win" => b.meta = true,
                _ => {
                    if !b.code.is_empty() {
                        return Err(format!("binding '{s}' has more than one non-modifier key"));
                    }
                    b.code = canonical_code(part)
                        .ok_or_else(|| format!("unknown key code '{part}' in binding '{s}'"))?;
                }
            }
        }
        if b.code.is_empty() {
            return Err(format!("binding '{s}' has no non-modifier key"));
        }
        Ok(b)
    }

    /// XDG GlobalShortcuts portal `preferred_trigger` description, e.g.
    /// "CTRL+ALT+d". See the portal spec's shortcut description format.
    pub fn portal_trigger(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("CTRL".into());
        }
        if self.alt {
            parts.push("ALT".into());
        }
        if self.shift {
            parts.push("SHIFT".into());
        }
        if self.meta {
            parts.push("LOGO".into());
        }
        parts.push(code_to_portal_key(&self.code));
        parts.join("+")
    }
}

/// Canonicalize a code name case-insensitively against the known set.
fn canonical_code(input: &str) -> Option<String> {
    let lower = input.to_ascii_lowercase();
    // Letters
    if let Some(rest) = lower.strip_prefix("key") {
        if rest.len() == 1 && rest.chars().all(|c| c.is_ascii_alphabetic()) {
            return Some(format!("Key{}", rest.to_ascii_uppercase()));
        }
    }
    // Single letter shorthand ("d" -> "KeyD")
    if lower.len() == 1 && lower.chars().all(|c| c.is_ascii_alphabetic()) {
        return Some(format!("Key{}", lower.to_ascii_uppercase()));
    }
    if let Some(rest) = lower.strip_prefix("digit") {
        if rest.len() == 1 && rest.chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("Digit{rest}"));
        }
    }
    if lower.len() == 1 && lower.chars().all(|c| c.is_ascii_digit()) {
        return Some(format!("Digit{lower}"));
    }
    if let Some(rest) = lower.strip_prefix('f') {
        if let Ok(n) = rest.parse::<u8>() {
            if (1..=24).contains(&n) {
                return Some(format!("F{n}"));
            }
        }
    }
    let named = [
        "Space", "Enter", "Tab", "Escape", "Backspace", "Delete", "Home", "End", "PageUp",
        "PageDown", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Minus", "Equal",
        "BracketLeft", "BracketRight", "Backslash", "Semicolon", "Quote", "Backquote", "Comma",
        "Period", "Slash", "CapsLock", "Insert",
    ];
    named.iter().find(|n| n.to_ascii_lowercase() == lower).map(|n| n.to_string())
}

/// Portal descriptions use xkb-style key names; lowercase letters work
/// broadly (KDE + GNOME implementations).
fn code_to_portal_key(code: &str) -> String {
    if let Some(l) = code.strip_prefix("Key") {
        return l.to_ascii_lowercase();
    }
    if let Some(d) = code.strip_prefix("Digit") {
        return d.to_string();
    }
    match code {
        "Space" => "space".into(),
        "Enter" => "Return".into(),
        "Escape" => "Escape".into(),
        "Tab" => "Tab".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_binding() {
        let b = ParsedBinding::parse("ctrl+alt+KeyD").unwrap();
        assert!(b.ctrl && b.alt && !b.shift && !b.meta);
        assert_eq!(b.code, "KeyD");
    }

    #[test]
    fn parses_shorthand_and_case() {
        assert_eq!(ParsedBinding::parse("CTRL+ALT+d").unwrap().code, "KeyD");
        assert_eq!(ParsedBinding::parse("shift+f5").unwrap().code, "F5");
        assert_eq!(ParsedBinding::parse("cmd+Space").unwrap().code, "Space");
    }

    #[test]
    fn rejects_garbage() {
        assert!(ParsedBinding::parse("ctrl+alt").is_err());
        assert!(ParsedBinding::parse("ctrl+flurb").is_err());
        assert!(ParsedBinding::parse("ctrl+KeyA+KeyB").is_err());
    }

    #[test]
    fn portal_format() {
        let b = ParsedBinding::parse("ctrl+alt+KeyD").unwrap();
        assert_eq!(b.portal_trigger(), "CTRL+ALT+d");
    }
}
