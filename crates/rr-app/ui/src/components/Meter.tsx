import { useEffect, useRef, useState } from "react";
import { useApp } from "../store";
import { meterPosition } from "../util";

/** Peak-hold fall speed, meter widths per second. */
const PEAK_FALL_PER_SEC = 0.35;
/** Bar release speed (attack is instant), meter widths per second. */
const BAR_FALL_PER_SEC = 1.5;
/** How long the clip lamp stays lit after a clip, ms. */
const CLIP_HOLD_MS = 2000;

interface Props {
  source: "input" | "output";
  label: string;
}

/**
 * Stereo VU meter: green / yellow / red zones on a -60..0 dBFS scale, a bar
 * per channel with instant attack and smooth release, a peak-hold tick, and a
 * clip lamp. Fed by the 25 Hz `levels` event and animated at display rate.
 */
export function Meter({ source, label }: Props) {
  const fills = [useRef<HTMLDivElement>(null), useRef<HTMLDivElement>(null)];
  const ticks = [useRef<HTMLDivElement>(null), useRef<HTMLDivElement>(null)];
  const [clipLit, setClipLit] = useState(false);
  const live = useApp((s) => s.levels.live);

  useEffect(() => {
    const bar = [0, 0];
    const peak = [0, 0];
    let lastClips = -1;
    let clipUntil = 0;
    let last = performance.now();
    let raf = 0;

    const draw = () => {
      raf = requestAnimationFrame(draw);
      const now = performance.now();
      const dt = (now - last) / 1000;
      last = now;
      const lv = useApp.getState().levels;
      const target = source === "input" ? [lv.in_l, lv.in_r] : [lv.out_l, lv.out_r];
      const clips = source === "input" ? lv.in_clips : lv.out_clips;

      for (let ch = 0; ch < 2; ch++) {
        const pos = meterPosition(target[ch]);
        bar[ch] = pos >= bar[ch] ? pos : Math.max(pos, bar[ch] - BAR_FALL_PER_SEC * dt);
        peak[ch] = pos >= peak[ch] ? pos : Math.max(0, peak[ch] - PEAK_FALL_PER_SEC * dt);
        const fill = fills[ch].current;
        const tick = ticks[ch].current;
        // The mask covers the unlit part, revealing the coloured zones beneath.
        if (fill) fill.style.left = `${bar[ch] * 100}%`;
        if (tick) {
          tick.style.left = `${peak[ch] * 100}%`;
          tick.style.opacity = peak[ch] > 0.01 ? "1" : "0";
        }
      }

      if (lastClips >= 0 && clips > lastClips) clipUntil = now + CLIP_HOLD_MS;
      lastClips = clips;
      const lit = now < clipUntil;
      setClipLit((was) => (was === lit ? was : lit));
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [source]);

  return (
    <div className={`vu${live ? "" : " idle"}`} role="img" aria-label={label}>
      <div className="vu-bars">
        {[0, 1].map((ch) => (
          <div className="vu-track" key={ch}>
            <div className="vu-mask" ref={fills[ch]} />
            <div className="vu-tick" ref={ticks[ch]} />
          </div>
        ))}
        <div className="vu-scale" aria-hidden>
          <span style={{ left: "0%" }}>-60</span>
          <span style={{ left: "50%" }}>-30</span>
          <span style={{ left: "70%" }}>-18</span>
          <span style={{ left: "90%" }}>-6</span>
          <span style={{ left: "100%" }}>0</span>
        </div>
      </div>
      <div className={`clip-lamp${clipLit ? " lit" : ""}`} title="CLIP" />
    </div>
  );
}
