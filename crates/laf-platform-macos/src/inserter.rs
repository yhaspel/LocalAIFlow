//! macOS text insertion — the three-rung chain, least invasive first.
//!
//! 1. **AX selected-text replacement**: write `AXSelectedText` on the focused
//!    element. Replaces the selection or inserts at the caret. No clipboard
//!    side effects, no synthetic events; supported by native text views and
//!    most Catalyst/Electron apps with AX enabled. Falls through when the
//!    attribute isn't settable (e.g. some terminals, games).
//! 2. **CGEvent unicode typing**: keyboard events carrying UTF-16 payloads
//!    via `CGEventKeyboardSetUnicodeString` — types anything, including
//!    emoji, regardless of keyboard layout. Slightly slower (chunked), and
//!    apps with custom key handling may reorder very fast input.
//! 3. **Pasteboard + ⌘V**: save NSPasteboard contents (text), set ours,
//!    synthesize ⌘V, restore after a delay. Most compatible; briefly
//!    occupies the clipboard and clipboard managers may record the entry.
//!
//! Both rungs 2 and 3 post events to other apps and therefore require the
//! Accessibility permission (checked up front with a pointer to onboarding).

use crate::ax;
use arboard::Clipboard;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use laf_core::traits::TextInserter;
use laf_core::types::{EngineError, EngineResult, InsertionMethod, InsertionReport};
use std::time::{Duration, Instant};

const KEYCODE_V: u16 = 9; // kVK_ANSI_V
/// UTF-16 units per synthetic key event (CGEvent payload limit is 20).
const TYPE_CHUNK: usize = 18;

pub struct MacInserter;

impl MacInserter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MacInserter {
    fn default() -> Self {
        Self::new()
    }
}

impl TextInserter for MacInserter {
    fn insert_text(&self, text: &str) -> EngineResult<InsertionReport> {
        let t0 = Instant::now();
        let mut notes: Vec<String> = Vec::new();

        if unsafe { ax::AXIsProcessTrusted() } == 0 {
            return Err(EngineError::Permission(
                "Accessibility permission is required to insert text. \
                 Grant it in System Settings → Privacy & Security → Accessibility \
                 (the onboarding screen has a button for this)."
                    .into(),
            ));
        }

        // ---- Rung 1: AX selected-text replacement ------------------------
        match ax::focused_element() {
            Ok(focused) => {
                let el = focused.0 as ax::AXUIElementRef;
                if ax::attr_settable(el, "AXSelectedText") {
                    match ax::set_string_attr(el, "AXSelectedText", text) {
                        Ok(()) => {
                            return Ok(report(InsertionMethod::AxDirect, text, t0, notes));
                        }
                        Err(e) => notes.push(format!("AXSelectedText set failed (AXError {e})")),
                    }
                } else {
                    notes.push("focused element does not accept AXSelectedText".into());
                }
            }
            Err(e) => notes.push(format!("AX focus lookup: {e}")),
        }

        // ---- Rung 2: CGEvent unicode typing ------------------------------
        match type_unicode(text) {
            Ok(()) => {
                return Ok(report(
                    InsertionMethod::SyntheticKeys { tool: "cgevent-unicode".into() },
                    text,
                    t0,
                    notes,
                ))
            }
            Err(e) => notes.push(format!("CGEvent typing: {e}")),
        }

        // ---- Rung 3: pasteboard + ⌘V --------------------------------------
        match paste_via_clipboard(text) {
            Ok(()) => Ok(report(
                InsertionMethod::ClipboardPaste { paste_tool: "cmd-v".into() },
                text,
                t0,
                notes,
            )),
            Err(e) => Err(EngineError::Insertion(format!(
                "all macOS insertion methods failed ({}); last error: {e}",
                notes.join(" | ")
            ))),
        }
    }
}

fn report(method: InsertionMethod, text: &str, t0: Instant, notes: Vec<String>) -> InsertionReport {
    InsertionReport {
        method,
        chars: text.chars().count(),
        elapsed_ms: t0.elapsed().as_millis() as u64,
        fallback_notes: notes,
    }
}

fn event_source() -> Result<CGEventSource, String> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "could not create CGEventSource".to_string())
}

/// Type arbitrary text using unicode-payload keyboard events.
pub fn type_unicode(text: &str) -> Result<(), String> {
    let source = event_source()?;
    let utf16: Vec<u16> = text.encode_utf16().collect();
    for chunk in utf16.chunks(TYPE_CHUNK) {
        // Key-down carrying the unicode payload, then a matching key-up.
        let down = CGEvent::new_keyboard_event(source.clone(), 0, true)
            .map_err(|_| "CGEvent keyboard (down) failed".to_string())?;
        down.set_string_from_utf16_unchecked(chunk);
        down.post(CGEventTapLocation::HID);
        let up = CGEvent::new_keyboard_event(source.clone(), 0, false)
            .map_err(|_| "CGEvent keyboard (up) failed".to_string())?;
        up.set_string_from_utf16_unchecked(chunk);
        up.post(CGEventTapLocation::HID);
        // Tiny pacing gap keeps slow apps from dropping events.
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

/// Synthesize ⌘V (or ⌘C with `copy = true`).
pub fn send_cmd_key(keycode: u16) -> Result<(), String> {
    let source = event_source()?;
    let down = CGEvent::new_keyboard_event(source.clone(), keycode, true)
        .map_err(|_| "CGEvent chord (down) failed".to_string())?;
    down.set_flags(CGEventFlags::CGEventFlagCommand);
    down.post(CGEventTapLocation::HID);
    let up = CGEvent::new_keyboard_event(source, keycode, false)
        .map_err(|_| "CGEvent chord (up) failed".to_string())?;
    up.set_flags(CGEventFlags::CGEventFlagCommand);
    up.post(CGEventTapLocation::HID);
    Ok(())
}

fn paste_via_clipboard(text: &str) -> Result<(), String> {
    let mut cb = Clipboard::new().map_err(|e| format!("pasteboard unavailable: {e}"))?;
    let previous = cb.get_text().ok();
    cb.set_text(text.to_string()).map_err(|e| format!("pasteboard set failed: {e}"))?;
    std::thread::sleep(Duration::from_millis(50));
    send_cmd_key(KEYCODE_V)?;
    // Restore after the target has read the data (pasteboard reads are
    // synchronous on macOS, but slow apps exist — 600 ms is safe).
    if let Some(prev) = previous {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            if let Ok(mut cb) = Clipboard::new() {
                let _ = cb.set_text(prev);
            }
        });
    }
    Ok(())
}
