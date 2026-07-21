import { useEffect, type RefObject } from "react";

const FOCUSABLE_SELECTOR =
  'a[href],button:not([disabled]),textarea:not([disabled]),' +
  'input:not([disabled]),select:not([disabled]),[tabindex]:not([tabindex="-1"])';

/**
 * Accessible dialog focus management (FE-A2).
 *
 * When `active` becomes true: move focus into `containerRef` (the first
 * focusable child, falling back to the container itself), and on close restore
 * focus to whatever was focused before the dialog opened.
 *
 * When `trap` is true (modal dialogs — `aria-modal="true"`), Tab/Shift-Tab is
 * cycled within the container and focus that escapes is pulled back. For
 * non-modal surfaces that intentionally stay interactive (e.g. a no-scrim
 * drawer), pass `trap = false` to get focus-in + restore without trapping Tab.
 */
export function useDialogFocus(
  active: boolean,
  containerRef: RefObject<HTMLElement | null>,
  trap = true,
): void {
  useEffect(() => {
    if (!active) return;
    const container = containerRef.current;
    if (!container) return;

    const previouslyFocused =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;

    const focusable = (): HTMLElement[] =>
      Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
        // Skip hidden elements (display:none → no offsetParent).
        (el) => el.offsetParent !== null || el === document.activeElement,
      );

    // Move focus into the dialog on open.
    (focusable()[0] ?? container).focus();

    let onKeyDown: ((e: KeyboardEvent) => void) | undefined;
    if (trap) {
      onKeyDown = (e: KeyboardEvent) => {
        if (e.key !== "Tab") return;
        const items = focusable();
        if (items.length === 0) {
          e.preventDefault();
          return;
        }
        const firstEl = items[0];
        const lastEl = items[items.length - 1];
        const activeEl = document.activeElement;
        if (!container.contains(activeEl)) {
          e.preventDefault();
          firstEl.focus();
        } else if (e.shiftKey && activeEl === firstEl) {
          e.preventDefault();
          lastEl.focus();
        } else if (!e.shiftKey && activeEl === lastEl) {
          e.preventDefault();
          firstEl.focus();
        }
      };
      container.addEventListener("keydown", onKeyDown);
    }

    return () => {
      if (onKeyDown) container.removeEventListener("keydown", onKeyDown);
      // Restore focus to the trigger so keyboard users land where they were.
      previouslyFocused?.focus?.();
    };
  }, [active, containerRef, trap]);
}
