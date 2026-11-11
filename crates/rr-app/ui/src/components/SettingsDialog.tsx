import { useTranslation } from "react-i18next";
import { LANGUAGES } from "../i18n";
import { useApp } from "../store";
import { HotkeyInput } from "./HotkeyInput";
import { Slider } from "./Slider";

function DeviceSelect({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string | null;
  options: { id: string; name: string }[];
  onChange: (v: string | null) => void;
}) {
  const { t } = useTranslation();
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      <select value={value ?? ""} onChange={(e) => onChange(e.target.value || null)}>
        <option value="">{t("settings.default")}</option>
        {options.map((o) => (
          <option key={o.id} value={o.id}>
            {o.name}
          </option>
        ))}
      </select>
    </label>
  );
}

export function SettingsDialog({ onClose }: { onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const settings = useApp((s) => s.settings);
  const update = useApp((s) => s.update);
  const devices = useApp((s) => s.devices);
  const ports = useApp((s) => s.ports);
  const refresh = useApp((s) => s.refreshHardware);
  const channels = useApp((s) => s.status.channels);
  if (!settings) return null;
  const { local, station } = settings.ptt;
  const hk = settings.hotkeys;

  const portSelect = (value: string, onChange: (v: string) => void) => (
    <select value={value} onChange={(e) => onChange(e.target.value)}>
      <option value="">{t("settings.none")}</option>
      {ports.map((p) => (
        <option key={p.name} value={p.name}>
          {p.name} {p.label}
        </option>
      ))}
    </select>
  );

  const channelSelect = (value: number | null, onChange: (v: number | null) => void) => (
    <select value={value ?? ""} onChange={(e) => onChange(e.target.value === "" ? null : Number(e.target.value))}>
      <option value="">{t("settings.none")}</option>
      {channels.map((c) => (
        <option key={c.id} value={c.id}>
          {c.name}
        </option>
      ))}
    </select>
  );

  return (
    <div className="dialog-backdrop" role="dialog" aria-modal="true" aria-label={t("settings.title")}>
      <div className="dialog">
        <header>
          <h2>{t("settings.title")}</h2>
          <button type="button" className="ghost" onClick={onClose}>
            {t("settings.close")}
          </button>
        </header>

        <h3>{t("settings.devices")}</h3>
        <DeviceSelect
          label={t("settings.input")}
          value={settings.audio.input_device}
          options={devices.filter((d) => d.input)}
          onChange={(v) => update((x) => void (x.audio.input_device = v))}
        />
        <DeviceSelect
          label={t("settings.output")}
          value={settings.audio.output_device}
          options={devices.filter((d) => d.output)}
          onChange={(v) => update((x) => void (x.audio.output_device = v))}
        />
        <button type="button" className="ghost" onClick={() => void refresh()}>
          {t("settings.refresh")}
        </button>
        <p className="hint">{t("settings.reconnect")}</p>

        <h3>{t("settings.ptt")}</h3>
        <fieldset>
          <legend>{t("settings.local")}</legend>
          <label className="check">
            <input type="checkbox" checked={local.enabled} onChange={(e) => update((x) => void (x.ptt.local.enabled = e.target.checked))} />
            {t("settings.enabled")}
          </label>
          <label className="field">
            <span className="field-label">{t("settings.port")}</span>
            {portSelect(local.port, (v) => update((x) => void (x.ptt.local.port = v)))}
          </label>
          <label className="field">
            <span className="field-label">{t("settings.senseLine")}</span>
            <select
              value={local.sense_line}
              onChange={(e) => update((x) => void (x.ptt.local.sense_line = e.target.value as typeof local.sense_line))}
            >
              <option value="cts">CTS</option>
              <option value="dsr">DSR</option>
              <option value="cd">CD</option>
              <option value="ri">RI</option>
            </select>
          </label>
          <label className="check">
            <input type="checkbox" checked={local.active_low} onChange={(e) => update((x) => void (x.ptt.local.active_low = e.target.checked))} />
            {t("settings.activeLow")}
          </label>
        </fieldset>

        <fieldset>
          <legend>{t("settings.station")}</legend>
          <label className="check">
            <input type="checkbox" checked={station.enabled} onChange={(e) => update((x) => void (x.ptt.station.enabled = e.target.checked))} />
            {t("settings.enabled")}
          </label>
          <label className="field">
            <span className="field-label">{t("settings.port")}</span>
            {portSelect(station.port, (v) => update((x) => void (x.ptt.station.port = v)))}
          </label>
          <label className="field">
            <span className="field-label">{t("settings.keyLine")}</span>
            <select
              value={station.key_line}
              onChange={(e) => update((x) => void (x.ptt.station.key_line = e.target.value as typeof station.key_line))}
            >
              <option value="rts">RTS</option>
              <option value="dtr">DTR</option>
            </select>
          </label>
          <label className="check">
            <input type="checkbox" checked={station.active_low} onChange={(e) => update((x) => void (x.ptt.station.active_low = e.target.checked))} />
            {t("settings.activeLow")}
          </label>
          <label className="check">
            <input type="checkbox" checked={station.vox} onChange={(e) => update((x) => void (x.ptt.station.vox = e.target.checked))} />
            {t("settings.stationVox")}
          </label>
          <Slider
            label={t("settings.stationVoxHang")}
            value={station.vox_hang_ms}
            min={0}
            max={5000}
            step={50}
            format={(v) => `${v} ms`}
            disabled={!station.vox}
            onChange={(v) => update((x) => void (x.ptt.station.vox_hang_ms = v))}
          />
          <Slider
            label={t("settings.stationHang")}
            value={station.hang_ms}
            min={0}
            max={2000}
            step={50}
            format={(v) => `${v} ms`}
            onChange={(v) => update((x) => void (x.ptt.station.hang_ms = v))}
          />
          <Slider
            label={t("settings.maxTx")}
            value={station.max_tx_secs}
            min={5}
            max={900}
            step={5}
            onChange={(v) => update((x) => void (x.ptt.station.max_tx_secs = v))}
          />
        </fieldset>
        <p className="hint">{t("settings.reconnect")}</p>

        <h3>{t("settings.hotkeys")}</h3>
        <HotkeyInput label={t("settings.pttKey")} value={hk.ptt} onChange={(v) => update((x) => void (x.hotkeys.ptt = v))} />
        <HotkeyInput label={t("settings.whisperKey")} value={hk.whisper} onChange={(v) => update((x) => void (x.hotkeys.whisper = v))} />
        <label className="field">
          <span className="field-label">{t("settings.whisperTarget")}</span>
          {channelSelect(hk.whisper_target, (v) => update((x) => void (x.hotkeys.whisper_target = v)))}
        </label>
        <HotkeyInput label={t("settings.listenKey")} value={hk.listen_in} onChange={(v) => update((x) => void (x.hotkeys.listen_in = v))} />
        <label className="field">
          <span className="field-label">{t("settings.listenTarget")}</span>
          {channelSelect(hk.listen_target, (v) => update((x) => void (x.hotkeys.listen_target = v)))}
        </label>
        <label className="check">
          <input
            type="checkbox"
            checked={hk.mute_home_on_listen}
            onChange={(e) => update((x) => void (x.hotkeys.mute_home_on_listen = e.target.checked))}
          />
          {t("settings.muteHome")}
        </label>
        <p className="hint">{t("settings.hotkeyHint")}</p>

        <h3>{t("settings.language")}</h3>
        <select
          value={i18n.language.slice(0, 2)}
          onChange={(e) => {
            void i18n.changeLanguage(e.target.value);
            update((x) => void (x.language = e.target.value));
          }}
        >
          {LANGUAGES.map((l) => (
            <option key={l.code} value={l.code}>
              {l.label}
            </option>
          ))}
        </select>
        <label className="field">
          <span className="field-label">{t("settings.theme")}</span>
          <select value={settings.theme} onChange={(e) => update((x) => void (x.theme = e.target.value as "dark" | "light"))}>
            <option value="dark">{t("settings.dark")}</option>
            <option value="light">{t("settings.light")}</option>
          </select>
        </label>
      </div>
    </div>
  );
}
