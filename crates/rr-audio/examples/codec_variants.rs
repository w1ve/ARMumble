//! Offline codec experiment: encode a raw mono f32 48 kHz file with several
//! Opus configurations, decode, and write each result as raw mono f32.
//!
//! `cargo run -p rr-audio --example codec_variants -- IN.f32 OUTDIR`

use opus::{Application, Bandwidth, Bitrate, Channels, Decoder, Encoder, Signal};

// Library dependencies the example does not use directly.
use rr_audio as _;
use rustfft as _;
use thiserror as _;
use tracing as _;

struct Variant {
    name: &'static str,
    app: Application,
    signal: Option<Signal>,
    bitrate: i32,
    vbr: bool,
    constrained: bool,
    frame: usize,
    stereo: bool,
}

fn run(v: &Variant, input: &[f32]) -> Vec<f32> {
    let ch = if v.stereo { Channels::Stereo } else { Channels::Mono };
    let mut enc = Encoder::new(48_000, ch, v.app).expect("enc");
    enc.set_bitrate(Bitrate::Bits(v.bitrate)).expect("br");
    enc.set_vbr(v.vbr).expect("vbr");
    enc.set_vbr_constraint(v.constrained).expect("cvbr");
    enc.set_complexity(10).expect("cx");
    enc.set_inband_fec(false).expect("fec");
    enc.set_dtx(false).expect("dtx");
    enc.set_packet_loss_perc(0).expect("plp");
    if let Some(s) = v.signal {
        enc.set_signal(s).expect("sig");
    }
    enc.set_max_bandwidth(Bandwidth::Fullband).expect("bw");
    enc.set_lsb_depth(24).expect("lsb");
    if v.stereo {
        enc.set_force_channels(Some(Channels::Stereo)).expect("fc");
        enc.set_phase_inversion_disabled(true).expect("pi");
    }
    let mut dec = Decoder::new(48_000, Channels::Stereo).expect("dec");
    let nch = if v.stereo { 2 } else { 1 };
    let mut pkt = vec![0_u8; 4000];
    let mut pcm = vec![0.0_f32; v.frame * nch];
    let mut outbuf = vec![0.0_f32; 5760 * 2];
    let mut out = Vec::with_capacity(input.len());
    let mut bytes = 0_usize;
    for chunk in input.chunks_exact(v.frame) {
        for (i, s) in chunk.iter().enumerate() {
            for c in 0..nch {
                pcm[i * nch + c] = *s;
            }
        }
        let n = enc.encode_float(&pcm, &mut pkt).expect("encode");
        bytes += n;
        let m = dec.decode_float(&pkt[..n], &mut outbuf, false).expect("decode");
        out.extend(outbuf[..m * 2].chunks_exact(2).map(|f| f[0]));
    }
    let secs = input.len() as f64 / 48_000.0;
    println!("{:<28} {:>6.1} kbit/s actual", v.name, bytes as f64 * 8.0 / secs / 1000.0);
    out
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let raw = std::fs::read(&a[1]).expect("read");
    let input: Vec<f32> = raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let dir = &a[2];
    let base = Variant { name: "", app: Application::Audio, signal: Some(Signal::Music), bitrate: 128_000, vbr: true, constrained: true, frame: 960, stereo: true };
    let variants = [
        Variant { name: "current_128k_stereo_20ms", ..base },
        Variant { name: "current_96k_stereo_40ms", bitrate: 96_000, frame: 1920, ..base },
        Variant { name: "cbr_128k_stereo", vbr: false, ..base },
        Variant { name: "mono_96k", stereo: false, bitrate: 96_000, ..base },
        Variant { name: "mono_cbr_96k", stereo: false, bitrate: 96_000, vbr: false, ..base },
        Variant { name: "stereo_256k", bitrate: 256_000, ..base },
        Variant { name: "stereo_510k_cbr", bitrate: 510_000, vbr: false, ..base },
        Variant { name: "lowdelay_128k_stereo", app: Application::LowDelay, ..base },
        Variant { name: "voip_auto_64k_mono", app: Application::Voip, signal: None, stereo: false, bitrate: 64_000, ..base },
    ];
    for v in &variants {
        let out = run(v, &input);
        let bytes: Vec<u8> = out.iter().flat_map(|s| s.to_le_bytes()).collect();
        std::fs::write(format!("{dir}/{}.f32", v.name), bytes).expect("write");
    }
}
