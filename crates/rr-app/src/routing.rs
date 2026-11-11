//! Two-radio audio routing: which input is transmitted and which output plays.
//!
//! Read lock-free by the audio callbacks; written by the UI and by OTRSP.

use std::sync::atomic::{AtomicU8, Ordering};

use rr_radio::{Listen, OtrspCommand, Radio};
use serde::Serialize;

use crate::settings::AudioSettings;

/// Which input device feeds the transmitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InputSel {
    /// Device A (radio 1).
    A,
    /// Device B (radio 2).
    B,
}

/// Which output device plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputSel {
    /// Device A only.
    A,
    /// Device B only.
    B,
    /// Both devices.
    Mix,
}

impl InputSel {
    /// Parse a settings value; anything unknown is `A`.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("b") {
            Self::B
        } else {
            Self::A
        }
    }
}

impl OutputSel {
    /// Parse a settings value; anything unknown is `A`.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "b" => Self::B,
            "mix" => Self::Mix,
            _ => Self::A,
        }
    }
}

/// The live routing.
#[derive(Debug, Default)]
pub struct Routing {
    input: AtomicU8,
    output: AtomicU8,
}

impl Routing {
    /// Selected input as requested (may name a device that is not configured).
    #[must_use]
    pub fn input(&self) -> InputSel {
        if self.input.load(Ordering::Relaxed) == 1 {
            InputSel::B
        } else {
            InputSel::A
        }
    }

    /// Selected output as requested.
    #[must_use]
    pub fn output(&self) -> OutputSel {
        match self.output.load(Ordering::Relaxed) {
            1 => OutputSel::B,
            2 => OutputSel::Mix,
            _ => OutputSel::A,
        }
    }

    /// Set the input; returns true if it changed.
    pub fn set_input(&self, sel: InputSel) -> bool {
        let v = u8::from(sel == InputSel::B);
        self.input.swap(v, Ordering::Relaxed) != v
    }

    /// Set the output; returns true if it changed.
    pub fn set_output(&self, sel: OutputSel) -> bool {
        let v = match sel {
            OutputSel::A => 0,
            OutputSel::B => 1,
            OutputSel::Mix => 2,
        };
        self.output.swap(v, Ordering::Relaxed) != v
    }

    /// Take the manual selection from settings.
    pub fn load(&self, a: &AudioSettings) -> bool {
        let i = self.set_input(InputSel::parse(&a.input_select));
        let o = self.set_output(OutputSel::parse(&a.output_select));
        i || o
    }

    /// Apply an OTRSP command; returns (input changed, output changed).
    pub fn apply_otrsp(&self, cmd: &OtrspCommand) -> (bool, bool) {
        match cmd {
            OtrspCommand::Tx(Radio::One) => (self.set_input(InputSel::A), false),
            OtrspCommand::Tx(Radio::Two) => (self.set_input(InputSel::B), false),
            OtrspCommand::Rx(Listen::Single(Radio::One)) => (false, self.set_output(OutputSel::A)),
            OtrspCommand::Rx(Listen::Single(Radio::Two)) => (false, self.set_output(OutputSel::B)),
            OtrspCommand::Rx(Listen::Both) => (false, self.set_output(OutputSel::Mix)),
            OtrspCommand::QueryName => (false, false),
        }
    }
}

/// Which outputs should be audible: (A, B). Without a B device, A always plays.
#[must_use]
pub fn audible(sel: OutputSel, has_b: bool) -> (bool, bool) {
    if !has_b {
        return (true, false);
    }
    match sel {
        OutputSel::A => (true, false),
        OutputSel::B => (false, true),
        OutputSel::Mix => (true, true),
    }
}

/// The input device actually used: B only if selected *and* configured.
#[must_use]
pub fn effective_input(sel: InputSel, has_b: bool) -> InputSel {
    if has_b {
        sel
    } else {
        InputSel::A
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn otrsp_drives_routing() {
        let r = Routing::default();
        assert_eq!(r.apply_otrsp(&OtrspCommand::Tx(Radio::Two)), (true, false));
        assert_eq!(r.input(), InputSel::B);
        assert_eq!(r.apply_otrsp(&OtrspCommand::Tx(Radio::Two)), (false, false));
        assert_eq!(r.apply_otrsp(&OtrspCommand::Rx(Listen::Both)), (false, true));
        assert_eq!(r.output(), OutputSel::Mix);
        let _ = r.apply_otrsp(&OtrspCommand::Rx(Listen::Single(Radio::One)));
        assert_eq!(r.output(), OutputSel::A);
    }

    #[test]
    fn missing_b_falls_back_to_a() {
        assert_eq!(audible(OutputSel::B, false), (true, false));
        assert_eq!(audible(OutputSel::Mix, true), (true, true));
        assert_eq!(audible(OutputSel::B, true), (false, true));
        assert_eq!(effective_input(InputSel::B, false), InputSel::A);
        assert_eq!(effective_input(InputSel::B, true), InputSel::B);
    }

    #[test]
    fn parses_settings_values() {
        assert_eq!(InputSel::parse("B"), InputSel::B);
        assert_eq!(InputSel::parse("x"), InputSel::A);
        assert_eq!(OutputSel::parse("mix"), OutputSel::Mix);
        assert_eq!(OutputSel::parse(""), OutputSel::A);
    }
}
