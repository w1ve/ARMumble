//! Elecraft K4 wire formats.
//!
//! * **CAT text** (serial port, and the plain network CAT port): ASCII
//!   commands ending in `;`, for example `TX;`, `RX;`, `TQ1;`.
//! * **K4 Remote** (network port 9205, from "K4-Remote Protocol rev. A1"):
//!   after connecting, the client sends the lowercase hex SHA-384 of the
//!   password, unframed. Everything after that is framed:
//!   `FE FD FC FB | length (u32 big-endian) | payload | FB FC FD FE`.
//!   The first payload byte is the type (0 CAT, 1 audio, 2 PAN, 3 mini-PAN).
//!   A CAT payload is `00 00 00` followed by the ASCII text.
//!
//! This client only ever sends CAT payloads. It must never send audio
//! payloads: the K4 transmits whenever audio frames arrive.

use sha2::{Digest, Sha384};

/// Start-of-frame marker.
pub const START: [u8; 4] = [0xFE, 0xFD, 0xFC, 0xFB];
/// End-of-frame marker.
pub const END: [u8; 4] = [0xFB, 0xFC, 0xFD, 0xFE];
/// Payload type of CAT text.
pub const TYPE_CAT: u8 = 0;
/// Largest payload accepted; anything bigger is a false start marker.
const MAX_PAYLOAD: usize = 512 * 1024;
/// Longest CAT command kept while waiting for its `;`.
const MAX_CAT: usize = 4096;

/// The authentication string for the K4 Remote port.
#[must_use]
pub fn password_hash(password: &str) -> String {
    Sha384::digest(password.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Wrap CAT text in a K4 Remote frame.
#[must_use]
pub fn cat_frame(text: &str) -> Vec<u8> {
    let payload_len = 3 + text.len();
    let mut out = Vec::with_capacity(12 + payload_len);
    out.extend_from_slice(&START);
    out.extend_from_slice(&u32::try_from(payload_len).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(&[TYPE_CAT, 0, 0]);
    out.extend_from_slice(text.as_bytes());
    out.extend_from_slice(&END);
    out
}

/// Splits a byte stream of CAT text into `;`-terminated commands.
#[derive(Debug, Default, Clone)]
pub struct CatSplitter {
    pending: Vec<u8>,
}

impl CatSplitter {
    /// Feed bytes; returns completed commands including their `;`.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        for &b in bytes {
            if b == b'\r' || b == b'\n' {
                continue;
            }
            self.pending.push(b);
            if b == b';' {
                let cmd = String::from_utf8_lossy(&self.pending).trim().to_owned();
                if cmd.len() > 1 {
                    out.push(cmd);
                }
                self.pending.clear();
            } else if self.pending.len() > MAX_CAT {
                self.pending.clear();
            }
        }
        out
    }
}

/// Reassembles K4 Remote frames from a byte stream and yields the CAT text
/// inside them. Audio and spectrum frames are skipped.
#[derive(Debug, Default)]
pub struct FrameReader {
    buf: Vec<u8>,
    cat: CatSplitter,
    /// Frames of any kind seen so far (proof the radio accepted the login).
    pub frames: u64,
}

impl FrameReader {
    /// Feed bytes; returns CAT commands found in complete frames.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            let Some(start) = find(&self.buf, &START) else {
                // Keep the last 3 bytes: a marker may straddle two reads.
                let keep = self.buf.len().min(3);
                let _ = self.buf.drain(..self.buf.len() - keep);
                break;
            };
            if start > 0 {
                let _ = self.buf.drain(..start);
            }
            if self.buf.len() < 8 {
                break;
            }
            let len = u32::from_be_bytes([self.buf[4], self.buf[5], self.buf[6], self.buf[7]]) as usize;
            if len > MAX_PAYLOAD {
                // Not a real frame: step past this marker and rescan.
                let _ = self.buf.drain(..1);
                continue;
            }
            let total = 8 + len + 4;
            if self.buf.len() < total {
                break;
            }
            if self.buf[8 + len..total] != END {
                let _ = self.buf.drain(..1);
                continue;
            }
            self.frames += 1;
            let payload = &self.buf[8..8 + len];
            if payload.first() == Some(&TYPE_CAT) && payload.len() > 3 {
                let text = payload[3..].to_vec();
                out.extend(self.cat.feed(&text));
            }
            let _ = self.buf.drain(..total);
        }
        out
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// What a CAT reply says about transmit, if anything: `TX;` / `TQ1;` mean
/// transmitting, `RX;` / `TQ0;` mean receiving.
#[must_use]
pub fn transmit_state(cmd: &str) -> Option<bool> {
    match cmd {
        "TX;" | "TQ1;" => Some(true),
        "RX;" | "TQ0;" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_the_elecraft_example() {
        // From "K4-Remote Protocol rev. A1": the password "tester".
        assert_eq!(
            password_hash("tester"),
            "0f06140de701f4106c3e66bf3dea1e40637daa0081ee6289cf087c105c78828f50cb3646ce687dcf03a2f795ffaf9285"
        );
    }

    #[test]
    fn cat_frame_layout() {
        let f = cat_frame("TX;");
        assert_eq!(&f[..4], &START);
        assert_eq!(&f[4..8], &[0, 0, 0, 6]);
        assert_eq!(&f[8..11], &[0, 0, 0]);
        assert_eq!(&f[11..14], b"TX;");
        assert_eq!(&f[14..], &END);
    }

    #[test]
    fn reader_extracts_cat_across_splits_and_skips_other_frames() {
        let mut audio = START.to_vec();
        audio.extend_from_slice(&5_u32.to_be_bytes());
        audio.extend_from_slice(&[1, 1, 7, 2, 0]);
        audio.extend_from_slice(&END);
        let mut stream = vec![0x55, 0xAA]; // garbage before the first frame
        stream.extend(cat_frame("PONG;TQ1;"));
        stream.extend(&audio);
        stream.extend(cat_frame("RX;"));
        let mut r = FrameReader::default();
        let mut got = Vec::new();
        for chunk in stream.chunks(5) {
            got.extend(r.feed(chunk));
        }
        assert_eq!(got, vec!["PONG;", "TQ1;", "RX;"]);
        assert_eq!(r.frames, 3);
    }

    #[test]
    fn reader_survives_a_corrupt_frame() {
        let mut bad = cat_frame("TX;");
        let last = bad.len() - 1;
        bad[last] = 0; // broken end marker
        let mut stream = bad;
        stream.extend(cat_frame("RX;"));
        let mut r = FrameReader::default();
        assert_eq!(r.feed(&stream), vec!["RX;"]);
    }

    #[test]
    fn splitter_and_transmit_state() {
        let mut s = CatSplitter::default();
        assert!(s.feed(b"TQ").is_empty());
        assert_eq!(s.feed(b"1;\r\nRX;ID017;"), vec!["TQ1;", "RX;", "ID017;"]);
        assert_eq!(transmit_state("TQ1;"), Some(true));
        assert_eq!(transmit_state("RX;"), Some(false));
        assert_eq!(transmit_state("FA00014060000;"), None);
    }
}
