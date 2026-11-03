import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { ActivityIcon, KeyboardIcon, MegaphoneIcon, MicIcon } from "../../icons";
import { useAppStore } from "../../store";
import type { AudioSettings, PttConfig, StereoMode, TransmitMode } from "../../types";
import { savePttConfig } from "../../preferencesStorage";
import Collapsible from "./Collapsible";
import styles from "./MobileRadioControls.module.css";

/**
 * Mobile operator controls. A phone has no keyboard, so this is a trimmed,
 * touch-first version of OperatorControlHead:
 *   - transmit mode is VOX or PTT only (no Continuous, no hotkeys);
 *   - PTT is a large press-and-hold TRANSMIT button (hold = key, release = stop)
 *     instead of a keyboard shortcut;
 *   - Whisper is a press-and-hold WHISPER button plus a target-channel select
 *     (no hotkey);
 *   - the stereo rendering controls (Mix/LR, Swap, Pan) are kept.
 *
 * Everything writes through the SAME backend commands as the desktop controls
 * (set_audio_settings, operator_ptt, whisper_target, set_ptt_config), so a
 * phone and a desktop drive the pipeline identically.
 */

/** VOX / PTT only for mobile. */
const MODE_PATCHES: Array<{
  value: TransmitMode;
  Icon: typeof ActivityIcon;
  patch: Partial<AudioSettings>;
}> = [
  { value: "voice_activity", Icon: ActivityIcon, patch: { transmit_mode: "voice_activity", push_to_talk: false } },
  { value: "push_to_talk", Icon: KeyboardIcon, patch: { transmit_mode: "push_to_talk", push_to_talk: true, auto_input_sensitivity: false } },
];

function effectiveMode(s: AudioSettings | null): TransmitMode {
  if (!s) return "voice_activity";
  if (s.transmit_mode === "voice_activity" && s.push_to_talk) return "push_to_talk";
  // Continuous isn't offered on mobile; present it as VOX.
  if (s.transmit_mode === "continuous") return "voice_activity";
  return s.transmit_mode;
}

interface Props {
  readonly serverId: string | undefined;
  readonly serverKey: string;
}

export default function MobileRadioControls({ serverId, serverKey }: Props) {
  const { t } = useTranslation(["settings", "sidebar"]);
  const tAudio = (k: string) => (t as (x: string) => string)(`settings:audio.${k}`);
  const tAct = (k: string) => (t as (x: string) => string)(`settings:activation.${k}`);

  const channels = useAppStore((s) => s.channels);
  const ownChannel = useAppStore((s) => s.currentChannel);

  const [audio, setAudio] = useState<AudioSettings | null>(null);
  const [config, setConfig] = useState<PttConfig | null>(null);
  const [pttHeld, setPttHeld] = useState(false);
  const [whisperHeld, setWhisperHeld] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void invoke<AudioSettings>("get_audio_settings", { serverId })
      .then((a) => { if (!cancelled) setAudio(a); })
      .catch(() => {});
    void invoke<PttConfig>("get_ptt_config", { serverId })
      .then((c) => { if (!cancelled) setConfig(c); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [serverId]);

  const mode = effectiveMode(audio);

  const changeMode = useCallback(
    (next: TransmitMode) => {
      const opt = MODE_PATCHES.find((p) => p.value === next);
      if (!opt || !audio) return;
      const merged = { ...audio, ...opt.patch };
      setAudio(merged);
      // Leaving PTT releases any held key so we can't get stuck on-air.
      if (next !== "push_to_talk" && pttHeld) {
        setPttHeld(false);
        void invoke("operator_ptt", { down: false }).catch(() => {});
      }
      void invoke("set_audio_settings", { settings: merged, serverId }).catch((e) =>
        console.error("mobile set_audio_settings:", e),
      );
    },
    [audio, serverId, pttHeld],
  );

  const patchAudio = useCallback(
    (p: Partial<AudioSettings>) => {
      setAudio((prev) => {
        if (!prev) return prev;
        const merged = { ...prev, ...p };
        void invoke("set_audio_settings", { settings: merged, serverId }).catch((e) =>
          console.error("mobile set_audio_settings:", e),
        );
        return merged;
      });
    },
    [serverId],
  );

  const patchConfig = useCallback(
    (p: Partial<PttConfig>) => {
      setConfig((prev) => {
        if (!prev) return prev;
        const next = { ...prev, ...p };
        void invoke("set_ptt_config", { config: next, serverId }).catch(() => {});
        void savePttConfig(next, serverKey).catch(() => {});
        return next;
      });
    },
    [serverId, serverKey],
  );

  // --- Press-and-hold TRANSMIT (PTT) ---------------------------------
  // Pointer events cover mouse + touch. We capture the pointer so a release
  // outside the button still ends transmit (never get stuck on-air).
  const pttDown = useCallback((e: React.PointerEvent) => {
    e.preventDefault();
    (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
    setPttHeld(true);
    void invoke("operator_ptt", { down: true }).catch(() => {});
  }, []);
  const pttUp = useCallback((e: React.PointerEvent) => {
    (e.target as HTMLElement).releasePointerCapture?.(e.pointerId);
    setPttHeld(false);
    void invoke("operator_ptt", { down: false }).catch(() => {});
  }, []);

  // --- Press-and-hold WHISPER ----------------------------------------
  const whisperTarget = config?.whisper_target_channel ?? null;
  const whisperReady = whisperTarget != null;
  const whisperDown = useCallback((e: React.PointerEvent) => {
    e.preventDefault();
    if (!whisperReady) return;
    (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
    setWhisperHeld(true);
    void invoke("whisper_target", { active: true }).catch(() => {});
  }, [whisperReady]);
  const whisperUp = useCallback((e: React.PointerEvent) => {
    (e.target as HTMLElement).releasePointerCapture?.(e.pointerId);
    setWhisperHeld(false);
    void invoke("whisper_target", { active: false }).catch(() => {});
  }, []);

  // Release held keys if the component unmounts mid-press.
  useEffect(() => () => {
    if (pttHeld) void invoke("operator_ptt", { down: false }).catch(() => {});
    if (whisperHeld) void invoke("whisper_target", { active: false }).catch(() => {});
  }, [pttHeld, whisperHeld]);

  // --- Stereo ---------------------------------------------------------
  const [incomingStereo, setIncomingStereo] = useState(false);
  const lastStereoAt = useRef(0);
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    const id = Math.floor(Math.random() * 0xffffffffff);
    invoke("scope_subscribe", { source: "output", id }).catch(() => {});
    listen<{ source: string; channels: number }>("scope-data", (e) => {
      if (disposed || e.payload.source !== "output") return;
      if (e.payload.channels >= 2) {
        lastStereoAt.current = performance.now();
        setIncomingStereo(true);
      }
    }).then((fn) => { if (disposed) fn(); else unlisten = fn; }).catch(() => {});
    const iv = window.setInterval(() => {
      if (performance.now() - lastStereoAt.current > 1500) setIncomingStereo(false);
    }, 500);
    return () => {
      disposed = true;
      if (unlisten) unlisten();
      window.clearInterval(iv);
      invoke("scope_unsubscribe", { source: "output", id }).catch(() => {});
    };
  }, []);

  const stereoMode: StereoMode = audio?.playback_stereo_mode ?? "stereo";
  const isMix = stereoMode === "mono";
  const isSwapped = stereoMode === "swap_lr";
  const pan = audio ? (audio.right_volume ?? 1) - (audio.left_volume ?? 1) : 0;
  const setPan = useCallback(
    (p: number) => {
      const clamped = Math.max(-1, Math.min(1, p));
      const left = clamped <= 0 ? 1 : 1 - clamped;
      const right = clamped >= 0 ? 1 : 1 + clamped;
      patchAudio({ left_volume: left, right_volume: right });
    },
    [patchAudio],
  );

  const otherChannels = useMemo(
    () => channels.filter((c) => c.id !== ownChannel),
    [channels, ownChannel],
  );

  if (!audio || !config) return null;

  const isPtt = mode === "push_to_talk";

  return (
    <div className={styles.root}>
      {/* TRANSMIT fold - open by default (the one control a ham needs fast). */}
      <Collapsible title={tAudio("transmitSection")} storageKey="mobile-transmit" defaultOpen>
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
              >
                <Icon width={16} height={16} />
                <span>{tAct(m.value)}</span>
              </button>
            );
          })}
        </div>
        {/* Press-and-hold TRANSMIT (PTT mode only) */}
        {isPtt && (
          <button
            type="button"
            className={`${styles.holdBtn} ${styles.holdPtt} ${pttHeld ? styles.holdActive : ""}`}
            onPointerDown={pttDown}
            onPointerUp={pttUp}
            onPointerCancel={pttUp}
            aria-pressed={pttHeld}
          >
            <MicIcon width={22} height={22} />
            <span>{pttHeld ? tAudio("ptt.onAir") : tAudio("ptt.holdToTransmit")}</span>
          </button>
        )}
      </Collapsible>

      {/* WHISPER fold - collapsed by default. */}
      <Collapsible title={tAudio("ptt.whisperGroupTitle")} storageKey="mobile-whisper" defaultOpen={false}>
        <select
          className={styles.select}
          value={whisperTarget ?? ""}
          onChange={(e) =>
            patchConfig({
              whisper_target_channel: e.target.value === "" ? null : Number(e.target.value),
            })
          }
          aria-label={tAudio("ptt.whisper")}
        >
          <option value="">{tAudio("ptt.whisperNoTarget")}</option>
          {otherChannels.map((c) => (
            <option key={c.id} value={c.id}>{c.name}</option>
          ))}
        </select>
        <button
          type="button"
          className={`${styles.holdBtn} ${styles.holdWhisper} ${whisperHeld ? styles.holdActive : ""}`}
          onPointerDown={whisperDown}
          onPointerUp={whisperUp}
          onPointerCancel={whisperUp}
          disabled={!whisperReady}
          aria-pressed={whisperHeld}
        >
          <MegaphoneIcon width={20} height={20} />
          <span>{tAudio("ptt.holdToWhisper")}</span>
        </button>
      </Collapsible>

      {/* STEREO fold - collapsed by default. */}
      <Collapsible title={tAudio("stereo.modeLabel")} storageKey="mobile-stereo" defaultOpen={false}>
        <span className={`${styles.badge} ${incomingStereo ? styles.badgeOn : ""}`}>
          {tAudio("stereo.label")}
        </span>
        <div className={styles.segmented} role="radiogroup" aria-label={tAudio("stereo.modeLabel")}>
          <button
            type="button"
            role="radio"
            aria-checked={isMix}
            className={`${styles.segBtn} ${isMix ? styles.segBtnActive : ""}`}
            onClick={() => patchAudio({ playback_stereo_mode: "mono" })}
          >
            {tAudio("stereo.mix")}
          </button>
          <button
            type="button"
            role="radio"
            aria-checked={!isMix}
            className={`${styles.segBtn} ${!isMix ? styles.segBtnActive : ""}`}
            onClick={() => patchAudio({ playback_stereo_mode: "stereo" })}
          >
            {tAudio("stereo.lr")}
          </button>
        </div>
        <div className={styles.stereoRow}>
          <button
            type="button"
            className={`${styles.smallBtn} ${isSwapped ? styles.segBtnActive : ""}`}
            onClick={() => patchAudio({ playback_stereo_mode: isSwapped ? "stereo" : "swap_lr" })}
            disabled={isMix}
          >
            {tAudio("stereo.swap")}
          </button>
          <div className={styles.panWrap}>
            <span className={styles.panLabel}>{tAudio("stereo.pan")}</span>
            <input
              type="range"
              className={styles.pan}
              min={-1}
              max={1}
              step={0.05}
              value={pan}
              onChange={(e) => setPan(parseFloat(e.target.value))}
              disabled={isMix}
              aria-label={tAudio("stereo.pan")}
            />
          </div>
        </div>
      </Collapsible>
    </div>
  );
}
