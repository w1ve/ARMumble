import { useTranslation } from "react-i18next";
import { useApp } from "../store";
import { Section } from "./Section";

const BITRATES = [32, 48, 64, 96, 128, 192, 256];
const FRAMES = [20, 40, 60];

export function LinkPanel() {
  const { t } = useTranslation();
  const settings = useApp((s) => s.settings);
  const update = useApp((s) => s.update);
  const status = useApp((s) => s.status);
  if (!settings) return null;
  const a = settings.audio;
  const plan = status.plan;

  return (
    <Section title={t("link.title")} open={false} badge={plan?.clamped ? <span className="badge warn">!</span> : null}>
      <div className="row">
        <label className="field grow">
          <span className="field-label">{t("link.bitrate")}</span>
          <select value={a.bitrate_kbps} onChange={(e) => update((x) => void (x.audio.bitrate_kbps = Number(e.target.value)))}>
            {BITRATES.map((b) => (
              <option key={b} value={b}>
                {b} kbit/s
              </option>
            ))}
          </select>
        </label>
        <label className="field grow">
          <span className="field-label">{t("link.maxFrame")}</span>
          <select value={a.max_frame_ms} onChange={(e) => update((x) => void (x.audio.max_frame_ms = Number(e.target.value)))}>
            {FRAMES.map((f) => (
              <option key={f} value={f}>
                {f} ms
              </option>
            ))}
          </select>
        </label>
      </div>
      <label className="check">
        <input type="checkbox" checked={a.stereo} onChange={(e) => update((x) => void (x.audio.stereo = e.target.checked))} />
        {t("link.stereo")}
      </label>

      {plan ? (
        <p className={plan.clamped ? "warn-text" : "muted"}>
          {t("link.plan")}: {plan.bitrate_kbps.toFixed(0)} kbit/s, {plan.frame_ms} ms ({t("link.wire")} {plan.wire_kbps.toFixed(0)} kbit/s)
          {plan.clamped ? ` - ${t("link.clamped")}` : ""}
        </p>
      ) : null}
      {status.udp !== null ? <p className={status.udp ? "muted" : "warn-text"}>{status.udp ? t("link.udp") : t("link.tcp")}</p> : null}
      <p className="muted">
        {t("link.dropped")}: {status.dropped}
        {status.audio_errors > 0 ? ` · ${t("link.audioErrors")}: ${status.audio_errors} ${status.audio_error}` : ""}
      </p>

      {status.speakers
        .filter((s) => s.active)
        .map((s) => (
          <p key={s.session} className="muted mono">
            #{s.session} {t("link.buffer")} {s.depth_ms.toFixed(0)}/{s.target_ms.toFixed(0)} ms · {t("link.jitter")} {s.jitter_ms.toFixed(0)} ms ·{" "}
            {s.ratio_ppm >= 0 ? "+" : ""}
            {s.ratio_ppm.toFixed(0)} ppm · {t("link.lost")} {s.lost} · {t("link.late")} {s.late}
          </p>
        ))}
      <label className="check">
        <input type="checkbox" checked={a.force_tcp} onChange={(e) => update((x) => void (x.audio.force_tcp = e.target.checked))} />
        {t("settings.forceTcp")}
      </label>
    </Section>
  );
}
