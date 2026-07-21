import { useEffect } from "react";
import { commands } from "./bindings";
import { useSurfaceStore } from "./inbox/router";
import { getShowAllEvents, setShowAllEvents } from "./settings/preferences";
import { selectWorkersNeedingReview, useOpsStore } from "./store";
import type { Vendor } from "./bindings";

interface KeyboardOptions {
  onOpenSettings: () => void;
}

const SCRIPT_KEYS: Record<
  string,
  "claude_happy" | "codex_blocked" | "gemini_happy"
> = {
  "1": "claude_happy",
  "2": "codex_blocked",
  "3": "gemini_happy",
};

const ENABLE_MOCK_SHORTCUTS =
  import.meta.env.DEV || import.meta.env.VITE_VIGLA_E2E === "1";

/// Batch 3 (B3.3) — mission-control reserved letters.
/// Bind these only via this module to avoid collisions; future
/// Settings or panel-local handlers must check before rebinding:
///   R / ⇧R / A / X / O / J / K — Review-queue triage map.
const VENDOR_CAN_RETRY: Record<Vendor, boolean> = {
  claude: true,
  codex: false,
  gemini: false,
  antigravity: false,
  kiro: false,
  copilot: false,
  opencode: false,
  mock: false,
};

/// Step 15 / Batch 3 — global keyboard shortcuts.
///
/// 1 / 2 / 3            spawn each mock script (live mode only)
/// ⌘H or Ctrl+H         toggle history (replay) mode
/// ⌘,                   open settings
///
/// Batch 3 — Review-queue triage map (live mode unless noted):
///   J / K              move queue focus next / prev (clamped); also
///                      moves drawer selection if drawer is open
///   O                  open drawer on focused card (active in replay too)
///   R                  retry focused worker (Claude-only; M3 parity gap)
///   ⇧R                 expand inline continue area on focused card
///   A / X              accept / reject focused card
///   Esc                cascade close — text input → drawer → review
///                      focus → replay. Steps 1 (text input) and 2
///                      (drawer) are handled locally inside their
///                      components; this hook handles steps 3 + 4.
///
/// Letters are inert when focus is inside an `<input>` / `<textarea>` /
/// content-editable element. Note: a power user mashing `A` inside the
/// inline continue textarea will type `a` — intentional (§10.4).
export function useGlobalKeyboard({ onOpenSettings }: KeyboardOptions) {
  const enterReplay = useOpsStore((s) => s.enterReplay);
  const exitReplay = useOpsStore((s) => s.exitReplay);
  const isReplay = useOpsStore((s) => s.replay.mode === "replay");

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const tag = target?.tagName?.toLowerCase();
      const inTextInput =
        tag === "input" ||
        tag === "textarea" ||
        tag === "select" ||
        target?.isContentEditable === true;

      const cmdOrCtrl = e.metaKey || e.ctrlKey;

      // ── Cmd/Ctrl combos work regardless of text-input focus. ──
      if (cmdOrCtrl && e.key === ",") {
        e.preventDefault();
        onOpenSettings();
        return;
      }
      if (cmdOrCtrl && (e.key === "h" || e.key === "H")) {
        e.preventDefault();
        if (isReplay) exitReplay();
        else enterReplay([]);
        return;
      }

      // S10 — Surface router (⌘+digit). ⌘2 auto-enables
      // showAllEvents so CommsFeed actually renders. Bare 1/2/3
      // mock-spawn bindings below remain unchanged.
      if (cmdOrCtrl && !e.altKey && !e.shiftKey) {
        if (e.key === "1") {
          e.preventDefault();
          useSurfaceStore.getState().setSurface("inbox");
          return;
        }
        if (e.key === "2") {
          e.preventDefault();
          if (!getShowAllEvents()) setShowAllEvents(true);
          useSurfaceStore.getState().setSurface("ops_room");
          return;
        }
        if (e.key === "3") {
          e.preventDefault();
          useSurfaceStore.getState().setSurface("history");
          return;
        }
      }

      // ── Esc cascade (§4.3) ──
      // Steps 1 (text input) and 2 (drawer) handle Esc locally; the
      // global handler covers steps 3 (clear review focus) and 4
      // (exit replay). When a text input has focus or the drawer is
      // open we bail out so their handlers win.
      if (e.key === "Escape") {
        if (inTextInput) return;
        const st = useOpsStore.getState();
        if (st.selectedWorkerId !== null) return; // drawer handles it
        if (st.reviewFocusedWorkerId !== null) {
          e.preventDefault();
          st.setReviewFocus(null);
          return;
        }
        if (st.replay.mode === "replay") {
          e.preventDefault();
          st.exitReplay();
          return;
        }
        return;
      }

      if (inTextInput) return;

      // Number keys spawn mocks only in dev / E2E harnesses.
      if (
        ENABLE_MOCK_SHORTCUTS &&
        !cmdOrCtrl &&
        !e.altKey &&
        !e.shiftKey &&
        SCRIPT_KEYS[e.key]
      ) {
        const script = SCRIPT_KEYS[e.key];
        e.preventDefault();
        if (!isReplay) {
          commands.startMockWorker(script, 1.0).catch(() => {
            // Surface via the comms feed status; ignore here.
          });
        }
        return;
      }

      // ── Batch 3 — Review-queue triage map ──
      if (cmdOrCtrl || e.altKey) return;

      const st = useOpsStore.getState();
      const queue = selectWorkersNeedingReview(st);
      const focusedId = st.reviewFocusedWorkerId;
      const focusedIdx = focusedId ? queue.indexOf(focusedId) : -1;

      const moveFocus = (delta: 1 | -1) => {
        if (queue.length === 0) return;
        let nextIdx: number;
        if (focusedIdx === -1) {
          // No focus yet — J focuses the first card, K focuses the last.
          nextIdx = delta === 1 ? 0 : queue.length - 1;
        } else {
          nextIdx = Math.min(
            Math.max(focusedIdx + delta, 0),
            queue.length - 1,
          );
        }
        const nextId = queue[nextIdx];
        st.setReviewFocus(nextId);
        // Drawer reciprocity (§4.4): if the drawer is open, follow.
        if (st.selectedWorkerId !== null) {
          st.selectWorker(nextId);
        }
      };

      switch (e.key) {
        case "j":
        case "J":
          e.preventDefault();
          moveFocus(1);
          return;
        case "k":
        case "K":
          e.preventDefault();
          moveFocus(-1);
          return;
        case "o":
        case "O":
          if (e.shiftKey) return; // ⇧O unused
          if (focusedId) {
            e.preventDefault();
            st.selectWorker(focusedId);
          }
          return;
        case "r":
        case "R":
          // R = retry (no shift) ; ⇧R = continue. Both disabled in replay.
          if (isReplay) return;
          if (!focusedId) return;
          {
            const w = st.workers[focusedId];
            if (!w || !VENDOR_CAN_RETRY[w.vendor]) {
              e.preventDefault();
              return;
            }
            if (e.shiftKey) {
              e.preventDefault();
              window.dispatchEvent(
                new CustomEvent("vigla:continue-expand", {
                  detail: { workerId: focusedId },
                }),
              );
            } else {
              e.preventDefault();
              commands.retryWorker(focusedId).catch(() => {
                /* surfaced in the drawer's retry flow */
              });
            }
          }
          return;
        case "a":
        case "A":
          if (e.shiftKey) return;
          if (isReplay) return;
          if (!focusedId) return;
          e.preventDefault();
          st.setReviewStatus(focusedId, "accepted");
          return;
        case "x":
        case "X":
          if (e.shiftKey) return;
          if (isReplay) return;
          if (!focusedId) return;
          e.preventDefault();
          st.setReviewStatus(focusedId, "rejected");
          return;
        default:
          return;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onOpenSettings, enterReplay, exitReplay, isReplay]);
}
