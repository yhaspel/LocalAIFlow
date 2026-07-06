//! espeak-ng grapheme→phoneme FFI. Verbatim from kokoroxide v0.1.5
//! (MIT/Apache-2.0) with the global init flag switched to an atomic and the
//! error type adapted. espeak-ng's C API is stable; these signatures match
//! speak_lib.h.

use crate::{KokoroError, Result};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

#[link(name = "espeak-ng")]
extern "C" {
    fn espeak_Initialize(
        output: c_int,
        buflength: c_int,
        path: *const c_char,
        options: c_int,
    ) -> c_int;

    fn espeak_SetVoiceByName(name: *const c_char) -> c_int;

    fn espeak_TextToPhonemes(
        textptr: *const *const c_void,
        textmode: c_int,
        phonememode: c_int,
    ) -> *const c_char;

    #[allow(dead_code)]
    fn espeak_Terminate() -> c_int;
}

const AUDIO_OUTPUT_RETRIEVAL: c_int = 0x02;
const ESPEAK_PHONEMES_IPA: c_int = 0x02;
const ESPEAK_PHONEMES_SHOW_STRESS: c_int = 0x04;
const ESPEAK_PHONEMES_TIE: c_int = 0x08;
const ESPEAK_CHARS_UTF8: c_int = 1;

static INIT: Once = Once::new();
static INITIALIZED: AtomicBool = AtomicBool::new(false);

pub struct EspeakG2P;

impl EspeakG2P {
    pub fn new() -> Result<Self> {
        unsafe {
            INIT.call_once(|| {
                let result = espeak_Initialize(AUDIO_OUTPUT_RETRIEVAL, 0, std::ptr::null(), 0);
                INITIALIZED.store(result >= 0, Ordering::SeqCst);
                if INITIALIZED.load(Ordering::SeqCst) {
                    let voice_name = CString::new("en-us").expect("static cstring");
                    let result = espeak_SetVoiceByName(voice_name.as_ptr());
                    if result != 0 {
                        eprintln!("Warning: failed to set espeak voice en-us ({result})");
                    }
                }
            });
        }
        if !INITIALIZED.load(Ordering::SeqCst) {
            return Err(KokoroError::Espeak(
                "failed to initialize espeak-ng (is libespeak-ng installed?)".into(),
            ));
        }
        Ok(EspeakG2P)
    }

    /// NOTE: espeak-ng's TextToPhonemes API is not thread-safe; callers keep
    /// a single G2P per process (the tokenizer owns one and synthesis is
    /// serialized by the engine).
    pub fn text_to_ipa(&self, text: &str) -> Result<String> {
        unsafe {
            let c_text =
                CString::new(text).map_err(|e| KokoroError::Espeak(format!("nul in text: {e}")))?;
            let mut text_ptr = c_text.as_ptr() as *const c_void;
            let mut all_phonemes = String::new();

            // espeak processes one sentence per call; loop until consumed.
            loop {
                let phoneme_mode =
                    ESPEAK_PHONEMES_IPA | ESPEAK_PHONEMES_SHOW_STRESS | ESPEAK_PHONEMES_TIE;
                let phonemes_ptr =
                    espeak_TextToPhonemes(&mut text_ptr, ESPEAK_CHARS_UTF8, phoneme_mode);
                if phonemes_ptr.is_null() {
                    break;
                }
                let phonemes = CStr::from_ptr(phonemes_ptr).to_string_lossy().to_string();
                if !phonemes.is_empty() {
                    if !all_phonemes.is_empty() {
                        all_phonemes.push(' ');
                    }
                    all_phonemes.push_str(&phonemes);
                }
                if text_ptr.is_null() {
                    break;
                }
                let remaining = CStr::from_ptr(text_ptr as *const c_char).to_string_lossy();
                if remaining.trim().is_empty() {
                    break;
                }
            }

            if all_phonemes.is_empty() {
                return Err(KokoroError::Espeak("espeak-ng produced no phonemes".into()));
            }
            Ok(all_phonemes)
        }
    }
}
