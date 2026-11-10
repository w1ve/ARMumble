//! Read-only probe against a live server: connect, print the visible roster,
//! the bandwidth plan and the scope behaviour, then leave.
//!
//! `cargo run -p rr-client --example probe -- HOST PORT PASSWORD [SCOPE_CHANNEL]`

use std::time::Duration;

use rr_client::{Event, RadioClient, Scope};

// Library dependencies the example does not use directly.
use rr_audio as _;
use rr_mumble as _;
use rr_radio as _;
use thiserror as _;

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().collect();
    let host = a.get(1).map_or("newyork.remote.radio", String::as_str);
    let port: u16 = a.get(2).and_then(|p| p.parse().ok()).unwrap_or(54736);
    let mut b = RadioClient::builder().server(host, port).username("rr-probe");
    if let Some(pw) = a.get(3) {
        b = b.password(pw.clone());
    }
    if let Some(s) = a.get(4) {
        b = b.scope(Scope::Channel(s.clone()));
    }
    let (client, _audio) = b.build().expect("build");
    let mut rx = client.subscribe();
    let _task = client.start();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    while let Ok(Ok(ev)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        println!("event: {ev:?}");
        if matches!(ev, Event::Rejected { .. }) {
            break;
        }
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    println!("own session {:?}, channel {:?}", client.own_session(), client.own_channel());
    println!("plan: {:?}", client.bandwidth_plan());
    for c in client.channels() {
        println!("channel {:>4} parent {:?} {}", c.id, c.parent_id, c.name);
    }
    for u in client.users() {
        println!("user {:>5} in {:>4}: {}", u.session, u.channel_id, u.name);
    }
    client.disconnect().await;
}
