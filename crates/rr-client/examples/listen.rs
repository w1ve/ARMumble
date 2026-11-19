//! Receive-only probe: join a channel, play out what is heard into a WAV file
//! (48 kHz stereo float) and print the receive statistics every second.
//!
//! `cargo run -p rr-client --example listen -- HOST PORT CHANNEL SECONDS OUT.wav [PASSWORD]`

use std::io::Write;
use std::time::Duration;

use rr_client::{RadioClient, Scope};

// Library dependencies the example does not use directly.
use rr_audio as _;
use rr_mumble as _;
use rr_radio as _;
use thiserror as _;

fn write_wav(path: &str, samples: &[f32]) -> std::io::Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let data_len = u32::try_from(samples.len() * 4).unwrap_or(u32::MAX);
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16_u32.to_le_bytes())?;
    f.write_all(&3_u16.to_le_bytes())?; // IEEE float
    f.write_all(&2_u16.to_le_bytes())?;
    f.write_all(&48_000_u32.to_le_bytes())?;
    f.write_all(&(48_000_u32 * 8).to_le_bytes())?;
    f.write_all(&8_u16.to_le_bytes())?;
    f.write_all(&32_u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        f.write_all(&s.to_le_bytes())?;
    }
    f.flush()
}

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().collect();
    let host = a.get(1).cloned().unwrap_or_default();
    let port: u16 = a.get(2).and_then(|p| p.parse().ok()).unwrap_or(54736);
    let channel = a.get(3).cloned().unwrap_or_default();
    let secs: u64 = a.get(4).and_then(|p| p.parse().ok()).unwrap_or(20);
    let out = a.get(5).cloned().unwrap_or_else(|| "listen.wav".into());
    let mut b = RadioClient::builder().server(host, port).username("rr-listen-probe").scope(Scope::Everything);
    if let Some(pw) = a.get(6).filter(|p| !p.is_empty()) {
        b = b.password(pw.clone());
    }
    let (client, mut audio) = b.build().expect("build");
    let mut events = client.subscribe();
    let _task = client.start();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while let Ok(Ok(ev)) = tokio::time::timeout_at(deadline, events.recv()).await {
        println!("event: {ev:?}");
        if matches!(ev, rr_client::Event::Connected { .. } | rr_client::Event::Rejected { .. }) {
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!("plan: {:?}", client.bandwidth_plan());
    let Some(ch) = client.channels().into_iter().find(|c| c.name == channel) else {
        println!("channel {channel:?} not found; channels:");
        for c in client.channels() {
            println!("  {:?}", c.name);
        }
        client.disconnect().await;
        return;
    };
    client.join_channel(ch.id).await.expect("join");
    let only = a.get(7).cloned();
    for u in client.users().into_iter().filter(|u| u.channel_id == ch.id) {
        let muted = only.as_ref().is_some_and(|n| *n != u.name);
        if muted {
            client.set_user_gain(u.session, 0.0);
        }
        println!("in channel: {} (session {}){}", u.name, u.session, if muted { " muted" } else { "" });
    }
    let names: std::collections::HashMap<u32, String> = client.users().into_iter().map(|u| (u.session, u.name)).collect();
    let mut ptt_events = client.subscribe();
    let t0 = std::time::Instant::now();
    let _ptt_log = tokio::spawn(async move {
        loop {
            match ptt_events.recv().await {
                Ok(rr_client::Event::RemotePtt { session, down }) => {
                    println!("{:>6.2}s REMOTE PTT from {} : {}", t0.elapsed().as_secs_f32(), names.get(&session).map_or("?", String::as_str), if down { "DOWN" } else { "up" });
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    let mut rec: Vec<f32> = Vec::with_capacity(48_000 * 2 * usize::try_from(secs).unwrap_or(20));
    // Render in 10 ms blocks paced by the real clock (Windows timers are coarse).
    let mut buf = vec![0.0_f32; 960];
    let start = tokio::time::Instant::now();
    let mut n = 0_u64;
    while start.elapsed() < Duration::from_secs(secs) {
        tokio::time::sleep(Duration::from_millis(5)).await;
        let due = u64::try_from(start.elapsed().as_millis() / 10).unwrap_or(0);
        while n < due {
            audio.output.render(&mut buf);
            rec.extend_from_slice(&buf);
            n += 1;
            if n % 100 != 0 {
                continue;
            }
            for s in client.speakers() {
                println!(
                    "t={:>3}s spk {} act {} depth {:.0}/{:.0} ms jit {:.0} ms ppm {:+.0} pkts {} lost {} late {} under {} over {} resync {}",
                    n / 100, s.speaker, s.active, s.depth_ms, s.target_ms, s.jitter_ms, s.ratio_ppm, s.packets, s.lost, s.late, s.underruns, s.overflows, s.resyncs
                );
            }
            println!("      sent/dropped {:?}", client.packet_counts());
        }
    }
    client.disconnect().await;
    write_wav(&out, &rec).expect("write wav");
    println!("wrote {out} ({} s)", rec.len() / 96_000);
}
