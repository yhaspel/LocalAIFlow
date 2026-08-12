//! Minimal native XTEST helpers (x11rb): synthesize key chords without
//! external tools. Arbitrary-unicode *typing* on X11 needs keymap remapping
//! (what xdotool does); for plain chords like Ctrl+V / Ctrl+C the standard
//! keymap already has the keycodes, so this stays small and dependency-free.

use laf_core::types::{EngineError, EngineResult};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt as XprotoExt;
use x11rb::protocol::xtest::ConnectionExt as XtestExt;

const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const KEYSYM_CONTROL_L: u32 = 0xffe3;
const KEYSYM_V: u32 = 0x0076;
const KEYSYM_C: u32 = 0x0063;

pub fn send_ctrl_v() -> EngineResult<()> {
    send_chord(KEYSYM_CONTROL_L, KEYSYM_V)
}

pub fn send_ctrl_c() -> EngineResult<()> {
    send_chord(KEYSYM_CONTROL_L, KEYSYM_C)
}

fn send_chord(modifier_sym: u32, key_sym: u32) -> EngineResult<()> {
    let (conn, screen_num) =
        x11rb::connect(None).map_err(|e| EngineError::Insertion(format!("X11 connect: {e}")))?;
    let root = conn.setup().roots[screen_num].root;

    let modifier = keycode_for(&conn, modifier_sym)?
        .ok_or_else(|| EngineError::Insertion("no keycode for Control".into()))?;
    let key = keycode_for(&conn, key_sym)?
        .ok_or_else(|| EngineError::Insertion("no keycode for target key".into()))?;

    for (kc, kind) in
        [(modifier, KEY_PRESS), (key, KEY_PRESS), (key, KEY_RELEASE), (modifier, KEY_RELEASE)]
    {
        conn.xtest_fake_input(kind, kc, x11rb::CURRENT_TIME, root, 0, 0, 0)
            .map_err(|e| EngineError::Insertion(format!("XTEST fake_input: {e}")))?;
    }
    conn.flush().map_err(|e| EngineError::Insertion(format!("X11 flush: {e}")))?;
    // Small settle so the target processes the chord before we move on.
    std::thread::sleep(std::time::Duration::from_millis(50));
    Ok(())
}

/// Scan the server keymap for a keycode producing `keysym`.
fn keycode_for(conn: &impl Connection, keysym: u32) -> EngineResult<Option<u8>> {
    let setup = conn.setup();
    let (min_kc, max_kc) = (setup.min_keycode, setup.max_keycode);
    let mapping = conn
        .get_keyboard_mapping(min_kc, max_kc - min_kc + 1)
        .map_err(|e| EngineError::Insertion(format!("get_keyboard_mapping: {e}")))?
        .reply()
        .map_err(|e| EngineError::Insertion(format!("keyboard mapping reply: {e}")))?;
    let per = mapping.keysyms_per_keycode as usize;
    for (i, chunk) in mapping.keysyms.chunks(per).enumerate() {
        if chunk.contains(&keysym) {
            return Ok(Some(min_kc + i as u8));
        }
    }
    Ok(None)
}
