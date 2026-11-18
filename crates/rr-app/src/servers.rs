//! The user's server list, kept in `server-list.json` in the app config folder.
//! (Not `servers.json`: the older Remote.Radio app uses that name, in another format, in the same folder.)
//!
//! On first run (no file yet) the list is seeded from `servers.default.json`,
//! which ships with the app. After that the file belongs to the user: an empty
//! list stays empty, and nothing in the program re-adds servers.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The list written on first run.
const DEFAULT_SERVERS: &str = include_str!("../servers.default.json");

/// One server the user can connect to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerEntry {
    /// Display name.
    pub name: String,
    /// IP address or DNS name.
    pub host: String,
    /// TCP/UDP port.
    pub port: u16,
    /// Server password (may be empty).
    pub password: String,
}

impl Default for ServerEntry {
    fn default() -> Self {
        Self { name: String::new(), host: String::new(), port: 64738, password: String::new() }
    }
}

/// Where the list lives inside the app config folder.
#[must_use]
pub fn servers_path(config_dir: &Path) -> PathBuf {
    config_dir.join("server-list.json")
}

/// The list shipped with the app.
#[must_use]
pub fn defaults() -> Vec<ServerEntry> {
    serde_json::from_str(DEFAULT_SERVERS).unwrap_or_default()
}

/// Read the list, seeding the file with the defaults if it does not exist.
/// An unreadable file is reported and left untouched (never overwritten).
pub fn load(path: &Path) -> Result<Vec<ServerEntry>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let list = defaults();
            save(path, &list)?;
            Ok(list)
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Check and tidy a list: names and hosts are trimmed and required, ports
/// must be non-zero, and names must be unique (case-insensitive).
pub fn validate(list: &[ServerEntry]) -> Result<Vec<ServerEntry>, String> {
    let mut out: Vec<ServerEntry> = Vec::with_capacity(list.len());
    for (i, s) in list.iter().enumerate() {
        let row = i + 1;
        let e = ServerEntry {
            name: s.name.trim().to_owned(),
            host: s.host.trim().to_owned(),
            port: s.port,
            password: s.password.clone(),
        };
        if e.name.is_empty() {
            return Err(format!("server {row}: name is empty"));
        }
        if e.host.is_empty() {
            return Err(format!("{}: address is empty", e.name));
        }
        if e.host.chars().any(char::is_whitespace) {
            return Err(format!("{}: address contains spaces", e.name));
        }
        if e.port == 0 {
            return Err(format!("{}: port must be 1 to 65535", e.name));
        }
        if out.iter().any(|o| o.name.eq_ignore_ascii_case(&e.name)) {
            return Err(format!("{}: name is used twice", e.name));
        }
        out.push(e);
    }
    Ok(out)
}

/// Write the list, creating the folder if needed.
pub fn save(path: &Path, list: &[ServerEntry]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
    std::fs::write(path, json + "\n").map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, host: &str, port: u16) -> ServerEntry {
        ServerEntry { name: name.into(), host: host.into(), port, password: String::new() }
    }

    #[test]
    fn shipped_defaults_are_valid() {
        let d = defaults();
        assert_eq!(d.len(), 14);
        assert!(validate(&d).is_ok());
    }

    #[test]
    fn first_run_seeds_then_user_owns_the_file() {
        let dir = std::env::temp_dir().join(format!("rr-servers-{}", std::process::id()));
        let path = servers_path(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load(&path).expect("seed").len(), 14);
        save(&path, &[]).expect("empty");
        assert!(load(&path).expect("reload").is_empty(), "an emptied list must stay empty");
        save(&path, &[entry("Mine", "10.0.0.5", 54736)]).expect("save");
        assert_eq!(load(&path).expect("reload"), vec![entry("Mine", "10.0.0.5", 54736)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_file_is_reported_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("rr-servers-bad-{}", std::process::id()));
        let path = servers_path(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(&path, "{ not json").expect("write");
        assert!(load(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{ not json");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validation_rules() {
        assert!(validate(&[entry(" A ", " h ", 1)]).is_ok_and(|v| v[0].name == "A" && v[0].host == "h"));
        assert!(validate(&[entry("", "h", 1)]).is_err());
        assert!(validate(&[entry("A", "", 1)]).is_err());
        assert!(validate(&[entry("A", "a b", 1)]).is_err());
        assert!(validate(&[entry("A", "h", 0)]).is_err());
        assert!(validate(&[entry("A", "h", 1), entry("a", "x", 2)]).is_err());
    }
}
