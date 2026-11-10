//! `rr-mumble` - the Mumble protocol core of `RemoteRadio`.
//!
//! An async Mumble client reduced to what an audio-only radio client needs:
//! TCP control over TLS, OCB2-encrypted UDP voice with TCP-tunnel fallback,
//! channel state, listen/whisper targets and plugin messages. There is no text,
//! media, persistent chat or audio processing here.
//!
//! ```text
//!  [TCP reader] --+    [UDP reader] --+
//!                  v                  v
//!          [Priority Work Queue]
//!          (UDP > TCP > User commands)
//!                  |
//!          [Client Event Loop]
//!           - updates ServerState
//!           - invokes EventHandler
//!           - executes CommandAction outputs
//! ```

/// Extension version announced to the server so Fancy-style servers relay our
/// plugin messages (remote PTT). Encoded with the Mumble v2 scheme.
pub const EXTENSION_VERSION: u64 = 3_u64 << 32;

pub mod client;
pub mod command;
pub mod error;
pub mod event;
pub mod message;
pub mod proto;
pub mod state;
pub mod transport;
pub mod work_queue;
