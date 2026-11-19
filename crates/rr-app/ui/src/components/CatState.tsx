import { useRef } from "react";
import { useTranslation } from "react-i18next";
import { useApp } from "../store";

/**
 * Live state of a CAT-keyed radio: link, ON AIR while this program keys it,
 * and what the radio itself reports.
 */
export function CatState({ radio }: { radio: number }) {
  const { t } = useTranslation();
  const live = useApp((s) => s.status.cat.find((c) => c.radio === radio));
  const phase = useApp((s) => s.status.phase);
  // When keying started, so the warning waits for the radio to answer (it is polled every second).
  const keyedSince = useRef<number | null>(null);
  const keyed = live?.connected === true && live.keyed;
  if (!keyed) keyedSince.current = null;
  else keyedSince.current ??= Date.now();
  const silent = keyed && live.radio_tx !== true && Date.now() - (keyedSince.current ?? Date.now()) > 1500;
  if (!live) {
    return <p className="hint">{phase === "connected" ? t("cat.notRunning") : t("cat.notRunningOffline")}</p>;
  }
  if (!live.connected) {
    return <p className="warn-text">{live.error ? `${t("cat.notConnected")}: ${live.error}` : t("cat.notConnected")}</p>;
  }
  return (
    <>
      <div className="row">
        <span className="muted grow">
          {t("cat.connected")}
          {live.radio_tx === true ? ` · ${t("cat.radioTx")}` : live.radio_tx === false ? ` · ${t("cat.radioRx")}` : ""}
        </span>
        {live.keyed ? <span className="badge on">{t("tx.onAir")}</span> : null}
      </div>
      {silent ? <p className="warn-text">{t("cat.notTransmitting")}</p> : null}
    </>
  );
}
