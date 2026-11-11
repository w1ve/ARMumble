//! Print the sound devices and the format each would run at.

// Library dependencies the example does not use directly.
use cpal as _;
use rr_audio as _;
use rr_client as _;
use thiserror as _;
use tracing as _;

fn main() {
    for d in rr_audio_cpal::list_devices() {
        let mut line = format!("{}{} [{}{}] {}", if d.is_default { "*" } else { " " }, d.name, if d.input { "in" } else { "" }, if d.output { "out" } else { "" }, d.id);
        if d.input {
            if let Ok(p) = rr_audio_cpal::input_params(Some(&d.id)) {
                line.push_str(&format!("  in={}Hz/{}ch", p.rate, p.channels));
            }
        }
        if d.output {
            if let Ok(p) = rr_audio_cpal::output_params(Some(&d.id)) {
                line.push_str(&format!("  out={}Hz/{}ch", p.rate, p.channels));
            }
        }
        println!("{line}");
    }
}
