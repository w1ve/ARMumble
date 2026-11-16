import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useApp } from "../store";
import { ServerEditor } from "./ServerEditor";

export function ConnectCard() {
  const { t } = useTranslation();
  const settings = useApp((s) => s.settings);
  const servers = useApp((s) => s.servers);
  const selectServer = useApp((s) => s.selectServer);
  const update = useApp((s) => s.update);
  const connect = useApp((s) => s.connect);
  const busy = useApp((s) => s.busy);
  const status = useApp((s) => s.status);
  const [editing, setEditing] = useState(false);
  if (!settings) return null;

  const s = settings.server;
  const chosen = servers.find((x) => x.name === s.name);

  return (
    <>
      <form
        className="card connect"
        onSubmit={(e) => {
          e.preventDefault();
          void connect();
        }}
      >
        <label className="field">
          <span className="field-label">{t("connect.server")}</span>
          <div className="row">
            <select className="grow" value={chosen?.name ?? ""} disabled={servers.length === 0} onChange={(e) => selectServer(e.target.value)}>
              {servers.length === 0 ? <option value="">{t("servers.none")}</option> : null}
              {servers.map((x) => (
                <option key={x.name} value={x.name}>
                  {x.name}
                </option>
              ))}
            </select>
            <button type="button" onClick={() => setEditing(true)}>
              {t("servers.edit")}
            </button>
          </div>
          {chosen ? (
            <span className="hint">
              {chosen.host}:{chosen.port}
            </span>
          ) : null}
        </label>

        <label className="field">
          <span className="field-label">{t("connect.username")}</span>
          <input
            value={s.username}
            autoCapitalize="characters"
            autoComplete="username"
            onChange={(e) => update((x) => void (x.server.username = e.target.value))}
          />
        </label>

        <label className="field">
          <span className="field-label">{t("connect.scope")}</span>
          <input value={s.scope} onChange={(e) => update((x) => void (x.server.scope = e.target.value))} />
          <span className="hint">{t("connect.scopeHint")}</span>
        </label>

        {status.scope_missing ? <p className="error">{t("connect.scopeMissing")}</p> : null}
        <button type="submit" className="primary" disabled={busy || !chosen || status.phase === "connecting"}>
          {status.phase === "connecting" ? t("status.connecting") : t("connect.connect")}
        </button>
      </form>
      {editing ? <ServerEditor onClose={() => setEditing(false)} /> : null}
    </>
  );
}
