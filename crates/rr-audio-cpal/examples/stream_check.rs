//! Open every input and output device for two seconds and report whether
//! audio callbacks arrive (checks that raw-mode streams open on this machine).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

// Library dependencies the example does not use directly.
use rr_audio as _;
use rr_audio_cpal as _;
use rr_client as _;
use thiserror as _;
use tracing as _;

fn main() {
    let host = cpal::default_host();
    let inputs = host.input_devices().map(Iterator::collect::<Vec<_>>).unwrap_or_default();
    let outputs = host.output_devices().map(Iterator::collect::<Vec<_>>).unwrap_or_default();
    for (d, input) in inputs.into_iter().map(|d| (d, true)).chain(outputs.into_iter().map(|d| (d, false))) {
        #[allow(deprecated)]
        let name = d.name().unwrap_or_default();
        let cfg = if input { d.default_input_config() } else { d.default_output_config() };
        let Ok(cfg) = cfg else {
            println!("{} {name}: no default config", if input { "IN " } else { "OUT" });
            continue;
        };
        let frames = Arc::new(AtomicU64::new(0));
        let f = Arc::clone(&frames);
        let ch = u64::from(cfg.channels());
        let err = |e| eprintln!("stream error: {e}");
        let stream = if input {
            d.build_input_stream(&cfg.config(), move |x: &[f32], _: &_| { let _ = f.fetch_add(x.len() as u64 / ch, Ordering::Relaxed); }, err, None)
        } else {
            d.build_output_stream(&cfg.config(), move |x: &mut [f32], _: &_| { x.fill(0.0); let _ = f.fetch_add(x.len() as u64 / ch, Ordering::Relaxed); }, err, None)
        };
        match stream {
            Ok(s) => {
                let _ = s.play();
                std::thread::sleep(Duration::from_secs(2));
                drop(s);
                println!("{} {name}: {} Hz {} ch, {} frames in 2 s", if input { "IN " } else { "OUT" }, cfg.sample_rate(), cfg.channels(), frames.load(Ordering::Relaxed));
            }
            Err(e) => println!("{} {name}: open failed: {e}", if input { "IN " } else { "OUT" }),
        }
    }
}
