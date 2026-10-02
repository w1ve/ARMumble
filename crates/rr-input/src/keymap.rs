//! The platform-independent key code table.
//!
//! [`rr_proto::InputEvent::Key`] carries a bare `u32` and points here for its
//! meaning. This module is that definition.
//!
//! # Why not reuse an existing numbering
//!
//! The obvious candidates are both wrong for this:
//!
//! - **Windows virtual-key codes** would make the Station trivial and the wire
//!   format Windows-only. A Linux or macOS Station later would have to either
//!   translate from a foreign platform's codes or break the protocol.
//! - **Raw DOM `KeyboardEvent.code` strings** are platform-independent but
//!   variable-length, so every keystroke would carry a string allocation, and
//!   typos would be silently unmatched rather than failing to compile.
//!
//! So [`KeyCode`] is an explicit enum with stable discriminants that are part of
//! the wire format. The Client converts from DOM codes once, at the edge, with
//! [`KeyCode::from_dom_code`]; the Station converts to platform keys once, at the
//! other edge. Both conversions are total functions over a closed set, and an
//! unrecognised key is **refused rather than guessed** — a wrong guess means
//! injecting a keystroke nobody pressed.
//!
//! # Stability
//!
//! The discriminants are wire format. Append new keys; never renumber. A test
//! pins a sample of them so renumbering fails loudly.

use std::fmt;

/// A key, identified independently of any platform.
///
/// Discriminants are **wire format** and must not be renumbered. Grouped in
/// hundreds so related keys can be added without disturbing anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u32)]
#[non_exhaustive]
pub enum KeyCode {
    // -- Letters, 100.. ------------------------------------------------
    /// The `A` key.
    A = 100,
    /// The `B` key.
    B = 101,
    /// The `C` key.
    C = 102,
    /// The `D` key.
    D = 103,
    /// The `E` key.
    E = 104,
    /// The `F` key.
    F = 105,
    /// The `G` key.
    G = 106,
    /// The `H` key.
    H = 107,
    /// The `I` key.
    I = 108,
    /// The `J` key.
    J = 109,
    /// The `K` key.
    K = 110,
    /// The `L` key.
    L = 111,
    /// The `M` key.
    M = 112,
    /// The `N` key.
    N = 113,
    /// The `O` key.
    O = 114,
    /// The `P` key.
    P = 115,
    /// The `Q` key.
    Q = 116,
    /// The `R` key.
    R = 117,
    /// The `S` key.
    S = 118,
    /// The `T` key.
    T = 119,
    /// The `U` key.
    U = 120,
    /// The `V` key.
    V = 121,
    /// The `W` key.
    W = 122,
    /// The `X` key.
    X = 123,
    /// The `Y` key.
    Y = 124,
    /// The `Z` key.
    Z = 125,

    // -- Digit row, 200.. ----------------------------------------------
    /// The `0` key on the digit row.
    Digit0 = 200,
    /// The `1` key on the digit row.
    Digit1 = 201,
    /// The `2` key on the digit row.
    Digit2 = 202,
    /// The `3` key on the digit row.
    Digit3 = 203,
    /// The `4` key on the digit row.
    Digit4 = 204,
    /// The `5` key on the digit row.
    Digit5 = 205,
    /// The `6` key on the digit row.
    Digit6 = 206,
    /// The `7` key on the digit row.
    Digit7 = 207,
    /// The `8` key on the digit row.
    Digit8 = 208,
    /// The `9` key on the digit row.
    Digit9 = 209,

    // -- Editing and whitespace, 300.. ---------------------------------
    /// Return / Enter.
    Enter = 300,
    /// Tab.
    Tab = 301,
    /// Space bar.
    Space = 302,
    /// Backspace.
    Backspace = 303,
    /// Forward delete.
    Delete = 304,
    /// Insert.
    Insert = 305,
    /// Escape.
    Escape = 306,

    // -- Navigation, 400.. ---------------------------------------------
    /// Left arrow.
    ArrowLeft = 400,
    /// Right arrow.
    ArrowRight = 401,
    /// Up arrow.
    ArrowUp = 402,
    /// Down arrow.
    ArrowDown = 403,
    /// Home.
    Home = 404,
    /// End.
    End = 405,
    /// Page up.
    PageUp = 406,
    /// Page down.
    PageDown = 407,

    // -- Modifiers, 500.. ----------------------------------------------
    /// Left shift.
    ShiftLeft = 500,
    /// Right shift.
    ShiftRight = 501,
    /// Left control.
    ControlLeft = 502,
    /// Right control.
    ControlRight = 503,
    /// Left alt / option.
    AltLeft = 504,
    /// Right alt / alt graph.
    AltRight = 505,
    /// Left meta (Windows / Command).
    MetaLeft = 506,
    /// Right meta (Windows / Command).
    MetaRight = 507,
    /// Caps lock.
    CapsLock = 508,

    // -- Function keys, 600.. ------------------------------------------
    /// F1.
    F1 = 600,
    /// F2.
    F2 = 601,
    /// F3.
    F3 = 602,
    /// F4.
    F4 = 603,
    /// F5.
    F5 = 604,
    /// F6.
    F6 = 605,
    /// F7.
    F7 = 606,
    /// F8.
    F8 = 607,
    /// F9.
    F9 = 608,
    /// F10.
    F10 = 609,
    /// F11.
    F11 = 610,
    /// F12.
    F12 = 611,

    // -- Punctuation, 700.. --------------------------------------------
    /// `-` / `_`.
    Minus = 700,
    /// `=` / `+`.
    Equal = 701,
    /// `[` / `{`.
    BracketLeft = 702,
    /// `]` / `}`.
    BracketRight = 703,
    /// `\` / `|`.
    Backslash = 704,
    /// `;` / `:`.
    Semicolon = 705,
    /// `'` / `"`.
    Quote = 706,
    /// `` ` `` / `~`.
    Backquote = 707,
    /// `,` / `<`.
    Comma = 708,
    /// `.` / `>`.
    Period = 709,
    /// `/` / `?`.
    Slash = 710,
}

impl KeyCode {
    /// Every key in the table, for exhaustive tests and UI enumeration.
    ///
    /// Kept in step with the enum by a test that checks every discriminant round
    /// trips, which fails if a key is added here but not there or vice versa.
    pub const ALL: &'static [Self] = &[
        Self::A, Self::B, Self::C, Self::D, Self::E, Self::F, Self::G, Self::H,
        Self::I, Self::J, Self::K, Self::L, Self::M, Self::N, Self::O, Self::P,
        Self::Q, Self::R, Self::S, Self::T, Self::U, Self::V, Self::W, Self::X,
        Self::Y, Self::Z,
        Self::Digit0, Self::Digit1, Self::Digit2, Self::Digit3, Self::Digit4,
        Self::Digit5, Self::Digit6, Self::Digit7, Self::Digit8, Self::Digit9,
        Self::Enter, Self::Tab, Self::Space, Self::Backspace, Self::Delete,
        Self::Insert, Self::Escape,
        Self::ArrowLeft, Self::ArrowRight, Self::ArrowUp, Self::ArrowDown,
        Self::Home, Self::End, Self::PageUp, Self::PageDown,
        Self::ShiftLeft, Self::ShiftRight, Self::ControlLeft, Self::ControlRight,
        Self::AltLeft, Self::AltRight, Self::MetaLeft, Self::MetaRight,
        Self::CapsLock,
        Self::F1, Self::F2, Self::F3, Self::F4, Self::F5, Self::F6,
        Self::F7, Self::F8, Self::F9, Self::F10, Self::F11, Self::F12,
        Self::Minus, Self::Equal, Self::BracketLeft, Self::BracketRight,
        Self::Backslash, Self::Semicolon, Self::Quote, Self::Backquote,
        Self::Comma, Self::Period, Self::Slash,
    ];

    /// The wire value.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }

    /// Parse a wire value.
    ///
    /// Returns `None` for anything unrecognised. The caller must refuse such an
    /// event: injecting a guessed key is worse than injecting nothing.
    #[must_use]
    pub fn from_u32(value: u32) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_u32() == value)
    }

    /// The DOM `KeyboardEvent.code` string for this key.
    ///
    /// The Client runs in a webview, so DOM codes are what it actually has. These
    /// strings are the W3C `code` values, which describe physical position and so
    /// survive keyboard-layout differences between the two machines.
    #[must_use]
    pub const fn dom_code(self) -> &'static str {
        match self {
            Self::A => "KeyA",
            Self::B => "KeyB",
            Self::C => "KeyC",
            Self::D => "KeyD",
            Self::E => "KeyE",
            Self::F => "KeyF",
            Self::G => "KeyG",
            Self::H => "KeyH",
            Self::I => "KeyI",
            Self::J => "KeyJ",
            Self::K => "KeyK",
            Self::L => "KeyL",
            Self::M => "KeyM",
            Self::N => "KeyN",
            Self::O => "KeyO",
            Self::P => "KeyP",
            Self::Q => "KeyQ",
            Self::R => "KeyR",
            Self::S => "KeyS",
            Self::T => "KeyT",
            Self::U => "KeyU",
            Self::V => "KeyV",
            Self::W => "KeyW",
            Self::X => "KeyX",
            Self::Y => "KeyY",
            Self::Z => "KeyZ",
            Self::Digit0 => "Digit0",
            Self::Digit1 => "Digit1",
            Self::Digit2 => "Digit2",
            Self::Digit3 => "Digit3",
            Self::Digit4 => "Digit4",
            Self::Digit5 => "Digit5",
            Self::Digit6 => "Digit6",
            Self::Digit7 => "Digit7",
            Self::Digit8 => "Digit8",
            Self::Digit9 => "Digit9",
            Self::Enter => "Enter",
            Self::Tab => "Tab",
            Self::Space => "Space",
            Self::Backspace => "Backspace",
            Self::Delete => "Delete",
            Self::Insert => "Insert",
            Self::Escape => "Escape",
            Self::ArrowLeft => "ArrowLeft",
            Self::ArrowRight => "ArrowRight",
            Self::ArrowUp => "ArrowUp",
            Self::ArrowDown => "ArrowDown",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "PageUp",
            Self::PageDown => "PageDown",
            Self::ShiftLeft => "ShiftLeft",
            Self::ShiftRight => "ShiftRight",
            Self::ControlLeft => "ControlLeft",
            Self::ControlRight => "ControlRight",
            Self::AltLeft => "AltLeft",
            Self::AltRight => "AltRight",
            Self::MetaLeft => "MetaLeft",
            Self::MetaRight => "MetaRight",
            Self::CapsLock => "CapsLock",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F6 => "F6",
            Self::F7 => "F7",
            Self::F8 => "F8",
            Self::F9 => "F9",
            Self::F10 => "F10",
            Self::F11 => "F11",
            Self::F12 => "F12",
            Self::Minus => "Minus",
            Self::Equal => "Equal",
            Self::BracketLeft => "BracketLeft",
            Self::BracketRight => "BracketRight",
            Self::Backslash => "Backslash",
            Self::Semicolon => "Semicolon",
            Self::Quote => "Quote",
            Self::Backquote => "Backquote",
            Self::Comma => "Comma",
            Self::Period => "Period",
            Self::Slash => "Slash",
        }
    }

    /// Parse a DOM `KeyboardEvent.code`.
    ///
    /// Returns `None` for keys outside the table — media keys, numpad, and the
    /// like. The Client should drop those rather than substitute something.
    #[must_use]
    pub fn from_dom_code(code: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.dom_code() == code)
    }

    /// Whether this key is a modifier.
    ///
    /// The Station uses this to release held modifiers when control changes hands:
    /// a controller that disconnects mid-chord would otherwise leave Ctrl stuck
    /// down on someone else's machine.
    #[must_use]
    pub const fn is_modifier(self) -> bool {
        matches!(
            self,
            Self::ShiftLeft
                | Self::ShiftRight
                | Self::ControlLeft
                | Self::ControlRight
                | Self::AltLeft
                | Self::AltRight
                | Self::MetaLeft
                | Self::MetaRight
        )
    }
}

impl fmt::Display for KeyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.dom_code())
    }
}
