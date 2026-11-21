//! Check the always-on station's "only to the talker" mode with three
//! clients in one channel: S (station, always on), A and B (operators).
//! A holds PTT and talks; A should still hear S, B should hear A but not S.
//!
//! `cargo run -p rr-client --example monitor_check -- HOST PORT CHANNEL [PASSWORD]`

use std::time::Duration;

use rr_audio::capture::TxMode;
use rr_client::{AudioEndpoints, Event, OperatorTalk, RadioClient, Scope};

// Library dependencies the example does not use directly.
use rr_mumble as _;
use rr_radio as _;
use thiserror as _;

async fn join(name: &str, host: &str, port: u16, pw: Option<&String>, channel: &str) -> (RadioClient, AudioEndpoints) {
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
    tokio::time::sleep(Duration::from_millis(500)).await;
    (c, audio)
}

/// Run all three for `secs`; S always feeds a tone, A feeds one if `a_talks`.
async fn pump(all: &mut [&mut AudioEndpoints; 3], a_talks: bool, secs: u64) {
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
            for f in tone.as_chunks_mut::<2>().0 {
                let s = 0.2 * ph.sin();
                ph += 2.0 * std::f32::consts::PI * 600.0 / 48_000.0;
                *f = [s, s];
            }
            all[0].input.process(&tone);
            all[1].input.process(if a_talks { &tone } else { &silence });
            all[2].input.process(&silence);
            for e in all.iter_mut() {
                e.output.render(&mut out);
            }
            n += 1;
        }
    }
}

fn heard(c: &RadioClient, from: &RadioClient) -> u64 {
    c.speakers().into_iter().filter(|s| Some(s.speaker) == from.own_session()).map(|s| s.packets).sum()
}

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (host, port, channel) = (a[1].clone(), a[2].parse::<u16>().expect("port"), a[3].clone());
    let pw = a.get(4).filter(|p| !p.is_empty());
    let (s, mut es) = join("rr-mon-station", &host, port, pw, &channel).await;
    let (op_a, mut ea) = join("rr-mon-a", &host, port, pw, &channel).await;
    let (op_b, mut eb) = join("rr-mon-b", &host, port, pw, &channel).await;
    s.capture().set_tx_mode(TxMode::Continuous);
    s.set_operator_talk(OperatorTalk::Talker);

    pump(&mut [&mut es, &mut ea, &mut eb], false, 2).await;
    let (a0, b0) = (heard(&op_a, &s), heard(&op_b, &s));
    println!("idle 2 s: A heard station {a0}, B heard station {b0} (expect about 100 each)");

    op_a.set_ptt(true);
    pump(&mut [&mut es, &mut ea, &mut eb], true, 3).await;
    op_a.set_ptt(false);
    let (a1, b1, ba) = (heard(&op_a, &s) - a0, heard(&op_b, &s) - b0, heard(&op_b, &op_a));
    println!("A talks 3 s: A heard station {a1} (expect about 150), B heard station {b1} (expect near 0), B heard A {ba} (expect about 150)");

    pump(&mut [&mut es, &mut ea, &mut eb], false, 2).await;
    let b2 = heard(&op_b, &s) - b0 - b1;
    println!("after A stops 2 s: B heard station {b2} again (expect about 70)");
    for c in [&s, &op_a, &op_b] {
        c.disconnect().await;
    }
}
