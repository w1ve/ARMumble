/**
 * Rig-PTT (RS-232) settings card. Advanced-only.
 *
 * Role-aware per CONTEXT §14d: an Operator senses a footswitch on an input
 * line (CTS/DSR/CD/RI) and the app sends a key command; a Station drives an
 * output line (DTR/RTS) to key its own rig, triggered by an operator command
 * and/or inbound-audio VOX. Keying is always behind the safety watchdog, so
 * the timing fields (deadman, max transmit) are the fail-safe limits.
 *
 * Loads/saves the backend `PttConfig` for the given server via the
 * `get_ptt_config` / `set_ptt_config` commands, and enumerates serial ports
 * with `list_serial_ports`.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import type { ClientRole, PttConfig, PttLine, SerialPortInfo } from "../../types";
import { Toggle, SliderField } from "./SharedControls";
import { KeyIcon } from "../../icons";
import styles from "./SettingsPage.module.css";

/** Default config used until the backend value loads. */
const DEFAULT_PTT: PttConfig = {
  enabled: false,
  role: "operator",
  port: "",
  line: "dtr",
  active_low: false,
  vox: false,
  vox_hang_ms: 150,
  lead_in_ms: 150,
  hang_ms: 250,
  deadman_ms: 2000,
  max_tx_secs: 120,
};

/** Lines a Station may drive (writable outputs). */
const OUTPUT_LINES: PttLine[] = ["dtr", "rts"];
/** Lines an Operator may sense (readable inputs). */
const INPUT_LINES: PttLine[] = ["cts", "dsr", "cd", "ri"];

const LINE_LABELS: Record<PttLine, string> = {
  dtr: "DTR (pin 4)",
  rts: "RTS (pin 7)",
  cts: "CTS (pin 8)",
  dsr: "DSR (pin 6)",
  cd: "CD (pin 1)",
  ri: "RI (pin 9)",
};

export function PttPanel({
  serverId,
  role,
}: Readonly<{
  /** Server whose PTT config is being edited (null = default/active). */
  serverId: string | null;
  /** The edited server's role, which fixes the PTT direction. */
  role: ClientRole;
}>) {
  const { t } = useTranslation("settings");
  const tStr = t as (key: string) => string;
  const [config, setConfig] = useState<PttConfig>(DEFAULT_PTT);
  const [ports, setPorts] = useState<SerialPortInfo[]>([]);
  const loaded = useRef(false);

  // The settings-layer role is the direction. Keep the stored config's role in
  // step with the server's role so the backend opens the right line direction.
  const pttRole: PttConfig["role"] = role === "station" ? "station" : "operator";
  const lineOptions = pttRole === "station" ? OUTPUT_LINES : INPUT_LINES;

  // Load the backend config + port list on mount / server change.
  useEffect(() => {
    loaded.current = false;
    const sid = serverId ?? undefined;
    void Promise.all([
      invoke<PttConfig>("get_ptt_config", { serverId: sid }),
      invoke<SerialPortInfo[]>("list_serial_ports"),
    ])
      .then(([cfg, p]) => {
        // Force the role to match the server; pick a valid default line if the
        // stored one is for the wrong direction.
        const lines = cfg.role === "station" ? OUTPUT_LINES : INPUT_LINES;
        const nextRole: PttConfig["role"] = role === "station" ? "station" : "operator";
        const validLines = nextRole === "station" ? OUTPUT_LINES : INPUT_LINES;
        const line = validLines.includes(cfg.line) ? cfg.line : validLines[0];
        void lines; // (kept for clarity)
        const normalised = { ...cfg, role: nextRole, line };
        setConfig(normalised);
        setPorts(p);
        loaded.current = true;
        // If the role/line had to be corrected, or PTT is enabled, re-apply so
        // the backend controller reflects what is shown (and a Station starts
        // keying) without requiring the user to toggle a field.
        if (normalised.enabled || cfg.role !== nextRole || cfg.line !== line) {
          invoke("set_ptt_config", { config: normalised, serverId: serverId ?? undefined }).catch(
            (e) => console.error("apply PTT config:", e),
          );
        }
      })
      .catch((e) => console.error("load PTT config:", e));
  }, [serverId, role]);

  // Debounced save on change.
  useEffect(() => {
    if (!loaded.current) return;
    const sid = serverId ?? undefined;
    const timer = setTimeout(() => {
      invoke("set_ptt_config", { config, serverId: sid }).catch((e) =>
        console.error("save PTT config:", e),
      );
    }, 400);
    return () => clearTimeout(timer);
  }, [config, serverId]);

  const patch = useCallback((p: Partial<PttConfig>) => {
    setConfig((prev) => ({ ...prev, ...p }));
  }, []);

  const refreshPorts = useCallback(() => {
    invoke<SerialPortInfo[]>("list_serial_ports")
      .then(setPorts)
      .catch((e) => console.error("list serial ports:", e));
  }, []);

  return (
    <section className={styles.section}>
      <div className={styles.toggleRow}>
        <div className={styles.toggleInfo}>
          <h3 className={styles.sectionTitle}>
            <KeyIcon width={16} height={16} /> {tStr("ptt.title")}
          </h3>
          <p className={styles.fieldHint}>
            {pttRole === "station" ? tStr("ptt.stationHint") : tStr("ptt.operatorHint")}
          </p>
        </div>
        <Toggle checked={config.enabled} onChange={() => patch({ enabled: !config.enabled })} />
      </div>

      {config.enabled && (
        <>
          {/* Serial port */}
          <h4 className={styles.groupTitle}>{tStr("ptt.serialPort")}</h4>
          <div className={styles.fieldRow}>
            <select
              className={styles.select}
              value={config.port}
              onChange={(e) => patch({ port: e.target.value })}
            >
              <option value="">{tStr("ptt.noPort")}</option>
              {ports.map((p) => (
                <option key={p.name} value={p.name}>
                  {p.label}
                </option>
              ))}
            </select>
            <button type="button" className={styles.ghostBtn} onClick={refreshPorts}>
              {tStr("ptt.refreshPorts")}
            </button>
          </div>

          {/* Control line */}
          <h4 className={styles.groupTitle}>
            {pttRole === "station" ? tStr("ptt.keyLine") : tStr("ptt.senseLine")}
          </h4>
          <select
            className={styles.select}
            value={config.line}
            onChange={(e) => patch({ line: e.target.value as PttLine })}
          >
            {lineOptions.map((l) => (
              <option key={l} value={l}>
                {LINE_LABELS[l]}
              </option>
            ))}
          </select>

          <div className={styles.toggleRow}>
            <div className={styles.toggleInfo}>
              <span className={styles.fieldLabel}>{tStr("ptt.activeLow")}</span>
            </div>
            <Toggle
              checked={config.active_low}
              onChange={() => patch({ active_low: !config.active_low })}
            />
          </div>

          {/* Station-only: VOX + its syllable-gap hold */}
          {pttRole === "station" && (
            <>
              <div className={styles.toggleRow}>
                <div className={styles.toggleInfo}>
                  <span className={styles.fieldLabel}>{tStr("ptt.vox")}</span>
                </div>
                <Toggle checked={config.vox} onChange={() => patch({ vox: !config.vox })} />
              </div>
              {config.vox && (
                <SliderField
                  label={tStr("ptt.voxHang")}
                  hint={tStr("ptt.voxHangHint")}
                  min={0}
                  max={300}
                  step={10}
                  value={config.vox_hang_ms}
                  onChange={(v) => patch({ vox_hang_ms: Math.round(v) })}
                  format={(v) => `${Math.round(v)} ms`}
                />
              )}
            </>
          )}

          {/* Timing */}
          <h4 className={styles.groupTitle}>{tStr("ptt.timing")}</h4>
          <SliderField
            label={tStr("ptt.leadIn")}
            min={0}
            max={500}
            step={10}
            value={config.lead_in_ms}
            onChange={(v) => patch({ lead_in_ms: Math.round(v) })}
            format={(v) => `${Math.round(v)} ms`}
          />
          <SliderField
            label={tStr("ptt.hang")}
            min={0}
            max={2000}
            step={10}
            value={config.hang_ms}
            onChange={(v) => patch({ hang_ms: Math.round(v) })}
            format={(v) => `${Math.round(v)} ms`}
          />

          {/* Safety limits */}
          <h4 className={styles.groupTitle}>{tStr("ptt.safety")}</h4>
          <p className={styles.fieldHint}>{tStr("ptt.safetyHint")}</p>
          <SliderField
            label={tStr("ptt.deadman")}
            min={500}
            max={10000}
            step={100}
            value={config.deadman_ms}
            onChange={(v) => patch({ deadman_ms: Math.round(v) })}
            format={(v) => `${(v / 1000).toFixed(1)} s`}
          />
          <SliderField
            label={tStr("ptt.maxTx")}
            min={10}
            max={600}
            step={5}
            value={config.max_tx_secs}
            onChange={(v) => patch({ max_tx_secs: Math.round(v) })}
            format={(v) => `${Math.round(v)} s`}
          />
        </>
      )}
    </section>
  );
}
