//! Check Listen In and Whisper between two clients in different channels.
//!
//! A sits in CHANNEL_A, B in CHANNEL_B.
//! 1. Listen In: A sends a tone (always on); B holds Listen In on CHANNEL_A.
//! 2. Whisper: B holds Whisper to CHANNEL_A (no PTT); A should hear B.
//!
//! `cargo run -p rr-client --example hold_check -- HOST PORT CHANNEL_A CHANNEL_B [PASSWORD]`

use std::time::Duration;

use rr_audio::capture::TxMode;
use rr_client::{AudioEndpoints, Event, RadioClient, Scope};

// Library dependencies the example does not use directly.
use rr_mumble as _;
use rr_radio as _;
use thiserror as _;

async fn join(name: &str, host: &str, port: u16, pw: Option<&String>, channel: &str) -> (RadioClient, AudioEndpoints, u32) {
    let mut b = RadioClient::builder().server(host, port).username(name).scope(Scope::Everything);
    if let Some(p) = pw {
        b = b.password(p.clone());
    }
    let (c, audio) = b.build().expect("build");
    let mut ev = c.subscribe();
    let _t = c.start();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while let Ok(Ok(e)) = tokio::time::timeout_at(deadline, ev.recv()).await {
        if let Event::Rejected { reason } = &e {
            panic!("{name}: rejected: {reason}");
        }
        if matches!(e, Event::Connected { .. }) {
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let id = c.channels().into_iter().find(|x| x.name == channel).map(|x| x.id).expect("channel");
    c.join_channel(id).await.expect("join");
    tokio::time::sleep(Duration::from_millis(700)).await;
    println!("{name}: session {:?} channel {:?}", c.own_session(), c.own_channel());
    (c, audio, id)
}

/// Run both clients' audio for `secs`: `a_talks`/`b_talks` feed a tone.
async fn pump(a: &mut AudioEndpoints, b: &mut AudioEndpoints, a_talks: bool, b_talks: bool, secs: u64) {
    let mut tone = vec![0.0_f32; 960];
    let silence = vec![0.0_f32; 960];
    let mut out = vec![0.0_f32; 960];
    let mut ph = 0.0_f32;
    let start = tokio::time::Instant::now();
    let mut n = 0_u64;
    while start.elapsed() < Duration::from_secs(secs) {
        tokio::time::sleep(Duration::from_millis(5)).await;
        let due = u64::try_from(start.elapsed().as_millis() / 10).unwrap_or(0);
        while n < due {
            for f in tone.chunks_exact_mut(2) {
                let s = 0.2 * ph.sin();
                ph += 2.0 * std::f32::consts::PI * 600.0 / 48_000.0;
                f[0] = s;
                f[1] = s;
            }
            a.input.process(if a_talks { &tone } else { &silence });
            b.input.process(if b_talks { &tone } else { &silence });
            a.output.render(&mut out);
            b.output.render(&mut out);
            n += 1;
        }
    }
}

fn heard(c: &RadioClient, from: Option<u32>) -> u64 {
    c.speakers().into_iter().filter(|s| Some(s.speaker) == from).map(|s| s.packets).sum()
}

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (host, port) = (a[1].clone(), a[2].parse::<u16>().expect("port"));
    let pw = a.get(5).filter(|p| !p.is_empty());
    let (ca, mut ea, chan_a) = join("rr-hold-a", &host, port, pw, &a[3]).await;
    let (cb, mut eb, _chan_b) = join("rr-hold-b", &host, port, pw, &a[4]).await;

    // Baseline: different channels, B hears nothing from A.
    ca.capture().set_tx_mode(TxMode::Continuous);
    pump(&mut ea, &mut eb, true, false, 2).await;
    let base = heard(&cb, ca.own_session());
    println!("baseline: B heard {base} packets from A (expect 0)");

    // Listen In.
    match cb.listen_in(chan_a, true, false).await {
        Ok(()) => println!("B listen_in(A's channel) sent"),
        Err(e) => println!("B listen_in failed: {e}"),
    }
    pump(&mut ea, &mut eb, true, false, 1).await;
    let li1 = heard(&cb, ca.own_session()) - base;
    pump(&mut ea, &mut eb, true, false, 2).await;
    let li = heard(&cb, ca.own_session()) - base;
    println!("  after 1 s: {li1} packets");
    println!("LISTEN IN: B heard {li} packets from A while listening (expect about 150)");
    let _ = cb.listen_in(chan_a, false, false).await;
    pump(&mut ea, &mut eb, true, false, 1).await;
    ca.capture().set_tx_mode(TxMode::Ptt);
    pump(&mut ea, &mut eb, false, false, 1).await;

    // Whisper: B holds Whisper to A's channel (no PTT); that alone must transmit.
    let before = heard(&ca, cb.own_session());
    match cb.whisper(chan_a, true).await {
        Ok(()) => println!("B whisper(A's channel) sent"),
        Err(e) => println!("B whisper failed: {e}"),
    }
    pump(&mut ea, &mut eb, false, true, 3).await;
    let _ = cb.whisper(chan_a, false).await;
    pump(&mut ea, &mut eb, false, false, 1).await;
    let wh = heard(&ca, cb.own_session()) - before;
    println!("WHISPER: A heard {wh} packets from B (expect about 150)");
    println!("B sent/dropped {:?}", cb.packet_counts());
    ca.disconnect().await;
    cb.disconnect().await;
}
