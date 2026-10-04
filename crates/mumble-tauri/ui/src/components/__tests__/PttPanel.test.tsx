/**
 * Tests for the serial PTT settings panel: role-aware line options and the
 * enable gating. Tauri commands are mocked; the panel loads its config and
 * the serial-port list on mount.
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import type { PttConfig, SerialPortInfo } from "../../types";

const invokeMock = vi.fn<(cmd: string, args?: unknown) => Promise<unknown>>();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(args[0] as string, args[1]),
}));

import { PttPanel } from "../../pages/settings/PttPanel";

function cfg(overrides: Partial<PttConfig> = {}): PttConfig {
  return {
    enabled: true,
    role: "operator",
    port: "",
    line: "cts",
    active_low: false,
    vox: false,
    vox_hang_ms: 150,
    lead_in_ms: 150,
    hang_ms: 250,
    deadman_ms: 2000,
    max_tx_secs: 120,
    ...overrides,
  };
}

const PORTS: SerialPortInfo[] = [{ name: "COM5", label: "COM5 (USB serial)" }];

function mockBackend(config: PttConfig) {
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_ptt_config") return Promise.resolve(config);
    if (command === "list_serial_ports") return Promise.resolve(PORTS);
    return Promise.resolve(undefined);
  });
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("PttPanel", () => {
  // The test harness does not initialise i18next, so t() returns the raw key;
  // assert against DOM structure (line options, by their hardware labels, which
  // are not translated) rather than translated copy.

  it("offers only input lines for an Operator", async () => {
    mockBackend(cfg({ role: "operator", line: "cts" }));
    render(<PttPanel serverId={null} role="operator" />);
    // Input lines present; output lines absent.
    await waitFor(() => expect(screen.getByRole("option", { name: /CTS/ })).toBeTruthy());
    expect(screen.getByRole("option", { name: /DSR/ })).toBeTruthy();
    expect(screen.queryByRole("option", { name: /DTR/ })).toBeNull();
    expect(screen.queryByRole("option", { name: /RTS/ })).toBeNull();
  });

  it("offers only output lines for a Station", async () => {
    mockBackend(cfg({ role: "station", line: "dtr" }));
    render(<PttPanel serverId={null} role="station" />);
    await waitFor(() => expect(screen.getByRole("option", { name: /DTR/ })).toBeTruthy());
    expect(screen.getByRole("option", { name: /RTS/ })).toBeTruthy();
    expect(screen.queryByRole("option", { name: /CTS/ })).toBeNull();
  });

  it("hides the line picker until enabled", async () => {
    mockBackend(cfg({ enabled: false, role: "operator" }));
    render(<PttPanel serverId={null} role="operator" />);
    // Give the mount load a tick, then confirm no line options are rendered.
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_ptt_config", expect.anything()));
    expect(screen.queryByRole("option", { name: /CTS/ })).toBeNull();
  });

  it("loads the serial port list into the picker", async () => {
    mockBackend(cfg({ enabled: true, role: "operator", port: "COM5" }));
    render(<PttPanel serverId={null} role="operator" />);
    await waitFor(() => expect(screen.getByText("COM5 (USB serial)")).toBeTruthy());
  });
});
