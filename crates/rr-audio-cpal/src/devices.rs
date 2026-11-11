//! Device enumeration and configuration choice.

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, DeviceId, SampleFormat, SupportedStreamConfig, SupportedStreamConfigRange};

use crate::stream::BackendError;

/// The engine's preferred device rate: no conversion needed.
const PREFERRED_RATE: u32 = 48_000;

/// A sound device as shown in a picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Stable identifier to pass back to the other functions.
    pub id: String,
    /// Human readable name.
    pub name: String,
    /// Can capture.
    pub input: bool,
    /// Can play.
    pub output: bool,
    /// The system default for its direction.
    pub is_default: bool,
}

/// Sample rate and channel count a stream will run at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamParams {
    /// Frames per second.
    pub rate: u32,
    /// Interleaved channels.
    pub channels: u16,
}

/// The name the operating system shows for a device.
///
/// cpal's `name()` is only the endpoint's short description ("Microphone",
/// "Speakers"), which is ambiguous when several cards are present. Windows'
/// sound control panel shows the endpoint *friendly name* instead, which adds
/// the adapter, e.g. "Microphone (USB Audio CODEC)". cpal puts that in the
/// extended lines; failing that we compose it from the adapter (driver) name.
fn display_name(d: &cpal::DeviceDescription) -> String {
    let short = d.name().trim();
    if let Some(full) = d.extended().iter().map(|s| s.trim()).find(|s| s.len() > short.len() && s.contains(short)) {
        return full.to_owned();
    }
    match d.driver().map(str::trim).filter(|s| !s.is_empty() && !short.contains(*s)) {
        Some(adapter) => format!("{short} ({adapter})"),
        None => short.to_owned(),
    }
}

/// Number repeated names ("Speakers (USB Audio)", "Speakers (USB Audio) #2")
/// so every entry in a picker is distinguishable.
fn disambiguate(devices: &mut [DeviceInfo]) {
    for i in 0..devices.len() {
        let same: Vec<usize> = (0..devices.len()).filter(|&j| devices[j].name == devices[i].name).collect();
        if same.len() < 2 || same[0] != i {
            continue;
        }
        for (n, &j) in same.iter().enumerate().skip(1) {
            devices[j].name = format!("{} #{}", devices[j].name, n + 1);
        }
    }
}

fn describe(dev: &Device, default_id: Option<&DeviceId>) -> Option<DeviceInfo> {
    let id = dev.id().ok()?;
    let name = dev.description().map_or_else(|_| id.to_string(), |d| display_name(&d));
    Some(DeviceInfo {
        is_default: default_id == Some(&id),
        id: id.to_string(),
        name,
        input: dev.supports_input(),
        output: dev.supports_output(),
    })
}

/// All input and output devices on the default host.
#[must_use]
pub fn list_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let def_in = host.default_input_device().and_then(|d| d.id().ok());
    let def_out = host.default_output_device().and_then(|d| d.id().ok());
    let mut out: Vec<DeviceInfo> = Vec::new();
    for (devices, def) in [(host.input_devices(), &def_in), (host.output_devices(), &def_out)] {
        let Ok(devices) = devices else { continue };
        for d in devices {
            let Some(info) = describe(&d, def.as_ref()) else { continue };
            match out.iter_mut().find(|e| e.id == info.id) {
                Some(e) => {
                    e.input |= info.input;
                    e.output |= info.output;
                    e.is_default |= info.is_default;
                }
                None => out.push(info),
            }
        }
    }
    disambiguate(&mut out);
    out
}

pub(crate) fn find_device(id: Option<&str>, input: bool) -> Result<Device, BackendError> {
    let host = cpal::default_host();
    if let Some(id) = id {
        let parsed = id.parse::<DeviceId>().map_err(|e| BackendError::NoDevice(format!("{id}: {e}")))?;
        return host.device_by_id(&parsed).ok_or_else(|| BackendError::NoDevice(id.to_owned()));
    }
    let dev = if input { host.default_input_device() } else { host.default_output_device() };
    dev.ok_or_else(|| BackendError::NoDevice("no default device".into()))
}

/// Score a supported range: prefer stereo, then the preferred rate, then f32.
fn score(r: &SupportedStreamConfigRange, want_channels: u16) -> (bool, bool, bool) {
    (
        r.channels() >= want_channels && r.channels() <= want_channels.max(2),
        (r.min_sample_rate()..=r.max_sample_rate()).contains(&PREFERRED_RATE),
        r.sample_format() == SampleFormat::F32,
    )
}

pub(crate) fn choose(dev: &Device, input: bool) -> Result<SupportedStreamConfig, BackendError> {
    let ranges: Vec<SupportedStreamConfigRange> = if input {
        dev.supported_input_configs().map(Iterator::collect)
    } else {
        dev.supported_output_configs().map(Iterator::collect)
    }
    .map_err(|e| BackendError::Config(e.to_string()))?;
    let best = ranges.iter().max_by_key(|r| score(r, 2));
    if let Some(cfg) = best.and_then(|r| (*r).try_with_sample_rate(PREFERRED_RATE)) {
        if score(best.unwrap_or(&ranges[0]), 2).0 {
            return Ok(cfg);
        }
    }
    let default = if input { dev.default_input_config() } else { dev.default_output_config() };
    default.map_err(|e| BackendError::Config(e.to_string()))
}

/// The rate and channel count the input device will run at.
pub fn input_params(device_id: Option<&str>) -> Result<StreamParams, BackendError> {
    let cfg = choose(&find_device(device_id, true)?, true)?;
    Ok(StreamParams { rate: cfg.sample_rate(), channels: cfg.channels() })
}

/// The rate and channel count the output device will run at.
pub fn output_params(device_id: Option<&str>) -> Result<StreamParams, BackendError> {
    let cfg = choose(&find_device(device_id, false)?, false)?;
    Ok(StreamParams { rate: cfg.sample_rate(), channels: cfg.channels() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str) -> DeviceInfo {
        DeviceInfo { id: name.into(), name: name.into(), input: true, output: false, is_default: false }
    }

    #[test]
    fn friendly_name_wins_over_short_name() {
        let d = cpal::DeviceDescriptionBuilder::new("Microphone")
            .driver("USB Audio CODEC")
            .add_extended_line("Microphone (USB Audio CODEC)")
            .build();
        assert_eq!(display_name(&d), "Microphone (USB Audio CODEC)");
    }

    #[test]
    fn adapter_is_appended_when_no_friendly_name() {
        let d = cpal::DeviceDescriptionBuilder::new("Speakers").driver("Realtek(R) Audio").build();
        assert_eq!(display_name(&d), "Speakers (Realtek(R) Audio)");
        let plain = cpal::DeviceDescriptionBuilder::new("Line In").build();
        assert_eq!(display_name(&plain), "Line In");
    }

    #[test]
    fn duplicates_are_numbered() {
        let mut v = vec![info("Microphone (USB)"), info("Speakers"), info("Microphone (USB)")];
        disambiguate(&mut v);
        assert_eq!(v[0].name, "Microphone (USB)");
        assert_eq!(v[2].name, "Microphone (USB) #2");
        assert_eq!(v[1].name, "Speakers");
    }
}
