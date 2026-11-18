//! Connections to a K4: serial port, plain network CAT, or K4 Remote.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use super::wire::{cat_frame, password_hash, CatSplitter, FrameReader};
use super::{K4Config, K4Link};

/// How long one read waits before returning nothing.
const READ_SLICE: Duration = Duration::from_millis(50);
/// Network connect timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// K4 Remote: no frame from the radio this long after the login means the
/// password was wrong (the radio sends no explicit reject).
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);

/// An open connection carrying CAT text.
pub(crate) trait Conn: Send {
    /// Send one or more CAT commands, for example `TX;`.
    fn send(&mut self, text: &str) -> std::io::Result<()>;
    /// Wait briefly for data; returns any complete CAT replies.
    fn recv(&mut self) -> std::io::Result<Vec<String>>;
    /// Whether this link must send `PING;` regularly to stay open.
    fn needs_keepalive(&self) -> bool;
}

/// Open a connection for `cfg`.
pub(crate) fn connect(cfg: &K4Config) -> Result<Box<dyn Conn>, String> {
    match cfg.link {
        K4Link::Serial => Serial::open(&cfg.serial_port).map(|c| Box::new(c) as Box<dyn Conn>),
        K4Link::Tcp if cfg.password.is_empty() => PlainTcp::open(&cfg.host, cfg.port).map(|c| Box::new(c) as Box<dyn Conn>),
        K4Link::Tcp => Remote::open(&cfg.host, cfg.port, &cfg.password).map(|c| Box::new(c) as Box<dyn Conn>),
    }
}

fn would_block(e: &std::io::Error) -> bool {
    matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)
}

struct Serial {
    port: Box<dyn serialport::SerialPort>,
    split: CatSplitter,
}

impl Serial {
    fn open(name: &str) -> Result<Self, String> {
        if name.trim().is_empty() {
            return Err("no serial port selected".into());
        }
        let port = serialport::new(name, 38_400)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(READ_SLICE)
            .open()
            .map_err(|e| format!("{name}: {e}"))?;
        Ok(Self { port, split: CatSplitter::default() })
    }
}

impl Conn for Serial {
    fn send(&mut self, text: &str) -> std::io::Result<()> {
        self.port.write_all(text.as_bytes())?;
        self.port.flush()
    }

    fn recv(&mut self) -> std::io::Result<Vec<String>> {
        let mut buf = [0_u8; 512];
        match self.port.read(&mut buf) {
            Ok(n) => Ok(self.split.feed(&buf[..n])),
            Err(e) if would_block(&e) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    fn needs_keepalive(&self) -> bool {
        false
    }
}

fn tcp(host: &str, port: u16) -> Result<TcpStream, String> {
    if host.trim().is_empty() {
        return Err("no network address set".into());
    }
    let addr = (host.trim(), port)
        .to_socket_addrs()
        .map_err(|e| format!("{host}: {e}"))?
        .next()
        .ok_or_else(|| format!("{host}: address not found"))?;
    let s = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).map_err(|e| format!("{host}:{port}: {e}"))?;
    s.set_nodelay(true).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(READ_SLICE)).map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
    Ok(s)
}

fn read_some(s: &mut TcpStream, buf: &mut [u8]) -> std::io::Result<usize> {
    match s.read(buf) {
        Ok(0) => Err(std::io::Error::new(ErrorKind::ConnectionAborted, "the radio closed the connection")),
        Ok(n) => Ok(n),
        Err(e) if would_block(&e) => Ok(0),
        Err(e) => Err(e),
    }
}

/// The K4's plain CAT text port (no password, no framing).
struct PlainTcp {
    s: TcpStream,
    split: CatSplitter,
}

impl PlainTcp {
    fn open(host: &str, port: u16) -> Result<Self, String> {
        Ok(Self { s: tcp(host, port)?, split: CatSplitter::default() })
    }
}

impl Conn for PlainTcp {
    fn send(&mut self, text: &str) -> std::io::Result<()> {
        self.s.write_all(text.as_bytes())
    }

    fn recv(&mut self) -> std::io::Result<Vec<String>> {
        let mut buf = [0_u8; 2048];
        let n = read_some(&mut self.s, &mut buf)?;
        Ok(self.split.feed(&buf[..n]))
    }

    fn needs_keepalive(&self) -> bool {
        false
    }
}

/// The K4 Remote port (password, framed; also streams audio and spectrum,
/// which are read and discarded).
struct Remote {
    s: TcpStream,
    reader: FrameReader,
    buf: Vec<u8>,
}

impl Remote {
    fn open(host: &str, port: u16, password: &str) -> Result<Self, String> {
        let mut s = tcp(host, port)?;
        s.write_all(password_hash(password).as_bytes()).map_err(|e| format!("login: {e}"))?;
        let mut me = Self { s, reader: FrameReader::default(), buf: vec![0; 64 * 1024] };
        // Any frame from the radio means the login was accepted.
        let deadline = Instant::now() + AUTH_TIMEOUT;
        while me.reader.frames == 0 {
            if Instant::now() > deadline {
                return Err("the radio did not answer the login: check the password".into());
            }
            let _ = me.recv_inner().map_err(|e| {
                if e.kind() == ErrorKind::ConnectionAborted || e.kind() == ErrorKind::ConnectionReset {
                    "the radio closed the connection at login: check the password".to_owned()
                } else {
                    format!("login: {e}")
                }
            })?;
        }
        Ok(me)
    }

    fn recv_inner(&mut self) -> std::io::Result<Vec<String>> {
        let n = read_some(&mut self.s, &mut self.buf)?;
        Ok(self.reader.feed(&self.buf[..n]))
    }
}

impl Conn for Remote {
    fn send(&mut self, text: &str) -> std::io::Result<()> {
        self.s.write_all(&cat_frame(text))
    }

    fn recv(&mut self) -> std::io::Result<Vec<String>> {
        self.recv_inner()
    }

    fn needs_keepalive(&self) -> bool {
        true
    }
}
