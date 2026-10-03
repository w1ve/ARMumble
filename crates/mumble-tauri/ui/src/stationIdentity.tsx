/**
 * Station-identity hinting for the channel user list.
 *
 * Mumble carries no wire signal for "this user is a radio station vs an
 * operator" — that distinction is a Remote.Radio concept. The authoritative
 * solution (planned) is to broadcast each client's role over the Fancy
 * plugin-data channel, the same hidden channel used for PTT. Until that lands,
 * this module provides a best-effort hint: a channel user is treated as a
 * station if their name matches one of the saved servers we connected to with
 * the `station` role.
 *
 * It is deliberately a hint, not a guarantee: it only covers stations the user
 * has saved, and it is replaced wholesale once the plugin-data role broadcast
 * exists.
 */

import { useEffect, useState, type ReactNode } from "react";
import { getSavedServers } from "./serverStorage";
import { RadioIcon } from "./icons";

/** Event fired when the saved-server list changes, so hints refresh. */
const SERVERS_CHANGED_EVENT = "rr:servers-changed";

/** Notify listeners that saved servers changed (call after add/edit/remove). */
export function notifyServersChanged(): void {
  globalThis.dispatchEvent(new CustomEvent(SERVERS_CHANGED_EVENT));
}

/** Load the lower-cased set of usernames saved with the `station` role. */
async function loadStationNames(): Promise<ReadonlySet<string>> {
  try {
    const servers = await getSavedServers();
    const names = servers
      .filter((s) => s.role === "station" && s.username.trim().length > 0)
      .map((s) => s.username.trim().toLowerCase());
    return new Set(names);
  } catch {
    return new Set();
  }
}

/**
 * React hook returning the set of station usernames (lower-cased). Refreshes
 * when the saved-server list changes.
 */
export function useStationNames(): ReadonlySet<string> {
  const [names, setNames] = useState<ReadonlySet<string>>(() => new Set());

  useEffect(() => {
    let cancelled = false;
    const refresh = () => {
      void loadStationNames().then((n) => {
        if (!cancelled) setNames(n);
      });
    };
    refresh();
    globalThis.addEventListener(SERVERS_CHANGED_EVENT, refresh);
    return () => {
      cancelled = true;
      globalThis.removeEventListener(SERVERS_CHANGED_EVENT, refresh);
    };
  }, []);

  return names;
}

/** Whether `name` is (hinted to be) a station identity. */
export function isStationName(
  name: string,
  stationNames: ReadonlySet<string>,
): boolean {
  return stationNames.has(name.trim().toLowerCase());
}

/**
 * The avatar glyph for a user: the RadioTower icon for a station, otherwise the
 * caller's own fallback (usually the first initial). One place so every roster
 * — channel sidebar, member lists, hover cards, profile panels — shows the
 * identical station mark.
 *
 * `size` is the icon pixel size; pass the same number the surrounding avatar
 * uses for its text so the glyph sits centred.
 */
export function StationGlyph({
  isStation,
  size,
  fallback,
}: {
  isStation: boolean;
  size: number;
  fallback: ReactNode;
}): ReactNode {
  return isStation ? <RadioIcon width={size} height={size} /> : fallback;
}

/** Convenience hook: returns a predicate `(name) => boolean` for stations. */
export function useIsStation(): (name: string) => boolean {
  const names = useStationNames();
  return (name: string) => isStationName(name, names);
}
