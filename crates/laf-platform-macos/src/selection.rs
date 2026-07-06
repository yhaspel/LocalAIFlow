//! Read the current selection in the frontmost app (for TTS).
//! Rung 1: `AXSelectedText` on the focused element — instant, no side
//! effects. Rung 2: pasteboard round-trip with a synthetic ⌘C.

use crate::ax;
use crate::inserter::send_cmd_key;
use arboard::Clipboard;
use laf_core::traits::SelectionReader;
use laf_core::types::{EngineError, EngineResult};
use std::time::Duration;

const KEYCODE_C: u16 = 8; // kVK_ANSI_C

pub struct MacSelectionReader;

impl MacSelectionReader {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MacSelectionReader {
    fn default() -> Self {
        Self::new()
    }
}

impl SelectionReader for MacSelectionReader {
    fn read_selection(&self) -> EngineResult<Option<String>> {
        // Rung 1: AX.
        if let Ok(focused) = ax::focused_element() {
            if let Ok(value) = ax::copy_attr(focused.0 as ax::AXUIElementRef, "AXSelectedText") {
                if let Some(text) = ax::cf_as_string(&value) {
                    if !text.is_empty() {
                        return Ok(Some(text));
                    }
                }
            }
        }

        // Rung 2: pasteboard + ⌘C (needs Accessibility for the event post).
        if unsafe { ax::AXIsProcessTrusted() } == 0 {
            return Err(EngineError::Permission(
                "reading the selection needs the Accessibility permission".into(),
            ));
        }
        let mut cb =
            Clipboard::new().map_err(|e| EngineError::Tts(format!("pasteboard: {e}")))?;
        let previous = cb.get_text().ok();
        send_cmd_key(KEYCODE_C).map_err(EngineError::Tts)?;
        std::thread::sleep(Duration::from_millis(250));
        let grabbed = cb.get_text().ok().filter(|t| !t.is_empty() && Some(t) != previous.as_ref());
        if let Some(prev) = previous {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(400));
                if let Ok(mut cb) = Clipboard::new() {
                    let _ = cb.set_text(prev);
                }
            });
        }
        Ok(grabbed)
    }
}
