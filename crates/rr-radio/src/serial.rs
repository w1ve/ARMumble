//! Real RS-232 backends (feature `serial`).

use rr_ptt::{ControlLine, SenseLine as HwSense, SerialPttConfig, SerialPttSink, SerialPttSource};

use crate::message::PttMessage;
use crate::runner::{LocalPtt, StationPtt};
use crate::settings::{KeyLine, LocalPttSettings, SenseLine, StationPttSettings};

pub use rr_ptt::available_ports;

/// Open the station's key line and start the keyer.
///
/// # Errors
/// Returns a message if the port cannot be opened or the worker not started.
pub fn open_station(settings: &StationPttSettings) -> Result<StationPtt, String> {
    if settings.port.is_empty() {
        return Err("no serial port selected for Station PTT".into());
    }
    let line = match settings.key_line {
        KeyLine::Dtr => ControlLine::Dtr,
        KeyLine::Rts => ControlLine::Rts,
    };
    let cfg = SerialPttConfig { port: settings.port.clone(), active_low: settings.active_low };
    let sink = SerialPttSink::open(&cfg, line).map_err(|e| format!("open Station PTT: {e}"))?;
    StationPtt::spawn(sink, settings).map_err(|e| format!("start Station PTT: {e}"))
}

/// Open the footswitch input and start polling it.
///
/// # Errors
/// Returns a message if the port cannot be opened or the worker not started.
pub fn open_local<F>(settings: &LocalPttSettings, on_message: F) -> Result<LocalPtt, String>
where
    F: Fn(PttMessage) + Send + 'static,
{
    if settings.port.is_empty() {
        return Err("no serial port selected for Local PTT".into());
    }
    let line = match settings.sense_line {
        SenseLine::Cts => HwSense::Cts,
        SenseLine::Dsr => HwSense::Dsr,
        SenseLine::Cd => HwSense::Cd,
        SenseLine::Ri => HwSense::Ri,
    };
    let cfg = SerialPttConfig { port: settings.port.clone(), active_low: settings.active_low };
    let source = SerialPttSource::open(&cfg, line).map_err(|e| format!("open Local PTT: {e}"))?;
    LocalPtt::spawn(source, on_message).map_err(|e| format!("start Local PTT: {e}"))
}
