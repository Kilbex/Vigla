// S3 — pure reducer over the inbox state slice. Mirrors the shape
// of `applyMissionEvent` (pure, side-effect-free) so the ingest
// layer can compose them deterministically.

import type { InboxAction, InboxState } from "./types";

/**
 * Apply one `InboxAction` to the state slice and return a fresh
 * slice. Never mutates the input.
 *
 * Cards are kept sorted by `seq` ascending so the UI renders the
 * oldest at the top of the list (the user reads top-down). Upsert
 * replaces by `id` — the same event sequence number can produce
 * the same card key, in which case "latest write wins". Resolve
 * marks a card resolved (it stays in the list, just dimmed).
 */
export function applyInboxAction(
  state: InboxState,
  action: InboxAction,
): InboxState {
  switch (action.type) {
    case "upsert": {
      const without = state.cards.filter((c) => c.id !== action.card.id);
      const merged = [...without, action.card];
      merged.sort((a, b) => a.seq - b.seq);
      return { cards: merged };
    }
    case "resolve": {
      // No-op if the id is unknown.
      const idx = state.cards.findIndex((c) => c.id === action.id);
      if (idx === -1) return state;
      const next = state.cards.map((c, i) =>
        i === idx ? { ...c, resolved: true } : c,
      );
      return { cards: next };
    }
    case "clear_for_mission": {
      const filtered = state.cards.filter(
        (c) => c.missionId !== action.missionId,
      );
      if (filtered.length === state.cards.length) return state;
      return { cards: filtered };
    }
  }
}

/** Identity selector — returns the state slice for components that
 *  want the full inbox. */
export const selectInbox = (s: InboxState): InboxState => s;
