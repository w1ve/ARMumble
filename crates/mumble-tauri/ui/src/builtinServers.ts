/**
 * Built-in Remote Radio servers.
 *
 * Remote.Radio Mumble is an amateur-radio-only client, so instead of browsing
 * the public Mumble directory it ships a curated list of the Remote Radio
 * network's own servers. They are free to use and the password is intentionally
 * shippable — these are shared demo/community servers, not private ones.
 *
 * Each location runs TWO servers on the same host:
 *   • a LEGACY standard-Mumble server on the default voice port 64738, and
 *   • a FANCY (stereo-capable) server on port 54736.
 * The list shows both as separate cards; a TCP availability check on the voice
 * port enables/greys each one. The list is hard-coded on purpose: it is part of
 * the product, not user data, so it is not persisted and cannot be edited.
 */

/** Default Mumble voice port — the LEGACY (standard Mumble 1.4/1.5) servers. */
export const RR_PORT = 64738;

/** Voice port of the FANCY (stereo-capable) servers at each location. */
export const RR_FANCY_PORT = 54736;

/** Shared password for the Remote Radio network (safe to ship). */
export const RR_PASSWORD = "Demo!";

/** Which server software a built-in entry targets. */
export type ServerFlavor = "legacy" | "fancy";

/** One built-in server entry. */
export interface BuiltinServer {
  /** City / display name, e.g. "New York City". */
  name: string;
  /** Fully-qualified host, e.g. "newyork.remote.radio". */
  host: string;
  /** Short square-icon label (2-3 letters), e.g. "NYC". */
  abbr: string;
  /** Voice port this entry connects to (64738 legacy, 54736 fancy). */
  port: number;
  /** Server software flavour at this port. */
  flavor: ServerFlavor;
}

/** Stable key for a built-in entry (host + port uniquely identify it). */
export function builtinKey(s: Pick<BuiltinServer, "host" | "port">): string {
  return `${s.host}:${s.port}`;
}

/** The locations in the Remote Radio network (one row of config per city). */
const RR_LOCATIONS: readonly { name: string; host: string; abbr: string }[] = [
  { name: "New York City", host: "newyork.remote.radio", abbr: "NYC" },
  { name: "Toronto", host: "toronto.remote.radio", abbr: "TOR" },
  { name: "London", host: "london.remote.radio", abbr: "LON" },
  { name: "Los Angeles", host: "la.remote.radio", abbr: "LA" },
  { name: "South Africa", host: "africa.remote.radio", abbr: "ZA" },
  { name: "Winnipeg", host: "winnipeg.remote.radio", abbr: "WPG" },
  { name: "Frankfurt", host: "frankfurt.remote.radio", abbr: "FRA" },
];

/**
 * The sign-in list: for every location, a FANCY (stereo) card first, then the
 * LEGACY card. Edited via {@link RR_LOCATIONS} and nowhere else.
 */
export const BUILTIN_SERVERS: readonly BuiltinServer[] = RR_LOCATIONS.flatMap((loc) => [
  { ...loc, port: RR_FANCY_PORT, flavor: "fancy" as const },
  { ...loc, port: RR_PORT, flavor: "legacy" as const },
]);

/** Case-insensitive filter over name and host, for the search box. */
export function filterBuiltinServers(query: string): readonly BuiltinServer[] {
  const q = query.trim().toLowerCase();
  if (!q) return BUILTIN_SERVERS;
  return BUILTIN_SERVERS.filter(
    (s) => s.name.toLowerCase().includes(q) || s.host.toLowerCase().includes(q),
  );
}
