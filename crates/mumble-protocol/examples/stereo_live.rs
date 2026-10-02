//! Live-server stereo relay harness.
//!
//! Answers the one question the unit tests cannot: **does a real Mumble server
//! relay a stereo Opus stream untouched?** `tests/opus_channels.rs` proves the
//! codec behaviour (a mono decoder downmixes stereo, a stereo decoder upmixes
//! mono) with no network involved. This drives a real server and measures what
//! comes out the other side.
//!
//! ```sh
//! cargo run -p mumble-protocol --features opus-codec --example stereo_live -- \
//!     --host newyork.remote.radio --password 'Demo!' --signal tones
//! ```
//!
//! An **example, not a test**, on purpose: it talks to an external host, so it
//! must never run in CI.
//!
//! # How it works, and why
//!
//! **Two connections.** Mumble never echoes your own audio back to you, so a
//! single session can prove nothing about relaying. The harness logs in twice —
//! one sender, one listener — joins both to the same channel, and measures what
//! the listener receives. This is the whole reason the harness exists rather
//! than being a few lines bolted onto the unit tests.
//!
//! **Audio over the TCP tunnel, not UDP.** Mumble's UDP voice path is encrypted
//! with OCB2-AES using keys from `CryptSetup`; the TCP tunnel is the standard
//! fallback and carries the identical `MumbleUDP.Audio` payload. Using it skips
//! the entire UDP crypto handshake for zero loss of fidelity in what is being
//! tested, because the server's relay decision is about `opus_data`, which is
//! byte-for-byte the same on both paths.
//!
//! **Byte-identity is the decisive check.** If the `opus_data` the listener
//! receives is byte-identical to what the sender sent, the server did not
//! transcode, re-encode, or downmix — which is exactly the claim the whole
//! no-protocol-change stereo plan rests on. Everything else here is
//! corroboration.
//!
//! **Frames are paced in real time.** Mumble servers enforce a per-user
//! bandwidth limit; firing a burst of packets would look like a flood and risks
//! being throttled or kicked. Pacing at the frame duration is both realistic and
//! polite.
//!
//! # ⚠️ Connection budget: this harness got itself autobanned once
//!
//! Each invocation opens **two** TLS sessions. An external shell loop that
//! re-ran it per configuration therefore opened 2N connections, and a nine-case
//! sweep made eighteen in about a minute. Mumble's default autoban is **10
//! attempts per 120 seconds, banning the IP for 300 seconds**, so the run died
//! partway through with every later connect failing as
//! `IO error: tls handshake eof` — murmur dropping a banned peer during the TLS
//! handshake, which looks like a TLS bug and is not one.
//!
//! Hence `--sweep`: it logs in **once** and runs the whole matrix over the same
//! pair of sessions, so a nine-case sweep costs two connections instead of
//! eighteen. Prefer it over any external loop. This is someone's live server with
//! real operators on it.
//!
//! # Signals
//!
//! | `--signal` | content | metric |
//! |---|---|---|
//! | `tones` | 450 Hz left, 1000 Hz right | per-channel separation, dB — directly comparable to `opus_channels.rs` |
//! | `noise` | independent noise both channels | L/R cross-correlation; ~0 means the image survived |
//! | `left-only` | noise left, silence right | crosstalk, dB — and the case most likely to tempt Opus into collapsing to mono |
//!
//! `noise` and `left-only` matter because real receiver audio is broadband and
//! noise-like, which couples harder than tones. The PRNG is a fixed-seed
//! xorshift rather than the `rand` crate, so runs are **reproducible** — which
//! this project has already learned to care about.

#![allow(
    unused_crate_dependencies,
    missing_docs,
    reason = "example: transitive deps are not directly imported"
)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "example binary: direct failure and long main are fine here"
)]

use std::collections::HashMap;
use std::time::Duration;

use mumble_protocol::audio::decoder::{AudioDecoder, OpusDecoder};
use mumble_protocol::audio::encoder::{
    AudioEncoder, EncodedPacket, ForceChannels, OpusEncoder, OpusEncoderConfig,
};
use mumble_protocol::audio::sample::{AudioFormat, AudioFrame, SampleFormat};
use mumble_protocol::command::{Authenticate, CommandAction, JoinChannel, SendPing};
use mumble_protocol::message::ControlMessage;
use mumble_protocol::proto::{mumble_tcp, mumble_udp};
use mumble_protocol::state::ServerState;
use mumble_protocol::transport::audio_codec::{decode_tunnel_audio, encode_tunnel_audio};
use mumble_protocol::transport::tcp::{TcpConfig, TcpTransport};

const SAMPLE_RATE: u32 = 48_000;
/// 20 ms at 48 kHz — Mumble's default frame.
const FRAME: usize = 960;
const TONE_LEFT: f32 = 450.0;
const TONE_RIGHT: f32 = 1000.0;
/// Frames discarded before measuring, so Opus is past its warm-up.
const WARMUP_FRAMES: usize = 10;

const STEREO_48KHZ_F32: AudioFormat = AudioFormat {
    sample_rate: SAMPLE_RATE,
    channels: 2,
    sample_format: SampleFormat::F32,
};

// ---------------------------------------------------------------------------
//  Options
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Opts {
    host: String,
    port: u16,
    password: Option<String>,
    channel: Option<String>,
    channel_id: Option<u32>,
    bitrate: i32,
    frames: usize,
    signal: Signal,
    force_stereo: bool,
    list_only: bool,
    allow_busy: bool,
    sweep: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Signal {
    Tones,
    Noise,
    LeftOnly,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            host: "newyork.remote.radio".to_owned(),
            port: 64738,
            password: None,
            channel: None,
            channel_id: None,
            // The measured floor for two uncorrelated receivers; see
            // tests/opus_channels.rs.
            bitrate: 96_000,
            frames: 60,
            signal: Signal::Tones,
            force_stereo: true,
            list_only: false,
            allow_busy: false,
            sweep: false,
        }
    }
}

fn usage() -> &'static str {
    "stereo_live - live Mumble stereo relay harness

  --host HOST          server hostname (default newyork.remote.radio)
  --port PORT          server port (default 64738)
  --password PASS      server password
  --channel NAME       channel to use; default picks an empty, enterable one
  --channel-id N       channel by id - preferred over --channel, since it
                       survives names containing spaces
  --bitrate BPS        Opus bitrate (default 96000, the measured stereo floor)
  --frames N           frames to send (default 60 = 1.2 s)
  --signal KIND        tones | noise | left-only  (default tones)
  --no-force-stereo    leave Opus channel coding on Auto
  --allow-busy         permit a channel that already has users in it
  --list-channels      list channels and users, then exit without transmitting
  --sweep              run the whole signal/bitrate matrix over ONE pair of
                       logins. Use this instead of an external loop: each
                       invocation costs two connections, and Mumble autobans
                       after 10 in 120 s.
"
}

fn parse_args() -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut need = |name: &str| -> Result<String, String> {
            args.next().ok_or_else(|| format!("{name} needs a value"))
        };
        match a.as_str() {
            "--host" => o.host = need("--host")?,
            "--port" => {
                o.port = need("--port")?
                    .parse()
                    .map_err(|e| format!("bad --port: {e}"))?;
            }
            "--password" => o.password = Some(need("--password")?),
            "--channel" => o.channel = Some(need("--channel")?),
            "--channel-id" => {
                o.channel_id = Some(
                    need("--channel-id")?
                        .parse()
                        .map_err(|e| format!("bad --channel-id: {e}"))?,
                );
            }
            "--bitrate" => {
                o.bitrate = need("--bitrate")?
                    .parse()
                    .map_err(|e| format!("bad --bitrate: {e}"))?;
            }
            "--frames" => {
                o.frames = need("--frames")?
                    .parse()
                    .map_err(|e| format!("bad --frames: {e}"))?;
            }
            "--signal" => {
                o.signal = match need("--signal")?.as_str() {
                    "tones" => Signal::Tones,
                    "noise" => Signal::Noise,
                    "left-only" => Signal::LeftOnly,
                    other => return Err(format!("unknown --signal '{other}'")),
                };
            }
            "--no-force-stereo" => o.force_stereo = false,
            "--allow-busy" => o.allow_busy = true,
            "--list-channels" => o.list_only = true,
            "--sweep" => o.sweep = true,
            "-h" | "--help" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if o.frames <= WARMUP_FRAMES {
        return Err(format!("--frames must exceed the {WARMUP_FRAMES}-frame warm-up"));
    }
    Ok(o)
}

// ---------------------------------------------------------------------------
//  Signal generation and analysis
// ---------------------------------------------------------------------------

/// Deterministic xorshift64*, so every run analyses the same noise.
///
/// Reproducibility is not a nicety here: this project has already drawn a wrong
/// conclusion from a non-reproducible audio measurement, and a fixed seed means
/// two runs differing is a real change rather than a different random draw.
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        Self(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [-1.0, 1.0).
    fn next_sample(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let x = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        // Top 24 bits give plenty of resolution for an audio test signal.
        ((x >> 40) as f32 / 8_388_608.0) - 1.0
    }
}

/// Build the whole interleaved stereo signal up front, so frames sliced from it
/// are phase-continuous. A discontinuity at each frame boundary would show up as
/// broadband splatter and pollute every measurement below.
fn build_signal(kind: Signal, frames: usize) -> Vec<f32> {
    let n = frames * FRAME;
    let mut out = Vec::with_capacity(n * 2);
    let mut rng = Rng::new();
    for i in 0..n {
        let t = i as f32 / SAMPLE_RATE as f32;
        let (l, r) = match kind {
            Signal::Tones => (
                0.5 * (std::f32::consts::TAU * TONE_LEFT * t).sin(),
                0.5 * (std::f32::consts::TAU * TONE_RIGHT * t).sin(),
            ),
            // Two independent draws: the worst case for a codec that couples
            // channels, and the closest of the three to real receiver audio.
            Signal::Noise => (0.35 * rng.next_sample(), 0.35 * rng.next_sample()),
            Signal::LeftOnly => (0.35 * rng.next_sample(), 0.0),
        };
        out.push(l);
        out.push(r);
    }
    out
}

fn frame_at(signal: &[f32], index: usize) -> AudioFrame {
    let lo = index * FRAME * 2;
    let slice = &signal[lo..lo + FRAME * 2];
    let mut data = Vec::with_capacity(slice.len() * 4);
    for s in slice {
        data.extend_from_slice(&s.to_ne_bytes());
    }
    AudioFrame {
        data,
        format: STEREO_48KHZ_F32,
        sequence: index as u64,
        is_silent: false,
    }
}

fn deinterleave(samples: &[f32]) -> (Vec<f32>, Vec<f32>) {
    (
        samples.iter().step_by(2).copied().collect(),
        samples.iter().skip(1).step_by(2).copied().collect(),
    )
}

fn rms(s: &[f32]) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt()
}

/// Magnitude at one frequency (Goertzel). Matches `opus_channels.rs` so the
/// figures from the two are directly comparable.
fn magnitude_at(samples: &[f32], freq_hz: f32) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let w = std::f32::consts::TAU * freq_hz / SAMPLE_RATE as f32;
    let coeff = 2.0 * w.cos();
    let mut s1 = 0.0f32;
    let mut s2 = 0.0f32;
    for &x in samples {
        let s = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    let power = s2.mul_add(s2, s1 * s1) - coeff * s1 * s2;
    power.max(0.0).sqrt() * 2.0 / samples.len() as f32
}

fn db(wanted: f32, unwanted: f32) -> f32 {
    20.0 * (wanted.max(1e-9) / unwanted.max(1e-9)).log10()
}

/// Normalised cross-correlation. ~0 means the channels are still independent;
/// approaching 1 means they have been collapsed toward a shared signal.
fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    let (a, b) = (&a[..n], &b[..n]);
    let ma = a.iter().sum::<f32>() / n as f32;
    let mb = b.iter().sum::<f32>() / n as f32;
    let mut num = 0.0f32;
    let mut da = 0.0f32;
    let mut dbv = 0.0f32;
    for i in 0..n {
        let x = a[i] - ma;
        let y = b[i] - mb;
        num += x * y;
        da += x * x;
        dbv += y * y;
    }
    let denom = (da * dbv).sqrt();
    if denom < 1e-12 {
        0.0
    } else {
        num / denom
    }
}

// ---------------------------------------------------------------------------
//  Mumble session
// ---------------------------------------------------------------------------

struct Session {
    transport: TcpTransport,
    state: ServerState,
    server_version: Option<u64>,
}

fn tcp_config(o: &Opts) -> TcpConfig {
    let _ = rustls::crypto::ring::default_provider().install_default();
    TcpConfig {
        server_host: o.host.clone(),
        server_port: o.port,
        // Mumble servers almost universally present a self-signed certificate;
        // the stock client's model is trust-on-first-use, not a CA chain.
        accept_invalid_certs: true,
        client_cert_pem: None,
        client_key_pem: None,
    }
}

/// Connect, authenticate, and read until `ServerSync`, collecting the channel
/// and user tree on the way.
async fn login(o: &Opts, username: &str) -> Result<Session, String> {
    let mut transport = TcpTransport::connect(&tcp_config(o))
        .await
        .map_err(|e| format!("connect to {}:{}: {e}", o.host, o.port))?;

    transport
        .send(&ControlMessage::Version(mumble_tcp::Version {
            version_v2: Some(0x0001_0005_0000_0000), // announce 1.5.0
            release: Some("rr-stereo-harness".into()),
            os: Some(std::env::consts::OS.into()),
            os_version: Some("harness".into()),
            ..Default::default()
        }))
        .await
        .map_err(|e| format!("send Version: {e}"))?;

    let auth = Authenticate {
        username: username.to_owned(),
        password: o.password.clone(),
        tokens: vec![],
    };
    let mut state = ServerState::new();
    for msg in &auth.execute(&state).tcp_messages {
        transport
            .send(msg)
            .await
            .map_err(|e| format!("send Authenticate: {e}"))?;
    }

    let mut server_version = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err("timed out waiting for ServerSync".to_owned());
        }
        let msg = tokio::time::timeout(Duration::from_secs(10), transport.recv())
            .await
            .map_err(|_| "recv timed out".to_owned())?
            .map_err(|e| format!("transport error: {e}"))?;
        match &msg {
            ControlMessage::Version(v) => {
                server_version = v.version_v2.or_else(|| v.version_v1.map(u64::from));
                state.apply_version(v);
            }
            ControlMessage::ChannelState(cs) => state.apply_channel_state(cs),
            ControlMessage::UserState(us) => state.apply_user_state(us),
            ControlMessage::ServerSync(sync) => {
                state.apply_server_sync(sync);
                break;
            }
            ControlMessage::Reject(r) => {
                return Err(format!(
                    "server rejected the connection: {:?} - {}",
                    r.r#type,
                    r.reason.as_deref().unwrap_or("no reason given")
                ));
            }
            _ => {}
        }
    }

    Ok(Session {
        transport,
        state,
        server_version,
    })
}

/// Send a TCP `Ping`.
///
/// ⚠️ **Not optional for any run lasting more than a few seconds.** Murmur
/// disconnects a client that has not pinged for about 30 seconds. An earlier
/// version of this harness never pinged, so single-case runs (~5 s) worked while
/// the 12-case sweep died at roughly the 38-second mark with
/// `peer closed connection without sending TLS close_notify` — which reads like a
/// TLS fault and is really just a keepalive timeout.
async fn ping(s: &mut Session) -> Result<(), String> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    let cmd = SendPing { timestamp };
    for msg in &cmd.execute(&s.state).tcp_messages {
        s.transport
            .send(msg)
            .await
            .map_err(|e| format!("send Ping: {e}"))?;
    }
    Ok(())
}

/// Pump messages for `dur`, keeping `state` current. Used to let channel/user
/// updates settle after a join.
async fn settle(s: &mut Session, dur: Duration) {
    let deadline = tokio::time::Instant::now() + dur;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        match tokio::time::timeout(remaining, s.transport.recv()).await {
            Ok(Ok(msg)) => match &msg {
                ControlMessage::ChannelState(cs) => s.state.apply_channel_state(cs),
                ControlMessage::UserState(us) => s.state.apply_user_state(us),
                ControlMessage::UserRemove(ur) => s.state.remove_user(ur.session),
                _ => {}
            },
            // A timeout just means the server had nothing more to say.
            Ok(Err(_)) | Err(_) => return,
        }
    }
}

/// Per-packet overhead: IP (20) + UDP (8) + OCB2 tag (4) + Mumble audio header
/// and varints (~6).
///
/// ⚠️ Duplicated from `mumble-tauri`'s `adjust_to_server_bandwidth`. That is a
/// smell: the figure and the clamping rule are **protocol** knowledge, not app
/// knowledge, and they belong in `mumble-protocol` so the client and anything
/// that measures the client agree by construction rather than by someone
/// remembering to update two places.
const OVERHEAD_BYTES: i32 = 20 + 8 + 4 + 6;

fn packets_per_sec(frame_ms: u32) -> i32 {
    1000_u32.div_ceil(frame_ms) as i32
}

/// Largest Opus bitrate that fits inside `max_bw` once overhead is paid.
fn max_opus_bitrate(max_bw: u32, frame_ms: u32) -> i32 {
    max_bw as i32 - packets_per_sec(frame_ms) * OVERHEAD_BYTES * 8
}

/// Total on-the-wire rate for a given Opus bitrate, at the IP layer.
fn wire_bps(bitrate: i32, frame_ms: u32) -> i32 {
    bitrate + packets_per_sec(frame_ms) * OVERHEAD_BYTES * 8
}

/// Report what the server permits, and what the **production** client would do
/// with the requested bitrate.
///
/// This matters because the harness deliberately does *not* self-limit: it sends
/// whatever `--bitrate` says. The real app applies the clamp above, so a bitrate
/// that relays fine here can still be unreachable in the shipped client. Printing
/// both keeps that distinction visible instead of producing a measurement nobody
/// can act on.
fn report_bandwidth(state: &ServerState, o: &Opts) {
    const FRAME_MS: u32 = 20;

    // Is this a stock Mumble server or a Fancy one? `Version.fancy_version`
    // (field 6) is the fork's capability marker, and it decides whether any
    // Fancy-specific behaviour is in play at all — worth knowing before
    // attributing an audio observation to "the server".
    println!("\n[2a] server identity");
    match state.connection.server_fancy_version {
        Some(v) => println!(
            "    Fancy Mumble server, fancy_version = {v} ({})",
            mumble_protocol::state::fancy_version_string(v)
        ),
        None => println!(
            "    stock Mumble server (no fancy_version announced, so no Fancy extensions)"
        ),
    }

    println!("\n[2b] bandwidth budget");
    match state.connection.max_bandwidth {
        Some(max_bw) => {
            let usable = max_opus_bitrate(max_bw, FRAME_MS);
            println!(
                "    server advertises max_bandwidth = {max_bw} bps ({:.1} kbps)",
                max_bw as f64 / 1000.0
            );
            println!(
                "    overhead at {FRAME_MS} ms = {} packets/s x {OVERHEAD_BYTES} B = {} bps",
                packets_per_sec(FRAME_MS),
                packets_per_sec(FRAME_MS) * OVERHEAD_BYTES * 8
            );
            println!(
                "    => largest Opus bitrate a conforming client may use: {usable} bps ({:.1} kbps)",
                usable as f64 / 1000.0
            );
            let requested = o.bitrate;
            if requested <= usable {
                println!("    requested {requested} bps fits, unclamped");
            } else {
                println!(
                    "    ⚠ requested {requested} bps does NOT fit: the shipped client would \
                     clamp to {usable} bps"
                );
                if usable < 96_000 {
                    println!(
                        "    ⚠ that is below the ~96 kbps where Opus stops coupling the two \
                         channels, so dual-receiver stereo would bleed badly"
                    );
                }
            }
            println!(
                "    this harness ignores the clamp on purpose and will send {requested} bps \
                 ({:.1} kbps on the wire)",
                wire_bps(requested, FRAME_MS) as f64 / 1000.0
            );
        }
        None => println!("    server advertised no max_bandwidth (no limit signalled)"),
    }
}

fn format_version(v: Option<u64>) -> String {
    match v {
        Some(x) => format!(
            "{}.{}.{}",
            (x >> 48) & 0xFFFF,
            (x >> 32) & 0xFFFF,
            (x >> 16) & 0xFFFF
        ),
        None => "unknown".to_owned(),
    }
}

/// Pick a channel to transmit in.
///
/// Defaults to an **empty** channel, and refuses an occupied one unless
/// `--allow-busy`. Transmitting test tones into a channel with people in it is
/// rude, and the instruction was to use a non-busy channel.
/// `ours` lists the harness's own session ids, which must not count towards a
/// channel's occupancy.
///
/// Both sessions land in the root channel at login, before any join, so without
/// this the harness refuses the root channel on a brand-new server by mistaking
/// itself for two other users.
fn choose_channel(
    state: &ServerState,
    o: &Opts,
    ours: &[u32],
) -> Result<(u32, String), String> {
    let occupancy = |id: u32| {
        state
            .users
            .values()
            .filter(|u| u.channel_id == id && !ours.contains(&u.session))
            .count()
    };

    if let Some(id) = o.channel_id {
        let ch = state
            .channels
            .get(&id)
            .ok_or_else(|| format!("no channel with id {id}"))?;
        let n = occupancy(id);
        if n > 0 && !o.allow_busy {
            return Err(format!(
                "channel {id} '{}' already has {n} user(s); pass --allow-busy to use it anyway",
                ch.name
            ));
        }
        return Ok((id, ch.name.clone()));
    }

    if let Some(want) = &o.channel {
        let ch = state
            .channels
            .values()
            .find(|c| c.name.eq_ignore_ascii_case(want))
            .ok_or_else(|| format!("no channel named '{want}'"))?;
        let n = occupancy(ch.channel_id);
        if n > 0 && !o.allow_busy {
            return Err(format!(
                "channel '{}' already has {n} user(s); pass --allow-busy to use it anyway",
                ch.name
            ));
        }
        return Ok((ch.channel_id, ch.name.clone()));
    }

    // Prefer an empty, enterable, non-root channel. Sorting by id keeps the
    // choice stable between runs, which makes the output comparable.
    let mut candidates: Vec<_> = state
        .channels
        .values()
        .filter(|c| c.parent_id.is_some() && !c.is_enter_restricted)
        .filter(|c| occupancy(c.channel_id) == 0)
        .collect();
    candidates.sort_by_key(|c| c.channel_id);

    if let Some(c) = candidates.first() {
        return Ok((c.channel_id, c.name.clone()));
    }
    Err("found no empty, enterable sub-channel; pass --channel NAME or --allow-busy".to_owned())
}

// ---------------------------------------------------------------------------
//  The measurement
// ---------------------------------------------------------------------------

/// What the listening session received, keyed by frame number.
struct Received {
    packets: HashMap<u64, Vec<u8>>,
    other_senders: HashMap<u32, usize>,
    /// Set when the link failed rather than merely going quiet.
    transport_error: Option<String>,
    /// Set when the server removed us or refused something.
    kicked: Option<String>,
}

/// One configuration to measure.
#[derive(Debug, Clone, Copy)]
struct Case {
    signal: Signal,
    bitrate: i32,
    force_stereo: bool,
}

/// What one case measured.
struct CaseResult {
    received: usize,
    identical: usize,
    /// Signal-appropriate figure: separation dB, correlation, or crosstalk dB.
    metric: String,
}

/// Read from the listener until `deadline`, collecting audio from `want_session`.
///
/// Takes `&mut` rather than consuming the session so one login can serve many
/// cases — see the connection-budget note at the top of this file.
async fn collect_audio(
    s: &mut Session,
    want_session: u32,
    deadline: tokio::time::Instant,
) -> Received {
    let mut packets = HashMap::new();
    let mut other_senders: HashMap<u32, usize> = HashMap::new();
    let mut transport_error = None;
    let mut kicked = None;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        match tokio::time::timeout(remaining, s.transport.recv()).await {
            Ok(Ok(msg)) => match &msg {
                ControlMessage::UdpTunnel(data) => {
                    if let Ok(audio) = decode_tunnel_audio(data) {
                        if audio.sender_session == want_session {
                            let _ = packets.insert(audio.frame_number, audio.opus_data);
                        } else {
                            *other_senders.entry(audio.sender_session).or_insert(0) += 1;
                        }
                    }
                }
                // Keep the view of the channel current, and notice if the server
                // moves or removes us mid-run.
                ControlMessage::UserState(us) => s.state.apply_user_state(us),
                ControlMessage::UserRemove(ur) => {
                    if Some(ur.session) == s.state.own_session() || ur.session == want_session {
                        kicked = Some(format!(
                            "server removed session {} mid-run{}",
                            ur.session,
                            ur.reason
                                .as_deref()
                                .map(|r| format!(": {r}"))
                                .unwrap_or_default()
                        ));
                    }
                    s.state.remove_user(ur.session);
                }
                ControlMessage::PermissionDenied(pd) => {
                    kicked = Some(format!("permission denied: {pd:?}"));
                }
                _ => {}
            },
            // A closed or broken connection must NOT look like silence. Reporting
            // these separately is the difference between "the server stopped
            // relaying" and "we were disconnected and never noticed", which an
            // earlier version of this harness could not distinguish — it reported
            // a dead link as "received no audio at all".
            Ok(Err(e)) => {
                transport_error = Some(format!("{e}"));
                break;
            }
            // Timeout: the deadline simply arrived with nothing more to read.
            Err(_) => break,
        }
    }
    Received {
        packets,
        other_senders,
        transport_error,
        kicked,
    }
}

#[tokio::main]
async fn main() {
    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}\n\n{}", usage());
            std::process::exit(2);
        }
    };

    if let Err(e) = run(&opts).await {
        eprintln!("\nFAILED: {e}");
        std::process::exit(1);
    }
}

async fn run(o: &Opts) -> Result<(), String> {
    println!("RemoteRadio stereo relay harness");
    println!("================================");
    println!("server   {}:{}", o.host, o.port);

    // Usernames must be unique on the server; derive a suffix from the clock.
    let tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() % 100_000)
        .unwrap_or(0);

    println!("\n[1] connecting listener");
    let mut rx = login(o, &format!("rr-rx-{tag}")).await?;
    println!(
        "    ok, server version {} ({})",
        format_version(rx.server_version),
        if rx.state.connection.supports_protobuf_audio() {
            "protobuf audio"
        } else {
            "legacy audio"
        }
    );

    println!("[2] connecting sender");
    let mut tx = login(o, &format!("rr-tx-{tag}")).await?;
    let tx_session = tx
        .state
        .own_session()
        .ok_or("server never told us our own session id")?;
    println!("    ok, sender session {tx_session}");

    report_bandwidth(&tx.state, o);

    // Let both sides receive the full channel and user tree.
    settle(&mut rx, Duration::from_millis(1500)).await;
    settle(&mut tx, Duration::from_millis(1500)).await;

    if o.list_only {
        let mut chans: Vec<_> = tx.state.channels.values().collect();
        chans.sort_by_key(|c| c.channel_id);
        println!("\n  id    users  enter?  name");
        println!("  ----  -----  ------  ----");
        for c in chans {
            let n = tx
                .state
                .users
                .values()
                .filter(|u| u.channel_id == c.channel_id)
                .count();
            println!(
                "  {:<4}  {:<5}  {:<6}  {}",
                c.channel_id,
                n,
                if c.is_enter_restricted { "no" } else { "yes" },
                c.name
            );
        }
        println!("\n(--list-channels: nothing transmitted)");
        return Ok(());
    }

    // Both of our own sessions are excluded from the occupancy check.
    let ours: Vec<u32> = [rx.state.own_session(), Some(tx_session)]
        .into_iter()
        .flatten()
        .collect();
    let (channel_id, channel_name) = choose_channel(&tx.state, o, &ours)?;
    println!("[3] joining channel {channel_id} '{channel_name}'");
    for s in [&mut rx, &mut tx] {
        let join = JoinChannel {
            channel_id,
            password: None,
        };
        for msg in &join.execute(&s.state).tcp_messages {
            s.transport
                .send(msg)
                .await
                .map_err(|e| format!("send JoinChannel: {e}"))?;
        }
    }
    settle(&mut rx, Duration::from_millis(1200)).await;
    settle(&mut tx, Duration::from_millis(1200)).await;

    // One login, many cases. See the connection-budget note at the top.
    if o.sweep {
        println!("\n[4] sweep: every case shares these two logins\n");
        println!("  signal      bitrate  force   recv  ident  metric");
        println!("  ----------  -------  ------  ----  -----  ------");
        let mut failures = 0usize;
        for case in sweep_matrix() {
            match run_case(o, case, &mut rx, &mut tx, tx_session, false).await {
                Ok(r) => {
                    if r.identical != r.received || r.received == 0 {
                        failures += 1;
                    }
                    println!(
                        "  {:<10}  {:>7}  {:<6}  {:>4}  {:>5}  {}",
                        format!("{:?}", case.signal).to_lowercase(),
                        case.bitrate,
                        if case.force_stereo { "stereo" } else { "auto" },
                        r.received,
                        r.identical,
                        r.metric
                    );
                }
                Err(e) => {
                    failures += 1;
                    println!(
                        "  {:<10}  {:>7}  {:<6}  {:>4}  {:>5}  FAILED: {e}",
                        format!("{:?}", case.signal).to_lowercase(),
                        case.bitrate,
                        if case.force_stereo { "stereo" } else { "auto" },
                        "-",
                        "-"
                    );
                }
            }
            // Keepalive on BOTH sessions, then drain both sockets. The ping is
            // what keeps a long sweep alive past murmur's ~30 s timeout.
            if let Err(e) = ping(&mut tx).await {
                return Err(format!("sender ping failed: {e}"));
            }
            if let Err(e) = ping(&mut rx).await {
                return Err(format!("listener ping failed: {e}"));
            }
            settle(&mut tx, Duration::from_millis(600)).await;
            settle(&mut rx, Duration::from_millis(600)).await;
        }
        println!("\n[5] verdict");
        if failures == 0 {
            println!("    PASS - every case relayed byte-for-byte untouched.");
            return Ok(());
        }
        return Err(format!("{failures} case(s) did not relay cleanly"));
    }

    let case = Case {
        signal: o.signal,
        bitrate: o.bitrate,
        force_stereo: o.force_stereo,
    };
    let _ = run_case(o, case, &mut rx, &mut tx, tx_session, true).await?;
    Ok(())
}

/// The matrix `--sweep` runs.
///
/// Weighted toward **noise**, because that is the closest of the three signals
/// to real receiver audio and the first live run showed it couples far harder
/// than tones: 0.49 correlation at 72 kbps against tones' near-perfect
/// separation. `left-only` is the unambiguous discriminator — silence in the
/// right channel means any energy there is crosstalk and nothing else.
fn sweep_matrix() -> Vec<Case> {
    let mut cases = Vec::new();
    for bitrate in [72_000, 96_000, 128_000, 160_000] {
        cases.push(Case {
            signal: Signal::Noise,
            bitrate,
            force_stereo: true,
        });
    }
    for bitrate in [72_000, 96_000, 128_000, 160_000] {
        cases.push(Case {
            signal: Signal::LeftOnly,
            bitrate,
            force_stereo: true,
        });
    }
    // Auto channel coding, to see whether forcing stereo changes anything on
    // signals that might tempt Opus to collapse.
    cases.push(Case {
        signal: Signal::Noise,
        bitrate: 96_000,
        force_stereo: false,
    });
    cases.push(Case {
        signal: Signal::LeftOnly,
        bitrate: 96_000,
        force_stereo: false,
    });
    // Tones for continuity with tests/opus_channels.rs.
    cases.push(Case {
        signal: Signal::Tones,
        bitrate: 72_000,
        force_stereo: true,
    });
    cases.push(Case {
        signal: Signal::Tones,
        bitrate: 96_000,
        force_stereo: true,
    });
    cases
}

/// Encode, transmit, collect and analyse one configuration over existing
/// sessions.
async fn run_case(
    o: &Opts,
    case: Case,
    rx: &mut Session,
    tx: &mut Session,
    tx_session: u32,
    verbose: bool,
) -> Result<CaseResult, String> {
    let config = OpusEncoderConfig {
        bitrate: case.bitrate,
        force_channels: if case.force_stereo {
            ForceChannels::Stereo
        } else {
            ForceChannels::Auto
        },
        ..OpusEncoderConfig::default()
    };
    let mut encoder =
        OpusEncoder::new(config, STEREO_48KHZ_F32).map_err(|e| format!("Opus encoder: {e}"))?;

    let signal = build_signal(case.signal, o.frames);
    let mut sent: HashMap<u64, Vec<u8>> = HashMap::new();
    let mut wire: Vec<(u64, Vec<u8>)> = Vec::with_capacity(o.frames);
    for i in 0..o.frames {
        let packet = encoder
            .encode(&frame_at(&signal, i))
            .map_err(|e| format!("encode frame {i}: {e}"))?;
        let _ = sent.insert(i as u64, packet.data.clone());
        wire.push((i as u64, packet.data));
    }
    let total_bytes: usize = wire.iter().map(|(_, d)| d.len()).sum();
    if verbose {
        println!(
            "[4] encoded {} stereo frames, {} bytes, {:?} signal, {} bps, force={}",
            o.frames,
            total_bytes,
            case.signal,
            case.bitrate,
            if case.force_stereo { "stereo" } else { "auto" }
        );
        println!("[5] transmitting (paced at 20 ms/frame to respect the server's rate limit)");
    }

    // Send first, then drain. The listener's socket buffers the relayed audio
    // meanwhile -- 60 frames is ~15 KB, far inside a default receive buffer --
    // which avoids borrowing both sessions into concurrent tasks and lets one
    // login serve the whole sweep.
    let protobuf = tx.state.connection.supports_protobuf_audio();
    let mut ticker = tokio::time::interval(Duration::from_millis(20));
    for (seq, data) in &wire {
        // `tick()` hands back the instant it fired; the workspace denies
        // unused_results, and the instant is genuinely not needed here.
        let _ = ticker.tick().await;
        let audio = mumble_udp::Audio {
            header: Some(mumble_udp::audio::Header::Target(0)),
            sender_session: 0, // server ignores this from clients
            frame_number: *seq,
            opus_data: data.clone(),
            positional_data: Vec::new(),
            volume_adjustment: 0.0,
            is_terminator: *seq + 1 == o.frames as u64,
        };
        let framed = encode_tunnel_audio(&audio, protobuf);
        tx.transport
            .send(&ControlMessage::UdpTunnel(framed))
            .await
            .map_err(|e| format!("send audio frame {seq}: {e}"))?;
    }

    // Drain the relayed audio. The tail allows for packets still in flight.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let received = collect_audio(rx, tx_session, deadline).await;

    // -- results -----------------------------------------------------------
    let got = received.packets.len();
    if verbose {
        println!("\n[6] relay");
        println!("    sent {} frames, listener received {got}", o.frames);
    }
    if got == 0 {
        // Say which of the three it was, rather than guessing.
        if let Some(e) = &received.transport_error {
            return Err(format!("listener's connection failed: {e}"));
        }
        if let Some(k) = &received.kicked {
            return Err(format!("listener was interfered with: {k}"));
        }
        return Err(
            "no audio arrived, but the listener's link is still healthy - the server accepted \
             the packets and did not relay them (bandwidth limit, mute, or a channel mismatch)"
                .to_owned(),
        );
    }

    let mut identical = 0usize;
    let mut differing = Vec::new();
    for (seq, original) in &sent {
        if let Some(back) = received.packets.get(seq) {
            if back == original {
                identical += 1;
            } else {
                differing.push((*seq, original.len(), back.len()));
            }
        }
    }
    if verbose {
        println!("    byte-identical payloads: {identical}/{got}");
        if !differing.is_empty() {
            println!(
                "    ⚠ {} payload(s) came back altered, e.g. frame {} ({} -> {} bytes)",
                differing.len(),
                differing[0].0,
                differing[0].1,
                differing[0].2
            );
        }
        if !received.other_senders.is_empty() {
            println!(
                "    note: also heard audio from {} other session(s) in the channel",
                received.other_senders.len()
            );
        }
    }

    // -- decode what a RemoteRadio client would hear ------------------------
    let mut ordered: Vec<_> = received.packets.iter().collect();
    ordered.sort_by_key(|(seq, _)| **seq);

    // Opus is predictive: decode the run in order and analyse only the tail,
    // once encoder and decoder are both past warm-up. Measuring a single frame,
    // or replaying one packet, produces meaningless numbers.
    let mut stereo_dec =
        OpusDecoder::new(STEREO_48KHZ_F32).map_err(|e| format!("stereo decoder: {e}"))?;
    let mut mono_dec = OpusDecoder::new(AudioFormat::MONO_48KHZ_F32)
        .map_err(|e| format!("mono decoder: {e}"))?;
    let mut stereo_tail: Vec<f32> = Vec::new();
    let mut mono_tail: Vec<f32> = Vec::new();
    for (i, (seq, data)) in ordered.iter().enumerate() {
        let pkt = EncodedPacket {
            data: (*data).clone(),
            sequence: **seq,
            frame_samples: FRAME as u32,
        };
        let s = stereo_dec
            .decode(&pkt)
            .map_err(|e| format!("stereo decode frame {seq}: {e}"))?;
        let m = mono_dec
            .decode(&pkt)
            .map_err(|e| format!("mono decode frame {seq}: {e}"))?;
        if i >= WARMUP_FRAMES {
            stereo_tail.extend_from_slice(s.as_f32_samples());
            mono_tail.extend_from_slice(m.as_f32_samples());
        }
    }
    if stereo_tail.is_empty() {
        return Err("too few frames survived to analyse; try --frames 60".to_owned());
    }

    let (left, right) = deinterleave(&stereo_tail);

    // The figure that characterises this signal. Each one answers a different
    // question, which is why the harness offers three signals rather than one.
    let metric = match case.signal {
        Signal::Tones => {
            let l = db(
                magnitude_at(&left, TONE_LEFT),
                magnitude_at(&left, TONE_RIGHT),
            );
            let r = db(
                magnitude_at(&right, TONE_RIGHT),
                magnitude_at(&right, TONE_LEFT),
            );
            format!("sep L {l:.1} dB / R {r:.1} dB")
        }
        Signal::Noise => {
            format!("corr {:.4}", correlation(&left, &right))
        }
        Signal::LeftOnly => {
            // Silence was sent in the right channel, so anything there is
            // crosstalk and nothing else. The least ambiguous of the three.
            //
            // ⚠️ In practice Opus returns the unused channel as *exact* digital
            // silence, so `db()` hits its 1e-9 denominator floor and prints
            // ~165 dB. That is the floor talking, not a measurement. Say so
            // rather than letting a reader take 165 dB at face value.
            let r = rms(&right);
            if r < 1e-7 {
                "right channel is digital silence (crosstalk below resolution)".to_owned()
            } else {
                format!("crosstalk {:.1} dB", db(rms(&left), r))
            }
        }
    };

    if verbose {
        println!("\n[7] stereo decode (what a RemoteRadio client hears)");
        println!("    L rms {:.4}   R rms {:.4}", rms(&left), rms(&right));
        match case.signal {
            Signal::Tones => {
                println!("    separation: {metric}");
                // Deliberately NOT inviting a direct comparison with
                // tests/opus_channels.rs. Goertzel rejection of the other tone
                // improves with window length, and this harness analyses
                // `--frames` minus warm-up (1 s at the default) against that
                // test's fixed 200 ms. Live 72 kbps reads ~28 dB where the test
                // reads ~13 dB for the same encoder settings, purely from the
                // longer window. Compare the *trend* across bitrates, not the
                // absolute figures.
                println!("    (window-dependent; compare across bitrates, not against the tests)");
            }
            Signal::Noise => {
                println!("    L/R {metric}  (0 = independent, 1 = collapsed to one signal)");
            }
            Signal::LeftOnly => {
                println!("    {metric}: right channel below left (silence was sent right)");
            }
        }

        println!("\n[8] mono decode (what every stock Mumble client hears)");
        println!("    rms {:.4}", rms(&mono_tail));
        if case.signal == Signal::Tones {
            println!(
                "    450 Hz {:.5}, 1000 Hz {:.5}, 7 kHz floor {:.5}",
                magnitude_at(&mono_tail, TONE_LEFT),
                magnitude_at(&mono_tail, TONE_RIGHT),
                magnitude_at(&mono_tail, 7_000.0)
            );
        }

        println!("\n[9] verdict");
        let relay_clean = identical == got && differing.is_empty();
        if relay_clean {
            println!("    PASS - the server relayed stereo Opus byte-for-byte untouched.");
        } else {
            println!(
                "    FAIL - payloads were altered in transit; the server is not a pure relay."
            );
        }
        let loss = o.frames - got;
        if loss > 0 {
            println!("    note: {loss} frame(s) did not arrive (ordinary loss, or rate limiting).");
        }
        if !relay_clean {
            return Err("relay was not byte-transparent".to_owned());
        }
    }

    Ok(CaseResult {
        received: got,
        identical,
        metric,
    })
}
