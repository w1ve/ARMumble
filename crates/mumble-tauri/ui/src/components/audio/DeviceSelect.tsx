import { useEffect, useId, useMemo, useRef, useState } from "react";
import { ChevronDownIcon } from "../../icons";
import type { AudioDevice } from "../../types";
import styles from "./DeviceSelect.module.css";

/**
 * Sentinel value for the explicit "(No device)" selection. Stored as the
 * device string so it round-trips through the same `selected_device` /
 * `selected_output_device` field. The backend treats this sentinel as
 * "tear the pipeline down / open nothing".
 */
export const NO_DEVICE = "\u0000none";

/** Sentinel-aware value: `null` => System default, NO_DEVICE => none. */
export type DeviceValue = string | null;

interface DeviceSelectProps {
  readonly devices: readonly AudioDevice[];
  /** `null` = System default, NO_DEVICE = (No device), else a device name. */
  readonly value: DeviceValue;
  readonly onChange: (value: DeviceValue) => void;
  /** Label for the System default option. */
  readonly systemDefaultLabel: string;
  /** Label for the (No device) option. */
  readonly noDeviceLabel: string;
  /** Suffix shown after the OS default device, e.g. "(default)". */
  readonly defaultSuffix: string;
  readonly ariaLabel?: string;
  /** When true, the picker is non-interactive (e.g. overridden by test tone). */
  readonly disabled?: boolean;
}

interface Row {
  readonly value: DeviceValue;
  readonly label: string;
}

/**
 * A custom, fixed-height, scrollable device picker. Replaces the native
 * `<select>` (whose popup height is OS/WebView-controlled and cannot be
 * capped) so long device lists scroll within a bounded dropdown. Includes a
 * "System default" row and an explicit "(No device)" row.
 */
export function DeviceSelect({
  devices,
  value,
  onChange,
  systemDefaultLabel,
  noDeviceLabel,
  defaultSuffix,
  ariaLabel,
  disabled = false,
}: Readonly<DeviceSelectProps>) {
  const [open, setOpen] = useState(false);
  const [activeIdx, setActiveIdx] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const listboxId = useId();

  const rows = useMemo<Row[]>(() => {
    const base: Row[] = [
      { value: null, label: systemDefaultLabel },
      { value: NO_DEVICE, label: noDeviceLabel },
    ];
    for (const d of devices) {
      base.push({ value: d.name, label: d.is_default ? `${d.name} ${defaultSuffix}` : d.name });
    }
    return base;
  }, [devices, systemDefaultLabel, noDeviceLabel, defaultSuffix]);

  const selectedRow = useMemo(
    () => rows.find((r) => r.value === value) ?? rows[0],
    [rows, value],
  );

  // Close on outside click.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);

  // When opening, sync the active row to the current selection and scroll it
  // into view.
  useEffect(() => {
    if (!open) return;
    const idx = rows.findIndex((r) => r.value === value);
    const start = idx >= 0 ? idx : 0;
    setActiveIdx(start);
    requestAnimationFrame(() => {
      const el = listRef.current?.children[start] as HTMLElement | undefined;
      el?.scrollIntoView({ block: "nearest" });
    });
  }, [open, rows, value]);

  const commit = (idx: number) => {
    const row = rows[idx];
    if (row) onChange(row.value);
    setOpen(false);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!open) {
      if (e.key === "ArrowDown" || e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        setOpen(true);
      }
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      setOpen(false);
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setActiveIdx((i) => {
        const next = Math.min(rows.length - 1, i + 1);
        (listRef.current?.children[next] as HTMLElement | undefined)?.scrollIntoView({ block: "nearest" });
        return next;
      });
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIdx((i) => {
        const next = Math.max(0, i - 1);
        (listRef.current?.children[next] as HTMLElement | undefined)?.scrollIntoView({ block: "nearest" });
        return next;
      });
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      commit(activeIdx);
    }
  };

  return (
    <div className={styles.root} ref={rootRef}>
      <button
        type="button"
        className={styles.trigger}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={ariaLabel}
        disabled={disabled}
        onClick={() => setOpen((v) => !v)}
        onKeyDown={onKeyDown}
      >
        <span className={styles.triggerLabel}>{selectedRow.label}</span>
        <ChevronDownIcon width={16} height={16} className={styles.chevron} />
      </button>
      {open && (
        <ul
          ref={listRef}
          className={styles.list}
          role="listbox"
          id={listboxId}
          aria-label={ariaLabel}
          tabIndex={-1}
        >
          {rows.map((row, idx) => {
            const isSelected = row.value === value;
            const isActive = idx === activeIdx;
            const isSentinel = row.value === null || row.value === NO_DEVICE;
            return (
              <li
                key={row.value ?? "__default__"}
                role="option"
                aria-selected={isSelected}
                className={[
                  styles.option,
                  isActive ? styles.optionActive : "",
                  isSelected ? styles.optionSelected : "",
                  isSentinel ? styles.optionSentinel : "",
                ].filter(Boolean).join(" ")}
                onMouseEnter={() => setActiveIdx(idx)}
                onClick={() => commit(idx)}
              >
                {row.label}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
