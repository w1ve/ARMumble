//! Remote.Radio's hidden control messages, carried over Mumble `PluginMessage`
//! envelopes (`plugin_name = "remote-radio"`), not the parked `rr-control`
//! WebRTC stack and not the forbidden `PluginDataTransmission` path.
//!
//! Two kinds travel here, each a `payloadType` discriminator with a small JSON
//! body:
//!
//! - [`PAYLOAD_TYPE_PTT`] — an operator keys/unkeys a Station's rig. The direct
//!   plugin-message path can lead the Mumble-relayed voice (CONTEXT §14d), and
//!   `PluginMessage` is a distinct message type from `TextMessage` so these
//!   never appear in chat.
//! - [`PAYLOAD_TYPE_ROLE`] — a client announces whether it is an Operator or a
//!   Station; the authoritative replacement for the saved-server name-match
//!   placeholder in the UI (CONTEXT §18g step 8).
//!
//! These constants and shapes mirror
//! `crates/mumble-tauri/ui/src/constants/remoteRadioPlugin.ts`; keep them in
//! lock-step.

use serde::{Deserialize, Serialize};

/// `plugin_name` for every Remote.Radio control message.
pub const PLUGIN_NAME: &str = "remote-radio";

/// `payloadType` for a [`PttPayload`].
pub const PAYLOAD_TYPE_PTT: &str = "ptt";

/// The role a Remote.Radio client plays. Mirrors the TS `ClientRole`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// An operator (sends mic audio, keys a remote station).
    Operator,
    /// A station (relays receiver audio, keys its own rig).
    Station,
}

/// PTT key/unkey intent from an operator to a station.
///
/// `seq` is a monotonic per-sender counter so a reordered or duplicated frame
/// is ignored (higher wins); `down` is the desired key state. The Station's
/// safety watchdog is the real guard — this is only the intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PttPayload {
    /// `true` = key (transmit), `false` = unkey.
    pub down: bool,
    /// Monotonic per-sender sequence number; higher wins.
    pub seq: u64,
}

// NOTE: the `role` broadcast is transported entirely on the frontend (it knows
// its own `SavedServer.role` and the channel), so the backend needs no
// `RolePayload`/`decode` for it. The authoritative session->role map lives in
// the TS store, fed by inbound `remote-radio/role` plugin messages. The TS
// mirror in `constants/remoteRadioPlugin.ts` carries the role shape. `Role`
// here is only the direction tag the PTT controller switches on.

/// Encode a payload to the JSON bytes carried in a `PluginMessage`.
///
/// # Errors
/// Returns the `serde_json` error on failure (practically never for these
/// owned types).
pub fn encode<T: Serialize>(payload: &T) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(payload)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "unwrap is acceptable in test code")]
mod tests {
    use super::*;

    #[test]
    fn ptt_payload_round_trips() {
        let p = PttPayload { down: true, seq: 42 };
        let bytes = encode(&p).unwrap();
        let back: PttPayload = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn ptt_field_names_match_ts() {
        let s = String::from_utf8(encode(&PttPayload { down: false, seq: 7 }).unwrap()).unwrap();
        assert!(s.contains("\"down\""), "got {s}");
        assert!(s.contains("\"seq\""), "got {s}");
    }

    #[test]
    fn ptt_decode_rejects_garbage() {
        assert!(serde_json::from_slice::<PttPayload>(b"not json").is_err());
    }

    #[test]
    fn role_serialises_lowercase_matching_ts() {
        // The direction tag the PTT controller switches on must match the TS
        // ClientRole string union "operator" | "station".
        assert_eq!(serde_json::to_string(&Role::Station).unwrap(), "\"station\"");
        assert_eq!(serde_json::to_string(&Role::Operator).unwrap(), "\"operator\"");
    }
}
