import { useState, useEffect, useCallback, useRef } from "react";
import { useTranslation } from "react-i18next";
import { ImageEditor } from "./ImageEditor";
import { FileDropZone } from "../../components/elements/FileDropZone";
import styles from "./BannerEditorModal.module.css";
import settingsStyles from "./SettingsPage.module.css";

interface AvatarEditorModalProps {
  avatar: string | null;
  onConfirm: (avatar: string | null) => void;
  onCancel: () => void;
}

export function AvatarEditorModal({
  avatar,
  onConfirm,
  onCancel,
}: Readonly<AvatarEditorModalProps>) {
  const [localImage, setLocalImage] = useState<string | undefined>(avatar ?? undefined);

  const [editorImage, setEditorImage] = useState<string | null>(null);
  const [showUnsavedHint, setShowUnsavedHint] = useState(false);
  const flashTimerRef = useRef<ReturnType<typeof setTimeout>>(undefined);

  const hasChanges = useCallback(() => {
    return (localImage ?? null) !== (avatar ?? null);
  }, [avatar, localImage]);

  const tryClose = useCallback(() => {
    if (!hasChanges()) {
      onCancel();
      return;
    }
    setShowUnsavedHint(true);
    clearTimeout(flashTimerRef.current);
    flashTimerRef.current = setTimeout(() => setShowUnsavedHint(false), 3000);
  }, [hasChanges, onCancel]);

  useEffect(() => {
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") tryClose();
    };
    document.addEventListener("keydown", handleKey);
    return () => document.removeEventListener("keydown", handleKey);
  }, [tryClose]);

  useEffect(() => () => clearTimeout(flashTimerRef.current), []);

  const handleFileSelect = useCallback((file: File) => {
    const reader = new FileReader();
    reader.onload = () => {
      const dataUrl = reader.result as string;
      if (file.type === "image/gif") {
        setLocalImage(dataUrl);
      } else {
        setEditorImage(dataUrl);
      }
    };
    reader.readAsDataURL(file);
  }, []);

  const handleEditorConfirm = useCallback((dataUrl: string) => {
    setLocalImage(dataUrl);
    setEditorImage(null);
  }, []);

  const handleApply = () => {
    onConfirm(localImage ?? null);
  };

  const { t } = useTranslation("settings");

  const handleRemoveImage = () => {
    setLocalImage(undefined);
  };

  return (
    <div className={settingsStyles.editorOverlay} onClick={tryClose}>
      <div
        className={styles.modal}
        onClick={(e) => e.stopPropagation()}
      >
        <h3 className={styles.title}>{t("avatarEditor.title")}</h3>

        {/* Preview */}
        <div className={styles.avatarPreview}>
          {localImage ? (
            <img src={localImage} alt={t("avatarEditor.previewAlt")} />
          ) : (
            <span className={styles.avatarPlaceholder}>{t("avatarEditor.noImage")}</span>
          )}
        </div>

        {/* Image picker */}
        <div className={styles.tabContent}>
          <FileDropZone
            accept="image/png,image/jpeg,image/webp,image/gif"
            onFile={handleFileSelect}
            label={t("avatarEditor.dropLabel")}
            shape="circle"
            preview={
              localImage ? (
                <img src={localImage} alt={t("avatarEditor.imgAlt")} />
              ) : undefined
            }
            onRemove={localImage ? handleRemoveImage : undefined}
          />
        </div>

        {/* Actions */}
        <div className={styles.actions}>
          <button
            type="button"
            className={settingsStyles.ghostBtn}
            onClick={onCancel}
          >
            {t("avatarEditor.discardBtn")}
          </button>
          <div className={styles.applyWrapper}>
            {showUnsavedHint && (
              <div className={styles.unsavedBubble}>
                {t("avatarEditor.unsavedHint")}
              </div>
            )}
            <button
              type="button"
              className={`${settingsStyles.applyBtn} ${showUnsavedHint ? styles.applyFlash : ""}`}
              onClick={handleApply}
            >
              {t("avatarEditor.applyBtn")}
            </button>
          </div>
        </div>
      </div>

      {editorImage && (
        <ImageEditor
          src={editorImage}
          cropShape="circle"
          targetWidth={128}
          targetHeight={128}
          maxBytes={100_000}
          onConfirm={handleEditorConfirm}
          onCancel={() => setEditorImage(null)}
        />
      )}
    </div>
  );
}
