# P2 latency baseline, and the capture backend that replaced it

**Outcome: capture went from ~33 ms to ~3.9 ms per frame (roughly 8x), taking
capture + encode from ~50 ms to ~22 ms at 1920x1200 — inside the 33.3 ms budget
for 30 fps.**

**Caveat that matters more than the headline: at 3840x2160 we get 7.5–13.8 fps,
and software encode is 78% of that frame time.** 1080p-class displays are
comfortable; 4K needs hardware encoding. See
[Resolution is the thing that decides whether this works](#resolution-is-the-thing-that-decides-whether-this-works).

**And 4K *is* reachable on this class of machine**: the same laptop's Quick Sync
encoder does 3840x2160 at **70–85 fps**, 4–8x faster than software. See
[Is 4K achievable on this class of machine?](#is-4k-achievable-on-this-class-of-machine-yes-with-hardware-encoding)

Jump to [the result](#measured-result) for the capture numbers, or read on for how
the original baseline was measured and why the first diagnosis was wrong.

The sections below are in the order they happened: the `xcap` baseline, the
attribution that found the real cost, the DXGI replacement and its measurements,
and a build-environment trap that briefly made the encoder look 10x slower than
it is.

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

**Consequence: the `xcap` backend can never reach 30 fps.** Capture alone
(33.6 ms) already exceeds the whole 33.3 ms frame budget, before encoding,
networking or decoding. Since "lowest latency possible" is the project's
primary goal, this is a structural limit rather than a tuning problem — which is
why the backend work was pulled forward from P7 instead of tuning around it.

## The replacement: DXGI Desktop Duplication

Implemented as `DxgiCapturer` behind the existing `ScreenCapturer` trait, using
[`windows-capture` 2.0.1](https://crates.io/crates/windows-capture) (MIT,
published 2026-08-08). That crate is best known for its Windows.Graphics.Capture
wrapper, but it also ships `dxgi_duplication_api`, a **pull-based** Desktop
Duplication wrapper — `acquire_next_frame(timeout_ms)` — which fits
`ScreenCapturer::grab()` directly. No push-to-pull bridge was needed, and no
`unsafe` of our own (`rr-capture` keeps `#![forbid(unsafe_code)]`).

Options considered and rejected:

| Option | Why not |
|---|---|
| `windows-capture`'s WGC handler API | Push model; would need a latest-frame channel to fit the trait, for no measured gain over the duplication wrapper in the same crate. |
| Hand-rolled DXGI on `windows` 0.62 | Same API, but `unsafe` FFI we would own and maintain. Only worth it if the wrapper fell short, and it did not. |
| `dxgcap`, `scrap`, `captrs` | Unmaintained since 2018–2020. |

### Measured result

Same machine, same monitor, release build, 1920x1200. Figures are the typical
value across four runs, with the full observed range where it matters:

| | `xcap` (GDI) | `DxgiCapturer` | Change |
|---|---|---|---|
| Capture work per frame | **33.0 ms** | **3.9 ms** | **~8x faster** |
| Range across runs | 32.6 – 33.6 ms | 3.7 – 5.3 ms | 6.2x worst observed |
| Capture worst single frame | 46.8 ms | 8.7 ms | |
| vs. 0.92 ms memcpy floor | 36x | 4.2x | |
| Encode (unchanged) | 16.9 – 17.2 ms | 17.8 – 21.5 ms | |
| capture + encode | **49.8 – 50.0 ms** | **21.7 – 26.8 ms** | |
| Sustainable frame rate | 20.0 – 20.1 fps | **37 – 46 fps** | inside the 33.3 ms budget |
| Cost of a static screen | full price, every frame | nothing | |

`xcap` is remarkably stable at ~33 ms because it does the same full readback
regardless of machine state. `DxgiCapturer` is cheap enough that *background
load* is visible in it: the 3.7 ms figures come from runs starting at ~20% CPU,
the 5.3 ms figure from a run starting at ~49%. That is contention from other
processes, not CPU throttling — see the caveats. Even the worst observed case
leaves room inside the 30 fps budget.

Benchmark detail, 30 delivered frames (`capture_bench`), two consecutive runs:

| Stage | Mean | Min | Max |
|---|---|---|---|
| `grab()` work (copy + map, wait excluded) | **3.69 / 3.90 ms** | 3.30 / 3.40 ms | 4.14 / 4.71 ms |
| blocked waiting for a change | 31.66 / 27.00 ms | 3.30 / 2.46 ms | 109.47 / 97.11 ms |
| session setup (first grab only) | 69.76 / 70.41 ms | — | — |

The per-frame spread is tight because the work is fixed: a GPU-side
`CopyResource` into a reused staging texture, a map, and one 8.8 MiB copy out.
Session setup — the Direct3D 11 device, the DXGI output lookup and the staging
texture — is paid once and excluded.

The blocked-wait column is not a cost. It is the Station idling while the user's
screen holds still; across these runs 178 of 200 calls found no change at all,
every one of which `xcap` would have charged a full 33 ms for.

### Measuring a change-driven backend honestly

Desktop Duplication blocks until the desktop changes, so a stopwatch around
`grab()` on a quiet screen measures how still the screen was, not what capture
costs. `ScreenCapturer::last_wait()` reports the blocked time and the probe
subtracts it. Every figure above is work, not waiting.

The first attempt at this used a zero acquire timeout so successful calls had no
wait to subtract. That worked, but it busy-spins a core, and the spinning
inflated the encode timings measured in the same loop. The blocking-plus-subtract
approach measures the same thing without perturbing anything else.

**Capture is no longer the bottleneck — encode is**, at 17–22 ms against
capture's ~3.9 ms. See the next section for how far that goes.

## Resolution is the thing that decides whether this works

The 1920x1200 figures above are comfortable. The same probe against this
machine's second display, a 3840x2160 panel (3.6x the pixels), is not:

| | 1920x1200 (2.3 Mpx) | 3840x2160 (8.3 Mpx) |
|---|---|---|
| `DxgiCapturer` capture work | 3.9 ms | 16–18 ms |
| `OpenH264` encode, real desktop content | 17–21 ms | **56–114 ms** |
| capture + encode | 22–27 ms | 72–133 ms |
| Sustainable | **37–46 fps** | **7.5–13.8 fps** |

Capture scales about as expected with pixel count. **Encode does not** — it is
78% of the 4K frame time and the reason 4K misses the 30 fps budget by 2–4x.

### Configuration is not the lever

`cargo run --release -p rr-codec --example encoder_tune --features dxgi-capture`
sweeps `complexity` (`Low`/`Medium`/`High`) and `num_threads` (1/auto/2/4/8)
against an identical frame sequence, interleaved, best-of-3:

| | 1920x1200 | 3840x2160 |
|---|---|---|
| Spread across all 10 configurations | 8.32 – 8.83 ms | 14.70 – 15.40 ms |
| Shipped configuration | 8.63 ms | 15.13 ms |

A 6% spread at 1080p and 5% at 4K. **Neither knob is worth setting**, and
`OpenH264`'s internal threading does not parallelise this workload at all —
`threads=1` and `threads=8` are indistinguishable. The shipped configuration is
already at the fast end.

### Encoding cost depends on content far more than on configuration

Note the gap between the sweep and the probe at the same resolution: 15 ms versus
56–114 ms at 4K. Same encoder, same settings. The difference is the *content*:

- The sweep paints a large high-entropy region. Motion estimation finds no
  matches and gives up quickly.
- The probe encodes a real, mostly-static desktop. Motion estimation has endless
  plausible matches to evaluate across 8.3 Mpx and searches exhaustively.

**`OpenH264` is slowest on exactly the content a remote desktop produces.** Any
encoder benchmark for this project that does not use real screen captures is
measuring the wrong thing, and any quoted encode figure is meaningless without
saying what it encoded.

### Measurement noise, and how to get past it

An earlier non-interleaved version of `encoder_tune` measured each configuration
once, in order. Identical configurations differed by up to **4x** between runs
(the shipped baseline measured 8.3 ms, 11.5 ms, 14.0 ms and 36 ms on different
runs), and position within the run predicted the result better than the setting
did — background processes stealing cores on a 4-core machine. The fix was to
interleave rounds and report the minimum, which collapsed the spread to the 6%
above. Mean-of-one-pass benchmarking on a busy desktop is not measurement.

## Is 4K achievable on this class of machine? Yes, with hardware encoding

Measured on the same i7-1165G7 / Iris Xe laptop, 3840x2160 H.264:

| Encoder | Per frame | Throughput | Media engine busy |
|---|---|---|---|
| `OpenH264`, real desktop content | 56–114 ms | 7.5–13.8 fps | n/a (CPU) |
| `libx264 -preset veryfast` | 23.9 ms | 41.9 fps | n/a (CPU) |
| **Quick Sync (`h264_qsv`)** | **11.8–14.1 ms** | **70–85 fps** | **62.7%** |

The Quick Sync figures *include* generating 4K source frames on the CPU in the
same process, so they are pessimistic: real use would feed captured frames
instead. **4K at 30 fps is comfortably achievable on this hardware** — the
fixed-function encoder is 4–8x faster than software and leaves most of the
33.3 ms budget for capture and transport.

This was measured with `ffmpeg -c:v h264_qsv`, used purely as an instrument.
ffmpeg is **not** a dependency of this project and nothing from it ships, so its
GPL licence does not touch the codebase. `h264_qsv` has no software fallback: it
either runs on Intel's media engine or fails to initialise, and it ran.

### Two traps when verifying hardware encoding on Windows

1. **Intel's media engine reports under `engtype_videodecode`, not
   `engtype_VideoEncode`.** One block does both. Watching
   `\GPU Engine(*engtype_VideoEncode)\Utilization Percentage` shows **0%** while
   Quick Sync is fully busy, which looks exactly like "hardware is not being
   used".
2. **Media Foundation falls back to a software MFT silently.** The
   `hwencode_probe` example reaches ~51 fps at 4K through
   `windows-capture`'s `VideoEncoder`, which *looks* like a hardware win against
   `OpenH264` — but the media engine stays at 0% and the CPU runs 37–73%. The
   cause is feeding it a CPU buffer: without a D3D device manager on the sink
   writer, a hardware MFT is never eligible. Any hardware-encode work must assert
   the hardware is actually in use, not infer it from being faster than the thing
   it replaced.

### What reaching it from our own code requires

- The **Media Foundation H.264 MFT** driven directly, with a D3D device manager,
  producing an H.264 elementary stream. One API covers Quick Sync, NVENC and
  AMD VCN, which is what makes it the right choice for varied hardware.
- `unsafe` COM FFI that we own. The workspace sets `unsafe_code = deny`, so this
  needs a deliberate, narrowly scoped exception — a project-convention decision,
  not an implementation detail.
- Capability detection and an `OpenH264` fallback, for machines with no hardware
  encoder (VMs, older hardware).
- Ideally **zero-copy**: feed the DXGI texture straight to the encoder so the
  frame never leaves the GPU, which also removes the 16–18 ms capture readback at
  4K. That changes the capture/codec boundary, since `ScreenCapturer::grab`
  currently hands back CPU bytes. Hardware encode from a CPU buffer would be
  ~14 ms encode + ~18 ms capture = ~32 ms, only just inside budget; zero-copy is
  what creates real headroom.

## Bug found by measuring at 4K

The 4K sweep failed outright with:

```text
Error: Backend("Invalid staging texture: format must match the frame")
```

`Session::open` sized the reusable staging texture from
`duplication_desc().ModeDesc.Format`, assuming the texture handed back by
`acquire_next_frame` would match. On the 4K display it did not, and
`buffer_with` rejected it, killing the capture. The risk was noted in the
original implementation and the fallback deliberately skipped; that was the wrong
call.

`grab_once` now compares the staging texture's description against the acquired
frame's and rebuilds when they disagree, trusting the frame rather than the mode
description. Cost is one `CreateTexture2D` on the rare frame that changes shape.
This would have shown up the first time anyone shared a 4K monitor.

## Correction: NASM, and a ten-fold encoder regression

The first run of the new probe reported encode at **139–164 ms** rather than the
~17 ms recorded here, on *both* backends. That was not the new code. The local
Windows build had no NASM on `PATH`, and `openh264-sys2` reports that in its
build log and carries on:

```text
Failed to compile NASM files, not using any assembly.
```

OpenH264's hand-written SIMD kernels were silently dropped and the pure-C
fallback used instead, which is roughly **10x slower**. The original baseline was
built on the Linux builder, where NASM 3.02 is present, so it was unaffected.

Installing NASM 3.02 locally (matching the Linux builder exactly) and rebuilding
reproduced the recorded baseline to within noise — `xcap` capture 32.6 ms vs
33.3 ms, encode 17.2 ms vs 16.6 ms — which is also a useful cross-check that the
two builders agree.

Two lessons worth keeping:

- A missing build tool here degrades performance **silently**. There is no error,
  only a log line nobody reads. Any machine that benchmarks this project needs
  NASM on `PATH`, and `E:\devtools\cargo-msvc.cmd` now fails loudly without it.
- Always reproduce the baseline before trusting a comparison against it.

## Caveats

- Multi-monitor is enumerated by both backends but only the primary was exercised
  end to end.
- Monitor ids differ between backends by design: `xcap` exposes Windows monitor
  handles (65639, 131073) while `DxgiCapturer` exposes the one-based display
  number from `\\.\DISPLAYn` (1, 2). The latter is stable for a given display
  configuration and readable in logs. A persisted monitor selection is therefore
  backend-specific.
- `DxgiCapturer` returned an empty friendly name for the primary monitor, where
  `xcap` returned the placeholder `Unknown Monitor 65639`. Neither is useful; the
  Station UI needs its own fallback label.
- Test machine: Lenovo `20XXX16500` laptop, **i7-1165G7** (Tiger Lake mobile,
  4 cores / 8 threads), **32 GB LPDDR4X-4267**, Windows 11. The CPU ran **at or
  slightly above its 2.80 GHz nominal clock** throughout (84–112% of nominal on
  the `% Processor Performance` counter, ≈2.4–3.1 GHz), so these are honest
  figures for a 4-core mobile i7 — not a throttled machine.

  An earlier revision of this document claimed the CPU was pinned at 1201 MHz
  and therefore power-limited. **That was wrong.**
  `Win32_Processor.CurrentClockSpeed` is a cached WMI value that is frozen at
  1201 MHz on this machine and never moves, under any load. Use
  `Win32_PerfFormattedData_Counters_ProcessorInformation.PercentProcessorPerformance`
  instead, which is live and tracks boost.

  What a faster machine would change: **encode**, which is CPU-bound and scales
  with cores. Capture is a GPU copy plus one 8.8 MiB memcpy, so it has much less
  to gain.
- The 10 GB/s used for the memcpy floor is deliberately conservative. This
  machine's LPDDR4X-4267 is well above that, so the real floor is below 0.92 ms
  and `DxgiCapturer` sits further from memory-bound than the stated 4.2x — i.e.
  there is probably some headroom left, though not enough to matter while encode
  costs 4x as much.
- A delta frame over a near-static screen encoded to **0 bytes**. That is correct
  — nothing changed enough to code — but the transport must treat an empty
  bitstream as "send nothing" rather than framing an empty packet.
