//! Linux text insertion — a runtime-detected fallback chain.
//!
//! Order and trade-offs:
//!
//! 1. **AT-SPI2 EditableText** (cleanest): inserts directly into the focused
//!    widget via the accessibility bus. No clipboard side effects, no fake
//!    input, works identically on X11 and Wayland. Coverage varies: GTK/Qt
//!    apps with a11y enabled expose it; Electron/Chromium only with
//!    `--force-renderer-accessibility`; some apps not at all → fall through.
//!
//! 2. **Synthetic typing**:
//!    * Wayland: `wtype` (zwp_virtual_keyboard_v1 — wlroots compositors like
//!      Sway/Hyprland; GNOME Mutter and older KWin do not expose it) → else
//!      `ydotool` (writes through /dev/uinput at the kernel level, fully
//!      compositor-agnostic, but needs ydotoold running and uinput access —
//!      see doctor).
//!    * X11: `xdotool type` (XTEST + keymap remapping for arbitrary
//!      unicode).
//!
//! 3. **Clipboard paste**: save clipboard → set text → synthesize Ctrl+V
//!    (wtype/ydotool on Wayland, native XTEST via x11rb on X11 — no external
//!    tool needed for a plain chord) → restore clipboard after a delay.
//!    Most compatible; caveats: terminals want Ctrl+Shift+V, and clipboard
//!    managers may capture the transient entry.

use crate::session::{detect_session, which, ydotoold_socket, SessionType};
use arboard::{Clipboard, GetExtLinux, LinuxClipboardKind, SetExtLinux};
use laf_core::traits::TextInserter;
use laf_core::types::{EngineError, EngineResult, InsertionMethod, InsertionReport};
use std::process::Command;
use std::time::{Duration, Instant};

pub struct LinuxInserter {
    session: SessionType,
    rt: tokio::runtime::Runtime,
}

impl LinuxInserter {
    pub fn new() -> EngineResult<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| EngineError::Other(format!("tokio runtime: {e}")))?;
        Ok(Self { session: detect_session(), rt })
    }
}

impl TextInserter for LinuxInserter {
    fn insert_text(&self, text: &str) -> EngineResult<InsertionReport> {
        let t0 = Instant::now();
        let mut notes: Vec<String> = Vec::new();

        // ---- Rung 1: AT-SPI2 EditableText -------------------------------
        match self.rt.block_on(async {
            tokio::time::timeout(Duration::from_millis(1200), crate::atspi_insert::insert(text))
                .await
                .map_err(|_| EngineError::Insertion("AT-SPI2 timed out".into()))?
        }) {
            Ok(()) => {
                return Ok(InsertionReport {
                    method: InsertionMethod::AtspiEditableText,
                    chars: text.chars().count(),
                    elapsed_ms: t0.elapsed().as_millis() as u64,
                    fallback_notes: notes,
                });
            }
            Err(e) => notes.push(format!("AT-SPI2: {e}")),
        }

        // ---- Rung 2: synthetic typing ------------------------------------
        match self.session {
            SessionType::Wayland => {
                if let Some(wtype) = which("wtype") {
                    match Command::new(&wtype).arg("--").arg(text).output() {
                        Ok(out) if out.status.success() => {
                            return Ok(report(
                                InsertionMethod::SyntheticKeys { tool: "wtype".into() },
                                text,
                                t0,
                                notes,
                            ));
                        }
                        Ok(out) => notes.push(format!(
                            "wtype failed (compositor without zwp_virtual_keyboard_v1?): {}",
                            String::from_utf8_lossy(&out.stderr).trim()
                        )),
                        Err(e) => notes.push(format!("wtype: {e}")),
                    }
                } else {
                    notes.push("wtype not installed".into());
                }
                if which("ydotool").is_some() {
                    if ydotoold_socket().is_some() {
                        match Command::new("ydotool").args(["type", "--"]).arg(text).output() {
                            Ok(out) if out.status.success() => {
                                return Ok(report(
                                    InsertionMethod::SyntheticKeys { tool: "ydotool".into() },
                                    text,
                                    t0,
                                    notes,
                                ));
                            }
                            Ok(out) => notes.push(format!(
                                "ydotool type failed: {}",
                                String::from_utf8_lossy(&out.stderr).trim()
                            )),
                            Err(e) => notes.push(format!("ydotool: {e}")),
                        }
                    } else {
                        notes.push("ydotoold not running (systemctl --user start ydotool)".into());
                    }
                } else {
                    notes.push("ydotool not installed".into());
                }
            }
            SessionType::X11 | SessionType::Unknown => {
                if which("xdotool").is_some() {
                    match Command::new("xdotool")
                        .args(["type", "--clearmodifiers", "--delay", "2", "--"])
                        .arg(text)
                        .output()
                    {
                        Ok(out) if out.status.success() => {
                            return Ok(report(
                                InsertionMethod::SyntheticKeys { tool: "xdotool".into() },
                                text,
                                t0,
                                notes,
                            ));
                        }
                        Ok(out) => notes.push(format!(
                            "xdotool type failed: {}",
                            String::from_utf8_lossy(&out.stderr).trim()
                        )),
                        Err(e) => notes.push(format!("xdotool: {e}")),
                    }
                } else {
                    notes.push("xdotool not installed (typing rung skipped)".into());
                }
            }
        }

        // ---- Rung 3: clipboard + paste chord -----------------------------
        match self.clipboard_paste(text, &mut notes) {
            Ok(tool) => Ok(report(InsertionMethod::ClipboardPaste { paste_tool: tool }, text, t0, notes)),
            Err(e) => Err(EngineError::Insertion(format!(
                "all insertion methods failed ({}); last error: {e}. Run the doctor (tray → Setup check) for exact fixes.",
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

impl LinuxInserter {
    fn clipboard_paste(&self, text: &str, notes: &mut Vec<String>) -> EngineResult<String> {
        let mut cb = Clipboard::new()
            .map_err(|e| EngineError::Insertion(format!("clipboard unavailable: {e}")))?;
        let previous = cb.get_text().ok();
        cb.set_text(text.to_string())
            .map_err(|e| EngineError::Insertion(format!("clipboard set failed: {e}")))?;
        // Give the compositor/apps a moment to see the new clipboard owner.
        std::thread::sleep(Duration::from_millis(60));

        let tool = self.synthesize_paste_chord(notes)?;

        // Restore the previous clipboard after the target app has had time to
        // request the data. Done on a background thread: on Wayland *we* are
        // the data source, so restoring too early would corrupt the paste.
        if let Some(prev) = previous {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(700));
                if let Ok(mut cb) = Clipboard::new() {
                    let _ = cb.set_text(prev);
                }
            });
        }
        Ok(tool)
    }

    /// Press Ctrl+V using the best available mechanism for the session.
    fn synthesize_paste_chord(&self, notes: &mut Vec<String>) -> EngineResult<String> {
        match self.session {
            SessionType::Wayland => {
                if which("wtype").is_some() {
                    // -M/-m press/release a modifier around the key.
                    let ok = Command::new("wtype")
                        .args(["-M", "ctrl", "-k", "v", "-m", "ctrl"])
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);
                    if ok {
                        return Ok("wtype".into());
                    }
                    notes.push("wtype paste chord failed".into());
                }
                if which("ydotool").is_some() && ydotoold_socket().is_some() {
                    // Linux input codes: KEY_LEFTCTRL=29, KEY_V=47.
                    let ok = Command::new("ydotool")
                        .args(["key", "29:1", "47:1", "47:0", "29:0"])
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);
                    if ok {
                        return Ok("ydotool".into());
                    }
                    notes.push("ydotool paste chord failed".into());
                }
                Err(EngineError::Insertion(
                    "no way to synthesize Ctrl+V on this Wayland session (install wtype, or ydotool + ydotoold)".into(),
                ))
            }
            SessionType::X11 | SessionType::Unknown => {
                crate::x11_input::send_ctrl_v().map(|_| "xtest".to_string()).map_err(|e| {
                    notes.push(format!("XTEST: {e}"));
                    EngineError::Insertion(format!("XTEST Ctrl+V failed: {e}"))
                })
            }
        }
    }
}

/// Read the PRIMARY selection (X11 & wlroots-Wayland) — used by TTS.
pub fn primary_selection() -> Option<String> {
    let mut cb = Clipboard::new().ok()?;
    cb.get().clipboard(LinuxClipboardKind::Primary).text().ok().filter(|s| !s.is_empty())
}

/// Set clipboard (used by the selection reader's Ctrl+C fallback restore).
pub fn set_clipboard(text: String) -> bool {
    Clipboard::new()
        .and_then(|mut cb| cb.set().clipboard(LinuxClipboardKind::Clipboard).text(text))
        .is_ok()
}
