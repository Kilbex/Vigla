import { useCallback, useState } from "react";
import MemoryDrawer from "./MemoryDrawer";

/**
 * Trigger + container for the Tier-2E Memory drawer. Lives next to
 * the Pin Note button in the command panel; clicking it slides the
 * drawer in from the right. The drawer mounts only while open, so
 * polling effects are bounded by user attention.
 *
 * The button mirrors the visual language of `command-panel-history`
 * (pill, mono font, subtle hover) — see `.memory-drawer-button` in
 * `index.css`.
 */
export default function MemoryDrawerButton() {
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  return (
    <>
      <button
        type="button"
        className={
          "memory-drawer-button" + (open ? " memory-drawer-button--on" : "")
        }
        onClick={() => setOpen((o) => !o)}
        aria-haspopup="dialog"
        aria-expanded={open}
        title="Show memory attached to this mission"
      >
        Memory
      </button>
      {open ? <MemoryDrawer onClose={close} /> : null}
    </>
  );
}
