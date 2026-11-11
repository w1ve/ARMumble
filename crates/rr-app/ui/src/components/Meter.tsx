import { useEffect, useRef, useState } from "react";
import { meterPosition } from "../util";

interface Props {
  left: number;
  right: number;
  clips: number;
  label: string;
}

/** Stereo level meter with peak hold and a clip lamp that stays lit for two seconds. */
export function Meter({ left, right, clips, label }: Props) {
  const [lit, setLit] = useState(false);
  const last = useRef(clips);
  const hold = useRef({ l: 0, r: 0 });

  useEffect(() => {
    if (clips > last.current) {
      setLit(true);
      const t = setTimeout(() => setLit(false), 2000);
      last.current = clips;
      return () => clearTimeout(t);
    }
    last.current = clips;
    return undefined;
  }, [clips]);

  const l = meterPosition(left);
  const r = meterPosition(right);
  hold.current.l = Math.max(l, hold.current.l - 0.02);
  hold.current.r = Math.max(r, hold.current.r - 0.02);

  const bar = (v: number, peak: number) => (
    <div className="meter-bar">
      <div className="meter-fill" style={{ width: `${v * 100}%` }} />
      <div className="meter-peak" style={{ left: `${peak * 100}%` }} />
    </div>
  );

  return (
    <div className="meter" role="img" aria-label={label}>
      <div className="meter-bars">
        {bar(l, hold.current.l)}
        {bar(r, hold.current.r)}
      </div>
      <div className={`clip-lamp${lit ? " lit" : ""}`} title="clip" />
    </div>
  );
}