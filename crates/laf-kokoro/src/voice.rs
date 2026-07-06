//! Voice style vectors. Verbatim from kokoroxide v0.1.5 (MIT/Apache-2.0),
//! error type adapted. Voice files are raw little-endian f32 arrays shaped
//! (N, 1, 256): one 256-dim style vector per possible token length.

use crate::{KokoroError, Result};
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Clone)]
pub struct VoiceStyle {
    pub data: Vec<f32>,
    pub vector_size: usize,
}

impl VoiceStyle {
    pub fn new(data: Vec<f32>, vector_size: usize) -> Self {
        VoiceStyle { data, vector_size }
    }

    pub fn get_style_vector(&self, size: usize) -> Vec<f32> {
        let mut result = self.data.iter().take(size).cloned().collect::<Vec<f32>>();
        while result.len() < size {
            result.push(0.0);
        }
        result
    }

    /// Select the style vector by token length — mirrors the reference
    /// Python implementation's `voices[len(tokens)]` indexing.
    pub fn get_style_vector_for_token_length(
        &self,
        token_length: usize,
        vector_size: usize,
    ) -> Vec<f32> {
        let offset = token_length * self.vector_size;
        if offset + vector_size <= self.data.len() {
            self.data[offset..offset + vector_size].to_vec()
        } else {
            let last_vector_start = (self.data.len() / self.vector_size) * self.vector_size;
            if last_vector_start + vector_size <= self.data.len() {
                self.data[last_vector_start..last_vector_start + vector_size].to_vec()
            } else {
                self.get_style_vector(vector_size)
            }
        }
    }
}

pub fn load_voice_style<P: AsRef<Path>>(path: P) -> Result<VoiceStyle> {
    let mut file = File::open(&path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    if buffer.len() < 4 {
        return Err(KokoroError::Other(format!(
            "voice file too small: {}",
            path.as_ref().display()
        )));
    }
    let style_data: Vec<f32> = buffer
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    Ok(VoiceStyle::new(style_data, 256))
}
