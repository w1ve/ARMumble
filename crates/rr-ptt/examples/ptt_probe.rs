//! Hardware probe for the serial PTT path. Build with `--features serial`.
//!
//! It does three things, so a real RS-232 adapter (or loopback) proves the
//! sink and source end to end, through the safety watchdog:
//!
//!   ptt_probe --list
//!       enumerate serial ports and exit.
//!
//!   ptt_probe --sink COM5 [--line dtr|rts] [--active-low]
//!       deassert on open, then key for 1 s / unkey for 1 s, five times,
//!       all through the Watchdog (deadman 2 s, max-tx 120 s). Watch an LED
//!       or a meter on the chosen line.
//!
//!   ptt_probe --source COM5 [--sense cts|dsr|cd|ri] [--active-low]
//!       poll the sense line at 20 ms for 20 s and print every key/unkey edge;
//!       press/release a footswitch and confirm the edges.
//!
//! This is an example, not a test, on purpose: it needs real hardware.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    unused_crate_dependencies,
    reason = "example binary: prints to the console and needs real serial hardware"
)]

use std::time::{Duration, Instant};

use rr_ptt::{
    available_ports, ControlLine, PttIntent, PttSource, SenseLine, SerialPttConfig, SerialPttSink,
    SerialPttSource, Watchdog, WatchdogConfig,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str);

    match mode {
        Some("--list") => list(),
        Some("--sink") => sink(&args),
        Some("--source") => source(&args),
        _ => {
            eprintln!(
                "usage:\n  ptt_probe --list\n  ptt_probe --sink <PORT> [--line dtr|rts] [--active-low]\n  ptt_probe --source <PORT> [--sense cts|dsr|cd|ri] [--active-low]"
            );
            std::process::exit(2);
        }
    }
}

fn opt(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn list() {
    match available_ports() {
        Ok(ports) if !ports.is_empty() => {
            println!("serial ports:");
            for (name, label) in ports {
                println!("  {name}  -  {label}");
            }
        }
        Ok(_) => println!("no serial ports found"),
        Err(e) => {
            eprintln!("port enumeration failed: {e}");
            std::process::exit(1);
        }
    }
}

fn sink(args: &[String]) {
    let port = args.get(1).cloned().expect("--sink needs a PORT");
    let line = match opt(args, "--line").as_deref() {
        Some("rts") => ControlLine::Rts,
        _ => ControlLine::Dtr,
    };
    let cfg = SerialPttConfig { port: port.clone(), active_low: has(args, "--active-low") };
    println!("opening {port} on {line:?} (active_low={})", cfg.active_low);
    let sink = SerialPttSink::open(&cfg, line).expect("open sink");
    println!("opened; line should be UNKEYED now (deasserted on open).");

    let mut wd = Watchdog::new(sink, WatchdogConfig::default());
    for n in 1..=5 {
        let t = Instant::now();
        println!("[{n}/5] KEY   (1 s)");
        wd.key(t).expect("key");
        pump(&mut wd, Duration::from_secs(1));
        let t = Instant::now();
        println!("[{n}/5] UNKEY (1 s)");
        wd.unkey(t).expect("unkey");
        pump(&mut wd, Duration::from_secs(1));
    }
    println!("done; dropping watchdog (releases the line).");
}

/// Run the watchdog tick loop for `dur`, sending a keep-alive each cycle so the
/// deadman never trips during an intentional key.
fn pump(wd: &mut Watchdog<SerialPttSink>, dur: Duration) {
    let start = Instant::now();
    while start.elapsed() < dur {
        let now = Instant::now();
        wd.keepalive(now);
        if let Err(e) = wd.tick(now) {
            eprintln!("watchdog tick error (failing closed): {e}");
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn source(args: &[String]) {
    let port = args.get(1).cloned().expect("--source needs a PORT");
    let line = match opt(args, "--sense").as_deref() {
        Some("dsr") => SenseLine::Dsr,
        Some("cd") => SenseLine::Cd,
        Some("ri") => SenseLine::Ri,
        _ => SenseLine::Cts,
    };
    let cfg = SerialPttConfig { port: port.clone(), active_low: has(args, "--active-low") };
    println!("opening {port} sensing {line:?} (active_low={})", cfg.active_low);
    let mut src = SerialPttSource::open(&cfg, line).expect("open source");
    println!("polling 20 s at 20 ms; press/release the footswitch...");

    let start = Instant::now();
    let mut last: Option<PttIntent> = None;
    while start.elapsed() < Duration::from_secs(20) {
        if let Some(intent) = src.poll() {
            if last != Some(intent) {
                println!("  {:>6}  edge -> {intent:?}", start.elapsed().as_millis());
                last = Some(intent);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("done.");
}
