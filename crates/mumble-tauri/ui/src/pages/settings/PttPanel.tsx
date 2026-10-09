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
import type {
  ClientRole,
  PttConfig,
  PttLine,
  SerialPortInfo,
  TransmitMode,
} from "../../types";
import { Toggle, SliderField } from "./SharedControls";
import { KeyIcon, MegaphoneIcon } from "../../icons";
import { useAppStore } from "../../store";
import { savePttConfig } from "../../preferencesStorage";
import {
  eventToShortcut,
  applyOperatorPttShortcut,
  applyWhisperShortcut,
  clearGlobalShortcut,
} from "./shortcutHelpers";
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
  ptt_hotkey: null,
  whisper_hotkey: null,
  whisper_target_channel: null,
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
  serverKey,
  role,
  transmitMode,
}: Readonly<{
  /** Server whose PTT config is being edited (null = default/active). */
  serverId: string | null;
  /** Persistence key ("host:port" or default bucket) for disk storage, so the
   *  config survives restarts and can be auto-applied on connect. */
  serverKey: string;
  /** The edited server's role, which fixes the PTT direction. */
  role: ClientRole;
  /** The edited server's effective activation mode. Operator PTT controls
   *  (footswitch line + hotkey) only apply in Push-to-Talk; in VOX/Continuous
   *  the operator isn't gating transmit, so they're hidden. A Station is driven
   *  by the remote operator + VOX regardless of mode, so its controls always
   *  show. */
  transmitMode: TransmitMode;
}>) {
  const { t } = useTranslation("settings");
  // The call sites use `ptt.*` keys, but those live under the `audio.*` group
  // in settings.json (same as the surrounding Voice panel). Prefix with
  // `audio.` so lookups resolve instead of echoing the raw key.
  const tStr = (key: string) => (t as (k: string) => string)(`audio.${key}`);
  const [config, setConfig] = useState<PttConfig>(DEFAULT_PTT);
  const [ports, setPorts] = useState<SerialPortInfo[]>([]);
  const loaded = useRef(false);

  // The settings-layer role is the direction. Keep the stored config's role in
  // step with the server's role so the backend opens the right line direction.
  const pttRole: PttConfig["role"] = role === "station" ? "station" : "operator";
  const lineOptions = pttRole === "station" ? OUTPUT_LINES : INPUT_LINES;

  // Gating (CONTEXT §14d):
  //  - A Station is keyed by the remote operator + inbound-audio VOX, so its
  //    serial output config is relevant in every activation mode.
  //  - An Operator only *gates* transmit with PTT in Push-to-Talk mode, so the
  //    footswitch line + the software hotkey are shown only then. In
  //    VOX/Continuous the mic opens on its own and there is nothing to key.
  const isStation = pttRole === "station";
  const showOperatorPtt = !isStation && transmitMode === "push_to_talk";

  // On-air: light the indicator while this client's own audio is live.
  const onAir = useAppStore(
    (s) => s.ownSession != null && s.talkingSessions.has(s.ownSession),
  );

  // Whisper is an Operator feature and independent of serial PTT.
  const showWhisper = !isStation;

  // --- Operator software PTT hotkey -----------------------------------
  const [recording, setRecording] = useState(false);
  // --- Operator Whisper hotkey ----------------------------------------
  const [recordingWhisper, setRecordingWhisper] = useState(false);
  const [whisperError, setWhisperError] = useState<string | null>(null);
  // Channels available as whisper targets (id -> name), loaded from the store.
  const channels = useAppStore((s) => s.channels);
  const ownChannel = useAppStore((s) => s.currentChannel);

  // Register (or clear) the global hotkey whenever it changes. Usable whenever
  // an operator has a hotkey set — INDEPENDENT of serial PTT (`config.enabled`):
  // the hotkey opens the Mumble mic and keys the remote via `operator_ptt`
  // regardless of whether a serial rig controller is in use. (Previously gated
  // on `config.enabled`, which left keyboard-only operators with a dead hotkey.)
  const hotkey = config.ptt_hotkey ?? "";
  const hotkeyActive = showOperatorPtt && hotkey !== "";
  useEffect(() => {
    if (hotkeyActive) {
      void applyOperatorPttShortcut(hotkey);
      return () => {
        void clearGlobalShortcut(hotkey);
      };
    }
    return undefined;
  }, [hotkeyActive, hotkey]);

  // Register (or clear) the whisper hotkey. Usable when an operator has both a
  // hotkey and a target channel set. Independent of serial-PTT enabled/mode.
  const whisperHotkey = config.whisper_hotkey ?? "";
  const whisperActive =
    showWhisper && whisperHotkey !== "" && config.whisper_target_channel != null;
  useEffect(() => {
    if (whisperActive) {
      void applyWhisperShortcut(whisperHotkey);
      return () => {
        void clearGlobalShortcut(whisperHotkey);
      };
    }
    return undefined;
  }, [whisperActive, whisperHotkey]);

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
      // Backend (live controller) + disk (so it auto-applies on next connect).
      invoke("set_ptt_config", { config, serverId: sid }).catch((e) =>
        console.error("save PTT config:", e),
      );
      savePttConfig(config, serverKey).catch((e) =>
        console.error("persist PTT config:", e),
      );
    }, 400);
    return () => clearTimeout(timer);
  }, [config, serverId, serverKey]);

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
            {config.enabled && onAir && (
              <span
                className={styles.pttOnAir}
                title={tStr("ptt.onAir")}
                aria-label={tStr("ptt.onAir")}
              >
                <span className={styles.pttOnAirDot} /> {tStr("ptt.onAir")}
              </span>
            )}
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

          {/* Control line. A Station always shows its key-output line. An
              Operator's footswitch sense line is only meaningful when it is
              actually gating transmit, i.e. Push-to-Talk mode. */}
          {(isStation || showOperatorPtt) && (
            <>
              <h4 className={styles.groupTitle}>
                {isStation ? tStr("ptt.keyLine") : tStr("ptt.senseLine")}
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
            </>
          )}

          {/* Operator software PTT hotkey (Push-to-Talk mode only). Held, it
              opens the Mumble mic and keys the remote station deterministically. */}
          {showOperatorPtt && (
            <>
              <h4 className={styles.groupTitle}>{tStr("ptt.hotkey")}</h4>
              <p className={styles.fieldHint}>{tStr("ptt.hotkeyHint")}</p>
              <div className={styles.fieldRow}>
                {recording ? (
                  <input
                    className={styles.select}
                    autoFocus
                    readOnly
                    value={tStr("ptt.hotkeyRecording")}
                    onKeyDown={(e) => {
                      e.preventDefault();
                      e.stopPropagation();
                      const combo = eventToShortcut(e);
                      if (combo) {
                        patch({ ptt_hotkey: combo });
                        setRecording(false);
                      }
                    }}
                    onBlur={() => setRecording(false)}
                  />
                ) : (
                  <button
                    type="button"
                    className={styles.ghostBtn}
                    onClick={() => {
                      if (hotkey) {
                        // Clear.
                        void clearGlobalShortcut(hotkey);
                        patch({ ptt_hotkey: null });
                      } else {
                        setRecording(true);
                      }
                    }}
                  >
                    {hotkey ? tStr("ptt.clearHotkey") : tStr("ptt.setHotkey")}
                  </button>
                )}
                {hotkey && !recording && (
                  <span className={styles.sliderValue}>{hotkey}</span>
                )}
              </div>
            </>
          )}

          {/* Station-only: syllable-gap hold. A station always keys on inbound
              operator audio when Serial PTT is enabled (no VOX toggle — the
              station's RX IS the audio to transmit). This hold keeps the key
              down briefly across gaps between the operator's words. */}
          {pttRole === "station" && (
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

      {/* Operator Whisper: redirect the mic into another channel while a hotkey
          is held. Independent of serial PTT, so it sits outside the enabled
          block. Operator role only. */}
      {showWhisper && (
        <>
          <h4 className={styles.groupTitle}>
            <MegaphoneIcon width={15} height={15} /> {tStr("ptt.whisper")}
          </h4>
          <p className={styles.fieldHint}>{tStr("ptt.whisperHint")}</p>

          {/* Target channel */}
          <select
            className={styles.select}
            value={config.whisper_target_channel ?? ""}
            onChange={(e) =>
              patch({
                whisper_target_channel: e.target.value === "" ? null : Number(e.target.value),
              })
            }
          >
            <option value="">{tStr("ptt.whisperNoTarget")}</option>
            {channels
              .filter((c) => c.id !== ownChannel)
              .map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
          </select>

          {/* Whisper hotkey */}
          <div className={styles.fieldRow}>
            {recordingWhisper ? (
              <input
                className={styles.select}
                autoFocus
                readOnly
                value={tStr("ptt.hotkeyRecording")}
                onKeyDown={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  const combo = eventToShortcut(e);
                  if (combo) {
                    // The whisper hotkey must differ from the PTT hotkey - both
                    // are global OS shortcuts and would otherwise collide.
                    if (combo === (config.ptt_hotkey ?? "")) {
                      setWhisperError(tStr("ptt.whisperHotkeyClash"));
                    } else {
                      patch({ whisper_hotkey: combo });
                      setWhisperError(null);
                    }
                    setRecordingWhisper(false);
                  }
                }}
                onBlur={() => setRecordingWhisper(false)}
              />
            ) : (
              <button
                type="button"
                className={styles.ghostBtn}
                onClick={() => {
                  if (whisperHotkey) {
                    void clearGlobalShortcut(whisperHotkey);
                    patch({ whisper_hotkey: null });
                    setWhisperError(null);
                  } else {
                    setRecordingWhisper(true);
                  }
                }}
              >
                {whisperHotkey ? tStr("ptt.whisperClearHotkey") : tStr("ptt.whisperSetHotkey")}
              </button>
            )}
            {whisperHotkey && !recordingWhisper && (
              <span className={styles.sliderValue}>{whisperHotkey}</span>
            )}
          </div>
          {whisperError && <p className={styles.fieldHint}>{whisperError}</p>}
        </>
      )}
    </section>
  );
}
