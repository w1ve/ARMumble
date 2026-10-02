# P2 latency baseline (pre-optimisation)

Measured with `cargo run -p rr-codec --example capture_probe --features real-capture`
(release build, cross-compiled for `x86_64-pc-windows-msvc`).

## Hardware / configuration

| Item | Value |
|---|---|
| Monitors | `id=65639` 1920x1200 (primary, used), `id=131073` V505-G9 3840x2160 |
| Capture backend | `xcap` 0.9.8 (`XcapCapturer`) — full-surface `capture_image()` |
| Encoder | `openh264` 0.9.8, `UsageType::ScreenContentRealTime`, `RateControlMode::Bitrate`, 8 Mbit/s target, `intra_frame_period = 0` |
| Frame size | 1920x1200 RGBA8, stride 7680 (packed) |
| Build | release, `nasm` available at build time |

## Results

### Single-frame
| Stage | Time |
|---|---|
| Capture (first) | 46.2 ms |
| Encode IDR | 39.2 ms (142,325 bytes) |
| Encode delta | 21.8 ms (3,533 bytes) |
| Decode | 17.3 ms |

### Sustained (60 frames)
| Stage | Average | Worst |
|---|---|---|
| Capture | **33.3 ms** | 38.6 ms |
| Encode | **16.6 ms** | 36.7 ms |
| **capture + encode** | **49.9 ms** | — |
| Throughput | **19.9 fps** | — |
| Encoded bitrate | 0.03 Mbit/s (near-static desktop) | — |

## Findings

**1. Capture is the bottleneck, not encoding.** Capture averages 33.3 ms versus
16.6 ms for encode — capture is *twice* the cost. `xcap`'s `capture_image()`
does a full-surface copy of the whole 1920x1200 framebuffer every call with no
damage-region tracking, so the cost is paid even when almost nothing changed
(note the 3.5 KB delta frames: the screen was nearly static, yet capture still
cost full price).

**2. We are over the 30 fps budget.** A 30 fps frame budget is 33.3 ms, but
capture + encode alone is 49.9 ms, giving ~20 fps before any network or decode
cost. That is usable but not the "lowest latency possible" target.

**3. This validates the P7 plan rather than contradicting it.** The spec already
schedules a Windows DXGI Desktop Duplication / `WGC` capture backend for P7
specifically for this reason. The numbers now quantify the expected win:
eliminating the full-surface copy should move capture from ~33 ms toward the
single-digit range, which alone would bring capture + encode under the 30 fps
budget without touching the encoder.

**4. Encoder tuning is already effective.** 3.5 KB delta frames against a
142 KB IDR shows the screen-content preset plus disabled periodic keyframes are
doing their job — bandwidth is dominated by actual change, not by codec
overhead. Encode worst-case (36.7 ms) is well above average (16.6 ms), which is
the IDR being re-sent; expected, and why keyframes are on-demand only.

**5. `OpenH264` auto-disabled two settings for screen content** — fixed. The
first probe run logged:
```
Warning:ParamValidation(), AdaptiveQuant(1) is not supported yet for screen content, auto turned off
Warning:ParamValidation(), BackgroundDetection(1) is not supported yet for screen content, auto turned off
```
Both default to on in `EncoderConfig` and are silently ignored in
`ScreenContentRealTime` mode. `Encoder::new` now sets
`adaptive_quantization(false)` and `background_detection(false)` explicitly, so
the configuration describes what actually happens and the warnings are gone
(confirmed by a second probe run).

## Repeatability

A second run after the encoder fix, same machine:

| Stage | Run 1 avg | Run 2 avg |
|---|---|---|
| Capture | 33.3 ms | 32.6 ms |
| Encode | 16.6 ms | 21.0 ms |
| capture + encode | 49.9 ms | 53.6 ms |
| Throughput | 19.9 fps | 18.5 fps |

Capture is stable across runs; encode varies more with whatever is on screen
(it is content-dependent). The conclusion is unchanged: **capture dominates**.

## Where the capture time actually goes

Measured with `cargo run --release -p rr-capture --example capture_bench
--features screen-capture`, 30 samples per stage, same machine:

| Stage | Mean | Min | Max |
|---|---|---|---|
| `Monitor::all()` (enumeration) | **2.64 µs** | 1.70 µs | 23.70 µs |
| `friendly_name()` (`QueryDisplayConfig`) | 30.30 µs | 21.70 µs | 110.30 µs |
| `id()` | 26 ns | — | 100 ns |
| **`capture_image()` (handle cached)** | **33.60 ms** | 28.52 ms | 49.02 ms |
| enumerate + capture (current path) | 33.98 ms | 20.90 ms | 55.01 ms |

### Conclusion: it is `capture_image()`, and it cannot be tuned away

An initial hypothesis — that re-enumerating monitors every frame (which
`XcapCapturer::grab` does, via `resolve()`) was the cost — is **wrong**.
Enumeration is 2.64 µs, about **1%** of the frame path. Caching the monitor
handle is tidy but worth nothing measurable.

The cost is `capture_image()` itself:

- 1920x1200 RGBA = **8.8 MiB** per frame.
- A raw memcpy of 8.8 MiB at 10 GB/s would take **~0.92 ms**.
- We measure **33.60 ms** — roughly **36x** the memory-bandwidth floor.

That gap is API overhead, not copying: `xcap` uses a GDI surface copy
(`BitBlt` + `GetDIBits`) which forces a slow GPU→CPU readback of the entire
framebuffer, with no damage-region tracking, every single call.

**Consequence: the current backend can never reach 30 fps.** Capture alone
(33.6 ms) already exceeds the whole 33.3 ms frame budget, before encoding,
networking or decoding. Since "lowest latency possible" is the project's
primary goal, this is a structural limit rather than a tuning problem.

## What P7 must beat

Capture average **33.6 ms** and combined capture + encode **49.9 ms** at
1920x1200. Re-run both the probe and the benchmark after the new backend lands.

### Recommended replacement

| Option | Notes |
|---|---|
| **`windows-capture` 2.0.1** (Windows.Graphics.Capture) | Actively maintained (1.8M downloads, Aug 2026). Push model: implement `GraphicsCaptureApiHandler::on_frame_arrived`, `frame.buffer()` for raw bytes. **Delivers frames only when the screen changes**, so a static desktop costs nothing. Works cross-GPU. No `unsafe` of our own. |
| Hand-rolled **DXGI Desktop Duplication** via `windows` 0.62 | Explicit dirty/move rectangles and the lowest ceiling; Microsoft's own guidance recommends DXGI for "ultra-low-latency, full-display mirroring (e.g. VDI or remote desktop)". Costs `unsafe` FFI we maintain, and requires running on the same GPU as the display. |
| Stale alternatives | `dxgcap` (2020), `scrap` (2018), `captrs` (2020) — all unmaintained; not viable. |

Recommendation: try **`windows-capture`** first. It is the far smaller change,
needs no `unsafe` from us, and its change-driven delivery directly removes the
"pay full price for a static screen" behaviour that dominates the current
numbers. Fall back to raw DXGI only if WGC does not hit the target.

Both are pull-vs-push mismatches with the current `ScreenCapturer::grab()`
trait; bridge with a single-slot "latest frame" channel, which is the right
semantic for remote desktop anyway (you always want the newest frame, never a
backlog).

## Caveats

- Multi-monitor is enumerated but only the primary was exercised end to end.
- The primary monitor reports as `Unknown Monitor 65639`; `xcap` could not
  resolve a friendly name for it. Cosmetic, but the Station UI should fall back
  gracefully rather than showing that string to users.
- Timings are from one machine; treat them as that machine's baseline.
