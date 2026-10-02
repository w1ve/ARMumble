//! An injector that records instead of acting.
//!
//! The whole point of [`crate::ControlGate`] is deciding whether a keystroke
//! happens. Testing that against a real input system would mean a test suite that
//! moves the developer's pointer, needs a desktop session, and cannot assert what
//! actually occurred. Recording the calls instead makes the interesting property —
//! *nothing was injected for an unauthorised peer* — directly assertable, and
//! lets the whole of P3's logic run on a headless CI box.
//!
//! Public rather than test-only on purpose: it is also the right tool for a dry-run
//! mode, and for reproducing a reported sequence without driving anyone's machine.

use rr_proto::MouseButton;

use crate::{InputError, InputInjector, KeyCode, Result};

/// One thing an injector was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordedAction {
    /// Pointer moved to absolute virtual-desktop coordinates.
    MouseMove {
        /// Absolute X.
        x: i32,
        /// Absolute Y.
        y: i32,
    },
    /// Button pressed or released.
    MouseButton {
        /// Which button.
        button: MouseButton,
        /// `true` when pressed.
        down: bool,
    },
    /// Wheel scrolled.
    Wheel {
        /// Horizontal delta.
        dx: i32,
        /// Vertical delta.
        dy: i32,
    },
    /// Key pressed or released.
    Key {
        /// Which key.
        key: KeyCode,
        /// `true` when pressed.
        down: bool,
    },
    /// Text typed.
    Text {
        /// What was typed.
        text: String,
    },
    /// Everything held was released.
    ReleaseAll,
}

/// An [`InputInjector`] that records calls instead of performing them.
#[derive(Debug, Default)]
pub struct RecordingInjector {
    /// Everything recorded, in order.
    actions: Vec<RecordedAction>,
    /// When set, every call fails with this message, to exercise error paths.
    fail_with: Option<String>,
}

impl RecordingInjector {
    /// A recorder that accepts everything.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A recorder that fails every call, so callers' error handling is exercised.
    #[must_use]
    pub fn failing(message: impl Into<String>) -> Self {
        Self {
            actions: Vec::new(),
            fail_with: Some(message.into()),
        }
    }

    /// Everything recorded so far, in order.
    #[must_use]
    pub fn actions(&self) -> &[RecordedAction] {
        &self.actions
    }

    /// How many actions were recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.actions.len()
    }

    /// Whether nothing has been recorded.
    ///
    /// The assertion most tests here actually want: *no* injection happened.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// Forget everything recorded.
    pub fn clear(&mut self) {
        self.actions.clear();
    }

    /// Record an action, or fail if this recorder is set to fail.
    fn record(&mut self, action: RecordedAction) -> Result<()> {
        if let Some(message) = self.fail_with.as_ref() {
            return Err(InputError::Injection(message.clone()));
        }
        self.actions.push(action);
        Ok(())
    }
}

impl InputInjector for RecordingInjector {
    fn mouse_move_abs(&mut self, x: i32, y: i32) -> Result<()> {
        self.record(RecordedAction::MouseMove { x, y })
    }

    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        self.record(RecordedAction::MouseButton { button, down })
    }

    fn wheel(&mut self, dx: i32, dy: i32) -> Result<()> {
        self.record(RecordedAction::Wheel { dx, dy })
    }

    fn key(&mut self, key: KeyCode, down: bool) -> Result<()> {
        self.record(RecordedAction::Key { key, down })
    }

    fn text(&mut self, text: &str) -> Result<()> {
        self.record(RecordedAction::Text {
            text: text.to_owned(),
        })
    }

    fn release_all(&mut self) -> Result<()> {
        self.record(RecordedAction::ReleaseAll)
    }
}
