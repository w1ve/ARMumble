/**
 * Persistent storage for user preferences (app-wide settings)
 * using `@tauri-apps/plugin-store` (Tauri Store v2).
 */

import { load } from "./utils/store";
import type {
  AudioSettings,
  ClientRole,
  NotificationSoundSettings,
  PttConfig,
  RegisteredUser,
  UserMode,
  UserPreferences,
} from "./types";

const STORE_FILE = "preferences.json";
const KEY = "preferences";
const AUDIO_KEY = "audioSettings";
const PTT_KEY = "pttConfig";

const DEFAULTS: UserPreferences = {
  userMode: "normal",
  hasCompletedSetup: false,
  defaultUsername: "",
  defaultRole: "operator",
  timeFormat: "auto",
  convertToLocalTime: true,
  dateFormat: "auto",
  numberFormat: "auto",
  enableNotifications: true,
  enableDualPath: false,
  debugLogging: false,
  logLevel: "info",
  logToFile: false,
  terminalLogging: false,
  autoZipLogs: false,
  calibrationSignature: null,
  serverTabOrder: [],
  disableTypingIndicators: false,
  disableOsmMaps: false,
  disableLinkPreviews: false,
  enableExternalEmbeds: false,
  streamerMode: false,
  autoReconnect: false,
  autoUpdateOnStartup: false,
  skippedUpdateVersion: null,
  persistDms: false,
  hideEmptyChannels: false,
  welcomeMessageDisplay: "once",
  showDisconnectWarning: true,
};

async function getStore() {
  return load(STORE_FILE, { autoSave: true, defaults: {} });
}

// In-flight + cached load promise so the many components that call
// getPreferences() on mount share a single IPC roundtrip.
let cachedPrefs: Promise<UserPreferences> | null = null;

/** Return the current user preferences, falling back to defaults. */
export async function getPreferences(): Promise<UserPreferences> {
  if (cachedPrefs) return cachedPrefs;
  cachedPrefs = (async () => {
    const store = await getStore();
    const prefs = await store.get<UserPreferences>(KEY);
    return prefs ? { ...DEFAULTS, ...prefs } : { ...DEFAULTS };
  })();
  try {
    return await cachedPrefs;
  } catch (e) {
    cachedPrefs = null;
    throw e;
  }
}

/** Persist the full preferences object. */
export async function setPreferences(
  prefs: UserPreferences,
): Promise<void> {
  const store = await getStore();
  await store.set(KEY, prefs);
  cachedPrefs = Promise.resolve(prefs);
  // Signal listeners (e.g. the server tab bar) so they can react to a pref
  // change without remounting.
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent("preferences-changed", { detail: prefs }));
  }
}

// Serializes preference writes. Each updatePreferences() does a read-modify-
// write of the shared `preferences` object; without serialization two
// overlapping calls both read the same `current` and the later-completing one
// silently drops the other's fields. The callers are mostly fire-and-forget
// (`.catch(() => {})`), so overlap is common - e.g. rapid mute toggles racing
// an unrelated pref write during (re)connect intermittently lost
// `voiceMutedOnReconnect`, breaking the muted reconnect-restore.
let prefWriteChain: Promise<unknown> = Promise.resolve();

/** Update specific preference fields.
 *
 * Patches are applied strictly in call order and never interleave, so
 * concurrent updates cannot clobber each other. */
export async function updatePreferences(
  patch: Partial<UserPreferences>,
): Promise<UserPreferences> {
  const apply = async (): Promise<UserPreferences> => {
    const current = await getPreferences();
    const updated = { ...current, ...patch };
    await setPreferences(updated);
    return updated;
  };
  const result = prefWriteChain.then(apply, apply);
  // Keep the chain alive even when a write rejects, so later writes still run.
  prefWriteChain = result.catch(() => {});
  return result;
}

/** Check whether this is the user's first run (setup not completed). */
export async function isFirstRun(): Promise<boolean> {
  const prefs = await getPreferences();
  return !prefs.hasCompletedSetup;
}

/** Get the stored user mode. */
export async function getUserMode(): Promise<UserMode> {
  const prefs = await getPreferences();
  return prefs.userMode;
}

/** Get the stored default username. */
export async function getDefaultUsername(): Promise<string> {
  const prefs = await getPreferences();
  return prefs.defaultUsername;
}

/** Finalise first-run setup by storing mode, default username, default role,
 *  and marking complete. */
export async function completeSetup(
  mode: UserMode,
  defaultUsername: string,
  defaultRole: ClientRole = "operator",
): Promise<void> {
  await updatePreferences({
    userMode: mode,
    defaultUsername,
    defaultRole,
    hasCompletedSetup: true,
  });
}

// -- Audio settings persistence (per-server) -----------------------

/**
 * Audio settings are stored per server, keyed by server address
 * ("host:port"), so each rig/station can have its own device, bitrate,
 * stereo and transmit-mode configuration.  The reserved key
 * {@link AUDIO_DEFAULT_KEY} holds the settings edited while nothing is
 * connected - these are the defaults the backend stamps onto every new
 * session (see `AppState::fresh_session_state`).
 *
 * Legacy single-blob values (stored directly under `AUDIO_KEY` before
 * settings became per-server) are migrated into the default bucket on
 * first read.
 */
type AudioSettingsMap = Record<string, AudioSettings>;

/** Reserved per-server key for the "no server connected" default profile. */
export const AUDIO_DEFAULT_KEY = "__default__";

async function readAudioMap(): Promise<AudioSettingsMap> {
  const store = await getStore();
  const map = (await store.get<AudioSettingsMap>(AUDIO_KEY)) ?? {};
  // Migrate a legacy single-blob value: before per-server settings, the
  // whole AudioSettings object lived directly under AUDIO_KEY.  Detect it
  // by a known scalar field and fold it into the default bucket.
  if (
    map &&
    typeof map === "object" &&
    typeof (map as unknown as AudioSettings).bitrate_bps === "number" &&
    !(AUDIO_DEFAULT_KEY in map)
  ) {
    const legacy = map as unknown as AudioSettings;
    const migrated: AudioSettingsMap = { [AUDIO_DEFAULT_KEY]: legacy };
    await store.set(AUDIO_KEY, migrated);
    return migrated;
  }
  return map;
}

/**
 * Return the persisted audio settings for a server (`host:port`), or for
 * the shared default when `serverKey` is omitted.  Falls back to the
 * default-bucket settings when a server has none of its own yet, so a
 * freshly-added server inherits the user's defaults.  Returns null when
 * nothing is saved at all.
 */
export async function getSavedAudioSettings(
  serverKey: string = AUDIO_DEFAULT_KEY,
): Promise<AudioSettings | null> {
  const map = await readAudioMap();
  return map[serverKey] ?? map[AUDIO_DEFAULT_KEY] ?? null;
}

/**
 * Persist audio settings for a server (`host:port`), or for the shared
 * default when `serverKey` is omitted.
 */
export async function saveAudioSettings(
  settings: AudioSettings,
  serverKey: string = AUDIO_DEFAULT_KEY,
): Promise<void> {
  const store = await getStore();
  const map = await readAudioMap();
  map[serverKey] = settings;
  await store.set(AUDIO_KEY, map);
}

// -- Serial PTT config persistence (per-server) --------------------

/**
 * Rig-PTT config is stored per server (`host:port`) so each station/operator
 * keeps its own port, line, hotkey and timing. It must be persisted on the
 * frontend (not just in backend session memory) so the controller can be
 * started automatically on connect - otherwise the rig only keys after the
 * user manually opens the PTT settings panel. Mirrors the audio-settings map;
 * the {@link AUDIO_DEFAULT_KEY} bucket is the "no server" default.
 */
type PttConfigMap = Record<string, PttConfig>;

/**
 * Return the persisted PTT config for a server (`host:port`), falling back to
 * the default bucket, or null when nothing is saved.
 */
export async function getSavedPttConfig(
  serverKey: string = AUDIO_DEFAULT_KEY,
): Promise<PttConfig | null> {
  const store = await getStore();
  const map = (await store.get<PttConfigMap>(PTT_KEY)) ?? {};
  return map[serverKey] ?? map[AUDIO_DEFAULT_KEY] ?? null;
}

/** Persist the PTT config for a server (`host:port`), or the shared default. */
export async function savePttConfig(
  config: PttConfig,
  serverKey: string = AUDIO_DEFAULT_KEY,
): Promise<void> {
  const store = await getStore();
  const map = (await store.get<PttConfigMap>(PTT_KEY)) ?? {};
  map[serverKey] = config;
  await store.set(PTT_KEY, map);
}

// -- Per-server voice mute persistence -----------------------------

/**
 * The mic mute state is persisted per server (`host:port`) so each rig comes
 * back the way it was left. This is distinct from the legacy global
 * `voiceOnReconnect`/`voiceMutedOnReconnect` prefs, which could not represent
 * a different mute state per server. A value of `undefined` means "never set"
 * and lets the connect logic apply the role/mode default (operator VOX -> live,
 * station -> muted on first run).
 */
type VoiceMutedMap = Record<string, boolean>;
const VOICE_MUTED_KEY = "voiceMutedByServer";

/** Return the saved mic-mute state for a server, or null if never set. */
export async function getSavedVoiceMuted(
  serverKey: string,
): Promise<boolean | null> {
  const store = await getStore();
  const map = (await store.get<VoiceMutedMap>(VOICE_MUTED_KEY)) ?? {};
  return serverKey in map ? map[serverKey] : null;
}

/** Persist the mic-mute state for a server. */
export async function saveVoiceMuted(
  serverKey: string,
  muted: boolean,
): Promise<void> {
  const store = await getStore();
  const map = (await store.get<VoiceMutedMap>(VOICE_MUTED_KEY)) ?? {};
  map[serverKey] = muted;
  await store.set(VOICE_MUTED_KEY, map);
}

// -- Dismissed persistence banners ---------------------------------

const DISMISSED_BANNERS_KEY = "dismissedPersistenceBanners";

/** Return channel IDs whose persistence banner was dismissed. */
export async function getDismissedBanners(): Promise<number[]> {
  const store = await getStore();
  return (await store.get<number[]>(DISMISSED_BANNERS_KEY)) ?? [];
}

/** Mark a channel's persistence banner as dismissed. */
export async function dismissBanner(channelId: number): Promise<void> {
  const store = await getStore();
  const current = (await store.get<number[]>(DISMISSED_BANNERS_KEY)) ?? [];
  if (!current.includes(channelId)) {
    await store.set(DISMISSED_BANNERS_KEY, [...current, channelId]);
  }
}

// -- Silenced channels (per-server, local-only) --------------------

/**
 * Silenced channels are keyed by server address ("host:port") so each
 * server has its own independent blacklist.
 */
const SILENCED_CHANNELS_KEY = "silencedChannels";

type SilencedMap = Record<string, number[]>;

/** Return the channel IDs silenced for a given server. */
export async function getSilencedChannels(
  serverKey: string,
): Promise<number[]> {
  const store = await getStore();
  const map = (await store.get<SilencedMap>(SILENCED_CHANNELS_KEY)) ?? {};
  return map[serverKey] ?? [];
}

/** Toggle the silenced state for a single channel on a server. */
export async function setSilencedChannel(
  serverKey: string,
  channelId: number,
  silenced: boolean,
): Promise<number[]> {
  const store = await getStore();
  const map = (await store.get<SilencedMap>(SILENCED_CHANNELS_KEY)) ?? {};
  const current = map[serverKey] ?? [];
  let updated: number[];
  if (silenced && !current.includes(channelId)) {
    updated = [...current, channelId];
  } else if (!silenced) {
    updated = current.filter((id) => id !== channelId);
  } else {
    return current;
  }
  map[serverKey] = updated;
  await store.set(SILENCED_CHANNELS_KEY, map);
  return updated;
}

// -- Per-user volume overrides (keyed by cert hash) ----------------

const USER_VOLUMES_KEY = "userVolumes";

type VolumeMap = Record<string, number>;

/** Return all persisted per-user volume overrides (hash -> 0-200). */
export async function getUserVolumes(): Promise<VolumeMap> {
  const store = await getStore();
  return (await store.get<VolumeMap>(USER_VOLUMES_KEY)) ?? {};
}

/** Persist a single user's volume override (or remove if 100). */
export async function saveUserVolume(
  hash: string,
  volume: number,
): Promise<void> {
  const store = await getStore();
  const map = (await store.get<VolumeMap>(USER_VOLUMES_KEY)) ?? {};
  if (volume === 100) {
    delete map[hash];
  } else {
    map[hash] = volume;
  }
  await store.set(USER_VOLUMES_KEY, map);
}

// -- Push-notification muted channels (per-server, local-only) -----

/**
 * Channels whose push notifications have been disabled.
 * Keyed by server address ("host:port"), same as silenced channels.
 * The muted list is also sent to the server via `fancy-push-update`
 * so the server can skip sending pushes for those channels.
 */
const MUTED_PUSH_CHANNELS_KEY = "mutedPushChannels";

type MutedPushMap = Record<string, number[]>;

/** Return channel IDs with push notifications disabled for a server. */
export async function getMutedPushChannels(
  serverKey: string,
): Promise<number[]> {
  const store = await getStore();
  const map =
    (await store.get<MutedPushMap>(MUTED_PUSH_CHANNELS_KEY)) ?? {};
  return map[serverKey] ?? [];
}

/** Toggle push-notification mute for a single channel on a server. */
export async function setMutedPushChannel(
  serverKey: string,
  channelId: number,
  muted: boolean,
): Promise<number[]> {
  const store = await getStore();
  const map =
    (await store.get<MutedPushMap>(MUTED_PUSH_CHANNELS_KEY)) ?? {};
  const current = map[serverKey] ?? [];
  let updated: number[];
  if (muted && !current.includes(channelId)) {
    updated = [...current, channelId];
  } else if (!muted) {
    updated = current.filter((id) => id !== channelId);
  } else {
    return current;
  }
  map[serverKey] = updated;
  await store.set(MUTED_PUSH_CHANNELS_KEY, map);
  return updated;
}

// -- Welcome-message "shown once" tracking (per-server) ------------

/**
 * Server keys ("host:port") whose welcome message has already been shown
 * under the "Show once" preference, so it isn't shown again on reconnect.
 */
const WELCOME_SHOWN_KEY = "welcomeShownServers";

/** Whether the welcome message has already been shown for a server. */
export async function hasShownWelcome(serverKey: string): Promise<boolean> {
  const store = await getStore();
  const seen = (await store.get<string[]>(WELCOME_SHOWN_KEY)) ?? [];
  return seen.includes(serverKey);
}

/** Record that a server's welcome message has been shown. */
export async function markWelcomeShown(serverKey: string): Promise<void> {
  const store = await getStore();
  const seen = (await store.get<string[]>(WELCOME_SHOWN_KEY)) ?? [];
  if (!seen.includes(serverKey)) {
    await store.set(WELCOME_SHOWN_KEY, [...seen, serverKey]);
  }
}

// -- Notification sound settings -----------------------------------

const NOTIFICATION_SOUNDS_KEY = "notificationSounds";

/** Return persisted notification sound settings, or null if not configured. */
export async function getNotificationSounds(): Promise<NotificationSoundSettings | null> {
  const store = await getStore();
  return (await store.get<NotificationSoundSettings>(NOTIFICATION_SOUNDS_KEY)) ?? null;
}

/** Persist notification sound settings. */
export async function saveNotificationSounds(
  settings: NotificationSoundSettings,
): Promise<void> {
  const store = await getStore();
  await store.set(NOTIFICATION_SOUNDS_KEY, settings);
  window.dispatchEvent(
    new CustomEvent("notification-sounds-changed", { detail: settings }),
  );
}

// -- Registered (offline) members cache (per-server) ---------------

/**
 * Cache of the registered-user list returned by `request_user_list`,
 * keyed by server address ("host:port").  Lets the members tab render
 * the offline list immediately on open while a fresh request is in
 * flight in the background.
 */
const REGISTERED_USERS_KEY = "registeredUsersCache";

interface RegisteredUsersCacheEntry {
  readonly users: readonly RegisteredUser[];
  readonly cachedAt: number;
}

type RegisteredUsersCacheMap = Record<string, RegisteredUsersCacheEntry>;

/** Return the cached registered users for a server, or null if none. */
export async function getCachedRegisteredUsers(
  serverKey: string,
): Promise<RegisteredUsersCacheEntry | null> {
  const store = await getStore();
  const map =
    (await store.get<RegisteredUsersCacheMap>(REGISTERED_USERS_KEY)) ?? {};
  return map[serverKey] ?? null;
}

/** Persist the registered-user list for a server.
 *
 * Only lightweight identity fields are persisted. Inline comments are
 * dropped (only the `comment_hash` marker is kept); avatars never reach
 * this cache because the backend now ships `texture_size` (a marker) rather
 * than the bytes. This keeps the store tiny - it previously grew to ~129 MB
 * on disk / ~250 MB resident when avatar bytes were persisted as `number[]`.
 * Avatars and long comments are fetched on demand and refreshed by the
 * background request. */
export async function saveCachedRegisteredUsers(
  serverKey: string,
  users: readonly RegisteredUser[],
): Promise<void> {
  const store = await getStore();
  const map =
    (await store.get<RegisteredUsersCacheMap>(REGISTERED_USERS_KEY)) ?? {};
  const lean: RegisteredUser[] = users.map((u) => ({
    user_id: u.user_id,
    name: u.name,
    last_seen: u.last_seen,
    last_channel: u.last_channel,
    // Small markers only: avatar presence and "a comment exists".
    texture_size: u.texture_size ?? null,
    comment_hash: u.comment_hash ? u.comment_hash : null,
  }));
  map[serverKey] = { users: lean, cachedAt: Date.now() };
  await store.set(REGISTERED_USERS_KEY, map);
}
