import {
  register,
  unregister,
  isRegistered,
} from "@tauri-apps/plugin-global-shortcut";
import { invoke } from "@tauri-apps/api/core";
import { load } from "../../utils/store";

export interface ShortcutBindings {
  // Voice - global
  pushToTalk: string;
  toggleMute: string;
  toggleDeafen: string;
  voicePriority: string;
  // Voice - in-app
  toggleActivationMode: string;
  // Channel navigation - in-app
  moveChannelUp: string;
  moveChannelDown: string;
  jumpToRootChannel: string;
  toggleChannelSidebar: string;
  toggleMemberPanel: string;
  openQuickSearch: string;
  openQuickSwitcher: string;
  // Window - in-app
  openSettings: string;
  toggleFullscreen: string;
  toggleDevOverlay: string;
}

export const DEFAULT_SHORTCUTS: ShortcutBindings = {
  pushToTalk: "",
  toggleMute: "Ctrl+Shift+M",
  toggleDeafen: "Ctrl+Shift+D",
  voicePriority: "",
  toggleActivationMode: "",
  moveChannelUp: "Alt+ArrowUp",
  moveChannelDown: "Alt+ArrowDown",
  jumpToRootChannel: "Alt+Home",
  toggleChannelSidebar: "Ctrl+B",
  toggleMemberPanel: "Ctrl+U",
  openQuickSearch: "Ctrl+F",
  openQuickSwitcher: "Ctrl+Shift+F",
  openSettings: "Ctrl+,",
  toggleFullscreen: "F11",
  toggleDevOverlay: "Ctrl+Shift+I",
};

/** Shortcut keys that require OS-level global registration. */
const GLOBAL_SHORTCUT_COMMANDS: Partial<Record<keyof ShortcutBindings, string>> = {
  toggleMute: "toggle_mute",
  toggleDeafen: "toggle_deafen",
};

/** Shortcut keys that use press-and-release (PTT-style) global registration. */
const PTT_SHORTCUT_COMMANDS: Partial<Record<keyof ShortcutBindings, { start: string; end: string }>> = {
  pushToTalk: { start: "push_to_talk_start", end: "push_to_talk_end" },
  voicePriority: { start: "voice_priority_start", end: "voice_priority_end" },
};

const SHORTCUT_STORE = "shortcuts.json";

export async function loadShortcuts(): Promise<ShortcutBindings> {
  const store = await load(SHORTCUT_STORE, { autoSave: true, defaults: {} });
  const saved = await store.get<Partial<ShortcutBindings>>("shortcuts");
  return { ...DEFAULT_SHORTCUTS, ...saved };
}

export async function saveShortcuts(shortcuts: ShortcutBindings): Promise<void> {
  const store = await load(SHORTCUT_STORE, { autoSave: true, defaults: {} });
  await store.set("shortcuts", shortcuts);
}

/**
 * Translate a `KeyboardEvent.code` (physical key, layout-independent) into the
 * accelerator token the Tauri global-shortcut plugin's parser understands
 * (the `global-hotkey` crate). We use `event.code` rather than `event.key`
 * because `key` is layout- and shift-dependent (e.g. Shift+1 yields "!", which
 * the parser rejects), whereas `code` is stable ("Digit1", "KeyW", "Comma").
 *
 * Returns null for codes the parser cannot accept, so the caller skips binding
 * an unregisterable combo instead of silently producing a dead shortcut.
 */
function codeToAcceleratorToken(code: string): string | null {
  // KeyA..KeyZ, Digit0..Digit9, F1..F24, Arrow*, Numpad* and most named keys
  // are accepted verbatim by the parser.
  if (/^(Key[A-Z]|Digit[0-9]|F[0-9]{1,2}|Numpad[0-9A-Za-z]+|Arrow(Up|Down|Left|Right))$/.test(code)) {
    return code;
  }
  // Punctuation / named keys whose `code` matches a parser token directly.
  const direct = new Set([
    "Backquote", "Backslash", "BracketLeft", "BracketRight", "Comma", "Equal",
    "Minus", "Period", "Quote", "Semicolon", "Slash", "Space", "Tab", "Enter",
    "Backspace", "Delete", "Home", "End", "PageUp", "PageDown", "Insert",
    "Escape", "CapsLock", "PrintScreen", "ScrollLock", "Pause", "ContextMenu",
    "NumLock",
  ]);
  if (direct.has(code)) return code;
  return null;
}

export function eventToShortcut(e: React.KeyboardEvent): string | null {
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Super");
  // Modifier-only presses are not a complete shortcut.
  if (["ControlLeft", "ControlRight", "AltLeft", "AltRight", "ShiftLeft",
       "ShiftRight", "MetaLeft", "MetaRight"].includes(e.code)) {
    return null;
  }
  const token = codeToAcceleratorToken(e.code);
  if (!token) return null;
  parts.push(token);
  return parts.join("+");
}

export async function applyGlobalShortcut(
  shortcut: string,
  command: string,
): Promise<void> {
  if (!shortcut) return;
  try {
    if (await isRegistered(shortcut)) await unregister(shortcut);
    await register(shortcut, (event) => {
      if (event.state === "Pressed") {
        invoke(command).catch(console.error);
      }
    });
  } catch (e) {
    console.warn(`Failed to register shortcut "${shortcut}":`, e);
  }
}

export async function applyPttShortcut(
  shortcut: string,
  startCommand: string,
  endCommand: string,
): Promise<void> {
  if (!shortcut) return;
  try {
    if (await isRegistered(shortcut)) await unregister(shortcut);
    await register(shortcut, (event) => {
      if (event.state === "Pressed") {
        invoke(startCommand).catch(console.error);
      } else if (event.state === "Released") {
        invoke(endCommand).catch(console.error);
      }
    });
  } catch (e) {
    console.warn(`Failed to register PTT shortcut "${shortcut}":`, e);
  }
}

/**
 * Register the operator software-PTT hotkey. Unlike {@link applyPttShortcut}
 * (which invokes argless start/end commands), the operator PTT is a single
 * command taking `{ down }`, so press/release map to `operator_ptt({down:true})`
 * / `operator_ptt({down:false})`. While held it opens the Mumble mic AND keys
 * the remote station (CONTEXT §14d, mode B).
 */
export async function applyOperatorPttShortcut(shortcut: string): Promise<void> {
  if (!shortcut) return;
  try {
    if (await isRegistered(shortcut)) await unregister(shortcut);
    await register(shortcut, (event) => {
      if (event.state === "Pressed") {
        invoke("operator_ptt", { down: true }).catch(console.error);
      } else if (event.state === "Released") {
        invoke("operator_ptt", { down: false }).catch(console.error);
      }
    });
  } catch (e) {
    console.warn(`Failed to register operator PTT shortcut "${shortcut}":`, e);
  }
}

/**
 * Register the operator "Whisper" hotkey. While held it redirects the mic into
 * the configured target channel via a Mumble VoiceTarget (the backend
 * `whisper_target` command opens the mic, stamps target id 1, and reverts on
 * release). Press/release map to `whisper_target({ active })`.
 */
export async function applyWhisperShortcut(shortcut: string): Promise<void> {
  if (!shortcut) return;
  try {
    if (await isRegistered(shortcut)) await unregister(shortcut);
    await register(shortcut, (event) => {
      if (event.state === "Pressed") {
        invoke("whisper_target", { active: true }).catch(console.error);
      } else if (event.state === "Released") {
        invoke("whisper_target", { active: false }).catch(console.error);
      }
    });
  } catch (e) {
    console.warn(`Failed to register whisper shortcut "${shortcut}":`, e);
  }
}

export async function clearGlobalShortcut(shortcut: string): Promise<void> {
  if (!shortcut) return;
  try {
    if (await isRegistered(shortcut)) await unregister(shortcut);
  } catch {
    /* ignore */
  }
}

/** Apply (or clear) all global shortcuts from a bindings object. */
export async function applyAllGlobalShortcuts(bindings: ShortcutBindings): Promise<void> {
  for (const [key, cmd] of Object.entries(GLOBAL_SHORTCUT_COMMANDS)) {
    const shortcut = bindings[key as keyof ShortcutBindings];
    if (shortcut) {
      await applyGlobalShortcut(shortcut, cmd);
    }
  }
  for (const [key, cmds] of Object.entries(PTT_SHORTCUT_COMMANDS)) {
    const shortcut = bindings[key as keyof ShortcutBindings];
    if (shortcut) {
      await applyPttShortcut(shortcut, cmds.start, cmds.end);
    }
  }
}

/** Determine how to (re)register a shortcut when the user changes it.
 *  Returns false if the key is in-app only (no global registration needed). */
export async function applyChangedShortcut(
  key: keyof ShortcutBindings,
  prev: string,
  next: string,
): Promise<void> {
  await clearGlobalShortcut(prev);
  const toggleCmd = GLOBAL_SHORTCUT_COMMANDS[key];
  if (toggleCmd) {
    await applyGlobalShortcut(next, toggleCmd);
    return;
  }
  const pttCmds = PTT_SHORTCUT_COMMANDS[key];
  if (pttCmds) {
    await applyPttShortcut(next, pttCmds.start, pttCmds.end);
  }
}
