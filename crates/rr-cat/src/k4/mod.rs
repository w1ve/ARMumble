//! Elecraft K4.
//!
//! PTT on is `TX;`, PTT off is `RX;`. The radio's own report (`TQ;` replies,
//! and `TX;` / `RX;` pushes) is shown so the operator can see whether the
//! radio really transmits.

mod link;
pub mod wire;

use serde::{Deserialize, Serialize};

pub(crate) use link::{connect, Conn};

/// How the K4 is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum K4Link {
    /// RS-232 or USB serial port, 38400 8N1.
    #[default]
    Serial,
    /// Network: K4 Remote (with password) or plain CAT (no password).
    Tcp,
}

/// K4 settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct K4Config {
    /// Serial port or network.
    pub link: K4Link,
    /// Serial port name (serial link).
    pub serial_port: String,
    /// Address or DNS name (network link).
    pub host: String,
    /// Network port: 9205 for K4 Remote, or the plain CAT port.
    pub port: u16,
    /// Radio password for K4 Remote. Empty means plain CAT text.
    pub password: String,
}

impl Default for K4Config {
    fn default() -> Self {
        Self { link: K4Link::Serial, serial_port: String::new(), host: String::new(), port: 9205, password: String::new() }
    }
}

/// Key command.
pub const KEY_ON: &str = "TX;";
/// Unkey command.
pub const KEY_OFF: &str = "RX;";
/// Transmit-state query (reply `TQ0;` or `TQ1;`).
pub const QUERY_TX: &str = "TQ;";
/// Keep-alive for K4 Remote.
pub const PING: &str = "PING;";
/// Identity query used by the connection test.
pub const QUERY_ID: &str = "ID;";
