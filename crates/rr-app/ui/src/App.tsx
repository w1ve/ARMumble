import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChannelTree } from "./components/ChannelTree";
import { ConnectCard } from "./components/ConnectCard";
import { LinkPanel } from "./components/LinkPanel";
import { ReceivePanel } from "./components/ReceivePanel";
import { RoutingBar } from "./components/RoutingBar";
import { SettingsDialog } from "./components/SettingsDialog";
import { TransmitPanel } from "./components/TransmitPanel";
import { detectLanguage } from "./i18n";
import { useApp } from "./store";

export function App() {
  const { t, i18n } = useTranslation();
  const init = useApp((s) => s.init);
  const settings = useApp((s) => s.settings);
  const status = useApp((s) => s.status);
  const busy = useApp((s) => s.busy);
  const actionError = useApp((s) => s.actionError);
  const disconnect = useApp((s) => s.disconnect);
  const [showSettings, setShowSettings] = useState(false);

  useEffect(() => {
    void init();
  }, [init]);

  useEffect(() => {
    if (!settings) return;
    document.documentElement.dataset.theme = settings.theme;
    void i18n.changeLanguage(settings.language || detectLanguage());
  }, [settings?.theme, settings?.language, i18n]); // eslint-disable-line react-hooks/exhaustive-deps

  const connected = status.phase === "connected";
  const error = actionError ?? status.error;

  return (
    <div className="app">
      <header className="topbar">
        <div className={`state state-${status.phase}`}>
          <span className="dot" aria-hidden />
          {t(`status.${status.phase}`)}
        </div>
        <div className="spacer" />
        {status.phase !== "disconnected" ? (
          <button type="button" className="ghost" onClick={() => void disconnect()} disabled={busy}>
            {t("connect.disconnect")}
          </button>
        ) : null}
        <button type="button" className="ghost" onClick={() => setShowSettings(true)}>
          {t("settings.open")}
        </button>
      </header>

      {error ? (
        <p className="error" role="alert">
          {error}
        </p>
      ) : null}

      <main>
        {status.phase === "disconnected" ? <ConnectCard /> : null}
        <ChannelTree />
        <RoutingBar />
        <div className="controls">
          <TransmitPanel />
          <ReceivePanel />
          {connected ? <LinkPanel /> : null}
        </div>
      </main>

      {showSettings ? <SettingsDialog onClose={() => setShowSettings(false)} /> : null}
    </div>
  );
}
