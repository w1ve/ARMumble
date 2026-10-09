//! UI value types, event payloads, and configuration structs serialised
//! to the React frontend.

use std::collections::{BTreeMap, HashMap};

use serde::{Serialize, Serializer};

use mumble_protocol::audio::filter::denoiser::NoiseSuppressionAlgorithm;
use mumble_protocol::state::PchatProtocol;

// --- Serialization helpers ----------------------------------------

fn serialize_pchat_protocol<S: Serializer>(protocol: &Option<PchatProtocol>, s: S) -> Result<S::Ok, S::Error> {
    match protocol {
        Some(p) => s.serialize_str(match p {
            PchatProtocol::None => "none",
            PchatProtocol::FancyV1FullArchive => "fancy_v1_full_archive",
            PchatProtocol::SignalV1 => "signal_v1",
        }),
        _ => s.serialize_none(),
    }
}

/// Derive a stable, non-zero `u32` "marker" from a blob hash (or the blob
/// itself).  Used as the serialised `texture_size`: it is non-zero whenever an
/// avatar exists (so the frontend knows to fetch it) and changes when the
/// avatar changes (so caches invalidate), without ever shipping the bytes.
pub(super) fn blob_marker(bytes: &[u8]) -> u32 {
    let mut buf = [0u8; 4];
    for (i, b) in bytes.iter().take(4).enumerate() {
        buf[i] = *b;
    }
    u32::from_le_bytes(buf) | 1
}

/// Emit only the byte length of a `String` (used for channel
/// `description`).  The frontend fetches the actual text on demand via
/// `get_channel_description`.
fn serialize_string_len_owned<S: Serializer>(text: &str, s: S) -> Result<S::Ok, S::Error> {
    if text.is_empty() {
        s.serialize_none()
    } else {
        s.serialize_some(&(text.len() as u32))
    }
}

// --- UI value types (serializable to the frontend) ----------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChannelEntry {
    pub id: u32,
    pub parent_id: Option<u32>,
    pub name: String,
    /// Channel description blob.  Serialised to the frontend as
    /// `description_size: u32 | null` (byte length only) to keep
    /// `get_channels` payloads small; fetched lazily via
    /// `get_channel_description`.
    #[serde(rename = "description_size", serialize_with = "serialize_string_len_owned")]
    pub description: String,
    /// SHA-256 hash of the description blob.  Internal tracking only;
    /// not serialised to the frontend.
    #[serde(skip)]
    pub description_hash: Option<Vec<u8>>,
    pub user_count: u32,
    /// Server-reported permission bitmask for this channel.
    /// `None` until a `PermissionQuery` response is received.
    pub permissions: Option<u32>,
    /// Whether the channel is temporary.
    pub temporary: bool,
    /// Channel sort position.
    pub position: i32,
    /// Maximum users allowed (0 = unlimited).
    pub max_users: u32,
    /// Persistent-chat protocol.  `None` if not announced by the server.
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serialize_pchat_protocol")]
    pub pchat_protocol: Option<PchatProtocol>,
    /// Maximum stored messages (0 = unlimited).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pchat_max_history: Option<u32>,
    /// Auto-delete after N days (0 = forever).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pchat_retention_days: Option<u32>,
    /// Key custodian cert hashes (Section 5.7).
    #[serde(skip)]
    pub pchat_key_custodians: Vec<String>,
    /// Whether the channel requires a password (token) to enter.
    #[serde(default)]
    pub is_enter_restricted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UserEntry {
    pub session: u32,
    pub name: String,
    pub channel_id: u32,
    /// Registered user ID. `None` means the user is not registered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u32>,
    /// Loaded avatar bytes.  Internal only: never serialised (the frontend
    /// fetches them on demand via `get_user_texture`).  `None` until the blob
    /// has actually been requested + received, so the backend does NOT hold an
    /// avatar for every connected user - only for those a client has viewed.
    #[serde(skip)]
    pub texture: Option<Vec<u8>>,
    /// Avatar existence/version marker, serialised to the frontend as
    /// `texture_size: u32 | null`.  Non-zero whenever the user HAS an avatar -
    /// even before its bytes are loaded - so the UI knows to fetch it; its
    /// value changes when the avatar changes so caches invalidate.  Derived
    /// from the server's `texture_hash` (or the inline blob length).
    #[serde(rename = "texture_size")]
    pub texture_marker: Option<u32>,
    /// Loaded comment/bio text.  Internal only: never serialised (the frontend
    /// fetches it on demand via `get_user_comment`).  `None` until requested,
    /// so the backend does not hold every user's (potentially banner-laden) bio
    /// - only those a client has viewed.
    #[serde(skip)]
    pub comment: Option<String>,
    /// Comment existence/version marker, serialised as `comment_size: u32 | null`
    /// (mirrors `texture_size`).  Non-zero whenever the user HAS a comment - even
    /// before its text is loaded - so the UI knows to fetch it; changes when the
    /// comment changes so caches invalidate.
    #[serde(rename = "comment_size")]
    pub comment_marker: Option<u32>,
    /// Server-side admin mute.
    pub mute: bool,
    /// Server-side admin deafen.
    pub deaf: bool,
    /// Suppressed by the server (e.g. moved to AFK channel).
    pub suppress: bool,
    /// User has self-muted.
    pub self_mute: bool,
    /// User has self-deafened.
    pub self_deaf: bool,
    /// Priority speaker status.
    pub priority_speaker: bool,
    /// TLS certificate hash (hex-encoded SHA-1). Used as stable identity
    /// for persistent chat key management.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// Server-advertised client capabilities (see `UserState.ClientFeature`).
    #[serde(skip)]
    pub client_features: Vec<i32>,
}

impl UserEntry {
    pub fn new(session: u32) -> Self {
        Self {
            session,
            name: String::new(),
            channel_id: 0,
            user_id: None,
            texture: None,
            texture_marker: None,
            comment_marker: None,
            comment: None,
            mute: false,
            deaf: false,
            suppress: false,
            self_mute: false,
            self_deaf: false,
            priority_speaker: false,
            hash: None,
            client_features: Vec::new(),
        }
    }

    /// Returns `true` if this user advertises E2EE persistent chat support.
    pub fn has_pchat_e2ee(&self) -> bool {
        use mumble_protocol::proto::mumble_tcp::user_state::ClientFeature;
        self.client_features
            .contains(&(ClientFeature::FeaturePchatE2ee as i32))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatMessage {
    pub sender_session: Option<u32>,
    pub sender_name: String,
    /// TLS certificate hash of the sender.  Stable across reconnects,
    /// allowing the frontend to resolve the sender's profile even when
    /// `sender_session` is stale or `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_hash: Option<String>,
    pub body: String,
    pub channel_id: u32,
    pub is_own: bool,
    /// When set, this message is a direct message (DM) to/from a specific user.
    /// The value is the *other* user's session ID (the conversation partner).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dm_session: Option<u32>,
    /// Unique message identifier (Fancy Mumble extension).
    /// `None` when the server/sender does not support extensions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// Unix epoch milliseconds (Fancy Mumble extension).
    /// `None` when the server/sender does not support extensions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
    /// `true` when the message came from a legacy (non-E2EE) client on a
    /// pchat-enabled channel and was therefore sent in plaintext.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_legacy: bool,
    /// When set, the message was edited at this Unix-epoch-millisecond timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<u64>,
    /// Whether this message is pinned.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// Certificate hash of the user who pinned this message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned_by: Option<String>,
    /// Unix epoch milliseconds when the message was pinned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned_at: Option<u64>,
    /// Plugin-authored origin: the `plugin_name` of the plugin that
    /// injected this message via a `chat-message` interaction
    /// response.  `None` for ordinary user/server messages.
    ///
    /// Component interactions on this message route back to this
    /// plugin so the originating handler can react.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
    /// Plugin-authored UI components attached to this message,
    /// rendered inline in the chat bubble below the body.  Stored
    /// as opaque JSON (mirroring the
    /// [`mumble_plugin_api::ActionRow`] wire format) so this crate
    /// does not have to depend on the plugin API.  `None` (or an
    /// empty array) means no interactive components.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_components: Option<serde_json::Value>,
}

impl ChatMessage {
    /// Ensure the message has a `message_id`, generating a UUID if absent.
    ///
    /// A stable ID is required so the offloading system can refer to the
    /// message across encrypt/store/restore cycles.
    pub fn ensure_id(&mut self) {
        if self.message_id.is_none() {
            self.message_id = Some(uuid::Uuid::new_v4().to_string());
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
}

// --- Server activity log ------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ServerLogEntry {
    pub timestamp_ms: u64,
    pub message: String,
}

impl ServerLogEntry {
    pub fn now(message: String) -> Self {
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            timestamp_ms,
            message,
        }
    }
}

// --- Server config ------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ServerConfig {
    pub max_message_length: u32,
    pub max_image_message_length: u32,
    pub allow_html: bool,
    pub webrtc_sfu_available: bool,
    /// Optional override for the Fancy Mumble REST API base URL,
    /// advertised by the server in `ServerConfig::fancy_rest_api_url`.
    /// `None` (or empty) means clients should fall back to whatever the
    /// individual plugin (e.g. file-server) reports in its plugin-data
    /// config. Useful when the HTTP interface is behind a reverse proxy.
    pub fancy_rest_api_url: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            // Mumble defaults per the protocol spec.
            max_message_length: 5000,
            max_image_message_length: 131072,
            allow_html: true,
            webrtc_sfu_available: false,
            fancy_rest_api_url: None,
        }
    }
}

/// Version and configuration metadata announced by the server during handshake.
/// Assembled from `Version`, `ServerSync`, and `ServerConfig` messages.
#[derive(Debug, Default, Clone, Serialize)]
pub struct ServerVersionInfo {
    /// Server release string (e.g. "Mumble 1.5.517").
    pub release: Option<String>,
    /// Server operating system (e.g. "Linux", "Windows").
    pub os: Option<String>,
    /// Server OS version string.
    pub os_version: Option<String>,
    /// Legacy protocol version v1 encoding: (major << 16) | (minor << 8) | patch.
    pub version_v1: Option<u32>,
    /// Protocol version v2 encoding.
    pub version_v2: Option<u64>,
    /// Fancy Mumble extension version (None = standard server).
    pub fancy_version: Option<u64>,
}

/// Full server info payload sent to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
    /// Host the client connected to.
    pub host: String,
    /// Port the client connected to.
    pub port: u16,
    /// Number of users currently on the server.
    pub user_count: u32,
    /// Maximum users allowed by the server (from `ServerConfig`).
    pub max_users: Option<u32>,
    /// Human-readable protocol version string.
    pub protocol_version: Option<String>,
    /// Fancy Mumble extension version.
    pub fancy_version: Option<u64>,
    /// Server release string.
    pub release: Option<String>,
    /// Server operating system.
    pub os: Option<String>,
    /// Maximum bandwidth allowed by the server (bits/s).
    pub max_bandwidth: Option<u32>,
    /// Whether Opus codec is supported.
    pub opus: bool,
}

// --- Debug stats ---------------------------------------------------

/// Debug statistics for the developer info panel.
#[derive(Debug, Clone, Serialize)]
pub struct DebugStats {
    /// Number of channel messages in memory.
    pub channel_message_count: usize,
    /// Number of DM messages in memory.
    pub dm_message_count: usize,
    /// Total messages (channel + DM).
    pub total_message_count: usize,
    /// Number of messages currently offloaded to disk.
    pub offloaded_count: usize,
    /// Number of channels known to the client.
    pub channel_count: usize,
    /// Number of users connected to the server.
    pub user_count: usize,
    /// Internal connection epoch counter.
    pub connection_epoch: u64,
    /// Current voice state as a string.
    pub voice_state: String,
    /// Seconds since the app was started.
    pub uptime_seconds: u64,
}

// --- Event payloads emitted to the frontend -----------------------

#[derive(Clone, Serialize)]
pub(crate) struct NewMessagePayload {
    pub channel_id: u32,
    pub sender_session: Option<u32>,
}

/// Emitted when a new direct message arrives.
#[derive(Clone, Serialize)]
pub(crate) struct NewDmPayload {
    /// Session ID of the conversation partner (the sender for incoming DMs).
    pub session: u32,
}

#[derive(Clone, Serialize)]
pub(crate) struct RejectedPayload {
    /// Id of the session that was rejected.  Allows the frontend to
    /// route the rejection to the correct tab and avoid clobbering
    /// other sessions' state.  May be `None` for early connect-time
    /// failures before a session was registered.
    #[serde(rename = "serverId")]
    pub server_id: Option<String>,
    pub reason: String,
    /// Protobuf `Reject.RejectType` value, if available.
    /// `3` = `WrongUserPW`, `4` = `WrongServerPW`.
    pub reject_type: Option<i32>,
}

/// Payload for the `server-disconnected` event.  Carries the id of
/// the session that was disconnected so the frontend can route the
/// event to the correct tab and avoid clobbering other sessions'
/// state.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DisconnectedPayload {
    pub server_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct UnreadPayload {
    /// `channel_id` -> unread count
    pub unreads: HashMap<u32, u32>,
}

#[derive(Clone, Serialize)]
pub(crate) struct DmUnreadPayload {
    /// `session_id` -> unread DM count
    pub unreads: HashMap<u32, u32>,
}

#[derive(Clone, Serialize)]
pub(crate) struct ListenDeniedPayload {
    pub channel_id: u32,
}

#[derive(Clone, Serialize)]
pub(crate) struct ChannelDeniedPayload {
    pub channel_id: u32,
}

#[derive(Clone, Serialize)]
pub(crate) struct PermissionDeniedPayload {
    pub deny_type: Option<i32>,
    pub reason: Option<String>,
}

/// Cached snapshot of the server's `PluginRegistry`.  Also forms the
/// `plugin-registry` Tauri event payload (frontend field names match
/// `PluginRegistryEntry` in `ui/src/store.ts`).  We cache it so the UI
/// can resync after an HMR reload, which loses the one-shot event.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PluginRegistryEntryPayload {
    pub plugin_name: String,
    pub version: String,
    pub plugin_slot: Option<u32>,
    pub info_json: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct PluginDataPayload {
    pub sender_session: Option<u32>,
    /// Raw payload bytes, serialized as a base64 string.  A plain
    /// `Vec<u8>` would serialize as a JSON array of numbers, which
    /// `serde_json` represents at ~32 heap bytes per payload byte - a
    /// 1.6 MB server-emotes broadcast measured 51 MB as a `Value` plus
    /// ~19 MB more in the Tauri event script.  Base64 keeps it at
    /// ~1.3x the byte size end to end.
    #[serde(serialize_with = "serialize_bytes_base64")]
    pub data: Vec<u8>,
    pub data_id: String,
}

/// Serialize a byte slice as a base64 string (see [`PluginDataPayload::data`]).
pub(crate) fn serialize_bytes_base64<S: Serializer>(
    bytes: &[u8],
    ser: S,
) -> Result<S::Ok, S::Error> {
    use base64::Engine as _;
    ser.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
}

#[derive(Clone, Serialize)]
pub(crate) struct WebRtcSignalPayload {
    pub sender_session: Option<u32>,
    pub target_session: Option<u32>,
    pub signal_type: i32,
    pub payload: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct CurrentChannelPayload {
    pub channel_id: u32,
}

/// Payload emitted when pchat history loading starts or finishes for a channel.
#[derive(Clone, Serialize)]
pub(crate) struct PchatHistoryLoadingPayload {
    pub channel_id: u32,
    pub loading: bool,
}

/// Payload emitted when a `PchatFetchResponse` has been fully processed.
#[derive(Clone, Serialize)]
pub(crate) struct PchatFetchCompletePayload {
    pub channel_id: u32,
    pub has_more: bool,
    pub total_stored: u32,
}

/// Payload emitted when a `PchatReactionDeliver` is received (single reaction event).
#[derive(Clone, Serialize)]
pub(crate) struct ReactionDeliverPayload {
    pub channel_id: u32,
    pub message_id: String,
    pub emoji: String,
    pub action: String,
    pub sender_hash: String,
    pub sender_name: String,
    pub timestamp: u64,
}

/// A single stored reaction within a `PchatReactionFetchResponse`.
#[derive(Clone, Serialize)]
pub(crate) struct StoredReactionPayload {
    pub message_id: String,
    pub emoji: String,
    pub sender_hash: String,
    pub sender_name: String,
    pub timestamp: u64,
}

/// Payload emitted when a `PchatReactionFetchResponse` is received (batch of reactions).
#[derive(Clone, Serialize)]
pub(crate) struct ReactionFetchResponsePayload {
    pub channel_id: u32,
    pub reactions: Vec<StoredReactionPayload>,
}

/// Payload emitted when a `PchatPinDeliver` is received (pin state change).
#[derive(Clone, Serialize)]
pub(crate) struct PinDeliverPayload {
    pub channel_id: u32,
    pub message_id: String,
    pub pinned: bool,
    pub pinner_hash: String,
    pub pinner_name: String,
    pub timestamp: u64,
}

/// Payload emitted when a `PchatPinFetchResponse` is received (batch of pins).
#[derive(Clone, Serialize)]
pub(crate) struct StoredPinPayload {
    pub message_id: String,
    pub pinner_hash: String,
    pub pinner_name: String,
    pub timestamp: u64,
}

/// Payload emitted when a `PchatPinFetchResponse` is received.
#[derive(Clone, Serialize)]
pub(crate) struct PinFetchResponsePayload {
    pub channel_id: u32,
    pub pins: Vec<StoredPinPayload>,
}

/// A pending key-share request waiting for user approval.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct PendingKeyShare {
    /// Channel that the key would be shared for.
    pub channel_id: u32,
    /// Certificate hash of the peer requesting the key.
    pub peer_cert_hash: String,
    /// Display name of the peer (resolved from current users).
    pub peer_name: String,
    /// Server-assigned request ID (present for consensus key-request path,
    /// `None` for proactive key-announce path).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Payload for the "pchat-key-share-request" frontend event.
#[derive(Clone, Serialize)]
pub(crate) struct KeyShareRequestPayload {
    pub channel_id: u32,
    pub peer_name: String,
    pub peer_cert_hash: String,
}
/// Payload for the \"pchat-key-share-requests-changed\" event (after approve/dismiss).
#[derive(Clone, Serialize)]
pub(crate) struct KeyShareRequestsChangedPayload {
    pub channel_id: u32,
    pub pending: Vec<PendingKeyShare>,
}
/// A user known to hold the encryption key for a channel.
#[derive(Clone, Debug, Serialize)]
pub struct KeyHolderEntry {
    /// TLS certificate hash (stable identity).
    pub cert_hash: String,
    /// Display name (resolved from online users or last known).
    pub name: String,
    /// Whether the user is currently online.
    pub is_online: bool,
}

/// Payload for the "pchat-key-holders-changed" event.
#[derive(Clone, Serialize)]
pub(crate) struct KeyHoldersChangedPayload {
    pub channel_id: u32,
    pub holders: Vec<KeyHolderEntry>,
}

/// Payload for the "pchat-key-revoked" event.
#[derive(Clone, Serialize)]
pub(crate) struct PchatKeyRevokedPayload {
    pub channel_id: u32,
}

/// Payload for the "pchat-signal-bridge-error" event.
/// Sent when the signal bridge library fails to load, making `SignalV1`
/// encryption unavailable.
#[derive(Clone, Serialize)]
pub(crate) struct SignalBridgeErrorPayload {
    pub message: String,
}

// --- Audio types --------------------------------------------------

/// Microphone amplitude payload emitted during mic test.
#[derive(Clone, Serialize)]
pub(crate) struct MicAmplitudePayload {
    /// RMS amplitude (0.0 - 1.0).
    pub rms: f32,
    /// Peak amplitude (0.0 - 1.0).
    pub peak: f32,
}

/// Auto-calibration result emitted when voice-activation auto-tunes
/// the noise-gate parameters.  Carries all four calibration knobs so
/// the frontend can refresh its UI atomically.
#[derive(Clone, Serialize)]
pub(crate) struct VoiceActivationCalibrationPayload {
    /// Auto-tuned open threshold (post-AGC RMS, 0.0 - 1.0).
    pub vad_threshold: f32,
    /// Close-threshold ratio relative to `vad_threshold`.
    pub noise_gate_close_ratio: f32,
    /// Frames to keep the gate open after audio drops below the close threshold.
    pub hold_frames: u32,
    /// Auto-tuned AGC max gain in dB.
    pub max_gain_db: f32,
}

/// Voice replay lifecycle, emitted on `voice-replay-state` so the
/// frontend can label its single Record / Stop / Playing button
/// without polling.
#[derive(Clone, Copy, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub(crate) enum VoiceReplayState {
    /// Capturing through the same filter chain the live voice pipeline
    /// uses (AGC + denoiser + noise gate).
    Recording { elapsed_ms: u32, capacity_ms: u32 },
    /// Replaying the captured buffer through the output device.
    Playing { elapsed_ms: u32, total_ms: u32 },
    /// Replay finished or was cancelled.
    Idle,
}

/// Latency measurement payload emitted during latency test.
#[derive(Clone, Serialize)]
pub(crate) struct LatencyPayload {
    /// Round-trip time in milliseconds.
    pub rtt_ms: f64,
}

/// UDP crypto packet counters (good / late / lost / resync).
#[derive(Clone, Default, Serialize)]
pub(crate) struct PacketStats {
    pub good: u32,
    pub late: u32,
    pub lost: u32,
    pub resync: u32,
}

/// Payload emitted via the `crypto-stats` event on each Ping exchange.
#[derive(Clone, Serialize)]
pub(crate) struct CryptoStatsPayload {
    /// Our local decrypt stats (packets we successfully received/decoded).
    pub from_client: PacketStats,
    /// Server-reported stats for packets it sent to us.
    pub to_client: PacketStats,
}

/// Rolling-window packet statistics.
#[derive(Clone, Serialize)]
pub(crate) struct RollingStatsPayload {
    /// Rolling window duration in seconds.
    pub time_window: u32,
    pub from_client: PacketStats,
    pub from_server: PacketStats,
}

/// Payload emitted when a `UserStats` response arrives from the server.
#[derive(Clone, Serialize)]
pub(crate) struct UserStatsPayload {
    pub session: u32,
    pub tcp_packets: u32,
    pub udp_packets: u32,
    pub tcp_ping_avg: f32,
    pub tcp_ping_var: f32,
    pub udp_ping_avg: f32,
    pub udp_ping_var: f32,
    pub bandwidth: Option<u32>,
    pub onlinesecs: Option<u32>,
    pub idlesecs: Option<u32>,
    pub strong_certificate: bool,
    pub opus: bool,
    /// Client version string (e.g. "1.5.517").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Operating system name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    /// Operating system version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    /// Client IP address (formatted string).  Only present for admins.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Total UDP crypto stats: packets received from the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_client: Option<PacketStats>,
    /// Total UDP crypto stats: packets sent to the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_server: Option<PacketStats>,
    /// Rolling-window packet statistics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rolling_stats: Option<RollingStatsPayload>,
}

/// Result sent through the oneshot channel when a `PchatAck` for a deletion
/// request is received from the server.
pub(crate) struct DeleteAckResult {
    pub success: bool,
    pub reason: Option<String>,
}

// --- Admin panel payload types ------------------------------------

/// A registered user entry returned by the server's `UserList` message.
#[derive(Debug, Clone, Serialize)]
pub struct RegisteredUserPayload {
    pub user_id: u32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_channel: Option<u32>,
    /// Avatar byte length, so the frontend knows an avatar exists without
    /// shipping the bytes in the bulk list. The bytes are cached backend-side
    /// and fetched on demand via `get_registered_user_texture` (mirrors how
    /// online users use `UserEntry::texture_size`). Shipping the bytes inline
    /// previously spiked the heap to >1 GB while emitting the `user-list` event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub texture_size: Option<u32>,
    /// Full comment when len < 128 (included inline by the server).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// SHA-1 hash of the comment when len >= 128. Presence means a comment
    /// exists but the full text must be requested via `request_user_comment`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment_hash: Option<Vec<u8>>,
}

/// A single registered-user comment delivered via `RequestBlob.user_id_comment`.
#[derive(Debug, Clone, Serialize)]
pub struct UserCommentPayload {
    pub user_id: u32,
    pub comment: String,
}

/// A registered user update sent from the frontend.
///
/// - `name: Some(new_name)` renames the user.
/// - `name: None` deletes (deregisters) the user.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RegisteredUserUpdate {
    pub user_id: u32,
    pub name: Option<String>,
}

/// A ban list entry sent to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct BanEntryPayload {
    pub address: String,
    pub mask: u32,
    pub name: String,
    pub hash: String,
    pub reason: String,
    pub start: String,
    pub duration: u32,
}

/// Full ACL data for a channel, emitted as event payload.
#[derive(Debug, Clone, Serialize)]
pub struct AclPayload {
    pub channel_id: u32,
    pub inherit_acls: bool,
    pub groups: Vec<AclGroupPayload>,
    pub acls: Vec<AclEntryPayload>,
}

/// A channel group entry within an ACL.
#[derive(Debug, Clone, Serialize)]
pub struct AclGroupPayload {
    pub name: String,
    pub inherited: bool,
    pub inherit: bool,
    pub inheritable: bool,
    pub add: Vec<u32>,
    pub remove: Vec<u32>,
    pub inherited_members: Vec<u32>,
    /// `FancyMumble` role customization fields. Optional/default to keep older servers working.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style_preset: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

/// A single ACL rule within a channel's ACL list.
#[derive(Debug, Clone, Serialize)]
pub struct AclEntryPayload {
    pub apply_here: bool,
    pub apply_subs: bool,
    pub inherited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub grant: u32,
    pub deny: u32,
}

// --- Admin panel input types (deserialized from frontend) ---------

/// A ban entry received from the frontend for updating the ban list.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct BanEntryInput {
    pub address: String,
    pub mask: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub start: String,
    #[serde(default)]
    pub duration: u32,
}

/// ACL update payload received from the frontend.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AclInput {
    pub channel_id: u32,
    pub inherit_acls: bool,
    pub groups: Vec<AclGroupInput>,
    pub acls: Vec<AclEntryInput>,
}

/// A group entry from the frontend for ACL updates.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AclGroupInput {
    pub name: String,
    #[serde(default = "default_true")]
    pub inherited: bool,
    #[serde(default = "default_true")]
    pub inherit: bool,
    #[serde(default = "default_true")]
    pub inheritable: bool,
    #[serde(default)]
    pub add: Vec<u32>,
    #[serde(default)]
    pub remove: Vec<u32>,
    #[serde(default)]
    pub inherited_members: Vec<u32>,
    /// `FancyMumble` role customization fields.
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub icon: Option<Vec<u8>>,
    #[serde(default)]
    pub style_preset: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// An ACL entry from the frontend for ACL updates.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AclEntryInput {
    #[serde(default = "default_true")]
    pub apply_here: bool,
    #[serde(default = "default_true")]
    pub apply_subs: bool,
    #[serde(default)]
    pub inherited: bool,
    pub user_id: Option<u32>,
    pub group: Option<String>,
    #[serde(default)]
    pub grant: u32,
    #[serde(default)]
    pub deny: u32,
}

const fn default_true() -> bool {
    true
}

// --- Search types -------------------------------------------------

/// Filter narrowing the search scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchFilter {
    All,
    Messages,
    Photos,
    Users,
    Links,
}

/// Category tag for a search result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchCategory {
    Channel,
    User,
    Message,
}

/// A single search result returned by the super-search command.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    /// What kind of item this is.
    pub category: SearchCategory,
    /// Fuzzy match score (lower = better match, 0 = exact).
    pub score: u32,
    /// Primary display text (channel name, username, or message snippet).
    pub title: String,
    /// Secondary context (e.g. channel name for a user, sender for a message).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// Numeric ID for channels (`channel_id`) or users (session).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
    /// Optional opaque string ID for results that are not addressed by `u32`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub string_id: Option<String>,
}

/// A single photo extracted from a chat message for the photo grid.
#[derive(Debug, Clone, Serialize)]
pub struct PhotoEntry {
    /// Image source (data-URL or remote URL).
    pub src: String,
    /// Who sent the message containing this image.
    pub sender_name: String,
    /// Channel ID when the photo is from a channel message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<u32>,
    /// DM session when the photo is from a direct message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dm_session: Option<u32>,
    /// Human-readable context (e.g. "in #General", "DM with Alice").
    pub context: String,
    /// Message timestamp (epoch ms), if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
}

// --- Audio device type --------------------------------------------

/// An available audio input device.
#[derive(Debug, Clone, Serialize)]
pub struct AudioDevice {
    pub name: String,
    pub is_default: bool,
}

/// How the outbound pipeline decides when to transmit.
///
/// This is deliberately separate from noise suppression. Suppression cleans up
/// the audio that *is* sent; the transmit mode decides *whether* a frame is
/// sent at all. Bundling the two — as the code used to, by hanging the voice
/// gate off the `noise_suppression` toggle — meant turning suppression off also
/// silently removed voice gating, which is wrong for an amateur-radio Station:
/// it wants the raw, unprocessed signal *and* a mic that is always live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TransmitMode {
    /// Always transmit. No voice gate, so the mic is permanently open. This is
    /// what a radio Station runs: the far end hears everything, continuously,
    /// exactly as a keyed-up transmitter would.
    Continuous,
    /// Transmit only while the input is above the voice-activation threshold.
    /// A noise gate drives this and is the historical default.
    #[default]
    VoiceActivity,
    /// Transmit only while the push-to-talk key is held. Gating is done by
    /// self-mute at the protocol level, not by a filter, so no voice gate is
    /// inserted in the chain.
    PushToTalk,
}

/// User-configurable audio settings.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct AudioSettings {
    /// Selected input device name (None = system default).
    pub selected_device: Option<String>,
    /// Whether auto-gain is enabled.
    pub auto_gain: bool,
    /// Voice activation threshold (0.0-1.0). Below this level -> silence.
    pub vad_threshold: f32,
    /// AGC maximum gain boost in dB (expert, default 15.0).
    #[serde(default = "AudioSettings::default_max_gain")]
    pub max_gain_db: f32,
    /// Close-threshold ratio relative to `vad_threshold` (expert, default 0.8).
    #[serde(default = "AudioSettings::default_close_ratio")]
    pub noise_gate_close_ratio: f32,
    /// Number of frames to hold the gate open after audio drops below threshold.
    #[serde(default = "AudioSettings::default_hold_frames")]
    pub hold_frames: u32,
    /// When the outbound pipeline transmits: always, on voice activity, or on
    /// push-to-talk. Absent in older saved configs, where the legacy
    /// `push_to_talk` boolean is the source of truth; [`AudioSettings::transmit_mode`]
    /// reconciles the two, so read transmit policy through that, not this field.
    #[serde(default)]
    pub transmit_mode: TransmitMode,
    /// Use push-to-talk instead of voice activation.
    ///
    /// Retained for backward compatibility with saved configs and the existing
    /// self-mute PTT path. New code should prefer [`AudioSettings::transmit_mode`],
    /// which supersedes this and adds the explicit `Continuous` mode a radio
    /// Station needs.
    #[serde(default)]
    pub push_to_talk: bool,
    /// Global shortcut string for PTT (e.g. "Alt+T").
    #[serde(default)]
    pub push_to_talk_key: Option<String>,
    /// Opus encoder bitrate in bits/s (e.g. 72000).
    #[serde(default = "AudioSettings::default_bitrate")]
    pub bitrate_bps: i32,
    /// Audio duration per Opus packet in milliseconds (10, 20, 40, or 60).
    #[serde(default = "AudioSettings::default_frame_size_ms")]
    pub frame_size_ms: u32,
    /// Whether the noise gate (noise suppression) is enabled.
    #[serde(default = "AudioSettings::default_noise_suppression")]
    pub noise_suppression: bool,
    /// Selected noise-suppression algorithm.  Only takes effect when
    /// `noise_suppression` is true.
    #[serde(default)]
    pub denoiser_algorithm: NoiseSuppressionAlgorithm,
    /// Per-algorithm tunable knobs (advanced/expert mode only).
    /// Keyed by `DenoiserParamSpec::id`; missing entries fall back to
    /// each spec's default.
    #[serde(default)]
    pub denoiser_params: BTreeMap<String, f32>,
    /// Selected output device name (None = system default).
    #[serde(default)]
    pub selected_output_device: Option<String>,
    /// Microphone volume multiplier (0.0-2.0, default 1.0).
    #[serde(default = "AudioSettings::default_volume")]
    pub input_volume: f32,
    /// Speaker volume multiplier (0.0-2.0, default 1.0).
    #[serde(default = "AudioSettings::default_volume")]
    pub output_volume: f32,    /// Automatically adjust input sensitivity based on ambient noise floor.
    #[serde(default)]
    pub auto_input_sensitivity: bool,
    /// Force audio to use TCP tunnel instead of UDP (e.g. behind strict NAT).
    #[serde(default)]
    pub force_tcp_audio: bool,
    /// How the two decoded channels map to the output: as-is stereo, summed
    /// to mono on both ears, or swapped. Lets an operator collapse a
    /// single-receiver stream or put the other receiver in the other ear.
    #[serde(default)]
    pub playback_stereo_mode: StereoMode,
    /// Left-channel playback gain multiplier (0.0-2.0, default 1.0). Lets an
    /// operator balance main vs sub receiver when they are split L/R.
    #[serde(default = "AudioSettings::default_volume")]
    pub left_volume: f32,
    /// Right-channel playback gain multiplier (0.0-2.0, default 1.0).
    #[serde(default = "AudioSettings::default_volume")]
    pub right_volume: f32,
}

/// PTT direction, fixed by this client's role on the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PttRole {
    /// Operator: sense a footswitch on an input line, send a key command.
    #[default]
    Operator,
    /// Station: drive an output line to key the local rig.
    Station,
}

/// Which RS-232 control line the PTT uses. Mirrors `rr_ptt::PttLine` and the
/// TS `PttLine`; kept as its own type so the settings layer does not depend on
/// the `rr-ptt` crate's feature-gated serial module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PttLine {
    /// Data Terminal Ready (writable output; the usual Station key line).
    #[default]
    Dtr,
    /// Request To Send (writable output).
    Rts,
    /// Clear To Send (readable input; a common footswitch return).
    Cts,
    /// Data Set Ready (readable input).
    Dsr,
    /// Carrier Detect (readable input).
    Cd,
    /// Ring Indicator (readable input).
    Ri,
}

/// Per-server rig-PTT configuration. Persisted alongside [`AudioSettings`].
///
/// The role is the server's role ([`crate::state::rr_plugin::Role`]): an
/// Operator senses a footswitch on an *input* line and sends a key command; a
/// Station drives an *output* line to key its rig, triggered by an operator
/// command and/or inbound-audio VOX. All timing fields feed the safety
/// watchdog and the lead-in/hang-time behaviour (CONTEXT §14d/§14e).
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct PttConfig {
    /// Whether serial PTT is active at all. Off by default - nothing touches a
    /// serial port, let alone a transmitter, until the user opts in.
    #[serde(default)]
    pub enabled: bool,
    /// This client's role on the server, which fixes the PTT direction: an
    /// Operator senses a footswitch input, a Station drives a key output.
    #[serde(default)]
    pub role: PttRole,
    /// Serial port name (`"COM5"`, `"/dev/ttyUSB0"`). Empty = unset.
    #[serde(default)]
    pub port: String,
    /// Control line to drive (Station) or sense (Operator).
    #[serde(default)]
    pub line: PttLine,
    /// Invert the active sense for odd interface wiring.
    #[serde(default)]
    pub active_low: bool,
    /// Station: also key on inbound-audio VOX, not only on an operator command.
    #[serde(default)]
    pub vox: bool,
    /// Station VOX hold (ms, 0-300): keep the key down this long after inbound
    /// audio stops, so inter-syllable gaps in the operator's speech do not drop
    /// PTT. Independent of `hang_ms` (which applies to the overall release).
    #[serde(default = "PttConfig::default_vox_hang_ms")]
    pub vox_hang_ms: u32,
    /// Milliseconds the key leads the audio (Station delays audio to the rig by
    /// this much so the transmitter is up before speech - CONTEXT §14d).
    #[serde(default = "PttConfig::default_lead_in_ms")]
    pub lead_in_ms: u32,
    /// Milliseconds to hold the key after release, so the tail is not chopped.
    #[serde(default = "PttConfig::default_hang_ms")]
    pub hang_ms: u32,
    /// Deadman timeout (ms): auto-release without a keep-alive.
    #[serde(default = "PttConfig::default_deadman_ms")]
    pub deadman_ms: u32,
    /// Maximum continuous transmit (seconds); hard ceiling.
    #[serde(default = "PttConfig::default_max_tx_secs")]
    pub max_tx_secs: u32,
    /// Operator software PTT hotkey (e.g. `"Ctrl+Shift+T"`), or `None` if
    /// unset. A global OS shortcut; while held it both opens this client's
    /// Mumble mic and sends an outbound `remote-radio/ptt` so the station keys
    /// its rig deterministically (not relying on VOX). Operator role only.
    #[serde(default)]
    pub ptt_hotkey: Option<String>,
    /// Operator "Whisper" hotkey (e.g. `"Ctrl+Shift+W"`), or `None` if unset. A
    /// global OS shortcut; while held it redirects the mic into
    /// [`Self::whisper_target_channel`] via a Mumble VoiceTarget, without leaving
    /// the current channel. Must differ from [`Self::ptt_hotkey`]. Operator only.
    #[serde(default)]
    pub whisper_hotkey: Option<String>,
    /// Channel id the Whisper hotkey redirects the mic into. `None` = unset
    /// (whisper disabled even if a hotkey is bound).
    #[serde(default)]
    pub whisper_target_channel: Option<u32>,
}

impl PttConfig {
    pub(crate) fn default_lead_in_ms() -> u32 {
        150
    }
    pub(crate) fn default_hang_ms() -> u32 {
        250
    }
    pub(crate) fn default_vox_hang_ms() -> u32 {
        150
    }
    pub(crate) fn default_deadman_ms() -> u32 {
        2000
    }
    pub(crate) fn default_max_tx_secs() -> u32 {
        120
    }
}

impl Default for PttConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            role: PttRole::default(),
            port: String::new(),
            line: PttLine::default(),
            active_low: false,
            vox: false,
            vox_hang_ms: Self::default_vox_hang_ms(),
            lead_in_ms: Self::default_lead_in_ms(),
            hang_ms: Self::default_hang_ms(),
            deadman_ms: Self::default_deadman_ms(),
            max_tx_secs: Self::default_max_tx_secs(),
            ptt_hotkey: None,
            whisper_hotkey: None,
            whisper_target_channel: None,
        }
    }
}

/// How the inbound stereo stream is mapped onto the output channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StereoMode {
    /// Left channel to the left ear, right to the right - the default, and
    /// what dual-receiver (main/sub RX) operating wants.
    #[default]
    Stereo,
    /// Sum both channels and play the mix on both ears. Useful for a
    /// single-receiver stream or listening on one earphone.
    Mono,
    /// Swap the channels (left<->right).
    SwapLr,
}

impl AudioSettings {
    pub(crate) fn default_max_gain() -> f32 {
        15.0
    }
    pub(crate) fn default_close_ratio() -> f32 {
        0.8
    }
    pub(crate) fn default_hold_frames() -> u32 {
        15
    }
    pub(crate) fn default_bitrate() -> i32 {
        72_000
    }
    pub(crate) fn default_frame_size_ms() -> u32 {
        20
    }
    pub(crate) fn default_noise_suppression() -> bool {
        true
    }
    pub(crate) fn default_volume() -> f32 {
        1.0
    }

    /// Convert an Opus packet duration in ms to samples-per-channel at
    /// 48 kHz.  Clamps to valid Opus frame sizes (10, 20, 40, 60 ms).
    pub fn frame_ms_to_samples(frame_size_ms: u32) -> usize {
        match frame_size_ms {
            10 => 480,
            40 => 1920,
            60 => 2880,
            _ => 960, // 20 ms default
        }
    }

    /// The effective transmit mode, reconciling the legacy `push_to_talk` flag.
    ///
    /// A config saved before `transmit_mode` existed deserialises the field to
    /// its default (`VoiceActivity`) but may carry `push_to_talk = true`. In
    /// that case push-to-talk is what the user actually chose, so it wins. Once
    /// a config is saved through the new UI the explicit mode is authoritative
    /// and the boolean is kept in step. Read transmit policy through here.
    #[must_use]
    pub fn transmit_mode(&self) -> TransmitMode {
        match self.transmit_mode {
            // Only honour the legacy flag when the explicit field is still at
            // its migration default; an explicit Continuous or VoiceActivity
            // choice must not be overridden by a stale boolean.
            TransmitMode::VoiceActivity if self.push_to_talk => TransmitMode::PushToTalk,
            mode => mode,
        }
    }

    /// Whether any pipeline-relevant setting differs from `other`.
    ///
    /// PTT key and UI-only fields are excluded since they don't
    /// require a pipeline restart.
    pub fn needs_pipeline_restart(&self, other: &Self) -> bool {
        self.selected_device != other.selected_device
            || self.auto_gain != other.auto_gain
            || (self.vad_threshold - other.vad_threshold).abs() > f32::EPSILON
            || (self.max_gain_db - other.max_gain_db).abs() > f32::EPSILON
            || (self.noise_gate_close_ratio - other.noise_gate_close_ratio).abs() > f32::EPSILON
            || self.hold_frames != other.hold_frames
            || self.bitrate_bps != other.bitrate_bps
            || self.frame_size_ms != other.frame_size_ms
            || self.noise_suppression != other.noise_suppression
            || self.denoiser_algorithm != other.denoiser_algorithm
            || self.denoiser_params != other.denoiser_params
            || self.auto_input_sensitivity != other.auto_input_sensitivity
            // The transmit mode decides whether the voice gate is in the chain,
            // so a change to it (including a change to the legacy flag that
            // feeds `transmit_mode()`) must rebuild the pipeline.
            || self.transmit_mode() != other.transmit_mode()
    }

    /// Whether the output device changed, requiring inbound pipeline restart.
    pub fn needs_inbound_restart(&self, other: &Self) -> bool {
        self.selected_output_device != other.selected_output_device
    }
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            selected_device: None,
            auto_gain: true,
            vad_threshold: 0.01,
            max_gain_db: 15.0,
            noise_gate_close_ratio: 0.8,
            hold_frames: 15,
            transmit_mode: TransmitMode::default(),
            push_to_talk: false,
            push_to_talk_key: None,
            bitrate_bps: 72_000,
            frame_size_ms: 20,
            noise_suppression: true,
            denoiser_algorithm: NoiseSuppressionAlgorithm::default(),
            denoiser_params: BTreeMap::new(),
            selected_output_device: None,
            input_volume: 1.0,
            output_volume: 1.0,
            auto_input_sensitivity: false,
            force_tcp_audio: false,
            playback_stereo_mode: StereoMode::default(),
            left_volume: 1.0,
            right_volume: 1.0,
        }
    }
}

/// Current voice state.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VoiceState {
    /// User is deaf + muted (default on connect / before enabling voice).
    #[default]
    Inactive,
    /// User has enabled voice calling - can speak and hear.
    Active,
    /// User is muted (mic off) but can still hear others.
    Muted,
}

// --- Onboarding workflow types ------------------------------------

/// Single answer chip on a multiple-choice onboarding question.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OnboardingAnswer {
    pub id: String,
    pub label: String,
    /// Channels added to the user's visible-channel set on selection.
    #[serde(default)]
    pub channel_ids: Vec<u32>,
    /// Mumble ACL group names the user is added to on selection.
    #[serde(default)]
    pub group_names: Vec<String>,
    /// Optional emoji glyph for the chip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    /// Optional description rendered beneath the label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Single multiple-choice question of the onboarding flow.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OnboardingQuestion {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub multi_select: bool,
    #[serde(default)]
    pub required: bool,
    /// True when the question must be answered before fully entering the server.
    #[serde(default)]
    pub ask_before_join: bool,
    pub answers: Vec<OnboardingAnswer>,
}

/// Server-managed onboarding configuration.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OnboardingConfig {
    pub version: u32,
    pub enabled: bool,
    #[serde(default)]
    pub default_channel_ids: Vec<u32>,
    pub questions: Vec<OnboardingQuestion>,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
}

/// One editable server setting (schema + current value), cached from a
/// `FancyServerSettings` broadcast and surfaced to the admin "Server Settings"
/// panel.  `type` drives the client's form-control factory.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ServerSetting {
    /// Config key (core keys, or `plugin.<name>.<key>` for plugin settings).
    pub key: String,
    /// Input type: `string` | `text` | `bool` | `int` | `enum` | `country` |
    /// `password`.
    #[serde(rename = "type")]
    pub r#type: String,
    /// Group/section the setting belongs to.
    pub group: String,
    /// Human-readable label.
    pub label: String,
    /// Current value (string-encoded).  Omitted for secret settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Allowed values for `enum` types.
    #[serde(default)]
    pub options: Vec<String>,
    /// Whether the value is a secret (masked, write-only).
    #[serde(default)]
    pub secret: bool,
    /// Optional one-line help text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

/// Editable server-settings snapshot advertised by the server to admins.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ServerSettingsSnapshot {
    /// All editable settings (core + currently-loaded plugins).
    pub settings: Vec<ServerSetting>,
    /// Monotonic revision so stale broadcasts can be dropped.
    pub revision: u64,
}

/// Selected answer ids for one question.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OnboardingSelection {
    pub question_id: String,
    pub answer_ids: Vec<String>,
}

/// User's onboarding response.  Sent to the server for ACL group
/// application and stored locally for the "Channels & Roles" editor.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OnboardingResponse {
    /// Cert hash of the responder (server-stamped, optional from client).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_at: Option<u64>,
    pub config_revision: u64,
    pub selections: Vec<OnboardingSelection>,
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code: panicking on failure is the intended behaviour")]
mod tests {
    use super::*;

    /// Regression test: the frontend sends `"fancy_v1_full_archive"` etc.
    /// and the parser must accept those exact strings.
    #[test]
    fn parse_pchat_protocol_str_roundtrip() {
        use super::super::parse_pchat_protocol_str;

        // Every variant the UI sends must survive a serialize -> parse roundtrip.
        let cases = [
            (PchatProtocol::None, "none"),
            (PchatProtocol::FancyV1FullArchive, "fancy_v1_full_archive"),
            (PchatProtocol::SignalV1, "signal_v1"),
        ];
        for (expected, input) in cases {
            assert_eq!(
                parse_pchat_protocol_str(input),
                expected,
                "parse_pchat_protocol_str({input:?}) should return {expected:?}",
            );
        }
    }

    #[test]
    fn serialize_channel_entry_with_signal_v1() {
        let entry = ChannelEntry {
            id: 5,
            parent_id: Some(0),
            name: "Secret".into(),
            description: String::new(),
            description_hash: None,
            user_count: 2,
            permissions: None,
            temporary: false,
            position: 0,
            max_users: 0,
            pchat_protocol: Some(PchatProtocol::SignalV1),
            pchat_max_history: Some(1000),
            pchat_retention_days: Some(7),
            pchat_key_custodians: Vec::new(), is_enter_restricted: false,
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        assert!(
            json.contains(r#""pchat_protocol":"signal_v1""#),
            "expected signal_v1 in JSON: {json}",
        );
    }
}
