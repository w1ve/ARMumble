import { useTranslation } from "react-i18next";
import { api, type ChannelMap, type TxMode } from "../api";
import { useApp } from "../store";
import { Meter } from "./Meter";
import { Section } from "./Section";
import { Slider } from "./Slider";
import { useHold } from "./useHold";

function PttButton() {
  const { t } = useTranslation();
  const status = useApp((s) => s.status);
  const hold = useHold(
    () => void api.setPtt(true),
    () => void api.setPtt(false),
  );
  const on = status.transmitting;
  return (
    <button type="button" className={`ptt${on ? " on-air" : ""}`} disabled={status.phase !== "connected"} {...hold}>
      {on ? t("tx.onAir") : t("tx.pttButton")}
    </button>
  );
}

export function TransmitPanel() {
  const { t } = useTranslation();
  const settings = useApp((s) => s.settings);
  const update = useApp((s) => s.update);
  const status = useApp((s) => s.status);
  if (!settings) return null;
  const a = settings.audio;
  const m = status.meters;

  return (
    <Section title={t("tx.title")} badge={status.transmitting ? <span className="badge on">{t("tx.onAir")}</span> : null}>
      <label className="field">
        <span className="field-label">{t("tx.mode")}</span>
        <select value={a.tx_mode} onChange={(e) => update((x) => void (x.audio.tx_mode = e.target.value as TxMode))}>
          <option value="ptt">{t("tx.ptt")}</option>
          <option value="vox">{t("tx.vox")}</option>
          <option value="continuous">{t("tx.continuous")}</option>
        </select>
      </label>

      {a.tx_mode === "ptt" ? <PttButton /> : null}

      {a.tx_mode === "vox" ? (
        <>
          <Slider
            label={t("tx.voxThreshold")}
            value={a.vox_threshold_db}
            min={-70}
            max={-5}
            format={(v) => `${v} dB`}
            onChange={(v) => update((x) => void (x.audio.vox_threshold_db = v))}
          />
          <Slider
            label={t("tx.voxHang")}
            value={a.vox_hang_ms}
            min={0}
            max={2000}
            step={50}
            format={(v) => `${v} ms`}
            onChange={(v) => update((x) => void (x.audio.vox_hang_ms = v))}
          />
        </>
      ) : null}

      <Meter left={m.in_l} right={m.in_r} clips={m.in_clips} label={t("tx.gain")} />
      <Slider
        label={t("tx.gain")}
        value={Math.round(a.input_gain * 100)}
        min={0}
        max={200}
        format={(v) => `${v}%`}
        warn={a.input_gain > 1}
        hint={a.input_gain > 1 ? t("tx.gainHigh") : undefined}
        onChange={(v) => update((x) => void (x.audio.input_gain = v / 100))}
      />
      {status.meters.in_clips > 0 && status.transmitting ? <p className="warn-text">{t("tx.clip")}</p> : null}

      <label className="field">
        <span className="field-label">{t("tx.map")}</span>
        <select value={a.channel_map} onChange={(e) => update((x) => void (x.audio.channel_map = e.target.value as ChannelMap))}>
          <option value="stereo">{t("tx.stereo")}</option>
          <option value="left">{t("tx.left")}</option>
          <option value="right">{t("tx.right")}</option>
          <option value="mix">{t("tx.mix")}</option>
        </select>
      </label>
      <p className="hint">{t("tx.noProcessing")}</p>
    </Section>
  );
}
