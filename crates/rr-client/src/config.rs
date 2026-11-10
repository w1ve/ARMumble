//! Client configuration.

use rr_audio::capture::CaptureConfig;

/// How much of the server the client may see and use.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Scope {
    /// The whole server.
    #[default]
    Everything,
    /// One channel (matched by name, case-insensitive) and its sub-channels.
    /// The client joins it on connect and never shows or enters anything else.
    Channel(String),
}

/// Everything needed to connect and carry audio.
#[derive(Debug, Clone)]
pub struct ClientSettings {
    /// Server host name or address.
    pub host: String,
    /// Server TCP/UDP port.
    pub port: u16,
    /// Mumble user name.
    pub username: String,
    /// Optional server password.
    pub password: Option<String>,
    /// Access tokens.
    pub tokens: Vec<String>,
    /// Visibility confinement.
    pub scope: Scope,
    /// Capture pipeline configuration (device rate, channels, TX mode, ...).
    pub capture: CaptureConfig,
    /// Playback device sample rate.
    pub output_rate: u32,
    /// Requested Opus bit rate in bits per second.
    pub bitrate_bps: u32,
    /// Longest frame the user accepts, in milliseconds (latency cap).
    pub max_frame_ms: u32,
    /// Send audio over TCP only.
    pub force_tcp: bool,
    /// Reconnect automatically with back-off.
    pub auto_reconnect: bool,
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            host: "localhost".into(),
            port: 64738,
            username: "RemoteRadio".into(),
            password: None,
            tokens: Vec::new(),
            scope: Scope::Everything,
            capture: CaptureConfig::default(),
            output_rate: 48_000,
            bitrate_bps: 128_000,
            max_frame_ms: 40,
            force_tcp: false,
            auto_reconnect: true,
        }
    }
}

/// Fluent builder for [`ClientSettings`].
#[derive(Debug, Clone, Default)]
pub struct ClientBuilder {
    pub(crate) settings: ClientSettings,
}

impl ClientBuilder {
    /// Server host and port.
    #[must_use]
    pub fn server(mut self, host: impl Into<String>, port: u16) -> Self {
        self.settings.host = host.into();
        self.settings.port = port;
        self
    }

    /// User name.
    #[must_use]
    pub fn username(mut self, name: impl Into<String>) -> Self {
        self.settings.username = name.into();
        self
    }

    /// Server password.
    #[must_use]
    pub fn password(mut self, pw: impl Into<String>) -> Self {
        self.settings.password = Some(pw.into());
        self
    }

    /// Confine the client to a scope.
    #[must_use]
    pub fn scope(mut self, scope: Scope) -> Self {
        self.settings.scope = scope;
        self
    }

    /// Capture pipeline configuration.
    #[must_use]
    pub fn capture(mut self, cfg: CaptureConfig) -> Self {
        self.settings.capture = cfg;
        self
    }

    /// Playback device rate.
    #[must_use]
    pub fn output_rate(mut self, hz: u32) -> Self {
        self.settings.output_rate = hz;
        self
    }

    /// Requested bit rate and longest accepted frame.
    #[must_use]
    pub fn quality(mut self, bitrate_bps: u32, max_frame_ms: u32) -> Self {
        self.settings.bitrate_bps = bitrate_bps;
        self.settings.max_frame_ms = max_frame_ms;
        self
    }

    /// Replace all settings.
    #[must_use]
    pub fn settings(mut self, s: ClientSettings) -> Self {
        self.settings = s;
        self
    }
}
