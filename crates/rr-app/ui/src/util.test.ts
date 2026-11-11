import { describe, expect, it } from "vitest";
import { buildTree, eventToAccelerator, meterPosition } from "./util";

const key = (code: string, mods: Partial<Record<"ctrlKey" | "shiftKey" | "altKey" | "metaKey", boolean>> = {}) => ({
  code,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  metaKey: false,
  ...mods,
});

describe("eventToAccelerator", () => {
  it("builds modifier combinations", () => {
    expect(eventToAccelerator(key("Space", { ctrlKey: true, shiftKey: true }))).toBe("Ctrl+Shift+Space");
    expect(eventToAccelerator(key("KeyP", { altKey: true }))).toBe("Alt+P");
  });
  it("refuses plain keys that would eat typing", () => {
    expect(eventToAccelerator(key("KeyA"))).toBeNull();
    expect(eventToAccelerator(key("Space"))).toBeNull();
  });
  it("allows function keys alone and ignores lone modifiers", () => {
    expect(eventToAccelerator(key("F9"))).toBe("F9");
    expect(eventToAccelerator(key("ControlLeft", { ctrlKey: true }))).toBeNull();
  });
});

describe("buildTree", () => {
  it("nests channels and places users", () => {
    const tree = buildTree(
      [
        { id: 0, parent_id: null, name: "root" },
        { id: 1, parent_id: 0, name: "a" },
      ],
      [{ session: 5, name: "u", channel_id: 1, self_mute: false, self_deaf: false }],
    );
    expect(tree).toHaveLength(1);
    expect(tree[0].children[0].users[0].name).toBe("u");
  });
  it("treats a scoped channel with a missing parent as a root", () => {
    const tree = buildTree([{ id: 2, parent_id: 9, name: "scoped" }], []);
    expect(tree).toHaveLength(1);
  });
});

describe("meterPosition", () => {
  it("maps dBFS to 0..1", () => {
    expect(meterPosition(0)).toBe(0);
    expect(meterPosition(1)).toBe(1);
    expect(meterPosition(0.001)).toBeCloseTo(0.0, 5);
    expect(meterPosition(0.1)).toBeCloseTo(2 / 3, 3);
  });
});
