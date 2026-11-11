import type { Channel, User } from "./api";

export interface TreeNode {
  channel: Channel;
  users: User[];
  children: TreeNode[];
}

/** Arrange channels into a tree and attach each user to their channel. */
export function buildTree(channels: Channel[], users: User[]): TreeNode[] {
  const nodes = new Map<number, TreeNode>();
  for (const channel of channels) nodes.set(channel.id, { channel, users: [], children: [] });
  for (const u of users) nodes.get(u.channel_id)?.users.push(u);
  const roots: TreeNode[] = [];
  for (const channel of channels) {
    const node = nodes.get(channel.id);
    if (!node) continue;
    const parent = channel.parent_id === null ? undefined : nodes.get(channel.parent_id);
    (parent ? parent.children : roots).push(node);
  }
  return roots;
}

const MODIFIER_CODES = new Set([
  "ControlLeft",
  "ControlRight",
  "ShiftLeft",
  "ShiftRight",
  "AltLeft",
  "AltRight",
  "MetaLeft",
  "MetaRight",
]);

function keyName(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  const named: Record<string, string> = {
    Space: "Space",
    Enter: "Enter",
    Tab: "Tab",
    ArrowUp: "Up",
    ArrowDown: "Down",
    ArrowLeft: "Left",
    ArrowRight: "Right",
    Home: "Home",
    End: "End",
    PageUp: "PageUp",
    PageDown: "PageDown",
    Insert: "Insert",
    Delete: "Delete",
    Backquote: "`",
    Minus: "-",
    Equal: "=",
    Comma: ",",
    Period: ".",
    Slash: "/",
  };
  return named[code] ?? null;
}

/**
 * Turn a key press into a global-shortcut accelerator such as `Ctrl+Shift+Space`.
 * Returns `null` for a lone modifier, an unsupported key, or a plain key with
 * no modifier (those would steal ordinary typing system-wide). Function keys
 * are allowed on their own.
 */
export function eventToAccelerator(e: Pick<KeyboardEvent, "code" | "ctrlKey" | "shiftKey" | "altKey" | "metaKey">): string | null {
  if (MODIFIER_CODES.has(e.code)) return null;
  const key = keyName(e.code);
  if (!key) return null;
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Super");
  const isFunctionKey = /^F\d+$/.test(key);
  if (mods.length === 0 && !isFunctionKey) return null;
  return [...mods, key].join("+");
}

/** Linear 0..1 peak to a 0..1 meter position on a -60..0 dBFS scale. */
export function meterPosition(peak: number): number {
  if (peak <= 0) return 0;
  const db = 20 * Math.log10(peak);
  return Math.min(1, Math.max(0, (db + 60) / 60));
}
