import { useState } from "react";
import { useTranslation } from "react-i18next";
import { api, type K4, type Rig } from "../api";
import { useApp } from "../store";

/** Why the settings cannot be used yet (a locale key), or null. */
export function k4Problem(k: K4): string | null {
  if (k.link === "serial") return k.serial_port.trim() ? null : "k4.needPort";
  if (!k.host.trim()) return "k4.needHost";
  if (!Number.isInteger(k.port) || k.port < 1 || k.port > 65535) return "k4.badPort";
  return null;
}

/**
 * Elecraft K4 CAT settings for one radio: a serial port (38400 8N1) or a
 * network address. Changes are kept in the dialog until Save.
 */
export function K4Dialog({ radio, rig, onSave, onClose }: { radio: number; rig: Rig; onSave: (rig: Rig) => Promise<void>; onClose: () => void }) {
  const { t } = useTranslation();
  const ports = useApp((s) => s.ports);
  const [k, setK] = useState<K4>(() => ({ ...rig.k4 }));
  const [showPw, setShowPw] = useState(false);
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const problem = k4Problem(k);
  const edit = (patch: Partial<K4>) => {
    setK((v) => ({ ...v, ...patch }));
    setResult(null);
  };
  const current: Rig = { model: "k4", k4: k };
  const test = async () => {
    setTesting(true);
    setResult(null);
    try {
      const id = await api.testRig(current);
      setResult({ ok: true, text: t("k4.testOk", { id }) });
    } catch (e) {
      setResult({ ok: false, text: String(e) });
    } finally {
      setTesting(false);
    }
  };
  const save = async () => {
    setSaveError(null);
    try {
      await onSave(current);
      onClose();
    } catch (e) {
      setSaveError(String(e));
    }
  };
  return (
    <div className="dialog-backdrop" role="dialog" aria-modal="true" aria-label={t("k4.title", { radio })}>
      <div className="dialog servers-dialog">
        <header>
          <h2>{t("k4.title", { radio })}</h2>
        </header>
        <div className="row">
          <label className="check">
            <input type="radio" name="k4-link" checked={k.link === "serial"} onChange={() => edit({ link: "serial" })} />
            {t("k4.serial")}
          </label>
          <label className="check">
            <input type="radio" name="k4-link" checked={k.link === "tcp"} onChange={() => edit({ link: "tcp" })} />
            {t("k4.network")}
          </label>
        </div>
        {k.link === "serial" ? (
          <>
            <label className="field">
              <span className="field-label">{t("settings.port")}</span>
              <select value={k.serial_port} onChange={(e) => edit({ serial_port: e.target.value })}>
                <option value="">{t("settings.none")}</option>
                {ports.map((p) => (
                  <option key={p.name} value={p.name}>
                    {p.name} {p.label}
                  </option>
                ))}
              </select>
            </label>
            <p className="hint">{t("k4.serialHint")}</p>
          </>
        ) : (
          <>
            <div className="row">
              <label className="field grow">
                <span className="field-label">{t("k4.host")}</span>
                <input value={k.host} spellCheck={false} autoCapitalize="off" onChange={(e) => edit({ host: e.target.value })} />
              </label>
              <label className="field narrow">
                <span className="field-label">{t("k4.port")}</span>
                <input
                  type="number"
                  min={1}
                  max={65535}
                  value={Number.isFinite(k.port) ? k.port : ""}
                  onChange={(e) => edit({ port: e.target.value === "" ? NaN : Number(e.target.value) })}
                />
              </label>
            </div>
            <label className="field">
              <span className="field-label">{t("k4.password")}</span>
              <div className="row">
                <input className="grow" type={showPw ? "text" : "password"} value={k.password} autoComplete="off" onChange={(e) => edit({ password: e.target.value })} />
                <button type="button" className="ghost" onClick={() => setShowPw((v) => !v)}>
                  {showPw ? t("servers.hide") : t("servers.show")}
                </button>
              </div>
            </label>
            <p className="hint">{t("k4.networkHint")}</p>
          </>
        )}
        {problem ? <p className="warn-text">{t(problem)}</p> : null}
        <div className="row">
          <button type="button" onClick={() => void test()} disabled={testing || problem !== null}>
            {testing ? t("k4.testing") : t("k4.test")}
          </button>
          {result ? <span className={result.ok ? "muted" : "error"}>{result.text}</span> : null}
        </div>
        <p className="hint">{t("k4.testHint")}</p>
        {saveError ? <p className="error">{saveError}</p> : null}
        <footer className="row dialog-buttons">
          <button type="button" className="ghost" onClick={onClose}>
            {t("servers.cancel")}
          </button>
          <button type="button" className="primary" onClick={() => void save()} disabled={problem !== null}>
            {t("servers.save")}
          </button>
        </footer>
      </div>
    </div>
  );
}
