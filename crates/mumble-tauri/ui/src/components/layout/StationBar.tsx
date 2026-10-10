import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  HeadphonesIcon,
  HeadphonesOffIcon,
  MicIcon,
  MicOffIcon,
} from "../../icons";
import { useAppStore, resolveRoleForServer } from "../../store";
import type { ClientRole } from "../../types";
import { InputLevelMeter } from "../audio/LevelMeter";
import OperatorControlHead from "./OperatorControlHead";
import styles from "./StationBar.module.css";

/**
 * The station bar sits at the top of the right pane (below the server tabs and
 * above the text window). It carries the mic / level-meter / headphone cluster
 * on the left and — for an Operator — a control head of live-critical controls
 * (transmit mode, PTT/whisper hotkeys, whisper target, stereo controls) that
 * flows across as many rows as needed, growing the bar and pushing the chat
 * area down.
 *
 * The server name is intentionally NOT shown here — it's already in the active
 * server tab (with a tooltip). The controls are only interactive once the user
 * is in a channel.
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

  // Role is not a reactive store field (it's module state), so resolve it for
  // the active connection into local state. The Operator control head renders
  // only for operators.
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

  if (!inChannel) {
    // Nothing to show until the user is in a channel.
    return <div className={styles.stationBar} />;
  }

  return (
    <div className={styles.stationBar}>
      {/* mic + level meter + headphone cluster (left) */}
      <div className={styles.cluster}>
        <button
          type="button"
          className={`${styles.voiceToggle} ${isActive ? styles.voiceActive : styles.voiceMuted}`}
          onClick={toggleMute}
          title={isActive ? t("channelSidebar.mute") : t("channelSidebar.unmute")}
          aria-label={isActive ? t("channelSidebar.mute") : t("channelSidebar.unmute")}
        >
          {isActive ? <MicIcon width={18} height={18} /> : <MicOffIcon width={18} height={18} />}
        </button>
        <div className={styles.meterWrap}>
          <InputLevelMeter />
        </div>
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
      </div>

      {/* Operator live controls (flow across rows after the cluster) */}
      {isOperator && (
        <OperatorControlHead serverId={activeServerId ?? undefined} serverKey={serverKey} />
      )}
    </div>
  );
}
