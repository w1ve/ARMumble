//! Relay the transmitting radio to a CW keyer on this computer as OTRSP text
//! over UDP (`127.0.0.1`).
//!
//! Only a two-radio **station** sends: that is where the keyer and the
//! transmitters are. The state comes from wherever the logger is (a local
//! OTRSP port, the other position over the server, or the buttons), so the
//! keyer follows in every case. Each datagram carries the full state, for
//! example `TX2\rRX2S\r`; it is sent on every change and repeated every
//! [`REPEAT`] so a restarted keyer catches up.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rr_radio::{Listen, Radio, So2rState};

/// How often the state is repeated while unchanged.
pub const REPEAT: Duration = Duration::from_millis(rr_radio::message::SO2R_REPEAT_MS);
/// Default keyer port (avoids N1MM's own UDP port, 12060).
pub const DEFAULT_PORT: u16 = 52110;

/// The OTRSP text for a state: the TX command, then the RX command.
#[must_use]
pub fn otrsp_text(s: So2rState) -> String {
    let n = |r: Radio| if r == Radio::One { 1 } else { 2 };
    let rx = match s.rx {
        Listen::Single(r) => format!("RX{}", n(r)),
        // Stereo receive, focus on the transmitting radio.
        Listen::Both => format!("RX{}S", n(s.tx)),
    };
    format!("TX{}\r{rx}\r", n(s.tx))
}

#[derive(Debug, Default)]
struct Last {
    text: String,
    port: u16,
    at: Option<Instant>,
}

/// Sends the state to the keyer; cheap to call often.
#[derive(Debug, Default)]
pub struct KeyerUdp {
    socket: Mutex<Option<UdpSocket>>,
    last: Mutex<Last>,
}

impl KeyerUdp {
    /// Send `state` to `127.0.0.1:port` if it changed, the port changed, it
    /// was never sent, or [`REPEAT`] has passed. `None` stops sending.
    pub fn update(&self, state: Option<So2rState>, port: u16) {
        let mut last = self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = state else {
            *last = Last::default();
            return;
        };
        let text = otrsp_text(state);
        let due = last.at.is_none_or(|t| t.elapsed() >= REPEAT);
        if text == last.text && port == last.port && !due {
            return;
        }
        self.send(&text, port);
        *last = Last { text, port, at: Some(Instant::now()) };
    }

    fn send(&self, text: &str, port: u16) {
        let mut slot = self.socket.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_none() {
            match UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)) {
                Ok(s) => *slot = Some(s),
                Err(e) => {
                    tracing::warn!("keyer UDP socket: {e}");
                    return;
                }
            }
        }
        let to = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        if let Some(s) = slot.as_ref() {
            // Nothing listening is normal (keyer not running): ignore errors.
            if let Err(e) = s.send_to(text.as_bytes(), to) {
                tracing::debug!("keyer UDP send to {to}: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_carries_tx_then_rx() {
        let s = |tx, rx| So2rState { tx, rx };
        assert_eq!(otrsp_text(s(Radio::One, Listen::Single(Radio::One))), "TX1\rRX1\r");
        assert_eq!(otrsp_text(s(Radio::Two, Listen::Single(Radio::One))), "TX2\rRX1\r");
        assert_eq!(otrsp_text(s(Radio::Two, Listen::Both)), "TX2\rRX2S\r");
    }

    #[test]
    fn sends_changes_and_repeats_to_a_listener() {
        let rx = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        rx.set_read_timeout(Some(Duration::from_secs(2))).expect("timeout");
        let port = rx.local_addr().expect("addr").port();
        let k = KeyerUdp::default();
        let mut buf = [0_u8; 64];

        k.update(Some(So2rState { tx: Radio::Two, rx: Listen::Both }), port);
        let n = rx.recv(&mut buf).expect("first datagram");
        assert_eq!(&buf[..n], b"TX2\rRX2S\r");

        // Same state again right away: not re-sent.
        k.update(Some(So2rState { tx: Radio::Two, rx: Listen::Both }), port);
        rx.set_read_timeout(Some(Duration::from_millis(150))).expect("timeout");
        assert!(rx.recv(&mut buf).is_err(), "unchanged state must not be re-sent early");

        // A change goes out immediately.
        k.update(Some(So2rState { tx: Radio::One, rx: Listen::Both }), port);
        let n = rx.recv(&mut buf).expect("changed datagram");
        assert_eq!(&buf[..n], b"TX1\rRX1S\r");
    }
}
