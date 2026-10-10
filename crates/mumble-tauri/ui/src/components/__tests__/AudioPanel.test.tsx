/**
 * Regression tests for AudioPanel features:
 * - Mic test start/stop flow
 * - VU meter rendering
 * - Auto input sensitivity toggle
 * - Shortcut event filtering (Pressed vs Released)
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import type { AudioSettings } from "../../types";

// -- Tauri mocks (must be declared before importing components) ----

const invokeMock = vi.fn<(cmd: string, args?: unknown) => Promise<unknown>>();
const listenMock = vi.fn<(event: string, handler: (event: { payload: unknown }) => void) => Promise<() => void>>();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(args[0] as string, args[1]),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => listenMock(args[0] as string, args[1] as (event: { payload: unknown }) => void),
}));

vi.mock("@tauri-apps/plugin-global-shortcut", () => ({
  register: vi.fn(),
  unregister: vi.fn(),
  isRegistered: vi.fn().mockResolvedValue(false),
}));

vi.mock("@tauri-apps/plugin-store", () => ({
  load: vi.fn().mockResolvedValue({
    get: vi.fn().mockResolvedValue(null),
    set: vi.fn().mockResolvedValue(undefined),
  }),
}));

// Import after mocks are in place.
import { AudioPanel } from "../../pages/settings/AudioPanel";
import { applyGlobalShortcut } from "../../pages/settings/shortcutHelpers";
import { register } from "@tauri-apps/plugin-global-shortcut";

// -- Helpers -------------------------------------------------------

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

function renderPanel(
  overrides: Partial<AudioSettings> = {},
  isExpert = false,
  role: "operator" | "station" = "operator",
) {
  const onChange = vi.fn();
  const onToggleAudioBackend = vi.fn();
  const settings = makeSettings(overrides);
  const result = render(
    <AudioPanel
      devices={[]}
      outputDevices={[]}
      settings={settings}
      onChange={onChange}
      isExpert={isExpert}
      useRodioBackend={false}
      onToggleAudioBackend={onToggleAudioBackend}
      servers={[]}
      selectedServer={null}
      onSelectServer={vi.fn()}
      role={role}
      serverKey="__default__"
    />,
  );
  return { onChange, settings, ...result };
}

// -- Tests ---------------------------------------------------------

beforeEach(() => {
  vi.clearAllMocks();
  invokeMock.mockResolvedValue(undefined);
  listenMock.mockResolvedValue(vi.fn());
});

// -- Activation Mode -----------------------------------------------

describe("Activation Mode selector", () => {
  it("renders three activation mode radio buttons", () => {
    renderPanel();
    expect(screen.getByLabelText("Voice Activation (VOX)")).toBeTruthy();
    expect(screen.getByLabelText("Continuous")).toBeTruthy();
    expect(screen.getByLabelText("Push to Talk")).toBeTruthy();
  });

  it("selects Voice Activation for transmit_mode=voice_activity", () => {
    renderPanel({ transmit_mode: "voice_activity", push_to_talk: false });
    expect(
      (screen.getByLabelText("Voice Activation (VOX)") as HTMLInputElement).checked,
    ).toBe(true);
  });

  it("selects Continuous for transmit_mode=continuous", () => {
    renderPanel({ transmit_mode: "continuous", push_to_talk: false });
    expect(
      (screen.getByLabelText("Continuous") as HTMLInputElement).checked,
    ).toBe(true);
  });

  it("selects Push to Talk for transmit_mode=push_to_talk", () => {
    renderPanel({ transmit_mode: "push_to_talk", push_to_talk: true });
    expect(
      (screen.getByLabelText("Push to Talk") as HTMLInputElement).checked,
    ).toBe(true);
  });

  it("reconciles the legacy push_to_talk flag to PTT", () => {
    // transmit_mode still at its migration default but push_to_talk set.
    renderPanel({ transmit_mode: "voice_activity", push_to_talk: true });
    expect(
      (screen.getByLabelText("Push to Talk") as HTMLInputElement).checked,
    ).toBe(true);
  });

  it("switches to Continuous mode without touching noise suppression", () => {
    const { onChange } = renderPanel({ transmit_mode: "voice_activity" });
    fireEvent.click(screen.getByLabelText("Continuous"));
    expect(onChange).toHaveBeenCalledWith({
      transmit_mode: "continuous",
      push_to_talk: false,
      auto_input_sensitivity: false,
    });
  });

  it("switches to Push to Talk mode", () => {
    const { onChange } = renderPanel({ transmit_mode: "voice_activity" });
    fireEvent.click(screen.getByLabelText("Push to Talk"));
    expect(onChange).toHaveBeenCalledWith({
      transmit_mode: "push_to_talk",
      push_to_talk: true,
      auto_input_sensitivity: false,
    });
  });

  it("switches back to Voice Activation mode", () => {
    const { onChange } = renderPanel({ transmit_mode: "continuous" });
    fireEvent.click(screen.getByLabelText("Voice Activation (VOX)"));
    expect(onChange).toHaveBeenCalledWith({
      transmit_mode: "voice_activity",
      push_to_talk: false,
    });
  });

  it("offers a Station Continuous only", () => {
    renderPanel({ transmit_mode: "continuous" }, false, "station");
    expect(screen.getByLabelText("Continuous")).toBeTruthy();
    expect(screen.queryByLabelText("Voice Activation (VOX)")).toBeNull();
    expect(screen.queryByLabelText("Push to Talk")).toBeNull();
  });

  it("shows Voice Activation settings only in VA mode", () => {
    renderPanel({ transmit_mode: "voice_activity", push_to_talk: false });
    expect(screen.getByText("Activation Mode", { selector: "h3" })).toBeTruthy();
    expect(screen.getByLabelText("Auto Calibrate")).toBeTruthy();
    expect(screen.getByLabelText("Manual Calibrate")).toBeTruthy();
  });

  it("hides Voice Activation settings in Continuous mode", () => {
    renderPanel({ transmit_mode: "continuous", push_to_talk: false });
    expect(screen.queryByLabelText("Auto Calibrate")).toBeNull();
  });

  it("hides Voice Activation settings in PTT mode", () => {
    renderPanel({ transmit_mode: "push_to_talk", push_to_talk: true });
    expect(screen.queryByLabelText("Auto Calibrate")).toBeNull();
  });

  it("shows PTT shortcut hint only in PTT mode", () => {
    // The PTT key binding itself lives in the Shortcuts tab; AudioPanel
    // just shows a pointer to it when push-to-talk is enabled.
    renderPanel({ transmit_mode: "push_to_talk", push_to_talk: true });
    expect(screen.getByText(/Shortcuts/, { selector: "strong" })).toBeTruthy();
  });

  it("hides PTT shortcut hint in Voice Activation mode", () => {
    renderPanel({ transmit_mode: "voice_activity", push_to_talk: false });
    expect(screen.queryByText(/configured in the/)).toBeNull();
  });
});

// -- Stereo output -------------------------------------------------

describe("Stereo output", () => {
  it("renders the three stereo mode cards", () => {
    renderPanel();
    expect(screen.getByLabelText("Stereo")).toBeTruthy();
    expect(screen.getByLabelText("Mono")).toBeTruthy();
    expect(screen.getByLabelText("Swap L/R")).toBeTruthy();
  });

  it("shows L/R volume sliders only in Stereo mode", () => {
    renderPanel({ playback_stereo_mode: "stereo" });
    expect(screen.getByText("Left Volume")).toBeTruthy();
    expect(screen.getByText("Right Volume")).toBeTruthy();
  });

  it("hides L/R volume sliders in Mono mode", () => {
    renderPanel({ playback_stereo_mode: "mono" });
    expect(screen.queryByText("Left Volume")).toBeNull();
    expect(screen.queryByText("Right Volume")).toBeNull();
  });

  it("switches stereo mode", () => {
    const { onChange } = renderPanel({ playback_stereo_mode: "stereo" });
    fireEvent.click(screen.getByLabelText("Mono"));
    expect(onChange).toHaveBeenCalledWith({ playback_stereo_mode: "mono" });
  });
});

// -- Calibration mode selector -------------------------------------

describe("Calibration mode selector", () => {
  it("shows the auto-mode Calibrate button when auto sensitivity is on", () => {
    renderPanel({ auto_input_sensitivity: true, noise_suppression: true });
    expect(screen.getByRole("button", { name: "Calibrate" })).toBeTruthy();
    // Manual sliders should not be shown.
    expect(screen.queryByText("Open")).toBeNull();
  });

  it("shows the manual Open and Close draggable triangles when auto sensitivity is off", () => {
    renderPanel({ auto_input_sensitivity: false, noise_suppression: true });
    // The triangles sitting on the meter are role=slider with aria-labels.
    expect(screen.getByLabelText("Open threshold")).toBeTruthy();
    expect(screen.getByLabelText("Close threshold")).toBeTruthy();
    // Manual mode shows the "Test Mic" button (not "Calibrate").
    expect(screen.getByText("Test Mic")).toBeTruthy();
    expect(screen.queryByText("Calibrate")).toBeNull();
  });

  it("dragging the Open triangle updates vad_threshold", () => {
    const { onChange } = renderPanel({
      auto_input_sensitivity: false,
      noise_suppression: true,
      vad_threshold: 0.05,
    });
    const slider = screen.getByLabelText("Open threshold") as HTMLInputElement;
    fireEvent.change(slider, { target: { value: "50" } });
    // 50% on the dB axis -> 10^(-30/20) ~= 0.0316
    expect(onChange).toHaveBeenCalled();
    const last = onChange.mock.calls[onChange.mock.calls.length - 1][0];
    expect(last.vad_threshold).toBeCloseTo(0.0316, 3);
  });

  it("switches into Auto mode by clicking the Auto Calibrate card", () => {
    const { onChange } = renderPanel({ auto_input_sensitivity: false, noise_suppression: true });
    fireEvent.click(screen.getByLabelText("Auto Calibrate"));
    expect(onChange).toHaveBeenCalledWith({ auto_input_sensitivity: true });
  });

  it("switches into Manual mode by clicking the Manual Calibrate card", () => {
    const { onChange } = renderPanel({ auto_input_sensitivity: true, noise_suppression: true });
    fireEvent.click(screen.getByLabelText("Manual Calibrate"));
    expect(onChange).toHaveBeenCalledWith({ auto_input_sensitivity: false });
  });
});

// -- Voice replay --------------------------------------------------

describe("Voice replay", () => {
  it("renders a Record Sample button in voice activation mode", () => {
    renderPanel({ noise_suppression: true, push_to_talk: false });
    expect(screen.getByText("Record Sample")).toBeTruthy();
  });

  it("invokes start_voice_replay when Record Sample is clicked", async () => {
    renderPanel({ noise_suppression: true, push_to_talk: false });
    await act(async () => {
      fireEvent.click(screen.getByText("Record Sample"));
    });
    expect(invokeMock).toHaveBeenCalledWith("start_voice_replay", undefined);
  });

  it("does not render replay control outside voice activation mode", () => {
    renderPanel({ push_to_talk: true });
    expect(screen.queryByText("Record Sample")).toBeNull();
  });
});

// -- Calibrate (mic test) ------------------------------------------

describe("Calibrate", () => {
  it("renders the Calibrate button in Auto mode", () => {
    renderPanel({ auto_input_sensitivity: true });
    expect(screen.getByRole("button", { name: "Calibrate" })).toBeTruthy();
  });

  it("does not show VU meter when not active", () => {
    const { container } = renderPanel({ auto_input_sensitivity: true });
    expect(container.querySelector("[class*='vuMeter']")).toBeNull();
  });

  it("invokes start_mic_test when Calibrate is clicked", async () => {
    renderPanel({ auto_input_sensitivity: true });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Calibrate" }));
    });
    expect(invokeMock).toHaveBeenCalledWith("start_mic_test", undefined);
  });

  it("shows Stop button after starting", async () => {
    renderPanel({ auto_input_sensitivity: true });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Calibrate" }));
    });
    expect(screen.getByText("Stop")).toBeTruthy();
  });

  it("shows VU meter after starting", async () => {
    renderPanel({ auto_input_sensitivity: true });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Calibrate" }));
    });
    // VU meter should contain dB labels
    expect(screen.getByText("-60")).toBeTruthy();
    expect(screen.getByText("0 dB")).toBeTruthy();
  });

  it("subscribes to mic-amplitude events when active", async () => {
    renderPanel({ auto_input_sensitivity: true });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Calibrate" }));
    });
    expect(listenMock).toHaveBeenCalledWith("mic-amplitude", expect.any(Function));
  });

  it("invokes stop_mic_test when Stop is clicked", async () => {
    renderPanel({ auto_input_sensitivity: true });
    // Start
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Calibrate" }));
    });
    invokeMock.mockClear();
    // Stop
    await act(async () => {
      fireEvent.click(screen.getByText("Stop"));
    });
    expect(invokeMock).toHaveBeenCalledWith("stop_mic_test", undefined);
  });

  it("hides VU meter after stopping", async () => {
    const { container } = renderPanel({ auto_input_sensitivity: true });
    // Start
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Calibrate" }));
    });
    // Stop
    await act(async () => {
      fireEvent.click(screen.getByText("Stop"));
    });
    expect(container.querySelector("[class*='vuMeter']")).toBeNull();
    expect(screen.getByRole("button", { name: "Calibrate" })).toBeTruthy();
  });

  it("does not start if invoke throws", async () => {
    invokeMock.mockImplementation((cmd) =>
      cmd === "start_mic_test"
        ? Promise.reject(new Error("no mic"))
        : Promise.resolve(undefined),
    );
    renderPanel({ auto_input_sensitivity: true });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Calibrate" }));
    });
    // Should still show "Calibrate" (not "Stop")
    expect(screen.getByRole("button", { name: "Calibrate" })).toBeTruthy();
  });

  it("calls stop_mic_test on unmount while active", async () => {
    const { unmount } = renderPanel({ auto_input_sensitivity: true });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Calibrate" }));
    });
    invokeMock.mockClear();
    unmount();
    expect(invokeMock).toHaveBeenCalledWith("stop_mic_test", undefined);
  });
});

// -- Shortcut event filtering (regression for double-fire bug) -----

describe("Shortcut event filtering", () => {
  it("only invokes command on Pressed, not Released", async () => {
    const registerFn = vi.mocked(register);
    registerFn.mockImplementation(async (_shortcut, handler) => {
      // Simulate the global-shortcut plugin firing both events.
      handler({ state: "Pressed", shortcut: "Ctrl+M" } as never);
      handler({ state: "Released", shortcut: "Ctrl+M" } as never);
    });

    await applyGlobalShortcut("Ctrl+M", "toggle_mute");

    // invoke should only have been called once (for Pressed).
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("toggle_mute", undefined);
  });

  it("does not invoke command on Released event", async () => {
    const registerFn = vi.mocked(register);
    registerFn.mockImplementation(async (_shortcut, handler) => {
      handler({ state: "Released", shortcut: "Ctrl+M" } as never);
    });

    await applyGlobalShortcut("Ctrl+M", "toggle_mute");

    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("skips registration for empty shortcut string", async () => {
    const registerFn = vi.mocked(register);
    await applyGlobalShortcut("", "toggle_mute");
    expect(registerFn).not.toHaveBeenCalled();
  });
});
