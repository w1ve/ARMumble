//! Wire-level message types.
//!
//! [`TcpMessageType`] maps numeric wire IDs to their protobuf types.
//! [`ControlMessage`] and [`UdpMessage`] carry fully decoded payloads.
//! [`ServerMessage`] is the unified inbound type used by the work queue.
//!
//! Only the messages an audio-only client needs are modelled. Anything else a
//! server sends (text, ACLs, ban lists, extension messages) decodes to
//! [`ControlMessage::Unknown`] and is skipped.
use crate::proto::{mumble_tcp, mumble_udp};

/// Mumble TCP message type IDs as defined by the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum TcpMessageType {
    /// Protocol version negotiation message.
    Version = 0,
    /// UDP audio tunnelled over TCP (fallback).
    UdpTunnel = 1,
    /// Client authentication handshake.
    Authenticate = 2,
    /// Keep-alive ping.
    Ping = 3,
    /// Server rejects the connection.
    Reject = 4,
    /// Server acknowledges successful authentication.
    ServerSync = 5,
    /// Server notifies clients that a channel was removed.
    ChannelRemove = 6,
    /// Channel metadata update.
    ChannelState = 7,
    /// A user disconnected from the server.
    UserRemove = 8,
    /// User state (mute, deafen, channel, etc.) changed.
    UserState = 9,
    /// Server denies an action.
    PermissionDenied = 12,
    /// Encryption key setup for the OCB-encrypted UDP path.
    CryptSetup = 15,
    /// Configure a voice target for whisper/shout.
    VoiceTarget = 19,
    /// Query or response for channel permissions.
    PermissionQuery = 20,
    /// Negotiated audio codec version.
    CodecVersion = 21,
    /// Global server configuration values (max bandwidth, limits, etc.).
    ServerConfig = 24,
    /// Generic plugin envelope (remote PTT), supported by Fancy-style servers.
    PluginMessage = 200,
}

impl TryFrom<u16> for TcpMessageType {
    type Error = crate::error::Error;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => Self::Version,
            1 => Self::UdpTunnel,
            2 => Self::Authenticate,
            3 => Self::Ping,
            4 => Self::Reject,
            5 => Self::ServerSync,
            6 => Self::ChannelRemove,
            7 => Self::ChannelState,
            8 => Self::UserRemove,
            9 => Self::UserState,
            12 => Self::PermissionDenied,
            15 => Self::CryptSetup,
            19 => Self::VoiceTarget,
            20 => Self::PermissionQuery,
            21 => Self::CodecVersion,
            24 => Self::ServerConfig,
            200 => Self::PluginMessage,
            other => return Err(crate::error::Error::UnknownMessageType(other)),
        })
    }
}

/// A decoded TCP control message received from (or to be sent to) the server.
#[derive(Debug, Clone)]
pub enum ControlMessage {
    /// Protocol version negotiation.
    Version(mumble_tcp::Version),
    /// Client authentication.
    Authenticate(mumble_tcp::Authenticate),
    /// Keep-alive ping.
    Ping(mumble_tcp::Ping),
    /// Server rejected the connection.
    Reject(mumble_tcp::Reject),
    /// Successful authentication acknowledgement.
    ServerSync(mumble_tcp::ServerSync),
    /// A channel was removed.
    ChannelRemove(mumble_tcp::ChannelRemove),
    /// Channel metadata update.
    ChannelState(mumble_tcp::ChannelState),
    /// A user disconnected.
    UserRemove(mumble_tcp::UserRemove),
    /// User state change (mute, channel, etc.).
    UserState(mumble_tcp::UserState),
    /// Server denied an action.
    PermissionDenied(mumble_tcp::PermissionDenied),
    /// OCB encryption key setup.
    CryptSetup(mumble_tcp::CryptSetup),
    /// Voice target (whisper/shout) configuration.
    VoiceTarget(mumble_tcp::VoiceTarget),
    /// Channel permission query or response.
    PermissionQuery(mumble_tcp::PermissionQuery),
    /// Negotiated audio codec version.
    CodecVersion(mumble_tcp::CodecVersion),
    /// Global server configuration values.
    ServerConfig(mumble_tcp::ServerConfig),
    /// Generic plugin envelope (bidirectional).
    PluginMessage(mumble_tcp::PluginMessage),
    /// UDP audio tunneled through TCP (fallback path).
    UdpTunnel(Vec<u8>),
    /// A message type this client does not use. Carries the wire type ID.
    Unknown(u16),
}

impl ControlMessage {
    /// The wire ID for this message.
    #[must_use]
    pub fn type_id(&self) -> u16 {
        match self {
            Self::Version(_) => TcpMessageType::Version as u16,
            Self::Authenticate(_) => TcpMessageType::Authenticate as u16,
            Self::Ping(_) => TcpMessageType::Ping as u16,
            Self::Reject(_) => TcpMessageType::Reject as u16,
            Self::ServerSync(_) => TcpMessageType::ServerSync as u16,
            Self::ChannelRemove(_) => TcpMessageType::ChannelRemove as u16,
            Self::ChannelState(_) => TcpMessageType::ChannelState as u16,
            Self::UserRemove(_) => TcpMessageType::UserRemove as u16,
            Self::UserState(_) => TcpMessageType::UserState as u16,
            Self::PermissionDenied(_) => TcpMessageType::PermissionDenied as u16,
            Self::CryptSetup(_) => TcpMessageType::CryptSetup as u16,
            Self::VoiceTarget(_) => TcpMessageType::VoiceTarget as u16,
            Self::PermissionQuery(_) => TcpMessageType::PermissionQuery as u16,
            Self::CodecVersion(_) => TcpMessageType::CodecVersion as u16,
            Self::ServerConfig(_) => TcpMessageType::ServerConfig as u16,
            Self::PluginMessage(_) => TcpMessageType::PluginMessage as u16,
            Self::UdpTunnel(_) => TcpMessageType::UdpTunnel as u16,
            Self::Unknown(id) => *id,
        }
    }
}

/// A decoded UDP message - either audio or a UDP ping.
#[derive(Debug, Clone)]
pub enum UdpMessage {
    /// An audio packet (encoded speech or music).
    Audio(mumble_udp::Audio),
    /// A UDP-level ping for latency measurement.
    Ping(mumble_udp::Ping),
}

/// Unified inbound message from either transport.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant, reason = "Control variant must hold a full ControlMessage; boxing would add heap allocation on the hot audio path")]
pub enum ServerMessage {
    /// Control-plane message received over TCP.
    Control(ControlMessage),
    /// Real-time audio/ping received over UDP (or UDP-over-TCP tunnel).
    Udp(UdpMessage),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_ids_round_trip() {
        for t in [
            TcpMessageType::Version,
            TcpMessageType::UdpTunnel,
            TcpMessageType::UserState,
            TcpMessageType::VoiceTarget,
            TcpMessageType::PluginMessage,
        ] {
            assert_eq!(TcpMessageType::try_from(t as u16).ok(), Some(t));
        }
    }

    #[test]
    fn text_and_extension_ids_are_unknown() {
        for id in [11_u16, 100, 131, 999] {
            assert!(TcpMessageType::try_from(id).is_err());
        }
    }

    #[test]
    fn unknown_keeps_its_id() {
        assert_eq!(ControlMessage::Unknown(11).type_id(), 11);
    }
}
