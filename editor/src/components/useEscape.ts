import { useEffect } from "react";

/** Calls `onEscape` when Esc is pressed (dialogs close this way). */
export function useEscape(onEscape: () => void) {
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopImmediatePropagation();
      onEscape();
    };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
  }, [onEscape]);
}
