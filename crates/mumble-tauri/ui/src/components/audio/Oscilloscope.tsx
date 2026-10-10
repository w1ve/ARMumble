import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import styles from "./Oscilloscope.module.css";

/**
 * Live waveform ("oscilloscope") display fed by the Rust audio pipeline.
 *
 * The backend taps the real capture (input) and mixer (output) chains and
 * emits small decimated sample blocks over a Tauri event. One trace is drawn
 * for mono; two stacked traces for stereo (L / R). The tap is reference
 * counted on the backend via `scope_subscribe` / `scope_unsubscribe`, so it
 * costs nothing when no scope is mounted.
 *
 * NOTE: The Rust taps + events are implemented in a later step. Until then
 * this renders a flat idle baseline and simply wires up the subscription
 * lifecycle, so the UI builds and lays out correctly.
 */

/** Which pipeline the scope is reading from. */
export type ScopeSource = "input" | "output";

/** Payload emitted by the backend `scope-data` event. */
interface ScopeFrame {
  /** "input" or "output". */
  readonly source: ScopeSource;
  /** Number of channels represented in `samples` (1 = mono, 2 = stereo). */
  readonly channels: number;
  /**
   * Interleaved, decimated samples in [-1, 1]. For stereo the samples are
   * interleaved L, R, L, R, ...; for mono it is a flat list.
   */
  readonly samples: number[];
}

const SCOPE_EVENT = "scope-data";
const WAVE_COLOR = "#ff3b30"; // red

interface OscilloscopeProps {
  readonly source: ScopeSource;
  /** Optional fixed height in px (defaults to CSS-driven height). */
  readonly height?: number;
  readonly className?: string;
  /** Accessible label. */
  readonly ariaLabel?: string;
}

export function Oscilloscope({ source, height, className, ariaLabel }: Readonly<OscilloscopeProps>) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  // Latest frame received for this source; drawn on the animation loop.
  const frameRef = useRef<ScopeFrame | null>(null);

  // Subscribe to the backend tap for this source (ref-counted) and listen
  // for sample frames. Unsubscribe on unmount so the tap can go idle.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;

    invoke("scope_subscribe", { source }).catch(() => {
      // Command may not exist yet during early builds; ignore.
    });

    listen<ScopeFrame>(SCOPE_EVENT, (event) => {
      if (disposed) return;
      if (event.payload.source !== source) return;
      frameRef.current = event.payload;
    })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        // ignore
      });

    return () => {
      disposed = true;
      if (unlisten) unlisten();
      invoke("scope_unsubscribe", { source }).catch(() => {
        // ignore
      });
    };
  }, [source]);

  // Draw loop.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    let raf = 0;

    const draw = () => {
      raf = requestAnimationFrame(draw);
      const ctx = canvas.getContext("2d");
      if (!ctx) return;

      // Match the backing store to the element's CSS size for crisp lines.
      const rect = canvas.getBoundingClientRect();
      const dpr = window.devicePixelRatio || 1;
      const w = Math.max(1, Math.round(rect.width * dpr));
      const h = Math.max(1, Math.round(rect.height * dpr));
      if (canvas.width !== w) canvas.width = w;
      if (canvas.height !== h) canvas.height = h;

      ctx.clearRect(0, 0, w, h);

      const frame = frameRef.current;
      const channels = frame && frame.channels >= 2 ? 2 : 1;

      ctx.lineWidth = Math.max(1, dpr);
      ctx.strokeStyle = WAVE_COLOR;
      ctx.lineJoin = "round";

      for (let ch = 0; ch < channels; ch++) {
        // Each channel gets a horizontal band of the canvas.
        const bandTop = (h / channels) * ch;
        const bandH = h / channels;
        const mid = bandTop + bandH / 2;

        ctx.beginPath();
        if (!frame || frame.samples.length === 0) {
          // Idle: flat baseline.
          ctx.moveTo(0, mid);
          ctx.lineTo(w, mid);
        } else {
          const total = frame.samples.length;
          const count = channels === 2 ? Math.floor(total / 2) : total;
          if (count <= 1) {
            ctx.moveTo(0, mid);
            ctx.lineTo(w, mid);
          } else {
            for (let i = 0; i < count; i++) {
              const idx = channels === 2 ? i * 2 + ch : i;
              const sample = frame.samples[idx] ?? 0;
              const x = (i / (count - 1)) * w;
              const y = mid - sample * (bandH / 2) * 0.9;
              if (i === 0) ctx.moveTo(x, y);
              else ctx.lineTo(x, y);
            }
          }
        }
        ctx.stroke();
      }
    };

    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, []);

  return (
    <canvas
      ref={canvasRef}
      className={`${styles.scope} ${className ?? ""}`.trim()}
      style={height != null ? { height } : undefined}
      role="img"
      aria-label={ariaLabel ?? "audio waveform"}
    />
  );
}

/** Convenience wrapper for the transmit (input) scope. */
export function InputOscilloscope(props: Readonly<Omit<OscilloscopeProps, "source">>) {
  return <Oscilloscope source="input" {...props} />;
}

/** Convenience wrapper for the output scope. */
export function OutputOscilloscope(props: Readonly<Omit<OscilloscopeProps, "source">>) {
  return <Oscilloscope source="output" {...props} />;
}
