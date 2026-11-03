import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { ActivityIcon, AudioWaveformIcon, KeyboardIcon, KeyIcon, MegaphoneIcon } from "../../icons";
import { useAppStore, sendRightChannelSource, syncOperatorHotkeys } from "../../store";
import { useAnyPeerHasSecondRadio } from "../../stationIdentity";
import type { AudioSettings, PttConfig, RightChannelSource, StereoMode, TransmitMode } from "../../types";
import { savePttConfig } from "../../preferencesStorage";
import { eventToShortcut } from "../../pages/settings/shortcutHelpers";
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
  /**
   * Layout variant.
   *  - `"bar"` (default): the original slim, wrapping single-row flow used in
   *    the mobile/narrow sidebar and anywhere space is tight.
   *  - `"panel"`: the radio-first large layout — the four control clusters
   *    (transmit, PTT hotkey, whisper, stereo) are rendered as big labeled
   *    groups in a grid, so a ham can read and hit them at a glance.
   */
  readonly variant?: "bar" | "panel";
}

export default function OperatorControlHead({ serverId, serverKey, variant = "bar" }: Props) {
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
  const whisperTarget = config?.whisper_target_channel ?? null;

  // Hotkey registration is owned SOLELY by the store (syncOperatorHotkeys);
  // this panel only EDITS the config and tells the owner the desired state, so
  // it never fights the connect-time registration or the PttPanel for the same
  // accelerator. The owner diffs and (un)registers only on a real change.
  useEffect(() => {
    void syncOperatorHotkeys(pttHotkey || null, whisperHotkey || null, whisperTarget);
  }, [pttHotkey, whisperHotkey, whisperTarget]);

  // --- Stereo playback controls (operator output side) ---------------
  // "Stereo" indicator reflects whether the INCOMING stream is actually
  // stereo. Derive it from the output scope tap's channel count (ref-counted).
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
    // Decay to "not stereo" if no stereo frames for a while (stream stopped).
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

  // Playback stereo mode: Mix = fold both channels into both ears ("mono");
  // L/R = keep channels separate ("stereo"); Swap toggles "swap_lr".
  const stereoMode: StereoMode = audio?.playback_stereo_mode ?? "stereo";
  const isMix = stereoMode === "mono";
  const isSwapped = stereoMode === "swap_lr";

  const patchAudio = useCallback(
    (p: Partial<AudioSettings>) => {
      setAudio((prev) => {
        if (!prev) return prev;
        const merged = { ...prev, ...p };
        void invoke("set_audio_settings", { settings: merged, serverId }).catch((e) =>
          console.error("control-head set_audio_settings:", e),
        );
        return merged;
      });
    },
    [serverId],
  );

  // Mix <-> L/R. Leaving Mix restores plain stereo; entering Mix is mono fold.
  const setMix = useCallback(
    (mix: boolean) => {
      patchAudio({ playback_stereo_mode: mix ? "mono" : "stereo" });
    },
    [patchAudio],
  );
  // Swap only applies in L/R; toggles between stereo and swap_lr.
  const setSwap = useCallback(
    (swap: boolean) => {
      patchAudio({ playback_stereo_mode: swap ? "swap_lr" : "stereo" });
    },
    [patchAudio],
  );
  // Pan: -1 (full left) .. 0 (center) .. +1 (full right), mapped onto the
  // left/right playback gain pair. Center = both 1.0.
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

  // Radio 1 / Radio 2 right-channel source. Operator-local toggle that tells
  // the Station (over the RR plugin channel) which radio feeds the right
  // channel. Enabled only when a Station advertised a 2nd radio.
  const dualAvailable = useAnyPeerHasSecondRadio();
  const [rightSource, setRightSource] = useState<RightChannelSource>("radio1");
  const chooseRightSource = useCallback((src: RightChannelSource) => {
    setRightSource(src);
    void sendRightChannelSource(src);
  }, []);

  // Stereo controls are only meaningful on an actual stereo stream.
  // The stereo RENDERING controls (Mix/LR, Swap, Pan) are output preferences
  // and must always be settable, even during silence - a ham sets them up
  // before traffic arrives. They are NOT gated on whether a stereo stream is
  // flowing right now. The live "Stereo" badge still reflects the real stream
  // state as an indicator, and the Radio 1/2 source toggle still depends on a
  // peer Station actually advertising a second radio.
  const stereoControlsEnabled = true;

  if (!audio || !config) return null;

  const isPanel = variant === "panel";
  // In panel mode each logical cluster is wrapped in a labeled group card;
  // in bar mode `Group` is a transparent pass-through so the original slim
  // single-row flow is unchanged.
  const Group = ({
    title,
    size,
    children,
  }: {
    title: string;
    size?: "wide" | "narrow";
    children: React.ReactNode;
  }) =>
    isPanel ? (
      <section
        className={`${styles.group} ${
          size === "wide" ? styles.groupWide : size === "narrow" ? styles.groupNarrow : ""
        }`}
      >
        <h4 className={styles.groupTitle}>{title}</h4>
        <div className={styles.groupBody}>{children}</div>
      </section>
    ) : (
      <>{children}</>
    );

  return (
    <div className={`${styles.head} ${isPanel ? styles.headPanel : ""}`}>
     <Group title={tAudio("activationMode")} size="wide">
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
     </Group>

     <Group title={tAudio("ptt.hotkey")} size="narrow">
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
                // Clearing the config triggers the sync effect, which the store
                // (sole owner) uses to unregister the OS hotkey.
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
        {/* In bar mode the key chip stays inline; in panel mode it drops to its
            own line (below) so a long combo like "Alt+G" is never clipped. */}
        {!isPanel && pttHotkey && !recordingPtt && (
          <span className={styles.keys}>{pttHotkey}</span>
        )}
      </div>
      {/* Panel mode: the set hotkey on its own line below the Set/Clear button,
          then the footswitch status line when a serial footswitch is enabled. */}
      {isPanel && pttHotkey && !recordingPtt && (
        <span className={`${styles.keys} ${styles.keysBlock}`}>{pttHotkey}</span>
      )}
      {isPanel && config.enabled && config.port && (
        <span className={styles.footswitchLine}>
          {tAudio("ptt.footswitchStatus")
            .replace("{port}", config.port)
            .replace("{line}", config.line.toUpperCase())}
        </span>
      )}
     </Group>

     <Group title={tAudio("ptt.whisperGroupTitle")}>
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
     </Group>

     <Group title={tAudio("stereo.modeLabel")}>
      {/* --- Stereo playback controls (operator output rendering) --- */}
      <span
        className={`${styles.stereoBadge} ${incomingStereo ? styles.stereoBadgeOn : ""}`}
        title={tAudio("stereo.indicatorTitle")}
      >
        {tAudio("stereo.label")}
      </span>

      {/* Mix | L/R */}
      <div
        className={`${styles.segmented} ${stereoControlsEnabled ? "" : styles.disabled}`}
        role="radiogroup"
        aria-label={tAudio("stereo.modeLabel")}
      >
        <button
          type="button"
          role="radio"
          aria-checked={isMix}
          className={`${styles.segBtn} ${isMix ? styles.segBtnActive : ""}`}
          onClick={() => stereoControlsEnabled && setMix(true)}
          disabled={!stereoControlsEnabled}
          title={tAudio("stereo.mixHint")}
        >
          {tAudio("stereo.mix")}
        </button>
        <button
          type="button"
          role="radio"
          aria-checked={!isMix}
          className={`${styles.segBtn} ${!isMix ? styles.segBtnActive : ""}`}
          onClick={() => stereoControlsEnabled && setMix(false)}
          disabled={!stereoControlsEnabled}
          title={tAudio("stereo.lrHint")}
        >
          {tAudio("stereo.lr")}
        </button>
      </div>

      {/* Swap L/R — only in L/R mode */}
      <button
        type="button"
        className={`${styles.hkBtn} ${isSwapped ? styles.segBtnActive : ""}`}
        onClick={() => setSwap(!isSwapped)}
        disabled={!stereoControlsEnabled || isMix}
        title={tAudio("stereo.swapHint")}
      >
        {tAudio("stereo.swap")}
      </button>

      {/* Pan — greyed in Mix */}
      <div className={styles.panWrap} title={tAudio("stereo.panHint")}>
        <span className={styles.panLabel}>{tAudio("stereo.pan")}</span>
        <input
          type="range"
          className={styles.pan}
          min={-1}
          max={1}
          step={0.05}
          value={pan}
          onChange={(e) => setPan(parseFloat(e.target.value))}
          disabled={!stereoControlsEnabled || isMix}
          aria-label={tAudio("stereo.pan")}
        />
      </div>

      {/* Radio 1 / Radio 2 right-channel source — only if a Station has a 2nd radio */}
      <div
        className={`${styles.segmented} ${dualAvailable ? "" : styles.disabled}`}
        role="radiogroup"
        aria-label={tAudio("stereo.rightSourceLabel")}
      >
        <button
          type="button"
          role="radio"
          aria-checked={rightSource === "radio1"}
          className={`${styles.segBtn} ${rightSource === "radio1" ? styles.segBtnActive : ""}`}
          onClick={() => dualAvailable && chooseRightSource("radio1")}
          disabled={!dualAvailable}
          title={tAudio("stereo.radio1Hint")}
        >
          {tAudio("stereo.radio1")}
        </button>
        <button
          type="button"
          role="radio"
          aria-checked={rightSource === "radio2"}
          className={`${styles.segBtn} ${rightSource === "radio2" ? styles.segBtnActive : ""}`}
          onClick={() => dualAvailable && chooseRightSource("radio2")}
          disabled={!dualAvailable}
          title={tAudio("stereo.radio2Hint")}
        >
          {tAudio("stereo.radio2")}
        </button>
      </div>
     </Group>
    </div>
  );
}
