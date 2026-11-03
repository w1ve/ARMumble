import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import {
  HeadphonesIcon,
  HeadphonesOffIcon,
  MicIcon,
  MicOffIcon,
} from "../../icons";
import { useAppStore, resolveRoleForServer } from "../../store";
import type { AudioDevice, AudioSettings, ClientRole, TransmitMode } from "../../types";
import { InputLevelMeter, OutputLevelMeter } from "../audio/LevelMeter";
import { DeviceSelect, type DeviceValue } from "../audio/DeviceSelect";
import OperatorControlHead from "./OperatorControlHead";
import MobileRadioControls from "./MobileRadioControls";
import styles from "./RadioControlPanel.module.css";

/**
 * The radio-first main-screen control panel. Replaces the slim StationBar on
 * wide desktop and is meant to be the DOMINANT region of the screen: audio is
 * everything for amateur radio, text is secondary.
 *
 * It surfaces, large and always-visible:
 *   - the mic/VU/mic-volume cluster (operator) or radio-in cluster (station),
 *   - the headphone/output-VU/speaker-volume cluster,
 *   - the input and output DEVICE selectors (previously Settings-only), and
 *   - for an operator, the full control head (transmit mode, PTT + whisper
 *     hotkeys, whisper channel, stereo controls) in its big "panel" variant.
 *
 * Every control writes through the SAME backend commands as the Settings tab
 * (get_audio_devices / get_output_devices / get_audio_settings /
 * set_audio_settings), so this panel and Settings stay in sync.
 */
export default function RadioControlPanel({ mobile = false }: { readonly mobile?: boolean }) {
  const { t } = useTranslation(["settings", "sidebar"]);
  const tAudio = (k: string) => (t as (x: string) => string)(`settings:audio.${k}`);
  const tSide = (k: string) => (t as (x: string) => string)(`sidebar:${k}`);

  const sessions = useAppStore((s) => s.sessions);
  const activeServerId = useAppStore((s) => s.activeServerId);
  const voiceState = useAppStore((s) => s.voiceState);
  const toggleMute = useAppStore((s) => s.toggleMute);
  const toggleDeafen = useAppStore((s) => s.toggleDeafen);
  const currentChannel = useAppStore((s) => s.currentChannel);
  const ownSession = useAppStore((s) => s.ownSession);
  const talkingSessions = useAppStore((s) => s.talkingSessions);

  const activeSession = useMemo(
    () => sessions.find((s) => s.id === activeServerId),
    [sessions, activeServerId],
  );
  const serverId = activeServerId ?? undefined;
  const serverKey = activeSession ? `${activeSession.host}:${activeSession.port}` : "";

  // Role (module state, not reactive) -> local state.
  const [role, setRole] = useState<ClientRole | null>(null);
  useEffect(() => {
    let cancelled = false;
    if (!activeSession) {
      setRole(null);
      return;
    }
    void resolveRoleForServer(activeSession.host, activeSession.port)
      .then((r) => { if (!cancelled) setRole(r); })
      .catch(() => { if (!cancelled) setRole("operator"); });
    return () => { cancelled = true; };
  }, [activeSession]);

  // Device lists + audio settings for the active server.
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [outputDevices, setOutputDevices] = useState<AudioDevice[]>([]);
  const [audio, setAudio] = useState<AudioSettings | null>(null);

  const reload = useCallback(() => {
    let cancelled = false;
    void Promise.all([
      invoke<AudioDevice[]>("get_audio_devices").catch(() => [] as AudioDevice[]),
      invoke<AudioDevice[]>("get_output_devices").catch(() => [] as AudioDevice[]),
      invoke<AudioSettings>("get_audio_settings", { serverId }).catch(() => null),
    ]).then(([inDevs, outDevs, cfg]) => {
      if (cancelled) return;
      setDevices(inDevs);
      setOutputDevices(outDevs);
      if (cfg) setAudio(cfg);
    });
    return () => { cancelled = true; };
  }, [serverId]);

  useEffect(() => reload(), [reload]);

  // Patch audio settings: optimistic local update + persist to backend.
  const patchAudio = useCallback(
    (p: Partial<AudioSettings>) => {
      setAudio((prev) => {
        if (!prev) return prev;
        const merged = { ...prev, ...p };
        void invoke("set_audio_settings", { settings: merged, serverId }).catch((e) =>
          console.error("radio-panel set_audio_settings:", e),
        );
        return merged;
      });
    },
    [serverId],
  );

  const isActive = voiceState === "active";
  const isInactive = voiceState === "inactive";
  const inChannel = currentChannel != null;
  const isOperator = role === "operator";
  const isStation = role === "station";

  // ON-AIR: is THIS client transmitting right now?
  //  - Continuous: always, while voice is active (the mic is permanently open).
  //  - VOX: while the voice gate is actually passing audio.
  //  - PTT: while the key/footswitch is held.
  // VOX and PTT both surface through `talkingSessions` (the backend emits
  // user-talking for our own session as the gate/key opens and closes); a
  // Station is always-on air while active. Continuous is forced true so a
  // momentary gap in the talking event can't blink the indicator off.
  const effectiveMode: TransmitMode | undefined = audio
    ? audio.transmit_mode === "voice_activity" && audio.push_to_talk
      ? "push_to_talk"
      : audio.transmit_mode
    : undefined;
  const selfTalking = ownSession != null && talkingSessions.has(ownSession);
  const onAir =
    isActive &&
    ((effectiveMode === "continuous" || isStation) ? true : selfTalking);

  // Role-based device labels (mirrors AudioPanel).
  const inputLabel = isStation
    ? tAudio("radioAudioDevice")
    : tAudio("microphoneDevice");
  const outputLabel = isStation
    ? tAudio("radioModulationInput")
    : tAudio("operatorListeningDevice");

  // Mic != playback: hide the chosen output from the input list and vice versa.
  const ident = (d: AudioDevice) => d.label ?? d.name;
  const identOf = (list: readonly AudioDevice[], sel: string | null | undefined) =>
    (sel ? (list.find((d) => d.name === sel)?.label ?? sel) : "");
  const selOutIdent = identOf(outputDevices, audio?.selected_output_device);
  const selInIdent = identOf(devices, audio?.selected_device);
  const inputDevices = devices.filter((d) => ident(d) !== selOutIdent);
  const outputDevicesFiltered = outputDevices.filter((d) => ident(d) !== selInIdent);

  const volPct = (v: number | undefined) => `${Math.round((v ?? 1) * 100)}%`;

  if (!inChannel || !audio) {
    return <div className={`${styles.panel} ${mobile ? styles.panelMobile : ""}`} />;
  }

  return (
    <div className={`${styles.panel} ${mobile ? styles.panelMobile : ""}`}>
      {/* --- Top row: input (mic/radio) and output (headphone) clusters --- */}
      <div className={styles.levelRow}>
        {/* INPUT cluster */}
        <section className={styles.levelCard}>
          <div className={styles.levelHead}>
            {/* Station has nothing to mute (it's a radio, always TX). */}
            {!isStation && (
              <button
                type="button"
                className={`${styles.toggle} ${isActive ? styles.toggleActive : styles.toggleMuted}`}
                onClick={toggleMute}
                title={isActive ? tSide("channelSidebar.mute") : tSide("channelSidebar.unmute")}
                aria-label={isActive ? tSide("channelSidebar.mute") : tSide("channelSidebar.unmute")}
              >
                {isActive ? <MicIcon width={20} height={20} /> : <MicOffIcon width={20} height={20} />}
              </button>
            )}
            <span className={styles.levelLabel}>{inputLabel}</span>
            {onAir && (
              <span className={styles.onAir} aria-label={tAudio("ptt.onAir")}>
                <span className={styles.onAirDot} /> {tAudio("ptt.onAir")}
              </span>
            )}
          </div>
          <InputLevelMeter className={styles.meter} />
          <label className={styles.sliderRow}>
            <span className={styles.sliderLabel}>{tAudio("microphoneVolume")}</span>
            <input
              type="range"
              min={0}
              max={2}
              step={0.01}
              value={audio.input_volume ?? 1}
              onChange={(e) => patchAudio({ input_volume: parseFloat(e.target.value) })}
              aria-label={tAudio("microphoneVolume")}
            />
            <span className={styles.sliderVal}>{volPct(audio.input_volume)}</span>
          </label>
          <DeviceSelect
            devices={inputDevices}
            value={(audio.selected_device ?? null) as DeviceValue}
            onChange={(selected_device) => patchAudio({ selected_device })}
            systemDefaultLabel={tAudio("systemDefault")}
            noDeviceLabel={tAudio("noDevice")}
            defaultSuffix={tAudio("deviceDefault")}
            ariaLabel={inputLabel}
          />
        </section>

        {/* OUTPUT cluster */}
        <section className={styles.levelCard}>
          <div className={styles.levelHead}>
            <button
              type="button"
              className={`${styles.toggle} ${isInactive ? styles.toggleMuted : styles.toggleActive}`}
              onClick={toggleDeafen}
              title={isInactive ? tSide("channelSidebar.enableVoice") : tSide("channelSidebar.disableVoice")}
              aria-label={isInactive ? tSide("channelSidebar.enableVoice") : tSide("channelSidebar.disableVoice")}
            >
              {isInactive ? (
                <HeadphonesOffIcon width={20} height={20} />
              ) : (
                <HeadphonesIcon width={20} height={20} />
              )}
            </button>
            <span className={styles.levelLabel}>{outputLabel}</span>
          </div>
          <OutputLevelMeter className={styles.meter} />
          <label className={styles.sliderRow}>
            <span className={styles.sliderLabel}>{tAudio("speakerVolume")}</span>
            <input
              type="range"
              min={0}
              max={2}
              step={0.01}
              value={audio.output_volume ?? 1}
              onChange={(e) => patchAudio({ output_volume: parseFloat(e.target.value) })}
              aria-label={tAudio("speakerVolume")}
            />
            <span className={styles.sliderVal}>{volPct(audio.output_volume)}</span>
          </label>
          <DeviceSelect
            devices={outputDevicesFiltered}
            value={(audio.selected_output_device ?? null) as DeviceValue}
            onChange={(selected_output_device) => patchAudio({ selected_output_device })}
            systemDefaultLabel={tAudio("systemDefault")}
            noDeviceLabel={tAudio("noDevice")}
            defaultSuffix={tAudio("deviceDefault")}
            ariaLabel={outputLabel}
          />
        </section>
      </div>

      {/* --- Operator live controls --- */}
      {isOperator &&
        (mobile ? (
          <MobileRadioControls serverId={serverId} serverKey={serverKey} />
        ) : (
          <OperatorControlHead serverId={serverId} serverKey={serverKey} variant="panel" />
        ))}
    </div>
  );
}
