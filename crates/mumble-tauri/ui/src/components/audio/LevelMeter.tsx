import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import styles from "./LevelMeter.module.css";

/**
 * Compact multi-color VU meter indicating audio level, fed by the Rust
 * pipeline tap (`scope-data`). Replaces the oscilloscope: it is purely a
 * level indicator — a green→yellow→red bar with a peak-hold marker — showing
 * that mic/radio audio is actually flowing. Just the bar, nothing else.
 *
 * Level is computed from the same decimated sample block the scope tap already
 * emits, so it needs no extra backend and works whenever a meter is mounted
 * (ref-counted via scope_subscribe / scope_unsubscribe).
 */

export type MeterSource = "input" | "output";

interface ScopeFrame {
  readonly source: MeterSource;
  readonly channels: number;
  readonly samples: number[];
}

const SCOPE_EVENT = "scope-data";
/** dB axis matching the Settings VuMeter. */
const VU_DB_MIN = -60;
const VU_DB_MAX = 0;
/** Peak-hold decay in %/sec. */
const PEAK_DECAY_PER_SEC = 60;
/** A level older than this (ms) decays to zero (audio stopped). */
const STALE_MS = 250;

function linearToPercent(amplitude: number): number {
  if (amplitude <= 0) return 0;
  const db = 20 * Math.log10(amplitude);
  const pct = ((db - VU_DB_MIN) / (VU_DB_MAX - VU_DB_MIN)) * 100;
  return Math.max(0, Math.min(100, pct));
}

/** RMS + peak of an interleaved sample block (all channels together). */
function levelOf(samples: number[]): { rms: number; peak: number } {
  if (samples.length === 0) return { rms: 0, peak: 0 };
  let sumSq = 0;
  let peak = 0;
  for (const s of samples) {
    sumSq += s * s;
    const a = Math.abs(s);
    if (a > peak) peak = a;
  }
  return { rms: Math.sqrt(sumSq / samples.length), peak };
}

interface LevelMeterProps {
  readonly source: MeterSource;
  readonly className?: string;
  readonly ariaLabel?: string;
}

export function LevelMeter({ source, className, ariaLabel }: Readonly<LevelMeterProps>) {
  const fillRef = useRef<HTMLDivElement>(null);
  const peakRef = useRef<HTMLDivElement>(null);
  // Latest measured level + when it arrived.
  const rmsRef = useRef(0);
  const peakRef2 = useRef(0);
  const atRef = useRef(0);
  const heldPeakPct = useRef(0);
  const lastDraw = useRef(performance.now());

  // Subscribe to the pipeline tap (ref-counted) and measure level per frame.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    const id = Math.floor(Math.random() * 0xffffffffff);

    invoke("scope_subscribe", { source, id }).catch(() => {});

    listen<ScopeFrame>(SCOPE_EVENT, (event) => {
      if (disposed || event.payload.source !== source) return;
      const { rms, peak } = levelOf(event.payload.samples);
      rmsRef.current = rms;
      peakRef2.current = peak;
      atRef.current = performance.now();
    })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {});

    return () => {
      disposed = true;
      if (unlisten) unlisten();
      invoke("scope_unsubscribe", { source, id }).catch(() => {});
    };
  }, [source]);

  // Draw loop: fill width + peak-hold marker.
  useEffect(() => {
    let raf = 0;
    const draw = () => {
      raf = requestAnimationFrame(draw);
      const now = performance.now();
      const dt = (now - lastDraw.current) / 1000;
      lastDraw.current = now;

      // Audio stopped -> collapse to zero.
      const stale = now - atRef.current > STALE_MS;
      const rms = stale ? 0 : rmsRef.current;
      const peak = stale ? 0 : peakRef2.current;

      const peakPct = linearToPercent(peak);
      heldPeakPct.current = Math.max(0, heldPeakPct.current - PEAK_DECAY_PER_SEC * dt);
      if (peakPct > heldPeakPct.current) heldPeakPct.current = peakPct;

      if (fillRef.current) fillRef.current.style.width = `${linearToPercent(rms)}%`;
      if (peakRef.current) peakRef.current.style.left = `${heldPeakPct.current}%`;
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, []);

  return (
    <div
      className={`${styles.meter} ${className ?? ""}`.trim()}
      role="img"
      aria-label={ariaLabel ?? "audio level"}
    >
      <div className={styles.track}>
        <div className={styles.fill} ref={fillRef} />
        <div className={styles.peak} ref={peakRef} />
      </div>
    </div>
  );
}

export function InputLevelMeter(props: Readonly<Omit<LevelMeterProps, "source">>) {
  return <LevelMeter source="input" {...props} />;
}

export function OutputLevelMeter(props: Readonly<Omit<LevelMeterProps, "source">>) {
  return <LevelMeter source="output" {...props} />;
}
