/** The Remote Radio network: every city runs a stereo server and a standard one. */
export const RR_PASSWORD = "Demo!";
export const RR_STEREO_PORT = 54736;
export const RR_STANDARD_PORT = 64738;

const CITIES: { name: string; host: string }[] = [
  { name: "New York City", host: "newyork.remote.radio" },
  { name: "Toronto", host: "toronto.remote.radio" },
  { name: "London", host: "london.remote.radio" },
  { name: "Los Angeles", host: "la.remote.radio" },
  { name: "South Africa", host: "africa.remote.radio" },
  { name: "Winnipeg", host: "winnipeg.remote.radio" },
  { name: "Frankfurt", host: "frankfurt.remote.radio" },
];

export interface BuiltinServer {
  name: string;
  host: string;
  port: number;
  stereo: boolean;
}

export const BUILTIN_SERVERS: BuiltinServer[] = CITIES.flatMap((c) => [
  { ...c, port: RR_STEREO_PORT, stereo: true },
  { ...c, port: RR_STANDARD_PORT, stereo: false },
]);

export const serverKey = (s: { host: string; port: number }) => `${s.host}:${s.port}`;