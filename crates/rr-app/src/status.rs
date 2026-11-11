//! The status snapshot pushed to the UI ten times a second.

use serde::Serialize;

/// Connection phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Not connected and not trying.
    Disconnected,
    /// Connecting or reconnecting.
    Connecting,
    /// Logged in.
    Connected,
}

/// Connection info updated from client events.
#[derive(Debug, Clone)]
pub struct ConnInfo {
    /// Phase.
    pub phase: Phase,
    /// Last problem, shown to the user.
    pub error: Option<String>,
    /// Audio path is UDP (true) or the TCP tunnel (false).
    pub udp: Option<bool>,
    /// The configured scope channel does not exist.
    pub scope_missing: bool,
}

impl Default for ConnInfo {
    fn default() -> Self {
        Self { phase: Phase::Disconnected, error: None, udp: None, scope_missing: false }
    }
}

/// A user in the roster.
#[derive(Debug, Clone, Serialize)]
pub struct UserDto {
    /// Session id.
    pub session: u32,
    /// Name.
    pub name: String,
    /// Channel id.
    pub channel_id: u32,
    /// Self-muted.
    pub self_mute: bool,
    /// Self-deafened.
    pub self_deaf: bool,
}

/// A channel in the tree.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelDto {
    /// Id.
    pub id: u32,
    /// Parent id.
    pub parent_id: Option<u32>,
    /// Name.
    pub name: String,
}

/// A talker being received.
#[derive(Debug, Clone, Serialize)]
pub struct SpeakerDto {
    /// Session id.
    pub session: u32,
    /// Audio is flowing.
    pub active: bool,
    /// Buffered ms.
    pub depth_ms: f32,
    /// Target ms.
    pub target_ms: f32,
    /// Network jitter ms.
    pub jitter_ms: f32,
    /// Drift correction ppm.
    pub ratio_ppm: f32,
    /// Concealed frames.
    pub lost: u64,
    /// Too-late packets.
    pub late: u64,
    /// Buffer underruns.
    pub underruns: u64,
}

/// The encoder plan.
#[derive(Debug, Clone, Serialize)]
pub struct PlanDto {
    /// Bit rate used, kbit/s.
    pub bitrate_kbps: f32,
    /// Frame size, ms.
    pub frame_ms: u32,
    /// The server limit forced a lower bit rate than requested.
    pub clamped: bool,
    /// Rate on the wire, kbit/s.
    pub wire_kbps: f32,
}

/// Audio levels, linear 0..1+, sent as the `levels` event.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Meters {
    /// Input left peak.
    pub in_l: f32,
    /// Input right peak.
    pub in_r: f32,
    /// Output left peak.
    pub out_l: f32,
    /// Output right peak.
    pub out_r: f32,
    /// Input samples that exceeded full scale.
    pub in_clips: u64,
    /// Output samples that exceeded full scale.
    pub out_clips: u64,
    /// An input stream is running (connected, or the idle monitor).
    pub live: bool,
}

/// Everything the UI shows.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    /// Phase.
    pub phase: Phase,
    /// Last error.
    pub error: Option<String>,
    /// Scope channel missing.
    pub scope_missing: bool,
    /// Audio over UDP.
    pub udp: Option<bool>,
    /// Our session.
    pub own_session: Option<u32>,
    /// Our channel.
    pub own_channel: Option<u32>,
    /// Users in scope.
    pub users: Vec<UserDto>,
    /// Channels in scope.
    pub channels: Vec<ChannelDto>,
    /// Active talkers.
    pub speakers: Vec<SpeakerDto>,
    /// Encoder plan.
    pub plan: Option<PlanDto>,
    /// Currently sending audio.
    pub transmitting: bool,
    /// PTT is held.
    pub ptt_down: bool,
    /// Packets sent / dropped.
    pub sent: u64,
    /// Packets dropped.
    pub dropped: u64,
    /// Sound-card stream errors.
    pub audio_errors: u64,
    /// Last sound-card error.
    pub audio_error: String,
    /// Sessions currently holding remote PTT.
    pub remote_ptt: Vec<u32>,
}

impl Status {
    /// The status shown when no client is running.
    #[must_use]
    pub fn idle() -> Self {
        Self {
            phase: Phase::Disconnected,
            error: None,
            scope_missing: false,
            udp: None,
            own_session: None,
            own_channel: None,
            users: Vec::new(),
            channels: Vec::new(),
            speakers: Vec::new(),
            plan: None,
            transmitting: false,
            ptt_down: false,
            sent: 0,
            dropped: 0,
            audio_errors: 0,
            audio_error: String::new(),
            remote_ptt: Vec::new(),
        }
    }
}
