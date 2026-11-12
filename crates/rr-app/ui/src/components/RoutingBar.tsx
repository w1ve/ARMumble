import { useTranslation } from "react-i18next";
import type { Listen } from "../api";
import { useApp } from "../store";

interface Choice<T extends string | number> {
  value: T;
  label: string;
}

function Segmented<T extends string | number>({
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
            key={String(c.value)}
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
 * Two-radio (SO2R) bar. An operator picks what to hear (radio 1, radio 2 or
 * both, from the station's left/right stream); a station picks which radio the
 * operator's voice and PTT go to. With OTRSP on, the logger drives it, whether
 * the logger runs on this computer or at the other position.
 */
export function RoutingBar() {
  const { t } = useTranslation();
  const settings = useApp((s) => s.settings);
  const route = useApp((s) => s.route);
  const update = useApp((s) => s.update);
  if (!settings || !settings.audio.so2r_enabled) return null;
  const a = settings.audio;
  const station = a.so2r_role === "station";
  const otrspOn = a.otrsp_enabled && route.source !== "manual";
  const radio = (n: number) => t("routing.radio", { n });

  const sourceText =
    route.source === "local" ? t("routing.fromLocal") : route.source === "remote" ? t("routing.fromRemote") : null;

  return (
    <section className="routing" aria-label={t("routing.title")}>
      {station ? (
        <>
          <Segmented<1 | 2>
            label={t("routing.transmit")}
            value={otrspOn ? route.tx : a.so2r_tx}
            disabled={otrspOn}
            choices={[
              { value: 1, label: "1" },
              { value: 2, label: "2" },
            ]}
            onPick={(v) => update((x) => void (x.audio.so2r_tx = v))}
          />
          <span className="route-state">
            {t("routing.listening")}: {route.rx === "both" ? t("routing.both") : radio(Number(route.rx))}
          </span>
        </>
      ) : (
        <>
          <Segmented<Listen>
            label={t("routing.listen")}
            value={otrspOn ? route.rx : a.so2r_rx}
            disabled={otrspOn}
            choices={[
              { value: "1", label: "1" },
              { value: "2", label: "2" },
              { value: "both", label: t("routing.both") },
            ]}
            onPick={(v) => update((x) => void (x.audio.so2r_rx = v))}
          />
          <span className={`route-state tx-${route.tx}`}>
            {t("routing.txOn")}: {radio(route.tx)}
          </span>
        </>
      )}
      <label
        className={`check otrsp${route.otrsp_available ? "" : " disabled"}`}
        title={route.otrsp_available ? (sourceText ?? "") : t("routing.otrspHint")}
      >
        <input
          type="checkbox"
          checked={a.otrsp_enabled && route.otrsp_available}
          disabled={!route.otrsp_available}
          onChange={(e) => update((x) => void (x.audio.otrsp_enabled = e.target.checked))}
        />
        {t("routing.otrsp")}
        {sourceText && otrspOn ? <span className="badge">{sourceText}</span> : null}
        {route.otrsp_error ? <span className="badge warn">!</span> : null}
      </label>
    </section>
  );
}
