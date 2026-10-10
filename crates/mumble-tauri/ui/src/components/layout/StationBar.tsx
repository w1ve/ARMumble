import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  HeadphonesIcon,
  HeadphonesOffIcon,
  MicIcon,
  MicOffIcon,
} from "../../icons";
import { useAppStore, resolveRoleForServer } from "../../store";
import type { ClientRole, SessionMeta } from "../../types";
import { InputOscilloscope } from "../audio/Oscilloscope";
import OperatorControlHead from "./OperatorControlHead";
import styles from "./StationBar.module.css";

/** Resolve a human label for a session tab (mirrors ServerTabsBar.tabLabel). */
function sessionLabel(meta: SessionMeta | undefined): string {
  if (!meta) return "";
  if (meta.label && meta.label.trim().length > 0) return meta.label;
  if (meta.host) return meta.host;
  return meta.username || "Server";
}

/**
 * The station bar sits at the top of the right pane (below the server tabs
 * and above the text window). It shows the active server's name, the
 * mic / headphone (mute / deafen) toggles, a live red input oscilloscope of
 * what is being transmitted, and — for an Operator — a full control head of
 * live-critical controls (transmit mode, PTT/whisper hotkeys, whisper target)
 * so a ham can change them in real time without opening Settings.
 *
 * The voice toggles are only interactive once the user is in a channel
 * (currentChannel != null); otherwise they are hidden, matching the old
 * sidebar behaviour.
 */
export default function StationBar() {
  const { t } = useTranslation(["sidebar", "common"]);
  const sessions = useAppStore((s) => s.sessions);
  const activeServerId = useAppStore((s) => s.activeServerId);
  const voiceState = useAppStore((s) => s.voiceState);
  const toggleMute = useAppStore((s) => s.toggleMute);
  const toggleDeafen = useAppStore((s) => s.toggleDeafen);
  const currentChannel = useAppStore((s) => s.currentChannel);

  const activeSession = useMemo(
    () => sessions.find((s) => s.id === activeServerId),
    [sessions, activeServerId],
  );
  const label = sessionLabel(activeSession);

  // Role is not a reactive store field (it's module state), so resolve it for
  // the active connection into local state. Re-resolves when the active
  // session changes. The Operator control head renders only for operators.
  const [role, setRole] = useState<ClientRole | null>(null);
  useEffect(() => {
    let cancelled = false;
    if (!activeSession) {
      setRole(null);
      return;
    }
    void resolveRoleForServer(activeSession.host, activeSession.port)
      .then((r) => { if (!cancelled) setRole(r); })
      .catch(() => { if (!cancelled) setRole("operator"); });
    return () => { cancelled = true; };
  }, [activeSession]);

  const isActive = voiceState === "active";
  const isInactive = voiceState === "inactive";
  const inChannel = currentChannel != null;
  const isOperator = role === "operator";

  const serverKey = activeSession ? `${activeSession.host}:${activeSession.port}` : "";

  const micBtn = (
    <button
      type="button"
      className={`${styles.voiceToggle} ${isActive ? styles.voiceActive : styles.voiceMuted}`}
      onClick={toggleMute}
      title={isActive ? t("channelSidebar.mute") : t("channelSidebar.unmute")}
      aria-label={isActive ? t("channelSidebar.mute") : t("channelSidebar.unmute")}
    >
      {isActive ? <MicIcon width={18} height={18} /> : <MicOffIcon width={18} height={18} />}
    </button>
  );

  const headphoneBtn = (
    <button
      type="button"
      className={`${styles.voiceToggle} ${isInactive ? styles.voiceMuted : styles.voiceActive}`}
      onClick={toggleDeafen}
      title={isInactive ? t("channelSidebar.enableVoice") : t("channelSidebar.disableVoice")}
      aria-label={isInactive ? t("channelSidebar.enableVoice") : t("channelSidebar.disableVoice")}
    >
      {isInactive ? (
        <HeadphonesOffIcon width={18} height={18} />
      ) : (
        <HeadphonesIcon width={18} height={18} />
      )}
    </button>
  );

  return (
    <div className={styles.stationBar}>
      <div className={styles.serverInfo} title={label}>
        <span className={styles.serverName}>{label}</span>
      </div>

      {inChannel && (
        <div className={styles.cluster}>
          {micBtn}
          <div className={styles.scopeWrap}>
            <InputOscilloscope />
          </div>
          {headphoneBtn}
        </div>
      )}

      {inChannel && isOperator && (
        <OperatorControlHead serverId={activeServerId ?? undefined} serverKey={serverKey} />
      )}
    </div>
  );
}
