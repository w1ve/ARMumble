//! Bridges `rr-mumble` events into the shared client state and the audio engine.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use rr_mumble::command::JoinChannel;
use rr_mumble::event::EventHandler;
use rr_mumble::message::{ControlMessage, UdpMessage};
use rr_radio::message::{PttMessage, So2rMessage, PAYLOAD_TYPE_PTT, PAYLOAD_TYPE_SO2R, PLUGIN_NAME};

use crate::event::Event;
use crate::shared::{ScopeRoot, Shared};

/// Receives everything the connection produces.
#[derive(Debug)]
pub(crate) struct Handler {
    pub(crate) shared: Arc<Shared>,
}

impl Handler {
    fn on_plugin(&self, p: &rr_mumble::proto::mumble_tcp::PluginMessage) {
        if p.plugin_name.as_deref() != Some(PLUGIN_NAME) {
            return;
        }
        let (Some(session), Some(bytes)) = (p.sender_session, p.payload.as_deref()) else {
            return;
        };
        match p.payload_type.as_deref() {
            Some(PAYLOAD_TYPE_PTT) => self.on_ptt(session, bytes),
            Some(PAYLOAD_TYPE_SO2R) => self.on_so2r(session, bytes),
            _ => {}
        }
    }

    fn on_so2r(&self, session: u32, bytes: &[u8]) {
        let Some(msg) = So2rMessage::decode(bytes) else {
            return;
        };
        let Some(state) = msg.state() else {
            return;
        };
        if self.shared.so2r_seq_accept(session, msg.seq) {
            self.shared.emit(Event::So2r { session, state });
        }
    }

    fn on_ptt(&self, session: u32, bytes: &[u8]) {
        let Some(msg) = PttMessage::decode(bytes) else {
            return;
        };
        let down = self.shared.ptt_filters().entry(session).or_default().accept(msg);
        if let Some(down) = down {
            self.shared.emit(Event::RemotePtt { session, down });
        }
    }

    /// After login, move into the scope channel if we are outside it.
    fn enter_scope(&self) {
        let target = {
            let st = self.shared.state();
            let root = match self.shared.scope_root(&st) {
                ScopeRoot::Channel(id) => id,
                ScopeRoot::All => return,
                ScopeRoot::Missing => {
                    drop(st);
                    self.shared.emit(Event::ScopeNotFound(scope_name(&self.shared)));
                    return;
                }
            };
            match self.shared.own_channel(&st) {
                Some(c) if self.shared.in_scope(&st, c) => return,
                _ => root,
            }
        };
        self.spawn_join(target);
    }

    fn spawn_join(&self, channel_id: u32) {
        let (Some(h), Some(rt)) = (self.shared.handle(), self.shared.rt.get()) else {
            return;
        };
        let _task = rt.spawn(async move {
            let _ = h.send(JoinChannel { channel_id, password: None }).await;
        });
    }

    fn on_audio(&self, a: &rr_mumble::proto::mumble_udp::Audio) {
        let sh = &self.shared;
        if sh.mute_home.load(Ordering::Relaxed) {
            let st = sh.state();
            let home = sh.own_channel(&st);
            let theirs = st.users.get(&a.sender_session).map(|u| u.channel_id);
            if home.is_some() && home == theirs {
                return;
            }
        }
        sh.activity.packet(a.is_terminator && a.opus_data.is_empty());
        for p in sh.playouts() {
            p.push_packet(a.sender_session, a.frame_number, a.opus_data.clone(), a.is_terminator);
        }
    }

    fn apply_state(&self, msg: &ControlMessage) -> bool {
        let mut st = self.shared.state();
        match msg {
            ControlMessage::UserState(u) => st.apply_user_state(u),
            ControlMessage::UserRemove(u) => st.remove_user(u.session),
            ControlMessage::ChannelState(c) => st.apply_channel_state(c),
            ControlMessage::ChannelRemove(c) => st.remove_channel(c.channel_id),
            ControlMessage::PermissionQuery(p) => st.apply_permission_query(p),
            ControlMessage::Version(v) => st.apply_version(v),
            ControlMessage::ServerSync(s) => st.apply_server_sync(s),
            _ => return false,
        }
        true
    }
}

fn scope_name(sh: &Shared) -> String {
    match &sh.settings.scope {
        crate::config::Scope::Channel(n) => n.clone(),
        crate::config::Scope::Everything => String::new(),
    }
}

impl EventHandler for Handler {
    fn on_control_message(&mut self, msg: &ControlMessage) {
        let changed = self.apply_state(msg);
        match msg {
            ControlMessage::ServerSync(s) => {
                if let Some(max) = s.max_bandwidth {
                    self.shared.server_limit.store(max, Ordering::Relaxed);
                    self.shared.replan();
                }
                self.enter_scope();
                self.shared.emit(Event::Connected { session: s.session.unwrap_or(0) });
            }
            ControlMessage::ServerConfig(c) => {
                if let Some(max) = c.max_bandwidth {
                    self.shared.server_limit.store(max, Ordering::Relaxed);
                    self.shared.replan();
                }
            }
            ControlMessage::PluginMessage(p) => self.on_plugin(p),
            ControlMessage::Reject(r) => {
                // A refused login will not improve by retrying.
                self.shared.shutdown.store(true, Ordering::Relaxed);
                self.shared.emit(Event::Rejected { reason: r.reason.clone().unwrap_or_default() });
            }
            ControlMessage::PermissionDenied(p) => {
                self.shared.emit(Event::Denied(p.reason.clone().unwrap_or_default()));
            }
            _ => {}
        }
        if changed && matches!(msg, ControlMessage::UserState(_) | ControlMessage::UserRemove(_) | ControlMessage::ChannelState(_) | ControlMessage::ChannelRemove(_)) {
            self.shared.emit(Event::Roster);
        }
    }

    fn on_udp_message(&mut self, msg: &UdpMessage) {
        if let UdpMessage::Audio(a) = msg {
            self.on_audio(a);
        }
    }

    fn on_audio_transport_changed(&mut self, udp_active: bool) {
        self.shared.emit(Event::Transport { udp: udp_active });
    }
}
