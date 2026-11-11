import { useTranslation } from "react-i18next";
import { useApp } from "../store";
import { BUILTIN_SERVERS, RR_PASSWORD, serverKey } from "../servers";

const CUSTOM = "custom";

export function ConnectCard() {
  const { t } = useTranslation();
  const settings = useApp((s) => s.settings);
  const update = useApp((s) => s.update);
  const connect = useApp((s) => s.connect);
  const busy = useApp((s) => s.busy);
  const status = useApp((s) => s.status);
  if (!settings) return null;

  const s = settings.server;
  const builtin = BUILTIN_SERVERS.find((b) => serverKey(b) === serverKey(s));
  const selected = builtin ? serverKey(builtin) : CUSTOM;

  return (
    <form
      className="card connect"
      onSubmit={(e) => {
        e.preventDefault();
        void connect();
      }}
    >
      <label className="field">
        <span className="field-label">{t("connect.server")}</span>
        <select
          value={selected}
          onChange={(e) => {
            const pick = BUILTIN_SERVERS.find((b) => serverKey(b) === e.target.value);
            if (!pick) return;
            update((x) => {
              x.server.host = pick.host;
              x.server.port = pick.port;
              x.server.password = RR_PASSWORD;
            });
          }}
        >
          {BUILTIN_SERVERS.map((b) => (
            <option key={serverKey(b)} value={serverKey(b)}>
              {b.name} ({b.stereo ? t("connect.fancy") : t("connect.legacy")})
            </option>
          ))}
          <option value={CUSTOM}>{t("connect.custom")}</option>
        </select>
      </label>

      {selected === CUSTOM ? (
        <div className="row">
          <label className="field grow">
            <span className="field-label">{t("connect.host")}</span>
            <input value={s.host} onChange={(e) => update((x) => void (x.server.host = e.target.value))} />
          </label>
          <label className="field narrow">
            <span className="field-label">{t("connect.port")}</span>
            <input
              type="number"
              min={1}
              max={65535}
              value={s.port}
              onChange={(e) => update((x) => void (x.server.port = Number(e.target.value)))}
            />
          </label>
        </div>
      ) : null}

      <label className="field">
        <span className="field-label">{t("connect.username")}</span>
        <input
          value={s.username}
          autoCapitalize="characters"
          autoComplete="username"
          onChange={(e) => update((x) => void (x.server.username = e.target.value))}
        />
      </label>

      {selected === CUSTOM ? (
        <label className="field">
          <span className="field-label">{t("connect.password")}</span>
          <input
            type="password"
            value={s.password}
            autoComplete="current-password"
            onChange={(e) => update((x) => void (x.server.password = e.target.value))}
          />
        </label>
      ) : null}

      <label className="field">
        <span className="field-label">{t("connect.scope")}</span>
        <input value={s.scope} onChange={(e) => update((x) => void (x.server.scope = e.target.value))} />
        <span className="hint">{t("connect.scopeHint")}</span>
      </label>

      {status.scope_missing ? <p className="error">{t("connect.scopeMissing")}</p> : null}
      <button type="submit" className="primary" disabled={busy || status.phase === "connecting"}>
        {status.phase === "connecting" ? t("status.connecting") : t("connect.connect")}
      </button>
    </form>
  );
}
