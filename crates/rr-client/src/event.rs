//! Events and snapshots published by the client.

use rr_audio::bandwidth::BandwidthPlan;

/// Something that happened. Query the snapshot accessors for details.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A connection attempt started.
    Connecting,
    /// Logged in and ready. Carries our session id.
    Connected {
        /// Our Mumble session id.
        session: u32,
    },
    /// The connection ended.
    Disconnected {
        /// Human readable reason.
        reason: String,
    },
    /// The server refused the login.
    Rejected {
        /// The server's explanation.
        reason: String,
    },
    /// Users or channels changed; re-read the roster.
    Roster,
    /// The configured scope channel does not exist on this server.
    ScopeNotFound(String),
    /// A remote operator pressed or released PTT.
    RemotePtt {
        /// Sender's session id.
        session: u32,
        /// True while held.
        down: bool,
    },
    /// Another position announced its two-radio (`SO2R`) state, normally because
    /// its logger drives OTRSP.
    So2r {
        /// Sender's session id.
        session: u32,
        /// The announced state.
        state: rr_radio::So2rState,
    },
    /// The encoder settings were (re)planned against the server's bandwidth limit.
    Bandwidth(BandwidthPlan),
    /// The audio path switched between UDP and the TCP tunnel.
    Transport {
        /// True when UDP is active.
        udp: bool,
    },
    /// The server denied an action.
    Denied(String),
}

/// A user as seen by the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    /// Session id.
    pub session: u32,
    /// Display name.
    pub name: String,
    /// Channel the user is in.
    pub channel_id: u32,
    /// Self-muted.
    pub self_mute: bool,
    /// Self-deafened.
    pub self_deaf: bool,
}

/// A channel as seen by the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelInfo {
    /// Channel id.
    pub id: u32,
    /// Parent channel, `None` for the top of the visible tree.
    pub parent_id: Option<u32>,
    /// Display name.
    pub name: String,
    /// Sort position among siblings.
    pub position: i32,
    /// Server-reported permission bits, once known.
    pub permissions: Option<u32>,
}
