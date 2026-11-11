import { useState } from "react";
import { useTranslation } from "react-i18next";
import { eventToAccelerator } from "../util";

export function HotkeyInput({ label, value, onChange }: { label: string; value: string; onChange: (v: string) => void }) {
  const { t } = useTranslation();
  const [recording, setRecording] = useState(false);
  return (
    <div className="field hotkey">
      <span className="field-label">{label}</span>
      <div className="row">
        <button
          type="button"
          className={`key-box${recording ? " recording" : ""}`}
          onClick={() => setRecording(true)}
          onBlur={() => setRecording(false)}
          onKeyDown={(e) => {
            if (!recording) return;
            e.preventDefault();
            if (e.code === "Escape") {
              setRecording(false);
              return;
            }
            const acc = eventToAccelerator(e.nativeEvent);
            if (acc) {
              onChange(acc);
              setRecording(false);
            }
          }}
        >
          {recording ? t("settings.record") : value || "—"}
        </button>
        <button type="button" className="ghost" onClick={() => onChange("")} disabled={!value}>
          {t("settings.clear")}
        </button>
      </div>
    </div>
  );
}