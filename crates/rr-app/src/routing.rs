//! Two-radio (SO2R) operation.
//!
//! The station sends radio 1 on the left channel and radio 2 on the right. A
//! shared state says which radio transmits and what the operator hears. It
//! comes from, in order of precedence:
//!
//! 1. a **local** OTRSP port (the logger runs on this computer),
//! 2. a **remote** announcement from the position whose logger drives OTRSP,
//! 3. the **manual** buttons.
//!
//! The position whose logger is local announces the state to the channel, so
//! the logger can run at either end. Each position applies only its part:
//!
//! * **Operator:** the headphones play radio 1, radio 2 or both.
//! * **Station:** the operator's voice goes to radio 1's or radio 2's mic
//!   input, and that radio's PTT line keys.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rr_audio::playout::StereoMode;
use rr_client::RadioClient;
use rr_radio::{Listen, OtrspCommand, Radio, So2rState};
use serde::Serialize;

use crate::settings::AudioSettings;

/// A remote announcement older than this no longer counts.
const REMOTE_FRESH: Duration = Duration::from_secs(6);

/// What this computer is in a two-radio setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Headphones and a mic; listens to the radios.
    Operator,
    /// Has the radios; feeds the operator's voice to them and keys them.
    Station,
}

impl Role {
    /// Parse a settings value; anything unknown is `Operator`.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("station") {
            Self::Station
        } else {
            Self::Operator
        }
    }
}

/// Where the state in effect came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The buttons in this window.
    Manual,
    /// The logger on this computer, over OTRSP.
    Local,
    /// The logger at the other position, relayed over the server.
    Remote,
}

/// Parse the manual state stored in settings.
#[must_use]
pub fn manual_state(a: &AudioSettings) -> So2rState {
    let tx = if a.so2r_tx == 2 { Radio::Two } else { Radio::One };
    let rx = match a.so2r_rx.as_str() {
        "2" => Listen::Single(Radio::Two),
        "both" => Listen::Both,
        _ => Listen::Single(Radio::One),
    };
    So2rState { tx, rx }
}

/// Live two-radio state shared by the UI, OTRSP and the audio engine.
#[derive(Debug)]
pub struct So2r {
    local: Mutex<So2rState>,
    remote: Mutex<Option<(So2rState, Instant)>>,
    /// Transmitting radio (1 or 2) for the station keyers.
    pub tx_select: Arc<AtomicU8>,
}

impl Default for So2r {
    fn default() -> Self {
        Self { local: Mutex::default(), remote: Mutex::new(None), tx_select: Arc::new(AtomicU8::new(1)) }
    }
}

impl So2r {
    /// Apply a command from the local OTRSP port; true if the state changed.
    pub fn otrsp(&self, cmd: &OtrspCommand) -> bool {
        self.local.lock().unwrap_or_else(PoisonError::into_inner).apply(cmd)
    }

    /// The state from the local OTRSP port.
    #[must_use]
    pub fn local(&self) -> So2rState {
        *self.local.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Record an announcement from another position.
    pub fn remote_update(&self, state: So2rState) {
        *self.remote.lock().unwrap_or_else(PoisonError::into_inner) = Some((state, Instant::now()));
    }

    /// Forget remote announcements (on disconnect).
    pub fn clear_remote(&self) {
        *self.remote.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// The latest remote state if it is still fresh.
    #[must_use]
    pub fn remote(&self) -> Option<So2rState> {
        let r = *self.remote.lock().unwrap_or_else(PoisonError::into_inner);
        r.filter(|(_, at)| at.elapsed() < REMOTE_FRESH).map(|(s, _)| s)
    }

    /// The state in effect and its source.
    #[must_use]
    pub fn effective(&self, a: &AudioSettings, otrsp_open: bool) -> (So2rState, Source) {
        if a.otrsp_enabled {
            if otrsp_open {
                return (self.local(), Source::Local);
            }
            if let Some(s) = self.remote() {
                return (s, Source::Remote);
            }
        }
        (manual_state(a), Source::Manual)
    }
}

/// How the playouts and keyers should be set for a state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// Presentation of output A.
    pub mode_a: StereoMode,
    /// Output A muted.
    pub mute_a: bool,
    /// Output B muted (if there is one).
    pub mute_b: bool,
    /// Radio to key (1 or 2).
    pub key: u8,
}

/// Work out the plan for `state`, or the normal single-radio plan when
/// two-radio operation is off.
#[must_use]
pub fn plan(a: &AudioSettings, state: Option<So2rState>, has_out_b: bool) -> Plan {
    let plain = if a.mono_output { StereoMode::Mono } else { StereoMode::Stereo };
    let Some(s) = state.filter(|_| a.so2r_enabled) else {
        return Plan { mode_a: plain, mute_a: false, mute_b: false, key: 1 };
    };
    match Role::parse(&a.so2r_role) {
        Role::Operator => {
            let mode_a = match s.rx {
                Listen::Single(Radio::One) => StereoMode::LeftOnly,
                Listen::Single(Radio::Two) => StereoMode::RightOnly,
                Listen::Both => plain,
            };
            Plan { mode_a, mute_a: false, mute_b: false, key: 1 }
        }
        Role::Station => {
            let two = s.tx == Radio::Two;
            let key = if two { 2 } else { 1 };
            if a.so2r_single_device {
                let mode_a = if two { StereoMode::ToRight } else { StereoMode::ToLeft };
                Plan { mode_a, mute_a: false, mute_b: false, key }
            } else if has_out_b {
                Plan { mode_a: StereoMode::Stereo, mute_a: two, mute_b: !two, key }
            } else {
                Plan { mode_a: StereoMode::Stereo, mute_a: false, mute_b: false, key }
            }
        }
    }
}

/// Apply a plan to a running client and the keyers.
pub fn apply(client: &RadioClient, p: Plan, tx_select: &AtomicU8) {
    let a = client.playout();
    a.set_stereo_mode(p.mode_a);
    a.set_muted(p.mute_a);
    if let Some(b) = client.playout_b() {
        b.set_stereo_mode(StereoMode::Stereo);
        b.set_muted(p.mute_b);
    }
    tx_select.store(p.key, Ordering::Relaxed);
}

/// The two-radio state as the UI shows it.
#[derive(Debug, Clone, Serialize)]
pub struct RouteDto {
    /// Two-radio operation is switched on.
    pub enabled: bool,
    /// This computer's role.
    pub role: Role,
    /// Transmitting radio, 1 or 2.
    pub tx: u8,
    /// Listening: `1`, `2` or `both`.
    pub rx: String,
    /// Where the state came from.
    pub source: Source,
    /// OTRSP is usable: a local port is open or another position is announcing.
    pub otrsp_available: bool,
    /// The local OTRSP port is open.
    pub otrsp_local: bool,
    /// Why the local OTRSP port could not be opened.
    pub otrsp_error: Option<String>,
}

/// Build the UI view.
#[must_use]
pub fn dto(so2r: &So2r, a: &AudioSettings, otrsp_open: bool, otrsp_error: Option<String>) -> RouteDto {
    let (s, source) = so2r.effective(a, otrsp_open);
    let n = |r: Radio| if r == Radio::One { 1 } else { 2 };
    RouteDto {
        enabled: a.so2r_enabled,
        role: Role::parse(&a.so2r_role),
        tx: n(s.tx),
        rx: match s.rx {
            Listen::Single(r) => n(r).to_string(),
            Listen::Both => "both".into(),
        },
        source,
        otrsp_available: otrsp_open || so2r.remote().is_some(),
        otrsp_local: otrsp_open,
        otrsp_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(role: &str) -> AudioSettings {
        AudioSettings { so2r_enabled: true, so2r_role: role.into(), ..AudioSettings::default() }
    }

    #[test]
    fn operator_hears_the_selected_radio() {
        let a = settings("operator");
        let p = |rx| plan(&a, Some(So2rState { tx: Radio::Two, rx }), false);
        assert_eq!(p(Listen::Single(Radio::One)).mode_a, StereoMode::LeftOnly);
        assert_eq!(p(Listen::Single(Radio::Two)).mode_a, StereoMode::RightOnly);
        assert_eq!(p(Listen::Both).mode_a, StereoMode::Stereo);
        assert_eq!(p(Listen::Both).key, 1, "the operator keys nothing");
    }

    #[test]
    fn station_routes_voice_and_ptt_to_the_tx_radio() {
        let a = settings("station");
        let tx2 = plan(&a, Some(So2rState { tx: Radio::Two, rx: Listen::Both }), true);
        assert_eq!((tx2.mute_a, tx2.mute_b, tx2.key), (true, false, 2));
        let tx1 = plan(&a, Some(So2rState { tx: Radio::One, rx: Listen::Both }), true);
        assert_eq!((tx1.mute_a, tx1.mute_b, tx1.key), (false, true, 1));
        let one_card = AudioSettings { so2r_single_device: true, ..a };
        let p = plan(&one_card, Some(So2rState { tx: Radio::Two, rx: Listen::Both }), false);
        assert_eq!((p.mode_a, p.key), (StereoMode::ToRight, 2));
    }

    #[test]
    fn disabled_is_plain_single_radio() {
        let a = AudioSettings { mono_output: true, ..AudioSettings::default() };
        let p = plan(&a, Some(So2rState { tx: Radio::Two, rx: Listen::Single(Radio::Two) }), true);
        assert_eq!(p, Plan { mode_a: StereoMode::Mono, mute_a: false, mute_b: false, key: 1 });
    }

    #[test]
    fn source_precedence_local_remote_manual() {
        let so2r = So2r::default();
        let mut a = settings("operator");
        a.so2r_rx = "both".into();
        assert_eq!(so2r.effective(&a, true).1, Source::Manual, "OTRSP off means manual");
        a.otrsp_enabled = true;
        assert_eq!(so2r.effective(&a, false).1, Source::Manual, "nothing announced yet");
        so2r.remote_update(So2rState { tx: Radio::Two, rx: Listen::Single(Radio::Two) });
        assert_eq!(so2r.effective(&a, false), (So2rState { tx: Radio::Two, rx: Listen::Single(Radio::Two) }, Source::Remote));
        let _ = so2r.otrsp(&OtrspCommand::Rx(Listen::Both));
        assert_eq!(so2r.effective(&a, true).1, Source::Local, "a local logger wins");
    }
}
