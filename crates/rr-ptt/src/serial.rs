//! Real RS-232 PTT I/O (behind the `serial` feature).
//!
//! Two directions, as §14d lays out:
//!
//! - **Station — output.** [`SerialPttSink`] drives DTR or RTS to key the rig
//!   via an optocoupler/open-collector stage. ⚠️ Many USB-serial chips assert
//!   DTR the instant the port opens, which would key the rig on app start, so
//!   the sink **explicitly deasserts both DTR and RTS before anything else** and
//!   again on drop.
//! - **Operator — input.** [`SerialPttSource`] senses a footswitch looped onto a
//!   readable line (CTS/DSR/CD/RI). There is no portable interrupt for control-
//!   line changes, so the application polls this at ~10-20 ms (imperceptible for
//!   PTT, negligible cost).
//!
//! An asserted line reads/writes as logic high. Whether "footswitch pressed" is
//! high or low depends on the interface wiring, so [`SerialPttConfig::active_low`]
//! lets the operator invert the sense.

use std::time::Duration;

use serialport::SerialPort;

use crate::{PttError, PttIntent, PttLine, PttSink, PttSource, Result};

/// A writable control line a [`SerialPttSink`] may drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlLine {
    /// Data Terminal Ready.
    Dtr,
    /// Request To Send.
    Rts,
}

/// A readable control line a [`SerialPttSource`] may sense.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Configuration shared by the serial sink and source.
#[derive(Debug, Clone)]
pub struct SerialPttConfig {
    /// OS port name (e.g. `"COM5"` on Windows, `"/dev/ttyUSB0"` on Linux).
    pub port: String,
    /// Invert the active sense: when `true`, "keyed"/"pressed" is the *deasserted*
    /// (low) line level. Default interfaces are active-high.
    pub active_low: bool,
}

/// Open a serial port for control-line use.
///
/// Baud rate is irrelevant — only the modem control lines are used, no data is
/// transferred — but the API requires one, so a conventional value is passed.
/// A short timeout keeps control-line reads from blocking the poll loop.
fn open(port: &str) -> Result<Box<dyn SerialPort>> {
    serialport::new(port, 9600)
        .timeout(Duration::from_millis(10))
        .open()
        .map_err(|e| PttError::Open(format!("{port}: {e}")))
}

/// Station-side PTT output: drives a control line to key the transmitter.
pub struct SerialPttSink {
    port: Box<dyn SerialPort>,
    line: ControlLine,
    active_low: bool,
}

// `Box<dyn SerialPort>` is not `Debug`; implement it by hand so the public type
// satisfies the workspace lint without leaking the port handle's internals.
impl std::fmt::Debug for SerialPttSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerialPttSink")
            .field("line", &self.line)
            .field("active_low", &self.active_low)
            .finish_non_exhaustive()
    }
}

impl SerialPttSink {
    /// Open `config.port` and immediately deassert both DTR and RTS so the rig
    /// is **not** keyed by the mere act of opening the port, then take ownership
    /// of `line` for keying.
    ///
    /// # Errors
    /// Returns [`PttError::Open`] if the port cannot be opened and
    /// [`PttError::Io`] if the initial safe (unkeyed) state cannot be set.
    pub fn open(config: &SerialPttConfig, line: ControlLine) -> Result<Self> {
        let mut port = open(&config.port)?;
        // Safe state first, before anything can observe the line. Both lines, so
        // whichever the hardware watches is low regardless of `line`.
        port.write_data_terminal_ready(false)
            .map_err(|e| PttError::Io(format!("initial DTR deassert: {e}")))?;
        port.write_request_to_send(false)
            .map_err(|e| PttError::Io(format!("initial RTS deassert: {e}")))?;
        let mut sink = Self { port, line, active_low: config.active_low };
        // Establish the unkeyed level on the chosen line explicitly too.
        sink.drive(false)?;
        Ok(sink)
    }

    /// Drive the chosen line to represent `keyed`, honouring `active_low`.
    fn drive(&mut self, keyed: bool) -> Result<()> {
        let level = keyed != self.active_low; // active_low inverts
        let r = match self.line {
            ControlLine::Dtr => self.port.write_data_terminal_ready(level),
            ControlLine::Rts => self.port.write_request_to_send(level),
        };
        r.map_err(|e| PttError::Io(format!("drive {:?}={level}: {e}", self.line)))
    }
}

impl PttSink for SerialPttSink {
    fn set_keyed(&mut self, down: bool) -> Result<()> {
        self.drive(down)
    }
}

impl Drop for SerialPttSink {
    fn drop(&mut self) {
        // Guarantee the rig is unkeyed when we let go of the port, rather than
        // trusting the adapter to deassert on close.
        let unkeyed = self.active_low; // the "unkeyed" logic level
        let _ = match self.line {
            ControlLine::Dtr => self.port.write_data_terminal_ready(unkeyed),
            ControlLine::Rts => self.port.write_request_to_send(unkeyed),
        };
    }
}

/// Operator-side PTT input: senses a footswitch on a readable control line.
pub struct SerialPttSource {
    port: Box<dyn SerialPort>,
    line: SenseLine,
    active_low: bool,
}

impl std::fmt::Debug for SerialPttSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerialPttSource")
            .field("line", &self.line)
            .field("active_low", &self.active_low)
            .finish_non_exhaustive()
    }
}

impl SerialPttSource {
    /// Open `config.port` for sensing `line`.
    ///
    /// # Errors
    /// Returns [`PttError::Open`] if the port cannot be opened.
    pub fn open(config: &SerialPttConfig, line: SenseLine) -> Result<Self> {
        // Deassert our outputs so a loopback interface sees a defined idle state
        // and we never key anything from the operator side.
        let mut port = open(&config.port)?;
        let _ = port.write_data_terminal_ready(false);
        let _ = port.write_request_to_send(false);
        Ok(Self { port, line, active_low: config.active_low })
    }

    fn read_level(&mut self) -> serialport::Result<bool> {
        match self.line {
            SenseLine::Cts => self.port.read_clear_to_send(),
            SenseLine::Dsr => self.port.read_data_set_ready(),
            SenseLine::Cd => self.port.read_carrier_detect(),
            SenseLine::Ri => self.port.read_ring_indicator(),
        }
    }
}

impl PttSource for SerialPttSource {
    fn poll(&mut self) -> Option<PttIntent> {
        match self.read_level() {
            Ok(level) => {
                let pressed = level != self.active_low; // active_low inverts
                Some(if pressed { PttIntent::Key } else { PttIntent::Unkey })
            }
            // A failed read this cycle is "no reading", never "unkey": dropping to
            // unkey on a transient glitch would stutter the transmission.
            Err(e) => {
                tracing::trace!("serial PTT sense read failed (treated as no change): {e}");
                None
            }
        }
    }
}

/// Enumerate the serial ports present on the system, as `(name, label)` pairs.
///
/// The label is a human-friendly description when the OS provides one (USB
/// product string, etc.), falling back to the port name. Sorted by name so the
/// UI list is stable.
///
/// # Errors
/// Returns [`PttError::Io`] if the OS port enumeration fails.
pub fn available_ports() -> Result<Vec<(String, String)>> {
    let mut ports = serialport::available_ports()
        .map_err(|e| PttError::Io(format!("enumerate ports: {e}")))?
        .into_iter()
        .map(|p| {
            let label = describe(&p);
            (p.port_name, label)
        })
        .collect::<Vec<_>>();
    ports.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(ports)
}

/// Human-friendly label for a port: USB product/manufacturer when known.
fn describe(p: &serialport::SerialPortInfo) -> String {
    match &p.port_type {
        serialport::SerialPortType::UsbPort(info) => {
            let product = info.product.clone().unwrap_or_default();
            let manuf = info.manufacturer.clone().unwrap_or_default();
            let desc = format!("{manuf} {product}").trim().to_owned();
            if desc.is_empty() {
                p.port_name.clone()
            } else {
                format!("{} ({desc})", p.port_name)
            }
        }
        _ => p.port_name.clone(),
    }
}

/// Map the role-neutral [`PttLine`] onto a [`ControlLine`] (output).
///
/// Returns `None` for a readable-only line, so a misconfiguration (asking to
/// *drive* CTS) is refused rather than silently misbehaving.
#[must_use]
pub fn control_line(line: PttLine) -> Option<ControlLine> {
    match line {
        PttLine::Dtr => Some(ControlLine::Dtr),
        PttLine::Rts => Some(ControlLine::Rts),
        _ => None,
    }
}

/// Map the role-neutral [`PttLine`] onto a [`SenseLine`] (input).
///
/// Returns `None` for a writable-only line.
#[must_use]
pub fn sense_line(line: PttLine) -> Option<SenseLine> {
    match line {
        PttLine::Cts => Some(SenseLine::Cts),
        PttLine::Dsr => Some(SenseLine::Dsr),
        PttLine::Cd => Some(SenseLine::Cd),
        PttLine::Ri => Some(SenseLine::Ri),
        _ => None,
    }
}
