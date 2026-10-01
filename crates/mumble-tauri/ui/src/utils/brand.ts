/**
 * Mode-aware product branding.
 *
 * All three apps (RemoteRadioClient, RemoteRadioStation, RemoteRadioMumble)
 * share one UI bundle, so the displayed product name is resolved at runtime
 * from the backend `app_mode` command rather than hard-coded. The result is
 * cached after the first call. Until the backend responds (e.g. the very first
 * paint of the loading splash) the neutral "RemoteRadio" brand is used.
 *
 * This module only resolves the mode and maps it to a translation *key*;
 * callers translate it with their own `t()` so the brand uses the same i18n
 * instance as the surrounding UI (and the test harness).
 */
import { invoke } from "@tauri-apps/api/core";

export type AppMode = "client" | "station" | "mumble";

/** Brand translation keys in the `common` namespace. */
export type BrandKey = "brand" | "brandClient" | "brandStation" | "brandMumble";

let cachedMode: AppMode | null = null;
let inflight: Promise<AppMode> | null = null;

/** Resolve the backend app mode (cached). Falls back to "client" off-Tauri. */
export async function getAppMode(): Promise<AppMode> {
  if (cachedMode) return cachedMode;
  if (!inflight) {
    inflight = invoke<string>("app_mode")
      .then((m) => {
        cachedMode = (m as AppMode) ?? "client";
        return cachedMode;
      })
      .catch(() => {
        // Not running inside Tauri (e.g. vitest / vite preview).
        cachedMode = "client";
        return cachedMode;
      });
  }
  return inflight;
}

/**
 * Map an app mode to its brand translation key (within the `common`
 * namespace). `null` yields the neutral "brand" key used before the backend
 * mode has resolved.
 */
export function brandKeyForMode(mode: AppMode | null): BrandKey {
  switch (mode) {
    case "station":
      return "brandStation";
    case "mumble":
      return "brandMumble";
    case "client":
      return "brandClient";
    default:
      return "brand";
  }
}
