/**
 * Unit tests for per-server audio settings persistence
 * (preferencesStorage).
 *
 * Audio settings became per-server (keyed by "host:port") with a reserved
 * default bucket for the "nothing connected" profile. These tests cover
 * per-server isolation, the default-inheritance fallback, and migration of
 * the legacy single-blob format into the default bucket.
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import type { AudioSettings } from "../../types";

// In-memory store backing for the mock.
let storeData: Record<string, unknown> = {};

vi.mock("@tauri-apps/plugin-store", () => ({
  load: vi.fn().mockImplementation(() =>
    Promise.resolve({
      get: vi.fn().mockImplementation((key: string) =>
        Promise.resolve(storeData[key] ?? null),
      ),
      set: vi.fn().mockImplementation((key: string, value: unknown) => {
        storeData[key] = value;
        return Promise.resolve();
      }),
    }),
  ),
}));

// Import after mocks are in place.
import {
  getSavedAudioSettings,
  saveAudioSettings,
  AUDIO_DEFAULT_KEY,
} from "../../preferencesStorage";

const AUDIO_KEY = "audioSettings";

function makeSettings(overrides: Partial<AudioSettings> = {}): AudioSettings {
  return {
    selected_device: null,
    selected_device_2: null,
    right_channel_source: "radio1",
    auto_gain: true,
    vad_threshold: 0.3,
    max_gain_db: 15,
    noise_gate_close_ratio: 0.8,
    hold_frames: 15,
    transmit_mode: "voice_activity",
    push_to_talk: false,
    push_to_talk_key: null,
    bitrate_bps: 72000,
    frame_size_ms: 20,
    noise_suppression: true,
    denoiser_algorithm: "rnnoise",
    denoiser_params: {},
    selected_output_device: null,
    input_volume: 1,
    output_volume: 1,
    auto_input_sensitivity: false,
    force_tcp_audio: false,    playback_stereo_mode: "stereo",    left_volume: 1,    right_volume: 1,
    ...overrides,
  };
}

describe("Per-server audio settings storage", () => {
  beforeEach(() => {
    storeData = {};
  });

  it("returns null when nothing is saved", async () => {
    expect(await getSavedAudioSettings()).toBeNull();
    expect(await getSavedAudioSettings("example.com:54736")).toBeNull();
  });

  it("saves and reads back the default profile", async () => {
    const s = makeSettings({ bitrate_bps: 96000 });
    await saveAudioSettings(s);
    const read = await getSavedAudioSettings();
    expect(read?.bitrate_bps).toBe(96000);
  });

  it("isolates settings per server", async () => {
    await saveAudioSettings(makeSettings({ bitrate_bps: 96000 }), "a.com:54736");
    await saveAudioSettings(makeSettings({ bitrate_bps: 128000 }), "b.com:54736");

    const a = await getSavedAudioSettings("a.com:54736");
    const b = await getSavedAudioSettings("b.com:54736");
    expect(a?.bitrate_bps).toBe(96000);
    expect(b?.bitrate_bps).toBe(128000);
  });

  it("a server with no settings of its own inherits the default profile", async () => {
    await saveAudioSettings(makeSettings({ bitrate_bps: 112000 }));
    // never saved for this specific server
    const inherited = await getSavedAudioSettings("fresh.com:54736");
    expect(inherited?.bitrate_bps).toBe(112000);
  });

  it("a server's own settings take precedence over the default", async () => {
    await saveAudioSettings(makeSettings({ bitrate_bps: 112000 }));
    await saveAudioSettings(makeSettings({ bitrate_bps: 160000 }), "own.com:54736");
    const own = await getSavedAudioSettings("own.com:54736");
    expect(own?.bitrate_bps).toBe(160000);
  });

  it("migrates a legacy single-blob value into the default bucket", async () => {
    // Simulate the pre-per-server format: the whole AudioSettings object
    // stored directly under AUDIO_KEY.
    storeData[AUDIO_KEY] = makeSettings({ bitrate_bps: 72000 });

    const read = await getSavedAudioSettings();
    expect(read?.bitrate_bps).toBe(72000);

    // The on-disk shape is now a map with the default bucket populated.
    const map = storeData[AUDIO_KEY] as Record<string, AudioSettings>;
    expect(map[AUDIO_DEFAULT_KEY]?.bitrate_bps).toBe(72000);
    // And a brand-new server now inherits that migrated default.
    const inherited = await getSavedAudioSettings("new.com:54736");
    expect(inherited?.bitrate_bps).toBe(72000);
  });
});
