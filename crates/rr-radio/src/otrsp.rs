//! OTRSP (Open Two Radio Switching Protocol, K1XM) decoder.
//!
//! Contest loggers (`N1MM+`, `Win-Test`, `DXLog`, `WriteLog`) send OTRSP commands to an
//! SO2R device over a serial port: ASCII words terminated by a carriage return,
//! 9600 baud 8N1. This module plays the role of that device for audio routing:
//!
//! | command          | meaning                                  |
//! |------------------|------------------------------------------|
//! | `TX1` / `TX2`    | transmit focus on radio 1 / 2            |
//! | `RX1` / `RX2`    | listen to radio 1 / 2                    |
//! | `RX1S` / `RX2S`  | listen to both (stereo), focus on 1 / 2  |
//! | `RX1R` / `RX2R`  | listen to both, reversed stereo          |
//! | `?NAME`          | device name query                        |
//!
//! Everything else (`AUX`, `?AUX`, radio-specific extensions) is ignored.

/// Which radio a command refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Radio {
    /// Radio 1 (device A).
    One,
    /// Radio 2 (device B).
    Two,
}

/// How the operator listens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listen {
    /// Only the given radio.
    Single(Radio),
    /// Both radios at once.
    Both,
}

/// A decoded OTRSP command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OtrspCommand {
    /// Transmit focus.
    Tx(Radio),
    /// Receive routing.
    Rx(Listen),
    /// The host asked for the device name; answer with [`DEVICE_NAME`].
    QueryName,
}

/// Name returned to `?NAME`.
pub const DEVICE_NAME: &str = "RemoteRadio Mumble";

/// Longest command we keep while waiting for a terminator.
const MAX_LINE: usize = 64;

/// Splits a byte stream into commands.
#[derive(Debug, Default, Clone)]
pub struct OtrspParser {
    line: Vec<u8>,
}

impl OtrspParser {
    /// Feed received bytes; returns the commands completed by them.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<OtrspCommand> {
        let mut out = Vec::new();
        for &b in bytes {
            if b == b'\r' || b == b'\n' {
                if let Some(cmd) = parse(&self.line) {
                    out.push(cmd);
                }
                self.line.clear();
            } else if self.line.len() < MAX_LINE {
                self.line.push(b);
            } else {
                // Runaway line without a terminator: drop it.
                self.line.clear();
            }
        }
        out
    }
}

/// Decode one command line (without its terminator).
#[must_use]
pub fn parse(line: &[u8]) -> Option<OtrspCommand> {
    let text = std::str::from_utf8(line).ok()?.trim().to_ascii_uppercase();
    let radio = |c: char| match c {
        '1' => Some(Radio::One),
        '2' => Some(Radio::Two),
        _ => None,
    };
    let mut chars = text.chars();
    match text.as_str() {
        "?NAME" => return Some(OtrspCommand::QueryName),
        t if t.starts_with("TX") && t.len() == 3 => {
            let _ = chars.nth(1);
            return chars.next().and_then(radio).map(OtrspCommand::Tx);
        }
        _ => {}
    }
    let rest = text.strip_prefix("RX")?;
    let mut it = rest.chars();
    let which = it.next().and_then(radio)?;
    match it.as_str() {
        "" => Some(OtrspCommand::Rx(Listen::Single(which))),
        "S" | "R" => Some(OtrspCommand::Rx(Listen::Both)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_tx_and_rx() {
        assert_eq!(parse(b"TX1"), Some(OtrspCommand::Tx(Radio::One)));
        assert_eq!(parse(b"tx2"), Some(OtrspCommand::Tx(Radio::Two)));
        assert_eq!(parse(b"RX1"), Some(OtrspCommand::Rx(Listen::Single(Radio::One))));
        assert_eq!(parse(b"RX2"), Some(OtrspCommand::Rx(Listen::Single(Radio::Two))));
        assert_eq!(parse(b"RX1S"), Some(OtrspCommand::Rx(Listen::Both)));
        assert_eq!(parse(b"RX2R"), Some(OtrspCommand::Rx(Listen::Both)));
        assert_eq!(parse(b"?NAME"), Some(OtrspCommand::QueryName));
    }

    #[test]
    fn ignores_everything_else() {
        for junk in [&b"AUX101"[..], b"?AUX1", b"TX3", b"RX3", b"RX1X", b"", b"TX", b"\xff\xfe"] {
            assert_eq!(parse(junk), None, "{junk:?}");
        }
    }

    #[test]
    fn stream_split_across_reads() {
        let mut p = OtrspParser::default();
        assert!(p.feed(b"TX").is_empty());
        assert_eq!(p.feed(b"2\rRX1S\r"), vec![OtrspCommand::Tx(Radio::Two), OtrspCommand::Rx(Listen::Both)]);
        assert_eq!(p.feed(b"AUX101\r\nRX2\n"), vec![OtrspCommand::Rx(Listen::Single(Radio::Two))]);
    }

    #[test]
    fn runaway_line_is_dropped() {
        let mut p = OtrspParser::default();
        assert!(p.feed(&[b'X'; 200]).is_empty());
        assert_eq!(p.feed(b"\rTX1\r"), vec![OtrspCommand::Tx(Radio::One)]);
    }
}
