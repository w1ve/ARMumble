import { useEffect, useRef } from "react";
import type { PointerEvent } from "react";

/** Pointer handlers for a press-and-hold button; always releases, even if focus is lost. */
export function useHold(onDown: () => void, onUp: () => void) {
  const held = useRef(false);

  const release = () => {
    if (held.current) {
      held.current = false;
      onUp();
    }
  };

  useEffect(() => {
    window.addEventListener("blur", release);
    return () => {
      window.removeEventListener("blur", release);
      release();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return {
    onPointerDown: (e: PointerEvent<HTMLElement>) => {
      e.currentTarget.setPointerCapture(e.pointerId);
      if (!held.current) {
        held.current = true;
        onDown();
      }
    },
    onPointerUp: release,
    onPointerCancel: release,
    onLostPointerCapture: release,
    onContextMenu: (e: { preventDefault: () => void }) => e.preventDefault(),
  };
}