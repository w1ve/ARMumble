import { ArrowRightIcon, BellIcon, BellOffIcon, DatabaseIcon, FileTextIcon, FolderIcon, PinIcon, PollIcon, PopoutIcon, SearchIcon } from "../../icons";
import { useTranslation } from "react-i18next";
import { isMobile } from "../../utils/platform";
import type { KeyTrustLevel } from "../../types";
import KeyTrustIndicator from "../security/KeyTrustIndicator";
import KebabMenu, { type KebabMenuItem } from "../elements/KebabMenu";
import styles from "./ChatView.module.css";

interface ChatHeaderProps {
  readonly channelName: string;
  readonly memberCount: number;
  readonly isInChannel: boolean;
  readonly isDm?: boolean;
  readonly isPersisted?: boolean;
  readonly onJoin?: () => void;
  readonly onChannelInfoToggle?: () => void;
  readonly onChannelSearch?: () => void;
  readonly keyTrustLevel?: KeyTrustLevel;
  readonly onVerifyClick?: () => void;
  readonly onPollCreate?: () => void;
  readonly isSilenced?: boolean;
  readonly onToggleSilence?: () => void;
  /** Whether there are unseen pin changes (shows red dot on kebab & menu item). */
  readonly hasNewPins?: boolean;
  /** Called when the user opens the pinned messages panel. */
  readonly onPinnedMessages?: () => void;
  /** Whether the user has unseen completed downloads. */
  readonly hasNewDownloads?: boolean;
  /** Called when the user opens the downloads panel. */
  readonly onDownloads?: () => void;
  /** Called when the user opens the "my shared files" panel. */
  readonly onMySharedFiles?: () => void;
  /** Called when the user opens the saved document library. */
  readonly onOpenDocLibrary?: () => void;
  /** Called when the user clicks "Pop out DM" (only meaningful when isDm). */
  readonly onPopOutDm?: () => void;
}

function buildKebabItems({
  onPollCreate,
  isSilenced,
  onToggleSilence,
  hasNewPins,
  onPinnedMessages,
  hasNewDownloads,
  onDownloads,
  onMySharedFiles,
  onOpenDocLibrary,
  onChannelSearch,
  onChannelInfoToggle,
  t,
}: Pick<ChatHeaderProps, "onPollCreate" | "isSilenced" | "onToggleSilence" | "hasNewPins" | "onPinnedMessages" | "hasNewDownloads" | "onDownloads" | "onMySharedFiles" | "onOpenDocLibrary" | "onChannelSearch" | "onChannelInfoToggle"> & { t: (key: string) => string }): KebabMenuItem[] {
  const items: KebabMenuItem[] = [];
  if (onChannelSearch) {
    items.push({
      id: "channel-search",
      label: t("header.searchInChannel"),
      icon: <SearchIcon width={16} height={16} />,
      onClick: onChannelSearch,
    });
  }
  if (onChannelInfoToggle) {
    items.push({
      id: "channel-info",
      label: t("header.channelInfo"),
      icon: <FolderIcon width={16} height={16} />,
      onClick: onChannelInfoToggle,
    });
  }
  if (onPinnedMessages) {
    items.push({
      id: "pinned-messages",
      label: t("header.pinnedMessages"),
      icon: <PinIcon width={15} height={15} />,
      badge: hasNewPins,
      onClick: onPinnedMessages,
    });
  }
  if (onDownloads) {
    items.push({
      id: "downloads",
      label: t("header.downloads"),
      icon: <FolderIcon width={16} height={16} />,
      badge: hasNewDownloads,
      onClick: onDownloads,
    });
  }
  if (onMySharedFiles) {
    items.push({
      id: "my-shared-files",
      label: t("header.mySharedFiles"),
      icon: <FileTextIcon width={15} height={15} />,
      onClick: onMySharedFiles,
    });
  }
  if (onPollCreate) {
    items.push({
      id: "create-poll",
      label: t("header.createPoll"),
      icon: <PollIcon width={16} height={16} />,
      onClick: onPollCreate,
    });
  }
  if (onOpenDocLibrary) {
    items.push({
      id: "browse-documents",
      label: t("header.browseDocuments"),
      icon: <FileTextIcon width={16} height={16} />,
      onClick: onOpenDocLibrary,
    });
  }
  if (onToggleSilence) {
    items.push({
      id: "toggle-silence",
      label: isSilenced ? t("header.unmuteChannel") : t("header.muteChannel"),
      icon: isSilenced
        ? <BellIcon width={16} height={16} />
        : <BellOffIcon width={16} height={16} />,
      active: isSilenced,
      onClick: onToggleSilence,
    });
  }
  return items;
}

export default function ChatHeader({
  channelName,
  memberCount,
  isInChannel,
  isDm,
  isPersisted,
  onJoin,
  onChannelInfoToggle,
  onChannelSearch,
  keyTrustLevel,
  onVerifyClick,
  onPollCreate,
  isSilenced,
  onToggleSilence,
  hasNewPins,
  onPinnedMessages,
  hasNewDownloads,
  onDownloads,
  onMySharedFiles,
  onOpenDocLibrary,
  onPopOutDm,
}: ChatHeaderProps) {
  const { t } = useTranslation("chat");
  const tStr = t as (key: string) => string;
  const prefix = isDm ? "@" : "#";
  const subtitle = isDm ? t("header.directMessage") : t("header.members", { count: memberCount });

  const privateBadge = isDm;

  return (
    <div className={styles.header}>
      <div className={styles.headerInfo}>
        <h2 className={styles.channelName}>
          {prefix} {channelName}
          {isPersisted && (
            <DatabaseIcon
              className={styles.persistedIcon}
              width={14}
              height={14}
              aria-label={t("header.persistedChat")}
            >
              <title>{t("header.persistedChatTooltip")}</title>
            </DatabaseIcon>
          )}
        </h2>
        {!isMobile && (<span className={styles.memberCount}>{subtitle}</span>)}
      </div>

      <div className={styles.headerActions}>
        {keyTrustLevel && !privateBadge && (
          <KeyTrustIndicator
            trustLevel={keyTrustLevel}
            onVerifyClick={onVerifyClick}
          />
        )}
        {privateBadge && onPopOutDm && (
          <button
            className={styles.serverInfoBtn}
            onClick={onPopOutDm}
            aria-label={t("header.popOutDm")}
            title={t("header.popOutDm")}
          >
            <PopoutIcon width={18} height={18} />
          </button>
        )}
        {onChannelSearch && !privateBadge && !isMobile && (
          <button
            className={styles.serverInfoBtn}
            onClick={onChannelSearch}
            aria-label={t("header.searchInChannel")}
            title={t("header.searchInChannel")}
          >
            <SearchIcon width={18} height={18} />
          </button>
        )}
        {onChannelInfoToggle && !privateBadge && !isMobile && (
          <button
            className={styles.serverInfoBtn}
            onClick={onChannelInfoToggle}
            aria-label={t("header.channelInfo")}
            title={t("header.channelInfo")}
          >
            <FolderIcon width={18} height={18} />
          </button>
        )}
        {!privateBadge && (
          <KebabMenu
            items={buildKebabItems({ onPollCreate, isSilenced, onToggleSilence, hasNewPins, onPinnedMessages, hasNewDownloads, onDownloads, onMySharedFiles, onOpenDocLibrary, onChannelSearch: isMobile ? onChannelSearch : undefined, onChannelInfoToggle: isMobile ? onChannelInfoToggle : undefined, t: tStr })}
            ariaLabel={t("header.channelOptions")}
            badge={hasNewPins || hasNewDownloads}
          />
        )}
        {!isInChannel && onJoin && (
          <button className={styles.joinBtn} onClick={onJoin}>
            {isMobile ? <ArrowRightIcon width={18} height={18} /> : t("header.joinChannel")}
          </button>
        )}
      </div>
    </div>
  );
}
