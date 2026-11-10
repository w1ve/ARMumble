//! User-facing PTT settings. Local and Station PTT are independent: a machine
//! may have a footswitch, a rig cable, both, or neither.

use serde::{Deserialize, Serialize};

/// Input line a footswitch is wired to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SenseLine {
    /// Clear To Send.
    Cts,
    /// Data Set Ready.
    Dsr,
    /// Carrier Detect.
    Cd,
    /// Ring Indicator.
    Ri,
}

/// Output line that keys the rig.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyLine {
    /// Data Terminal Ready.
    Dtr,
    /// Request To Send.
    Rts,
}

/// Local PTT: a footswitch on an RS-232 input line. Pressing it transmits your
/// microphone locally and, when a remote station is configured, tells that
/// station to key its rig.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalPttSettings {
    /// Enable the footswitch.
    pub enabled: bool,
    /// Serial port name (for example `COM3` or `/dev/ttyUSB0`).
    pub port: String,
    /// Input line the footswitch is wired to.
    pub sense_line: SenseLine,
    /// True when the switch pulls the line low while pressed.
    pub active_low: bool,
}

impl Default for LocalPttSettings {
    fn default() -> Self {
        Self { enabled: false, port: String::new(), sense_line: SenseLine::Cts, active_low: false }
    }
}

/// Station PTT: an RS-232 output line that keys the rig when a remote operator
/// asks to transmit, or when VOX hears inbound audio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StationPttSettings {
    /// Enable rig keying.
    pub enabled: bool,
    /// Serial port name.
    pub port: String,
    /// Output line that keys the rig.
    pub key_line: KeyLine,
    /// True when the rig keys on a low level.
    pub active_low: bool,
    /// Key the rig whenever inbound audio is present. Essential for radios that
    /// cannot VOX over a remote audio connection. Works together with remote
    /// PTT messages: either one keys the rig.
    pub vox: bool,
    /// How long VOX holds the key after audio stops, in milliseconds.
    pub vox_hang_ms: u32,
    /// How long to hold the key after the last request ends, in milliseconds.
    pub hang_ms: u32,
    /// A remote PTT request expires unless refreshed within this time (ms).
    pub deadman_ms: u32,
    /// Longest single transmission in seconds. The rig is released after this.
    pub max_tx_secs: u32,
}

impl Default for StationPttSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: String::new(),
            key_line: KeyLine::Rts,
            active_low: false,
            vox: true,
            vox_hang_ms: 500,
            hang_ms: 150,
            deadman_ms: 2_000,
            max_tx_secs: 180,
        }
    }
}

impl StationPttSettings {
    /// Largest accepted VOX hang, in milliseconds.
    pub const MAX_VOX_HANG_MS: u32 = 5_000;

    /// Clamp every field to a sane range.
    #[must_use]
    pub fn sanitised(mut self) -> Self {
        self.vox_hang_ms = self.vox_hang_ms.min(Self::MAX_VOX_HANG_MS);
        self.hang_ms = self.hang_ms.min(2_000);
        self.deadman_ms = self.deadman_ms.clamp(500, 10_000);
        self.max_tx_secs = self.max_tx_secs.clamp(5, 900);
        self
    }
}

/// All PTT settings together.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PttSettings {
    /// Footswitch input.
    pub local: LocalPttSettings,
    /// Rig output.
    pub station: StationPttSettings,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitise_clamps() {
        let s = StationPttSettings { vox_hang_ms: 99_999, deadman_ms: 1, max_tx_secs: 0, ..Default::default() }
            .sanitised();
        assert_eq!(s.vox_hang_ms, 5_000);
        assert_eq!(s.deadman_ms, 500);
        assert_eq!(s.max_tx_secs, 5);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s: PttSettings = serde_json::from_str(r#"{"station":{"enabled":true,"port":"COM4"}}"#).expect("parse");
        assert!(s.station.enabled && s.station.vox);
        assert_eq!(s.station.port, "COM4");
        assert!(!s.local.enabled);
    }
}
