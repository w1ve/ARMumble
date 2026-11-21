//! Does a client hear itself? Two clients join CHANNEL. A sends a 600 Hz
//! tone for 4 s; both report which sessions they received audio from.
//!
//! `cargo run -p rr-client --example self_echo -- HOST PORT CHANNEL [PASSWORD]`

use std::time::Duration;

use rr_audio::capture::TxMode;
use rr_client::{Event, RadioClient, Scope};

// Library dependencies the example does not use directly.
use rr_mumble as _;
use rr_radio as _;
use thiserror as _;

async fn join(name: &str, host: &str, port: u16, pw: Option<&String>, channel: &str) -> (RadioClient, rr_client::AudioEndpoints) {
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
    if let Some(id) = c.channels().into_iter().find(|x| x.name == channel).map(|x| x.id) {
        c.join_channel(id).await.expect("join");
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!("{name}: session {:?} channel {:?}", c.own_session(), c.own_channel());
    (c, audio)
}

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (host, port, channel, pw) = (a[1].clone(), a[2].parse::<u16>().expect("port"), a[3].clone(), a.get(4).filter(|p| !p.is_empty()));
    let (talker, mut ta) = join("rr-echo-talk", &host, port, pw, &channel).await;
    let (_listener, mut la) = join("rr-echo-hear", &host, port, pw, &channel).await;
    talker.capture().set_tx_mode(TxMode::Continuous);
    let mut frame = vec![0.0_f32; 960];
    let mut out = vec![0.0_f32; 960];
    let mut phase = 0.0_f32;
    let start = tokio::time::Instant::now();
    let mut n = 0_u64;
    while start.elapsed() < Duration::from_secs(4) {
        tokio::time::sleep(Duration::from_millis(5)).await;
        let due = u64::try_from(start.elapsed().as_millis() / 10).unwrap_or(0);
        while n < due {
            for f in frame.chunks_exact_mut(2) {
                let s = 0.2 * phase.sin();
                phase += 2.0 * std::f32::consts::PI * 600.0 / 48_000.0;
                f[0] = s;
                f[1] = s;
            }
            ta.input.process(&frame);
            ta.output.render(&mut out);
            la.output.render(&mut out);
            n += 1;
        }
    }
    talker.capture().set_tx_mode(TxMode::Ptt);
    let own = talker.own_session();
    println!("talker sent {:?} (sent, dropped)", talker.packet_counts());
    for (who, c) in [("talker", &talker), ("listener", &_listener)] {
        for s in c.speakers() {
            println!("{who} received from session {} ({}): {} packets", s.speaker, if Some(s.speaker) == own { "ITSELF" } else { "other" }, s.packets);
        }
    }
}
