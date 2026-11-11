//! Persistent user settings.

use std::path::{Path, PathBuf};

use rr_audio::capture::{ChannelMap, TxMode};
use rr_radio::PttSettings;
use serde::{Deserialize, Serialize};

/// Which server to join.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerSettings {
    /// Host name.
    pub host: String,
    /// Port.
    pub port: u16,
    /// Login name.
    pub username: String,
    /// Server password.
    pub password: String,
    /// Confine the client to this channel and its sub-channels; empty means the whole server.
    pub scope: String,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            host: "newyork.remote.radio".into(),
            port: 54736,
            username: String::new(),
            password: "Demo!".into(),
            scope: String::new(),
        }
    }
}

/// Audio devices and levels. Nothing here enables any processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// Input device id; `None` is the system default.
    pub input_device: Option<String>,
    /// Output device id; `None` is the system default.
    pub output_device: Option<String>,
    /// Requested Opus bit rate in kbit/s.
    pub bitrate_kbps: u32,
    /// Send two channels.
    pub stereo: bool,
    /// Longest frame in ms the user accepts (latency cap).
    pub max_frame_ms: u32,
    /// `stereo`, `left`, `right` or `mix`.
    pub channel_map: String,
    /// Input gain, 1.0 is unity.
    pub input_gain: f32,
    /// Master output volume, 1.0 is unity.
    pub master_gain: f32,
    /// `ptt`, `vox` or `continuous`.
    pub tx_mode: String,
    /// VOX threshold in dBFS.
    pub vox_threshold_db: f32,
    /// VOX hang time in ms.
    pub vox_hang_ms: u32,
    /// Fold the output to mono.
    pub mono_output: bool,
    /// Send audio over TCP only.
    pub force_tcp: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            bitrate_kbps: 128,
            stereo: true,
            max_frame_ms: 40,
            channel_map: "stereo".into(),
            input_gain: 1.0,
            master_gain: 1.0,
            tx_mode: "ptt".into(),
            vox_threshold_db: -40.0,
            vox_hang_ms: 400,
            mono_output: false,
            force_tcp: false,
        }
    }
}

impl AudioSettings {
    /// The channel map as an engine value.
    #[must_use]
    pub fn map(&self) -> ChannelMap {
        match self.channel_map.as_str() {
            "left" => ChannelMap::Left,
            "right" => ChannelMap::Right,
            "mix" => ChannelMap::Mix,
            _ => ChannelMap::Stereo,
        }
    }

    /// The transmit mode as an engine value.
    #[must_use]
    pub fn mode(&self) -> TxMode {
        match self.tx_mode.as_str() {
            "vox" => TxMode::Vox,
            "continuous" => TxMode::Continuous,
            _ => TxMode::Ptt,
        }
    }
}

/// System-wide hotkeys (accelerator strings such as `Ctrl+Shift+Space`) and
/// their targets.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeySettings {
    /// Push to talk.
    pub ptt: String,
    /// Hold to whisper to `whisper_target`.
    pub whisper: String,
    /// Hold to also hear `listen_target`.
    pub listen_in: String,
    /// Channel id to whisper to.
    pub whisper_target: Option<u32>,
    /// Channel id to listen to.
    pub listen_target: Option<u32>,
    /// Mute the home channel while Listen In is held.
    pub mute_home_on_listen: bool,
}

/// Everything that is saved.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Server and login.
    pub server: ServerSettings,
    /// Audio.
    pub audio: AudioSettings,
    /// Hotkeys.
    pub hotkeys: HotkeySettings,
    /// PTT hardware.
    pub ptt: PttSettings,
    /// UI language code.
    pub language: String,
    /// UI theme: `dark` or `light`.
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server: ServerSettings::default(),
            audio: AudioSettings::default(),
            hotkeys: HotkeySettings::default(),
            ptt: PttSettings::default(),
            language: String::new(),
            theme: "dark".into(),
        }
    }
}

impl Settings {
    /// Load from `path`, falling back to defaults when missing or unreadable.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Write to `path`, creating the directory if needed.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }
}

/// Where settings live inside the app config directory.
#[must_use]
pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json_fills_defaults() {
        let s: Settings = serde_json::from_str(r#"{"audio":{"bitrate_kbps":96}}"#).expect("parse");
        assert_eq!(s.audio.bitrate_kbps, 96);
        assert!((s.audio.input_gain - 1.0).abs() < f32::EPSILON);
        assert_eq!(s.server.port, 54736);
    }

    #[test]
    fn defaults_apply_no_gain_or_processing() {
        let a = AudioSettings::default();
        assert!((a.input_gain - 1.0).abs() < f32::EPSILON);
        assert!((a.master_gain - 1.0).abs() < f32::EPSILON);
        assert!(a.stereo);
    }

    #[test]
    fn round_trips_through_a_file() {
        let dir = std::env::temp_dir().join("rr-app-settings-test");
        let path = settings_path(&dir);
        let mut s = Settings::default();
        s.server.username = "W1AW".into();
        s.save(&path).expect("save");
        assert_eq!(Settings::load(&path).server.username, "W1AW");
        let _ = std::fs::remove_dir_all(dir);
    }
}
