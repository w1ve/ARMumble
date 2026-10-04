# RemoteRadio — Project Context

> Working context for the RemoteRadio project (RRMumble / ARMumble fork).
> Last updated: end of **P7b (hardware H.264 encode, 4K at 41.2 fps — committed)**,
> with **P3 (input control) written but uncommitted and never compiled** — see §12.

---

## 1. What this project is

> ⚠️ **Domain context, and it matters for design decisions: RemoteRadio exists to
> remote *amateur radio transceivers*.** The Mumble voice path carries **radio
> audio**, not conversation, and the remote-desktop path exists to drive rig
> control software. This was not recorded in the first twelve sections of this
> document and has already changed the right answer to several audio questions —
> see **§14**, which covers stereo (dual-receiver) audio, transmit modes and rig
> PTT. When evaluating anything audio-related, assume **radio audio with all
> speech-oriented processing switched off**, not a voice call.

Converting the **ARMumble** repo (a fork of the public FancyMumble Mumble
client, Rust + Tauri 2 + React) into **three** deliverables:

| App | Platform | Role |
|---|---|---|
| **RemoteRadioStation** | Windows desktop | Mumble client **+** remote-desktop *host* (captures screen, injects input, serves clipboard/files) |
| **RemoteRadioClient** | Windows desktop | Mumble client **+** remote-desktop *viewer/controller* |
| **RemoteRadioMumble** | Android | The existing Android app, **renamed only** |

Approved design decisions (all locked, see `ARMumble/doc/remote-radio/SPEC.md` §10):

1. **Fully peer-to-peer** media. Mumble is a **signaling relay only** — no custom
   server, no SFU. Any Fancy-compatible Mumble server works.
2. **Native capture + encode** (xcap baseline → DXGI/WGC fast path in P7).
3. **Lowest latency possible** (H.264 primary via openh264, VP8 fallback).
4. Features: **multi-monitor, clipboard sync, file transfer**. **No audio**
   over WebRTC (Mumble owns voice).
5. **TURN via coturn in Docker**, deployed next to the user's Linux Mumble
   servers; credentials minted by a small co-deployed REST endpoint.
6. **Two thin binary crates over a shared library** (not one app with a flag).
7. **Cooperative control handoff only** — no forcible takeover.
8. Heavy test coverage at every phase.
9. Connect policy: **any authenticated member of the Station's Mumble channel**
   may view and request control (no allow-list in v1).
10. Bind peer DTLS fingerprint to the Mumble user `hash` (anti-SDP-substitution).
11. `unsafe` only in capture/HW-FFI crates, lint-overridden with justification.

---

## 2. Machines & build workflow

### Local (Windows)
- Repo: `e:\AI\RRMumble\ARMumble`
- Rust on **E:** — `CARGO_HOME=E:\rust\cargo`, `RUSTUP_HOME=E:\rust\rustup`
  (persisted as User env vars; `E:\rust\cargo\bin` on User PATH).
  rustc/cargo **1.99.0**, MSVC toolchain.
- CMake 4.3.1 portable at `E:\devtools\cmake-4.3.1-windows-x86_64\bin`,
  Ninja 1.12.1 at `E:\devtools\ninja` (both on User PATH).
- Node v22.19.0 / npm. WebView2 runtime 154.x present.
- **Local MSVC is incomplete** (VS18 Insiders is OneCore-only, no VC headers).
  We do **not** build desktop apps locally — see below. But the *library* crates
  do build locally via **`E:\devtools\cargo-msvc.cmd <cargo args...>`**, which
  sources `E:\devtools\BuildTools\VC\Auxiliary\Build\vcvars64.bat` (MSVC
  14.44.35207 + SDK 10.0.19041.0), puts **nasm** on PATH, `cd`s to the repo and
  echoes `CARGO_EXIT=`. Without it, cargo picks the OneCore MSVC and fails with
  `LNK1104: cannot open file 'msvcrt.lib'`.
- **nasm 3.02** at `E:\devtools\nasm` (matches big-linux exactly). Mandatory —
  see §8c.
- Hardware (`W1VE-REMOTE`): Lenovo `20XXX16500` **laptop**, **i7-1165G7** Tiger
  Lake mobile, **4 cores / 8 threads**, **32 GB LPDDR4X-4267**, touch screen.
  Runs **at or just above its 2.80 GHz nominal** clock under load (84–112% of
  nominal). Timings are representative for a 4-core mobile i7, but they are
  **sensitive to background load** (Chrome/Kiro at ~50% CPU moved DXGI capture
  from 3.7 ms to 5.3 ms), so prefer ratios over absolutes and note the load.
- ⚠️ **`Win32_Processor.CurrentClockSpeed` is frozen at 1201 MHz on this machine**
  and never changes under any load. It is a cached WMI value. I believed it once
  and wrote "power-limited, figures pessimistic" into a committed doc; it was
  wrong. Use
  `Win32_PerfFormattedData_Counters_ProcessorInformation.PercentProcessorPerformance`
  (live, tracks boost).
- ⚠️ **Intel reports its media engine under `engtype_videodecode`, not
  `engtype_VideoEncode`.** Reading the "VideoEncode" GPU engine counter shows
  **0%** while Quick Sync is fully saturated. This nearly made me conclude a
  working hardware encoder was software. Sum `engtype_videodecode` across
  `GPU Engine` instances instead (`E:\devtools\gpu-engines.ps1`).
- Measurement helpers in `E:\devtools\`: `cargo-msvc.cmd` (build wrapper, see
  above), `verify-local.ps1`, `run-probe.ps1`, `run-probe-both.ps1`,
  `run-bench.ps1`, `run-bench-encode.ps1`, `clock-line.ps1`, `qsv-bench.ps1`,
  `gpu-engines.ps1`, `hw-report.ps1`, `thermal-check.ps1`, `load-sample.ps1`,
  `cpu-info.ps1`, `clock-under-load.ps1`, `smoke-test-both.ps1`.
- **`ffmpeg` is installed locally as a *measurement instrument only*.** It
  established that `h264_qsv` reaches 70–85 fps at 4K on this GPU, which is how
  we knew the target was achievable before writing `rr-hwcodec`. It is **not a
  dependency**, nothing ships with it, and its GPL is untouched.

### Remote build host (authoritative)
- SSH alias **`big-linux`** (via `cloudflared` ProxyCommand; resolves to user
  `gerry`, home `/home/gerry`). 24 cores, 121 GB RAM, Ubuntu.
- Repo: `/home/gerry/RRMumble/ARMumble`
- Toolchains documented in `/home/gerry/RRMumble/opt/BUILDING-LOCAL.md`, with
  `opt/env-android.sh` and `opt/env-windows.sh` to source.
- rustc/cargo 1.98.1, cargo-tauri 2.12.0, cargo-xwin, clang-cl/lld (homebrew),
  JDK17 at `~/opt/jdk17`, Android SDK at `~/opt/android-sdk` (NDK 27.0.12077973).
- **nasm 3.02** is at `/home/linuxbrew/.linuxbrew/bin/nasm` but **NOT on default
  PATH** — must prepend `/home/linuxbrew/.linuxbrew/bin`. Without it `openh264`
  **silently** drops its SIMD kernels; measured cost ~10x on encode (140 ms vs
  17 ms per 1080p frame). See §8c.
- A non-interactive `ssh` shell loads no profile, so `cargo` is not on PATH
  either; every remote script must `export PATH="$HOME/.cargo/bin:...:$PATH"` or
  everything exits 127.
- `libasound2-dev` is **not installed**, so `cargo clippy --workspace
  --all-targets` fails on `alsa-sys`. Pre-existing; verify changed crates
  individually, plus the Windows target via `cargo xwin`.
- Node via nvm at `/home/gerry/.nvm/versions/node/v24.21.0/bin` — **env scripts
  do not load nvm**, and Tauri's `beforeBuildCommand` runs `npm run build` via
  `/bin/sh`, so build launchers must prepend that path explicitly.

### Sync method that works
Author files locally → `tar -czf` a batch → `scp` to big-linux → `tar -xzf`
there → build/clippy/test remotely → read results back.
Both repos on branch **`feature/remote-radio-p0`**, committed on both sides
(**not pushed to origin**).

### Shell gotchas (cost real time; do not repeat)
- The local PowerShell tool **intermittently swallows stdout**, and sometimes
  fails to create an output file at all when given a long inline command.
  **The reliable pattern is: write a `.ps1` file into `E:\devtools\` and run it**
  (`powershell -NoProfile -ExecutionPolicy Bypass -File ...`), having the script
  tee its own output to a text file, then `read_file` that. Do not fight the
  inline path.
- `Start-Process -ArgumentList` **splits paths containing spaces** → use `.cmd`
  scripts or native `& exe arg arg` for `msiexec`.
- Screenshots in the agent session **do not reflect the user's live desktop** —
  never instruct the user to click dialogs based on them.
- Long remote builds: launch detached (`setsid ... & disown` or `nohup`), poll
  the log; a blocking `ssh` may return before the work finishes.

---

## 3. Windows SDK repair (RESOLVED — do not redo)

A `--nocache` flag I passed to the VS Build Tools installer orphaned the
**Kits Configuration Installer** (Windows SDK 10.0.19041,
product `{85FC198B-F293-0ED4-CD62-09F136CBF5AD}`), which then blocked installs
with a repair prompt.

**Real root cause** (found in the MSI's own Registry table, not the ACLs):

```
Key:   SOFTWARE\Microsoft\Windows Kits\CEIP
Name:  OptIn
Value: #[KITSCEIPOPTIN]     # "#" = REG_DWORD; property normally set by the bootstrapper
```

A bare `msiexec /i` left `KITSCEIPOPTIN` empty → tried to write a DWORD with no
value → `Error 1406 ... System error .` (blank code = the tell) → 1603 rollback.

**Fix that worked** (`E:\devtools\repair-kits4.cmd`):
```cmd
msiexec /i "D:\Installers\Kits Configuration Installer-x86_en-us.msi" ^
  KITSCEIPOPTIN=0 REINSTALL=ALL REINSTALLMODE=vomus /qn /norestart
```
Exit 0. `OptIn=0` present in both registry views; product registered as
`Kits Configuration Installer 10.1.19041.5609`.

Official SDK ISO at `E:\devtools\dl\windows-sdk-19041\windows-sdk-19041.iso`
(may still be mounted as `D:`).

**Outstanding (optional, non-blocking):** revert an unnecessary explicit
Administrators ACE I added to `HKLM\SOFTWARE\Microsoft\Windows Kits`:
```
powershell -NoProfile -ExecutionPolicy Bypass -File E:\devtools\revert-kits-acl.ps1
```
Backup: `E:\devtools\windows-kits-backup.reg`.

**Lessons:** never pass `--nocache` to VS/SDK installers; never write to
`C:\ProgramData\Package Cache` (TrustedInstaller-owned); always read the MSI
`/l*v` verbose log instead of hypothesising.

---

## 4. Phase status

| Phase | Status |
|---|---|
| **P0** scaffolding, 2 branded binaries, Android rename | ✅ complete |
| **P1** P2P transport skeleton (`rr-webrtc`) | ✅ complete |
| **P2** video path (`rr-capture`, `rr-codec`, video track) | ✅ complete — committed, CI wired, validated on real Windows hardware |
| **P7a** Windows fast-path capture (`DxgiCapturer`) | ✅ complete — **pulled forward from P7**, user-approved |
| **P7b** hardware H.264 encode (`rr-hwcodec`) | ✅ complete — **4K at 41.2 fps**, committed. See §11 |
| P3 input control + arbitration | ✅ **complete** — `rr-input` crate + the live `rr-webrtc` wiring (`StationControl`, §12 step 5) both done, verified, committed both machines |
| P4 NAT traversal / TURN Docker | ⬜ |
| P5 multi-viewer | ⬜ |
| P6 clipboard + file transfer | ⬜ |
| P8 UI polish, security review, acceptance | ⬜ |

Added after the phase plan was written, by user decision — **not** numbered into
the P-series because they cut across it:

| Work | Status |
|---|---|
| **Monitor origins** in `MonitorInfo` (wire + capture + layout) | ✅ done, verified both machines, committed — §12 |
| **§14 radio audio & rig PTT**: stereo, transmit modes, serial PTT | 🟢 **in progress** — §14c transmit-mode decoupling done + committed; next is §14f.1 sample-rate. See §14g for order |
| **§13 macOS target** | ⬜ **backburner** (investigated only; Linux desktop never a target) |

**Why P7a moved:** `xcap` capture measured ~33 ms/frame at 1920x1200, which
alone exceeded the whole 33.3 ms budget for 30 fps, so every later phase would
have been validated against an unreachable frame rate. The user approved pulling
the capture backend forward rather than proceeding to P3 at ~20 fps.

### Workspace crates (`ARMumble/Cargo.toml` members)
Original: `fancy-utils`, `fancy-denoiser-deepfilter`, `mumble-protocol`,
`mumble-tauri`. (`crates/signal-bridge` is **excluded** — AGPL, built separately.)

Added by this project:
```
crates/rr-proto      # wire types (control/clipboard/file + presence)
crates/rr-session    # cooperative control-arbitration state machine
crates/rr-webrtc     # P2P transport: offer/answer, trickle ICE, data + video
crates/rr-capture    # ScreenCapturer trait, synthetic source, DXGI + xcap backends
crates/rr-codec      # H.264 encode/decode + runtime hardware/software selection
crates/rr-hwcodec    # Windows Media Foundation hardware H.264 (the only unsafe rr-* crate)
crates/app-client    # [[bin]] RemoteRadioClient (thin)
crates/app-station   # [[bin]] RemoteRadioStation (thin)
```

Written but **not yet a member** (§12): `crates/rr-input`.

---

## 5. P0 details (complete)

**Key insight:** `tauri::generate_context!()` reads `tauri.conf.json` from the
**calling crate's** `CARGO_MANIFEST_DIR` at compile time. So each branded binary
**must** call `generate_context!()` itself. Hence:
- `mumble-tauri` gained `Mode { Client, Station }` and
  `run_with(context: tauri::Context, mode: Mode)`; `run()` kept for Android
  (defaults to `Client`). `.manage(mode)` so the backend can read it.
- `app-client` / `app-station` are thin binaries, each with its own
  `tauri.conf.json` (`com.remoteradio.client` / `com.remoteradio.station`),
  `frontendDist: "../mumble-tauri/ui/dist"`, own `icons/` dir, and a
  `plugins.updater` block (its absence crashed startup).
- Each needs **`serde_json`** as a dependency — `generate_context!` expands to
  code referencing it once a `plugins` section exists.

**Android rename:** display name via
`gen/android/app/src/main/res/values/strings.xml` (`app_name`,
`main_activity_title`) **and** `mumble-tauri/tauri.conf.json`
`productName`/`title`. `applicationId`/`namespace` stay `com.fancymumble.app`
for signing/push/deep-link continuity. Verified in the APK with `aapt2`.

**Gotcha:** `cargo tauri android build` must run from
`crates/mumble-tauri` — with multiple binary crates it otherwise guesses
`app-client` and errors `gen/android doesn't exist`.

### Three launch bugs found by actually running the binaries
1. **Crash on launch** — shared lib registers `tauri-plugin-updater` but the new
   configs had no `plugins.updater` → `PluginInitialization` panic. Fixed by
   adding the block + `serde_json`.
2. **Relaunch loop** — updater startup check hit a nonexistent release feed and
   kept reopening/relaunching. Fixed: `updater::init` has
   `const UPDATER_ENABLED: bool = false` and now reveals the main window
   directly until a real feed exists.
3. **Both apps couldn't run together** — `platform/windows.rs` hardcoded a single
   `DEEP_LINK_PORT = 58722`, so the second to start exited silently. **Fatal for
   this project.** Fixed with `DEEP_LINK_PORT_BASE` + `deep_link_port()` deriving
   a per-variant offset from the exe name (station +1 → 58723, client +2 → 58724).

### Branding
Shared UI resolves its name at runtime: backend command **`app_mode`**
(in `commands/window.rs`, returns `"client" | "station" | "mumble"`) +
`ui/src/utils/brand.ts` (`getAppMode()`, `brandKeyForMode()`, `BrandKey`) +
`LoadingSplash.tsx`. Locale keys `brand`, `brandClient`, `brandStation`,
`brandMumble` in `ui/src/locales/en/common.json`.
Note: translate with the component's own `t` (react-i18next), **not** the raw
`i18next` singleton, or the test harness renders empty.

### Icons
User supplied `E:\AI\RRMumble\RemoteRadioClient.svg` and
`E:\AI\RRMumble\remoteradiostation.svg` (512×512 viewBox). Uploaded to
`big-linux:/home/gerry/RRMumble/icons-src/`. Generated per-app sets with
`cargo tauri icon <svg> -o icons` **run inside each crate** (it accepts SVG and
emits ico/png/icns/Android mipmaps/iOS/Appx). Distinct artwork confirmed
(client `icon.ico` 46537 B vs station 36050 B). Both `tauri.conf.json`
`bundle.icon` arrays point at their own `icons/`.

### Smoke-test gate (added after the crash reached the user)
`E:\devtools\smoke-test-client.ps1 -ExePath <exe> [-SettleSeconds n]` — verifies
the process stays alive, PID set is stable (relaunch-loop detection), webview
children spawn, and stderr has no panic/PluginInitialization.
`E:\devtools\smoke-test-both.ps1` — verifies Client **and** Station run
simultaneously and bind different deep-link ports. **Both PASSED.**

### P0 artifacts (in `E:\AI\RRMumble\artifacts\`)
- `RemoteRadioClient_0.3.0_x64-setup.exe`
- `RemoteRadioStation_0.3.0_x64-setup.exe`
- `RemoteRadioMumble-0.3.0-arm64.apk`

Portable (no install) copies for testing: `E:\AI\RRMumble\portable\<App>\`
(exe + `signal_bridge.dll`).

---

## 6. Existing in-repo WebRTC (important background)

The fork **already had** a working screen-share, but it is **SFU-based** and
browser-based, which is *not* what we're building:
- Signaling already exists and is fully wired: **`WebRtcSignal`, Mumble wire
  ID 120**, with `SignalType { START=0, STOP=1, SDP_OFFER=2, SDP_ANSWER=3,
  ICE_CANDIDATE=4 }`, a `payload` string, `target_session` (0 = broadcast to
  channel), server-stamped `sender_session`.
  - Backend inbound: `mumble-tauri/src/state/handler/webrtc_signal.rs` → emits
    the `webrtc-signal` Tauri event.
  - Backend command: `commands/realtime.rs::send_webrtc_signal(target_session,
    signal_type, payload, server_id)`.
  - Frontend: `store.ts` `sendWebRtcSignal` / `onWebRtcSignal` + `listen`.
- Media, however, flows **client ↔ server SFU** (never peer-to-peer); the SFU is
  an external server module **not in this repo**. Capture/encode/decode all
  happen in the **webview** (`getDisplayMedia`), with a
  `webrtc_sfu_available` capability flag.
- There is **no native media code** and **no RTCDataChannel** anywhere — both
  are greenfield, which is what `rr-webrtc`/`rr-capture`/`rr-codec` provide.

**We reuse the signaling, not the media path.**

---

## 7. P1 details (complete) — `crates/rr-webrtc`

### Verified `webrtc` 0.21.0 API (a REDESIGN; old 0.1x examples are wrong)
- Async wrapper over a sans-IO core; runtime-agnostic, default feature
  `runtime-tokio`. Needs `async-trait`.
- Build: `PeerConnectionBuilder::new().with_configuration(cfg)
  .with_handler(Arc::new(h)).with_udp_addrs(vec![...]).build().await?`
  - `with_udp_addrs` is generic `Vec<A>` requiring `'static` → pass owned
    `Vec<String>`, **not** `Vec<&str>` borrowed from config (E0521).
- Config: `RTCConfigurationBuilder::default().with_ice_servers(vec![RTCIceServer{
  urls, ..Default::default() }]).build()`
- `PeerConnection` trait methods all take `&self` (Arc-shareable), async:
  `create_offer`, `create_answer`, `set_local_description`,
  `set_remote_description`, `add_ice_candidate(RTCIceCandidateInit)`,
  `create_data_channel(&str, Option<RTCDataChannelInit>)`, `close`.
- Events: implement `PeerConnectionEventHandler` with `#[async_trait]`; all
  methods have default no-ops: `on_ice_candidate(RTCPeerConnectionIceEvent)`,
  `on_connection_state_change(RTCPeerConnectionState)`,
  `on_data_channel(Arc<dyn DataChannel>)`, `on_track(Arc<dyn TrackRemote>)`, etc.
- `RTCIceCandidate::to_json() -> Result<RTCIceCandidateInit>` for trickling.
- **Data channels use a poll model, not callbacks:**
  `while let Some(event) = dc.poll().await` → `DataChannelEvent { OnOpen,
  OnError, OnClosing, OnClose, OnBufferedAmountLow, OnBufferedAmountHigh,
  OnMessage(RTCDataChannelMessage { is_string: bool, data: BytesMut }) }`.
  `send(BytesMut)`, `send_text`, `try_send`, `writable`, `close`, `id()`.

### Structure
- `src/lib.rs` — `TransportError`, `Result`, `SessionObserver` trait,
  `RecordingObserver` (optional mpsc forwarding for deterministic tests),
  `NullObserver`, `DEFAULT_STUN_SERVERS`, `CONTROL_CHANNEL_LABEL = "rr-control"`.
- `src/signal.rs` (+ `src/signal/tests.rs`) — `SignalMessage` enum tagged
  `"kind"` mirroring Mumble's `SignalType` 1:1
  (`start/stop/offer/answer/ice_candidate`); `SignalTransport` async trait
  (`send`/`recv`); `InMemorySignaling::pair()` for tests. **This is what makes
  the handshake testable with no server.**
- `src/session.rs` — `ConnectionState` (mapped from `RTCPeerConnectionState`),
  `SessionConfig` (`::default()` with STUN, `::loopback()` host-candidates-only),
  internal `Handler`, and `PeerSession` with:
  `offer()` (Client role — creates the control channel, sends offer),
  `answer()` (Station role), `run()` (signaling loop),
  `wait_control_channel()`, `send_control()`, `request_keyframe()`, `close()`.
  Trickle ICE both ways; candidates arriving **before** the remote description
  are queued and flushed (they legitimately race the SDP).
- `src/video.rs` (+ `src/video/tests.rs`) — see P2 below.
- `tests/loopback.rs` — 2 integration tests.

**Roles:** Client (viewer) = **offerer**; Station (host) = **answerer**. Keeps
the Station passive until asked and scales to N viewers, one PC each.

---

## 8. P2 details — `rr-capture` + `rr-codec` + video track

### Verified APIs
**xcap 0.9.8** — `Monitor::all() -> XCapResult<Vec<Monitor>>`; all accessors
return `XCapResult`: `id()`, `friendly_name()`, `width()`, `height()`,
`scale_factor()`, `is_primary()`; `capture_image() -> XCapResult<RgbaImage>`
(**RGBA**, tightly packed), `capture_region(x,y,w,h)`. Screen capture works on
Windows ≥8.1 / macOS / Linux-X11; **Wayland not fully supported**. xcap frames
carry **no stride/format/timestamp** — `rr-capture` adds them.

**openh264 0.9.8** — **BSD-2** (Cisco core *and* wrapper) → safe to redistribute
with this MIT codebase. `nasm` on PATH = up to 3x faster.
- `Encoder::with_api_config(OpenH264API::from_source(), EncoderConfig)`,
  `encode(&impl YUVSource)`, `encode_at(.., Timestamp)`,
  **`force_intra_frame()`** ← keyframe-on-demand.
- `EncoderConfig` setters: `bitrate`, `max_frame_rate`, `usage_type`,
  `rate_control_mode`, `intra_frame_period`, `sps_pps_strategy`, `profile`,
  `level`, `complexity`, `qp`, `num_threads`, `vui`, `scene_change_detect`, …
- Wrapper types: `BitRate::from_bps`, `FrameRate::from_hz`,
  `IntraFramePeriod::from_num_frames` / `::auto`.
- Enums: `UsageType::ScreenContentRealTime` (what we use),
  `RateControlMode::{Quality,Bitrate,Bufferbased,Timestamp,BitrateModePostSkip,Off}`,
  `FrameType::{Invalid,IDR,I,P,Skip,IPMixed}`.
- `formats::YUVBuffer` — `new(w,h)`, `from_*_source`, **`read_rgba8()`** (reuse
  the buffer; per-frame alloc at 30 fps is pure waste). Slice wrappers
  `RgbaSliceU8`, `BgraSliceU8`. `YUVSource` needs `dimensions/strides/y/u/v`.
- `decoder::Decoder::decode(&[u8]) -> Result<Option<DecodedYUV>>`
  (**`None` is normal**, e.g. parameter sets); `DecodedYUV::write_rgba8(&mut [u8])`.

**Track types live in webrtc's sans-IO core crates, not re-exported from
`webrtc`'s root** — this cost two compile rounds:
- `rtc-media = "0.21.0"` → `rtc_media::Sample { data: Bytes, timestamp: Instant,
  duration, packet_timestamp: u32, prev_dropped_packets: u16, .. }`.
  **No `Default`** — use `Sample::new(now)` then assign fields.
- `rtc = "0.21.0"` → `rtc::rtp_transceiver::{SSRC, PayloadType}` and
  `rtc::rtp_transceiver::rtp_sender::RtpCodecKind`
  (**the `rtp_codec` module that defines it is private** — import via the
  `rtp_sender` re-export).
- `webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample::new(
  now: Instant, track: MediaStreamTrack) -> Result<Self>` (**returns Result**);
  `.sample_writer(ssrc, payload_type).write_sample(&Sample).await`.
- `MediaStreamTrack::new(stream_id, track_id, label, RtpCodecKind, codings)`.
- `TrackRemote` also uses the `poll()` model.

### `crates/rr-capture`
Feature **`screen-capture = ["xcap", "tracing"]`**; **default features off** so
CI and the Client need no display server or dependency.
- `CaptureError { Backend, UnknownMonitor, NoMonitors, InconsistentFrame }`
- `PixelFormat { Rgba8, Bgra8 }` with `bytes_per_pixel()`
- `MonitorInfo { id, name, width, height, primary }`
- `Frame { data, width, height, stride, format, captured_at: Instant,
  sequence: u64 }` with `validate()`, `row(y)` (strips padding), `age()`
- `trait ScreenCapturer { monitors(), select_monitor(id), grab() }` (sync, `Send`)
- `SyntheticCapturer::new(w,h)` — **rounds dimensions down to even** (H.264 4:2:0
  requires it; odd sizes otherwise fail deep in the encoder),
  `.with_row_padding(n)`, and `expected_pixel(width,x,y,sequence)` as a shared
  test oracle. Renders a moving white vertical bar over a static gradient so the
  encoder sees real motion.
- `XcapCapturer::new()` — resolves the primary monitor by default.
- **11 tests pass.**

### `crates/rr-codec`
Depends on `rr-capture` + `openh264` with `features = ["source"]`.
- `CodecError { EncoderInit, Encode, DecoderInit, Decode, Geometry{width,height,
  reason}, UnsupportedFormat }`; `check_geometry()` rejects zero/odd
  (uses `is_multiple_of(2)` — clippy requires it over `% 2 != 0`).
- `EncoderSettings { bitrate_bps: 8_000_000, max_fps: 30.0 }`
- `Encoder::new(w,h,settings)` → `UsageType::ScreenContentRealTime`,
  `RateControlMode::Bitrate`, `IntraFramePeriod::from_num_frames(0)`
  (**periodic keyframes disabled** — their bitrate spike shows up as a stall).
  Methods: `dimensions()`, `request_keyframe()`, `keyframe_pending()`,
  `encode(&Frame) -> EncodedFrame { data, is_keyframe, sequence }`.
  Repacks padded rows (`RgbaSliceU8` needs packed rows). Reuses `YUVBuffer`.
  **`force_keyframe` is cleared only after a successful encode**, so a failure
  cannot swallow a viewer's request. `first_frame` forces the opening IDR.
- `Decoder::new()`, `decode(&[u8]) -> Result<Option<DecodedFrame>>`;
  `DecodedFrame { rgba, width, height }` with `stride()`, `pixel(x,y)`.
- **14 tests pass**, incl. encode→decode round trip, pixel-level similarity vs
  the synthetic source, keyframe-on-demand, IDR>delta sizing, padded strides,
  and the decoder surviving garbage without panicking.

### `rr-webrtc/src/video.rs`
- `H264_PAYLOAD_TYPE = 102` (dynamic range 96–127), `VIDEO_STREAM_ID`,
  `VIDEO_TRACK_ID`.
- `VideoSender::new(ssrc) -> Result<Self>`, `.track()`,
  `.send_encoded(&[u8])` — derives sample duration from the gap since the last
  write, clamping zero/absurd values.
- `VideoReceiver` — `push_access_unit()`, `take_access_units()`,
  `pending_count()`.
- `PeerSession::request_keyframe()` sends `ControlMsg::RequestKeyframe`.
- `rr-capture`/`rr-codec` are **dev-dependencies** of `rr-webrtc` (used only by
  the integration test); the library stays codec-agnostic and moves opaque
  bytes. Needed `#[cfg(test)] use rr_capture as _;` (same for `rr_codec`) to
  satisfy `unused_crate_dependencies`.
- `tests/video_pipeline.rs` — 3 tests: capture→encode→decode chain,
  video-sender creation, and a **live P2P session carrying a real keyframe
  request** Client→Station.

### Verification status (big-linux)
Last full run: **`CLIPPY_EXIT=0`** and **`TEST_EXIT=0`** across all five
RemoteRadio crates (`rr-proto`, `rr-session`, `rr-webrtc`, `rr-capture`,
`rr-codec`) under `-D warnings` with the project's strict lints.
Test counts seen: rr-proto 12, rr-session 19, rr-webrtc lib 9,
loopback 2, video_pipeline 3, rr-capture 11, rr-codec 14.

Verification scripts on big-linux: `test-p2.sh`, `check-video.sh`,
`test-rr-codec.sh`, `test-rr-webrtc.sh`, `build-rr-webrtc.sh`,
**`verify-dxgi.sh`**, **`verify-targeted.sh`**, **`lock-check.sh`**,
`wait-verify.sh` (all authored locally in `E:\devtools\` then `scp`'d).

---

## 8b. P7a: the DXGI capture backend (complete)

Commits: local **`e0bc2b88`**, big-linux **`f861c008`** + **`a9d9ade9`**.
Full write-up with all numbers: `doc/remote-radio/P2-BASELINE.md`.

### Result
| | `xcap` (GDI) | `DxgiCapturer` |
|---|---|---|
| capture work / frame | ~33 ms (flat) | **~3.9 ms** (3.7–5.3 ms, load-sensitive) |
| vs 0.92 MiB/ms memcpy floor | 36x | 4.2x |
| capture + encode | ~50 ms | **~22 ms** |
| sustainable | 20 fps | **37–46 fps** |
| static screen | full price every frame | **free** |

**Capture is no longer the bottleneck — encode is.** And resolution decides
whether the whole thing works:

| | 1920x1200 (2.3 Mpx) | 3840x2160 (8.3 Mpx) |
|---|---|---|
| capture (DXGI) | 3.9 ms | 16–18 ms |
| encode (real desktop content) | 17–21 ms | **56–114 ms** |
| sustainable | **37–46 fps** | **7.5–13.8 fps** |

⚠️ **4K misses 30 fps by 2–4x and encode is 78% of the frame time.** Software
config is *not* a lever: all ten `complexity`/`num_threads` combinations land
within 6% (`encoder_tune` example), and `threads=1` == `threads=8`, so
`OpenH264`'s internal threading does not parallelise this workload at all.

Also learned: **encode cost depends on content far more than configuration** —
15 ms for a high-entropy synthetic 4K frame vs 56–114 ms for a real mostly-static
4K desktop, same encoder. Motion estimation searches hardest when there are many
plausible matches, so `OpenH264` is slowest on exactly what a remote desktop
produces. Never benchmark this encoder on synthetic frames.

And: **one-pass benchmarking on this machine is worthless** — identical configs
varied up to 4x, with position-in-run predicting the result better than the
setting. `encoder_tune` interleaves rounds and takes the minimum (spread → 6%).

### Implementation
- `crates/rr-capture/src/dxgi_backend.rs`, feature **`dxgi-capture`**,
  `windows-capture = "2.0.1"` under `[target.'cfg(windows)'.dependencies]`
  (the implicit feature still resolves on Linux; the module is
  `#[cfg(all(windows, feature = "dxgi-capture"))]`).
- Uses that crate's **`dxgi_duplication_api`**, not its WGC handler. It is
  **pull-based** (`acquire_next_frame(timeout_ms)`), so it fits
  `ScreenCapturer::grab()` directly — no push-to-pull bridge, no `unsafe` of
  ours, `#![forbid(unsafe_code)]` intact.
- **Worker thread is mandatory**, not a workaround: `windows-rs` does not mark
  COM interfaces `Send`, and `ScreenCapturer: Send`. `DxgiCapturer` is a `Send`
  handle posting `Box<dyn FnOnce(&mut Engine) + Send>` jobs to one thread that
  owns the D3D11 device for its whole life.
- Reuses one `StagingTexture` (`frame.buffer_with(&mut staging)`); build it from
  `dup.duplication_desc().ModeDesc.Format` so no `DXGI_FORMAT` needs naming.
  Borrow trick: destructure `&mut Session` into `{ dup, staging, .. }`.
- **BGRA8** requested (native desktop format, zero conversion). `rr-codec`
  gained `YUVBuffer::read_bgra8(BgraSliceU8::new(..))`; `CodecError::
  UnsupportedFormat` was deleted since the `match` is now exhaustive.
- `AccessLost` (mode change, secure desktop, fast user switching, DWM flip)
  rebuilds the session and retries **once**. Detected by message substring
  because `dup_err` flattens the wrapper's error type.
- ⚠️ **The acquired frame's texture format can differ from
  `duplication_desc().ModeDesc.Format`.** Assuming they match broke 4K capture
  outright (`Invalid staging texture: format must match the frame`). `grab_once`
  compares the staging texture's desc against `frame.texture_desc()` and rebuilds
  when they disagree. Trust the frame, not the mode description.
- Monitor ids = one-based `\\.\DISPLAYn` number (1, 2) via `Monitor::index()`.
  **Differs from `xcap`'s HMONITOR-derived ids** (65639, 131073), so a persisted
  monitor choice is backend-specific.
- New `CaptureError::NoNewFrame` = "screen is still". **Not an error**; callers
  must loop. `DEFAULT_TIMEOUT_MS = 100`.
- New `ScreenCapturer::last_wait() -> Option<Duration>` (defaults to `None`).
  Without it, timing `grab()` on a quiet desktop measures the user's idleness.
  A first attempt used `with_timeout_ms(0)` instead, which works but busy-spins
  a core and inflated the encode timings measured in the same loop.

### Verified APIs (`windows-capture` 2.0.1)
```rust
monitor::Monitor::{enumerate, primary, from_index, index, name, device_name,
                  width, height, as_raw_hmonitor}   // Copy + Eq + Send
dxgi_duplication_api::DxgiDuplicationApi::{new, new_options, recreate,
    device, duplication_desc, width, height, format, acquire_next_frame}
DxgiDuplicationFrame::{buffer, buffer_with, texture_desc, device}
DxgiDuplicationFrameBuffer::{width, height, row_pitch, format, has_padding,
    as_raw_buffer, as_nopadding_buffer}
d3d11::StagingTexture::new(device, w, h, DXGI_FORMAT)
Error::{Timeout, AccessLost, OutputNotFound, InvalidStagingTexture, ...}
```
`enable_per_monitor_dpi_awareness` is called internally and tolerates
`E_ACCESSDENIED`, so it is safe inside an already-DPI-aware Tauri process.

### Tooling
- `crates/rr-capture/examples/capture_bench.rs` — attributes `xcap`'s cost and
  measures DXGI work with the wait subtracted, plus an explicit warm-up grab
  (session setup is ~70 ms and must not land in the samples).
- `crates/rr-codec/examples/capture_probe.rs` — runs the **full pipeline against
  every compiled-in backend** and prints a side-by-side comparison.
  `--features real-capture,dxgi-capture`. Takes an optional **1-based monitor
  index** (an index, not an id — ids are backend-specific): `capture_probe.exe 2`
  measures the second display. **This is the trustworthy instrument** for
  absolute numbers because it uses real screen content.
- `crates/rr-codec/examples/encoder_tune.rs` — sweeps `OpenH264` `complexity` and
  `num_threads`, interleaved, best-of-3. Also takes a monitor index. Its
  synthetic workload is *not* representative; use it for comparing settings, not
  for absolute figures.
- Tests: rr-capture 11→**14**, rr-codec 14→**17**. The valuable one encodes the
  same image through the RGBA *and* BGRA paths and compares decoded output — a
  red/blue swap would show up immediately, where "does it error?" would not.
- CI gained a Windows-only clippy pass for the feature-gated backends (the
  default pass never compiled them) and the OpenH264 assembly assertion below.

---

## 8c. The NASM trap (cost ~1 hour; never repeat)

The first run of the new probe reported encode at **139–164 ms** instead of
~17 ms, **on both backends**. Not the new code. The local Windows build had no
`nasm` on PATH and `openh264-sys2` says so only in its build log, then carries on:

```text
target/release/build/openh264-sys2-*/output:
Failed to compile NASM files, not using any assembly.
```

OpenH264's hand-written SIMD kernels were dropped for the pure-C fallback:
**~10x slower encode, no error anywhere.** Installing nasm 3.02 reproduced the
recorded baseline to within noise (`xcap` 32.6 ms vs 33.3 ms recorded; encode
17.2 ms vs 16.6 ms), which also cross-checks that both builders agree.

Guards now in place:
- `E:\devtools\cargo-msvc.cmd` exits with `NASM_MISSING` if nasm is absent.
- CI step **"Verify OpenH264 built its assembly kernels"** greps for that line
  and fails the build.

**Rule: always reproduce the old baseline before trusting a comparison to it.**

---

## 9. Project conventions (strict — enforced by workspace lints)

From `ARMumble/Cargo.toml`: `unsafe_code = deny`, `missing_docs = deny`,
`unused_results = deny`, `unwrap_used = deny`, `expect_used = warn`,
`doc_markdown = warn`, `allow_attributes_without_reason = deny`,
`excessive_nesting = deny`, `too_many_lines = deny`.

Consequences for new code:
- Production code uses **zero** `.unwrap()` / `.expect()`.
- Every identifier like `RemoteRadio`, `WebRTC`, `OpenH264` in a doc comment
  must be in **backticks**.
- Test modules need, with reasons:
  ```rust
  #![allow(clippy::unwrap_used, reason = "unwrap is acceptable in test code")]
  #![allow(clippy::expect_used, reason = "expect is acceptable in test code")]
  #![allow(unused_results, reason = "...")]            // if results ignored
  #![allow(unused_crate_dependencies, reason = "...")] // integration tests
  ```
  (Pattern copied from `crates/mumble-tauri/src/state/handler/tests.rs`.)

Lints found only on big-linux, not locally (its clippy is stricter/newer):
- `chunks_exact_to_as_chunks` → use `as_chunks_mut::<4>().0`.
- `dead_code` on helpers shared by two feature-gated functions — gate the helper
  module with the same `cfg(any(...))` rather than reaching for
  `allow(dead_code)`.
- A Windows-only dependency must go under
  `[target.'cfg(windows)'.dependencies]`, not plain `[dependencies]` — this bit
  `rr-capture` inside `rr-hwcodec`.
- `tracing::warn` must be **qualified at the call site** (not imported) in
  `rr-codec/src/select.rs`, or the unused-import lint fires on the other target.

CI (`.github/workflows/ci.yml`) now: builds the shared UI before clippy, runs
`cargo clippy --workspace --all-targets -D warnings`, a **Windows-only clippy
pass for the feature-gated capture backends**, the **OpenH264 assembly
assertion** (§8c), `cargo test` across all five RemoteRadio crates, and builds
both branded desktop apps plus the Android APK.

---

## 10. Immediate next steps

### Current heads (both clean, 0 dirty, **not pushed to origin**)

> ⚠️ **Updated.** Since this section was first written, several more batches
> landed (monitor origins, Opus stereo controls, the stereo_live harness, the
> Fancy 1.6 server, the completed `rr-input` crate, the P3 `rr-webrtc`
> `StationControl` wiring, and §14c transmit-mode decoupling), all committed on
> both machines with divergent histories but byte-identical content. **Latest
> heads:**
>
> | | HEAD | last work |
> |---|---|---|
> | local `e:\AI\RRMumble\ARMumble` | **`0c3ba9f7`** | §14c (`ca768be9`→`0c3ba9f7`) on top of P3 wiring (`5a051797`→`90dd3313`) |
> | big-linux `/home/gerry/RRMumble/ARMumble` | **`f7280c54`** | §14c (`f03a266b`→`f7280c54`) on top of P3 wiring (`c0f8d657`→`29d38278`) |
>
> P3 `rr-webrtc` wiring = 4 commits (deps, StationControl fabric, lib wiring,
> integration tests); §14c = 2 commits (TransmitMode enum/field, gate decoupling
> + tests). Content parity sha256-verified per batch.

The table below is the **historical** snapshot from when §10 was written; keep it
for the commit hashes it records, not as the current tip.

| | HEAD | note |
|---|---|---|
| local `e:\AI\RRMumble\ARMumble` | **`9e6672c1`** | 4 commits past P7b |
| big-linux `/home/gerry/RRMumble/ARMumble` | **`d1c3fcac`** | same content, different history |

The four commits, in order (identical messages on both machines, different
hashes because the histories diverged long ago):

| local | big-linux | subject |
|---|---|---|
| `cf57fe11` | `234ef46d` | `feat(rr-proto,rr-capture)`: real monitor origins |
| `3ec5f472` | `ca40d2a4` | `fix(clippy)`: five pre-existing lints |
| `61b2e84f` | `b97b9ff9` | `feat(mumble-protocol)`: Opus channel proof + stereo controls |
| `9e6672c1` | `d1c3fcac` | `wip(rr-input)`: incomplete, not a workspace member |

Content parity confirmed by `sha256` on `rr-proto/src/lib.rs`,
`rr-capture/src/dxgi_backend.rs`, `mumble-protocol/tests/opus_channels.rs`,
`mumble-protocol/src/audio/encoder.rs` and `rr-input/src/layout.rs` — all
byte-identical. Committed state verified green: **386 tests, every clippy pass 0,
no lock drift.**

⚠️ **Git identity differs per machine, by existing convention.** Local is
configured (`w1ve <gerry@w1ve.com>`). **big-linux has no identity at all**, so
commits there must pass one per invocation:
```bash
git -c user.name=kiro -c user.email=kiro@local commit -F msgfile
```
That is what the pre-existing `commit-*.sh` scripts do and what `git log -1
--format=%an` shows. **Do not run `git config`** to "fix" it.

⚠️ **Two commit-script traps, both hit this session.** `Set-Content -Encoding
utf8` under **PowerShell 5.1 writes a BOM**, and git keeps it, so every commit
subject began with an invisible U+FEFF; the first four commits had to be redone
(`git reset <base>` — mixed, so the working tree was untouched). Use
`[System.IO.File]::WriteAllText($f, $msg, (New-Object System.Text.UTF8Encoding($false)))`,
or invoke `pwsh` rather than `powershell`. Scripts:
`E:\devtools\commit-session.ps1` (local) and `commit-remote.sh` (big-linux).

⚠️ **`context.md` — this file — is NOT under version control.** It sits at
`e:\AI\RRMumble\context.md`, outside the `ARMumble` repo, and
`e:\AI\RRMumble` is not a git repository. The project's primary handoff document
has no history and no backup. Worth fixing.

Uncommitted locally, **not yet on big-linux**:
- **`?? crates/rr-input/`** (§12).
- The **monitor-origin change** (§12, resolved): `rr-proto/src/{lib,tests}.rs`,
  `rr-capture/Cargo.toml`, `rr-capture/src/{lib,synthetic,xcap_backend,
  dxgi_backend,tests}.rs`, `rr-codec/examples/capture_probe.rs`, and
  `rr-input/src/layout.rs`. `Cargo.lock` and the workspace `Cargo.toml` are
  **unchanged** — no new packages, `xcap` was already an optional dependency.
- **Stereo groundwork** (§14b): `mumble-protocol/src/audio/encoder.rs` (new
  `ForceChannels` enum + `phase_inversion_disabled`, both defaulting to Opus's own
  defaults so mono is unchanged), and the new
  `mumble-protocol/tests/opus_channels.rs` (8 tests).
- **Five pre-existing clippy fixes**, none caused by this work, all found by
  running clippy over crates and feature combinations nothing routinely covered:
  `rr-webrtc/src/{lib,signal}.rs` (`double_must_use` ×5),
  `rr-input/src/gate.rs` (`branches_sharing_code`), and **three
  `chunks_exact_to_as_chunks`** in `mumble-protocol`:
  `src/audio/decoder.rs`, `src/audio/mixer.rs` (a test), and
  `src/audio/filter/denoiser/rnnoise.rs`.

**Both machines carry the identical working tree**, synced by
`E:\devtools\sync-origins.ps1` → `scp` → `tar -xzf`. big-linux is still at
`df47169d`; **nothing is committed on either side.**

### ⚠️ Why five pre-existing lints were hiding
`cargo clippy --workspace --all-targets` uses **default features**, so it never
compiles `mumble-protocol`'s `opus-codec` / `rnnoise-denoiser` code, and CI only
ever *tests* those paths (`cargo test --features opus-codec --lib`) rather than
linting them. The three `chunks_exact_mut` sites sat in exactly that blind spot.
`rr-webrtc`'s `double_must_use` hid differently: it needs clippy **1.99** (local),
not big-linux's 0.1.98.

**Lesson for CI:** add a clippy pass over `mumble-protocol` with
`--features opus-codec,rnnoise-denoiser --all-targets`, mirroring the Windows-only
capture-backend pass added in §8b for the same reason. Without it these reappear.

### Toolchain gaps found this session
- ✅ **`protoc` now installed locally — `mumble-protocol` builds on Windows for the
  first time.** It never had before: `build.rs` drives `prost-build`, which
  refuses to run without `protoc`, which is why only the `rr-*` crates had ever
  been built locally and every audio test had to go to big-linux.
  - **`E:\devtools\protoc\bin\protoc.exe`, pinned to 25.3** to match big-linux's
    `~/.local/bin/protoc` exactly, so generated code cannot differ between the two
    machines because of a compiler version. Portable extract, matching how cmake,
    ninja and nasm are already handled — no installer, nothing in Program Files.
    Installed by `E:\devtools\install-protoc.ps1`; zip cached at
    `E:\devtools\dl\protoc-25.3-win64.zip`, sha256
    `D6B336B852726364313330631656B7F395DDE5B1141B169F5C4B8D43CDF01482`.
  - **`E:\devtools\cargo-msvc.cmd` now puts it on `PATH` *and* sets `PROTOC`**
    (`prost-build` consults the variable first), with a `PROTOC_MISSING` guard
    mirroring the existing `NASM_MISSING` one. Unlike the NASM case there is no
    silent fallback — the build fails outright — but it failed quietly enough that
    nobody noticed the crate had never been built here.
  - Cross-checked against big-linux: the `opus_channels` separation figures are
    **bit-identical below the 96 kbps cliff** (5.4/5.6 dB at 48k, 13.0/13.3 dB at
    72k) and differ above it (local 44.8/71.3 dB vs remote 52.6/61.6 dB at 96k),
    where residual crosstalk is in the numerical noise floor and `libopus` build
    differences show through. Both are far beyond audible, so the conclusion is
    unaffected — but note the absolute figures above the cliff are **build
    dependent** and should not be compared across machines.
- ⚠️ **On big-linux `protoc` is at `$HOME/.local/bin/protoc`** (25.3, per
  `opt/BUILDING-LOCAL.md`) and is on **neither** the cargo bin dir nor the brew
  prefix. Every remote script must export
  `PATH="$HOME/.cargo/bin:$HOME/.local/bin:/home/linuxbrew/.linuxbrew/bin:$PATH"`
  — the brew-and-cargo pair documented in §2 is not sufficient.

- Two **pre-existing** clippy fixes found while verifying the above, and fixed:
  `rr-webrtc/src/{lib,signal}.rs` and `rr-input/src/gate.rs` (see below).

### Verified on both machines

| check | where | result |
|---|---|---|
| clippy, default features, `--all-targets`, `-D warnings` | local Windows | **0** |
| clippy, full capture feature set | local Windows | **0** |
| clippy, default features, all 6 `rr-*` crates | big-linux native | **`CLIPPY_LINUX=0`** |
| clippy, **Windows target**, all real backends (`cargo xwin`) | big-linux | **`CLIPPY_WIN=0`** |
| clippy, `mumble-protocol` + `opus-codec,rnnoise-denoiser` | big-linux | **`MP_CLIPPY=0`** |
| tests, `rr-*` crates | both | **88 passed** |
| tests, `mumble-protocol` lib | big-linux | **248 passed** |
| tests, `opus_channels` (new) | big-linux | **8 passed** |
| tests, `audio_quality` (mono regression guard) | big-linux | **21 passed** |
| **total** | big-linux | **365 passed, 0 failed** |
| `openh264` assembly kernels | big-linux | **`NASM_FALLBACK=0`** |
| `Cargo.lock` drift | big-linux | **none** |

The `audio_quality` suite passing is the specific evidence that adding two fields
to `OpusEncoderConfig` changed nothing for mono: its 21 spectral/level assertions
are all measured on the mono path.

One-shot re-run of all of the above:
```bash
ssh big-linux "bash /home/gerry/RRMumble/verify-all-remote.sh"
```

Test spread: rr-proto 12→**14**, rr-session 19, rr-webrtc 9 + 2 loopback + 3 video,
rr-capture 14→**15**, rr-codec 17, rr-hwcodec 9.

The `cargo xwin` pass is the one that matters most: `dxgi_backend.rs` and its new
`monitor_origins()` exist only under `cfg(windows)` + `dxgi-capture`, so it is the
only place on big-linux they compile at all. `LOCK_CLEAN=1` is the proof that the
feature change pulled in no new packages — `xcap` was already an optional
dependency.

Scripts: `E:\devtools\verify-origins.ps1` and `verify-origins2.ps1` (local),
`E:\devtools\check-rr-input.ps1` (the temp-member trick for `rr-input`),
`E:\devtools\sync-origins.ps1` (tar the exact file set and list it), and
`big-linux:/home/gerry/RRMumble/verify-origins-remote.sh` (log at
`verify-origins.log`, summary at `verify-origins-run.txt`).

### ⚠️ Measured cost: `dxgi-capture` no longer builds Linux-*native*
Because `dxgi-capture` now enables `screen-capture`, `cargo clippy -p rr-capture
--features dxgi-capture` on Linux fails — `DXGI_ON_LINUX=101`, build script of
**`wayland-sys v0.31.11`**. Measured, not assumed; the remote script runs it as an
explicit informational step. Before this change that command succeeded, because
`windows-capture` was target-gated out and the module compiled to nothing.

This is the same class as the pre-existing `--features real-capture` failure on
this host (§10): a **missing system library**, not a code fault, for a combination
nothing builds — DXGI is Windows-only, and CI's dxgi pass plus all three
`verify-*.sh` scripts target Windows via `cargo xwin`. If it ever needs to be
clean, the only two routes are a *different* Windows-only geometry crate, or the
`unsafe` exception in `rr-capture` — one feature flag cannot be made
target-conditional in cargo.

### Fixed: two pre-existing clippy failures
Neither was caused by the origin work; both were found by running clippy over
crates the local box had not covered before.

**`rr-webrtc` — 5x `clippy::double_must_use`** at `src/signal.rs:52` and
`src/lib.rs:90`. Both spans point at an **`#[async_trait]` attribute line**, not at
anything written in the file: the macro desugars each `async fn` to one returning
`Pin<Box<dyn Future>>` and marks it `#[must_use]` to preserve `async fn`
semantics, but a boxed future is already `#[must_use]`, so clippy sees it twice.
There is nothing in the source to remove. Fixed with
`#[allow(clippy::double_must_use, reason = "emitted by the `async_trait`
expansion, not by this code")]` on the two trait declarations
(`SignalTransport`, `SessionObserver`) — 2 + 3 methods = the 5 errors.
Native `async fn` in traits would avoid the macro entirely but is **not
`dyn`-compatible**, and both traits are used behind `Arc<dyn ..>`, so
`async-trait` stays.
⚠️ Appears **only on clippy 1.99.0** (local), not big-linux's 0.1.98 — which is
why CI had been green. Confirmed pre-existing by stashing every change and
re-running at committed HEAD. **Expect more of these as CI's stable clippy
advances; local is now the leading indicator, not the lagging one.**

**`rr-input/src/gate.rs:261` — `clippy::branches_sharing_code`**: all five arms of
`inject`'s `match` ended `Ok(None)`. Moved it after the `match` and left every
refusal as an early `return`, which reads better than it lints: there is one way
to succeed and it is now written once.

The two repos have **divergent histories** (different hashes, same content).
`doc/remote-radio/P2-BASELINE.md` and `crates/rr-capture/src/dxgi_backend.rs`
verified byte-identical by `sha256`. Their `Cargo.lock`s had drifted
historically; both now carry **big-linux's** lock (verified locally with
`--locked`), where adding `windows-capture` was purely additive (+53 lines,
0 version changes — it also pulls `rayon`/`crossbeam`).

Verification at that head: Linux clippy on changed crates **0**, Windows-target
clippy with all backends via `cargo xwin` **0**, local Windows clippy on both
feature sets **0**, tests **0**. **85 tests**: rr-proto 12, rr-session 19,
rr-webrtc 9 + 2 loopback + 3 video, rr-capture 14, rr-codec 17, rr-hwcodec 9.

Verification scripts on big-linux: `verify-committed.sh` (the one to run),
plus `verify-hwcodec2.sh`, `verify-targeted.sh`, `lock-check.sh`, `commit-*.sh`.
Locally: `E:\devtools\verify-local.ps1`.

### P2 closeout (all done)
- Committed on both machines; big-linux HEAD was **`1a5bbb25`**, 0 dirty files,
  `CLIPPY_EXIT=0`, `TEST_EXIT=0`, **70 tests**.
- CI tests `rr-capture` + `rr-codec` and installs **nasm** on Linux (apt) and
  Windows (choco) in both the `test` and `build` jobs, since openh264 builds
  from source and uses it for assembly kernels.
- **Real-hardware validation:** `crates/rr-codec/examples/capture_probe.rs`
  (feature `real-capture`), built for Windows by
  `E:\devtools\probe-win-release.sh`, binary at
  `E:\AI\RRMumble\probe\capture_probe.exe`. **PASS** — both monitors enumerated
  (1920x1200 primary + V505-G9 3840x2160), real frame grabbed and validated
  non-blank, IDR + delta encoded, decoded at correct geometry.
- **Baseline in `ARMumble/doc/remote-radio/P2-BASELINE.md`:** capture avg
  **33.3 ms**, encode avg **16.6 ms**, combined **49.9 ms**, **19.9 fps**.
  **Key finding: capture was the bottleneck, not encode** — `xcap`'s
  `capture_image()` copies the whole framebuffer per call with no damage
  tracking, costing full price even on a static screen (3.5 KB deltas vs a
  142 KB IDR). That put us *over* the 33.3 ms 30-fps budget, which is why the
  DXGI backend was pulled forward — **now done, see §8b**.
- Probe also surfaced that `OpenH264` auto-disables `AdaptiveQuant` and
  `BackgroundDetection` in screen-content mode; both now set `false` explicitly.

> **Gotcha:** `cargo clippy -p rr-codec --features real-capture` **fails on
> big-linux** with `system library wayland-client ... not found`. That is a
> missing dev library on the headless host, **not a code problem** — the Station
> is Windows-only. Verify that path with
> `cargo xwin build --target x86_64-pc-windows-msvc` instead (passes).

### Next: finish P3 — input control + arbitration
Steps 1–4 are **DONE** (§12b): `crates/rr-input` is written, a workspace member,
and green on both machines — 23 tests, both feature sets clippy-clean, Windows
target via `cargo xwin`, no `unsafe`. `MonitorLayout::from_advertised` copies the
real origins and a test asserts the negative-origin `(-953, -2160)` case. CI
wired. Committed.

**The one remaining P3 step:**
5. Wire `ControlGate` over the live `rr-control` data channel in `rr-webrtc`,
   with auto-connect and auto-grant-when-free. `ControlGate::handle` already
   takes a `SessionId` + `ControlMsg` and returns an `Outcome` carrying the
   `Effect`s to broadcast — so this is plumbing: decode inbound `rr-control`
   frames to `ControlMsg`, call `handle`, and relay the `Outcome.effects` /
   `keyframe_requested` / `monitor_selected` back out. The Station builds the
   gate with `EnigoInjector` (feature `inject`); the Client never does.

Exit criterion: one controller drives the Station, handoff works, and the
arbitration edge cases already covered by `rr-session`'s 19 tests hold over a
real link. ⚠️ This now also needs the **Fancy server** standing up to relay
`WebRtcSignal` — which is deployed (§16): `fancy-mumble:1.6` on
`newyork.remote.radio:54736`, verified relaying stereo + announcing
`fancy_version`.

### Useful commands
```powershell
# Remote verify (clippy + all tests, prints head/dirty/exit codes)
ssh big-linux "bash /home/gerry/RRMumble/verify-committed.sh"

# Build library crates locally (sets up MSVC + nasm; echoes CARGO_EXIT=)
cmd /c "E:\devtools\cargo-msvc.cmd clippy -p rr-capture -p rr-codec --all-targets --features rr-capture/screen-capture,rr-capture/dxgi-capture,rr-codec/real-capture,rr-codec/dxgi-capture"

# Compare both capture backends end to end on this machine (writes probe-run.txt)
powershell -NoProfile -ExecutionPolicy Bypass -File E:\devtools\run-probe.ps1

# Attribute capture cost / measure DXGI work (writes bench-run.txt)
powershell -NoProfile -ExecutionPolicy Bypass -File E:\devtools\run-bench.ps1

# Smoke-test both desktop apps locally
powershell -File E:\devtools\smoke-test-both.ps1

# Build all three apps on big-linux
ssh big-linux "nohup bash /home/gerry/RRMumble/build-windows-final.sh >/dev/null 2>&1 &"
```

### Known limitations to carry forward
- **Nothing consumes `rr-capture` in the apps yet.** It is a dependency of
  `rr-codec` and a dev-dependency of `rr-webrtc` only. When the Station's send
  loop is written, it must select `DxgiCapturer` on Windows and enable the
  `dxgi-capture` feature; `XcapCapturer` stays the non-Windows fallback.
- `DxgiCapturer` returns an **empty** friendly name for the primary monitor here
  (`xcap` returned the placeholder `Unknown Monitor 65639`). Neither is useful —
  the Station UI needs its own fallback label.
- A delta frame over a near-static screen encoded to **0 bytes**. Correct, but
  the transport must treat an empty bitstream as "send nothing" rather than
  framing an empty packet.
- Only the **primary** monitor has been exercised end to end, on both backends.
- Linux **desktop** target is not set up (needs webkit2gtk etc. via apt; sudo
  was declined). Android + Windows are the supported targets.
- Windows installer is **unsigned**; APK is signed with a **dev** keystore.
- Updater is **disabled** in code until a real release feed exists.
- Pure P2P means the Station uploads **one encoded stream per viewer**
  (N viewers = N× uplink). Mitigated in P5 with per-peer ABR + a viewer cap.

---

## 11. P7b: `rr-hwcodec` — hardware H.264 (COMPLETE)

Commits: local **`38566026`**, big-linux **`df47169d`**.
Full write-up with all numbers: `doc/remote-radio/P2-BASELINE.md`.

### Result: 4K at 41.2 fps
Measured on this i7-1165G7 laptop, 3840x2160, DXGI capture, real desktop content:

| encoder | capture | encode | total/frame | fps | CPU clock |
|---|---|---|---|---|---|
| **Quick Sync** (`rr-hwcodec`) | 14.27 ms | **10.01 ms** | **24.27 ms** | **41.2** | **45% of nominal** |
| `OpenH264` software | 14.81 ms | 61.41 ms | 76.22 ms | 13.1 | 109–123% of nominal |

**6.1x faster encode, and it leaves the CPU almost idle** — the clock figure is
the part that matters for a remote-desktop host, because the Station also has to
run Mumble voice and whatever the user is actually doing.

Acceptance criteria all met: 4K ≥30 fps end to end; software fallback works
(`RR_FORCE_SOFTWARE=1` pins it); and the hardware is **asserted** to be in use
rather than assumed — see the silent-software-MFT trap below.

### Verified four ways
Local Windows clippy (both feature sets) + tests; Linux stub-path clippy +
tests; Windows target via `cargo xwin`; and the CI unsafe-confinement check.
**40 tests** across `rr-capture` (14), `rr-codec` (17), `rr-hwcodec` (9).

### What `rr-hwcodec` contains
- **Detection** — `available_h264_encoders()` / `has_hardware_h264()` via
  `MFTEnumEx`. On this machine: `[HARDWARE] Intel® Quick Sync Video H.264
  Encoder MFT` + `[software] H264 Encoder MFT`. **Drivers register the same
  encoder twice** — dedupe by name.
- **`HwEncoder`** — async MFT encode producing Annex-B with SPS/PPS on
  keyframes; `ICodecAPI` / `CODECAPI_AVEncVideoForceKeyFrame` for
  keyframe-on-demand.
- **`HwEncoderSettings`** — bitrate, fps, and **`prefer_bgra`** (default true;
  set false to force the NV12 path).
- **`bgra_to_nv12()`** — BT.601 studio swing, stride-aware, 2x2 chroma
  averaging. **No `unsafe` at all in `convert.rs`.** Kept as the fallback for
  MFTs that refuse BGRA.
- Examples: `list_encoders`, `encode_synthetic` (end-to-end correctness —
  asserts NAL types `[9,7,8,6,5]`), `bench_encode` (phase breakdown).

### Design decision: negotiate BGRA input *before* NV12
Measured CPU cost of `bgra_to_nv12`: 2.8 ms at 1920x1200, **11.4 ms at 4K**.
At 4K that alone would have put capture + convert + encode at ~39 ms — outside
the 33.3 ms budget even with fast silicon. Quick Sync accepts BGRA directly and
converts on the GPU, so **negotiating BGRA first means that 11.4 ms is never
paid**. `InputLayout` records which case we actually got, so if a GPU forces
NV12 the cost stays visible rather than silently appearing as "slow encode".

### Design decision: `encode()` must not block for its own output
The first working 4K build ran at **41.9 ms/frame**. Cause: `encode()` waited
for the encoder to hand back the frame it had just submitted. Hardware encoders
are pipelined — they want the next input while the previous one is still in
flight. Draining non-blockingly dropped it to **~10 ms**, a **4x** win from
nothing but call ordering. Only `pump_until_need_input` blocks, and only to
apply backpressure. Lesson: with a pipelined encoder, per-frame latency and
throughput are different numbers, and blocking collapses them into the worse one.

### Media Foundation gotchas — each one produced a *misleading* failure
1. **Set output type → negotiate input → set the output type again.** Input
   negotiation invalidates the output type. Skip the second set and Quick Sync
   raises `MF_E_TRANSFORM_STREAM_CHANGE` on the first `ProcessOutput`, which
   reads like a resolution problem and is not.
2. **On renegotiation, *overlay* `MF_MT_FRAME_SIZE` / `MF_MT_FRAME_RATE` /
   `MF_MT_AVG_BITRATE` onto the MFT's own preferred type.** Building a bare type
   with just those attributes → `E_UNEXPECTED` (0x8000FFFF), with no hint which
   attribute was missing.
3. **Draining is event-driven.** Polling `ProcessOutput` after
   `MFT_MESSAGE_COMMAND_DRAIN` is an out-of-turn call on an async MFT →
   `E_UNEXPECTED` **after all 60 frames encoded correctly**, so the failure
   looks like a flush bug when it is a protocol violation. Wait for
   `METransformDrainComplete`.
4. **Intel's MFT is asynchronous** → needs `MF_TRANSFORM_ASYNC_UNLOCK`, then the
   `METransformNeedInput` / `METransformHaveOutput` event loop. A synchronous
   `ProcessInput`/`ProcessOutput` implementation returns
   `MF_E_TRANSFORM_ASYNC_LOCKED` and would have worked only on machines exposing
   a sync MFT.

### ⚠️ The silent software MFT
`windows-capture`'s own `VideoEncoder` reported **51 fps at 4K** — and the GPU
media engine sat at **0%**. It had quietly selected the **software** H264 MFT.
Media Foundation substitutes software without erroring, so *throughput alone can
never prove hardware is in use*. Always cross-check the GPU engine counter (and
remember Intel files it under `engtype_videodecode`, §2). This is the same class
of trap as the NASM one in §8c: a silent fallback that only shows up as "the
numbers are a bit off".

### The unsafe exception (user-approved)
- **`rr-hwcodec` is the only `rr-*` crate permitted `unsafe`**, and the only
  crate anywhere taking it crate-wide
  (`#![allow(unsafe_code, reason = "...")]`). CI enforces this, **scoped to
  `crates/rr-*`**. All COM is confined inside; the public API is safe.
- ⚠️ **The workspace was never `unsafe`-free.** The CI guard immediately caught
  that claim: `mumble-tauri` and `mumble-protocol` carry **ten** pre-existing
  per-item `#[allow(unsafe_code)]` for platform FFI (WASAPI, `oboe`, `glib`,
  `WebView2` COM, byte reinterpretation, `signal-bridge`). Narrower than ours
  and out of scope, but do not claim otherwise.
- Rules kept: nothing `unsafe` in the public API; every `unsafe` block states
  its invariant; COM freed by RAII (`ActivateArray` releases each element *and*
  frees the `MFTEnumEx` allocation — otherwise one COM object leaks per
  enumeration).

### `windows-rs` 0.62 API corrections (the docs/examples online are older)
- `MFSetAttributeSize` / `MFSetAttributeRatio` **do not exist**. Pack manually:
  `((high as u64) << 32) | low as u64`, then `SetUINT64`.
- `VARIANT` + `ICodecAPI::SetValue` are gated behind feature
  **`Win32_System_Ole`**.
- `GetEvent` takes `MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0)` to block, `(1)`
  for `MF_EVENT_FLAG_NO_WAIT`.
- `GetType()` returns `MF_EVENT_TYPE`; compare via
  `METransformNeedInput.0.unsigned_abs()`.
- `MFTEnumEx` takes the `MFT_ENUM_FLAG` newtype, not a `u32`.
- Required features: `Win32_Foundation`, `Win32_Media_MediaFoundation`,
  `Win32_System_Com`, `Win32_Graphics_Direct3D11`, `Win32_Graphics_Dxgi_Common`,
  `Win32_System_Variant`, `Win32_System_Ole`.

### `rr-codec::VideoEncoder` — the runtime selection layer
Picks hardware when available, falls back to `OpenH264` otherwise, and exposes
`EncodeStats` so the choice and its cost are observable rather than inferred.
**`RR_FORCE_SOFTWARE=1`** pins software — this is how the comparison table above
was produced on one machine in one run.

### The next large optimisation: zero-copy
`bench_encode` at 4K splits encode into: **build the input sample (a 33 MiB
CPU-side staging copy) 5.62 ms**, submit 2.91 ms, drain 3.4 µs — so **staging is
66% of encode time**. Feeding the DXGI texture straight to the MFT via a D3D
device manager would remove that copy **and** the 14 ms capture readback
together, since they are the same copy in two directions. That is the only
remaining change with multiple-milliseconds upside; everything else is noise by
comparison. Deferred — 41.2 fps clears the bar, and P3 is worth more.

---

## 12. P3: `crates/rr-input` — COMPLETE (crate), live wiring remaining

> ✅ **DONE (this is now history).** `rr-input` is a workspace member, both
> missing modules are written, and it is green on both machines:
> `CLIPPY_DEFAULT=0 CLIPPY_INJECT=0 TEST=0 UNSAFE_GUARD=0`, **23 tests**, plus the
> `cargo xwin` Windows-target pass. `Cargo.lock` gained `enigo` 0.6.1 and
> `xkbcommon` (the `inject` feature's Linux dep). Details below in §12b; the only
> P3 work left is wiring `ControlGate` over the live `rr-control` channel
> (step 5).

### Files written
| file | contents |
|---|---|
| `Cargo.toml` | deps `rr-proto`, `rr-session`, `thiserror`, `tracing`; optional `enigo = "0.6.1"`; feature `inject = ["enigo"]` |
| `src/lib.rs` | `InputInjector` trait (`mouse_move_abs`, `mouse_button`, `wheel`, `key`, `text`, `release_all`), `InputError`, `#![forbid(unsafe_code)]` |
| `src/layout.rs` | `MonitorPlacement`, `MonitorLayout::{new, single, get, to_absolute, from_advertised}` |
| `src/keymap.rs` | `KeyCode` (81 keys), `ALL`, `as_u32`, `from_u32`, `dom_code`, `from_dom_code`, `is_modifier` |
| `src/gate.rs` | `ControlGate::{new, handle, tick, disconnect, set_layout, set_shared_monitor}`, `Outcome`, `Refusal`, `RefusalCounts` |
| `src/recorder.rs` | `RecordingInjector`, `RecordedAction` — lets gating and coordinate mapping be tested headless |

### §12b. Both modules now written (the two that used to be missing)

> ✅ Both are written and green on both machines: 23 tests, no `unsafe`, both
> feature sets clippy-clean, `cargo xwin` Windows-target pass.
>
> ⚠️ `enigo` 0.6.1 cross-platform key gotcha, cost one Linux build failure:
> `Key::LWin`/`Key::RWin` are Windows-only — they compiled locally and failed on
> big-linux. The portable key is `Key::Meta` (enigo''s "windows/super/command"),
> so MetaLeft/MetaRight both map to it and lose the side, same as AltLeft/AltRight
> -> `Key::Alt`. The local Windows build does NOT prove the `inject` feature
> compiles; enigo''s `Key` is heavily cfg(target_os)-gated. A CI clippy step with
> `--features inject` now guards this. The injector uses discrete Press/Release,
> never Click (the wire protocol carries down/up separately).

Original note, for the record:
- **`src/enigo_backend.rs`** — the real injector behind feature `inject`. I was
  mid-`fs_write` when interrupted. Intended `enigo` 0.6.1 API:
  `Enigo::new(&Settings::default())`; traits `Mouse` / `Keyboard` must be in
  scope; `move_mouse(x, y, Coordinate::Abs)`;
  `button(Button::Left, Direction::Press)`; `scroll(len, Axis::Vertical)`;
  `key(Key::Unicode('a'), Direction::Click)`; `text("…")`.
- **`src/tests.rs`** — declared by `#[cfg(test)] mod tests;`.

### Design decision: `ControlGate` is structural, not a convention
"Remember to check `may_control` before injecting" is a bug waiting to happen.
Instead the injector is **private to the gate**, and there is **no path to
inject without presenting a `SessionId`**. The gate re-checks
`Arbiter::may_control` on **every** event, using the same arbiter instance that
decides handoff, so a stale permission cannot exist. A misbehaving peer that
spams input after losing control is refused by construction, not by diligence.

### Design decision: refuse rather than guess
Refuse (and count, via `RefusalCounts`, so a flood is visible rather than
silent):
- input from a non-controller,
- input aimed at a monitor that is not being shared,
- an unmapped key code.

**Clamp, don't refuse, for out-of-range coordinates.** A pointer at the very
edge of the screen is normal; dropping those events makes the remote pointer
stick at the border, which users read as "the connection broke". Clamping is the
behaviour they expect.

### Design decision: own `KeyCode` enum with stable `u32` discriminants
Rejected **Windows VK codes** — they would make the wire format Windows-only,
and the Android app is already in the picture. Rejected **raw DOM `code`
strings** — an allocation per keystroke, and a typo fails to match *silently*.
A closed enum with stable numbers keeps the wire format platform-neutral and
makes an unknown code an explicit refusal.

### ✅ RESOLVED: monitor origins (uncommitted, verified on hardware)
`rr_proto::MonitorInfo` and `rr_capture::MonitorInfo` now both carry
**`x: i32, y: i32`** — the monitor's top-left on the virtual desktop, in physical
pixels. `MonitorLayout::from_advertised` copies them straight through; nothing
infers an arrangement any more. **`PROTOCOL_VERSION` bumped 1 → 2.**

**The origins are required, not `#[serde(default)]`.** A defaulted origin
deserialises to `(0, 0)`, stacking every display on the primary — plausible
looking, and it misroutes every click on a secondary monitor. A decode failure is
loud; the version bump means a v1 peer is refused by `check_version` first, which
is actionable, rather than by a JSON error, which is not. `rr-proto` has a test
that fails if anyone adds `#[serde(default)]` to these fields.

**Where each backend gets them:**
- `XcapCapturer` — `xcap::Monitor::x()` / `y()`, native on every platform.
- `SyntheticCapturer` — `(0, 0)`, pinned by a test, because downstream
  coordinate-mapping tests rely on relative == absolute for it.
- `DxgiCapturer` — **`windows-capture`'s `Monitor` exposes no position**, and
  querying Windows directly would mean `unsafe` in `rr-capture`, which CI forbids
  (§11). So `dxgi_backend::monitor_origins()` uses `xcap` *for geometry only*, and
  the **`dxgi-capture` feature now depends on `screen-capture`**.

Two details that make that sound rather than a bodge:
1. **One coordinate space, not two that look alike.** `windows-capture` reads
   `width`/`height` from `EnumDisplaySettingsW`'s `DEVMODEW`; `xcap` reads `x`/`y`
   from `dmPosition` in that same structure. `dmPosition` is also the
   DPI-independent choice — `GetMonitorInfoW`'s `rcMonitor` is scaled unless the
   process is per-monitor DPI aware.
2. **The join key is exact.** `xcap::Monitor::name()` returns the device name
   (`\\.\DISPLAY2`); `display_number()` parses it exactly as
   `windows_capture::Monitor::index()` does, so the number cannot come to mean a
   different display than the id `enumerate()` hands out. A monitor with no origin
   **fails the enumeration** rather than being placed by guesswork.

⚠️ **`cargo` rejects aliasing one package twice** ("depends on crate `xcap v0.9.8`
multiple times with different names"), so the tidy
`xcap-origins = { package = "xcap" }` trick for keeping xcap Windows-only is *not
available*. Hence the feature dependency. Cost: `--features dxgi-capture` now
pulls `xcap`, which a *Linux-native* build would need X11/Wayland libs for —
nothing does that. Checked: CI's dxgi clippy pass is Windows-only, and
`verify-dxgi.sh`, `verify-targeted.sh` and `verify-hwcodec2.sh` on big-linux all
already pass `screen-capture` alongside it and target Windows via `cargo xwin`.

**Hardware verification (this laptop, two displays).** Both backends were run
through `capture_probe`, which now prints origins:

| backend | primary 1920x1200 | V505-G9 3840x2160 |
|---|---|---|
| DXGI (ids 1, 2) | `(0, 0)` | **`(-953, -2160)`** |
| `xcap` (ids 65639, 131073) | `(0, 0)` | **`(-953, -2160)`** |

Two backends with entirely different id schemes agree exactly, which is what
proves the DXGI join resolves to the right display — a wrong join would have given
monitor 1 monitor 2's origin. And the real arrangement is **negative on both
axes**: the 4K panel sits above and to the left. The old left-to-right guess would
have placed it at `(1920, 0)` — out by 2873 px across and 2160 px down, i.e. every
click on that monitor landing on the wrong display or on nothing. The `-953` is
not a round number either, so no heuristic would have found it.

Capture/encode timings unchanged (DXGI capture 5.4 ms at 49% background load,
`xcap` 31.5 ms, both in line with §8b): `monitors()` is a setup-time call, so the
extra enumeration is not in the per-frame path.

### Pre-existing pieces this builds on (already written and tested)
- **`rr-proto`**: `InputEvent::{MouseMove{monitor,x,y}, MouseButton{button,down},
  Wheel{dx,dy}, Key{code,down}, Text{text}}`;
  `ControlMsg::{Input, RequestControl, ApproveHandoff{to}, DenyHandoff{to},
  ReleaseControl, ControlState{controller,pending}, SelectMonitor{monitor},
  RequestKeyframe}`; `MouseButton`; `Presence`; `PROTOCOL_VERSION: u16 = 2`
  (was 1 before the monitor-origin change, §12).
- **`rr-session`** (19 tests): `SessionId = u32`, `Holder::{Free, Held}`,
  `ControlState`, `Effect::{Broadcast, NotifyControlRequested,
  NotifyRequestRejected}`, `RejectReason::{Denied, TimedOut}`,
  `Input::{Request, Approve, Deny, Release, Disconnect, Tick}`,
  `Arbiter::{new, holder, controller, pending, may_control, state, apply,
  request_at}`.

---

## 13. BACKBURNER: macOS target (investigated, not started)

> **Decision: deferred.** Investigated only — no code written, no config changed.
> **Linux desktop is explicitly never a target** (keep it as a *CI platform*
> though: it is the cheapest runner for the OS-independent crates).

### Verdict: not a huge change. The cost is Apple's packaging and permission
### model, not Rust.

The fork is far more portable than its "Windows + Android" target list implies.
The dominant `cfg` axis across the whole codebase is
**`not(target_os = "android")` meaning "desktop"**, which macOS satisfies for
free.

### Already present (the pleasant surprises)
- **Audio has no Windows-specific code at all.** `mumble-tauri/src/audio/` is two
  traits (`AudioDeviceFactory`, `MixingPlayback`) over **`rodio`** (default, chosen
  at runtime by `USE_RODIO_BACKEND`) and **`cpal`** (legacy fallback). The only
  `cfg` is android vs desktop. Both crates ship CoreAudio backends, and
  `Cargo.lock` already resolves `coreaudio-rs` next to `alsa`. **There is no
  hand-written WASAPI layer to port.**
- `alsa-sys` — the thing that breaks `cargo clippy --workspace` on big-linux — is
  just `cpal`'s Linux arm resolved into the lock. It never *builds* off Linux.
  Nothing to remove.
- **A macOS window backend already exists**: `platform/window/macos.rs`
  (`NSWindow` `contentAspectRatio` + `sharingType` via `objc2` `msg_send!`, with a
  file-level `allow(unsafe_code)`), and `mumble-tauri/Cargo.toml` already carries a
  `cfg(target_os = "macos")` dependency table (`objc2`, `objc2-foundation`).
- `build.rs` already knows `libsignal_bridge.dylib`, and the pchat loader has a
  `cfg!(target_os = "macos")` arm.
- The dock-badge code already has a macOS arm; `system_uses_24h()` already falls
  back correctly off Windows.
- CI already contains **unused `runner.os == 'macOS'` branches** — the workflow was
  pre-wired for a runner that was never added to either matrix.
- **The `rr-*` crates were designed for this.** `rr-codec::VideoEncoder` is already
  an enum with a `#[cfg(windows)] Hardware(..)` arm falling back to `Software(..)`;
  `rr-hwcodec` is already a dependency-free stub off Windows; `rr-capture` is a
  trait with swappable backends; `rr-input`'s `keymap.rs` docs state outright that
  Windows VK codes were rejected so "a Linux or macOS Station later" would not have
  to break the wire format. **The monitor-origin work (§12) is already
  macOS-correct** — `xcap` reports origins natively there, so `XcapCapturer` needs
  no new code.

A macOS build would compile and run fairly early. It would just be slow and badly
packaged.

### The lever: Client is cheap, Station is not
| | macOS **Client** (viewer) | macOS **Station** (host) |
|---|---|---|
| screen capture | not needed | `ScreenCaptureKit` backend |
| hardware encode | not needed | VideoToolbox crate |
| input injection | not needed | `enigo` + Accessibility permission |
| TCC permissions | microphone only | microphone + Screen Recording + Accessibility |

The crates already encode this split: `rr-capture`'s backends and `rr-input`'s
injector are optional features *because the Client never captures or injects*.
**Do not let the Station's capture/encode work block shipping a Client.**

### Two genuinely new engineering pieces (Station only)
1. **`ScreenCaptureKit` capture backend.** `xcap`'s macOS path uses
   **`CGWindowListCreateImage`** (verified by reading `xcap-0.9.8/src/macos/
   capture.rs:15`) — a full-surface copy, the same class of API as the GDI baseline
   that measured 33 ms/frame on Windows. Apple deprecated it in macOS 14 in favour
   of `ScreenCaptureKit`. So expect to repeat the P7a exercise behind the existing
   `ScreenCapturer` trait. ⚠️ Unlike DXGI's pull model, **SCK is push-based**
   (delegate callbacks), so it needs a push-to-pull bridge — but `DxgiCapturer`'s
   worker-thread + job-queue pattern is the template.
   *Not measured on real hardware; the slowness is inferred from the API shape.*
2. **`rr-vtcodec` for VideoToolbox**, mirroring `rr-hwcodec`, plus one more `cfg`
   arm in `rr-codec/src/select.rs`. Shipping without it is legitimate: you get
   software `OpenH264`, which per §8b is fine at 1080p and 2–4x short at 4K.

### Non-code costs (the actual bulk of the work)
- ⚠️ **big-linux cannot build macOS.** This is the one structural change to the
  workflow. Apple's SDK licence effectively requires Apple hardware, and osxcross
  is a legal and practical mess. Needs a Mac or GitHub Actions `macos-latest`.
  The author-locally → `tar` → `scp` → build-remotely loop stays valid for Windows
  and Android and simply does not extend to macOS.
- ⚠️ **Accessibility permission is the nastiest UX item.** Synthetic input
  injection requires the user to enable the app manually under Privacy & Security →
  Accessibility. It cannot be granted programmatically, needs a signed bundle to
  persist, and can reset on update. Unavoidable first-run friction for a
  remote-control host. Screen Recording is a second, separate prompt.
- **Signing + notarization become mandatory**, not optional — far more hostile than
  the unsigned Windows installer currently shipped.
- Bundle config absent in all three `tauri.conf.json`: no `bundle.macOS` block,
  no `minimumSystemVersion`, no entitlements, no `.icns`, no `CFBundleURLTypes`
  for the `fancy://` scheme, and **no `NSMicrophoneUsageDescription`** (hard
  requirement on macOS 10.14+ for the app's core function).
- signal-bridge dylib must move into `Contents/Frameworks` with a correct
  `install_name`/`@rpath` and its own signature.
- Updater: release JSON emits only a `windows-x86_64` key; needs `darwin-aarch64`.
- **Frontend item:** all three configs set `decorations: false` for the custom
  titlebar. On macOS that removes the traffic lights — needs
  `titleBarStyle: "Overlay"` or `macOSPrivateApi`, plus a titlebar inset in the
  React UI. This is the largest non-packaging work item.
- `platform/mod.rs:103-119` — macOS falls to `NoPlatform`, so no `pre_init`,
  `setup`, `teardown` or single-instance. Mostly *fine*: LaunchServices gives
  `.app` single-instance and Apple-Event URL delivery for free, so the Windows
  loopback-TCP deep-link shim (§5, `DEEP_LINK_PORT_BASE`) needs **no** macOS
  equivalent — provided the Info.plist scheme is declared.
- Review the two `unsafe impl Send` blocks in `audio/desktop.rs:44-48` and `:418`.
  Their stated reason is *"WASAPI COM objects are MTA-safe"*, which would be
  **factually wrong on CoreAudio**. They compile and probably work; the default
  rodio backend sidesteps them entirely.

### ⚠️ Pre-existing bug found during this investigation (independent of macOS)
`crates/app-client/tauri.conf.json` and `crates/app-station/tauri.conf.json` both
list `icons/32x32.png`, `icons/128x128.png`, `icons/icon.icns`, `icons/icon.ico` —
but **`crates/app-client/icons/` and `crates/app-station/icons/` do not exist in
the repo, and there is no `.icns` anywhere.** Those bundle configs would fail icon
resolution on any bundle build today. Note §5 claims the icon sets were generated
with `cargo tauri icon` *on big-linux* — so they may exist there and never have
been synced back. **Check before the next release build.**

### ⚠️ Warning that follows from this project's own history
On Apple Silicon, `OpenH264`'s x86 assembly path is irrelevant — it uses its own
arm64 kernels compiled through the C compiler, and **`nasm` is not involved at
all**. So the CI guard from §8c protects x86 only and would give **false
confidence** on `aarch64-apple-darwin`. Given that this exact class of silent
fallback already cost an hour on nasm (§8c) and nearly fooled us on the software
MFT (§11), verify the arm64 kernels are really built before trusting any macOS
encode timing.

**Cheapest first experiment, needs nothing but a Mac and one command:** does
`openh264-sys2` with `features = ["source"]` build at all for
`aarch64-apple-darwin`? Unverified.

---

## 14. Amateur-radio audio & rig PTT (APPROVED — work to do, not started)

> **Investigated and agreed with the user. No code written yet.**
> This section supersedes any earlier assumption that the Mumble voice path is
> carrying human speech.

### 14a. What this application is actually for

**RemoteRadio remotes amateur radio transceivers.** The Mumble voice path is
carrying **radio audio**, not conversation. That single fact changes the right
answer to several audio questions:

| | direction | content | wants |
|---|---|---|---|
| Station → Client | rig receive audio | **stereo** (main + sub RX) | continuous, unprocessed |
| Client → Station | operator microphone | mono | PTT or VOX, keys the rig |

The user already turns **echo cancellation** and **noise suppression** off in the
stock Mumble client and wants this app to behave that way by default.

⚠️ **Capture and playback channel counts are independent, and differ per app.**
This is a design requirement, not a detail — there is no single global "stereo
mode":

| | capture | playback |
|---|---|---|
| **Station** | **stereo** (rig RX L/R) | mono (operator voice → rig mic input) |
| **Client** | mono (operator mic) | **stereo** (rig RX → headphones) |

Because the two binaries have different Tauri identifiers
(`com.remoteradio.station` / `com.remoteradio.client`), `AudioSettings` already
persist to separate app-data directories — so these opposite configurations need
no sharing, negotiation or per-role branching in the settings layer.

### 14b. Stereo audio

#### ⚠️ Correct the premise first: **Mumble 1.5 does not support stereo voice**
The user believed 1.5 added it. It did not. Evidence:
- Upstream issue **#2829** (stereo) has been open for years; **#4193** (2020) was
  filed by a contributor offering to implement it *because* "little progress has
  been made". Still open.
- No 1.5.x changelog from the 1.5.517 RC through 1.5.915 mentions stereo. They
  mention positional-audio work, including a fix for "crash when trying to use
  positional audio on a mono output device".
- **Decisive, in this repo:** `proto/MumbleUDP.proto` *is* the 1.5 protobuf audio
  format, and `Audio` is `target`/`context`, `sender_session`, `frame_number`,
  `opus_data`, `positional_data`, `volume_adjustment`, `is_terminator`. **No
  channel count.** Nor in `Version`, `Authenticate`, `CodecVersion`, `ServerSync`.

Mumble's model is: encode mono, upmix on receive for positional panning. So "no
client supports stereo" is right, but because the protocol never specified it —
not because clients lag.

#### Why it is still achievable, with no protocol or server change
**Opus packets are self-describing** — channel count lives in the TOC byte — and
Mumble relays `opus_data` as an opaque blob. So:
- A stereo Opus packet is a valid Opus packet; **1.4 and 1.5 servers should both
  relay it untouched.**
- **Mono fallback is free, not a feature we build:** a receiving client with a
  mono decoder has libopus downmix it. They hear mono.
- A **stereo decoder on our side handles mono senders** (libopus upmixes), so
  inbound can simply always be stereo and both kinds of sender work.

⚠️ **The server version is NOT the fallback trigger.** `supports_protobuf_audio()`
(`state.rs:229-236`, `V1_5_0 = (1<<48)|(5<<32)`) only picks legacy-varint vs
protobuf **framing**; `opus_data` is opaque in both, so channels are orthogonal.
The real constraint is **bandwidth** (`ServerSync.max_bandwidth`, and
`max_bandwidth_per_user` in the UDP ping).

#### ✅ MEASURED: dual-receiver audio needs ≥96 kbps — there is a cliff
Main and sub RX (IC-7610, IC-9700, FTdx101 put them on L/R) are **uncorrelated**
channels, and Opus stereo couples channels on the assumption they are related.
This was measured rather than guessed, with two hard-panned uncorrelated tones
through the real `OpusEncoder`/`OpusDecoder` — see
`crates/mumble-protocol/tests/opus_channels.rs`, test
`separation_table_across_bitrate_and_channel_forcing`:

| bitrate | left sep | right sep | verdict |
|---|---|---|---|
| 48 kbps | 5.4 dB | 5.6 dB | unusable |
| **72 kbps** (current default) | **13.0 dB** | **13.3 dB** | audible crosstalk |
| **96 kbps** | **52.6 dB** | **61.6 dB** | clean |
| 128 kbps | 54.0 dB | 66.0 dB | clean |
| 160 kbps | 57.9 dB | 66.6 dB | clean |

**There is a ~40 dB cliff between 72 and 96 kbps** — Opus abandoning
coupled/intensity stereo for real dual-channel coding once it has the bits. So
this is not a "more is better" dial: below the cliff a sub-receiver audibly bleeds
into the main one, above it separation is effectively perfect and extra bits buy
almost nothing. **96 kbps is the floor; ship above it for headroom.**

⚠️ **Those are pure tones — the easiest case.** Real receiver audio is broadband
and noise-like, which couples harder. Validate against a real signal before
fixing the shipping default. `state/audio.rs` already has a bandwidth-negotiation
ladder that clamps bitrate against the server cap and steps frame size — reuse it
so stereo degrades predictably instead of failing.

#### ✅ RESOLVED: `opus 0.3.1` exposes the controls we need
No unsafe, no upstream PR. Both are now plumbed into `OpusEncoderConfig` with
defaults equal to Opus's own, so **mono behaviour is byte-for-byte unchanged**:
- **`set_force_channels(Option<Channels>)`** → new `ForceChannels { Auto, Mono,
  Stereo }` enum (mirrors the existing `OpusApplication` wrapper pattern).
  `Auto` = `OPUS_AUTO` = default.
- **`set_phase_inversion_disabled(bool)`** → new `phase_inversion_disabled` field,
  default `false`. Worth knowing about: Opus uses phase inversion when coupling
  stereo, which can partially **cancel on downmix to mono** — and since Mumble has
  no stereo support, *every stock client hears a mono downmix of whatever we
  send*, so that is the normal case for everyone else, not a corner case.

Also available and possibly relevant later: `set_signal` (voice/music hint),
`set_bandwidth` / `set_max_bandwidth`, `set_expert_frame_duration`,
`set_prediction_disabled`.

⚠️ **Neither control showed any measurable effect in the table** — identical
figures across all four combinations at every bitrate, and an identical mono
downmix level (rms 0.2501) either way. That is **expected rather than
reassuring**: hard-panned uncorrelated tones never tempt Opus to collapse to mono
and have almost no coupling to invert, so the signal does not exercise either
control. Both are retained on documented Opus behaviour, **not** on evidence from
here. The missing test is *transitional*: a signal that starts correlated
(squelched sub-RX) and then diverges, measuring how promptly the divergence
appears. Until it exists, treat `ForceChannels::Stereo` as a reasoned precaution.

#### Already stereo-ready (no work)
- `AudioFormat.channels: u16` is a real field, not a constant.
- `OpusEncoder::new` / `OpusDecoder::new` already map 1→`Mono`, 2→`Stereo` and
  error above 2; decoder buffer sizing already scales by channels.
- `sample_count()`, `decode_lost()`, `frame_seq_units()`, `insert_silence()` all
  already honour channels.
- `AudioFrame` documents **interleaved L R L R** as the project-wide convention;
  nothing is planar anywhere.
- Opus `application` already defaults to **`Audio`** (CELT/music) on desktop
  rather than `Voip` — correct for radio audio. `dtx: false` — correct.

#### Hard mono, must change
| what | where |
|---|---|
| capture constructors hardcode `AudioFormat::MONO_48KHZ_F32`, no override | `rodio_desktop.rs:59`, `desktop.rs:86` |
| unconditional downmix (sums/averages interleaved channels) | `rodio_desktop.rs` `capture_thread` ~L190-213, `desktop.rs` `handle_cpal_input` L98-117 |
| `read_frame()` drains `frame_size`, not `frame_size * channels` | `rodio_desktop.rs:100-112`, `desktop.rs:131-160` |
| `MumbleMixerSource::channels()` returns `MONO_CHANNELS` | `rodio_desktop.rs:497` |
| one mono sample fanned to every output channel | `desktop.rs` `write_output_frame` L388-414 |
| `batch_drain_speakers`' parameter literally named `mono_needed` | `desktop.rs:251-285` |
| `SpeakerBuffers` is a flat `VecDeque<f32>` with **no channel metadata** | `mixer.rs:41` |

⚠️ **The subtle, audible one:** the mixer's fade constants are *sample*-relative,
not *frame*-relative — `FADE_LEN = 240` ("5 ms"), `CROSSFADE_LEN = 24`,
`MAX_SPEAKER_BUFFER_SAMPLES = 19_200` ("400 ms mono"). Fed interleaved stereo they
cover **half** the intended wall-clock time **and apply unequal gain to L vs R**.
That is a nasty artefact to diagnose from the symptom. Convert to frame-relative.

#### The denoiser problem is cancelled by 14c
nnnoiseless / DeepFilterNet / OMLSA are mono-only 48 kHz algorithms in
`FilterChain`, and the trait hands them a flat `&mut [f32]` with no channel
awareness. That was the **one place the mono assumption is algorithmic rather than
mechanical**, and the only real design fork (per-channel inference at 2x
DeepFilterNet cost, vs an awkward "stereo disables denoising" compromise).
**Since radio use runs with the filter chain empty, that fork disappears.**
Everything remaining is arithmetic, and unit-testable with no radio attached.

#### ✅ DONE: the codec assumptions are proven
`crates/mumble-protocol/tests/opus_channels.rs` — **8 tests, all passing**,
clippy clean. Run with:
```bash
cargo test -p mumble-protocol --features opus-codec --test opus_channels -- --nocapture
```
The two load-bearing ones:
- **`stereo_packet_decoded_by_a_mono_decoder_is_a_real_downmix`** — a mono decoder
  handles a stereo packet, and **both** source tones survive, so `libopus` really
  downmixes rather than dropping a channel. **This is the free mono fallback**:
  every stock Mumble client keeps working, with no negotiation and no protocol
  change.
- **`mono_packet_decoded_by_a_stereo_decoder_duplicates_the_channel`** — a stereo
  decoder handles mono packets with L and R identical to within 1e-5. So the
  **inbound path can be unconditionally stereo**, which matters because
  `AudioMixer`'s per-speaker buffers carry no format metadata at all.

Plus: `stereo_round_trip_keeps_the_channels_apart`, the separation table,
`the_mono_default_bitrate_is_not_enough_for_two_receivers`,
`forcing_stereo_is_safe_when_both_channels_are_identical`,
`phase_inversion_setting_keeps_the_mono_downmix_intact`, and
`frame_duration_is_per_channel_but_byte_length_is_not` (which pins the
sample-vs-frame invariant the mixer constants must be converted to respect).

#### ⚠️⚠️ THE METHODOLOGY TRAP — cost one wrong conclusion, do not repeat
**Opus is a predictive codec: encoder and decoder both carry state across
packets.** The first version of these tests measured a single frame obtained by
feeding the *same packet* to the decoder repeatedly. Every decode after the first
advances the decoder's state as if time had passed, so it reconstructs from
predictions that no longer match — the measurement was meaningless.

It produced this table, which was duly believed for about a minute:

| bitrate | left | right |
|---|---|---|
| 72 kbps | 5.0 dB | 12.1 dB |
| 96 kbps | 27.1 dB | 35.3 dB |
| **128 kbps** | **18.9 dB** | **14.8 dB** |
| **160 kbps** | **14.6 dB** | **12.6 dB** |

**More bits cannot reduce channel separation.** The non-monotonicity — plus
`force_channels` and `phase_inversion` showing *byte-identical* results across
every combination — is what exposed the harness. The fix: encode a run of
phase-continuous frames, decode that run **in order**, and analyse only the tail
once both sides are past warm-up (now 10 warm-up + 10 measured = a 200 ms window,
5 Hz bins).

`separation_table_across_bitrate_and_channel_forcing` now **asserts monotonicity
with a 3 dB tolerance**, so this specific mistake cannot recur silently. Same
family as the NASM trap (§8c) and the silent software MFT (§11): *physically
implausible results mean the instrument is wrong, not the system under test.*

#### Still to verify: server relay
Against `crates/mumble-protocol/docker-compose.test.yml` (exists; already sets
`bandwidth: 320000`, so there is headroom). Pin a **1.4** and a **1.5** image and
prove each relays a stereo packet untouched. Believed true — the server should
only read the Opus TOC for duration/bandwidth accounting, which is
channel-independent — but **prove it, do not trust it.**

🎁 **The user has offered a live Mumble 1.5 server for testing.** Take it up for
exactly the things a container cannot show: real dual-receiver audio through a
real server, the separation figures on broadband signals rather than tones, what
a *stock* Mumble client actually hears when we transmit stereo into a shared
channel, and whether the server's bandwidth accounting objects to 96 kbps+. Ask
for host/port/credentials when starting §14g step 3.

### 14c. Transmit mode: decouple VAD from noise suppression ✅ DONE

> **Implemented and committed both machines.** `TransmitMode { Continuous,
> VoiceActivity, PushToTalk }` added to `mumble-tauri/src/state/types.rs`
> (`AudioSettings.transmit_mode`, `#[serde(default)]` = `VoiceActivity`), with a
> `transmit_mode()` accessor that reconciles the legacy `push_to_talk` boolean so
> old saved configs still behave. `build_outbound_filters` in
> `state/audio.rs` now inserts the `NoiseGate` **only** in `VoiceActivity` mode,
> independent of `noise_suppression` (the denoiser stays on its own toggle).
> `needs_pipeline_restart()` reacts to a mode change. 6 behaviour tests.
> mumble-tauri is a desktop/Android crate and does **not** build on big-linux (no
> ALSA, not in `verify-all-remote.sh`), so this was verified on local Windows:
> `cargo check`/`clippy` clean (only 3 pre-existing `doc_markdown` warnings in
> `updater/mod.rs`, untouched), 240 lib tests + the 6 new ones pass. Still to do:
> a UI control to surface the mode (the backend is complete).

**Original design notes (kept for reference):**

**Current behaviour, confirmed:**
- **PTT is NOT in the audio pipeline.** `push_to_talk_start()` / `_end()`
  (`state/audio.rs:580`, `:619`) toggle **self-mute at the Mumble protocol level**
  (`SetSelfMute`). `needs_pipeline_restart()` explicitly excludes
  `push_to_talk_key`. **So PTT and the transmit UI keep working with every filter
  off.**
- **VOX/VAD *is* the `NoiseGate`**, which sets `frame.is_silent`;
  `OutboundPipeline::tick()` branches on it to emit `Audio` / `Terminator` /
  `Silence`. The pipeline's own test comment: *"Without a noise gate the
  `is_silent` flag stays false, so this should yield Audio."*
- ⚠️ `NoiseGate::new(..)` is pushed **inside** `if settings.noise_suppression`
  (`state/audio.rs` `build_outbound_filters`). Nothing is pushed
  unconditionally, so `auto_gain = false` + `noise_suppression = false` yields a
  **literally empty `FilterChain`** — the user's existing configuration — and that
  empty chain is *where continuous transmit comes from*.

**Work:** introduce an explicit transmit mode — **`Continuous` / `VoiceActivity` /
`PushToTalk`** — and build the gate from the mode, **independent of the denoiser
toggle**. All inputs already exist (`vad_threshold`, `noise_gate_close_ratio`,
`hold_frames`, `push_to_talk`, `push_to_talk_key`); it is mostly moving the
`NoiseGate` push out of the `noise_suppression` block, plus a settings field and a
UI control. `noise_suppression` currently defaults to `true` (`types.rs:1118`) —
a radio profile flips it.

Worth doing on its own merits: it fixes a wart for ordinary voice users (today,
wanting VOX forces a denoiser into the mic path) and makes "the Station transmits
continuously" an explicit choice rather than a side effect of unticking a box.

**Consequence to make visible, not hide:** a continuously transmitting Station is
permanently "talking" in the channel — constant upstream bandwidth and a
permanently lit talking indicator in everyone's user list. It should look
intentional rather than stuck.

### 14d. Rig PTT on the Station (new capability)

**Approach: RS-232 control-line change driving a transistor/optocoupler switch
that keys the rig's PTT line directly. Deliberately *not* CAT** — CAT command sets
are radio-specific, whereas a PTT contact closure is universal (this is the same
mechanism hamlib calls `PTT_TYPE_DTR` / `PTT_TYPE_RTS`, and what SignaLink,
RigBlaster and homebrew interfaces have always done).

**Two ways control is asserted, both wanted:**

| mode | trigger | latency | works with |
|---|---|---|---|
| **A. Explicit** | Client UI button, hotkey, or **RS-232 input pin change** (footswitch) | lowest | RemoteRadioClient only |
| **B. Audio-detected** | the Station's **noise gate** on the inbound Mumble stream | higher, needs lead-in | **any Mumble client**, incl. a phone |

Mode B is worth building for its own sake: it means the rig can be operated from a
stock Mumble client with no RemoteRadio software at all.

**Serial plumbing** (`serialport` 4.x — cross-platform, safe, keeps
`#![forbid(unsafe_code)]`):
- **Output / Station:** `write_data_terminal_ready(bool)` (DTR) or
  `write_request_to_send(bool)` (RTS). These are the writable lines.
- **Input / Client footswitch:** `read_clear_to_send()` (CTS),
  `read_data_set_ready()` (DSR), `read_carrier_detect()`, `read_ring_indicator()`.
  These are the readable lines — a footswitch typically loops an asserted output
  back into one of them.
- ⚠️ **No portable interrupt-driven pin-change notification.** Poll at ~10–20 ms
  (imperceptible for PTT, negligible cost). Windows `WaitCommEvent` with
  `EV_CTS`/`EV_DSR` would be event-driven but needs `windows`-crate FFI and
  therefore **unsafe** — not worth it; start with polling.
- ⚠️ **Some USB-serial chips assert DTR on port open.** That would **key the rig
  the moment the app starts.** Open the port and *explicitly deassert both DTR and
  RTS before anything else*, and verify per adapter.
- Hardware note: use an **optocoupler or open-collector** stage, not a bare
  transistor referenced to PC ground — a PC↔rig ground loop is a classic source of
  RF hum on transmit.

**Proposed shape — mirror the patterns already in this project:**
New crate **`crates/rr-ptt`** (safe, no `unsafe`):
- `trait PttSource` — produces key/unkey intent (serial input pin, UI command,
  inbound-audio gate). Mirrors `ScreenCapturer`'s swappable-backend shape.
- `trait PttSink` — asserts it (serial DTR/RTS). Mirrors `InputInjector`.
- `SerialPtt` — the real implementation; `RecordingPtt` — records instead of
  acting, so the state machine and **all the safety timers are testable headless**,
  exactly as `RecordingInjector` does for input.
- A **watchdog decorator** over `PttSink` enforcing the limits in 14e.

⚠️ **Gate PTT through the existing arbitration.** Keying a transmitter is an input
event in the same sense as a keystroke, so it belongs on the same checked path:
a new `rr_proto::ControlMsg::Ptt { down: bool }` authorised by
`rr_session::Arbiter::may_control`, like every other `ControlMsg` in
`rr-input`'s `ControlGate`. **Only the session holding control may key the rig.**
This also inherits the right failure behaviour for free: `ControlGate::disconnect`
already calls `release_all()` because "a controller who vanishes mid-chord would
leave Ctrl stuck down" — a controller vanishing mid-transmission is **the same bug
class with worse consequences.**

**Latency budget — PTT must lead the audio.** Keying at the instant audio starts
clips the first syllable, because the audio is behind by: Opus frame (20 ms
default) + the mixer's `PRE_BUFFER_SAMPLES` (**4800 = 100 ms**) + network RTT +
the rig's own T/R switchover (10–50 ms). Therefore:
- **PTT must be a control message, not inferred from audio arrival** (mode A).
  Conveniently, `rr-control` is a **direct P2P data channel with no jitter
  buffer**, whereas voice is relayed via the Mumble server — so the control path
  can legitimately *beat* the audio, which is exactly the lead-in needed.
- Make **lead-in and hang-time configurable**; the lead is variable because it
  depends on two different paths.
- For **mode B**, the Station can delay what it feeds the rig's audio input: key
  PTT, wait the configured lead, *then* start feeding audio. Costs latency but
  prevents clipping, and the Station owns that buffer.
- **Hang time** on release so the tail of a transmission is not chopped.

### 14e. ⚠️ Safety: the stuck-transmitter problem

Keying a transmitter is a **physical action with hardware and regulatory
consequences**, and this is the one part of the feature where "fail safe" is not
optional. A stuck PTT can cook the finals, cause prolonged interference, and is an
unattended-transmission problem.

**Non-negotiable requirements:**
1. **Deadman watchdog on the Station.** PTT auto-releases after N ms without a
   keep-alive (mode A) or without audio (mode B). Default short, configurable.
2. **Maximum continuous transmit timeout**, configurable, independent of 1.
3. **Release immediately on any disconnect** — data-channel close, Mumble
   disconnect, control handoff, or loss of the controlling session.
4. **Release on process exit and on panic.** Closing a serial port usually
   deasserts DTR/RTS, but **verify it per adapter** rather than relying on it;
   carry an explicit release in the shutdown path and a `Drop` impl.
5. **A local, always-available kill path on the Station** that does not depend on
   the network being healthy.
6. Fail **closed**: any ambiguity, any error from the serial layer, any arbitration
   doubt → unkey. This is `rr-input`'s "refuse rather than guess" rule applied to
   something with a bigger blast radius.

Note the user is responsible for complying with their own licensing rules for
remote/unattended operation (these vary by country and typically require a control
operator able to shut the transmitter down) — requirement 5 exists partly for that.

### 14f. ⚠️ Pre-existing bugs found during these investigations

**1. Capture sample rate is assumed, not enforced — matters specifically for rig
audio.** `RodioCapture::new` sets `format: AudioFormat::MONO_48KHZ_F32` at
construction; `start()` then only *prefers* 48 kHz
(`prefer_sample_rates([SAMPLE_RATE_48K])`), reads the real rate back from
`mic.config()`, and **merely logs it**. There is **no capture resampler**. So a
codec that opens at 44.1 kHz has its samples consumed at 44.1 k but **labelled
48 k** → Opus encodes them as 48 k → everyone hears the radio ~**8.8% fast and
about a semitone and a half sharp**. The cpal path fails differently: it hardcodes
`sample_rate: 48_000` in the `StreamConfig` (`desktop.rs:174`), so a 44.1k-only
device **will not open at all**. Most modern rig USB codecs are 48 kHz, but
SignaLink and various USB dongles are 44.1 k. **Fix on its own merits.**

**2. Missing icon directories** (also noted in §13). `crates/app-client/
tauri.conf.json` and `crates/app-station/tauri.conf.json` both reference
`icons/32x32.png`, `icons/128x128.png`, `icons/icon.icns`, `icons/icon.ico`, but
**neither `icons/` directory exists locally and there is no `.icns` anywhere**.
§5 says the sets were generated with `cargo tauri icon` **on big-linux** — they may
never have been synced back. **Check before the next release build.**

### 14g. Suggested order of work

0. ✅ **DONE — Opus assumptions proven** (§14b). Both channel-mismatch properties
   hold, so mono fallback is free and no capability gate is needed. Bitrate floor
   measured at **96 kbps** (cliff from 13 dB to 53 dB separation between 72 and
   96). `ForceChannels` and `phase_inversion_disabled` plumbed into
   `OpusEncoderConfig` with Opus-default values. 8 tests, clippy clean.
   **Remaining from this step:** the server relay test, and the transitional
   `ForceChannels` test (correlated → diverging signal).
1. 🟢 **CURRENT — fix the sample-rate bug** (14f.1). Independent, small, and it
   silently corrupts radio audio today.
2. ✅ **DONE — transmit-mode decoupling** (14c). Small, self-contained, no stereo
   needed, and it is a prerequisite for mode-B PTT. See §14c for what shipped.
3. **Stereo plumbing** (14b): add `AudioFormat::STEREO_48KHZ_F32`; make capture and
   playback channel counts **independent and configurable**; replace the two
   downmix sites with conditionals; convert the mixer's sample-relative counts to
   frame-relative.
4. **Bitrate/`force_channels` work** for uncorrelated dual-RX audio.
5. **`rr-ptt`** (14d) with the safety layer (14e) built in from the start, not
   added afterwards — and `RecordingPtt` first, so the timers are tested before
   anything can key a real radio.
6. **Radio-mode profile**: filters off, stereo on, higher bitrate, `Audio`
   application, DTX off, continuous transmit — a named preset matching what the
   user already configures by hand in stock Mumble.

**Not in scope:** Android (`RemoteRadioMumble`) stereo. Its `oboe` backend is a
separate `PlatformAudioFactory` and would need the same treatment; stereo
*playback* there is the only part likely to matter, and it is deferred.

---

## 15. This document moved into the repo

`context.md` was an untracked file at `e:\AI\RRMumble\context.md`, outside the
repo, with no history or backup. It is now
`ARMumble/doc/remote-radio/CONTEXT.md` and committed. Edit it here.

---

## 16. Server investigation and the Fancy Mumble container (committed)

Everything in this section was settled by reading the real server source
(`SetZero/mumble-server`, shallow clone at `e:\AI\RRMumble\mumble-server`,
branches `1.5.x` and `1.6.x`) and by live tests, not by guessing.

### 16a. Two corrections to earlier claims in this document
- ⚠️ **Mumble DOES enforce `bandwidth`.** Earlier notes called it advisory. It
  is not. `Server::processMsg` charges every packet against a **360-slot rolling
  window** (`BandwidthRecord::addFrame`, `N_BANDWIDTH_SLOTS 360`) and silently
  suppresses anything over budget. Proven live: 450 frames of 128 kbps stereo at
  the default `bandwidth=72000` delivered **exactly 360** — the ring size — then
  packets vanished after ~7 s. A Station transmits continuously, so this failure
  is guaranteed, not theoretical. The earlier "advisory" conclusion came from
  1.2 s test runs that never filled the ring.
- ⚠️ **Stock Mumble clients already DECODE stereo.** `AudioOutputSpeech.cpp` on
  both 1.5.x and 1.6.x: `bStereo = true; opus_decoder_create(rate, 2, …)` plus
  `OPUS_SET_PHASE_INVERSION_DISABLED(1)` — the exact control added to our
  encoder, for the exact reason. Their *encoder* is hardcoded mono
  (`opus_encoder_create(rate, 1, …)`). So Mumble's real asymmetry is: it can
  receive stereo, it just never sends it (upstream #2829 is about the capture
  side). **We only change the sending side; receiving already works everywhere.**
  Caveat: decoding stereo does not prove the stock client's mixer/positional
  path keeps the channels separated on output — unverified.

### 16b. The bandwidth numbers that matter
- Mumble's code default is **`iMaxBandwidth = 558000`** (`Meta.cpp`: "510000
  Opus + 9600 position + 38400 TCP overhead"), with **no clamp** on the
  configured value. The widely repeated "128 kbps server cap" is folklore;
  558 kbps is the real ceiling, bounded by Opus's own 510 kbps.
- Server per-packet overhead is **32 bytes** (20 IP + 8 UDP + 4 crypt); our
  client's self-limit uses **38**, i.e. slightly more conservative — the safe
  direction.
- For sustained 128 kbps stereo at 20 ms frames: need `bandwidth` >= ~140 800;
  **set 192000** for headroom to 160 kbps. The RemoteRadio container ships this.

### 16c. Stereo is relayed untouched — proven on a real 1.6 server
`fancy-mumble:1.6` built below, `bandwidth=192000`: **450/450 frames
byte-identical**, no suppression. The server never decodes audio — zero `opus_*`
calls in `src/murmur/`; it relays `opus_data` as an opaque blob and only reads
the payload *size* for rate accounting. That is why stereo passes through.

### 16d. ⚠️ RemoteRadio REQUIRES a Fancy server for signalling
`WebRtcSignal` (wire ID **120**) is a Fancy-only extension. The fork registers
88 TCP message types: 27 stock, 61 Fancy additions. **Stock Mumble silently
drops unknown types**, so RemoteRadio's offer/answer/ICE exchange fails on a
stock server with no error. `newyork.remote.radio` is stock (announces no
`fancy_version`), so the remote-desktop half cannot run there as-is.

Reframes locked design decision #1: "any Fancy-compatible Mumble server" is
load-bearing, not decorative. Voice and stereo work on stock; **signalling needs
the fork**.

⚠️ **The Fancy server must run with the SFU NOT loaded.** `msgWebRtcSignal` has
two modes: with an SFU it *intercepts* START/STOP/SDP_OFFER/ICE_CANDIDATE and
answers them itself; only the "pure relay mode (no SFU loaded)" fallback forwards
them peer to peer. RemoteRadio needs the relay. The container is built
`-Dwebrtc-sfu=OFF` and its startup log confirms "WebRTC SFU disabled in config -
screen sharing will use client-to-client relay".

⚠️ **The fork's `1.5.x` branch has NO Fancy extensions** — 27 message types, no
`WebRtcSignal`, a vanilla tracking branch last touched Feb 2025. All Fancy work
is on `1.6.x`, which is a different (per-table) database schema from 1.5. So
RemoteRadio needs 1.6.x, and porting users from a stock 1.5 server means a
one-way schema migration — **copy the database and migrate the copy**, never the
live one.

### 16e. The Fancy Mumble 1.6 Docker image
Build context: `big-linux:/home/gerry/fancy-server/` (Dockerfile,
mumble-server.ini, docker-entrypoint.sh, build-and-run.sh, verify-fancy-server.sh,
sustain.sh). Authored locally in `E:\devtools\fancy-server\`.

- **Image `fancy-mumble:1.6`**, ~598 MB. Multi-stage, server only. Running on
  big-linux: container `fancy-mumble`, port **54736** (tcp+udp), data in
  `/home/gerry/fancy-server/fancy-mumble-data/`, runs as the host uid.
- **Portable export** `fancy-mumble-1.6.tar.gz` (~111 MB) via
  `./build-and-run.sh --save`. `docker load` on any VPS — no build toolchain
  needed. Built FROM ubuntu:24.04 but runs on a 22.04 VPS; a built image does not
  care about the host release.
- **Credentials in effect:** SuperUser `remoteradioadmin`, Ice write secret
  `remoteRadioIce`. ⚠️ These are documented here deliberately so the running
  server is reproducible; rotate before any public exposure.
- **Ice RPC is ON** (`-Dice=ON`), endpoint `tcp -h 127.0.0.1 -p 6502` (loopback
  only — reach it by SSH, or override `MUMBLE_ICE_ENDPOINT` and firewall it).
  Gives live `setConf`, so `bandwidth` can change without a restart.

#### ⚠️ Six build failures, two of them genuine upstream bugs
The fork's `1.6.x` HEAD does **not** build or link cleanly on Ubuntu 24.04 /
GCC 13 out of the box. The Dockerfile works around all six, each documented
inline:
1. Protobuf not found → add `libprotobuf-dev`, `protobuf-compiler`.
2. SOCI hard-fails configuring its MySQL/PostgreSQL backends → install all
   backend dev headers in the builder stage (discarded, so free in the final
   image); runtime needs `libmysqlclient`/`libpq` because the binary links them.
3. `DNS-SD library not found!` → `-Dzeroconf=OFF` (mDNS, meaningless on a VPS).
   Also `-Ddbus=OFF`, `-Dtranslations=OFF` (needs lrelease), `-Dlto=OFF` (halves
   link time).
4. **UPSTREAM BUG: missing `#include`s.** `std::chrono`, `std::views`,
   `std::memory` and nine other headers are used-but-not-included across ~100
   files; they compile only where libstdc++ supplies them transitively. Fixed
   with GCC `-include` on the command line (`MUMBLE_FORCED_INCLUDES`) rather than
   editing ~100 files — the first `#include` in a file is not always top-level,
   so blind sed insertion can land inside an `#ifdef`.
5. **UPSTREAM BUG: `1.6.x` HEAD does not link.** PR #5
   (feat/audit-log-protocol) added five `FancyAudit*` message types to the
   dispatch table — `Server.h` macro-generates the declarations, `Server.cpp`
   generates the dispatch calls — but no handler bodies exist, so the link fails
   with five undefined references. The Dockerfile appends five no-op definitions
   to `Messages.cpp` (the correct semantics: ignore an unhandled message, the
   shape the fork already uses elsewhere). **Both #4 and #5 are worth reporting
   upstream.**
6. Unresolved runtime libraries + the Rust plugin host. The runtime stage picks
   `t64`-suffixed package names live (Ubuntu 24.04 time_t transition) via a
   `pick` helper, and `COPY`s `libmumble_plugin_host.so` from the builder (it is
   a cargo cdylib, not an apt package) then `ldconfig`. An `ldd` check fails the
   *image build* if anything is unresolved, rather than letting it crash-loop.

#### ⚠️ Entrypoint SuperUser bug, fixed
First version ran `--read-su-pw` on every start, so any later run passing a
different value silently reset the password (hit during image export — a
throwaway run with `=x` reset it). Now gated on the database not yet existing;
`MUMBLE_SUPERUSER_PASSWORD` on a subsequent start is ignored.

#### The verification harness, generalised for this
`crates/mumble-protocol/examples/stereo_live.rs` (committed) grew `--list-channels`,
a `[2a] server identity` line (reads `fancy_version`), a `[2b] bandwidth budget`
line (reads `max_bandwidth`, computes the usable Opus ceiling, shows whether a
requested bitrate would be clamped by a conforming client), and `--sweep` (runs
the whole signal/bitrate matrix over ONE pair of logins). ⚠️ **The autoban
lesson:** each invocation opens two TLS sessions; an external loop that re-ran it
got the IP banned (Mumble default: 10 attempts / 120 s → 300 s ban), which
surfaces as `tls handshake eof` at connect and reads like a TLS fault.
`--sweep` exists so a matrix costs two connections, not 2N. Also ⚠️ a long run
needs periodic `Ping` or murmur drops the client at ~30 s — the harness pings
between sweep cases.

Docker on big-linux: `docker` group, Docker 29.8.2, 24 cores. `sudo` on
big-linux needs an interactive password the agent cannot supply, so the user ran
the install and `usermod -aG docker gerry` steps.

---

## 17. Live-server audio findings (committed; see §14 for the plan)
`crates/mumble-protocol/tests/opus_channels.rs` (8 tests, offline) plus
`examples/stereo_live.rs` (live) established:
- **Mono fallback is free** — a stock mono decoder downmixes our stereo and both
  tones survive; a stereo decoder upmixes mono to identical channels. No protocol
  change, no capability gate.
- **Dual-receiver stereo needs >= 96 kbps** — a ~40 dB separation cliff between
  72 kbps (13 dB, bleeds) and 96 kbps (>50 dB, clean). Pure tones; broadband
  receiver audio couples harder, so **default 128 kbps** with margin. Below-cliff
  figures are bit-identical Windows vs Linux; above-cliff vary by libopus build.
- **`opus 0.3.1` exposes `set_force_channels` and `set_phase_inversion_disabled`**
  — both now in `OpusEncoderConfig`, defaulting to Opus's own defaults so mono is
  unchanged. Retained on documented behaviour, not measured benefit (tones don't
  exercise them).
- ⚠️ **Methodology trap, cost a wrong conclusion:** Opus is predictive; measuring
  a frame by replaying one packet is meaningless. Encode a run, decode in order,
  analyse the tail. The table test now asserts monotonicity so it can't recur.

Still to do from §14: the server relay test is now DONE (§16c); the transitional
`ForceChannels` test and the broadband spectrally-separable test remain.

---

## 18. PIVOT — "Remote.Radio Mumble" (dedicated stereo Mumble client)

> **User decision (big tack change).** The remote-desktop work (P1–P3, P7, the
> `rr-*` crates, `StationControl`) is **parked in git as-is**. The focus moves to
> turning the **Client variant** into a standalone product: a stereo Mumble
> client purpose-built for remote amateur-radio operating. Remote desktop resumes
> *after* this. There is no stereo Mumble client in existence (the Mumble authors
> judged stereo useless for gamers); it is essential for amateur radio —
> **SO2R** (one radio per ear) and modern rigs' main/sub receivers (sub in one
> ear, main in the other).

### 18a. Identity
- Product name **`Remote.Radio Mumble`** (with the dot). Exe **`rrmumble`**.
- This is the **Client** binary (`crates/app-client`), `Mode::Client`. The
  Station binary keeps the parked remote-desktop role.
- The brand strings resolve so Client/Android show "Remote.Radio Mumble"; the
  neutral `brand` key also becomes "Remote.Radio Mumble" (this is the only
  shipping desktop product now).

### 18b. Confirmed decisions
1. **PTT signalling rides hidden Mumble messages** (text-message / plugin-data),
   **not** the `rr-control` WebRTC channel (that belongs to the parked
   remote-desktop stack and needs no server for plain Mumble). The PTT key/unkey
   and the operator-command keying both use this channel, and these messages are
   **hidden from the chat display**. (Chosen over a new Fancy server message type
   to avoid a server dependency for PTT.)
2. **Station PTT output triggers on BOTH**: audio-detected VOX on the received
   Mumble stream (works even with a stock client) **and** an explicit operator
   key/unkey command arriving over the hidden-message channel.
3. The 7 built-in servers use password **`Demo!`** (free, OK to ship in the exe).
4. Live tests: **MacPro == `big-linux`**, Fancy 1.6 server, port **54736**,
   password **`Demo!`**.

### 18c. The 7 hardcoded Remote Radio servers
All `*.remote.radio:54736`, Fancy 1.6, password `Demo!`. Shown as a built-in
group **above** saved servers on the sign-in screen; no browse-public anywhere.
New York (`newyork.remote.radio`), Toronto (`toronto.remote.radio`), London
(`london.remote.radio`), Los Angeles (`la.remote.radio`), South Africa
(`africa.remote.radio`), Winnipeg (`winnipeg.remote.radio`), Frankfurt
(`frankfurt.remote.radio`).

### 18d. Station vs Operator mode (per saved server / per session)
A radio choice on sign-in sets the role:
- **Operator** — username label "Operator Callsign", hint "W1VE, etc". Round
  avatar (circle + initial), as today.
- **Station** — username label "Station Name", hint "Elecraft-K3, etc". **Square**
  avatar in a different theme colour + initial. Everywhere the UI says "user" →
  "Station" for station identities.

### 18e. MVP task list (UI first, then serial PTT)
1. Rebrand Client → `Remote.Radio Mumble` / `rrmumble`; wire per-mode brand
   through TitleBar, `index.html`, and the ConnectPage `<h1>` (was "Fancy Mumble").
2. Remove **browse-public-servers** (keep `fetch_file_server_capabilities`,
   `ping_server`).
3. Remove **Friends** (frontend-only; untangle `dmStorage` friend-keying).
4. Remove **Klipy/GIF** (strip the built-in API key).
5. **Hardcoded server group** + mockup sign-in layout (`signin-mockup.png`).
6. Add-server default port **54736** (old ports still supported).
7. **Station/Operator** role on sign-in → label/hint changes, persisted.
8. **"Station" terminology + square avatar** for station identities.
9. **Per-server audio settings** (key by `ServerId`; AudioPanel server dropdown).
10. **Activation-mode gating**: Station = Continuous only ("Continuously transmit
    the station receiver audio."); Operator "Voice Activation (VOX)" + "See
    Advanced to trigger station-side PTT"; PTT card "Transmit only when a hotkey
    or serial PTT footswitch (see Advanced) is pressed." Wire to real
    `transmit_mode`.
11. **Stereo routing UI + playback**: single-server (L→left/R→right, or mixed;
    separate L/R volume when split) and multi-server (each physical out channel =
    a `[Server]-Left/[Server]-Right` dropdown). Stop mono-summing in the mixer.
12. **Finish §14 backend audio** (stereo plumbing, bitrate/`force_channels`, HF
    defaults) with tests + live MacPro verification.
13. **PAUSE for user review of all UI changes.**
14. **Serial PTT** (after review): `rr-ptt` crate (`serialport` 4.x, no unsafe).
    Operator = serial **input** (DSR/CTS going low → PTT). Station = serial
    **output** (drive DTR/RTS low to key TX). Dynamic alphabetical COM-port list,
    "(none)" first, PnP hot-plug aware, sticky selection. Advanced-settings UI per
    role. Key/unkey over the hidden-message channel (18b.1).

### 18f. Removals rationale
No public Mumble servers ever (amateur-radio-only app). Friends and Klipy are
gamer/social features irrelevant to the use case and are frontend-only, so cheap
to remove. HF receive audio must **not** be distorted by speech processing —
default to minimal/no noise suppression for the radio profile.

### 18g. Session progress — UI pivot DONE through MVP step 8 (uncommitted)

> **✅ COMMITTED (this batch).** The UI pivot (MVP steps 1–9 below) is now
> committed locally in 5 logical commits on `feature/remote-radio-p0`, on top of
> the already-landed §14f.1 sample-rate fix (`4696bbec` resampler + `98409e17`
> rate-relabel) and the §10 doc commit `8ce234e2`. **Still LOCAL only — NOT on
> big-linux** (mumble-tauri cannot build there: no ALSA). The app runs via
> `tauri dev` and all UI changes were visually reviewed and approved by the user.
>
> | commit | subject |
> |---|---|
> | `a89b1376` | `feat(brand)`: rebrand Client to Remote.Radio Mumble / rrmumble |
> | `a6fc8ac0` | `feat(ui)`: remove gamer/social features (public servers, Friends, Klipy/GIF) |
> | `0a40681d` | `feat(ui)`: hardcoded server group + Operator/Station role on sign-in |
> | `e98cd1ac` | `feat(ui)`: first-run role, persisted default role, Station iconography |
> | `209a822d` | `fix(app-client)`: resolve ACL capabilities so the app clears the splash |
>
> Pre-commit verification (local Windows): `npm run build` (tsc+vite) clean;
> `vitest run` **976 passed / 82 files, 0 fail**; `cargo check -p mumble-tauri`
> clean; `cargo check -p app-client` clean except the expected 9 unused-extern
> warnings (MVP task #7). Commit messages written BOM-free via
> `UTF8Encoding($false)`. `crates/*/gen/schemas/` is now gitignored (generated
> per-binary ACL output; app-station never tracked it either).

**DONE and user-approved (MVP steps 1–8 + welcome role + station iconography):**

1. **Branding → "Remote.Radio Mumble" / exe `rrmumble`.**
   - `crates/app-client/tauri.conf.json`: `productName "Remote.Radio Mumble"`,
     `identifier "com.remoteradio.mumble"`, window `title "Remote.Radio Mumble"`.
     (The dot in productName is accepted by tauri-build.)
   - `crates/app-client/Cargo.toml`: `[[bin]] name = "rrmumble"`, `default-run`.
   - `ui/src/locales/en/common.json`: `brand`/`brandClient`/`brandMumble` =
     "Remote.Radio Mumble"; `brandStation` kept "RemoteRadio Station".
   - `ui/index.html` title; ConnectPage `<h1>` → `t("common:brand")`;
     IdentitiesPanel `.fmid` filter label. TitleBar already used `t("brand")`.

2. **Removed browse-public-servers.** Deleted `components/server/PublicServerList.{tsx,module.css}`
   + its test. ConnectPage: removed the "public" view/handlers/link. `types.ts`:
   removed `PublicServer`. `commands/public_servers.rs`: rewritten to keep ONLY
   `fetch_file_server_capabilities` (removed `fetch_public_servers` + XML parser +
   tests); unregistered from `lib.rs`. Removed now-unused `quick-xml` dep from
   `mumble-tauri/Cargo.toml`. Orphan `publicServersLink` locale keys left (dead).

3. **Removed Friends** (frontend-only; DMs kept — they key off user hash).
   Deleted `pages/FriendsPage.{tsx,module.css}`, `friendsStorage.ts`. Stripped
   friend-toggle from `UserContextMenu`, the `/friends` route + lazy import in
   `App.tsx`, the friends tab in `ServerTabsBar`, the friends row in
   `AddServerPopover`. `dmStorage.ts` untouched. Orphan i18n keys left (dead).

4. **Removed Klipy/GIF.** Deleted `components/chat/gif/{GifPicker.tsx,
   GifPicker.module.css,klipyConfig.ts}` and `pages/settings/KlipyGifBrowser.{tsx,
   module.css}`. ChatComposer: removed GIF button + picker + `onGifSelect` prop.
   ChatView/useChatSend: removed `handleGifSelect`. AvatarEditorModal +
   BannerEditorModal rewritten to drop the gif tab (Avatar = image only; Banner =
   color + image). SettingsPage/AdvancedPanel: removed the Klipy API-key field.
   `types.ts`/`preferencesStorage.ts`: removed `klipyApiKey`. `App.tsx`: removed
   `setKlipyApiKey`. `media.ts` `fetchAsDataUrl` kept (now unused, harmless).

5. **Hardcoded 7-server sign-in group.** NEW `ui/src/builtinServers.ts`
   (`RR_PORT = 54736`, `RR_PASSWORD = "Demo!"`, `BUILTIN_SERVERS` with
   name/host/abbr for NYC/TOR/LON/LA/ZA/WPG/FRA, `filterBuiltinServers`). NEW
   `components/server/BuiltinServerList.{tsx,module.css}` matching the mockup
   (heading + "+ Add Server", quick-select dropdown, search, cards with a square
   gradient city badge). ConnectPage renders it above the saved `ServerList`
   (which now only shows when there are saved servers); initial view is always
   "servers" so the built-in list shows even with zero saved.

6. **Add-server default port 54736** (was 64738): ConnectPage state, resetWizard,
   both `Number.parseInt(port) || RR_PORT` fallbacks.

7. **Operator/Station role.** `types.ts`: `ClientRole = "operator" | "station"`,
   `SavedServer.role?`. ConnectPage wizard step 0 has an Operator/Station radio
   (`.roleToggle`/`.roleOption` CSS); step-1 username label/placeholder is
   role-aware; station step-1 hint overridden with
   `wizard.stationNameHint`. `handleConnectAndSave` persists `role`.
   `handleBuiltinConnect` connects with `RR_PASSWORD`, username = defaultUsername
   or the abbr, no cert, not persisted.

8. **"Station" iconography.** NEW `ui/src/stationIdentity.tsx` (`.tsx` because it
   has JSX): `useStationNames()` (set of lower-cased usernames of saved servers
   with `role === "station"`), `isStationName`, `useIsStation()`, `StationGlyph`
   (returns `<RadioIcon>` when station else the fallback), `notifyServersChanged()`
   fires `"rr:servers-changed"`. ConnectPage calls `notifyServersChanged()` on
   add/edit/delete. The station glyph is wired EVERYWHERE a user appears:
   `UserListItem` (gradient SQUARE badge via `.avatar.avatarStation` compound
   selector + icon), `ProfilePreviewCard`, `UserProfileView`, `UserHoverCard`,
   `ChannelIconList` (modern), `ModernChannelList` (flat member + CollapsedAvatar),
   `ClassicChannelList` (StackedAvatar + TooltipUser). All pass `isStation` through
   `UserHoverCardPortal` too. NOT YET wired: the bottom-left self-identity chip.

   **⚠️ Station detection is a PLACEHOLDER (option b):** name-match of a channel
   user against saved `station`-role usernames. The AUTHORITATIVE design (agreed):
   broadcast each client's role over the Fancy **plugin-data** channel (the same
   hidden channel used for PTT), visible to all clients. Deferred to the serial-PTT
   phase since they share transport. `stationIdentity.tsx` header documents this.

9. **Welcome screen (first-run) reworked (option 2).** `WelcomePage` now has an
   Operator/Station role toggle that sets the persisted default role; brand header
   `t("common:brand")` + `welcomeTagline`; role-aware name label. `types.ts`
   `UserPreferences.defaultRole?`; `preferencesStorage` DEFAULTS `defaultRole:
   "operator"` + `completeSetup(mode, name, role)`. ConnectPage reads
   `prefs.defaultRole` into its role state and `resetWizard` uses it. New
   `en/server.json` welcome keys (`welcomeTagline`, `welcomeRole*`,
   `welcomeNameLabel*`). (de/fr/zh fall back to key names — English is default.)

**ICON:** stations use `RadioIcon`, aliased in `ui/src/icons.ts` to lucide
**`Antenna`** (the dipole/mast — user's final choice after trying RadioReceiver
and RadioTower). To change the station icon globally, change that one alias.

**ICONS GENERATED:** `npx tauri icon RemoteRadioClient.svg` produced the full set
into `crates/app-client/icons/`. Installed `@tauri-apps/cli` as a UI devDep to do
it. These are PLACEHOLDER icons from the generic client SVG — swap when the real
Remote.Radio Mumble logo exists, then regenerate.

**⚠️ CRITICAL BUILD FIX — app-client capabilities were empty (`{}`).** This caused
the app to hang on the loading splash: with no ACL, every Tauri plugin IPC
(store, notification, …) was denied, so `isFirstRun()`'s store read rejected and
`firstRun` stayed `null` forever. **Root cause:** Tauri resolves capabilities per
*binary* from a `capabilities/` dir next to the config, and can only resolve a
plugin's permission schema when that plugin is a **direct dependency** of the
binary. `app-client` had neither. **Fix:** (a) added
`crates/app-client/capabilities/*.json` (the 5 files copied from
`mumble-tauri/capabilities/`), and (b) added the Tauri plugin crates as direct
deps in `crates/app-client/Cargo.toml` (`store, opener, notification, dialog,
deep-link, global-shortcut, window-state, updater, process`). Generated
`capabilities.json` went from `{}` (2 bytes) → ~2.6 KB. Also added a safety net:
`App.tsx` `isFirstRun().then(setFirstRun).catch(() => setFirstRun(false))`.
The 9 "unused extern crate" warnings in the `rrmumble` build come from those
plugin deps (used for ACL, not `use`d) — harmless, TODO silence with
`use tauri_plugin_x as _;` lines (MVP task #7).

**HOW TO RUN / DEV (both scripts in `E:\devtools\`):**
- **`tauri-dev.cmd`** — `tauri dev` with the MSVC+nasm+protoc env; Vite on :1420 +
  hot reload. UI edits reload live; Rust edits incrementally rebuild. **Use this
  for UI iteration — do NOT do a full `tauri build` for every tweak.**
- **`tauri-build.cmd`** — production `tauri build --no-bundle`; standalone exe at
  `target/release/rrmumble.exe` with the frontend embedded (no dev server). ~10 min.
- ⚠️ **A plain `cargo run`/debug build loads from `devUrl` (localhost:1420) and the
  fork auto-spawns Vite.** A *stale* Vite server on :1420 serving an old bundle was
  the first (red-herring) cause of a hang; always kill :1420 before launching.
  Standalone review = `tauri build`; live iteration = `tauri dev`.
- First-run was reset this session by deleting
  `%APPDATA%\com.remoteradio.mumble\preferences.json` to show the welcome screen.

**VERIFIED:** UI `tsc --noEmit` clean after every change; `npm run build` clean;
full `vitest run` = **976 passed / 82 files, 0 fail** (before the station-glyph
spread; re-run before commit). `mumble-tauri` `cargo check` clean. app-client
`tauri build` succeeds and the standalone exe launches past the splash.
**mumble-tauri does NOT build on big-linux (no ALSA)** — it is desktop/Android
only, so verify UI/app-client work LOCALLY on Windows; big-linux stays for the
headless RemoteRadio crates + mumble-protocol.

**KNOWN LEFTOVER BRANDING LEAK:** the right-side profile panel shows client
**Version "FancyMumble 0.3.0"** — a Rust-side version/agent string, not UI. Fix in
the next backend pass (grep the fork for the client-version/user-agent constant).

**MVP REMAINING (next session), in order:**
- ✅ **DONE — committed this UI batch** in 5 logical commits (see the table at
  the top of §18g). Grouped branding / removals / sign-in / welcome+role+avatar /
  build-fixes. pwsh no-BOM writer, local only, `vitest` re-run green first.
- ✅ **DONE — per-server audio settings** (commit `4faa3b4b`). `get/set_audio_settings`
  gained an optional `serverId`; `AppState::audio_settings_for` /
  `set_audio_settings_for` resolve the target `SharedState` from the registry
  (fallback = active session), and only report a pipeline/force-TCP restart when
  the target IS the active session. TS persistence is now a
  `Record<"host:port", AudioSettings>` with a reserved default bucket
  (`AUDIO_DEFAULT_KEY`) that seeds new servers; legacy single-blob values migrate
  into it on first read. SettingsPage tracks the edited server (defaults to
  active), reloads on switch without tripping the debounced autosave, and keys the
  invoke (serverId) + persistence (host:port) by it. AudioPanel got a "Settings
  for" dropdown. ChatPage's activation-mode toggle passes the active serverId.
  New `AudioSettingsStorage.test.ts` (6 tests); UI 982 pass; backend clippy clean
  (only the 3 pre-existing updater `doc_markdown` warnings). LOCAL only.
- ✅ **DONE — activation-mode gating + copy** (commit `c5c8806d`). TS `AudioSettings`
  gained `transmit_mode` (`continuous`/`voice_activity`/`push_to_talk`, matching the
  backend serde). `ActivationModeSelector` now reads/writes `transmit_mode`
  directly (keeping `push_to_talk` in step for the legacy self-mute path) and
  **no longer touches `noise_suppression`** — the §14c decoupling is now reflected
  in the UI. It takes a `role` prop: a **Station is offered Continuous only** with
  station copy ("Continuously transmit the station receiver audio."); Operator gets
  "Voice Activation (VOX)" (+ "See Advanced to trigger station-side PTT") and the
  PTT card ("Transmit only when a hotkey or serial PTT footswitch (see Advanced) is
  pressed."). AudioPanel computes the effective mode from `transmit_mode` for the
  voice-gate/PTT-hint visibility. SettingsPage resolves the edited server's role
  (saved-server role by host:port, else the user's `defaultRole`) and normalises a
  Station to Continuous. AudioPanel activation tests rewritten (+Station-only and
  legacy-reconcile cases). UI 984 pass. LOCAL only.
  ⚠️ `VoicePanel.tsx` still toggles the raw `push_to_talk` bool, but it is **dead
  code** (not imported/rendered anywhere) — left untouched. Other locales (de/fr/zh)
  fall back to English for the renamed activation keys, same as the rest of the pivot.
- **Stereo routing UI + playback** (needs backend stereo plumbing first).
- ✅ **DONE — §14 backend stereo plumbing** (commit `057cb42a`, LIVE-VERIFIED).
  `AudioFormat::STEREO_48KHZ_F32` added; the inbound mixer is built stereo at all
  three `state/audio.rs` sites (a stereo decoder upmixes mono senders, so mono
  traffic is unaffected — the free fallback). The three mono-sample mixer
  constants are channel-aware now: the per-speaker buffer cap scales by channels,
  and the cold-start fade + boundary crossfade advance **per audio frame** so L
  and R get equal gain (the §14b bug). The **rodio** backend (default) yields
  interleaved L R and declares `channels()=2`; the **cpal** fallback folds L+R→mono
  for its mono-fan output (documented limitation — true stereo is on rodio).
  Capture stays mono. Local: clippy 0 on both crates, mixer 20 / opus_channels 8 /
  mumble-tauri audio 35 pass. **Live on the Fancy 1.6 server
  (`newyork.remote.radio:54736`, 192 kbps):** `stereo_live --sweep` PASS — relayed
  byte-for-byte, left-only = digital silence in the right channel at every bitrate,
  tones separation 28 dB @72k → 58/75 dB @96k (the cliff). **User confirmed by ear
  on the built `rrmumble.exe`:** clean L/R separation, two hard-panned tones at
  96 kbps. Still TODO from §14: the radio-mode **profile** (≥96 kbps default,
  filters off, `Audio` application) and bitrate/`force_channels` UI exposure.
- ✅ **DONE — stereo routing UI (layer 1)** (commit `ac6b8956`). An operator
  "Stereo Output" card picks how the decoded stereo maps to the device:
  **Stereo** (L→left/R→right, default; + independent L/R gain sliders), **Mono**
  (sum both to both ears), or **Swap L/R**. `AudioSettings` gained
  `playback_stereo_mode` (`StereoMode` enum) + `left_volume`/`right_volume`
  (serde defaults, old configs unchanged). Backend: `mixer::apply_stereo_mode`
  (interleaved in-place fold, no-op fast path) + a lock-free `StereoControl`
  handle wired through `create_mixing_playback` into both backends and
  live-updated in `set_audio_settings_for` (no pipeline restart); rodio applies
  it in `refill_chunk`, cpal before its mono fold, Android ignores it (deferred).
  ⚠️ `voice_replay` now duplicates its mono mic recording to L/R so replay plays
  at correct speed. Tests: mixer 25 (+5), AudioPanel +4, UI 988, clippy 0.
  **DEFERRED to its own session: the multi-server physical-output-channel
  routing matrix (layer 2)** — a larger per-server output-routing layer; the
  current playback mixes all speakers into one shared stereo stream.
- **Serial PTT** (`rr-ptt`) + the authoritative role broadcast that replaces the
  station name-match placeholder.
- ✅ **DONE — silenced the 9 unused-extern warnings** (commit `c4618f35`).
  `crates/app-client/src/main.rs` binds each plugin dep as `use
  tauri_plugin_x as _;` (they are direct deps only for ACL-schema resolution,
  used by the shared lib). The `rrmumble` build is now warning-free. Also fixed a
  `chunks_exact_to_as_chunks` clippy lint in the new `apply_stereo_mode`.
- ✅ **DONE — Serial PTT (`rr-ptt`) + authoritative role broadcast** (§18e.14,
  §14d/§14e, §18b.1). Commits `71b63d8e` (rr-ptt crate), `3c45056f` (backend
  wiring), `7f4db4dc` (UI).
  - **`crates/rr-ptt`** (safe, `#![forbid(unsafe_code)]`, `serialport` 4.x behind
    a `serial` feature): `PttSink`/`PttSource` traits, `SerialPttSink` (drives
    DTR/RTS; **deasserts both on open** to dodge the USB auto-key trap, releases
    on drop) / `SerialPttSource` (senses CTS/DSR/CD/RI; a failed read is "no
    change", never unkey), `RecordingPtt*` doubles, and the **`Watchdog`** (§14e:
    deadman, latching max-transmit ceiling, fail-closed on any sink error, release
    on drop; time injected so it is tested headless). 12 crate tests. **Serial
    path hardware-verified** by the user on a com0com null-modem pair: DTR/RTS key
    in sync and start low on open; a sense line captured all key/unkey edges.
  - **Transport = Mumble `PluginMessage`** (`plugin_name "remote-radio"`,
    payloadType `ptt`/`role`), a distinct message type from `TextMessage` so it is
    inherently hidden from chat. **Not** `PluginDataTransmission` (forbidden in
    Fancy 0.3.2+) and **not** the parked `rr-control` WebRTC stack. There is no
    `rr-session` arbiter in this path — the watchdog is the safety gate.
  - **Backend** (`state/ptt.rs`, desktop): a worker-thread `PttController` owns the
    watchdog + serial. Station keys on a remote operator command **and/or**
    inbound-audio **VOX** (mode B, a poller mirrors `talking_sessions`); Operator
    senses a footswitch and emits outbound `remote-radio/ptt`. Lead-in/hang-time
    smooth the T/R switch. `PttConfig` per-server; commands `list_serial_ports`,
    `get/set_ptt_config`, `apply_remote_ptt`. Released on disconnect (§14e req 3).
  - **Frontend**: `dispatchPluginMessage` routes role → an authoritative
    session→role map (`stationIdentity.tsx`, replacing the name-match placeholder;
    `useIsStation` prefers it, falls back to the hint) and ptt → `apply_remote_ptt`
    for a Station. `announceLocalRole` broadcasts on connect. `PttPanel` (Advanced,
    per role) with the COM-port picker, line/active-low, VOX (station), and the
    lead/hang/deadman/max-tx sliders. 4 panel tests; UI vitest 992/84.
  - ⚠️ **Still open (follow-ups):** full disk persistence of `PttConfig` +
    auto-start-on-connect independent of opening Settings (today it re-applies when
    the Audio panel loads while connected); and the §14 **radio-mode profile**
    (default ≥96 kbps, filters off, `Audio` application, continuous) +
    bitrate/`force_channels` UI exposure. End-to-end over-the-air PTT through the
    built app is the user's to confirm.

**NEW FILES this session (so `git add` catches them):**
`ui/src/builtinServers.ts`, `ui/src/stationIdentity.tsx`,
`ui/src/components/server/BuiltinServerList.{tsx,module.css}`,
`crates/app-client/capabilities/*.json`, `crates/app-client/icons/*`.
