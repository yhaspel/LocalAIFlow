//! Read the current text selection (for the TTS "read selection" hotkey).
//!
//! Chain:
//! 1. PRIMARY selection — instant and side-effect-free. Works on X11 always;
//!    on Wayland it needs the data-control protocol (wlroots compositors).
//! 2. Clipboard round-trip: save clipboard → synthesize Ctrl+C → read →
//!    restore. Works everywhere a copy chord can be synthesized.

use crate::inserter::primary_selection;
use crate::session::{detect_session, which, ydotoold_socket, SessionType};
use arboard::Clipboard;
use laf_core::traits::SelectionReader;
use laf_core::types::{EngineError, EngineResult};
use std::process::Command;
use std::time::Duration;

pub struct LinuxSelectionReader {
    session: SessionType,
}

impl LinuxSelectionReader {
    pub fn new() -> Self {
        Self { session: detect_session() }
    }
}

impl Default for LinuxSelectionReader {
    fn default() -> Self {
        Self::new()
    }
}

impl SelectionReader for LinuxSelectionReader {
    fn read_selection(&self) -> EngineResult<Option<String>> {
        // Rung 1: PRIMARY selection.
        if let Some(text) = primary_selection() {
            return Ok(Some(text));
        }

        // Rung 2: clipboard round-trip via synthetic Ctrl+C.
        let mut cb = Clipboard::new()
            .map_err(|e| EngineError::Tts(format!("clipboard unavailable: {e}")))?;
        let previous = cb.get_text().ok();

        let copied = match self.session {
            SessionType::Wayland => {
                if which("wtype").is_some()
                    && Command::new("wtype")
                        .args(["-M", "ctrl", "-k", "c", "-m", "ctrl"])
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false)
                {
                    true
                } else {
                    which("ydotool").is_some()
                        && ydotoold_socket().is_some()
                        && Command::new("ydotool")
                            .args(["key", "29:1", "46:1", "46:0", "29:0"]) // CTRL+C
                            .status()
                            .map(|s| s.success())
                            .unwrap_or(false)
                }
            }
            SessionType::X11 | SessionType::Unknown => crate::x11_input::send_ctrl_c().is_ok(),
        };
        if !copied {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(250));

        let grabbed = cb.get_text().ok().filter(|t| !t.is_empty() && Some(t) != previous.as_ref());

        // Restore the user's clipboard regardless of outcome.
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
