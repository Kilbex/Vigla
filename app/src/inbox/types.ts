// S3 — inbox surface types. Mirrors the Rust orchestrator's
// `EventVisibility` / `InboxKind` / `Severity` shapes, plus the
// frontend-only `InboxCard` representation used by the inbox state
// reducer and the visual components.

import type { AuthorityBound, EventVisibility, InboxKind, Severity } from "../bindings";

export type { EventVisibility, InboxKind, Severity };

/**
 * Frontend representation of one inbox card. Built by the ingest
 * reducer from a `MissionEvent` whose visibility verdict was
 * `Inbox`. Persists in the missions store until the card is
 * resolved (Escalation cards) or replaced by a fresh one of the
 * same kind for the same key.
 */
export interface InboxCard {
  /** Stable id — `mission_id + ":" + seq + ":" + worker_id?` or
   *  `mission_id + ":" + kind` for mission-level cards. The same
   *  card key dedupes when an event of the same kind fires twice
   *  for the same worker — the latest wins. */
  id: string;
  /** Which mission this card belongs to. */
  missionId: string;
  /** Source event sequence; for tie-breaking when ordering cards. */
  seq: number;
  /** RFC3339 timestamp from the source event. */
  surfacedAt: string;
  /** Inbox kind from the Rust verdict. */
  kind: InboxKind;
  /** Severity from the Rust verdict. */
  severity: Severity;
  /** Short human-readable headline (one line, < 80 chars). */
  title: string;
  /** Longer explanation — may include multi-line evidence. */
  detail: string | null;
  /** For Escalation cards: which AuthorityBound tripped. `null`
   *  for Completion and SideEffect cards. */
  bound: AuthorityBound | null;
  /** True once the user has resolved the card (Escalation only).
   *  Completion cards never resolve — they stay as history. */
  resolved: boolean;
}

/**
 * Discriminated union of actions that drive the inbox reducer.
 * Each action carries enough context for a pure state transition.
 */
export type InboxAction =
  | { type: "upsert"; card: InboxCard }
  | { type: "resolve"; id: string }
  | { type: "clear_for_mission"; missionId: string };

/**
 * Inbox state slice. Lives inside `ActiveMission` so it stays scoped per
 * mission; audit history is queried through the separate History surface.
 */
export interface InboxState {
  cards: InboxCard[];
}

export const emptyInboxState = (): InboxState => ({ cards: [] });

/**
 * True if the card requires user action (ActionRequired severity
 * AND not yet resolved). Used by the macOS-banner gate and the
 * inbox-overview "needs you" counter.
 */
export function requiresAction(card: InboxCard): boolean {
  return card.severity === "action_required" && !card.resolved;
}

/**
 * Glyph for a card's severity. Pure presentation helper used by
 * `InboxCard.tsx`.
 */
export function severityGlyph(severity: Severity): string {
  switch (severity) {
    case "info":
      return "✓";
    case "warning":
      return "!";
    case "action_required":
      return "⚠";
  }
}

/**
 * Human label for an inbox kind. Pure presentation helper.
 */
export function kindLabel(kind: InboxKind): string {
  switch (kind) {
    case "escalation":
      return "Escalation";
    case "completion":
      return "Completion";
    case "side_effect":
      return "Side effect";
  }
}

/**
 * Human label for an AuthorityBound. Pure presentation helper.
 * Mirrors the snake_case wire form from the Rust enum.
 */
export function boundLabel(bound: AuthorityBound | null): string | null {
  if (!bound) return null;
  switch (bound) {
    case "scope":
      return "Scope";
    case "reversibility":
      return "Reversibility";
    case "risk":
      return "Risk";
    case "quality":
      return "Quality";
  }
}
