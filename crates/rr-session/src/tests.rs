//! Exhaustive tests for the cooperative control-arbitration state machine.

// Test setup frequently drives the machine purely for its state transitions
// and asserts on `holder()`/`pending()` afterwards rather than the returned
// effects; ignoring those `Vec<Effect>` results is intentional here.
#![allow(unused_results, reason = "tests drive transitions and assert on state, not every returned effect")]

use super::*;

fn broadcast(controller: Option<SessionId>, pending: Option<SessionId>) -> Effect {
    Effect::Broadcast(ControlState {
        controller,
        pending,
    })
}

#[test]
fn starts_free() {
    let a = Arbiter::new(0);
    assert_eq!(a.holder(), Holder::Free);
    assert_eq!(a.controller(), None);
    assert_eq!(a.pending(), None);
    assert!(!a.may_control(1));
}

#[test]
fn first_request_is_auto_granted() {
    let mut a = Arbiter::new(0);
    let fx = a.apply(Input::Request { who: 1 });
    assert_eq!(a.holder(), Holder::Held(1));
    assert_eq!(fx, vec![broadcast(Some(1), None)]);
    assert!(a.may_control(1));
    assert!(!a.may_control(2));
}

#[test]
fn holder_rerequest_is_noop() {
    let mut a = Arbiter::new(0);
    a.apply(Input::Request { who: 1 });
    let fx = a.apply(Input::Request { who: 1 });
    assert!(fx.is_empty());
    assert_eq!(a.holder(), Holder::Held(1));
}

#[test]
fn second_request_starts_cooperative_handoff() {
    let mut a = Arbiter::new(0);
    a.apply(Input::Request { who: 1 });
    let fx = a.apply(Input::Request { who: 2 });
    // Holder still holds; requester is pending; holder is notified.
    assert_eq!(a.holder(), Holder::Held(1));
    assert_eq!(a.pending(), Some(2));
    assert!(fx.contains(&Effect::NotifyControlRequested { requester: 2 }));
    assert!(fx.contains(&broadcast(Some(1), Some(2))));
}

#[test]
fn approve_transfers_control() {
    let mut a = Arbiter::new(0);
    a.apply(Input::Request { who: 1 });
    a.apply(Input::Request { who: 2 });
    let fx = a.apply(Input::Approve { to: 2 });
    assert_eq!(a.holder(), Holder::Held(2));
    assert_eq!(a.pending(), None);
    assert_eq!(fx, vec![broadcast(Some(2), None)]);
    assert!(a.may_control(2));
    assert!(!a.may_control(1));
}

#[test]
fn approve_wrong_session_is_ignored() {
    let mut a = Arbiter::new(0);
    a.apply(Input::Request { who: 1 });
    a.apply(Input::Request { who: 2 });
    // Approving someone who isn't pending does nothing.
    let fx = a.apply(Input::Approve { to: 99 });
    assert!(fx.is_empty());
    assert_eq!(a.holder(), Holder::Held(1));
    assert_eq!(a.pending(), Some(2));
}

#[test]
fn deny_keeps_holder_and_rejects_requester() {
    let mut a = Arbiter::new(0);
    a.apply(Input::Request { who: 1 });
    a.apply(Input::Request { who: 2 });
    let fx = a.apply(Input::Deny { to: 2 });
    assert_eq!(a.holder(), Holder::Held(1));
    assert_eq!(a.pending(), None);
    assert!(fx.contains(&Effect::NotifyRequestRejected {
        requester: 2,
        reason: RejectReason::Denied,
    }));
}

#[test]
fn release_frees_token_when_no_waiters() {
    let mut a = Arbiter::new(0);
    a.apply(Input::Request { who: 1 });
    let fx = a.apply(Input::Release);
    assert_eq!(a.holder(), Holder::Free);
    assert_eq!(fx, vec![broadcast(None, None)]);
}

#[test]
fn release_grants_pending_requester() {
    let mut a = Arbiter::new(0);
    a.apply(Input::Request { who: 1 });
    a.apply(Input::Request { who: 2 }); // 2 pending
    let fx = a.apply(Input::Release); // 1 releases -> 2 gets it
    assert_eq!(a.holder(), Holder::Held(2));
    assert_eq!(a.pending(), None);
    assert_eq!(fx, vec![broadcast(Some(2), None)]);
}

#[test]
fn third_request_is_queued_behind_pending() {
    let mut a = Arbiter::new(0);
    a.apply(Input::Request { who: 1 }); // holder
    a.apply(Input::Request { who: 2 }); // pending
    let fx = a.apply(Input::Request { who: 3 }); // queued
    assert!(fx.is_empty(), "queued request should emit no effect");
    assert_eq!(a.pending(), Some(2));
    // Deny 2 -> 3 should be promoted to pending (holder asked again).
    let fx = a.apply(Input::Deny { to: 2 });
    assert_eq!(a.pending(), Some(3));
    assert!(fx.contains(&Effect::NotifyControlRequested { requester: 3 }));
    assert!(fx.contains(&Effect::NotifyRequestRejected {
        requester: 2,
        reason: RejectReason::Denied,
    }));
}

#[test]
fn holder_disconnect_frees_and_grants_pending() {
    let mut a = Arbiter::new(0);
    a.apply(Input::Request { who: 1 });
    a.apply(Input::Request { who: 2 }); // 2 pending
    let fx = a.apply(Input::Disconnect { who: 1 });
    assert_eq!(a.holder(), Holder::Held(2));
    assert_eq!(fx, vec![broadcast(Some(2), None)]);
}

#[test]
fn holder_disconnect_with_no_waiters_frees() {
    let mut a = Arbiter::new(0);
    a.apply(Input::Request { who: 1 });
    let fx = a.apply(Input::Disconnect { who: 1 });
    assert_eq!(a.holder(), Holder::Free);
    assert_eq!(fx, vec![broadcast(None, None)]);
}

#[test]
fn pending_requester_disconnect_promotes_queue() {
    let mut a = Arbiter::new(0);
    a.apply(Input::Request { who: 1 }); // holder
    a.apply(Input::Request { who: 2 }); // pending
    a.apply(Input::Request { who: 3 }); // queued
    let fx = a.apply(Input::Disconnect { who: 2 });
    assert_eq!(a.holder(), Holder::Held(1), "holder unaffected");
    assert_eq!(a.pending(), Some(3), "queue promoted to pending");
    assert!(fx.contains(&Effect::NotifyControlRequested { requester: 3 }));
}

#[test]
fn queued_requester_disconnect_is_removed() {
    let mut a = Arbiter::new(0);
    a.apply(Input::Request { who: 1 });
    a.apply(Input::Request { who: 2 });
    a.apply(Input::Request { who: 3 }); // queued
    a.apply(Input::Disconnect { who: 3 });
    // Deny 2 -> nobody to promote, holder stays, pending clears.
    let fx = a.apply(Input::Deny { to: 2 });
    assert_eq!(a.holder(), Holder::Held(1));
    assert_eq!(a.pending(), None);
    assert!(fx.contains(&Effect::NotifyRequestRejected {
        requester: 2,
        reason: RejectReason::Denied,
    }));
}

#[test]
fn disconnect_of_unrelated_session_is_noop() {
    let mut a = Arbiter::new(0);
    a.apply(Input::Request { who: 1 });
    let fx = a.apply(Input::Disconnect { who: 42 });
    assert!(fx.is_empty());
    assert_eq!(a.holder(), Holder::Held(1));
}

#[test]
fn pending_request_times_out_and_holder_stays() {
    let mut a = Arbiter::new(100); // timeout = 100 ticks
    a.apply(Input::Request { who: 1 });
    // Use explicit-time request so the deadline is now(10) + 100 = 110.
    let fx = a.request_at(2, 10);
    assert!(fx.contains(&Effect::NotifyControlRequested { requester: 2 }));
    assert_eq!(a.pending(), Some(2));

    // Not yet expired.
    assert!(a.apply(Input::Tick { now: 109 }).is_empty());
    assert_eq!(a.pending(), Some(2));

    // Expired.
    let fx = a.apply(Input::Tick { now: 110 });
    assert_eq!(a.holder(), Holder::Held(1), "holder retained on timeout");
    assert_eq!(a.pending(), None);
    assert!(fx.contains(&Effect::NotifyRequestRejected {
        requester: 2,
        reason: RejectReason::TimedOut,
    }));
}

#[test]
fn timeout_zero_disables_expiry() {
    let mut a = Arbiter::new(0);
    a.apply(Input::Request { who: 1 });
    a.request_at(2, 10);
    let fx = a.apply(Input::Tick { now: u64::MAX });
    assert!(fx.is_empty());
    assert_eq!(a.pending(), Some(2));
}

#[test]
fn may_control_gate_is_exclusive() {
    let mut a = Arbiter::new(0);
    a.apply(Input::Request { who: 1 });
    a.apply(Input::Request { who: 2 }); // pending, not controlling
    assert!(a.may_control(1));
    assert!(!a.may_control(2));
    a.apply(Input::Approve { to: 2 });
    assert!(!a.may_control(1));
    assert!(a.may_control(2));
}

#[test]
fn release_when_free_is_noop() {
    let mut a = Arbiter::new(0);
    let fx = a.apply(Input::Release);
    assert!(fx.is_empty());
    assert_eq!(a.holder(), Holder::Free);
}
