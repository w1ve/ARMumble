import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { SearchIcon, CircleDotIcon } from "../../icons";
import { StationIcon } from "../icons/StationIcon";
import { filterBuiltinServers, builtinKey, type BuiltinServer } from "../../builtinServers";
import type { ClientRole, ServerPingResult } from "../../types";
import styles from "./BuiltinServerList.module.css";

/** Availability of a built-in entry: unknown while first probing, then up/down. */
type Avail = "checking" | "up" | "down";

interface Props {
  /** Connect to the chosen built-in Remote Radio server with the selected role. */
  readonly onConnect: (server: BuiltinServer, role: ClientRole) => void;
  /** Role pre-selected in the Operator/Station toggle (user's default). */
  readonly defaultRole?: ClientRole;
  readonly disabled?: boolean;
}

/**
 * The built-in Remote Radio server picker shown on the sign-in screen.
 *
 * The Operator/Station toggle sits on top, then a fixed "remote.radio.org"
 * directory of hard-coded server cards (see `builtinServers.ts`), each with a
 * square city badge. This is a fixed-server architecture: there is deliberately
 * no add/edit/delete/browse here.
 */
export default function BuiltinServerList({ onConnect, defaultRole, disabled }: Props) {
  const { t } = useTranslation("server");
  const [query, setQuery] = useState("");
  const [role, setRole] = useState<ClientRole>(defaultRole ?? "operator");
  const filtered = useMemo(() => filterBuiltinServers(query), [query]);

  // Per-entry availability (keyed by host:port). A TCP connect to the voice
  // port decides up/down; a down server is greyed out and not connectable.
  const [avail, setAvail] = useState<Record<string, Avail>>({});
  // Throttle: remember the last probe time per key so re-renders don't re-ping.
  const lastPing = useRef<Map<string, number>>(new Map());

  const probeAll = useCallback(() => {
    const THROTTLE_MS = 30_000;
    const now = Date.now();
    for (const s of filterBuiltinServers("")) {
      const key = builtinKey(s);
      const last = lastPing.current.get(key);
      if (last !== undefined && now - last < THROTTLE_MS) continue;
      lastPing.current.set(key, now);
      setAvail((prev) => (prev[key] ? prev : { ...prev, [key]: "checking" }));
      invoke<ServerPingResult>("ping_server", { host: s.host, port: s.port })
        .then((r) => setAvail((prev) => ({ ...prev, [key]: r.online ? "up" : "down" })))
        .catch(() => setAvail((prev) => ({ ...prev, [key]: "down" })));
    }
  }, []);

  // Probe on mount and re-probe every 30 s while the picker is shown.
  useEffect(() => {
    probeAll();
    const id = setInterval(probeAll, 30_000);
    return () => clearInterval(id);
  }, [probeAll]);

  return (
    <div className={styles.wrap}>
      {/* Connect-as role selector (applies to whichever server you tap). */}
      <div className={styles.roleRow} role="radiogroup" aria-label={t("builtin.connectAs")}>
        <span className={styles.roleRowLabel}>{t("builtin.connectAs")}</span>
        <div className={styles.roleToggle}>
          <button
            type="button"
            role="radio"
            aria-checked={role === "operator"}
            className={`${styles.roleOption} ${role === "operator" ? styles.roleOptionActive : ""}`}
            onClick={() => setRole("operator")}
            disabled={disabled}
            title={t("builtin.roleOperatorHint")}
          >
            <CircleDotIcon width={13} height={13} />
            {t("builtin.roleOperator")}
          </button>
          <button
            type="button"
            role="radio"
            aria-checked={role === "station"}
            className={`${styles.roleOption} ${role === "station" ? styles.roleOptionActive : ""}`}
            onClick={() => setRole("station")}
            disabled={disabled}
            title={t("builtin.roleStationHint")}
          >
            <StationIcon width={13} height={13} />
            {t("builtin.roleStation")}
          </button>
        </div>
      </div>

      {/* Fixed built-in server directory — users cannot add/remove these. */}
      <div className={styles.header}>
        <span className={styles.heading}>remote.radio.org</span>
      </div>

      {/* Search box. */}
      <div className={styles.searchWrap}>
        <SearchIcon width={16} height={16} className={styles.searchIcon} />
        <input
          className={styles.search}
          type="search"
          placeholder="Search servers..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          disabled={disabled}
        />
      </div>

      {/* Server cards. Each location has a FANCY (stereo) and a LEGACY card. */}
      <div className={styles.list}>
        {filtered.length === 0 ? (
          <div className={styles.empty}>No matching servers.</div>
        ) : (
          filtered.map((s) => {
            const key = builtinKey(s);
            const status = avail[key] ?? "checking";
            const isUp = status === "up";
            const isDown = status === "down";
            const isFancy = s.flavor === "fancy";
            // Down servers are greyed out and not connectable.
            const cardDisabled = disabled || isDown;
            const dotClass = isUp ? styles.dotUp : styles.dotDown;
            const dotTitle = isUp
              ? t("builtin.statusOnline")
              : isDown
                ? t("builtin.statusOffline")
                : t("builtin.statusChecking");
            return (
              <button
                key={key}
                type="button"
                className={`${styles.card} ${isDown ? styles.cardOffline : ""}`}
                onClick={() => { if (!cardDisabled) onConnect(s, role); }}
                disabled={cardDisabled}
                aria-disabled={cardDisabled}
              >
                <span className={styles.badge}>{s.abbr}</span>
                <span className={styles.cardText}>
                  <span className={styles.cardHost}>{s.host}</span>
                  <span className={styles.cardName}>
                    <span className={dotClass} title={dotTitle} aria-label={dotTitle} />
                    Public {s.name}
                    {isFancy ? (
                      <span className={`${styles.tag} ${isUp ? styles.tagStereo : ""}`}>
                        {t("builtin.tagStereo")}
                      </span>
                    ) : (
                      <span className={styles.tag}>{t("builtin.tagLegacy")}</span>
                    )}
                  </span>
                </span>
              </button>
            );
          })
        )}
      </div>
    </div>
  );
}
