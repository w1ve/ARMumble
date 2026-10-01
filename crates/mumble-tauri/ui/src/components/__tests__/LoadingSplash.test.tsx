/**
 * Unit tests for the LoadingSplash component.
 *
 * Verifies that the rotating subtitle cycles, that the override
 * `message` prop pins the text, and that the message pool is sane.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act } from "@testing-library/react";
import LoadingSplash, {
  __TEST_FUNNY_MESSAGES,
} from "../elements/LoadingSplash";

describe("LoadingSplash", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("renders a non-empty brand title", () => {
    render(<LoadingSplash />);
    const status = screen.getByRole("status");
    // Children are: spinner (empty, aria-hidden), title, subtitle. The
    // brand title is the middle child; assert it is non-empty. The exact
    // string depends on resolved app mode / i18n init, so presence only.
    // (The funny-message pool is covered by the rotation test below.)
    const heading = status.children[1]?.textContent ?? "";
    expect(heading.length).toBeGreaterThan(0);
  });

  it("uses an explicit title verbatim", () => {
    render(<LoadingSplash title="Custom Brand" message="x" />);
    expect(screen.getByText("Custom Brand")).toBeTruthy();
  });

  it("rotates the subtitle on a timer", () => {
    render(<LoadingSplash />);
    const status = screen.getByRole("status");
    const initial = status.lastElementChild?.textContent ?? "";

    // Advance enough for at least a couple of rotations.
    act(() => {
      vi.advanceTimersByTime(1800 * 5);
    });
    const later = status.lastElementChild?.textContent ?? "";
    expect(__TEST_FUNNY_MESSAGES).toContain(later);
    // The pool has 17+ entries, so 5 rotations should reach a different one.
    expect(later).not.toBe(initial);
  });

  it("respects an explicit message and never rotates it", () => {
    render(<LoadingSplash message="Pinned text" />);
    expect(screen.getByText("Pinned text")).toBeTruthy();
    act(() => {
      vi.advanceTimersByTime(1800 * 10);
    });
    expect(screen.getByText("Pinned text")).toBeTruthy();
  });

  it("ships a non-empty pool of unique messages", () => {
    expect(__TEST_FUNNY_MESSAGES.length).toBeGreaterThanOrEqual(8);
    const unique = new Set(__TEST_FUNNY_MESSAGES);
    expect(unique.size).toBe(__TEST_FUNNY_MESSAGES.length);
  });
});
