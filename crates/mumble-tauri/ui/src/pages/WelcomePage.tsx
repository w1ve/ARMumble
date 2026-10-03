import { useState, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { completeSetup } from "../preferencesStorage";
import BrandLogo from "../components/elements/BrandLogo";
import type { UserMode, ClientRole } from "../types";
import styles from "./WelcomePage.module.css";

export default function WelcomePage({ onComplete }: Readonly<{ onComplete?: () => void }>) {
  const { t } = useTranslation(["server", "common"]);
  const navigate = useNavigate();
  const [mode, setMode] = useState<UserMode>("normal");
  const [role, setRole] = useState<ClientRole>("operator");
  const [username, setUsername] = useState("");
  const [saving, setSaving] = useState(false);

  const handleSubmit = useCallback(
    async (e: { preventDefault: () => void }) => {
      e.preventDefault();
      if (!username.trim()) return;
      setSaving(true);
      await completeSetup(mode, username.trim(), role);
      // Generate a default certificate for TLS client auth.
      try {
        await invoke("generate_certificate", { label: "default" });
      } catch {
        // Non-fatal - the user can still connect anonymously.
      }
      onComplete?.();
      navigate("/", { replace: true });
    },
    [mode, role, username, navigate, onComplete],
  );

  return (
    <div className={styles.page}>
      <div className={styles.card}>
        {/* Logo */}
        <div className={styles.logo}>
          <BrandLogo size={52} className={styles.logoIcon} />
          <h1 className={styles.title}>{t("common:brand")}</h1>
          <p className={styles.welcomeSubtitle}>{t("welcomeTagline")}</p>
        </div>

        <form onSubmit={handleSubmit}>
          {/* Role: operator or station */}
          <div className={styles.field}>
            <span className={styles.label}>{t("welcomeRoleLabel")}</span>
            <div className={styles.modeToggle} role="radiogroup" aria-label={t("welcomeRoleLabel")}>
              <button
                type="button"
                className={`${styles.modeOption} ${role === "operator" ? styles.modeActive : ""}`}
                onClick={() => setRole("operator")}
                aria-pressed={role === "operator"}
              >
                <span className={styles.modeTitle}>{t("welcomeRoleOperator")}</span>
                <span className={styles.modeHint}>{t("welcomeRoleOperatorHint")}</span>
              </button>
              <button
                type="button"
                className={`${styles.modeOption} ${role === "station" ? styles.modeActive : ""}`}
                onClick={() => setRole("station")}
                aria-pressed={role === "station"}
              >
                <span className={styles.modeTitle}>{t("welcomeRoleStation")}</span>
                <span className={styles.modeHint}>{t("welcomeRoleStationHint")}</span>
              </button>
            </div>
          </div>

          {/* Name (callsign or station name) */}
          <div className={styles.field}>
            <label htmlFor="welcome-username" className={styles.label}>
              {role === "station" ? t("welcomeNameLabelStation") : t("welcomeNameLabelOperator")}
            </label>
            <input
              id="welcome-username"
              className={styles.input}
              type="text"
              placeholder={role === "station" ? "Elecraft-K3, etc" : "W1VE, etc"}
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              autoFocus
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
            />
          </div>

          {/* Mode selection */}
          <div className={styles.field}>
            <span className={styles.label}>{t("interface")}</span>
            <div className={styles.modeToggle} role="radiogroup">
              <button
                type="button"
                className={`${styles.modeOption} ${mode === "normal" ? styles.modeActive : ""}`}
                onClick={() => setMode("normal")}
                aria-pressed={mode === "normal"}
              >
                <span className={styles.modeTitle}>{t("mode.simple")}</span>
                <span className={styles.modeHint}>{t("mode.simpleHint")}</span>
              </button>
              <button
                type="button"
                className={`${styles.modeOption} ${mode === "expert" ? styles.modeActive : ""}`}
                onClick={() => setMode("expert")}
                aria-pressed={mode === "expert"}
              >
                <span className={styles.modeTitle}>{t("mode.advanced")}</span>
                <span className={styles.modeHint}>{t("mode.advancedHint")}</span>
              </button>
            </div>
          </div>

          <button
            className={styles.button}
            type="submit"
            disabled={!username.trim() || saving}
          >
            {saving ? t("submitting") : t("submit")}
          </button>
        </form>
      </div>
    </div>
  );
}
