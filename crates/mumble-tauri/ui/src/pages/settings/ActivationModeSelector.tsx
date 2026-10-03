import { useTranslation } from "react-i18next";
import type { AudioSettings, ClientRole, TransmitMode } from "../../types";
import { ActivityIcon, AudioWaveformIcon, KeyboardIcon } from "../../icons";
import { RadioCardGroup, type RadioCardOption } from "../../components/elements/RadioCardGroup";

/**
 * The transmit mode a card represents. These map 1:1 onto the backend
 * `TransmitMode` enum; `transmit_mode` is the source of truth and
 * `push_to_talk` is kept in step only for the legacy self-mute PTT path.
 *
 * Crucially this does NOT touch `noise_suppression`: §14c decoupled the
 * voice gate from the denoiser, so a radio Station can run Continuous with
 * suppression on or off independently.
 */
const MODE_PATCHES: Array<{
  value: TransmitMode;
  Icon: typeof ActivityIcon;
  patch: Partial<AudioSettings>;
}> = [
  {
    value: "voice_activity",
    Icon: ActivityIcon,
    patch: { transmit_mode: "voice_activity", push_to_talk: false },
  },
  {
    value: "continuous",
    Icon: AudioWaveformIcon,
    patch: {
      transmit_mode: "continuous",
      push_to_talk: false,
      auto_input_sensitivity: false,
    },
  },
  {
    value: "push_to_talk",
    Icon: KeyboardIcon,
    patch: {
      transmit_mode: "push_to_talk",
      push_to_talk: true,
      auto_input_sensitivity: false,
    },
  },
];

/** The effective transmit mode, reconciling the legacy `push_to_talk` flag the
 *  same way the backend's `AudioSettings::transmit_mode()` does: an explicit
 *  mode wins, but a config still at the default that carries `push_to_talk`
 *  resolves to PTT. */
function effectiveMode(s: AudioSettings): TransmitMode {
  if (s.transmit_mode === "voice_activity" && s.push_to_talk) return "push_to_talk";
  return s.transmit_mode;
}

export function ActivationModeSelector({
  settings,
  onChange,
  role = "operator",
}: Readonly<{
  settings: AudioSettings;
  onChange: (patch: Partial<AudioSettings>) => void;
  /** The role of the server these settings apply to. A Station transmits
   *  its receiver audio continuously, so it is offered Continuous only. */
  role?: ClientRole;
}>) {
  const { t } = useTranslation("settings");
  const tStr = t as (key: string) => string;

  // A Station is a transmitter keyed up continuously; it only ever runs
  // Continuous, so don't offer VOX/PTT for it.
  const visible = role === "station"
    ? MODE_PATCHES.filter((p) => p.value === "continuous")
    : MODE_PATCHES;

  const options: Array<RadioCardOption<TransmitMode> & { patch: Partial<AudioSettings> }> =
    visible.map((p) => ({
      value: p.value,
      label: tStr(`activation.${p.value}`),
      description:
        p.value === "continuous" && role === "station"
          ? tStr("activation.continuousStationDesc")
          : tStr(`activation.${p.value}Desc`),
      Icon: p.Icon,
      patch: p.patch,
    }));

  // A Station only has Continuous available, so show it selected even if a
  // stale saved config still says otherwise (SettingsPage normalises the
  // persisted value separately).
  const activeMode: TransmitMode =
    role === "station" ? "continuous" : effectiveMode(settings);

  return (
    <RadioCardGroup
      name="activation_mode"
      options={options}
      value={activeMode}
      onChange={(mode) => {
        const opt = options.find((o) => o.value === mode);
        if (opt) onChange(opt.patch);
      }}
    />
  );
}
