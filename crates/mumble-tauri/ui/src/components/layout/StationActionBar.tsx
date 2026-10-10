import { lazy, Suspense, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  InfoIcon,
  LogoutIcon,
  RecordIcon,
  SettingsIcon,
  ShieldIcon,
} from "../../icons";
import { useAppStore } from "../../store";
import { getPreferences } from "../../preferencesStorage";
import { PERM_WRITE } from "../../utils/permissions";
import styles from "./StationActionBar.module.css";

const RecordingModal = lazy(() => import("../sidebar/RecordingModal"));

interface StationActionBarProps {
  /** Toggle the server info panel. */
  readonly onServerInfoToggle?: () => void;
}

/**
 * The bottom action row of the right pane: server info, settings, admin,
 * optional record, and disconnect. Relocated out of the channel sidebar's
 * voice panel so the sidebar is purely the channel/member tree + self row.
 */
export default function StationActionBar({ onServerInfoToggle }: Readonly<StationActionBarProps>) {
  const { t } = useTranslation(["sidebar", "common"]);
  const navigate = useNavigate();
  const disconnect = useAppStore((s) => s.disconnect);
  const channels = useAppStore((s) => s.channels);
  const voiceState = useAppStore((s) => s.voiceState);

  const [devMode, setDevMode] = useState(false);
  const [showRecordingModal, setShowRecordingModal] = useState(false);

  useEffect(() => {
    getPreferences().then((prefs) => setDevMode(prefs.userMode === "developer"));
  }, []);

  // Write permission on the root channel (id 0) = traditional admin indicator.
  const isAdmin = useMemo(() => {
    const root = channels.find((ch) => ch.id === 0);
    return root?.permissions != null && (root.permissions & PERM_WRITE) !== 0;
  }, [channels]);

  return (
    <div className={styles.actionBar}>
      {onServerInfoToggle && (
        <button
          type="button"
          className={styles.iconBtn}
          onClick={onServerInfoToggle}
          title={t("channelSidebar.serverInfo")}
          aria-label={t("channelSidebar.serverInfo")}
        >
          <InfoIcon width={18} height={18} />
        </button>
      )}
      <button
        type="button"
        className={styles.iconBtn}
        onClick={() => navigate("/settings")}
        title={t("channelSidebar.audioSettings")}
        aria-label={t("channelSidebar.audioSettings")}
      >
        <SettingsIcon width={18} height={18} />
      </button>
      {isAdmin && (
        <button
          type="button"
          className={styles.iconBtn}
          onClick={() => navigate("/admin")}
          title={t("channelSidebar.adminPanel")}
          aria-label={t("channelSidebar.adminPanel")}
        >
          <ShieldIcon width={18} height={18} />
        </button>
      )}
      {devMode && voiceState !== "inactive" && (
        <button
          type="button"
          className={`${styles.iconBtn} ${showRecordingModal ? styles.activeBtn : ""}`}
          onClick={() => setShowRecordingModal(true)}
          title={t("channelSidebar.recordAudio")}
          aria-label={t("channelSidebar.recordAudio")}
        >
          <RecordIcon width={18} height={18} />
        </button>
      )}
      <button
        type="button"
        className={styles.disconnectBtn}
        onClick={disconnect}
        title={t("channelSidebar.disconnect")}
      >
        <LogoutIcon width={16} height={16} />
        {t("channelSidebar.disconnect")}
      </button>

      {showRecordingModal && (
        <Suspense fallback={null}>
          <RecordingModal onClose={() => setShowRecordingModal(false)} />
        </Suspense>
      )}
    </div>
  );
}
