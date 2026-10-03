/**
 * Built-in Remote Radio servers.
 *
 * Remote.Radio Mumble is an amateur-radio-only client, so instead of browsing
 * the public Mumble directory it ships a curated list of the Remote Radio
 * network's own servers. They are free to use and the password is intentionally
 * shippable — these are shared demo/community servers, not private ones.
 *
 * All run the Fancy Mumble 1.6 server on port 54736. The list is hard-coded on
 * purpose: it is part of the product, not user data, so it is not persisted and
 * cannot be edited or deleted. A user who wants a different server adds it as a
 * normal saved server.
 */

/** Port every Remote Radio server listens on. */
export const RR_PORT = 54736;

/** Shared password for the Remote Radio network (safe to ship). */
export const RR_PASSWORD = "Demo!";

/** One built-in server entry. */
export interface BuiltinServer {
  /** City / display name, e.g. "New York City". */
  name: string;
  /** Fully-qualified host, e.g. "newyork.remote.radio". */
  host: string;
  /** Short square-icon label (2-3 letters), e.g. "NYC". */
  abbr: string;
}

/**
 * The Remote Radio network, in the order shown in the sign-in list. Edited
 * here and nowhere else.
 */
export const BUILTIN_SERVERS: readonly BuiltinServer[] = [
  { name: "New York City", host: "newyork.remote.radio", abbr: "NYC" },
  { name: "Toronto", host: "toronto.remote.radio", abbr: "TOR" },
  { name: "London", host: "london.remote.radio", abbr: "LON" },
  { name: "Los Angeles", host: "la.remote.radio", abbr: "LA" },
  { name: "South Africa", host: "africa.remote.radio", abbr: "ZA" },
  { name: "Winnipeg", host: "winnipeg.remote.radio", abbr: "WPG" },
  { name: "Frankfurt", host: "frankfurt.remote.radio", abbr: "FRA" },
];

/** Case-insensitive filter over name and host, for the search box. */
export function filterBuiltinServers(query: string): readonly BuiltinServer[] {
  const q = query.trim().toLowerCase();
  if (!q) return BUILTIN_SERVERS;
  return BUILTIN_SERVERS.filter(
    (s) => s.name.toLowerCase().includes(q) || s.host.toLowerCase().includes(q),
  );
}
