import { describe, expect, it } from "vitest";
import { buildTree, eventToAccelerator, meterPosition, serverListError, syncServer } from "./util";

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

describe("syncServer", () => {
  const list = [
    { name: "NYC", host: "newyork.remote.radio", port: 54736, password: "a" },
    { name: "Mine", host: "10.0.0.5", port: 64738, password: "b" },
  ];
  const cur = (o: Partial<{ name: string; host: string; port: number }>) => ({
    name: "",
    host: "",
    port: 64738,
    username: "W1VE",
    password: "",
    scope: "Ch",
    ...o,
  });

  it("follows edits to the chosen server and keeps login details", () => {
    const edited = [{ ...list[0], host: "nyc2.example", password: "new" }, list[1]];
    const s = syncServer(cur({ name: "NYC", host: "newyork.remote.radio", port: 54736 }), edited);
    expect(s).toMatchObject({ name: "NYC", host: "nyc2.example", password: "new", username: "W1VE", scope: "Ch" });
  });
  it("adopts old settings by address and falls back to the first server", () => {
    expect(syncServer(cur({ host: "10.0.0.5", port: 64738 }), list).name).toBe("Mine");
    expect(syncServer(cur({ name: "Gone" }), list).name).toBe("NYC");
  });
  it("chooses nothing when the list is empty", () => {
    expect(syncServer(cur({ name: "NYC", host: "x" }), [])).toMatchObject({ name: "", host: "" });
  });
});

describe("serverListError", () => {
  const ok = { name: "A", host: "h.example", port: 1, password: "" };
  it("accepts a good list and flags the first bad row", () => {
    expect(serverListError([ok])).toBeNull();
    expect(serverListError([ok, { ...ok, name: " " }])).toEqual({ row: 1, key: "servers.errName" });
    expect(serverListError([ok, { ...ok, name: "a" }])).toEqual({ row: 1, key: "servers.errDuplicate" });
    expect(serverListError([{ ...ok, host: "a b" }])?.key).toBe("servers.errHost");
    expect(serverListError([{ ...ok, port: 0 }])?.key).toBe("servers.errPort");
    expect(serverListError([{ ...ok, port: NaN }])?.key).toBe("servers.errPort");
  });
});
