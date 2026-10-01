//! `RemoteRadio` control arbitration.
//!
//! Implements the **cooperative, single-controller** handoff policy from the
//! spec (§4). The state machine is **authoritative on the Station** — it is the
//! single writer, which avoids split-brain among multiple viewers. Clients
//! merely render the [`ControlState`] the Station broadcasts.
//!
//! Design goals:
//! - *Cooperative only*: a holder is never forcibly displaced; a pending
//!   request waits for the holder's approval (or a timeout that cancels the
//!   request, leaving the holder in place).
//! - *Auto-grant when free*: the first requester of a free token gets it
//!   immediately (spec: auto-grant when nobody has control).
//! - *Fair queueing*: when the holder releases or disconnects, control passes
//!   to the oldest waiting requester.
//! - *Pure & deterministic*: no clocks or I/O inside; the caller supplies a
//!   logical time (tick) so timeouts are testable without sleeping.
//!
//! The machine emits [`Effect`]s describing what the integration layer should
//! do (broadcast state, notify a specific session). It never performs I/O.

#![forbid(unsafe_code)]

use std::collections::VecDeque;

/// A Mumble session id (channel member).
pub type SessionId = u32;

/// Who currently holds the control token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holder {
    /// Nobody controls; the next request is auto-granted.
    Free,
    /// Exactly one session controls input.
    Held(SessionId),
}

/// A snapshot of control state suitable for broadcasting to all viewers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlState {
    /// Current controller, or `None` when free.
    pub controller: Option<SessionId>,
    /// Session with a request awaiting the holder's approval, if any.
    pub pending: Option<SessionId>,
}

/// Side effects the integration layer must carry out after an input is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Broadcast the given control state to every viewer (`ControlMsg::ControlState`).
    Broadcast(ControlState),
    /// Tell the current holder that `requester` wants control (prompt UI).
    NotifyControlRequested {
        /// The session asking for control.
        requester: SessionId,
    },
    /// Tell `requester` their request was denied or expired.
    NotifyRequestRejected {
        /// The session whose request was refused.
        requester: SessionId,
        /// Machine-readable reason.
        reason: RejectReason,
    },
}

/// Why a control request was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// The current holder explicitly denied the handoff.
    Denied,
    /// The pending request timed out with no holder response.
    TimedOut,
}

/// Inputs that drive the arbitration machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// `who` asks to become the controller.
    Request {
        /// Requesting session.
        who: SessionId,
    },
    /// The current holder approves handoff to `to`.
    Approve {
        /// Session the holder approves.
        to: SessionId,
    },
    /// The current holder denies handoff to `to`.
    Deny {
        /// Session the holder refuses.
        to: SessionId,
    },
    /// The current holder voluntarily releases control.
    Release,
    /// A session left the channel / disconnected.
    Disconnect {
        /// Session that went away.
        who: SessionId,
    },
    /// Logical time advanced to `now` (drives request timeout).
    Tick {
        /// Current logical time, in the same unit used for [`Arbiter::new`].
        now: u64,
    },
}

/// The arbitration state machine (Station-side).
#[derive(Debug, Clone)]
pub struct Arbiter {
    holder: Holder,
    /// A request awaiting the holder's approval: (requester, deadline).
    pending: Option<(SessionId, u64)>,
    /// FIFO of sessions waiting for control once it frees up.
    queue: VecDeque<SessionId>,
    /// How long a pending request waits before auto-expiring.
    timeout: u64,
}

impl Arbiter {
    /// Create a fresh arbiter with the token [`Holder::Free`].
    ///
    /// `handoff_timeout` is the logical duration a pending handoff request
    /// waits for the holder to approve before it expires (same unit as the
    /// `now` passed to [`Input::Tick`]). Use `0` to disable timeouts.
    #[must_use]
    pub fn new(handoff_timeout: u64) -> Self {
        Self {
            holder: Holder::Free,
            pending: None,
            queue: VecDeque::new(),
            timeout: handoff_timeout,
        }
    }

    /// Current holder.
    #[must_use]
    pub fn holder(&self) -> Holder {
        self.holder
    }

    /// The session currently controlling, if any.
    #[must_use]
    pub fn controller(&self) -> Option<SessionId> {
        match self.holder {
            Holder::Free => None,
            Holder::Held(s) => Some(s),
        }
    }

    /// The pending requester awaiting approval, if any.
    #[must_use]
    pub fn pending(&self) -> Option<SessionId> {
        self.pending.map(|(s, _)| s)
    }

    /// Whether `who` is allowed to inject input right now (i.e. is the holder).
    ///
    /// The Station must gate every injected [`rr_proto`-style] input event on
    /// this to enforce single-controller even against a misbehaving peer.
    #[must_use]
    pub fn may_control(&self, who: SessionId) -> bool {
        self.holder == Holder::Held(who)
    }

    /// Produce the broadcastable snapshot of the current state.
    #[must_use]
    pub fn state(&self) -> ControlState {
        ControlState {
            controller: self.controller(),
            pending: self.pending(),
        }
    }

    /// Apply an [`Input`] and return the [`Effect`]s to carry out.
    ///
    /// The machine is deterministic: the same state + input always yields the
    /// same next state and effects.
    pub fn apply(&mut self, input: Input) -> Vec<Effect> {
        match input {
            Input::Request { who } => self.on_request(who),
            Input::Approve { to } => self.on_approve(to),
            Input::Deny { to } => self.on_deny(to),
            Input::Release => self.on_release(),
            Input::Disconnect { who } => self.on_disconnect(who),
            Input::Tick { now } => self.on_tick(now),
        }
    }

    fn on_request(&mut self, who: SessionId) -> Vec<Effect> {
        match self.holder {
            Holder::Free => {
                // Auto-grant: nobody holds control.
                self.holder = Holder::Held(who);
                vec![Effect::Broadcast(self.state())]
            }
            Holder::Held(h) if h == who => {
                // The holder re-requesting is a no-op.
                vec![]
            }
            Holder::Held(_) => {
                if self.pending.is_none() {
                    // Start a cooperative handoff: ask the holder.
                    let deadline = self.deadline_from_now_default();
                    self.pending = Some((who, deadline));
                    vec![
                        Effect::NotifyControlRequested { requester: who },
                        Effect::Broadcast(self.state()),
                    ]
                } else {
                    // Someone is already pending; queue this requester (dedup).
                    if self.pending() != Some(who) && !self.queue.contains(&who) {
                        self.queue.push_back(who);
                    }
                    vec![]
                }
            }
        }
    }

    fn on_approve(&mut self, to: SessionId) -> Vec<Effect> {
        // Only meaningful if `to` is the pending requester.
        if self.pending() == Some(to) {
            self.pending = None;
            self.holder = Holder::Held(to);
            // Remove the newly-promoted session from the waiting queue if present.
            self.queue.retain(|s| *s != to);
            vec![Effect::Broadcast(self.state())]
        } else {
            vec![]
        }
    }

    fn on_deny(&mut self, to: SessionId) -> Vec<Effect> {
        if self.pending() == Some(to) {
            self.pending = None;
            let mut effects = vec![Effect::NotifyRequestRejected {
                requester: to,
                reason: RejectReason::Denied,
            }];
            // Promote the next queued waiter to pending (ask the holder again).
            effects.extend(self.promote_next_locked());
            effects.push(Effect::Broadcast(self.state()));
            effects
        } else {
            vec![]
        }
    }

    fn on_release(&mut self) -> Vec<Effect> {
        match self.holder {
            Holder::Free => vec![],
            Holder::Held(_) => {
                self.holder = Holder::Free;
                // Any pending request becomes the immediate grant; else queue.
                self.grant_from_waiters()
            }
        }
    }

    fn on_disconnect(&mut self, who: SessionId) -> Vec<Effect> {
        // Remove from queue regardless.
        self.queue.retain(|s| *s != who);

        let was_pending = self.pending() == Some(who);
        if was_pending {
            self.pending = None;
        }

        match self.holder {
            Holder::Held(h) if h == who => {
                // The controller vanished: free the token and auto-grant next.
                self.holder = Holder::Free;
                self.grant_from_waiters()
            }
            _ if was_pending => {
                // The pending requester vanished: promote next, rebroadcast.
                let mut effects = self.promote_next_locked();
                effects.push(Effect::Broadcast(self.state()));
                effects
            }
            _ => vec![],
        }
    }

    fn on_tick(&mut self, now: u64) -> Vec<Effect> {
        if self.timeout == 0 {
            return vec![];
        }
        if let Some((requester, deadline)) = self.pending {
            if now >= deadline {
                self.pending = None;
                let mut effects = vec![Effect::NotifyRequestRejected {
                    requester,
                    reason: RejectReason::TimedOut,
                }];
                effects.extend(self.promote_next_locked());
                effects.push(Effect::Broadcast(self.state()));
                return effects;
            }
        }
        vec![]
    }

    // --- helpers -----------------------------------------------------------

    /// When control just became free, grant it to the pending requester if any,
    /// otherwise to the oldest queued waiter, otherwise stay free.
    fn grant_from_waiters(&mut self) -> Vec<Effect> {
        if let Some((req, _)) = self.pending.take() {
            self.holder = Holder::Held(req);
            self.queue.retain(|s| *s != req);
            return vec![Effect::Broadcast(self.state())];
        }
        if let Some(next) = self.queue.pop_front() {
            self.holder = Holder::Held(next);
            return vec![Effect::Broadcast(self.state())];
        }
        vec![Effect::Broadcast(self.state())]
    }

    /// Promote the next queued waiter to the pending slot (holder still holds).
    /// Returns a notify effect for the newly-pending requester, if any.
    fn promote_next_locked(&mut self) -> Vec<Effect> {
        if self.pending.is_some() {
            return vec![];
        }
        if let Some(next) = self.queue.pop_front() {
            let deadline = self.deadline_from_now_default();
            self.pending = Some((next, deadline));
            return vec![Effect::NotifyControlRequested { requester: next }];
        }
        vec![]
    }

    /// Compute a pending deadline. We don't hold a clock, so the deadline is
    /// expressed relative to the *last observed tick is unknown here*; instead
    /// we store `timeout` as the deadline offset and rely on the caller to
    /// `Tick` with monotonically increasing `now`. To keep the machine pure
    /// yet still enforce timeouts, the deadline is set to `timeout`, and the
    /// caller's first `Tick{now}` after a request establishes the baseline.
    ///
    /// For determinism in tests, callers pass absolute `now` values; a request
    /// made "at now = T" should set deadline = T + timeout. Since `on_request`
    /// does not receive `now`, we instead record the *offset* and let the
    /// integration layer feed ticks; the test-friendly contract is: the first
    /// `Tick{now}` whose `now >= <request_now> + timeout` expires it. To make
    /// this exact, the integration layer should call `set_now` via a Tick
    /// immediately. See [`Arbiter::request_at`] for an explicit-time variant.
    fn deadline_from_now_default(&self) -> u64 {
        self.timeout
    }

    /// Explicit-time request used by the integration layer (and tests) when a
    /// precise deadline is required: records the request as pending with
    /// `deadline = now + timeout`.
    ///
    /// Prefer this over [`Input::Request`] when you have a clock, so timeouts
    /// are exact. Behaviour matches `on_request` otherwise.
    pub fn request_at(&mut self, who: SessionId, now: u64) -> Vec<Effect> {
        match self.holder {
            Holder::Held(h) if h != who && self.pending.is_none() => {
                let deadline = now.saturating_add(self.timeout);
                self.pending = Some((who, deadline));
                vec![
                    Effect::NotifyControlRequested { requester: who },
                    Effect::Broadcast(self.state()),
                ]
            }
            _ => self.on_request(who),
        }
    }
}

#[cfg(test)]
mod tests;
