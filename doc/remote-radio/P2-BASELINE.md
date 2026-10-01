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

## What P7 must beat

Capture average **33.3 ms** and combined capture + encode **49.9 ms** at
1920x1200. Re-run this same probe after the DXGI/`WGC` backend lands and compare.

## Caveats

- Multi-monitor is enumerated but only the primary was exercised end to end.
- The primary monitor reports as `Unknown Monitor 65639`; `xcap` could not
  resolve a friendly name for it. Cosmetic, but the Station UI should fall back
  gracefully rather than showing that string to users.
- Timings are from one machine; treat them as that machine's baseline.
