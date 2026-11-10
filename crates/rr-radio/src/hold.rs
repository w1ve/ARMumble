//! Hold-to-activate hotkeys: Whisper and Listen In.
//!
//! * **Whisper**: while held, your transmission goes to a chosen channel
//!   instead of your own.
//! * **Listen In**: while held, you also hear a chosen channel; on release you
//!   stop hearing it. Optionally the home channel is muted meanwhile.
//!
//! These types are pure state machines. They turn key presses and releases into
//! [`HoldAction`]s for the connection layer to carry out, and guarantee that
//! every press is undone by exactly one release.

/// What the connection layer should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldAction {
    /// Start listening to a channel (Mumble channel-listen add).
    ListenAdd(u32),
    /// Stop listening to a channel (Mumble channel-listen remove).
    ListenRemove(u32),
    /// Send voice to this channel, or back to normal with `None`.
    Whisper(Option<u32>),
    /// Mute (`true`) or restore (`false`) the home channel's audio.
    MuteHome(bool),
}

/// The Listen In hold state.
#[derive(Debug, Default, Clone)]
pub struct ListenIn {
    active: Option<u32>,
    muted_home: bool,
}

impl ListenIn {
    /// The hotkey went down. `home` is the channel you are in.
    pub fn press(&mut self, target: u32, home: u32, mute_home: bool) -> Vec<HoldAction> {
        if self.active.is_some() || target == home {
            return Vec::new();
        }
        self.active = Some(target);
        let mut out = vec![HoldAction::ListenAdd(target)];
        if mute_home {
            self.muted_home = true;
            out.push(HoldAction::MuteHome(true));
        }
        out
    }

    /// The hotkey went up.
    pub fn release(&mut self) -> Vec<HoldAction> {
        let mut out = Vec::new();
        if let Some(t) = self.active.take() {
            out.push(HoldAction::ListenRemove(t));
        }
        if std::mem::take(&mut self.muted_home) {
            out.push(HoldAction::MuteHome(false));
        }
        out
    }

    /// The channel currently being listened to, if any.
    #[must_use]
    pub fn active(&self) -> Option<u32> {
        self.active
    }
}

/// The Whisper hold state.
#[derive(Debug, Default, Clone)]
pub struct WhisperHold {
    active: bool,
}

impl WhisperHold {
    /// The hotkey went down.
    pub fn press(&mut self, target: u32, home: u32) -> Vec<HoldAction> {
        if self.active || target == home {
            return Vec::new();
        }
        self.active = true;
        vec![HoldAction::Whisper(Some(target))]
    }

    /// The hotkey went up.
    pub fn release(&mut self) -> Vec<HoldAction> {
        if std::mem::take(&mut self.active) {
            vec![HoldAction::Whisper(None)]
        } else {
            Vec::new()
        }
    }

    /// Whether the hold is active.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_in_adds_then_removes() {
        let mut l = ListenIn::default();
        assert_eq!(l.press(7, 1, false), vec![HoldAction::ListenAdd(7)]);
        assert_eq!(l.press(8, 1, false), vec![], "second press ignored while held");
        assert_eq!(l.release(), vec![HoldAction::ListenRemove(7)]);
        assert_eq!(l.release(), vec![]);
    }

    #[test]
    fn listen_in_can_mute_home_and_restores_it() {
        let mut l = ListenIn::default();
        assert_eq!(
            l.press(7, 1, true),
            vec![HoldAction::ListenAdd(7), HoldAction::MuteHome(true)]
        );
        assert_eq!(
            l.release(),
            vec![HoldAction::ListenRemove(7), HoldAction::MuteHome(false)]
        );
    }

    #[test]
    fn listening_to_home_channel_is_a_no_op() {
        let mut l = ListenIn::default();
        assert_eq!(l.press(1, 1, true), vec![]);
        assert_eq!(l.release(), vec![]);
    }

    #[test]
    fn whisper_press_release() {
        let mut w = WhisperHold::default();
        assert_eq!(w.press(3, 1), vec![HoldAction::Whisper(Some(3))]);
        assert!(w.is_active());
        assert_eq!(w.release(), vec![HoldAction::Whisper(None)]);
        assert_eq!(w.release(), vec![]);
    }
}
