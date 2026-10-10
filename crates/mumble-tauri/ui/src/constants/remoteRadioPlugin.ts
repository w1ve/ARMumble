/**
 * Remote.Radio's own hidden control channel, carried over Mumble
 * `PluginMessage` envelopes (Tauri event `plugin-message`), NOT the parked
 * remote-desktop `rr-control` WebRTC stack and NOT the forbidden
 * `PluginDataTransmission` path.
 *
 * Two message kinds travel under `plugin_name = "remote-radio"`:
 *
 * - **ptt**  — an operator keys/unkeys a station's rig. Direct peer path, so it
 *   can lead the Mumble-relayed voice (see CONTEXT §14d). Hidden from chat
 *   because `PluginMessage` is a distinct Mumble message type from `TextMessage`.
 * - **role** — a client announces whether it is an Operator or a Station, so
 *   peers can show the right identity/iconography. This is the authoritative
 *   replacement for the saved-server name-match placeholder in
 *   `stationIdentity.tsx` (CONTEXT §18g step 8).
 *
 * These string constants and payload shapes MUST stay in lock-step with the
 * Rust mirror in `crates/mumble-tauri/src/state/rr_plugin.rs`.
 */

import type { ClientRole } from "../types";

/** `plugin_name` for every Remote.Radio control message. */
export const RR_PLUGIN_NAME = "remote-radio";

/** `payloadType` discriminators under {@link RR_PLUGIN_NAME}. */
export enum RrPayloadType {
  /** Key/unkey a station's PTT. Payload: {@link RrPttPayload}. */
  Ptt = "ptt",
  /** Announce this client's role. Payload: {@link RrRolePayload}. */
  Role = "role",
  /** Operator tells a Station which radio feeds its transmitted stereo RIGHT
   *  channel (dual-radio). Payload: {@link RrRightChannelSourcePayload}. */
  RightChannelSource = "right_channel_source",
}

/**
 * PTT key/unkey from an operator to a station.
 *
 * `seq` monotonically increases per sender so a stale/reordered frame is
 * ignored; `down` is the desired key state. The station's own safety watchdog
 * (deadman + max-transmit) is the real guard — this message is just intent.
 */
export interface RrPttPayload {
  /** `true` = key (transmit), `false` = unkey. */
  down: boolean;
  /** Monotonic per-sender sequence number; higher wins. */
  seq: number;
}

/** Role announcement from a client. */
export interface RrRolePayload {
  /** The sender's role on this server. */
  role: ClientRole;
  /** Station only: whether a 2nd radio input is configured, so operators know
   *  to enable the Radio 1 / Radio 2 right-channel control. Absent/false means
   *  no dual-radio available. */
  hasSecondRadio?: boolean;
}

/** Operator -> Station: which radio feeds the transmitted stereo RIGHT
 *  channel. The station re-mixes its capture on receipt (one stereo stream). */
export interface RrRightChannelSourcePayload {
  /** `radio1` = normal stereo / mix of radio 1; `radio2` = Left=radio1 mono,
   *  Right=radio2 mono. */
  source: import("../types").RightChannelSource;
}
