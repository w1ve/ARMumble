//! Check that remote PTT reaches another client: two clients join CHANNEL,
//! A holds PTT (default 3 s), B prints every remote PTT it receives.
//! Use a channel where no station listens, or a real radio will key.
//!
//! `cargo run -p rr-client --example ptt_relay -- HOST PORT CHANNEL [PASSWORD] [HOLD_SECONDS]`

use std::time::{Duration, Instant};

use rr_client::{Event, RadioClient, Scope};

// Library dependencies the example does not use directly.
use rr_audio as _;
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
    let id = c.channels().into_iter().find(|x| x.name == channel).map(|x| x.id).expect("channel");
    c.join_channel(id).await.expect("join");
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!("{name}: session {:?} in channel {:?}", c.own_session(), c.own_channel());
    (c, audio)
}

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (host, port, channel, pw) = (a[1].clone(), a[2].parse::<u16>().expect("port"), a[3].clone(), a.get(4).filter(|p| !p.is_empty()));
    let hold: u64 = a.get(5).and_then(|h| h.parse().ok()).unwrap_or(3);
    let (rx_client, _ra) = join("rr-ptt-rx", &host, port, pw, &channel).await;
    let (tx_client, _ta) = join("rr-ptt-tx", &host, port, pw, &channel).await;
    let mut ev = rx_client.subscribe();
    let t0 = Instant::now();
    let watcher = tokio::spawn(async move {
        let mut n = 0;
        while t0.elapsed() < Duration::from_secs(hold + 3) {
            if let Ok(Ok(Event::RemotePtt { session, down })) = tokio::time::timeout(Duration::from_millis(200), ev.recv()).await {
                n += 1;
                println!("{:>5.2}s rx got REMOTE PTT from session {session}: {}", t0.elapsed().as_secs_f32(), if down { "DOWN" } else { "up" });
            }
        }
        n
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    println!("{:>5.2}s tx PTT down", t0.elapsed().as_secs_f32());
    tx_client.set_ptt(true);
    tokio::time::sleep(Duration::from_secs(hold)).await;
    println!("{:>5.2}s tx PTT up", t0.elapsed().as_secs_f32());
    tx_client.set_ptt(false);
    let n = watcher.await.unwrap_or(0);
    println!("received {n} remote PTT events");
    tx_client.disconnect().await;
    rx_client.disconnect().await;
}
