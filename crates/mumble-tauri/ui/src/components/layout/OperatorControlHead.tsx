import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { ActivityIcon, AudioWaveformIcon, KeyboardIcon, KeyIcon, MegaphoneIcon } from "../../icons";
import { useAppStore } from "../../store";
import type { AudioSettings, PttConfig, TransmitMode } from "../../types";
import { savePttConfig } from "../../preferencesStorage";
import {
  eventToShortcut,
  applyOperatorPttShortcut,
  applyWhisperShortcut,
  clearGlobalShortcut,
} from "../../pages/settings/shortcutHelpers";
import styles from "./OperatorControlHead.module.css";

/**
 * Operator "control head": the live-critical Voice-settings controls surfaced
 * directly in the station bar so a ham can change them in real time without
 * opening Settings. Operator role only (a Station runs Continuous and keys off
 * inbound audio, so these don't apply).
 *
 * Everything writes through the SAME backend commands the Settings tab uses:
 *   - transmit mode  -> get_audio_settings + set_audio_settings
 *   - PTT / whisper hotkeys + whisper target -> get_ptt_config + set_ptt_config
 *     (+ savePttConfig to disk), registered via the global-shortcut helpers.
 * So changing them here is identical to changing them in Settings.
 */

const MODE_PATCHES: Array<{
  value: TransmitMode;
  Icon: typeof ActivityIcon;
  patch: Partial<AudioSettings>;
}> = [
  { value: "voice_activity", Icon: ActivityIcon, patch: { transmit_mode: "voice_activity", push_to_talk: false } },
  { value: "continuous", Icon: AudioWaveformIcon, patch: { transmit_mode: "continuous", push_to_talk: false, auto_input_sensitivity: false } },
  { value: "push_to_talk", Icon: KeyboardIcon, patch: { transmit_mode: "push_to_talk", push_to_talk: true, auto_input_sensitivity: false } },
];

function effectiveMode(s: AudioSettings | null): TransmitMode {
  if (!s) return "voice_activity";
  if (s.transmit_mode === "voice_activity" && s.push_to_talk) return "push_to_talk";
  return s.transmit_mode;
}

interface Props {
  /** Active server id for scoping get/set commands (undefined = active). */
  readonly serverId: string | undefined;
  /** host:port persistence key for savePttConfig. */
  readonly serverKey: string;
}

export default function OperatorControlHead({ serverId, serverKey }: Props) {
  const { t } = useTranslation(["settings", "sidebar"]);
  const tAudio = (k: string) => (t as (x: string) => string)(`settings:audio.${k}`);
  const tAct = (k: string) => (t as (x: string) => string)(`settings:activation.${k}`);

  const channels = useAppStore((s) => s.channels);
  const ownChannel = useAppStore((s) => s.currentChannel);

  const [audio, setAudio] = useState<AudioSettings | null>(null);
  const [config, setConfig] = useState<PttConfig | null>(null);
  const [recordingPtt, setRecordingPtt] = useState(false);
  const [recordingWhisper, setRecordingWhisper] = useState(false);
  const [whisperError, setWhisperError] = useState<string | null>(null);
  const pttLoaded = useRef(false);

  // Load current audio settings + PTT config for the active server.
  useEffect(() => {
    let cancelled = false;
    pttLoaded.current = false;
    void invoke<AudioSettings>("get_audio_settings", { serverId })
      .then((a) => { if (!cancelled) setAudio(a); })
      .catch(() => {});
    void invoke<PttConfig>("get_ptt_config", { serverId })
      .then((c) => { if (!cancelled) { setConfig(c); pttLoaded.current = true; } })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [serverId]);

  const mode = effectiveMode(audio);

  // Change transmit mode -> merge patch into audio settings + persist.
  const changeMode = useCallback(
    (next: TransmitMode) => {
      const opt = MODE_PATCHES.find((p) => p.value === next);
      if (!opt || !audio) return;
      const merged = { ...audio, ...opt.patch };
      setAudio(merged);
      void invoke("set_audio_settings", { settings: merged, serverId }).catch((e) =>
        console.error("control-head set_audio_settings:", e),
      );
    },
    [audio, serverId],
  );

  // Persist PTT config (backend + disk).
  const persistConfig = useCallback(
    (next: PttConfig) => {
      void invoke("set_ptt_config", { config: next, serverId }).catch((e) =>
        console.error("control-head set_ptt_config:", e),
      );
      void savePttConfig(next, serverKey).catch(() => {});
    },
    [serverId, serverKey],
  );

  const patchConfig = useCallback(
    (p: Partial<PttConfig>) => {
      setConfig((prev) => {
        if (!prev) return prev;
        const next = { ...prev, ...p };
        persistConfig(next);
        return next;
      });
    },
    [persistConfig],
  );

  const pttHotkey = config?.ptt_hotkey ?? "";
  const whisperHotkey = config?.whisper_hotkey ?? "";

  // Register / clear the operator PTT hotkey live, mirroring PttPanel.
  useEffect(() => {
    if (pttHotkey) {
      void applyOperatorPttShortcut(pttHotkey);
      return () => { void clearGlobalShortcut(pttHotkey); };
    }
    return undefined;
  }, [pttHotkey]);

  // Register / clear the whisper hotkey live (needs a target channel too).
  const whisperReady = whisperHotkey !== "" && config?.whisper_target_channel != null;
  useEffect(() => {
    if (whisperReady) {
      void applyWhisperShortcut(whisperHotkey);
      return () => { void clearGlobalShortcut(whisperHotkey); };
    }
    return undefined;
  }, [whisperReady, whisperHotkey]);

  if (!audio || !config) return null;

  return (
    <div className={styles.head}>
      {/* Transmit mode: VOX | Continuous | PTT */}
      <div className={styles.segmented} role="radiogroup" aria-label={tAudio("activationMode")}>
        {MODE_PATCHES.map((m) => {
          const active = mode === m.value;
          const Icon = m.Icon;
          return (
            <button
              key={m.value}
              type="button"
              role="radio"
              aria-checked={active}
              className={`${styles.segBtn} ${active ? styles.segBtnActive : ""}`}
              onClick={() => changeMode(m.value)}
              title={tAct(m.value)}
            >
              <Icon width={13} height={13} />
              <span className={styles.segLabel}>{tAct(m.value)}</span>
            </button>
          );
        })}
      </div>

      {/* PTT hotkey set/unset + keystrokes */}
      <div className={styles.hotkeyGroup}>
        <KeyIcon width={13} height={13} className={styles.groupIcon} />
        {recordingPtt ? (
          <input
            className={styles.capture}
            autoFocus
            readOnly
            value={tAudio("ptt.hotkeyRecording")}
            onKeyDown={(e) => {
              e.preventDefault();
              e.stopPropagation();
              const combo = eventToShortcut(e);
              if (combo) {
                patchConfig({ ptt_hotkey: combo });
                setRecordingPtt(false);
              }
            }}
            onBlur={() => setRecordingPtt(false)}
          />
        ) : (
          <button
            type="button"
            className={styles.hkBtn}
            onClick={() => {
              if (pttHotkey) {
                void clearGlobalShortcut(pttHotkey);
                patchConfig({ ptt_hotkey: null });
              } else {
                setRecordingPtt(true);
              }
            }}
            title={tAudio("ptt.hotkey")}
          >
            {pttHotkey ? tAudio("ptt.clearHotkey") : tAudio("ptt.setHotkey")}
          </button>
        )}
        {pttHotkey && !recordingPtt && <span className={styles.keys}>{pttHotkey}</span>}
      </div>

      {/* Whisper hotkey set/unset + keystrokes */}
      <div className={styles.hotkeyGroup}>
        <MegaphoneIcon width={13} height={13} className={styles.groupIcon} />
        {recordingWhisper ? (
          <input
            className={styles.capture}
            autoFocus
            readOnly
            value={tAudio("ptt.hotkeyRecording")}
            onKeyDown={(e) => {
              e.preventDefault();
              e.stopPropagation();
              const combo = eventToShortcut(e);
              if (combo) {
                if (combo === pttHotkey) {
                  setWhisperError(tAudio("ptt.whisperHotkeyClash"));
                } else {
                  patchConfig({ whisper_hotkey: combo });
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
            className={styles.hkBtn}
            onClick={() => {
              if (whisperHotkey) {
                void clearGlobalShortcut(whisperHotkey);
                patchConfig({ whisper_hotkey: null });
                setWhisperError(null);
              } else {
                setRecordingWhisper(true);
              }
            }}
            title={tAudio("ptt.whisper")}
          >
            {whisperHotkey ? tAudio("ptt.whisperClearHotkey") : tAudio("ptt.whisperSetHotkey")}
          </button>
        )}
        {whisperHotkey && !recordingWhisper && <span className={styles.keys}>{whisperHotkey}</span>}
      </div>

      {/* Whisper target channel */}
      <select
        className={styles.channelSelect}
        value={config.whisper_target_channel ?? ""}
        onChange={(e) =>
          patchConfig({
            whisper_target_channel: e.target.value === "" ? null : Number(e.target.value),
          })
        }
        title={tAudio("ptt.whisper")}
        aria-label={tAudio("ptt.whisper")}
      >
        <option value="">{tAudio("ptt.whisperNoTarget")}</option>
        {channels
          .filter((c) => c.id !== ownChannel)
          .map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
      </select>

      {whisperError && <span className={styles.err}>{whisperError}</span>}
    </div>
  );
}
