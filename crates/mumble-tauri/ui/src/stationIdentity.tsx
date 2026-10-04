/**
 * Station-identity hinting for the channel user list.
 *
 * Mumble carries no wire signal for "this user is a radio station vs an
 * operator" — that distinction is a Remote.Radio concept. The authoritative
 * answer is a role broadcast each client sends over the hidden
 * `remote-radio/role` plugin-message channel (see `recordPeerRole` /
 * `peerIsStation`), keyed by Mumble session id.
 *
 * A best-effort name hint remains as a fallback for peers that have not (yet)
 * announced a role — e.g. a stock Mumble client, or before the broadcast
 * arrives: a channel user is treated as a station if their name matches one of
 * the saved servers we connected to with the `station` role. The authoritative
 * map always wins when present.
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

// --- Authoritative role map (fed by remote-radio/role plugin broadcasts) ----

/** Event fired when the authoritative session->role map changes. */
const PEER_ROLES_CHANGED_EVENT = "rr:peer-roles-changed";

/** Mumble session id -> whether that peer announced itself a station. The
 *  authoritative replacement for the saved-server name match: it covers any
 *  peer that ran a role broadcast, not just ones we have saved. */
const peerStationSessions = new Map<number, boolean>();

/**
 * Record a peer's announced role (from an inbound `remote-radio/role`
 * plugin message). `session` is the sender's Mumble session id.
 */
export function recordPeerRole(session: number, role: "operator" | "station"): void {
  peerStationSessions.set(session, role === "station");
  globalThis.dispatchEvent(new CustomEvent(PEER_ROLES_CHANGED_EVENT));
}

/** Forget a peer's role when they leave the server. */
export function forgetPeerRole(session: number): void {
  if (peerStationSessions.delete(session)) {
    globalThis.dispatchEvent(new CustomEvent(PEER_ROLES_CHANGED_EVENT));
  }
}

/** Clear all announced roles (e.g. on disconnect). */
export function clearPeerRoles(): void {
  if (peerStationSessions.size > 0) {
    peerStationSessions.clear();
    globalThis.dispatchEvent(new CustomEvent(PEER_ROLES_CHANGED_EVENT));
  }
}

/** Authoritative station flag for a session id, or `undefined` if that peer
 *  has not announced a role (caller falls back to the name hint). */
export function peerIsStation(session: number | null | undefined): boolean | undefined {
  if (session == null) return undefined;
  return peerStationSessions.get(session);
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

/**
 * Convenience hook: returns a predicate `(name, session?) => boolean` for
 * stations. When a `session` id is given and that peer has announced its role,
 * the authoritative broadcast wins; otherwise it falls back to the saved-server
 * name hint. Re-renders when either the name set or the role map changes.
 */
export function useIsStation(): (name: string, session?: number | null) => boolean {
  const names = useStationNames();
  // Re-render on authoritative role changes (the map itself is module state).
  const [, bump] = useState(0);
  useEffect(() => {
    const onChange = () => bump((n) => n + 1);
    globalThis.addEventListener(PEER_ROLES_CHANGED_EVENT, onChange);
    return () => globalThis.removeEventListener(PEER_ROLES_CHANGED_EVENT, onChange);
  }, []);
  return (name: string, session?: number | null) =>
    peerIsStation(session) ?? isStationName(name, names);
}
