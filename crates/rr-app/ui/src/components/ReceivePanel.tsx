import { useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { useApp } from "../store";
import { Meter } from "./Meter";
import { Section } from "./Section";
import { Slider } from "./Slider";

export function ReceivePanel() {
  const { t } = useTranslation();
  const settings = useApp((s) => s.settings);
  const update = useApp((s) => s.update);
  const status = useApp((s) => s.status);
  const [gains, setGains] = useState<Record<number, number>>({});
  if (!settings) return null;
  const a = settings.audio;
  const nameOf = (session: number) => status.users.find((u) => u.session === session)?.name ?? `#${session}`;
  const active = status.speakers.filter((s) => s.active);

  return (
    <Section title={t("rx.title")} open={false}>
      <Meter source="output" label={t("rx.volume")} />
      <Slider
        label={t("rx.volume")}
        value={Math.round(a.master_gain * 100)}
        min={0}
        max={200}
        format={(v) => `${v}%`}
        warn={a.master_gain > 1}
        onChange={(v) => update((x) => void (x.audio.master_gain = v / 100))}
      />
      <label className="check">
        <input type="checkbox" checked={a.mono_output} onChange={(e) => update((x) => void (x.audio.mono_output = e.target.checked))} />
        {t("rx.mono")}
      </label>

      <h3>{t("rx.speakers")}</h3>
      {active.length === 0 ? <p className="muted">{t("rx.none")}</p> : null}
      {active.map((sp) => (
        <Slider
          key={sp.session}
          label={nameOf(sp.session)}
          value={Math.round((gains[sp.session] ?? 1) * 100)}
          min={0}
          max={200}
          format={(v) => `${v}%`}
          onChange={(v) => {
            setGains((g) => ({ ...g, [sp.session]: v / 100 }));
            void api.setUserGain(sp.session, v / 100);
          }}
        />
      ))}
    </Section>
  );
}
