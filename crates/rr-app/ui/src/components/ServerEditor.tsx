import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ServerEntry } from "../api";
import { useApp } from "../store";
import { serverListError } from "../util";

const BLANK: ServerEntry = { name: "", host: "", port: 64738, password: "" };

/**
 * Edit the server list: name, address, port and password for each entry.
 * Changes are kept in the dialog until Save; Cancel discards them.
 */
export function ServerEditor({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const saved = useApp((s) => s.servers);
  const saveServers = useApp((s) => s.saveServers);
  const [rows, setRows] = useState<ServerEntry[]>(() => saved.map((s) => ({ ...s })));
  const [shown, setShown] = useState<Set<number>>(new Set());
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const problem = serverListError(rows);
  const edit = (i: number, patch: Partial<ServerEntry>) =>
    setRows((r) => r.map((row, j) => (j === i ? { ...row, ...patch } : row)));
  const remove = (i: number) => {
    setRows((r) => r.filter((_, j) => j !== i));
    setShown(new Set());
  };
  const move = (i: number, by: number) =>
    setRows((r) => {
      const j = i + by;
      if (j < 0 || j >= r.length) return r;
      const next = [...r];
      [next[i], next[j]] = [next[j], next[i]];
      return next;
    });

  const save = async () => {
    setSaving(true);
    setSaveError(null);
    try {
      await saveServers(rows);
      onClose();
    } catch (e) {
      setSaveError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="dialog-backdrop" role="dialog" aria-modal="true" aria-label={t("servers.title")}>
      <div className="dialog servers-dialog">
        <header>
          <h2>{t("servers.title")}</h2>
        </header>

        {rows.length === 0 ? <p className="muted">{t("servers.empty")}</p> : null}
        <ul className="server-rows">
          {rows.map((s, i) => {
            const bad = problem?.row === i;
            return (
              <li key={i} className={`server-row${bad ? " bad" : ""}`}>
                <div className="row">
                  <label className="field grow">
                    <span className="field-label">{t("servers.name")}</span>
                    <input value={s.name} onChange={(e) => edit(i, { name: e.target.value })} />
                  </label>
                  <div className="server-row-actions">
                    <button type="button" className="ghost" onClick={() => move(i, -1)} disabled={i === 0} aria-label={t("servers.up")} title={t("servers.up")}>
                      ↑
                    </button>
                    <button
                      type="button"
                      className="ghost"
                      onClick={() => move(i, 1)}
                      disabled={i === rows.length - 1}
                      aria-label={t("servers.down")}
                      title={t("servers.down")}
                    >
                      ↓
                    </button>
                    <button type="button" className="ghost danger" onClick={() => remove(i)} aria-label={t("servers.remove")} title={t("servers.remove")}>
                      ✕
                    </button>
                  </div>
                </div>
                <div className="row">
                  <label className="field grow">
                    <span className="field-label">{t("servers.host")}</span>
                    <input value={s.host} spellCheck={false} autoCapitalize="off" onChange={(e) => edit(i, { host: e.target.value })} />
                  </label>
                  <label className="field narrow">
                    <span className="field-label">{t("servers.port")}</span>
                    <input
                      type="number"
                      min={1}
                      max={65535}
                      value={Number.isFinite(s.port) ? s.port : ""}
                      onChange={(e) => edit(i, { port: e.target.value === "" ? NaN : Number(e.target.value) })}
                    />
                  </label>
                </div>
                <label className="field">
                  <span className="field-label">{t("servers.password")}</span>
                  <div className="row">
                    <input
                      className="grow"
                      type={shown.has(i) ? "text" : "password"}
                      value={s.password}
                      autoComplete="off"
                      onChange={(e) => edit(i, { password: e.target.value })}
                    />
                    <button
                      type="button"
                      className="ghost"
                      onClick={() =>
                        setShown((v) => {
                          const n = new Set(v);
                          if (n.has(i)) n.delete(i);
                          else n.add(i);
                          return n;
                        })
                      }
                    >
                      {shown.has(i) ? t("servers.hide") : t("servers.show")}
                    </button>
                  </div>
                </label>
                {bad && problem ? <p className="warn-text">{t(problem.key)}</p> : null}
              </li>
            );
          })}
        </ul>

        <button type="button" onClick={() => setRows((r) => [...r, { ...BLANK }])}>
          {t("servers.add")}
        </button>
        {saveError ? <p className="error">{saveError}</p> : null}
        <footer className="row dialog-buttons">
          <button type="button" className="ghost" onClick={onClose}>
            {t("servers.cancel")}
          </button>
          <button type="button" className="primary" onClick={() => void save()} disabled={saving || problem !== null}>
            {t("servers.save")}
          </button>
        </footer>
      </div>
    </div>
  );
}
