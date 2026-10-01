//! `RemoteRadio` shared wire protocol.
//!
//! These types are exchanged between a `RemoteRadioStation` (host) and a
//! `RemoteRadioClient` (viewer/controller) over three WebRTC data channels
//! (`rr-control`, `rr-clipboard`, `rr-files`), plus the presence payloads
//! carried inside Mumble `WebRtcSignal` START/STOP messages.
//!
//! Everything here is pure data + (de)serialisation logic with no I/O, so it
//! is cheap to unit-test exhaustively and is shared verbatim by both apps.
//!
//! # Versioning
//!
//! Every top-level channel envelope carries a [`PROTOCOL_VERSION`]. Peers
//! exchange their version in the presence handshake ([`Presence::version`])
//! and in each envelope so an incompatible peer can be detected and refused
//! gracefully rather than misinterpreting bytes.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// Wire-protocol version. Bump on any breaking change to the message shapes
/// below. Peers compare this during the presence handshake.
pub const PROTOCOL_VERSION: u16 = 1;

/// Errors produced while decoding a wire message.
#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    /// The payload was not valid JSON for the expected type.
    #[error("malformed message: {0}")]
    Malformed(#[from] serde_json::Error),
    /// The peer announced a protocol version we cannot speak.
    #[error("incompatible protocol version: local={local}, remote={remote}")]
    IncompatibleVersion {
        /// Our version.
        local: u16,
        /// The peer's announced version.
        remote: u16,
    },
}

/// Returns `Ok(())` when a peer announcing `remote` can interoperate with the
/// local [`PROTOCOL_VERSION`].
///
/// v1 policy: exact-match only. Later versions may widen this to a range.
///
/// # Errors
/// Returns [`ProtoError::IncompatibleVersion`] when the versions differ.
pub fn check_version(remote: u16) -> Result<(), ProtoError> {
    if remote == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(ProtoError::IncompatibleVersion {
            local: PROTOCOL_VERSION,
            remote,
        })
    }
}

// ---------------------------------------------------------------------------
// Presence (carried in Mumble WebRtcSignal START/STOP payload JSON)
// ---------------------------------------------------------------------------

/// The role a channel member plays in a `RemoteRadio` session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// A host that captures and serves its screen.
    Station,
    /// A viewer/controller.
    Client,
}

/// A monitor advertised by a Station.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorInfo {
    /// Stable per-session monitor index.
    pub id: u32,
    /// Human-readable label (e.g. `"Monitor 1 (2560x1440)"`).
    pub label: String,
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// Whether this is the OS primary monitor.
    pub primary: bool,
}

/// Optional capability flags a peer advertises so the other side can enable or
/// hide features without probing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    /// Clipboard sync is offered.
    #[serde(default)]
    pub clipboard: bool,
    /// File transfer is offered.
    #[serde(default)]
    pub file_transfer: bool,
    /// Supported video codecs, most-preferred first (e.g. `["h264", "vp8"]`).
    #[serde(default)]
    pub codecs: Vec<String>,
}

/// Presence payload broadcast by a Station (as the JSON body of a
/// `WebRtcSignal` START message) so Clients can discover it. A matching STOP
/// clears availability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    /// Protocol version the sender speaks.
    pub version: u16,
    /// Sender role.
    pub role: Role,
    /// Monitors available for sharing (Station only).
    #[serde(default)]
    pub monitors: Vec<MonitorInfo>,
    /// Session id of the current controller, if any (Station only).
    #[serde(default)]
    pub controller: Option<u32>,
    /// Advertised capabilities.
    #[serde(default)]
    pub caps: Caps,
}

impl Presence {
    /// Serialise to the JSON string carried in a `WebRtcSignal` payload.
    ///
    /// # Errors
    /// Returns [`ProtoError`] if serialisation fails (practically never for
    /// these owned types).
    pub fn to_json(&self) -> Result<String, ProtoError> {
        Ok(serde_json::to_string(self)?)
    }

    /// Parse a presence payload from a `WebRtcSignal` START body.
    ///
    /// # Errors
    /// Returns [`ProtoError::Malformed`] on invalid JSON.
    pub fn from_json(s: &str) -> Result<Self, ProtoError> {
        Ok(serde_json::from_str(s)?)
    }
}

// ---------------------------------------------------------------------------
// Control channel ("rr-control"): input + handoff + session control
// ---------------------------------------------------------------------------

/// Mouse button identifiers (stable across the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    /// Primary (usually left) button.
    Left,
    /// Secondary (usually right) button.
    Right,
    /// Middle button / wheel click.
    Middle,
    /// Browser/back extra button.
    Back,
    /// Browser/forward extra button.
    Forward,
}

/// A single input event produced by the controller and injected by the
/// Station. Coordinates are **absolute**, in physical pixels, relative to the
/// addressed monitor's top-left, so the Station can map them without knowing
/// the Client's display geometry.
///
/// Tagged with `"ev"` (not `"t"`) because an `InputEvent` is nested inside the
/// internally-tagged [`ControlMsg::Input`] variant; using the same tag key for
/// both would collide into a duplicate `"t"` field on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum InputEvent {
    /// Absolute pointer move within `monitor`.
    MouseMove {
        /// Target monitor id (as advertised in [`Presence::monitors`]).
        monitor: u32,
        /// X in physical pixels from the monitor's left edge.
        x: i32,
        /// Y in physical pixels from the monitor's top edge.
        y: i32,
    },
    /// Mouse button press or release.
    MouseButton {
        /// Which button.
        button: MouseButton,
        /// `true` = pressed, `false` = released.
        down: bool,
    },
    /// Scroll wheel delta (lines; positive = up/right).
    Wheel {
        /// Horizontal delta.
        dx: i32,
        /// Vertical delta.
        dy: i32,
    },
    /// Key press or release by platform-independent key code.
    Key {
        /// Key identifier (see mapping table in `rr-input`).
        code: u32,
        /// `true` = pressed, `false` = released.
        down: bool,
    },
    /// Direct unicode text entry (IME / paste-like typing).
    Text {
        /// The text to type.
        text: String,
    },
}

/// Cooperative control-handoff and session-management messages carried on
/// `rr-control` alongside [`InputEvent`]s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ControlMsg {
    /// Carries an input event from the active controller to the Station.
    Input(InputEvent),
    /// A Client asks to become the controller.
    RequestControl,
    /// The current holder approves a pending request for `to`.
    ApproveHandoff {
        /// Session id being granted control.
        to: u32,
    },
    /// The current holder denies a pending request for `to`.
    DenyHandoff {
        /// Session id being refused.
        to: u32,
    },
    /// The current holder voluntarily gives up control.
    ReleaseControl,
    /// Station → all: authoritative control-state broadcast.
    ControlState {
        /// Current controller session id, or `None` when free.
        controller: Option<u32>,
        /// Session id with a pending request awaiting approval, if any.
        pending: Option<u32>,
    },
    /// Controller asks the Station to switch the shared monitor.
    SelectMonitor {
        /// Monitor id to share.
        monitor: u32,
    },
    /// Viewer asks the Station to emit a fresh keyframe (IDR), e.g. after loss
    /// or on first subscribe.
    RequestKeyframe,
}

impl ControlMsg {
    /// Serialise for transmission on the `rr-control` data channel.
    ///
    /// # Errors
    /// Returns [`ProtoError`] if serialisation fails.
    pub fn encode(&self) -> Result<Vec<u8>, ProtoError> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Decode a `rr-control` frame.
    ///
    /// # Errors
    /// Returns [`ProtoError::Malformed`] on invalid bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtoError> {
        Ok(serde_json::from_slice(bytes)?)
    }
}

// ---------------------------------------------------------------------------
// Clipboard channel ("rr-clipboard")
// ---------------------------------------------------------------------------

/// A clipboard update synced between peers (policy-gated, bidirectional).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClipboardMsg {
    /// UTF-8 text clipboard contents.
    Text {
        /// The clipboard text.
        text: String,
    },
    /// Image clipboard contents (PNG, base64 in JSON transport).
    ImagePng {
        /// Base64-encoded PNG bytes.
        data_base64: String,
    },
}

// ---------------------------------------------------------------------------
// File channel ("rr-files")
// ---------------------------------------------------------------------------

/// Chunked file-transfer protocol messages on `rr-files`.
///
/// Flow: `Offer` → `Accept`/`Reject` → repeated `Chunk` → `Complete`.
/// Each `Chunk` is acknowledged implicitly by data-channel reliability; the
/// final `Complete` carries a hash for integrity verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum FileMsg {
    /// Sender offers a file.
    Offer {
        /// Transfer id (unique per sender for the session).
        id: u64,
        /// File name (sanitised by the receiver before use).
        name: String,
        /// Total size in bytes.
        size: u64,
        /// Hex-encoded SHA-256 of the whole file for end-to-end integrity.
        sha256: String,
    },
    /// Receiver accepts the offer and is ready for chunks.
    Accept {
        /// Transfer id being accepted.
        id: u64,
    },
    /// Receiver rejects the offer (policy, size limit, user declined).
    Reject {
        /// Transfer id being rejected.
        id: u64,
        /// Human-readable reason.
        reason: String,
    },
    /// A chunk of file data (base64 in JSON transport).
    Chunk {
        /// Transfer id this chunk belongs to.
        id: u64,
        /// Zero-based chunk sequence number.
        seq: u32,
        /// Base64-encoded chunk bytes.
        data_base64: String,
    },
    /// Sender signals end of file.
    Complete {
        /// Transfer id completed.
        id: u64,
    },
}

#[cfg(test)]
mod tests;
