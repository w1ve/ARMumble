import { create } from "zustand";
import {
  api,
  onLevels,
  onRoute,
  onStatus,
  type Device,
  type Levels,
  type Port,
  type Route,
  type ServerEntry,
  type Settings,
  type Status,
} from "./api";
import { syncServer } from "./util";

const IDLE: Status = {
  phase: "disconnected",
  error: null,
  scope_missing: false,
  udp: null,
  own_session: null,
  own_channel: null,
  users: [],
  channels: [],
  speakers: [],
  plan: null,
  transmitting: false,
  ptt_down: false,
  sent: 0,
  dropped: 0,
  audio_errors: 0,
  audio_error: "",
  remote_ptt: [],
  cat: [],
};

const NO_LEVELS: Levels = { in_l: 0, in_r: 0, out_l: 0, out_r: 0, in_clips: 0, out_clips: 0, live: false };

const NO_ROUTE: Route = {
  enabled: false,
  role: "operator",
  tx: 1,
  rx: "both",
  source: "manual",
  otrsp_available: false,
  otrsp_local: false,
  otrsp_error: null,
};

interface AppStore {
  status: Status;
  /** Two-radio routing, live from the backend. */
  route: Route;
  /** Meter levels, updated 25 times a second. */
  levels: Levels;
  settings: Settings | null;
  /** The user's server list. */
  servers: ServerEntry[];
  /** Pick a server from the list for the next connection. */
  selectServer: (name: string) => void;
  /** Validate and save a new list (throws with the reason if invalid). */
  saveServers: (list: ServerEntry[]) => Promise<void>;
  devices: Device[];
  ports: Port[];
  busy: boolean;
  actionError: string | null;
  init: () => Promise<void>;
  /** Edit settings; the change is saved shortly after and applied live. */
  update: (edit: (s: Settings) => void) => void;
  refreshHardware: () => Promise<void>;
  connect: () => Promise<void>;
  disconnect: () => Promise<void>;
}

let saveTimer: ReturnType<typeof setTimeout> | undefined;
let started = false;

export const useApp = create<AppStore>((set, get) => ({
  status: IDLE,
  route: NO_ROUTE,
  levels: NO_LEVELS,
  settings: null,
  servers: [],
  devices: [],
  ports: [],
  busy: false,
  actionError: null,

  async init() {
    if (started) return;
    started = true;
    await onStatus((status) => set({ status }));
    await onLevels((levels) => set({ levels }));
    await onRoute((route) => set({ route }));
    set({ route: await api.getRoute() });
    set({ settings: await api.loadSettings() });
    try {
      const servers = await api.listServers();
      set({ servers });
      const cur = get().settings;
      if (cur) {
        const next = syncServer(cur.server, servers);
        if (JSON.stringify(next) !== JSON.stringify(cur.server)) get().update((x) => void (x.server = next));
      }
    } catch (e) {
      set({ actionError: String(e) });
    }
    await get().refreshHardware();
  },

  update(edit) {
    const current = get().settings;
    if (!current) return;
    const next = structuredClone(current);
    edit(next);
    set({ settings: next });
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => {
      api.saveSettings(next).catch((e: unknown) => set({ actionError: String(e) }));
    }, 250);
  },

  selectServer(name) {
    const pick = get().servers.find((s) => s.name === name);
    if (!pick) return;
    get().update((x) => {
      x.server = { ...x.server, ...syncServer(x.server, [pick]) };
    });
  },

  async saveServers(list) {
    const saved = await api.saveServers(list);
    set({ servers: saved });
    const cur = get().settings;
    if (cur) {
      const next = syncServer(cur.server, saved);
      if (JSON.stringify(next) !== JSON.stringify(cur.server)) get().update((x) => void (x.server = next));
    }
  },

  async refreshHardware() {
    const [devices, ports] = await Promise.all([api.listDevices(), api.listPorts()]);
    set({ devices, ports });
  },

  async connect() {
    const s = get().settings;
    if (!s) return;
    set({ busy: true, actionError: null });
    try {
      clearTimeout(saveTimer);
      await api.saveSettings(s);
      await api.connect();
    } catch (e) {
      set({ actionError: String(e) });
    } finally {
      set({ busy: false });
    }
  },

  async disconnect() {
    set({ busy: true });
    try {
      await api.disconnect();
    } finally {
      set({ busy: false });
    }
  },
}));
