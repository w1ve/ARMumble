//! Linux-only: enumerate audio devices by their PipeWire **friendly names**
//! (the same names the Ubuntu / GNOME volume control shows), instead of raw
//! ALSA PCM technical names.
//!
//! cpal/ALSA only exposes technical PCM names ("HDA Intel PCH, CS4208 Analog",
//! `hw:CARD=PCH,DEV=0`) and a single ambiguous bidirectional "PipeWire Sound
//! Server" node. The friendly, directional names ("Built-in Audio Analog
//! Stereo", "USB PnP Audio Device Mono") live in PipeWire node metadata.
//!
//! We query them with `pw-dump` (ships with PipeWire; `pactl` is often absent)
//! and map each node to a cpal-openable device name so the existing
//! capture/playback open path keeps working:
//!   - DISPLAY  = `node.description`   (friendly, what the user sees/picks)
//!   - OPEN-ID  = the matching cpal device name (resolved by ALSA card), so
//!     `find_input_by_name` / `find_output_by_name` can still open it.
//!
//! Lists are directional (Source = mic, Sink = speaker) and de-duplicated by
//! friendly name.

use crate::state::types::AudioDevice;

/// One PipeWire audio node of interest.
struct PwNode {
    description: String,
    card_name: String,
    alsa_path: String,
    is_sink: bool,
    is_default: bool,
}

/// Parse `pw-dump` JSON into the audio Source/Sink nodes we care about.
fn parse_pw_dump(json: &str, default_sink: &str, default_source: &str) -> Vec<PwNode> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(arr) = value.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for obj in arr {
        if obj.get("type").and_then(|t| t.as_str()) != Some("PipeWire:Interface:Node") {
            continue;
        }
        let props = obj
            .get("info")
            .and_then(|i| i.get("props"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let class = props.get("media.class").and_then(|v| v.as_str()).unwrap_or("");
        let is_sink = class.contains("Audio/Sink");
        let is_source = class.contains("Audio/Source");
        if !is_sink && !is_source {
            continue;
        }
        // Skip monitor sources (they capture OUTPUT, not a real mic).
        if is_source {
            let name = props.get("node.name").and_then(|v| v.as_str()).unwrap_or("");
            if name.contains("monitor") {
                continue;
            }
        }
        let description = props
            .get("node.description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if description.is_empty() {
            continue;
        }
        let node_name = props.get("node.name").and_then(|v| v.as_str()).unwrap_or("");
        let card_name = props
            .get("alsa.card_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let alsa_path = props
            .get("api.alsa.pcm.name")
            .or_else(|| props.get("api.alsa.path"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let is_default = (is_sink && node_name == default_sink)
            || (is_source && node_name == default_source);
        out.push(PwNode {
            description,
            card_name,
            alsa_path,
            is_sink,
            is_default,
        });
    }
    out
}

/// Run `pw-dump` and return its stdout, or `None` if the tool is unavailable.
fn run_pw_dump() -> Option<String> {
    let out = std::process::Command::new("pw-dump").output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

/// Read the default sink/source node names from `pw-metadata` (best-effort).
fn default_nodes() -> (String, String) {
    let read = |key: &str| -> String {
        let Ok(out) = std::process::Command::new("pw-metadata")
            .args(["-n", "default", key])
            .output()
        else {
            return String::new();
        };
        let text = String::from_utf8_lossy(&out.stdout);
        // Lines look like:  update: id:0 key:'default.audio.sink' value:'{"name":"..."}'
        for line in text.lines() {
            if let Some(idx) = line.find("value:'") {
                let rest = &line[idx + 7..];
                if let Some(end) = rest.find('\'') {
                    let json = &rest[..end];
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json) {
                        if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                            return name.to_string();
                        }
                    }
                }
            }
        }
        String::new()
    };
    (read("default.audio.sink"), read("default.audio.source"))
}

/// Map a PipeWire node to a cpal-openable device name using the ALSA card name,
/// so the existing `find_input_by_name` / `find_output_by_name` open path works.
/// `cpal_names` is the list the backend already enumerates (per-card PCMs).
/// Falls back to the node's raw ALSA path if no card match is found.
fn open_id_for(node: &PwNode, cpal_names: &[String]) -> String {
    // cpal names look like "HDA Intel PCH, CS4208 Analog" or "hw:CARD=PCH,DEV=0".
    // Match by the ALSA card name (e.g. "HDA Intel PCH", "USB PnP Audio Device").
    if !node.card_name.is_empty() {
        let card_lc = node.card_name.to_lowercase();
        if let Some(m) = cpal_names
            .iter()
            .find(|n| n.to_lowercase().contains(&card_lc))
        {
            return m.clone();
        }
    }
    // Fall back to the raw ALSA path (directly openable, e.g. "front:0" / "hw:3").
    if !node.alsa_path.is_empty() {
        return node.alsa_path.clone();
    }
    node.description.clone()
}

/// De-duplicate by friendly label (first occurrence wins) and build the list.
fn to_devices(nodes: &[PwNode], want_sink: bool, cpal_names: &[String]) -> Vec<AudioDevice> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for n in nodes.iter().filter(|n| n.is_sink == want_sink) {
        if !seen.insert(n.description.clone()) {
            continue; // no duplicate names
        }
        out.push(AudioDevice {
            name: open_id_for(n, cpal_names),
            label: n.description.clone(),
            is_default: n.is_default,
        });
    }
    out
}

/// Enumerate input (microphone) devices by friendly PipeWire name.
/// `cpal_names` are the backend's already-enumerated openable device names.
/// Returns `None` when PipeWire is not queryable (caller falls back to cpal).
pub fn enumerate_inputs(cpal_names: &[String]) -> Option<Vec<AudioDevice>> {
    let json = run_pw_dump()?;
    let (default_sink, default_source) = default_nodes();
    let nodes = parse_pw_dump(&json, &default_sink, &default_source);
    if nodes.is_empty() {
        return None;
    }
    let devices = to_devices(&nodes, false, cpal_names);
    if devices.is_empty() {
        None
    } else {
        Some(devices)
    }
}

/// Enumerate output (speaker/headphone) devices by friendly PipeWire name.
pub fn enumerate_outputs(cpal_names: &[String]) -> Option<Vec<AudioDevice>> {
    let json = run_pw_dump()?;
    let (default_sink, default_source) = default_nodes();
    let nodes = parse_pw_dump(&json, &default_sink, &default_source);
    if nodes.is_empty() {
        return None;
    }
    let devices = to_devices(&nodes, true, cpal_names);
    if devices.is_empty() {
        None
    } else {
        Some(devices)
    }
}
