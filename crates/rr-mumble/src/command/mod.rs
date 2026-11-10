//! Self-describing commands using the Command pattern.
//!
//! Each command lives in its own file, carries its own data, and knows how to
//! produce the protocol output it needs. Only what an audio-only radio client
//! needs is kept: connect, move between channels, listen/whisper targets, mute
//! and deafen, plugin messages (remote PTT), voice and ping.

mod authenticate;
mod channel_listen;
mod disconnect;
mod join_channel;
mod permission_query;
mod send_audio;
mod send_ping;
mod send_plugin_message;
mod set_self_deaf;
mod set_self_mute;
mod set_voice_target;

pub use self::core::{BoxedCommand, CommandAction, CommandOutput};
pub use authenticate::Authenticate;
pub use channel_listen::ChannelListen;
pub use disconnect::Disconnect;
pub use join_channel::JoinChannel;
pub use permission_query::PermissionQuery;
pub use send_audio::SendAudio;
pub use send_ping::SendPing;
pub use send_plugin_message::SendPluginMessage;
pub use set_self_deaf::SetSelfDeaf;
pub use set_self_mute::SetSelfMute;
pub use set_voice_target::{SetVoiceTarget, VoiceTargetEntry};

mod core {
    use std::fmt::Debug;

    use crate::message::{ControlMessage, UdpMessage};
    use crate::state::ServerState;

    /// The output(s) a command wants to send after executing.
    #[derive(Debug, Default)]
    pub struct CommandOutput {
        /// Control messages to send over TCP.
        pub tcp_messages: Vec<ControlMessage>,
        /// Audio/ping messages to send over UDP.
        pub udp_messages: Vec<UdpMessage>,
        /// Whether the client should disconnect after processing.
        pub disconnect: bool,
    }

    /// Trait implemented by every user-facing command.
    pub trait CommandAction: Debug + Send + Sync + 'static {
        /// Produce the protocol messages this command needs to send.
        fn execute(&self, state: &ServerState) -> CommandOutput;
    }

    /// Type-erased command stored in the work queue.
    pub type BoxedCommand = Box<dyn CommandAction>;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "unwrap is acceptable in test code")]
    use super::*;
    use crate::message::{ControlMessage, UdpMessage};
    use crate::proto::mumble_tcp;
    use crate::state::ServerState;

    fn state_with_session(session_id: u32) -> ServerState {
        let mut state = ServerState::new();
        state.apply_server_sync(&mumble_tcp::ServerSync {
            session: Some(session_id),
            ..Default::default()
        });
        state
    }

    #[test]
    fn authenticate_produces_authenticate_message() {
        let cmd = Authenticate {
            username: "TestUser".into(),
            password: Some("secret".into()),
            tokens: vec!["token1".into()],
        };
        let output = cmd.execute(&ServerState::new());
        assert_eq!(output.tcp_messages.len(), 1);
        match &output.tcp_messages[0] {
            ControlMessage::Authenticate(auth) => {
                assert_eq!(auth.username.as_deref(), Some("TestUser"));
                assert_eq!(auth.opus, Some(true));
            }
            other => panic!("expected Authenticate, got {other:?}"),
        }
    }

    #[test]
    fn disconnect_sets_disconnect_flag() {
        let output = Disconnect.execute(&ServerState::new());
        assert!(output.disconnect);
    }

    #[test]
    fn join_channel_uses_session_id() {
        let cmd = JoinChannel { channel_id: 3, password: None };
        let output = cmd.execute(&state_with_session(42));
        match &output.tcp_messages[0] {
            ControlMessage::UserState(us) => {
                assert_eq!(us.session, Some(42));
                assert_eq!(us.channel_id, Some(3));
            }
            other => panic!("expected UserState, got {other:?}"),
        }
    }

    #[test]
    fn channel_listen_add_and_remove() {
        let cmd = ChannelListen { add: vec![1, 2], remove: vec![3] };
        let output = cmd.execute(&state_with_session(10));
        match &output.tcp_messages[0] {
            ControlMessage::UserState(us) => {
                assert_eq!(us.listening_channel_add, vec![1, 2]);
                assert_eq!(us.listening_channel_remove, vec![3]);
            }
            other => panic!("expected UserState, got {other:?}"),
        }
    }

    #[test]
    fn send_audio_produces_udp_message() {
        let cmd = SendAudio {
            opus_data: vec![0xDE, 0xAD],
            target: 0,
            frame_number: 42,
            positional_data: None,
            is_terminator: false,
        };
        let output = cmd.execute(&ServerState::new());
        match &output.udp_messages[0] {
            UdpMessage::Audio(a) => {
                assert_eq!(a.opus_data, vec![0xDE, 0xAD]);
                assert_eq!(a.frame_number, 42);
            }
            other => panic!("expected Audio, got {other:?}"),
        }
    }

    #[test]
    fn set_voice_target() {
        let cmd = SetVoiceTarget {
            id: 1,
            targets: vec![VoiceTargetEntry {
                sessions: vec![10, 20],
                channel_id: Some(5),
                group: None,
                links: true,
                children: false,
            }],
        };
        let output = cmd.execute(&ServerState::new());
        match &output.tcp_messages[0] {
            ControlMessage::VoiceTarget(vt) => {
                assert_eq!(vt.id, Some(1));
                assert_eq!(vt.targets[0].channel_id, Some(5));
            }
            other => panic!("expected VoiceTarget, got {other:?}"),
        }
    }
}
