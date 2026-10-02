//! Integration test for the Station-side control fabric.
//!
//! [`StationControl`] is where one arbitration gate meets many viewers, and the
//! property that matters is a security property: **only the viewer that holds
//! control can drive the desktop**, and every viewer is told, authoritatively,
//! who that is. This test drives the public control path — the same
//! [`SessionObserver::on_control`] a live `PeerSession` calls — and asserts both
//! halves: what reaches the injector, and what reaches each viewer's sink.
//!
//! The gate keeps its injector private on purpose, so the test cannot peek at
//! it through the gate. Instead it hands the gate an injector that writes every
//! recorded action into a vector the test also holds a handle to. That proves
//! the one thing worth proving — that nothing was injected for an unauthorised
//! peer — without widening the production surface.

#![allow(clippy::unwrap_used, reason = "unwrap is acceptable in test code")]
#![allow(clippy::expect_used, reason = "expect is acceptable in test code")]
#![allow(
    unused_crate_dependencies,
    reason = "integration test uses the library's public API, not all of its deps directly"
)]

use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use tokio::sync::Mutex as TokioMutex;

use rr_input::{
    ControlGate, InputError, InputInjector, KeyCode, MonitorLayout, RecordedAction,
};
use rr_proto::{ControlMsg, InputEvent, MouseButton};
use rr_webrtc::{ControlSink, SessionObserver, StationControl, StationEvent};

/// Session ids for the two simulated viewers.
const VIEWER_A: u32 = 11;
const VIEWER_B: u32 = 22;

/// An injector that writes every call into a shared vector, so a test holding a
/// clone of that vector can assert exactly what was injected. The gate owns the
/// injector privately; this is how the test sees through it without the gate
/// exposing anything in production.
#[derive(Clone)]
struct SharedRecordingInjector {
    actions: Arc<StdMutex<Vec<RecordedAction>>>,
}

impl SharedRecordingInjector {
    fn new() -> (Self, Arc<StdMutex<Vec<RecordedAction>>>) {
        let actions = Arc::new(StdMutex::new(Vec::new()));
        (
            Self {
                actions: Arc::clone(&actions),
            },
            actions,
        )
    }

    fn push(&mut self, action: RecordedAction) -> Result<(), InputError> {
        self.actions.lock().unwrap().push(action);
        Ok(())
    }
}

impl InputInjector for SharedRecordingInjector {
    fn mouse_move_abs(&mut self, x: i32, y: i32) -> Result<(), InputError> {
        self.push(RecordedAction::MouseMove { x, y })
    }
    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<(), InputError> {
        self.push(RecordedAction::MouseButton { button, down })
    }
    fn wheel(&mut self, dx: i32, dy: i32) -> Result<(), InputError> {
        self.push(RecordedAction::Wheel { dx, dy })
    }
    fn key(&mut self, key: KeyCode, down: bool) -> Result<(), InputError> {
        self.push(RecordedAction::Key { key, down })
    }
    fn text(&mut self, text: &str) -> Result<(), InputError> {
        self.push(RecordedAction::Text {
            text: text.to_owned(),
        })
    }
    fn release_all(&mut self) -> Result<(), InputError> {
        self.push(RecordedAction::ReleaseAll)
    }
}

/// A control sink that records what the Station tried to send to one viewer.
#[derive(Clone)]
struct RecordingSink {
    sent: Arc<TokioMutex<Vec<ControlMsg>>>,
}

impl RecordingSink {
    fn new() -> (Arc<Self>, Arc<TokioMutex<Vec<ControlMsg>>>) {
        let sent = Arc::new(TokioMutex::new(Vec::new()));
        (
            Arc::new(Self {
                sent: Arc::clone(&sent),
            }),
            sent,
        )
    }
}

#[async_trait]
impl ControlSink for RecordingSink {
    async fn send(&self, msg: &ControlMsg) -> rr_webrtc::Result<()> {
        self.sent.lock().await.push(msg.clone());
        Ok(())
    }
}

/// Build a fabric with one monitor and two registered viewers, returning the
/// pieces a test asserts against.
async fn fixture() -> Fixture {
    let (injector, injected) = SharedRecordingInjector::new();
    // A single 1920x1080 monitor with id 0, placed at the origin.
    let gate = ControlGate::new(injector, MonitorLayout::single(0, 1920, 1080), 0);
    let (control, events) = StationControl::new(gate);
    // The shared monitor must be set, or input to monitor 0 is refused as
    // "aimed at a monitor that is not being shared".
    control.set_shared_monitor(Some(0)).await;

    let (sink_a, sent_a) = RecordingSink::new();
    let (sink_b, sent_b) = RecordingSink::new();
    let obs_a = control.register(VIEWER_A, sink_a as _).await;
    let obs_b = control.register(VIEWER_B, sink_b as _).await;

    Fixture {
        control,
        events,
        injected,
        sent_a,
        sent_b,
        obs_a,
        obs_b,
    }
}

struct Fixture {
    control: StationControl<SharedRecordingInjector>,
    events: tokio::sync::mpsc::UnboundedReceiver<StationEvent>,
    injected: Arc<StdMutex<Vec<RecordedAction>>>,
    sent_a: Arc<TokioMutex<Vec<ControlMsg>>>,
    sent_b: Arc<TokioMutex<Vec<ControlMsg>>>,
    obs_a: rr_webrtc::GateObserver<SharedRecordingInjector>,
    obs_b: rr_webrtc::GateObserver<SharedRecordingInjector>,
}

fn injected_snapshot(f: &Fixture) -> Vec<RecordedAction> {
    f.injected.lock().unwrap().clone()
}

/// The core property: a viewer that holds control drives the desktop, a viewer
/// that does not is silently refused, and every viewer is told who holds it.
#[tokio::test]
async fn controller_drives_station_and_non_controller_is_refused() {
    let f = fixture().await;

    // A asks for control. Nobody holds it, so the arbiter auto-grants, and the
    // authoritative state must reach *both* viewers.
    f.obs_a.on_control(ControlMsg::RequestControl).await;

    assert_eq!(f.control.controller().await, Some(VIEWER_A));
    let a_state = ControlMsg::ControlState {
        controller: Some(VIEWER_A),
        pending: None,
    };
    assert_eq!(
        f.sent_a.lock().await.as_slice(),
        std::slice::from_ref(&a_state),
        "A must learn it holds control"
    );
    assert_eq!(
        f.sent_b.lock().await.as_slice(),
        std::slice::from_ref(&a_state),
        "B must learn A holds control, even though B did not ask"
    );

    // A, as the holder, moves the pointer and presses a key: both are injected.
    f.obs_a
        .on_control(ControlMsg::Input(InputEvent::MouseMove {
            monitor: 0,
            x: 100,
            y: 50,
        }))
        .await;
    f.obs_a
        .on_control(ControlMsg::Input(InputEvent::Key {
            code: KeyCode::A.as_u32(),
            down: true,
        }))
        .await;

    assert_eq!(
        injected_snapshot(&f),
        vec![
            RecordedAction::MouseMove { x: 100, y: 50 },
            RecordedAction::Key {
                key: KeyCode::A,
                down: true,
            },
        ],
        "the controller's input must reach the injector unchanged"
    );

    // B is NOT the controller. Its input must be dropped — nothing new reaches
    // the injector.
    f.obs_b
        .on_control(ControlMsg::Input(InputEvent::MouseButton {
            button: MouseButton::Left,
            down: true,
        }))
        .await;

    assert_eq!(
        injected_snapshot(&f).len(),
        2,
        "a non-controller's input must never be injected"
    );
}

/// A second viewer asking for control while A holds it starts a cooperative
/// handoff: the broadcast now names B as pending, and A does not lose control.
#[tokio::test]
async fn second_requester_becomes_pending_without_displacing_holder() {
    let f = fixture().await;

    f.obs_a.on_control(ControlMsg::RequestControl).await;
    // Clear the auto-grant broadcasts so the next assertion is isolated.
    f.sent_a.lock().await.clear();
    f.sent_b.lock().await.clear();

    f.obs_b.on_control(ControlMsg::RequestControl).await;

    // A still holds control; B is pending.
    assert_eq!(f.control.controller().await, Some(VIEWER_A));
    let pending_state = ControlMsg::ControlState {
        controller: Some(VIEWER_A),
        pending: Some(VIEWER_B),
    };
    // The authoritative "A holds, B pending" state reaches both viewers.
    assert!(
        f.sent_a.lock().await.contains(&pending_state),
        "A must be told B is now pending"
    );
    assert!(
        f.sent_b.lock().await.contains(&pending_state),
        "B must be told its request is pending, not granted"
    );

    // Now A approves the handoff to B. Control moves; both learn B holds it.
    f.sent_a.lock().await.clear();
    f.sent_b.lock().await.clear();
    f.obs_a
        .on_control(ControlMsg::ApproveHandoff { to: VIEWER_B })
        .await;

    assert_eq!(f.control.controller().await, Some(VIEWER_B));
    let b_state = ControlMsg::ControlState {
        controller: Some(VIEWER_B),
        pending: None,
    };
    assert!(f.sent_a.lock().await.contains(&b_state));
    assert!(f.sent_b.lock().await.contains(&b_state));

    // The outgoing holder's keys are released at handoff, so a chord held
    // across the change does not stick. That release reaches the injector.
    assert!(
        injected_snapshot(&f).contains(&RecordedAction::ReleaseAll),
        "held keys must be released when control changes hands"
    );
}

/// A keyframe request is ungated — any viewer may ask — and surfaces as an event
/// for the capture layer. A monitor switch is gated to the controller.
#[tokio::test]
async fn keyframe_is_open_and_monitor_switch_is_gated() {
    let mut f = fixture().await;

    // B, with no control, may still ask for a keyframe (a late joiner needs one).
    f.obs_b.on_control(ControlMsg::RequestKeyframe).await;
    match f.events.try_recv() {
        Ok(StationEvent::KeyframeRequested { session }) => assert_eq!(session, VIEWER_B),
        other => panic!("expected a keyframe event from B, got {other:?}"),
    }

    // A monitor switch from a non-controller is refused: no event emitted.
    f.obs_b
        .on_control(ControlMsg::SelectMonitor { monitor: 1 })
        .await;
    assert!(
        f.events.try_recv().is_err(),
        "a non-controller must not be able to switch the shared monitor"
    );

    // Give A control, then A switches the monitor: that event does surface.
    f.obs_a.on_control(ControlMsg::RequestControl).await;
    f.obs_a
        .on_control(ControlMsg::SelectMonitor { monitor: 1 })
        .await;
    match f.events.try_recv() {
        Ok(StationEvent::MonitorSelected { monitor }) => assert_eq!(monitor, 1),
        other => panic!("expected a monitor-selected event from A, got {other:?}"),
    }
}

/// When the controller disconnects, the token frees and every remaining viewer
/// is told, so nobody is left believing a vanished peer still drives the desktop.
#[tokio::test]
async fn disconnect_of_controller_frees_the_token_and_broadcasts() {
    let f = fixture().await;

    f.obs_a.on_control(ControlMsg::RequestControl).await;
    assert_eq!(f.control.controller().await, Some(VIEWER_A));
    f.sent_b.lock().await.clear();

    // A's leg fails; the fabric must release its control and tell B.
    f.control.unregister(VIEWER_A).await;

    assert_eq!(f.control.controller().await, None);
    let freed = ControlMsg::ControlState {
        controller: None,
        pending: None,
    };
    assert!(
        f.sent_b.lock().await.contains(&freed),
        "B must learn control is free after A vanished"
    );

    // A disconnecting controller holding keys must have them released.
    assert!(
        injected_snapshot(&f).contains(&RecordedAction::ReleaseAll),
        "a vanishing controller's held keys must be released"
    );
}
