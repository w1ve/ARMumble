//! The stereo test clip: white noise in both ears, then left only, then right
//! only, so the other position can check channel separation by ear.

use std::sync::{Arc, OnceLock};

/// 48 kHz, 2 channels, 16-bit PCM.
static WAV: &[u8] = include_bytes!("../assets/stereotest.wav");

static CLIP: OnceLock<Result<Arc<[f32]>, String>> = OnceLock::new();

/// The clip as interleaved stereo floats at 48 kHz (decoded once).
pub fn clip() -> Result<Arc<[f32]>, String> {
    CLIP.get_or_init(|| decode(WAV)).clone()
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes([*b.get(i)?, *b.get(i + 1)?, *b.get(i + 2)?, *b.get(i + 3)?]))
}

/// Decode a 48 kHz stereo 16-bit PCM WAV.
fn decode(b: &[u8]) -> Result<Arc<[f32]>, String> {
    if b.get(0..4) != Some(b"RIFF") || b.get(8..12) != Some(b"WAVE") {
        return Err("stereo test: not a WAV file".into());
    }
    let mut i = 12;
    let mut format = None;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = u32_at(b, i + 4).ok_or("stereo test: bad chunk")? as usize;
        let body = i + 8;
        if id == b"fmt " {
            format = Some((u16_at(b, body), u16_at(b, body + 2), u32_at(b, body + 4), u16_at(b, body + 14)));
        } else if id == b"data" {
            if format != Some((Some(1), Some(2), Some(48_000), Some(16))) {
                return Err("stereo test: the clip must be 48 kHz stereo 16-bit PCM".into());
            }
            let end = (body + len).min(b.len());
            let pcm: Vec<f32> = b[body..end]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|s| f32::from(i16::from_le_bytes(*s)) / 32_768.0)
                .collect();
            return Ok(pcm.into());
        }
        i = body + len + (len & 1);
    }
    Err("stereo test: no audio in the WAV file".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_clip_decodes() {
        let c = clip().expect("clip");
        // About 42 s of stereo.
        assert!(c.len() > 48_000 * 2 * 30, "{} samples", c.len());
        assert_eq!(c.len() % 2, 0);
    }
}
