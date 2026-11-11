import { useTranslation } from "react-i18next";
import type { InputSel, OutputSel } from "../api";
import { useApp } from "../store";

interface Choice<T extends string> {
  value: T;
  label: string;
}

function Segmented<T extends string>({
  label,
  value,
  choices,
  disabled,
  onPick,
}: {
  label: string;
  value: T;
  choices: Choice<T>[];
  disabled: boolean;
  onPick: (v: T) => void;
}) {
  return (
    <div className="route-group" role="radiogroup" aria-label={label}>
      <span className="route-label">{label}</span>
      <div className="segmented">
        {choices.map((c) => (
          <button
            key={c.value}
            type="button"
            role="radio"
            aria-checked={value === c.value}
            className={value === c.value ? "on" : ""}
            disabled={disabled}
            onClick={() => onPick(c.value)}
          >
            {c.label}
          </button>
        ))}
      </div>
    </div>
  );
}

/**
 * Two-radio routing: which input is transmitted (A/B), which output plays
 * (A/B/Mix), and whether OTRSP from the logger switches both. Shown only once
 * a second device is configured; always visible so it can be used with the
 * Transmit and Receive sections collapsed.
 */
export function RoutingBar() {
  const { t } = useTranslation();
  const settings = useApp((s) => s.settings);
  const route = useApp((s) => s.route);
  const update = useApp((s) => s.update);
  if (!settings) return null;
  const a = settings.audio;
  const hasInB = !!a.input_device_b;
  const hasOutB = !!a.output_device_b;
  if (!hasInB && !hasOutB) return null;

  const otrspPort = a.otrsp_port.trim() !== "";
  const otrspOn = otrspPort && a.otrsp_enabled;
  // While OTRSP is in control, show what it selected; otherwise the manual choice.
  const input: InputSel = otrspOn ? route.input : a.input_select;
  const output: OutputSel = otrspOn ? route.output : a.output_select;

  return (
    <section className="routing" aria-label={t("routing.title")}>
      <Segmented<InputSel>
        label={t("routing.input")}
        value={hasInB ? input : "a"}
        disabled={!hasInB || otrspOn}
        choices={[
          { value: "a", label: "A" },
          { value: "b", label: "B" },
        ]}
        onPick={(v) => update((x) => void (x.audio.input_select = v))}
      />
      <Segmented<OutputSel>
        label={t("routing.output")}
        value={hasOutB ? output : "a"}
        disabled={!hasOutB || otrspOn}
        choices={[
          { value: "a", label: "A" },
          { value: "b", label: "B" },
          { value: "mix", label: t("routing.mix") },
        ]}
        onPick={(v) => update((x) => void (x.audio.output_select = v))}
      />
      <label
        className={`check otrsp${otrspPort ? "" : " disabled"}`}
        title={otrspPort ? (route.otrsp_error ?? a.otrsp_port) : t("routing.otrspHint")}
      >
        <input
          type="checkbox"
          checked={otrspOn}
          disabled={!otrspPort}
          onChange={(e) => update((x) => void (x.audio.otrsp_enabled = e.target.checked))}
        />
        {t("routing.otrsp")}
        {otrspPort && route.otrsp_error ? <span className="badge warn">!</span> : null}
      </label>
    </section>
  );
}
