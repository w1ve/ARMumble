//! The real input injector, backed by [`enigo`].
//!
//! This is the one place in the project that actually moves the Station's
//! pointer and presses its keys — remote code execution by design. It is behind
//! the `inject` feature so the Client, which never injects, does not link an
//! input backend at all, and so the arbitration and coordinate logic can be
//! tested on a headless CI box with no input system present.
//!
//! `enigo` uses `SendInput` on Windows, so **no `unsafe` reaches this crate** —
//! the one hard rule that keeps `rr-input` inside the project's
//! `unsafe`-confined-to-`rr-hwcodec` policy.
//!
//! # Translation, not interpretation
//!
//! This backend is deliberately dumb. It maps a [`KeyCode`] or [`MouseButton`]
//! to its `enigo` equivalent and performs it. *Whether* an event should happen is
//! [`crate::ControlGate`]'s decision, made before anything here is called, so
//! this file has no policy, no clamping, and no arbitration — just a total
//! mapping and a single `enigo` call per event.
//!
//! # The key mapping, and its two deliberate collapses
//!
//! `enigo`'s `Key` enum does not distinguish left and right for every modifier.
//! Where it does not, two of our codes map to one `enigo` key:
//!
//! - `AltLeft` and `AltRight` both become `Key::Alt` — `enigo` 0.6 has no
//!   `LAlt`/`RAlt`. In practice `AltRight` is `AltGr` on some layouts, which this
//!   loses; acceptable until a user needs it, and called out here so the loss is
//!   not silent.
//! - `MetaLeft` and `MetaRight` both become `Key::Meta` (the key enigo documents
//!   as "windows / super / command"). enigo's left/right `Win` variants are
//!   Windows-only, so `Key::Meta` is the portable choice and keeps the Linux
//!   `inject` build compiling — the side distinction is lost, same as Alt.
//!
//! Letters, digits and punctuation go through `Key::Unicode(char)`, which types
//! the character by its unshifted base — correct because the controller sends a
//! separate `ShiftLeft`/`ShiftRight` event for the shifted form, exactly as a
//! physical keyboard produces them.

use enigo::{
    Axis, Button as EnigoButton, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings,
};
use rr_proto::MouseButton;

use crate::{InputError, InputInjector, KeyCode, Result};

/// Turn an `enigo` error into our error type.
fn inject_err(e: impl std::fmt::Display) -> InputError {
    InputError::Injection(e.to_string())
}

/// The real injector.
///
/// Holds one `Enigo` connection for its lifetime; constructing it per event
/// would re-open the platform input channel on every keystroke.
pub struct EnigoInjector {
    enigo: Enigo,
}

impl std::fmt::Debug for EnigoInjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `Enigo` is not `Debug`, and its internals are not interesting here.
        f.debug_struct("EnigoInjector").finish_non_exhaustive()
    }
}

impl EnigoInjector {
    /// Open the platform input connection.
    ///
    /// # Errors
    /// Returns [`InputError::Unavailable`] if the input system cannot be reached
    /// — no display/session, or the platform refusing a connection. The caller
    /// should treat this as "this machine cannot be a Station" rather than retry.
    pub fn new() -> Result<Self> {
        let enigo = Enigo::new(&Settings::default())
            .map_err(|_| InputError::Unavailable("could not open the platform input system"))?;
        Ok(Self { enigo })
    }
}

/// Map a wire mouse button to `enigo`'s. Total: every `MouseButton` has an
/// `enigo` equivalent, so this cannot fail.
const fn button(b: MouseButton) -> EnigoButton {
    match b {
        MouseButton::Left => EnigoButton::Left,
        MouseButton::Right => EnigoButton::Right,
        MouseButton::Middle => EnigoButton::Middle,
        MouseButton::Back => EnigoButton::Back,
        MouseButton::Forward => EnigoButton::Forward,
    }
}

/// Map a [`KeyCode`] to an `enigo` [`Key`].
///
/// Total over the closed `KeyCode` set, so there is no unmapped case to refuse
/// here — [`crate::ControlGate`] has already refused anything that was not a
/// known `KeyCode` before this is reached. See the module docs for the two
/// left/right collapses.
fn key(code: KeyCode) -> Key {
    use KeyCode as K;
    match code {
        // Letters: Unicode base character; Shift arrives as its own event.
        K::A => Key::Unicode('a'),
        K::B => Key::Unicode('b'),
        K::C => Key::Unicode('c'),
        K::D => Key::Unicode('d'),
        K::E => Key::Unicode('e'),
        K::F => Key::Unicode('f'),
        K::G => Key::Unicode('g'),
        K::H => Key::Unicode('h'),
        K::I => Key::Unicode('i'),
        K::J => Key::Unicode('j'),
        K::K => Key::Unicode('k'),
        K::L => Key::Unicode('l'),
        K::M => Key::Unicode('m'),
        K::N => Key::Unicode('n'),
        K::O => Key::Unicode('o'),
        K::P => Key::Unicode('p'),
        K::Q => Key::Unicode('q'),
        K::R => Key::Unicode('r'),
        K::S => Key::Unicode('s'),
        K::T => Key::Unicode('t'),
        K::U => Key::Unicode('u'),
        K::V => Key::Unicode('v'),
        K::W => Key::Unicode('w'),
        K::X => Key::Unicode('x'),
        K::Y => Key::Unicode('y'),
        K::Z => Key::Unicode('z'),
        // Digit row: base character, as above.
        K::Digit0 => Key::Unicode('0'),
        K::Digit1 => Key::Unicode('1'),
        K::Digit2 => Key::Unicode('2'),
        K::Digit3 => Key::Unicode('3'),
        K::Digit4 => Key::Unicode('4'),
        K::Digit5 => Key::Unicode('5'),
        K::Digit6 => Key::Unicode('6'),
        K::Digit7 => Key::Unicode('7'),
        K::Digit8 => Key::Unicode('8'),
        K::Digit9 => Key::Unicode('9'),
        // Editing / whitespace.
        K::Enter => Key::Return,
        K::Tab => Key::Tab,
        K::Space => Key::Space,
        K::Backspace => Key::Backspace,
        K::Delete => Key::Delete,
        K::Insert => Key::Insert,
        K::Escape => Key::Escape,
        // Navigation.
        K::ArrowLeft => Key::LeftArrow,
        K::ArrowRight => Key::RightArrow,
        K::ArrowUp => Key::UpArrow,
        K::ArrowDown => Key::DownArrow,
        K::Home => Key::Home,
        K::End => Key::End,
        K::PageUp => Key::PageUp,
        K::PageDown => Key::PageDown,
        // Modifiers. `enigo` 0.6 distinguishes L/R shift and control, but has
        // only one `Alt`, and uses `LWin`/`RWin` for meta.
        K::ShiftLeft => Key::LShift,
        K::ShiftRight => Key::RShift,
        K::ControlLeft => Key::LControl,
        K::ControlRight => Key::RControl,
        K::AltLeft | K::AltRight => Key::Alt,
        K::MetaLeft | K::MetaRight => Key::Meta,
        K::CapsLock => Key::CapsLock,
        // Function keys.
        K::F1 => Key::F1,
        K::F2 => Key::F2,
        K::F3 => Key::F3,
        K::F4 => Key::F4,
        K::F5 => Key::F5,
        K::F6 => Key::F6,
        K::F7 => Key::F7,
        K::F8 => Key::F8,
        K::F9 => Key::F9,
        K::F10 => Key::F10,
        K::F11 => Key::F11,
        K::F12 => Key::F12,
        // Punctuation: unshifted base, matching the letter/digit convention.
        K::Minus => Key::Unicode('-'),
        K::Equal => Key::Unicode('='),
        K::BracketLeft => Key::Unicode('['),
        K::BracketRight => Key::Unicode(']'),
        K::Backslash => Key::Unicode('\\'),
        K::Semicolon => Key::Unicode(';'),
        K::Quote => Key::Unicode('\''),
        K::Backquote => Key::Unicode('`'),
        K::Comma => Key::Unicode(','),
        K::Period => Key::Unicode('.'),
        K::Slash => Key::Unicode('/'),
    }
}

/// Press (`true`) or release (`false`) as an `enigo` [`Direction`].
///
/// Always a discrete press or release, never `Click`: the wire protocol carries
/// down and up as separate events, so a controller holding a key produces a
/// `down` with no matching `up` until release — collapsing to `Click` would
/// break held keys and modifier chords.
const fn direction(down: bool) -> Direction {
    if down {
        Direction::Press
    } else {
        Direction::Release
    }
}

impl InputInjector for EnigoInjector {
    fn mouse_move_abs(&mut self, x: i32, y: i32) -> Result<()> {
        // Absolute virtual-desktop coordinates: `MonitorLayout` has already
        // mapped the monitor-relative wire position before this is reached.
        self.enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(inject_err)
    }

    fn mouse_button(&mut self, b: MouseButton, down: bool) -> Result<()> {
        self.enigo
            .button(button(b), direction(down))
            .map_err(inject_err)
    }

    fn wheel(&mut self, dx: i32, dy: i32) -> Result<()> {
        // Two axes, two calls; skip an axis with no delta so a pure vertical
        // scroll does not emit a spurious horizontal event. `enigo`'s positive
        // direction is down/right, matching the wire convention documented on
        // `InputInjector::wheel`.
        if dx != 0 {
            self.enigo.scroll(dx, Axis::Horizontal).map_err(inject_err)?;
        }
        if dy != 0 {
            self.enigo.scroll(dy, Axis::Vertical).map_err(inject_err)?;
        }
        Ok(())
    }

    fn key(&mut self, code: KeyCode, down: bool) -> Result<()> {
        self.enigo
            .key(key(code), direction(down))
            .map_err(inject_err)
    }

    fn text(&mut self, text: &str) -> Result<()> {
        self.enigo.text(text).map_err(inject_err)
    }

    fn release_all(&mut self) -> Result<()> {
        // Release every modifier, unconditionally. enigo tracks no "currently
        // held" set we can query, so the safe move at a change of control is to
        // issue a release for each modifier: releasing one that is not held is a
        // harmless no-op, whereas failing to release one that *is* held leaves it
        // stuck down on the Station for the next controller. This is why the wire
        // protocol and `ControlGate` lean on `release_all` at every handoff.
        for code in KeyCode::ALL.iter().copied().filter(|k| k.is_modifier()) {
            self.enigo
                .key(key(code), Direction::Release)
                .map_err(inject_err)?;
        }
        Ok(())
    }
}
