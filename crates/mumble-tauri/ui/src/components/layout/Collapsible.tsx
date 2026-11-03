import { useCallback, useState } from "react";
import { ChevronDownIcon } from "../../icons";
import styles from "./Collapsible.module.css";

/**
 * A titled collapsible section. Tap the title bar to expand/collapse. The
 * open/closed state is persisted to localStorage under `storageKey` so it
 * survives app restarts (used by the mobile radio controls, where the audio
 * sections fold away so the channel/operator list is the primary surface).
 */
interface CollapsibleProps {
  readonly title: string;
  /** Stable key for persisting the open/closed state. */
  readonly storageKey: string;
  /** Open state when nothing is persisted yet. */
  readonly defaultOpen?: boolean;
  readonly children: React.ReactNode;
}

export default function Collapsible({
  title,
  storageKey,
  defaultOpen = false,
  children,
}: CollapsibleProps) {
  const key = `rr.collapsible.${storageKey}`;
  const [open, setOpen] = useState<boolean>(() => {
    const saved = localStorage.getItem(key);
    return saved === null ? defaultOpen : saved === "1";
  });

  const toggle = useCallback(() => {
    setOpen((v) => {
      const next = !v;
      localStorage.setItem(key, next ? "1" : "0");
      return next;
    });
  }, [key]);

  return (
    <section className={styles.section}>
      <button
        type="button"
        className={styles.header}
        onClick={toggle}
        aria-expanded={open}
      >
        <span className={styles.title}>{title}</span>
        <ChevronDownIcon
          width={18}
          height={18}
          className={`${styles.chevron} ${open ? styles.chevronOpen : ""}`}
        />
      </button>
      {open && <div className={styles.body}>{children}</div>}
    </section>
  );
}
