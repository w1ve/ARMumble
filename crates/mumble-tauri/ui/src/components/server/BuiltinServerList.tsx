import { useMemo, useState } from "react";
import { PlusIcon, SearchIcon } from "../../icons";
import {
  BUILTIN_SERVERS,
  filterBuiltinServers,
  type BuiltinServer,
} from "../../builtinServers";
import styles from "./BuiltinServerList.module.css";

interface Props {
  /** Connect to the chosen built-in Remote Radio server. */
  readonly onConnect: (server: BuiltinServer) => void;
  /** Open the add-your-own-server wizard. */
  readonly onAddNew: () => void;
  readonly disabled?: boolean;
}

/**
 * The built-in Remote Radio server picker shown on the sign-in screen.
 *
 * Mirrors the sign-in mockup: a "SAVED SERVERS / + Add Server" header, a
 * quick-select dropdown of the whole network, a search box, and a scrollable
 * list of server cards each with a square city badge. The list is hard-coded
 * (see `builtinServers.ts`); there is deliberately no edit/delete/browse.
 */
export default function BuiltinServerList({ onConnect, onAddNew, disabled }: Props) {
  const [query, setQuery] = useState("");
  const filtered = useMemo(() => filterBuiltinServers(query), [query]);

  return (
    <div className={styles.wrap}>
      <div className={styles.header}>
        <span className={styles.heading}>Remote Radio Servers</span>
        <button
          type="button"
          className={styles.addLink}
          onClick={onAddNew}
          disabled={disabled}
        >
          <PlusIcon width={16} height={16} />
          Add Server
        </button>
      </div>

      {/* Quick-select dropdown of the whole network. */}
      <select
        className={styles.dropdown}
        disabled={disabled}
        defaultValue=""
        onChange={(e) => {
          const server = BUILTIN_SERVERS.find((s) => s.host === e.target.value);
          if (server) onConnect(server);
          e.currentTarget.selectedIndex = 0; // reset so re-picking the same host fires again
        }}
      >
        <option value="" disabled>
          Choose a server…
        </option>
        {BUILTIN_SERVERS.map((s) => (
          <option key={s.host} value={s.host}>
            {s.name} ({s.host})
          </option>
        ))}
      </select>

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

      {/* Server cards. */}
      <div className={styles.list}>
        {filtered.length === 0 ? (
          <div className={styles.empty}>No matching servers.</div>
        ) : (
          filtered.map((s) => (
            <button
              key={s.host}
              type="button"
              className={styles.card}
              onClick={() => onConnect(s)}
              disabled={disabled}
            >
              <span className={styles.badge}>{s.abbr}</span>
              <span className={styles.cardText}>
                <span className={styles.cardHost}>{s.host}</span>
                <span className={styles.cardName}>{s.name}</span>
              </span>
            </button>
          ))
        )}
      </div>
    </div>
  );
}
