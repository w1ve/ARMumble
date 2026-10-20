import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import type {
  AudioDevice,
  AudioSettings,
  CryptoStats,
  ClientRole,
  NoiseSuppressionAlgorithm,
  PacketStats,
  ServerId,
  SessionMeta,
  StereoMode,
} from "../../types";
import {
  ActivityIcon,
  AudioWaveformIcon,
  MicOffIcon,
  SlidersIcon,
  SparklesIcon,
} from "../../icons";
import { isDesktopPlatform } from "../../utils/platform";
import { Toggle, SliderField } from "./SharedControls";
import { DenoiserAdvancedControls } from "./DenoiserAdvancedControls";
import { ActivationModeSelector } from "./ActivationModeSelector";
import { CalibrationPanel } from "./CalibrationPanel";
import { PttPanel } from "./PttPanel";
import { RadioCardGroup, type RadioCardOption } from "../../components/elements/RadioCardGroup";
import { DeviceSelect } from "../../components/audio/DeviceSelect";
import { InputLevelMeter, OutputLevelMeter } from "../../components/audio/LevelMeter";
import styles from "./SettingsPage.module.css";
import panelStyles from "./AudioPanel.module.css";
import { registerSettings } from "./settingsSearchRegistry";

registerSettings("voice")
  .add("audio.inputDevice", ["microphone", "mic"])
  .add("audio.outputDevice", ["speaker", "headphones"])
  .add("audio.activationMode", ["voice activity", "vad", "push to talk", "ptt"])
  .add("audio.noiseSuppression", ["denoise", "background noise"])
  .add("audio.audioProcessing", ["echo", "gain"])
  .add("audio.compression")
  .add("audio.network", ["jitter", "bandwidth"])
  .add("audio.stats.title", ["statistics", "debug"])
  .add("audio.expert");

const FRAME_SIZE_OPTIONS = [
  { value: 10, label: "10 ms" },
  { value: 20, label: "20 ms" },
  { value: 40, label: "40 ms" },
  { value: 60, label: "60 ms" },
];

function buildDenoiserOptions(t: (key: string) => string): readonly RadioCardOption<NoiseSuppressionAlgorithm>[] {
  return [
    {
      value: "none",
      label: t("audio.denoiser.off"),
      description: t("audio.denoiser.offDesc"),
      Icon: MicOffIcon,
    },
    {
      value: "rnnoise",
      label: t("audio.denoiser.rnnoise"),
      description: t("audio.denoiser.rnnoiseDesc"),
      Icon: ActivityIcon,
    },
    {
      value: "deepfilternet",
      label: t("audio.denoiser.deepfilternet"),
      description: t("audio.denoiser.deepfilternetDesc"),
      Icon: SparklesIcon,
    },
    {
      value: "omlsa_imcra",
      label: t("audio.denoiser.omlsa"),
      description: t("audio.denoiser.omlsaDesc"),
      Icon: SlidersIcon,
    },
    {
      value: "spectral_subtraction",
      label: t("audio.denoiser.spectral"),
      description: t("audio.denoiser.spectralDesc"),
      Icon: AudioWaveformIcon,
    },
  ];
}

function StatsRow({ label, stats }: Readonly<{ label: string; stats: PacketStats }>) {
  const total = stats.good + stats.late + stats.lost;
  const lossPercent = total > 0 ? ((stats.lost / total) * 100).toFixed(1) : "0.0";
  return (
    <div className={panelStyles.statsRow}>
      <span className={panelStyles.statsLabel}>{label}</span>
      <span className={panelStyles.statsValues}>
        {stats.good} good &middot; {stats.late} late &middot; {stats.lost} lost ({lossPercent}%) &middot; {stats.resync} resync
      </span>
    </div>
  );
}

function AudioStatsSection() {
  const { t } = useTranslation("settings");
  const [cryptoStats, setCryptoStats] = useState<CryptoStats | null>(null);

  useEffect(() => {
    const unlisten = listen<CryptoStats>("crypto-stats", (event) => {
      setCryptoStats(event.payload);
    });
    return () => { unlisten.then((f) => f()); };
  }, []);

  if (!cryptoStats) {
    return (
      <section className={styles.section}>
        <h3 className={styles.sectionTitle}>{t("audio.stats.title")}</h3>
        <p className={styles.fieldHint}>
          {t("audio.stats.noStats")}
        </p>
      </section>
    );
  }

  return (
    <section className={styles.section}>
      <h3 className={styles.sectionTitle}>{t("audio.stats.title")}</h3>
      <p className={styles.fieldHint}>
        {t("audio.stats.udpCounters")}
      </p>
      <StatsRow label={t("audio.stats.toClient")} stats={cryptoStats.to_client} />
      <StatsRow label={t("audio.stats.fromClient")} stats={cryptoStats.from_client} />
    </section>
  );
}

/** Sentinel 2nd-radio device meaning "generate a test tone" instead of
 *  opening real hardware. Must match `TEST_TONE_DEVICE` in the Rust backend
 *  (commands/audio.rs). Lets an operator exercise the dual-radio right channel
 *  with no second radio connected. */
const TEST_TONE_DEVICE = "\u0001testtone";

export function AudioPanel({
  devices,
  outputDevices,
  settings,
  onChange,
  isExpert,
  useRodioBackend,
  onToggleAudioBackend,
  servers,
  selectedServer,
  onSelectServer,
  role,
  serverKey,
}: Readonly<{
  devices: AudioDevice[];
  outputDevices: AudioDevice[];
  settings: AudioSettings;
  onChange: (patch: Partial<AudioSettings>) => void;
  isExpert: boolean;
  useRodioBackend: boolean;
  onToggleAudioBackend: () => void;
  /** Connected sessions the user can scope audio settings to. */
  servers: readonly SessionMeta[];
  /** Which server is being edited (`null` = the shared default profile). */
  selectedServer: ServerId | null;
  /** Change the server being edited. */
  onSelectServer: (id: ServerId | null) => void;
  /** Role of the server being edited; a Station is offered Continuous only. */
  role: ClientRole;
  /** Persistence key ("host:port", or the default bucket) for the edited
   *  server, so the PTT panel can save its config to disk. */
  serverKey: string;
}>) {
  const { t } = useTranslation("settings");
  const tStr = t as (key: string) => string;
  const isStation = role === "station";
  // Role-based device labels. A Station is at the rig: its "input" is the
  // radio's audio and its "output" modulates the radio. An Operator is the
  // remote human: a microphone in, a listening device out.
  const inputLabel = isStation ? t("audio.radioAudioDevice") : t("audio.microphoneDevice");
  const outputLabel = isStation ? t("audio.radioModulationInput") : t("audio.operatorListeningDevice");
  // Mic and playback must be DIFFERENT devices: capturing the same node you
  // play to means the "mic" records the received audio (full-duplex loopback).
  // So the input list hides the currently-selected output device, and the
  // output list hides the currently-selected input device.
  const inputDevices = devices.filter(
    (d) => d.name !== (settings.selected_output_device ?? ""),
  );
  const outputDevicesFiltered = outputDevices.filter(
    (d) => d.name !== (settings.selected_device ?? ""),
  );
  // The 2nd input (second radio) list excludes the first input AND the output,
  // so the two radios can't be the same device, nor the same as playback.
  const secondInputDevices = devices.filter(
    (d) =>
      d.name !== (settings.selected_device ?? "") &&
      d.name !== (settings.selected_output_device ?? ""),
  );
  const denoiserOptions = buildDenoiserOptions(tStr);
  const stereoOptions: RadioCardOption<StereoMode>[] = [
    {
      value: "stereo",
      label: tStr("audio.stereoModeStereo"),
      description: tStr("audio.stereoModeStereoDesc"),
      Icon: AudioWaveformIcon,
    },
    {
      value: "mono",
      label: tStr("audio.stereoModeMono"),
      description: tStr("audio.stereoModeMonoDesc"),
      Icon: ActivityIcon,
    },
    {
      value: "swap_lr",
      label: tStr("audio.stereoModeSwap"),
      description: tStr("audio.stereoModeSwapDesc"),
      Icon: SlidersIcon,
    },
  ];
  const [availableAlgorithms, setAvailableAlgorithms] = useState<
    NoiseSuppressionAlgorithm[]
  >(["none", "omlsa_imcra", "spectral_subtraction"]);
  useEffect(() => {
    invoke<NoiseSuppressionAlgorithm[]>("get_available_denoiser_algorithms")
      .then((algos) => {
        if (Array.isArray(algos)) setAvailableAlgorithms(algos);
      })
      .catch(() => { /* keep the conservative default */ });
  }, []);

  // Stereo on-air test: transmit the embedded stereo tone to the other side.
  // The backend emits `stereo-test-state` (true while playing, false when done
  // or stopped) so the button can toggle Play <-> Stop.
  const [stereoTesting, setStereoTesting] = useState(false);
  useEffect(() => {
    const unlisten = listen<boolean>("stereo-test-state", (e) => {
      setStereoTesting(e.payload);
    });
    return () => { unlisten.then((f) => f()); };
  }, []);

  // Test-tone 2nd radio: a sentinel device that makes the backend synthesise a
  // tone on the right channel so dual-radio can be exercised with no 2nd radio.
  // Toggling it on also forces the right channel to Radio 2 so it's audible
  // immediately; toggling off restores whatever was selected before.
  const toneOn = settings.selected_device_2 === TEST_TONE_DEVICE;
  const prevSecondRadio = useRef<{
    device: string | null;
    source: AudioSettings["right_channel_source"];
  }>({ device: null, source: "radio1" });
  const toggleTestTone = () => {
    if (toneOn) {
      onChange({
        selected_device_2: prevSecondRadio.current.device,
        right_channel_source: prevSecondRadio.current.source,
      });
    } else {
      prevSecondRadio.current = {
        device: settings.selected_device_2 ?? null,
        source: settings.right_channel_source,
      };
      onChange({
        selected_device_2: TEST_TONE_DEVICE,
        right_channel_source: "radio2",
      });
    }
  };

  // The voice-gate calibration UI applies only in Voice-Activation mode.
  // Reconcile the legacy push_to_talk flag the same way the backend does.
  const effectiveMode =
    settings.transmit_mode === "voice_activity" && settings.push_to_talk
      ? "push_to_talk"
      : settings.transmit_mode;
  const isVoiceGate = effectiveMode === "voice_activity";

  return (
    <>
      <h2 className={styles.panelTitle}>{t("audio.panelTitle")}</h2>

      {/* -- Which server these settings apply to ----------- */}
      <section className={styles.section}>
        <h3 className={styles.sectionTitle}>{t("audio.appliesTo")}</h3>
        <select
          className={styles.select}
          value={selectedServer ?? ""}
          onChange={(e) =>
            onSelectServer(e.target.value === "" ? null : (e.target.value as ServerId))
          }
        >
          <option value="">{t("audio.defaultProfile")}</option>
          {servers.map((s) => (
            <option key={s.id} value={s.id}>
              {s.label}
            </option>
          ))}
        </select>
        <p className={styles.fieldHint}>
          {selectedServer === null
            ? t("audio.defaultProfileHint")
            : t("audio.serverProfileHint")}
        </p>
      </section>

      {/* -- Input & Output Devices (side by side) ---------- */}
      <section className={styles.section}>
        <div className={panelStyles.deviceColumns}>
          <div className={panelStyles.deviceColumn}>
            <h3 className={styles.sectionTitle}>{inputLabel}</h3>
            <DeviceSelect
              devices={inputDevices}
              value={settings.selected_device ?? null}
              onChange={(selected_device) => onChange({ selected_device })}
              systemDefaultLabel={t("audio.systemDefault")}
              noDeviceLabel={t("audio.noDevice")}
              defaultSuffix={t("audio.deviceDefault")}
              ariaLabel={inputLabel}
            />
            <div className={panelStyles.deviceScope}>
              <InputLevelMeter ariaLabel={inputLabel} />
            </div>
            <SliderField
              label={isStation ? t("audio.radioPlaybackLevel") : t("audio.microphoneVolume")}
              min={0}
              max={2}
              step={0.01}
              value={settings.input_volume}
              onChange={(v) => onChange({ input_volume: v })}
              format={(v) => `${Math.round(v * 100)}%`}
            />

            {/* Second radio input (Station only). '(No device)' disables the
                dual-radio feature. Cannot be the same device as the first. */}
            {isStation && (
              <>
                <h3 className={styles.sectionTitle}>{t("audio.secondRadioDevice")}</h3>
                <DeviceSelect
                  devices={secondInputDevices}
                  value={toneOn ? null : (settings.selected_device_2 ?? null)}
                  onChange={(selected_device_2) => onChange({ selected_device_2 })}
                  systemDefaultLabel={t("audio.systemDefault")}
                  noDeviceLabel={t("audio.noDevice")}
                  defaultSuffix={t("audio.deviceDefault")}
                  ariaLabel={t("audio.secondRadioDevice")}
                  disabled={toneOn}
                />
                <p className={styles.fieldHint}>{t("audio.secondRadioHint")}</p>
                <button
                  type="button"
                  className={styles.ghostBtn}
                  aria-pressed={toneOn}
                  onClick={toggleTestTone}
                >
                  {toneOn ? t("audio.testToneStop") : t("audio.testTonePlay")}
                </button>
                <p className={styles.fieldHint}>{t("audio.testToneHint")}</p>
              </>
            )}
          </div>

          <div className={panelStyles.deviceColumn}>
            <h3 className={styles.sectionTitle}>{outputLabel}</h3>
            <DeviceSelect
              devices={outputDevicesFiltered}
              value={settings.selected_output_device ?? null}
              onChange={(selected_output_device) => onChange({ selected_output_device })}
              systemDefaultLabel={t("audio.systemDefault")}
              noDeviceLabel={t("audio.noDevice")}
              defaultSuffix={t("audio.deviceDefault")}
              ariaLabel={outputLabel}
            />
            <div className={panelStyles.deviceScope}>
              <OutputLevelMeter ariaLabel={outputLabel} />
            </div>
            <SliderField
              label={t("audio.speakerVolume")}
              min={0}
              max={2}
              step={0.01}
              value={settings.output_volume}
              onChange={(v) => onChange({ output_volume: v })}
              format={(v) => `${Math.round(v * 100)}%`}
            />
          </div>
        </div>

        {/* Station-only explanation of the radio audio in/out roles. */}
        {isStation && (
          <p className={styles.fieldHint}>
            <strong>{t("audio.radioAudioDevice")}</strong>
            {t("audio.stationDeviceExplainA")}
            <strong>{t("audio.radioModulationInput")}</strong>
            {t("audio.stationDeviceExplainB")}
          </p>
        )}
      </section>

      {/* -- Stereo Output ------------------------------------------ */}
      <section className={styles.section}>
        <h3 className={styles.sectionTitle}>{t("audio.stereoOutput")}</h3>
        <p className={styles.fieldHint}>{t("audio.stereoOutputHint")}</p>
        <RadioCardGroup
          name="playback_stereo_mode"
          options={stereoOptions}
          value={settings.playback_stereo_mode}
          onChange={(playback_stereo_mode) => onChange({ playback_stereo_mode })}
        />
        {settings.playback_stereo_mode === "stereo" && (
          <>
            <SliderField
              label={t("audio.leftVolume")}
              min={0}
              max={2}
              step={0.01}
              value={settings.left_volume}
              onChange={(v) => onChange({ left_volume: v })}
              format={(v) => `${Math.round(v * 100)}%`}
            />
            <SliderField
              label={t("audio.rightVolume")}
              min={0}
              max={2}
              step={0.01}
              value={settings.right_volume}
              onChange={(v) => onChange({ right_volume: v })}
              format={(v) => `${Math.round(v * 100)}%`}
            />
          </>
        )}
        {/* The stereo test TRANSMITS stereo, which only a Station does — an
            operator sends mono — so the button is shown only in Station mode. */}
        {isDesktopPlatform() && role === "station" && (
          <>
            <p className={styles.fieldHint}>{t("audio.stereoTestHint")}</p>
            <button
              type="button"
              className={styles.ghostBtn}
              onClick={() =>
                invoke(stereoTesting ? "stop_stereo_test" : "play_stereo_test").catch((e) =>
                  console.error("stereo test:", e),
                )
              }
            >
              {stereoTesting ? t("audio.stereoTestStop") : t("audio.stereoTestPlay")}
            </button>
          </>
        )}
      </section>

      {/* -- Activation Mode + Voice Gate ---------------------------- */}
      <section className={styles.section}>
        <h3 className={styles.sectionTitle}>{t("audio.activationMode")}</h3>
        <p className={styles.fieldHint}>
          {t("audio.activationModeHint")}
        </p>
        <ActivationModeSelector settings={settings} onChange={onChange} role={role} />
        {effectiveMode === "push_to_talk" && (
          <p
            className={styles.fieldHint}
            dangerouslySetInnerHTML={{ __html: t("audio.pttHint", { tab: t("audio.shortcutsTab") }) }}
          />
        )}
        {isVoiceGate && (
          <>
            <h4 className={styles.groupTitle}>{t("audio.voiceGate")}</h4>
            <CalibrationPanel settings={settings} onChange={onChange} />
          </>
        )}
      </section>

      {/* Noise suppression and auto-gain are hidden for a Station: its audio is
          high-fidelity receiver output transmitted untouched (the backend runs
          NO outbound DSP for a Station), so these voice-mic processors would
          only degrade it. */}
      {!isStation && (
      <>
      {/* -- Noise Suppression --------------------------------------- */}
      <section className={styles.section}>
        <h3 className={styles.sectionTitle}>{t("audio.noiseSuppression")}</h3>
        <p className={styles.fieldHint}>
          {t("audio.noiseSuppressionHint")}
        </p>
        <RadioCardGroup
          name="denoiser_algorithm"
          options={denoiserOptions.filter((o) => availableAlgorithms.includes(o.value))}
          value={settings.denoiser_algorithm}
          onChange={(denoiser_algorithm) => onChange({ denoiser_algorithm })}
        />
        {isExpert && (
          <DenoiserAdvancedControls
            algorithm={settings.denoiser_algorithm}
            settings={settings}
            onChange={onChange}
          />
        )}
      </section>

      {/* -- Audio Processing ------------------------------- */}
      <section className={styles.section}>
        <h3 className={styles.sectionTitle}>{t("audio.audioProcessing")}</h3>

        <div className={styles.toggleRow}>
          <div className={styles.toggleInfo}>
            <span className={styles.fieldLabel}>{t("audio.autoGain")}</span>
            <p className={styles.fieldHint}>
              {t("audio.autoGainHint")}
            </p>
          </div>
          <Toggle
            checked={settings.auto_gain}
            onChange={() => onChange({ auto_gain: !settings.auto_gain })}
          />
        </div>

        {settings.auto_gain && !settings.auto_input_sensitivity && (
          <SliderField
            label={t("audio.maxAmplification")}
            hint={t("audio.maxAmplificationHint")}
            min={1}
            max={40}
            step={1}
            value={settings.max_gain_db}
            onChange={(v) => onChange({ max_gain_db: v })}
            format={(v) => `${Math.round(v)} dB`}
          />
        )}
      </section>
      </>
      )}

      {/* -- Compression ------------------------------------ */}
      <section className={styles.section}>
        <h3 className={styles.sectionTitle}>{t("audio.compression")}</h3>
        {/* A Station transmits forced stereo; below ~96 kb/s Opus's channels
            bleed and artifact, so the station quality floor is 128 kb/s to
            match the backend (STATION_MIN_BITRATE). */}
        <SliderField
          label={t("audio.quality")}
          hint={isStation ? t("audio.qualityHintStation") : t("audio.qualityHint")}
          min={isStation ? 128 : 8}
          max={320}
          step={8}
          value={Math.max(isStation ? 128 : 8, settings.bitrate_bps / 1000)}
          onChange={(v) => onChange({ bitrate_bps: v * 1000 })}
          format={(v) => `${v} kb/s`}
        />
        <div className={styles.field}>
          <div className={styles.fieldRow}>
            <span className={styles.fieldLabel}>{t("audio.audioPerPacket")}</span>
            <span className={styles.sliderValue}>
              {settings.frame_size_ms} ms
            </span>
          </div>
          <p className={styles.fieldHint}>
            {t("audio.audioPerPacketHint")}
          </p>
          <div className={styles.radioGroup}>
            {FRAME_SIZE_OPTIONS.map((opt) => (
              <label key={opt.value} className={styles.radioLabel}>
                <input
                  type="radio"
                  name="frame_size_ms"
                  value={opt.value}
                  checked={settings.frame_size_ms === opt.value}
                  onChange={() => onChange({ frame_size_ms: opt.value })}
                />
                {opt.label}
              </label>
            ))}
          </div>
        </div>
      </section>

      {/* -- Network ---------------------------------------- */}
      <section className={styles.section}>
        <h3 className={styles.sectionTitle}>{t("audio.network")}</h3>

        <div className={styles.toggleRow}>
          <div className={styles.toggleInfo}>
            <span className={styles.fieldLabel}>{t("audio.forceTcpAudio")}</span>
            <p className={styles.fieldHint}>
              {t("audio.forceTcpAudioHint")}
            </p>
          </div>
          <Toggle
            checked={settings.force_tcp_audio}
            onChange={() => onChange({ force_tcp_audio: !settings.force_tcp_audio })}
          />
        </div>
      </section>

      {/* -- Expert settings -------------------------------- */}
      {isExpert && isDesktopPlatform() && (
        <section className={styles.section}>
          <h3 className={styles.sectionTitle}>{t("audio.expert")}</h3>
          <div className={styles.toggleRow}>
            <div className={styles.toggleInfo}>
              <span className={styles.fieldLabel}>{t("audio.legacyAudioBackend")}</span>
              <p className={styles.fieldHint}>
                {t("audio.legacyAudioBackendHint")}
              </p>
            </div>
            <Toggle
              checked={!useRodioBackend}
              onChange={onToggleAudioBackend}
            />
          </div>
        </section>
      )}

      {/* -- Rig PTT (RS-232) -------------------------------- */}
      {isExpert && isDesktopPlatform() && (
        <PttPanel
          serverId={selectedServer}
          serverKey={serverKey}
          role={role}
          transmitMode={effectiveMode}
        />
      )}

      {/* -- Audio Statistics -------------------------------- */}
      {isExpert && <AudioStatsSection />}


    </>
  );
}
