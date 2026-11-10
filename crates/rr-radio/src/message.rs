//! The remote PTT wire message and its receive-side filter.

use serde::{Deserialize, Serialize};

/// Mumble plugin-message `plugin_name` used by `RemoteRadio`.
pub const PLUGIN_NAME: &str = "remote-radio";
/// `payloadType` of a [`PttMessage`].
pub const PAYLOAD_TYPE_PTT: &str = "ptt";
/// How often a held PTT is re-sent so a lost release cannot leave a rig keyed.
pub const REFRESH_MS: u64 = 500;

/// A PTT request: `down` is the wanted state, `seq` a per-sender counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PttMessage {
    /// True to key, false to release.
    pub down: bool,
    /// Monotonic sequence number; higher wins.
    pub seq: u64,
}

impl PttMessage {
    /// Encode as the JSON bytes carried in a plugin message.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Decode from plugin-message bytes.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }
}

/// Drops duplicated and reordered messages from one sender.
#[derive(Debug, Default, Clone)]
pub struct SeqFilter {
    last: Option<u64>,
}

impl SeqFilter {
    /// A sender that restarts its counter is accepted once it falls this far
    /// behind.
    const RESTART_GAP: u64 = 1_000;

    /// Returns the wanted key state if this message is new.
    pub fn accept(&mut self, msg: PttMessage) -> Option<bool> {
        match self.last {
            Some(last) if msg.seq <= last && last - msg.seq < Self::RESTART_GAP => {
                // A repeat of the latest message is a refresh, not stale.
                if msg.seq == last {
                    return Some(msg.down);
                }
                None
            }
            _ => {
                self.last = Some(msg.seq);
                Some(msg.down)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let m = PttMessage { down: true, seq: 7 };
        assert_eq!(PttMessage::decode(&m.encode()), Some(m));
        assert_eq!(PttMessage::decode(b"junk"), None);
    }

    #[test]
    fn stale_messages_are_dropped_but_refreshes_pass() {
        let mut f = SeqFilter::default();
        assert_eq!(f.accept(PttMessage { down: true, seq: 5 }), Some(true));
        assert_eq!(f.accept(PttMessage { down: true, seq: 5 }), Some(true));
        assert_eq!(f.accept(PttMessage { down: false, seq: 4 }), None);
        assert_eq!(f.accept(PttMessage { down: false, seq: 6 }), Some(false));
    }

    #[test]
    fn restarted_sender_is_accepted() {
        let mut f = SeqFilter::default();
        assert_eq!(f.accept(PttMessage { down: true, seq: 50_000 }), Some(true));
        assert_eq!(f.accept(PttMessage { down: false, seq: 1 }), Some(false));
    }
}
