import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Phase = "disconnected" | "connecting" | "connected";

export interface User {
  session: number;
  name: string;
  channel_id: number;
  self_mute: boolean;
  self_deaf: boolean;
}
export interface Channel {
  id: number;
  parent_id: number | null;
  name: string;
}
export interface Speaker {
  session: number;
  active: boolean;
  depth_ms: number;
  target_ms: number;
  jitter_ms: number;
  ratio_ppm: number;
  lost: number;
  late: number;
  underruns: number;
}
export interface Plan {
  bitrate_kbps: number;
  frame_ms: number;
  clamped: boolean;
  wire_kbps: number;
}
export interface Levels {
  in_l: number;
  in_r: number;
  out_l: number;
  out_r: number;
  in_clips: number;
  out_clips: number;
  live: boolean;
}
export interface Status {
  phase: Phase;
  error: string | null;
  scope_missing: boolean;
  udp: boolean | null;
  own_session: number | null;
  own_channel: number | null;
  users: User[];
  channels: Channel[];
  speakers: Speaker[];
  plan: Plan | null;
  transmitting: boolean;
  ptt_down: boolean;
  sent: number;
  dropped: number;
  audio_errors: number;
  audio_error: string;
  remote_ptt: number[];
}

export type TxMode = "ptt" | "vox" | "continuous";
export type So2rRole = "operator" | "station";
export type Listen = "1" | "2" | "both";

/**
 * Live two-radio (SO2R) state. It comes from the logger on this computer
 * ("local"), the logger at the other position ("remote"), or the buttons.
 */
export interface Route {
  enabled: boolean;
  role: So2rRole;
  tx: 1 | 2;
  rx: Listen;
  source: "manual" | "local" | "remote";
  otrsp_available: boolean;
  otrsp_local: boolean;
  otrsp_error: string | null;
}
export type ChannelMap = "stereo" | "left" | "right" | "mix";

export interface Settings {
  server: { host: string; port: number; username: string; password: string; scope: string };
  audio: {
    input_device: string | null;
    output_device: string | null;
    input_device_b: string | null;
    output_device_b: string | null;
    so2r_enabled: boolean;
    so2r_role: So2rRole;
    so2r_single_device: boolean;
    so2r_tx: 1 | 2;
    so2r_rx: Listen;
    otrsp_port: string;
    keyer_udp_enabled: boolean;
    keyer_udp_port: number;
    otrsp_enabled: boolean;
    bitrate_kbps: number;
    stereo: boolean;
    max_frame_ms: number;
    channel_map: ChannelMap;
    input_gain: number;
    master_gain: number;
    tx_mode: TxMode;
    vox_threshold_db: number;
    vox_hang_ms: number;
    mono_output: boolean;
    force_tcp: boolean;
  };
  hotkeys: {
    ptt: string;
    whisper: string;
    listen_in: string;
    whisper_target: number | null;
    listen_target: number | null;
    mute_home_on_listen: boolean;
  };
  ptt: {
    local: { enabled: boolean; port: string; sense_line: "cts" | "dsr" | "cd" | "ri"; active_low: boolean };
    station: {
      enabled: boolean;
      port: string;
      key_line: "dtr" | "rts";
      active_low: boolean;
      vox: boolean;
      vox_hang_ms: number;
      hang_ms: number;
      deadman_ms: number;
      max_tx_secs: number;
    };
    station2: { enabled: boolean; port: string; key_line: "dtr" | "rts"; active_low: boolean };
  };
  language: string;
  theme: "dark" | "light";
}

export interface Device {
  id: string;
  name: string;
  input: boolean;
  output: boolean;
  is_default: boolean;
}
export interface Port {
  name: string;
  label: string;
}

export const api = {
  loadSettings: () => invoke<Settings>("load_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  listDevices: () => invoke<Device[]>("list_devices"),
  listPorts: () => invoke<Port[]>("list_serial_ports"),
  connect: () => invoke<void>("connect"),
  disconnect: () => invoke<void>("disconnect"),
  joinChannel: (id: number) => invoke<void>("join_channel", { id }),
  setPtt: (down: boolean) => invoke<void>("set_ptt", { down }),
  listenIn: (target: number, down: boolean) => invoke<void>("listen_in", { target, down }),
  whisper: (target: number, down: boolean) => invoke<void>("whisper", { target, down }),
  setUserGain: (session: number, gain: number) => invoke<void>("set_user_gain", { session, gain }),
  getRoute: () => invoke<Route>("get_route"),
};

export function onStatus(cb: (s: Status) => void): Promise<UnlistenFn> {
  return listen<Status>("status", (e) => cb(e.payload));
}

export function onLevels(cb: (l: Levels) => void): Promise<UnlistenFn> {
  return listen<Levels>("levels", (e) => cb(e.payload));
}

export function onRoute(cb: (r: Route) => void): Promise<UnlistenFn> {
  return listen<Route>("route", (e) => cb(e.payload));
}
