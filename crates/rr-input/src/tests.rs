//! Tests for `rr-input`.
//!
//! These run headless: every test drives [`ControlGate`] through a
//! [`RecordingInjector`], so the dangerous part — *did an input actually
//! happen, and only for an authorised peer* — is directly assertable with no
//! display, no input system, and no risk of moving the developer's pointer.
//!
//! The emphasis is deliberate. The valuable property of this crate is not that
//! a keystroke reaches the OS (that is `enigo`'s job, tested by `enigo`), but
//! that the gate's structure makes an unauthorised keystroke *unreachable*. So
//! most of these assert on `RecordingInjector::is_empty()` and on the refusal
//! counters, not on injected output.

#![allow(
    clippy::unwrap_used,
    reason = "unwrap is acceptable in test code"
)]
#![allow(
    clippy::expect_used,
    reason = "expect is acceptable in test code"
)]

use rr_proto::{ControlMsg, InputEvent};
use rr_session::Effect;

use crate::{
    ControlGate, KeyCode, MonitorLayout, MonitorPlacement, RecordedAction, RecordingInjector,
    Refusal,
};

// A disabled handoff timeout keeps these tests about authorisation, not timing;
// the timeout path belongs to rr-session's own suite.
const NO_TIMEOUT: u64 = 0;

/// A single-monitor layout at the origin, so monitor-relative coordinates equal
/// absolute ones and a test can read injected positions directly.
fn single_monitor() -> MonitorLayout {
    MonitorLayout::single(1, 1920, 1080)
}

/// Build a gate that records instead of injecting, with one shared monitor.
fn gate() -> ControlGate<RecordingInjector> {
    let mut g = ControlGate::new(RecordingInjector::new(), single_monitor(), NO_TIMEOUT);
    g.set_shared_monitor(Some(1));
    g
}

fn mouse_move(monitor: u32, x: i32, y: i32) -> ControlMsg {
    ControlMsg::Input(InputEvent::MouseMove { monitor, x, y })
}

// ---------------------------------------------------------------------------
//  The core guarantee: no injection without control
// ---------------------------------------------------------------------------

/// Input from a session that never requested control is refused and counted,
/// and — the point of the whole crate — **nothing is injected**.
#[test]
fn input_from_a_non_controller_injects_nothing() {
    let mut g = gate();
    // Session 7 never took control.
    let outcome = g.handle(7, &mouse_move(1, 100, 200)).unwrap();

    assert!(!outcome.injected);
    assert_eq!(outcome.refused, Some(Refusal::NotController));
    assert_eq!(g.refusals().not_controller, 1);
    // The assertion that matters: nothing reached the injector at all.
    assert!(g.injector_actions().is_empty());
}

/// The holder's input is injected; a *different* session's input in the same
/// breath is not. Checks the gate re-evaluates control per event rather than
/// trusting a once-authorised peer.
#[test]
fn only_the_holder_drives_the_pointer() {
    let mut g = gate();
    // Session 5 requests control of a free token: auto-granted.
    let _ = g.handle(5, &ControlMsg::RequestControl).unwrap();
    assert_eq!(g.controller(), Some(5));

    // Holder moves the pointer: injected.
    let ok = g.handle(5, &mouse_move(1, 10, 20)).unwrap();
    assert!(ok.injected);
    assert_eq!(ok.refused, None);

    // A different session tries in the same session: refused.
    let bad = g.handle(9, &mouse_move(1, 30, 40)).unwrap();
    assert!(!bad.injected);
    assert_eq!(bad.refused, Some(Refusal::NotController));
}

/// Control genuinely changes hands, and the gate follows it: the old holder is
/// refused the instant control moves, without any per-session cache to go stale.
#[test]
fn control_handoff_moves_who_may_inject() {
    let mut g = gate();
    let _ = g.handle(1, &ControlMsg::RequestControl).unwrap(); // 1 holds
    let _ = g.handle(2, &ControlMsg::RequestControl).unwrap(); // 2 pending
    let _ = g.handle(1, &ControlMsg::ApproveHandoff { to: 2 }).unwrap();
    assert_eq!(g.controller(), Some(2));

    // The former holder can no longer inject.
    let old = g.handle(1, &mouse_move(1, 1, 1)).unwrap();
    assert!(!old.injected, "the former holder must be refused after handoff");

    // The new holder can.
    let new = g.handle(2, &mouse_move(1, 2, 2)).unwrap();
    assert!(new.injected);
}

// ---------------------------------------------------------------------------
//  Refuse-rather-than-guess, and clamp-rather-than-refuse
// ---------------------------------------------------------------------------

/// Input aimed at a monitor that is not being shared is refused: the controller
/// cannot see it, so acting on it would be acting blind.
#[test]
fn input_to_an_unshared_monitor_is_refused() {
    let mut g = gate(); // shared monitor = 1
    let _ = g.handle(3, &ControlMsg::RequestControl).unwrap();
    let outcome = g.handle(3, &mouse_move(2, 10, 10)).unwrap();
    assert!(!outcome.injected);
    assert_eq!(outcome.refused, Some(Refusal::UnknownMonitor(2)));
    assert_eq!(g.refusals().unknown_monitor, 1);
}

/// An unmapped key code is refused rather than guessed — a wrong guess types
/// something the controller never pressed.
#[test]
fn an_unmapped_key_is_refused() {
    let mut g = gate();
    let _ = g.handle(3, &ControlMsg::RequestControl).unwrap();
    // 999_999 is not in the KeyCode table.
    let outcome = g
        .handle(3, &ControlMsg::Input(InputEvent::Key { code: 999_999, down: true }))
        .unwrap();
    assert!(!outcome.injected);
    assert_eq!(outcome.refused, Some(Refusal::UnknownKey(999_999)));
    assert_eq!(g.refusals().unknown_key, 1);
}

/// A pointer past the monitor edge is **clamped**, not refused: dropping it
/// would make the remote pointer appear to stick at the border.
#[test]
fn out_of_range_coordinates_are_clamped_not_refused() {
    let injector = RecordingInjector::new();
    let mut g = ControlGate::new(injector, MonitorLayout::single(1, 1920, 1080), NO_TIMEOUT);
    g.set_shared_monitor(Some(1));
    let _ = g.handle(1, &ControlMsg::RequestControl).unwrap();

    // Way past the bottom-right corner.
    let outcome = g.handle(1, &mouse_move(1, 5000, 5000)).unwrap();
    assert!(outcome.injected, "an edge pointer is clamped and still injected");
    assert_eq!(outcome.refused, None);

    let actions = g.injector_actions();
    // Clamped to the last addressable pixel: 1919 x 1079.
    assert_eq!(actions, &[RecordedAction::MouseMove { x: 1919, y: 1079 }]);
}

// ---------------------------------------------------------------------------
//  Modifier release at every change of control
// ---------------------------------------------------------------------------

/// When the holder releases control, held modifiers are released on the
/// Station — a controller who lets go mid-chord must not leave Shift down for
/// whoever controls next.
#[test]
fn releasing_control_releases_held_modifiers() {
    let mut g = gate();
    let _ = g.handle(1, &ControlMsg::RequestControl).unwrap();
    let _ = g.handle(1, &mouse_move(1, 1, 1)).unwrap(); // some real input first
    g.clear_injector();

    let _ = g.handle(1, &ControlMsg::ReleaseControl).unwrap();
    assert!(
        g.injector_actions().contains(&RecordedAction::ReleaseAll),
        "release of control must flush held keys"
    );
}

/// A controller that vanishes mid-chord also triggers a modifier release.
#[test]
fn disconnect_of_the_holder_releases_modifiers() {
    let mut g = gate();
    let _ = g.handle(1, &ControlMsg::RequestControl).unwrap();
    g.clear_injector();

    let _ = g.disconnect(1);
    assert!(g.injector_actions().contains(&RecordedAction::ReleaseAll));
}

/// A non-holder disconnecting must NOT flush modifiers — the real controller is
/// still typing, and an unexpected release would corrupt their input.
#[test]
fn disconnect_of_a_non_holder_does_not_release_modifiers() {
    let mut g = gate();
    let _ = g.handle(1, &ControlMsg::RequestControl).unwrap(); // 1 holds
    g.clear_injector();

    let _ = g.disconnect(2); // some other session leaves
    assert!(
        g.injector_actions().is_empty(),
        "a non-holder leaving must not touch the holder's input"
    );
}

// ---------------------------------------------------------------------------
//  Ungated requests, and keyframe/monitor control
// ---------------------------------------------------------------------------

/// Any viewer may ask for a keyframe, controller or not — a passive viewer needs
/// it to see anything, so gating it would make joining impossible.
#[test]
fn keyframe_request_is_ungated() {
    let mut g = gate();
    let outcome = g.handle(42, &ControlMsg::RequestKeyframe).unwrap();
    assert!(outcome.keyframe_requested);
    assert!(!outcome.injected);
    assert_eq!(outcome.refused, None);
}

/// Switching the shared monitor is a controller action; a non-controller is
/// refused.
#[test]
fn select_monitor_requires_control() {
    let mut g = gate();
    let refused = g.handle(8, &ControlMsg::SelectMonitor { monitor: 2 }).unwrap();
    assert_eq!(refused.monitor_selected, None);
    assert_eq!(refused.refused, Some(Refusal::NotController));

    let _ = g.handle(8, &ControlMsg::RequestControl).unwrap();
    let ok = g.handle(8, &ControlMsg::SelectMonitor { monitor: 2 }).unwrap();
    assert_eq!(ok.monitor_selected, Some(2));
}

/// The gate surfaces the arbiter's broadcast effects so the caller can relay
/// control state to viewers.
#[test]
fn request_control_surfaces_a_broadcast_effect() {
    let mut g = gate();
    let outcome = g.handle(1, &ControlMsg::RequestControl).unwrap();
    assert!(
        outcome
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Broadcast(_))),
        "a control grant must broadcast the new state"
    );
}

// ---------------------------------------------------------------------------
//  A full keyboard chord, end to end
// ---------------------------------------------------------------------------

/// A realistic Ctrl+C: modifier down, letter down, letter up, modifier up —
/// each a separate wire event, each injected in order for the holder.
#[test]
fn a_modifier_chord_injects_each_event_in_order() {
    let mut g = gate();
    let _ = g.handle(1, &ControlMsg::RequestControl).unwrap();
    g.clear_injector();

    let down = |code: u32| ControlMsg::Input(InputEvent::Key { code, down: true });
    let up = |code: u32| ControlMsg::Input(InputEvent::Key { code, down: false });

    let _ = g.handle(1, &down(KeyCode::ControlLeft.as_u32())).unwrap();
    let _ = g.handle(1, &down(KeyCode::C.as_u32())).unwrap();
    let _ = g.handle(1, &up(KeyCode::C.as_u32())).unwrap();
    let _ = g.handle(1, &up(KeyCode::ControlLeft.as_u32())).unwrap();

    assert_eq!(
        g.injector_actions(),
        &[
            RecordedAction::Key { key: KeyCode::ControlLeft, down: true },
            RecordedAction::Key { key: KeyCode::C, down: true },
            RecordedAction::Key { key: KeyCode::C, down: false },
            RecordedAction::Key { key: KeyCode::ControlLeft, down: false },
        ]
    );
}

// ---------------------------------------------------------------------------
//  KeyCode table integrity (wire-format stability)
// ---------------------------------------------------------------------------

/// Every key in `ALL` round-trips through its wire value, and `ALL` matches the
/// enum: a key added to one but not the other fails here.
#[test]
fn every_keycode_round_trips_through_its_wire_value() {
    for &k in KeyCode::ALL {
        assert_eq!(KeyCode::from_u32(k.as_u32()), Some(k), "u32 round trip for {k}");
    }
    // Discriminants must be unique.
    let mut seen = std::collections::HashSet::new();
    for &k in KeyCode::ALL {
        assert!(seen.insert(k.as_u32()), "duplicate discriminant {}", k.as_u32());
    }
}

/// Every key round-trips through its DOM code too — the Client's edge of the
/// translation.
#[test]
fn every_keycode_round_trips_through_its_dom_code() {
    for &k in KeyCode::ALL {
        assert_eq!(KeyCode::from_dom_code(k.dom_code()), Some(k), "dom round trip for {k}");
    }
}

/// Pinned discriminants: these are wire format and must never be renumbered.
/// Changing any of them breaks interoperability with a peer on the old numbers,
/// so this test is a deliberate tripwire.
#[test]
fn wire_discriminants_are_pinned() {
    assert_eq!(KeyCode::A.as_u32(), 100);
    assert_eq!(KeyCode::Digit0.as_u32(), 200);
    assert_eq!(KeyCode::Enter.as_u32(), 300);
    assert_eq!(KeyCode::ArrowLeft.as_u32(), 400);
    assert_eq!(KeyCode::ShiftLeft.as_u32(), 500);
    assert_eq!(KeyCode::F1.as_u32(), 600);
    assert_eq!(KeyCode::Minus.as_u32(), 700);
}

/// Unknown values and codes return `None`, so the gate refuses them.
#[test]
fn unknown_keys_do_not_map() {
    assert_eq!(KeyCode::from_u32(0), None);
    assert_eq!(KeyCode::from_u32(999_999), None);
    assert_eq!(KeyCode::from_dom_code("MediaPlayPause"), None);
    assert_eq!(KeyCode::from_dom_code(""), None);
}

/// Exactly the eight modifiers report as modifiers; nothing else does.
#[test]
fn only_modifiers_are_modifiers() {
    let modifiers: Vec<_> = KeyCode::ALL.iter().copied().filter(|k| k.is_modifier()).collect();
    assert_eq!(modifiers.len(), 8);
    assert!(KeyCode::ControlLeft.is_modifier());
    assert!(KeyCode::MetaRight.is_modifier());
    assert!(!KeyCode::A.is_modifier());
    assert!(!KeyCode::CapsLock.is_modifier(), "CapsLock is not a held modifier here");
    assert!(!KeyCode::Enter.is_modifier());
}

// ---------------------------------------------------------------------------
//  MonitorLayout coordinate mapping
// ---------------------------------------------------------------------------

/// A monitor at the origin maps relative to absolute 1:1 within bounds.
#[test]
fn single_monitor_maps_identically_within_bounds() {
    let layout = MonitorLayout::single(1, 1920, 1080);
    assert_eq!(layout.to_absolute(1, 0, 0), Some((0, 0)));
    assert_eq!(layout.to_absolute(1, 100, 200), Some((100, 200)));
    assert_eq!(layout.to_absolute(1, 1919, 1079), Some((1919, 1079)));
}

/// Out-of-range clamps to the last addressable pixel, never one past the edge.
#[test]
fn mapping_clamps_to_the_monitor_rectangle() {
    let layout = MonitorLayout::single(1, 1920, 1080);
    assert_eq!(layout.to_absolute(1, -50, -50), Some((0, 0)));
    assert_eq!(layout.to_absolute(1, 9999, 9999), Some((1919, 1079)));
}

/// An unknown monitor id maps to `None` — the event is refused, not guessed onto
/// some default display.
#[test]
fn unknown_monitor_maps_to_none() {
    let layout = MonitorLayout::single(1, 1920, 1080);
    assert_eq!(layout.to_absolute(99, 10, 10), None);
}

/// A monitor placed at a negative virtual-desktop origin — the real
/// dual-display case the monitor-origin work exists for — offsets correctly.
/// This is the scenario the old left-to-right guess got wrong.
#[test]
fn negative_origin_monitor_offsets_correctly() {
    // A 4K panel above and to the left of the primary, as measured on real
    // hardware: (-953, -2160).
    let layout = MonitorLayout::new(vec![
        MonitorPlacement { id: 1, x: 0, y: 0, width: 1920, height: 1200 },
        MonitorPlacement { id: 2, x: -953, y: -2160, width: 3840, height: 2160 },
    ]);
    // Top-left of monitor 2 maps to its origin.
    assert_eq!(layout.to_absolute(2, 0, 0), Some((-953, -2160)));
    // A point inside monitor 2 offsets from that origin.
    assert_eq!(layout.to_absolute(2, 100, 50), Some((-853, -2110)));
    // Monitor 1 is unaffected.
    assert_eq!(layout.to_absolute(1, 10, 10), Some((10, 10)));
}

// ---------------------------------------------------------------------------
//  Injector error handling
// ---------------------------------------------------------------------------

/// If the injector itself fails, `handle` returns an error rather than silently
/// claiming success — the one case that is a genuine fault, not a refusal.
#[test]
fn an_injector_failure_surfaces_as_an_error() {
    let injector = RecordingInjector::failing("simulated input-system failure");
    let mut g = ControlGate::new(injector, MonitorLayout::single(1, 1920, 1080), NO_TIMEOUT);
    g.set_shared_monitor(Some(1));
    let _ = g.handle(1, &ControlMsg::RequestControl).unwrap();

    let result = g.handle(1, &mouse_move(1, 10, 10));
    assert!(result.is_err(), "injection failure must surface as Err, not a false success");
}
