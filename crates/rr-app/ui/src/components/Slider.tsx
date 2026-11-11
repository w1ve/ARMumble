import type { ReactNode } from "react";

interface Props {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  format?: (v: number) => string;
  onChange: (v: number) => void;
  warn?: boolean;
  hint?: ReactNode;
  disabled?: boolean;
}

export function Slider({ label, value, min, max, step = 1, format, onChange, warn, hint, disabled }: Props) {
  return (
    <label className={`field slider${warn ? " warn" : ""}`}>
      <span className="field-label">
        {label}
        <output>{format ? format(value) : value}</output>
      </span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      {hint ? <span className="hint">{hint}</span> : null}
    </label>
  );
}