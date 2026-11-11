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

/// Background reader for OTRSP commands on a serial port. Dropping it closes
/// the port.
#[derive(Debug)]
pub struct OtrspListener {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl OtrspListener {
    /// Open `port` at 9600 8N1 and call `on_command` for every command.
    /// `?NAME` is answered directly and not forwarded.
    ///
    /// # Errors
    /// Returns a message if the port cannot be opened or the thread started.
    pub fn open<F>(port: &str, mut on_command: F) -> Result<Self, String>
    where
        F: FnMut(crate::OtrspCommand) + Send + 'static,
    {
        use std::io::{ErrorKind, Read};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::Duration;

        let mut serial = serialport::new(port, 9600)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|e| format!("OTRSP port {port}: {e}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let name = port.to_owned();
        let thread = std::thread::Builder::new()
            .name("rr-otrsp".into())
            .spawn(move || {
                let mut parser = crate::OtrspParser::default();
                let mut buf = [0_u8; 256];
                while !flag.load(Ordering::Relaxed) {
                    match serial.read(&mut buf) {
                        Ok(0) => {}
                        Ok(n) => dispatch(&mut parser, &buf[..n], &mut serial, &mut on_command),
                        Err(e) if e.kind() == ErrorKind::TimedOut => {}
                        Err(e) => {
                            tracing::warn!("OTRSP port {name} read failed: {e}");
                            std::thread::sleep(Duration::from_millis(500));
                        }
                    }
                }
            })
            .map_err(|e| format!("start OTRSP listener: {e}"))?;
        Ok(Self { stop, thread: Some(thread) })
    }
}

/// Hand decoded commands to `on_command`, answering `?NAME` on the port.
fn dispatch<W, F>(parser: &mut crate::OtrspParser, bytes: &[u8], port: &mut W, on_command: &mut F)
where
    W: std::io::Write + ?Sized,
    F: FnMut(crate::OtrspCommand),
{
    for cmd in parser.feed(bytes) {
        if cmd == crate::OtrspCommand::QueryName {
            let reply = format!("{}\r", crate::otrsp::DEVICE_NAME);
            let _ = port.write_all(reply.as_bytes());
        } else {
            on_command(cmd);
        }
    }
}

impl Drop for OtrspListener {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
