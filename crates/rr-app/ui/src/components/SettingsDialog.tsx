import { useTranslation } from "react-i18next";
import type { So2rRole } from "../api";
import { LANGUAGES } from "../i18n";
import { useApp } from "../store";
import { HotkeyInput } from "./HotkeyInput";
import { Slider } from "./Slider";

function DeviceSelect({
  label,
  value,
  options,
  emptyLabel,
  onChange,
}: {
  label: string;
  value: string | null;
  options: { id: string; name: string }[];
  /** Text of the empty choice: "System default" for A, "(None)" for B. */
  emptyLabel: string;
  onChange: (v: string | null) => void;
}) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      <select value={value ?? ""} onChange={(e) => onChange(e.target.value || null)}>
        <option value="">{emptyLabel}</option>
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
  const route = useApp((s) => s.route);
  const so2r = settings?.audio.so2r_enabled ?? false;
  const station = so2r && settings?.audio.so2r_role === "station";
  /** A two-radio station with one sound card per radio. */
  const twoCards = station && !settings?.audio.so2r_single_device;
  if (!settings) return null;
  const { local, station: stationPtt, station2 } = settings.ptt;
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

        <h3>{t("settings.so2r")}</h3>
        <label className="check">
          <input type="checkbox" checked={so2r} onChange={(e) => update((x) => void (x.audio.so2r_enabled = e.target.checked))} />
          {t("settings.so2rEnable")}
        </label>
        {so2r ? (
          <>
            <label className="field">
              <span className="field-label">{t("settings.so2rRole")}</span>
              <select value={settings.audio.so2r_role} onChange={(e) => update((x) => void (x.audio.so2r_role = e.target.value as So2rRole))}>
                <option value="operator">{t("settings.roleOperator")}</option>
                <option value="station">{t("settings.roleStation")}</option>
              </select>
            </label>
            <p className="hint">{station ? t("settings.roleStationHint") : t("settings.roleOperatorHint")}</p>
            {station ? (
              <label className="check">
                <input
                  type="checkbox"
                  checked={settings.audio.so2r_single_device}
                  onChange={(e) => update((x) => void (x.audio.so2r_single_device = e.target.checked))}
                />
                {t("settings.singleDevice")}
              </label>
            ) : null}
            <label className="field">
              <span className="field-label">{t("settings.otrspPort")}</span>
              <select value={settings.audio.otrsp_port} onChange={(e) => update((x) => void (x.audio.otrsp_port = e.target.value))}>
                <option value="">{t("settings.otrspDisabled")}</option>
                {ports.map((p) => (
                  <option key={p.name} value={p.name}>
                    {p.name} {p.label}
                  </option>
                ))}
              </select>
            </label>
            {settings.audio.otrsp_port && route.otrsp_error ? <p className="warn-text">{route.otrsp_error}</p> : null}
            <p className="hint">{t("settings.otrspHelp")}</p>
          </>
        ) : null}

        <h3>{t("settings.devices")}</h3>
        <DeviceSelect
          label={twoCards ? t("settings.inputA") : t("settings.input")}
          value={settings.audio.input_device}
          options={devices.filter((d) => d.input)}
          emptyLabel={t("settings.default")}
          onChange={(v) => update((x) => void (x.audio.input_device = v))}
        />
        {twoCards ? (
          <DeviceSelect
            label={t("settings.inputB")}
            value={settings.audio.input_device_b}
            options={devices.filter((d) => d.input)}
            emptyLabel={t("settings.noDevice")}
            onChange={(v) => update((x) => void (x.audio.input_device_b = v))}
          />
        ) : null}
        <DeviceSelect
          label={twoCards ? t("settings.outputA") : t("settings.output")}
          value={settings.audio.output_device}
          options={devices.filter((d) => d.output)}
          emptyLabel={t("settings.default")}
          onChange={(v) => update((x) => void (x.audio.output_device = v))}
        />
        {twoCards ? (
          <DeviceSelect
            label={t("settings.outputB")}
            value={settings.audio.output_device_b}
            options={devices.filter((d) => d.output)}
            emptyLabel={t("settings.noDevice")}
            onChange={(v) => update((x) => void (x.audio.output_device_b = v))}
          />
        ) : null}
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
            <input type="checkbox" checked={stationPtt.enabled} onChange={(e) => update((x) => void (x.ptt.station.enabled = e.target.checked))} />
            {t("settings.enabled")}
          </label>
          <label className="field">
            <span className="field-label">{t("settings.port")}</span>
            {portSelect(stationPtt.port, (v) => update((x) => void (x.ptt.station.port = v)))}
          </label>
          <label className="field">
            <span className="field-label">{t("settings.keyLine")}</span>
            <select
              value={stationPtt.key_line}
              onChange={(e) => update((x) => void (x.ptt.station.key_line = e.target.value as typeof stationPtt.key_line))}
            >
              <option value="rts">RTS</option>
              <option value="dtr">DTR</option>
            </select>
          </label>
          <label className="check">
            <input type="checkbox" checked={stationPtt.active_low} onChange={(e) => update((x) => void (x.ptt.station.active_low = e.target.checked))} />
            {t("settings.activeLow")}
          </label>
          <label className="check">
            <input type="checkbox" checked={stationPtt.vox} onChange={(e) => update((x) => void (x.ptt.station.vox = e.target.checked))} />
            {t("settings.stationVox")}
          </label>
          <Slider
            label={t("settings.stationVoxHang")}
            value={stationPtt.vox_hang_ms}
            min={0}
            max={5000}
            step={50}
            format={(v) => `${v} ms`}
            disabled={!stationPtt.vox}
            onChange={(v) => update((x) => void (x.ptt.station.vox_hang_ms = v))}
          />
          <Slider
            label={t("settings.stationHang")}
            value={stationPtt.hang_ms}
            min={0}
            max={2000}
            step={50}
            format={(v) => `${v} ms`}
            onChange={(v) => update((x) => void (x.ptt.station.hang_ms = v))}
          />
          <Slider
            label={t("settings.maxTx")}
            value={stationPtt.max_tx_secs}
            min={5}
            max={900}
            step={5}
            onChange={(v) => update((x) => void (x.ptt.station.max_tx_secs = v))}
          />
        </fieldset>

        {station ? (
          <fieldset>
            <legend>{t("settings.station2")}</legend>
            <label className="check">
              <input type="checkbox" checked={station2.enabled} onChange={(e) => update((x) => void (x.ptt.station2.enabled = e.target.checked))} />
              {t("settings.enabled")}
            </label>
            <label className="field">
              <span className="field-label">{t("settings.port")}</span>
              {portSelect(station2.port, (v) => update((x) => void (x.ptt.station2.port = v)))}
            </label>
            <label className="field">
              <span className="field-label">{t("settings.keyLine")}</span>
              <select
                value={station2.key_line}
                onChange={(e) => update((x) => void (x.ptt.station2.key_line = e.target.value as typeof station2.key_line))}
              >
                <option value="dtr">DTR</option>
                <option value="rts">RTS</option>
              </select>
            </label>
            <label className="check">
              <input type="checkbox" checked={station2.active_low} onChange={(e) => update((x) => void (x.ptt.station2.active_low = e.target.checked))} />
              {t("settings.activeLow")}
            </label>
            <p className="hint">{t("settings.station2Hint")}</p>
          </fieldset>
        ) : null}
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
