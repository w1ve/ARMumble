//! End-to-end tests against fake K4s on localhost.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rr_ptt::PttSink;

use super::k4::wire::{cat_frame, password_hash, CatSplitter, FrameReader, END, START};
use super::k4::{K4Config, K4Link};
use super::{test_connection, CatPtt, RigConfig, RigModel};

type Log = Arc<Mutex<Vec<String>>>;

fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if ok() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

fn has(log: &Log, cmd: &str) -> bool {
    log.lock().is_ok_and(|l| l.iter().any(|c| c == cmd))
}

fn count(log: &Log, cmd: &str) -> usize {
    log.lock().map_or(0, |l| l.iter().filter(|c| *c == cmd).count())
}

fn cfg(port: u16, password: &str) -> RigConfig {
    RigConfig {
        model: RigModel::K4,
        k4: K4Config { link: K4Link::Tcp, serial_port: String::new(), host: "127.0.0.1".into(), port, password: password.into() },
    }
}

/// A fake K4 that answers `TQ;` from its own key state and records commands.
/// `remote` selects the K4 Remote protocol (login hash + framing).
fn serve(stream: TcpStream, log: &Log, remote: Option<&str>, stop_after: Option<usize>) {
    let mut s = stream;
    s.set_read_timeout(Some(Duration::from_millis(50))).expect("timeout");
    if let Some(pw) = remote {
        let mut hash = vec![0_u8; 96];
        if s.read_exact(&mut hash).is_err() || hash != password_hash(pw).as_bytes() {
            return; // wrong password: stay silent, like the radio
        }
        // Accept the login by streaming (an audio frame, as the radio does).
        let mut audio = START.to_vec();
        audio.extend_from_slice(&4_u32.to_be_bytes());
        audio.extend_from_slice(&[1, 1, 0, 3]);
        audio.extend_from_slice(&END);
        let _ = s.write_all(&audio);
    }
    let mut frames = FrameReader::default();
    let mut text = CatSplitter::default();
    let mut tx = false;
    let mut seen = 0;
    let mut buf = [0_u8; 4096];
    loop {
        let n = match s.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => n,
            Err(_) => continue,
        };
        let cmds = if remote.is_some() { frames.feed(&buf[..n]) } else { text.feed(&buf[..n]) };
        for c in cmds {
            let reply = match c.as_str() {
                "TX;" => {
                    tx = true;
                    None
                }
                "RX;" => {
                    tx = false;
                    None
                }
                "TQ;" => Some(if tx { "TQ1;" } else { "TQ0;" }.to_owned()),
                "ID;" => Some("ID017;".to_owned()),
                "PING;" => Some("PONG;".to_owned()),
                _ => None,
            };
            if let Ok(mut l) = log.lock() {
                l.push(c);
            }
            seen += 1;
            if let Some(r) = reply {
                let bytes = if remote.is_some() { cat_frame(&r) } else { r.into_bytes() };
                let _ = s.write_all(&bytes);
            }
            if stop_after.is_some_and(|k| seen >= k) {
                return; // drop the connection
            }
        }
    }
}

fn fake(remote: Option<&'static str>, drop_first_after: Option<usize>) -> (u16, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let log: Log = Arc::default();
    let l = Arc::clone(&log);
    let _ = thread::spawn(move || {
        let mut first = true;
        for stream in listener.incoming().flatten() {
            let stop = if first { drop_first_after } else { None };
            first = false;
            serve(stream, &l, remote, stop);
        }
    });
    (port, log)
}

#[test]
fn plain_cat_keys_unkeys_and_reports_the_radio() {
    let (port, log) = fake(None, None);
    let mut rig = CatPtt::open(&cfg(port, "")).expect("open");
    let mon = rig.monitor();
    wait_for("connect", || mon.status().connected);
    rig.set_keyed(true).expect("key");
    wait_for("TX;", || has(&log, "TX;"));
    wait_for("radio reports TX", || mon.status().radio_tx == Some(true));
    rig.set_keyed(false).expect("unkey");
    wait_for("RX;", || has(&log, "RX;"));
    wait_for("radio reports RX", || mon.status().radio_tx == Some(false));
    drop(rig);
    thread::sleep(Duration::from_millis(100));
    assert_eq!(count(&log, "RX;"), 1, "no extra unkey when we are not keyed");
}

#[test]
fn remote_protocol_logs_in_frames_cat_and_keeps_alive() {
    let (port, log) = fake(Some("tester"), None);
    let mut rig = CatPtt::open(&cfg(port, "tester")).expect("open");
    let mon = rig.monitor();
    wait_for("connect", || mon.status().connected);
    wait_for("PING;", || has(&log, "PING;"));
    rig.set_keyed(true).expect("key");
    wait_for("radio reports TX", || mon.status().radio_tx == Some(true));
    drop(rig);
    wait_for("RX; on close", || has(&log, "RX;"));
}

#[test]
fn reconnect_restores_the_key_and_close_releases_it() {
    // The first session drops after 3 commands (TQ;, TX;, TQ;).
    let (port, log) = fake(None, Some(3));
    let mut rig = CatPtt::open(&cfg(port, "")).expect("open");
    let mon = rig.monitor();
    wait_for("connect", || mon.status().connected);
    rig.set_keyed(true).expect("key");
    wait_for("TX; resent after reconnect", || count(&log, "TX;") >= 2);
    drop(rig);
    wait_for("RX; on close", || has(&log, "RX;"));
}

#[test]
fn keying_while_unreachable_is_refused() {
    // Nothing listens on this port.
    let port = TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port();
    let mut rig = CatPtt::open(&cfg(port, "")).expect("open");
    let mon = rig.monitor();
    wait_for("error reported", || mon.status().error.is_some());
    assert!(rig.set_keyed(true).is_err(), "must fail so the watchdog fails closed");
    assert!(rig.set_keyed(false).is_ok());
}

#[test]
fn connection_test_reads_the_identity() {
    let (port, _log) = fake(None, None);
    assert_eq!(test_connection(&cfg(port, "")).expect("test"), "ID017;");
    let (rport, _rlog) = fake(Some("tester"), None);
    assert_eq!(test_connection(&cfg(rport, "tester")).expect("test"), "ID017;");
}

#[test]
fn wrong_password_is_reported() {
    let (port, _log) = fake(Some("tester"), None);
    let err = test_connection(&cfg(port, "wrong")).expect_err("must fail");
    assert!(err.contains("password"), "{err}");
}

#[test]
fn config_file_shape() {
    let c: RigConfig = serde_json::from_str(r#"{"model":"k4","k4":{"link":"tcp","host":"k4.local","password":"x"}}"#).expect("parse");
    assert_eq!(c.k4.port, 9205, "missing fields take defaults");
    assert_eq!(c.k4.link, K4Link::Tcp);
    let back: RigConfig = serde_json::from_str(&serde_json::to_string(&c).expect("json")).expect("parse");
    assert_eq!(back, c);
}

#[test]
fn key_test_reports_transmit_and_always_releases() {
    let (port, log) = fake(None, None);
    let r = super::test_key(&cfg(port, ""), Duration::from_millis(700)).expect("key test");
    assert!(r.transmitted, "replies: {}", r.replies);
    assert!(r.replies.contains("TQ1;"));
    wait_for("RX;", || has(&log, "RX;"));
}
