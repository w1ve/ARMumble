# RemoteRadio — Technical Specification (v0.1 draft for review)

> Status: **v1.0 — APPROVED.** All design decisions are resolved (see §10).
> This document specs the modification of the ARMumble (FancyMumble fork)
> project into two complementary Windows desktop applications plus a renamed
> Android app. Implementation proceeds per the phased plan in §9.

---

## 1. Overview & goals

Build three deliverables from one Rust/Tauri workspace:

| App | Platform | Role |
|---|---|---|
| **RemoteRadioStation** | Windows (desktop) | Mumble client **+** remote-desktop *host*: captures its own screen, injects received input, serves clipboard/files. |
| **RemoteRadioClient** | Windows (desktop) | Mumble client **+** remote-desktop *viewer/controller*: renders the Station's screen, captures local mouse/keyboard and sends it, drives handoff UI. |
| **RemoteRadioMumble** | Android | The existing ARMumble Android app, **renamed only** (no behavioral change in v1). |

Core principles (from requirements):

- **Fully peer-to-peer media.** The Mumble server is used **only** as a
  signaling relay. No custom/SFU server module is required; any
  Fancy-compatible Mumble server works. (This is a deliberate departure from
  the existing in-repo screen-share, which routes media through a server SFU
  that is *not* in this repository.)
- **Lowest latency possible.** Native Rust capture (`xcap` + a Windows
  DXGI/WGC fast path), native encode, `webrtc-rs` transport, latency-tuned
  encoder settings, direct P2P paths with STUN hole-punching and TURN fallback.
- **Multi-viewer, single-controller.** Several Clients may *view* one Station
  concurrently; exactly one may *control* mouse/keyboard at a time. Control
  handoff is **cooperative** (no forcible takeover).
- **"Users" = Mumble channel members.** Presence, identity, and signaling all
  key off the Mumble channel the user is in.
- **No audio over WebRTC** — voice stays on Mumble.
- **Features:** screen (multi-monitor), mouse+keyboard control, clipboard sync,
  file transfer.
- **Everything verified by tests** (unit, integration, loopback E2E, NAT-sim).

---

## 2. What already exists (ground truth from code review)

Documented so the design reuses, not reinvents. File references are to the
current tree under `crates/`.

- **Signaling over Mumble is already built and wired.** `WebRtcSignal`
  (Mumble.proto wire ID **120**) carries `target_session`, server-stamped
  `sender_session`, a `SignalType` enum (`START=0, STOP=1, SDP_OFFER=2,
  SDP_ANSWER=3, ICE_CANDIDATE=4`), and a `payload` string (SDP, or JSON for
  ICE). Full path exists:
  - Backend inbound: `mumble-tauri/src/state/handler/webrtc_signal.rs` → emits
    the `webrtc-signal` Tauri event (`WebRtcSignalPayload`).
  - Backend command: `commands/realtime.rs::send_webrtc_signal(target_session,
    signal_type, payload, server_id)` → `AppState::send_webrtc_signal`
    (`state/protocol_commands.rs`) → `command::SendWebRtcSignal`.
  - Frontend: `store.ts` `sendWebRtcSignal(...)` / `onWebRtcSignal(...)` +
    `listen("webrtc-signal", ...)`; constants in `constants/tauriEvents.ts`.
  - Legacy fallback: `fancy_message_support.rs` / `fancy_codec.rs` wrap
    `WebRtcSignal` into `PluginDataTransmission` for pre-0.2.12 servers.
- **The existing media stack is SFU-based and lives in the webview**
  (`ui/.../stream/useScreenShare.ts`, `pages/StreamPopoutPage.tsx`): browser
  `getDisplayMedia`, one send-only PC to a **server SFU**, `webrtc_sfu_available`
  capability flag. **We will not use this media path.** We keep its *signaling*
  patterns and the START/STOP presence idea.
- **No native media code** anywhere (no `webrtc`/`str0m`/`gstreamer`/`scrap`/
  `xcap`, no encoder). **No `RTCDataChannel`** anywhere. Both are greenfield.
- **State & presence**: protocol `ServerState` (`mumble-protocol/src/state.rs`:
  `User`, `Channel`, `own_session()`); rich UI `SharedState` in the Tauri
  backend; join/leave handlers in `state/handler/user_state.rs` &
  `user_remove.rs` emit `state-changed`, `current-channel-changed`, etc.
- **Command/event bridge**: commands registered via `all_command_handlers!`
  (`generate_handler!`) in `lib.rs`; backend→frontend via `AppHandle::emit`
  with a `serverId` stamp (`state/event_handler.rs`, `state/handler/mod.rs`).
- **App identity** is file-based: desktop `tauri.conf.json`
  (`productName=FancyMumble`, `identifier=com.fancymumble.app`); Android
  `gen/android/app/build.gradle.kts` (`applicationId`). CI already uses the
  `TAURI_CONFIG` env override to inject per-OS bundle resources — the hook we
  reuse for per-variant identity.
- **Platform abstraction** exists: `platform::PlatformHooks` trait with a
  cfg-selected `Active` type (`platform/mod.rs`), window backends under
  `platform/window/`. Desktop vs Android deps are `cfg`-gated in
  `mumble-tauri/Cargo.toml`.
- **Build/packaging**: desktop cross-built from Linux via `cargo-xwin` + NSIS
  (per `opt/BUILDING-LOCAL.md` on the build host); signal-bridge is an
  AGPL-isolated cdylib built separately and bundled as a resource.

---

## 3. Architecture

### 3.1 Component diagram (target)

```
 RemoteRadioStation (Windows)                       RemoteRadioClient (Windows)
 ─────────────────────────────                      ─────────────────────────────
  Mumble client (unchanged)                          Mumble client (unchanged)
  rr-capture    (xcap / DXGI-WGC)                     egui/<video> render target
  rr-encode     (H.264/VP8, latency-tuned)            rr-decode (openh264/vpx)
  rr-input-sink (enigo inject)                        rr-input-src (capture mouse/kb)
  rr-clipboard  (arboard)                             rr-clipboard (arboard)
  rr-files      (chunked xfer)                         rr-files (chunked xfer)
  rr-session    (arbitration host)                     rr-session (controller/viewer)
        │                                                     │
        │   webrtc-rs PeerConnection (one per viewer)         │
        │   • Video track(s): encoded samples  ───────────────▶ render
        │   • DataChannel "control"  ◀─────────────────────────  input/handoff
        │   • DataChannel "clipboard" ◀───────────────────────▶  clipboard
        │   • DataChannel "files"     ◀───────────────────────▶  files
        │                                                     │
        └──────────── ICE: STUN hole-punch, TURN relay ───────┘
                              ▲                 ▲
                              │ SDP/ICE         │ presence (START/STOP/CONTROL)
                              └──── Mumble WebRtcSignal (ID 120) relay ────┘
```

### 3.2 Workspace / crate layout (decision #6: thin binaries over shared lib)

New crates (all MIT; keep the AGPL signal-bridge isolation intact):

```
crates/
  rr-core/            # shared Tauri app library: builder, commands, UI bridge,
                      # mode selection (Client vs Station), re-exports mumble.
  rr-webrtc/          # webrtc-rs peer management, tracks, data channels,
                      # ICE/STUN/TURN config, signaling glue to WebRtcSignal.
  rr-capture/         # screen capture: trait + xcap backend + windows DXGI/WGC
                      # fast path; multi-monitor enumeration.
  rr-codec/           # encode/decode: H.264 (openh264) + VP8 (libvpx/rav1e?),
                      # latency presets, keyframe-on-demand, bitrate control.
  rr-input/           # input model (serde) + enigo sink + client-side capture.
  rr-transfer/        # clipboard sync + chunked file transfer over data channels.
  rr-session/         # control-arbitration state machine (cooperative handoff),
                      # viewer registry, presence.
  rr-proto/           # shared wire types (control/clipboard/file messages),
                      # versioned, serde + bincode/prost.
  app-client/         # [[bin]] RemoteRadioClient — thin: picks Client mode.
  app-station/        # [[bin]] RemoteRadioStation — thin: picks Station mode.
```

- `mumble-tauri` stays as the **Android** app crate (renamed product only →
  RemoteRadioMumble), and continues to produce the shared protocol/UI it does
  today. **[CONFIRM]** whether we (a) refactor `mumble-tauri`'s reusable guts
  into `rr-core` and have all three apps depend on it, or (b) keep
  `mumble-tauri` as-is for Android and let `rr-core` depend on it. Recommended:
  (a) — extract shared code into `rr-core`, Android app becomes another thin
  binary. Larger refactor but one source of truth.
- Each desktop binary ships its **own `tauri.conf.json`** (distinct
  `productName`, `identifier` — e.g. `com.remoteradio.station` /
  `com.remoteradio.client` — window title, icons, updater endpoint). Distinct
  identifiers are mandatory: shared identifiers collide on install,
  single-instance, and deep-link registration.
- Mode is also selectable at runtime via a cargo feature **and** a config
  constant so `rr-core` can `#[cfg(feature="station")]`-gate host-only
  subsystems (capture/encode/input-sink) out of the Client binary and vice
  versa, keeping each binary minimal and reducing attack surface.

### 3.3 Media pipeline (Station → Client), lowest-latency path

1. **Capture** (`rr-capture`): behind a `ScreenCapturer` trait.
   - Portable backend: `xcap` (frames are raw BGRA; we add our own
     stride/format/timestamp metadata since xcap omits them).
   - Windows fast path **[CONFIRM]**: native **DXGI Desktop Duplication** or
     **Windows.Graphics.Capture (WGC)** for zero-copy, damage-region, high-FPS
     capture. This is the real "lowest latency" win on Windows; `xcap` is the
     cross-platform fallback and dev baseline.
   - Multi-monitor: enumerate monitors; capture the selected one; allow
     switching mid-session via a control message.
2. **Encode** (`rr-codec`): latency-tuned, **not** quality-tuned.
   - Default **H.264** via `openh264` (BSD, redistributable) with
     `UltraLow`-latency / zero-B-frame / sliced config, infinite GOP +
     **keyframe-on-demand** (new viewer or packet loss requests an IDR).
     VP8 (`libvpx`) as the fallback/negotiated alternative.
   - **[CONFIRM]** hardware encoders (NVENC / AMF / QuickSync / Media
     Foundation) as an optional backend for big latency/CPU wins. Recommend
     making the encoder a trait with a SW default and optional HW backends
     feature-gated per vendor; auto-detect at runtime.
   - Adaptive bitrate driven by `webrtc-rs` RTCP feedback (REMB/TWCC) per peer.
3. **Transport** (`rr-webrtc`): `TrackLocalStaticSample.write_sample()` feeds
   encoded Annex-B/VP8 frames into one video track **per viewer PC**. Each
   viewer is an independent `RTCPeerConnection` (true P2P, decision #1).
4. **Decode & render** (`rr-codec` + UI): `openh264`/`vpx` decode →
   RGBA/texture → render in the Client's right-pane. Decision between a
   WebRTC-native `<video>` sink vs. a Rust-side decoder + GPU texture is in
   §3.6.

**Uplink budget (decision #2 — N× cost acknowledged).** With pure P2P the
Station encodes/sends one stream per viewer. We mitigate, not eliminate:
- Per-peer adaptive bitrate; a global cap splits bandwidth across viewers.
- Encode once, packetize per peer where codecs/bitrate match (optimization,
  phase 2+).
- Document a recommended ceiling (e.g. ≤ ~5 viewers at 1080p30 on a typical
  uplink) and surface a UI warning past a threshold.
- **[CONFIRM]** a future opt-in SFU/relay for large audiences is explicitly
  out of scope for v1.

### 3.4 Control & data channels (greenfield, over `webrtc-rs` DataChannels)

Three ordered/unordered channels per PC, negotiated in the offer:

| Channel | Reliability | Purpose |
|---|---|---|
| `rr-control` | reliable, ordered | input events (only from the active controller), control-handoff protocol, monitor switch, cursor/metadata, keyframe requests. |
| `rr-clipboard` | reliable, ordered | clipboard text/image sync (bidirectional, policy-gated). |
| `rr-files` | reliable, ordered | chunked file transfer with offer/accept, backpressure, hashing. |

Input events (`rr-input`/`rr-proto`): serde-serialized (enigo is serde-friendly)
compact structs — `MouseMove{mon, x, y}` (absolute, per-monitor DPI-aware),
`MouseButton{btn, down}`, `Wheel{dx,dy}`, `Key{code, down}`, `Text{str}`.
The Station **validates the sender is the current controller** before injecting
(defense in depth even though only the controller should be sending).

### 3.5 Signaling & NAT traversal (decision #1 + diagram)

Reuse Mumble `WebRtcSignal` (ID 120) exactly as the existing code does, but for
P2P between two *clients* (not client↔SFU):

- **Discovery/presence.** A Station announces availability by broadcasting
  `START` (target 0 = channel) with a small JSON payload: `{role:"station",
  monitors:[...], busy_controller: <session|null>, caps:[...]}`. Clients cache
  per-session presence (mirrors the existing `broadcastingSessions` idea).
  `STOP` clears it. Late joiners: Station re-announces on channel-member-added
  (existing pattern) **[CONFIRM]** or Clients actively probe on join.
- **Offer/answer.** Because the SFU assumption ("both sides offer, server
  answers") no longer holds, we define roles: **Client (viewer) is the
  offerer**, **Station is the answerer** (Station adds the video track in its
  answer; Client uses `recvonly` video + the data channels it created). This
  keeps the Station passive until a viewer asks, and supports N viewers cleanly.
- **ICE.** `RTCConfiguration.ice_servers` = STUN (public) + TURN (yours).
  Trickle ICE over `ICE_CANDIDATE` signals (JSON), with the pre-remote-desc
  candidate queue the existing code already demonstrates. `serverId` pinning
  carried over for multi-connection correctness.
- **STUN** for server-reflexive candidates / hole-punching (~80–85% success).
- **TURN** relay fallback for symmetric/port-restricted NAT (see §6).
- Signaling payloads are versioned (`v` field) so Client/Station can detect
  incompatible peers and refuse gracefully.

### 3.6 Rendering approach **[CONFIRM]**

Two viable render targets for the Client's screen pane:
- **(A) WebView `<video>` + browser WebRTC** for *decode/render only*, while
  capture/encode/input remain native. Simplest render path, GPU-accelerated by
  WebView2, reuses existing `<video>` patterns. But mixing native `webrtc-rs`
  (Station) with browser WebRTC (Client) is still standards-compliant P2P.
- **(B) Fully native**: `webrtc-rs` on both ends, Rust decoder, render to a GPU
  texture surfaced in the UI (egui/iced per your diagram, or a Tauri custom
  surface). Lowest latency and full control; most work; trickier Tauri
  integration.

Recommendation: start **(B) native both ends** to honor "lowest latency" and
the P2P/webrtc-rs mandate, but prototype **(A)** first as a de-risking spike if
native render integration proves heavy. **[CONFIRM]** which to commit to for v1.

---

## 4. Control arbitration (cooperative, decision #7)

State machine lives in `rr-session`, authoritative **on the Station** (single
writer avoids split-brain). Clients render its broadcast state.

States of the control token:
- `Free` — nobody controls. First viewer to request is granted immediately
  (requirement: auto-grant when nobody has it).
- `Held(session)` — one controller. Others see the holder's name.
- `HandoffPending(from, to)` — a request is awaiting the current holder's
  approval (cooperative; no forcible takeover).

Events (over `rr-control` + presence):
- `RequestControl{from}` → if `Free`: grant, `Held(from)`, broadcast
  `ControlGranted{from}`. If `Held(h)`: enter `HandoffPending(h, from)`,
  notify `h` with `ControlRequested{from}`.
- `ApproveHandoff{to}` (only from current holder) → `Held(to)`, broadcast.
- `DenyHandoff{to}` (holder) → back to `Held(h)`, notify requester.
- `ReleaseControl` (holder) → `Free`, broadcast; auto-grant to the oldest
  pending requester if any.
- Holder disconnects / leaves channel → `Free` (then auto-grant per queue).
- Timeouts: a `HandoffPending` auto-expires (configurable) back to `Held`.

Auto-connect behavior (requirement): on login, a Client scans channel members;
if a Station with video available is present, auto-create the viewer PC; if the
token is `Free`, auto-request control (→ immediate grant). If `Held`, show the
holder's name and a "Request control" button.

Edge cases to test: holder leaves mid-handoff; two simultaneous requests;
Station restart; viewer count changes; a viewer that is not the controller
sending input (must be rejected).

---

## 5. UI specification

### RemoteRadioClient (new layout)
- **Left 1/3**: tabbed **Channels** and **Members** (reuse existing channel/user
  components). Member rows show role badges: `Station`, `Controller`,
  `Viewing`. The current controller's name is always visible.
- **Right 2/3**: remote-desktop surface (the Station's screen). Overlays:
  connection/latency HUD, monitor selector, "Request control" / "Release"
  button, "You are viewing — <holder> is in control" banner, file-drop zone.
- **Chat**: demoted to a small collapsible panel/drawer (requirement: "very
  small part of the UI").
- Login flow unchanged (asks for Mumble server); after sync, auto-discovery runs.

### RemoteRadioStation (mostly unchanged + host controls)
- Keep the existing Fancy UI.
- Add a **Station panel**: enable/disable hosting, monitor selection, encoder
  settings (codec, max bitrate, FPS, HW toggle), viewer list with per-viewer
  stats, current controller + handoff approve/deny, clipboard/file-transfer
  permission toggles, TURN/STUN config, a "who can connect" policy
  **[CONFIRM]** (any channel member vs. allow-list).

---

## 6. TURN/STUN & deployment (decision #5 — Docker for your Linux Mumble servers)

- **STUN**: use public STUN by default (Google), configurable; your coturn also
  provides STUN.
- **TURN**: ship a **coturn Docker deployment** intended to run alongside each
  Linux Mumble server.
  - Deliverable: `deploy/turn/` with `docker-compose.yml`, a templated
    `turnserver.conf`, and a short runbook.
  - **Auth**: time-limited **REST/ephemeral credentials** (coturn
    `use-auth-secret`) rather than static long-term creds, so apps mint
    short-lived TURN credentials. **[CONFIRM]** who mints them — options:
    (a) a tiny credential endpoint co-deployed with coturn, or (b) derive from
    a shared secret the Station holds and passes to viewers over the (TLS)
    Mumble signaling channel. Recommend (a) for cleanliness; (b) avoids extra
    infra. This is a real security decision — flagging it.
  - **Ports/TLS**: UDP/TCP 3478, TLS 5349, relay port range; document firewall
    and the Cloudflare-tunnel implications on `big-linux`.
  - **Testing**: a compose profile that runs coturn + a NAT simulation so CI
    can exercise the relay path (see §8).

---

## 7. Security & privacy

- **Transport encryption**: WebRTC media/data are DTLS-SRTP encrypted
  end-to-end between peers by default. Signaling rides Mumble's TLS link.
- **Remote-control is dangerous by nature.** Mitigations:
  - Station-side **explicit opt-in** to host; nothing is shared until enabled.
  - Controller must be an authenticated member of the Station's Mumble channel;
    Station validates `sender_session` == granted controller before injecting
    any input.
  - Clipboard & file transfer are **off by default**, per-direction toggles,
    with size/type limits and path sandboxing for received files.
  - File transfers are hashed (integrity) and written to a quarantined dir.
  - DTLS fingerprints exchanged in SDP; **[CONFIRM]** whether to additionally
    pin peer identity to the Mumble user `hash` to prevent a malicious relay
    from substituting SDP (defense against a hostile signaling server).
  - Rate-limit input injection; clamp coordinates to monitor bounds.
- **TURN credentials** are short-lived (§6).
- No telemetry (keep the project's existing stance).
- `unsafe_code = "deny"` is workspace policy; native capture/HW-encoder FFI
  that needs `unsafe` must be isolated in a crate that locally overrides the
  lint with a documented justification. **[CONFIRM]** acceptable.

---

## 8. Test strategy (decision: "confirm everything with lots of tests")

Layered, mostly runnable in CI headless/loopback:

1. **Unit tests** per crate:
   - `rr-proto`: round-trip (ser/de), version negotiation, malformed input.
   - `rr-session`: the arbitration state machine — exhaustive event/state
     transitions, all edge cases in §4 (property-test the transition table).
   - `rr-codec`: encode→decode fidelity on synthetic frames, keyframe-on-demand,
     bitrate clamp.
   - `rr-input`: event serialization, coordinate clamping, DPI mapping;
     injection tested on a loopback/headless mock (not real OS input in CI).
   - `rr-transfer`: chunking, backpressure, hash verification, resume.
2. **Integration (two in-process peers)**: a `webrtc-rs` loopback harness that
   connects a Station and a Client in one test binary over localhost ICE —
   verifies full handshake, a video track flowing (synthetic capture source),
   control channel, handoff, clipboard, a file transfer. This is the backbone
   E2E and runs in CI without displays or external servers.
3. **Signaling-over-Mumble integration**: reuse the repo's existing
   docker-based Mumble test server (`mumble-protocol/docker-compose.test.yml`,
   `cargo test ... --test integration`) to assert `WebRtcSignal` round-trips
   between two real clients.
4. **NAT-traversal tests**: a Docker compose with namespaced "NAT" networks +
   coturn to force a relay path; assert ICE succeeds via `relay` candidates
   when direct is blocked (symmetric-NAT simulation).
5. **Capture backends**: xcap and the Windows DXGI/WGC path tested on a Windows
   runner (self-hosted / `windows-latest`) capturing a known test pattern.
6. **Latency/throughput benchmarks** (criterion + a glass-to-glass harness)
   tracked in `scripts/bench-history.csv` style, gated as non-blocking.
7. **Soak/fuzz**: long-running viewer churn (join/leave storms), handoff
   fuzzing, malformed-signal fuzzing (`cargo fuzz` on `rr-proto`).
8. **Manual acceptance checklist** per release (real multi-NAT, multi-monitor,
   HW encoder matrix). Full WCAG/assistive validation of the UI requires manual
   testing — noted, not auto-claimed.

CI additions: build all three apps (Android keeps the existing green Android
build; two desktop binaries cross-built via cargo-xwin + NSIS); run crate unit
tests + the loopback E2E on every PR; run the NAT/coturn + Mumble-docker suites
on a schedule/nightly.

---

## 9. Phased implementation plan (milestones, each independently testable)

- **P0 — Scaffolding.** Create `rr-core` and the two thin desktop binaries with
  distinct identities; rename Android product → RemoteRadioMumble; CI builds all
  three. *Exit:* three installers/APK build green; apps launch and connect to
  Mumble unchanged.
- **P1 — P2P transport skeleton.** `rr-webrtc` + `rr-proto`: Client offers,
  Station answers, over Mumble `WebRtcSignal`; STUN only; a single data channel
  echo. *Exit:* loopback E2E connects two peers and round-trips a message;
  real two-machine connect on same LAN.
- **P2 — Video path.** `rr-capture` (xcap baseline) + `rr-codec` (openh264 SW,
  latency preset) + render in Client. One viewer. *Exit:* glass-to-glass video
  on LAN; keyframe-on-demand; benchmark baseline.
- **P3 — Input control + arbitration.** `rr-input` (enigo sink + client
  capture) over `rr-control`; `rr-session` cooperative handoff; auto-connect &
  auto-grant. *Exit:* single controller drives the Station; handoff works;
  arbitration edge-case tests pass.
- **P4 — NAT traversal hardening.** TURN integration + `deploy/turn/` coturn
  Docker; ephemeral creds; NAT-sim test suite. *Exit:* relay path verified
  under simulated symmetric NAT; cross-internet two-machine test.
- **P5 — Multi-viewer.** N independent viewer PCs; per-peer ABR; viewer list +
  stats UI; uplink cap + warning. *Exit:* 3+ concurrent viewers, one
  controller; soak test green.
- **P6 — Clipboard + file transfer.** `rr-transfer` over `rr-clipboard` /
  `rr-files`; permission toggles; sandboxing. *Exit:* bidirectional clipboard;
  resumable file xfer with integrity checks.
- **P7 — Windows fast-path + HW encode (latency).** DXGI/WGC capture backend;
  optional NVENC/AMF/QSV. *Exit:* measured latency/CPU improvement vs P2
  baseline; backend auto-select.
- **P8 — UI polish, security review, docs, acceptance.** Final Client 1/3–2/3
  layout, Station panel, demoted chat; threat-model review; manual acceptance
  matrix.

---

## 10. Resolved decisions (v1.0 — locked)

1. **Shared-code refactor**: **EXTRACT** `mumble-tauri` reusable guts into
   `rr-core`; all three apps become thin binaries over it. *(Confirmed.)*
2. **Windows capture fast path**: ship **xcap baseline** for v1; add WGC/DXGI
   fast path in **P7**. *(Confirmed.)*
3. **Encoder**: **openh264 SW default**; HW backends (NVENC/AMF/QSV) optional,
   auto-detected, in **P7**. *(Confirmed.)*
4. **Primary codec**: **H.264** (openh264) primary, **VP8** fallback.
   *(Confirmed.)*
5. **Render path**: commit to **native Rust decode + GPU texture (B)** for
   lowest latency; a WebView `<video>` spike is allowed only as early de-risk.
   *(Confirmed.)*
6. **Video-pane toolkit**: **stay in the existing Tauri/React shell** to reuse
   the channels/members UI; render video to a native surface/texture rather
   than switching the whole app to egui/iced. *(Confirmed.)*
7. **TURN credential minting**: **co-deployed REST endpoint** issuing
   time-limited coturn ephemeral credentials. *(Confirmed.)*
8. **Presence refresh**: **both** — Station re-announces on member-join **and**
   Clients probe on join. *(Confirmed.)*
9. **Connect policy**: **any authenticated member of the Station's Mumble
   channel may view and request control.** No allow-list in v1. *(Confirmed.)*
10. **Identity pinning**: **yes** — bind the peer's DTLS fingerprint to the
    Mumble user `hash` so a hostile signaling server cannot substitute SDP.
    *(Confirmed.)*
11. **`unsafe` isolation**: **yes** — `unsafe` permitted only in
    capture/HW-FFI crates via a locally-scoped, documented lint override; the
    rest of the workspace keeps `unsafe_code = "deny"`. *(Confirmed.)*
12. **Audio**: **none over WebRTC**; Mumble handles voice. *(Confirmed.)*

---

## 11. Non-goals (v1)

- No server-side SFU / large-audience broadcast.
- No audio over WebRTC.
- No mobile remote-desktop host/controller (Android stays a plain Mumble
  client, renamed).
- No cross-platform host beyond Windows for the Station in v1 (capture/input
  are Windows-first; the trait design leaves Linux/macOS open later).
```
