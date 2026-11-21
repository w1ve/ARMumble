//! Check that every listed device id opens that same device again.

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::DeviceId;

// Library dependencies the example does not use directly.
use rr_audio as _;
use rr_audio_cpal as _;
use rr_client as _;
use thiserror as _;
use tracing as _;

fn main() {
    let host = cpal::default_host();
    for d in host.devices().map(Iterator::collect::<Vec<_>>).unwrap_or_default() {
        let Ok(id) = d.id() else { continue };
        let text = id.to_string();
        let back = text.parse::<DeviceId>().ok().and_then(|p| host.device_by_id(&p));
        #[allow(deprecated)]
        let (a, b) = (d.name().unwrap_or_default(), back.as_ref().and_then(|x| x.name().ok()).unwrap_or_else(|| "NOT FOUND".into()));
        let same = back.as_ref().and_then(|x| x.id().ok()).is_some_and(|x| x == id);
        println!("{} in={} | {a} -> {b} | {text}", if same { "ok  " } else { "BAD " }, d.supports_input());
    }
}
