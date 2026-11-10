//! State shared between the connection task, the audio threads and the API.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use rr_audio::bandwidth::{plan, BandwidthPlan};
use rr_audio::capture::CaptureControl;
use rr_audio::codec::EncoderSettings;
use rr_audio::playout::PlayoutHandle;
use rr_mumble::client::ClientHandle;
use rr_mumble::state::ServerState;
use rr_radio::SeqFilter;
use tokio::sync::broadcast;

use crate::config::{ClientSettings, Scope};
use crate::event::{ChannelInfo, Event, UserInfo};

/// Voice target slot used for whispers.
pub(crate) const WHISPER_SLOT: u32 = 1;
/// Inbound audio counts as active for this long after the last packet.
const ACTIVITY_TIMEOUT: Duration = Duration::from_millis(250);

/// Where the scope points once channels are known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeRoot {
    All,
    Channel(u32),
    Missing,
}

/// Tracks whether inbound audio is flowing, for VOX keying of a rig.
#[derive(Debug)]
pub(crate) struct Activity {
    epoch: Instant,
    last_ms: AtomicU64,
    open: AtomicBool,
}

impl Activity {
    fn new() -> Self {
        Self { epoch: Instant::now(), last_ms: AtomicU64::new(0), open: AtomicBool::new(false) }
    }

    pub(crate) fn packet(&self, terminator: bool) {
        self.last_ms.store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
        self.open.store(!terminator, Ordering::Relaxed);
    }

    pub(crate) fn is_active(&self) -> bool {
        let age = self.epoch.elapsed().as_millis() as u64 - self.last_ms.load(Ordering::Relaxed);
        self.open.load(Ordering::Relaxed) && age < ACTIVITY_TIMEOUT.as_millis() as u64
    }
}

#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) settings: ClientSettings,
    pub(crate) state: Mutex<ServerState>,
    pub(crate) handle: Mutex<Option<ClientHandle>>,
    pub(crate) events: broadcast::Sender<Event>,
    pub(crate) playout: PlayoutHandle,
    pub(crate) capture: Arc<CaptureControl>,
    pub(crate) target: AtomicU32,
    pub(crate) mute_home: AtomicBool,
    pub(crate) activity: Activity,
    pub(crate) server_limit: AtomicU32,
    pub(crate) bitrate: AtomicU32,
    pub(crate) stereo: AtomicBool,
    pub(crate) max_frame: AtomicU32,
    pub(crate) ptt_filters: Mutex<HashMap<u32, SeqFilter>>,
    pub(crate) ptt_seq: AtomicU64,
    pub(crate) ptt_down: AtomicBool,
    pub(crate) sent: AtomicU64,
    pub(crate) dropped: AtomicU64,
    pub(crate) plan: Mutex<Option<BandwidthPlan>>,
    pub(crate) rt: OnceLock<tokio::runtime::Handle>,
    pub(crate) shutdown: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Shared {
    pub(crate) fn new(
        settings: ClientSettings,
        playout: PlayoutHandle,
        capture: Arc<CaptureControl>,
    ) -> Arc<Self> {
        let (events, _) = broadcast::channel(256);
        Arc::new(Self {
            bitrate: AtomicU32::new(settings.bitrate_bps),
            stereo: AtomicBool::new(settings.capture.encoder.stereo),
            max_frame: AtomicU32::new(settings.max_frame_ms),
            settings,
            state: Mutex::new(ServerState::new()),
            handle: Mutex::new(None),
            events,
            playout,
            capture,
            target: AtomicU32::new(0),
            mute_home: AtomicBool::new(false),
            activity: Activity::new(),
            server_limit: AtomicU32::new(0),
            ptt_filters: Mutex::new(HashMap::new()),
            ptt_seq: AtomicU64::new(0),
            ptt_down: AtomicBool::new(false),
            sent: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            plan: Mutex::new(None),
            rt: OnceLock::new(),
            shutdown: AtomicBool::new(false),
        })
    }

    pub(crate) fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    pub(crate) fn state(&self) -> MutexGuard<'_, ServerState> {
        lock(&self.state)
    }

    pub(crate) fn handle(&self) -> Option<ClientHandle> {
        lock(&self.handle).clone()
    }

    /// Non-blocking variant for the audio thread.
    pub(crate) fn try_handle(&self) -> Option<ClientHandle> {
        self.handle.try_lock().ok().and_then(|g| g.clone())
    }

    pub(crate) fn set_handle(&self, h: Option<ClientHandle>) {
        *lock(&self.handle) = h;
    }

    pub(crate) fn ptt_filters(&self) -> MutexGuard<'_, HashMap<u32, SeqFilter>> {
        lock(&self.ptt_filters)
    }

    pub(crate) fn current_plan(&self) -> Option<BandwidthPlan> {
        *lock(&self.plan)
    }

    pub(crate) fn scope_root(&self, st: &ServerState) -> ScopeRoot {
        match &self.settings.scope {
            Scope::Everything => ScopeRoot::All,
            Scope::Channel(name) => st
                .channels
                .values()
                .find(|c| c.name.eq_ignore_ascii_case(name))
                .map_or(ScopeRoot::Missing, |c| ScopeRoot::Channel(c.channel_id)),
        }
    }

    /// Whether `channel` lies inside the scope.
    pub(crate) fn in_scope(&self, st: &ServerState, channel: u32) -> bool {
        match self.scope_root(st) {
            ScopeRoot::All => true,
            ScopeRoot::Missing => false,
            ScopeRoot::Channel(root) => {
                let mut cur = Some(channel);
                for _ in 0..64 {
                    match cur {
                        Some(id) if id == root => return true,
                        Some(id) => cur = st.channels.get(&id).and_then(|c| c.parent_id),
                        None => return false,
                    }
                }
                false
            }
        }
    }

    pub(crate) fn own_channel(&self, st: &ServerState) -> Option<u32> {
        st.own_session().and_then(|s| st.users.get(&s)).map(|u| u.channel_id)
    }

    pub(crate) fn users(&self) -> Vec<UserInfo> {
        let st = self.state();
        let mut v: Vec<UserInfo> = st
            .users
            .values()
            .filter(|u| self.in_scope(&st, u.channel_id))
            .map(|u| UserInfo {
                session: u.session,
                name: u.name.clone(),
                channel_id: u.channel_id,
                self_mute: u.self_mute,
                self_deaf: u.self_deaf,
            })
            .collect();
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        v
    }

    pub(crate) fn channels(&self) -> Vec<ChannelInfo> {
        let st = self.state();
        let root = self.scope_root(&st);
        let mut v: Vec<ChannelInfo> = st
            .channels
            .values()
            .filter(|c| self.in_scope(&st, c.channel_id))
            .map(|c| ChannelInfo {
                id: c.channel_id,
                parent_id: if root == ScopeRoot::Channel(c.channel_id) { None } else { c.parent_id },
                name: c.name.clone(),
                position: c.position,
                permissions: c.permissions,
            })
            .collect();
        v.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        v
    }

    /// Recompute the encoder settings from the user's request and the server's
    /// limit, apply them to the capture engine and announce the result.
    pub(crate) fn replan(&self) {
        let limit = self.server_limit.load(Ordering::Relaxed);
        let stereo = self.stereo.load(Ordering::Relaxed);
        let p = plan(
            self.bitrate.load(Ordering::Relaxed),
            stereo,
            self.max_frame.load(Ordering::Relaxed),
            (limit > 0).then_some(limit),
        );
        self.capture.set_encoder(EncoderSettings { bitrate_bps: p.bitrate_bps, frame_ms: p.frame_ms, stereo });
        let changed = {
            let mut slot = lock(&self.plan);
            let changed = *slot != Some(p);
            *slot = Some(p);
            changed
        };
        if changed {
            self.emit(Event::Bandwidth(p));
        }
    }
}
