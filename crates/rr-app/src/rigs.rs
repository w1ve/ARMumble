//! CAT settings for the station's radios, kept in `rigs.json` in the app
//! config folder (next to `settings.json`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The CAT settings of one radio. Mirrors `rr_cat::RigConfig` so the file can
/// be read and written on platforms without CAT support too.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rig {
    /// Model: `k4`.
    pub model: String,
    /// Elecraft K4 settings.
    pub k4: K4,
}

/// Elecraft K4 settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct K4 {
    /// `serial` or `tcp`.
    pub link: String,
    /// Serial port (38400 8N1).
    pub serial_port: String,
    /// Address or DNS name.
    pub host: String,
    /// Network port.
    pub port: u16,
    /// Radio password (K4 Remote). Empty for plain CAT.
    pub password: String,
}

impl Default for K4 {
    fn default() -> Self {
        Self { link: "serial".into(), serial_port: String::new(), host: String::new(), port: 9205, password: String::new() }
    }
}

/// Both radios of a station.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rigs {
    /// Radio 1 (the only radio of a single-radio station).
    pub radio1: Rig,
    /// Radio 2 of a two-radio station.
    pub radio2: Rig,
}

/// Result of keying a radio briefly to test CAT PTT.
#[derive(Debug, Clone, Serialize)]
pub struct KeyTest {
    /// The radio said it was transmitting.
    pub transmitted: bool,
    /// What the radio sent during the test.
    pub replies: String,
}

/// Where the file lives.
#[must_use]
pub fn rigs_path(config_dir: &Path) -> PathBuf {
    config_dir.join("rigs.json")
}

/// Read the file; a missing file gives defaults (it is written on first save).
pub fn load(path: &Path) -> Result<Rigs, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Rigs::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Write the file.
pub fn save(path: &Path, rigs: &Rigs) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(rigs).map_err(|e| e.to_string())?;
    std::fs::write(path, json + "\n").map_err(|e| e.to_string())
}

/// Check one radio's settings before use.
pub fn validate(r: &Rig) -> Result<(), String> {
    match r.model.as_str() {
        "" | "k4" => {
            let k = &r.k4;
            match k.link.as_str() {
                "serial" if k.serial_port.trim().is_empty() => Err("Elecraft K4: choose a serial port".into()),
                "serial" => Ok(()),
                "tcp" if k.host.trim().is_empty() => Err("Elecraft K4: enter the radio's address".into()),
                "tcp" if k.port == 0 => Err("Elecraft K4: port must be 1 to 65535".into()),
                "tcp" => Ok(()),
                other => Err(format!("Elecraft K4: unknown connection type {other:?}")),
            }
        }
        other => Err(format!("unknown radio model {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = std::env::temp_dir().join(format!("rr-rigs-{}", std::process::id()));
        let path = rigs_path(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load(&path).expect("missing file"), Rigs::default());
        let r = Rigs {
            radio1: Rig {
                model: "k4".into(),
                k4: K4 { link: "tcp".into(), host: "k4.local".into(), password: "pw".into(), ..K4::default() },
            },
            ..Rigs::default()
        };
        save(&path, &r).expect("save");
        assert_eq!(load(&path).expect("load"), r);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validation() {
        let rig = |link: &str, port: &str, host: &str| Rig {
            model: "k4".into(),
            k4: K4 { link: link.into(), serial_port: port.into(), host: host.into(), ..K4::default() },
        };
        assert!(validate(&rig("serial", "COM5", "")).is_ok());
        assert!(validate(&rig("serial", "", "")).is_err());
        assert!(validate(&rig("tcp", "", "10.0.0.9")).is_ok());
        assert!(validate(&rig("tcp", "", " ")).is_err());
        assert!(validate(&Rig { model: "ic7610".into(), ..Rig::default() }).is_err());
    }
}
