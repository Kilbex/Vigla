// S3 — thin client around the Rust `mission_event_visibility`
// Tauri command. Cached by event type so the same shape only
// crosses the IPC boundary once per app session.
//
// Falls back to a small in-process table on IPC failure so the
// UI remains usable when the orchestrator is degraded. The
// fallback is intentionally conservative — unknown events return
// `Internal` (no UI surfacing) rather than wrongly surfacing
// arbitrary events.

import type { MissionEvent, EventVisibility, MissionEventKindDto } from "../bindings";
import { commands } from "../bindings";

const cache = new Map<string, EventVisibility>();

/**
 * Look up the visibility verdict for a single mission event. The
 * `MissionEvent` envelope is unwrapped to its `MissionEventKind`
 * payload before crossing the IPC boundary.
 *
 * Cache key is the event type for variants whose visibility is a
 * pure function of the variant tag. For `arbiter.decided` the
 * verdict depends on the `bound` payload field (Escalate → inbox
 * with action_required; Extend → internal; etc.), so the cache
 * key incorporates `bound` (or `none`) to keep the cache correct.
 */
export async function fetchVisibility(
  event: MissionEvent,
): Promise<EventVisibility> {
  const key = cacheKey(event);
  const hit = cache.get(key);
  if (hit) return hit;

  try {
    const kind = unwrapToKind(event) as MissionEventKindDto;
    const verdict = await commands.missionEventVisibility(kind);
    cache.set(key, verdict);
    return verdict;
  } catch {
    // IPC failure — fall back to a conservative table that
    // matches the Rust mapping's intent. Internal events stay
    // silent; the user can recover by reloading the window.
    const fallback = fallbackVerdict(event.type);
    cache.set(key, fallback);
    return fallback;
  }
}

/**
 * Build a stable cache key for an event. Pure event-type for most
 * variants; for `arbiter.decided` the verdict varies along two
 * axes — the `bound` (only present for Escalate) and the decision
 * kind (Accept/Extend/Scrub/Escalate). Both feed the key so each
 * distinct Rust verdict is cached independently.
 *
 * For `supervisor.recovery_decided` the verdict is `Internal` today
 * but the action kind (retry/pause/escalate/request_supervisor) is
 * a forward-compat discriminator: if a future visibility rule
 * surfaces Escalate actions as Inbox while Retry stays Internal,
 * a single shared cache slot would alias the two. The cache key
 * incorporates the parsed action kind so each variant caches
 * independently. Mirrors the `arbiter.decided` discriminator
 * pattern (see commit 9809437).
 */
function cacheKey(event: MissionEvent): string {
  if (event.type === "arbiter.decided") {
    const payload = (event as { payload?: { bound?: unknown; decision_json?: string } }).payload;
    const bound = payload?.bound ?? "none";
    const decisionKind = parseDecisionKind(payload?.decision_json);
    return `arbiter.decided:${bound}:${decisionKind}`;
  }
  if (event.type === "supervisor.recovery_decided") {
    const payload = (event as { payload?: { action_json?: string } }).payload;
    const actionKind = parseRecoveryActionKind(payload?.action_json);
    return `supervisor.recovery_decided:${actionKind}`;
  }
  return event.type;
}

/**
 * Extract the discriminator tag from a serialized ArbiterDecision.
 * Rust shape: `{"kind":"accept"|"extend"|"scrub"|"escalate", ...}`.
 * Returns `"unknown"` when the payload is missing or malformed so
 * cache-key collisions are surfaced as a single bucket per failure
 * mode rather than silently aliasing distinct verdicts.
 */
function parseDecisionKind(decisionJson: string | undefined): string {
  if (!decisionJson) return "unknown";
  try {
    const parsed = JSON.parse(decisionJson) as { kind?: unknown };
    return typeof parsed.kind === "string" ? parsed.kind : "unknown";
  } catch {
    return "unknown";
  }
}

/**
 * Extract the discriminator tag from a serialized RecoveryAction.
 * The Rust enum is externally tagged (`#[serde(rename_all =
 * "snake_case")]` with no `tag`), so the wire shape is a single
 * top-level key: `{"retry":{...}}`, `{"pause":{...}}`,
 * `{"escalate":{...}}`, `{"request_supervisor":{...}}`. Returns
 * `"unknown"` when the payload is missing, malformed, or doesn't
 * match the expected single-key object — a safe default that keeps
 * malformed events bucketed together rather than crashing the
 * inbox pipeline.
 */
function parseRecoveryActionKind(actionJson: string | undefined): string {
  if (!actionJson) return "unknown";
  try {
    const parsed = JSON.parse(actionJson) as unknown;
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      const keys = Object.keys(parsed as Record<string, unknown>);
      if (keys.length === 1) return keys[0]!;
    }
    return "unknown";
  } catch {
    return "unknown";
  }
}

/**
 * The Tauri command expects a `MissionEventKind` (the inner
 * variant), not the outer `MissionEvent` envelope. The bindings
 * generated by specta represent `MissionEventKind` as the same
 * `{type, payload}` shape that `MissionEvent` flattens, so the
 * envelope strip is structural.
 */
function unwrapToKind(event: MissionEvent): unknown {
  // The envelope is `{ mission_id, seq, ts, type, payload }`.
  // The kind is `{ type, payload }`.
  return { type: event.type, payload: (event as { payload?: unknown }).payload };
}

/**
 * Conservative fallback verdict table for use when the Tauri
 * command is unavailable. Mirrors the *Internal* branches of the
 * Rust mapping for the most-common event types. Unknown types
 * collapse to `Internal` so silent-by-default holds even in the
 * degraded mode.
 */
function fallbackVerdict(eventType: string): EventVisibility {
  switch (eventType) {
    case "mission.completed":
    case "mission.merge_resolved":
      return { kind: "inbox", inbox_kind: "completion", severity: "info" } as unknown as EventVisibility;
    case "mission.aborted":
    case "boundary.sub_supervisor_refused":
      return { kind: "inbox", inbox_kind: "escalation", severity: "warning" } as unknown as EventVisibility;
    case "boundary.side_effect_logged":
      return { kind: "inbox", inbox_kind: "side_effect", severity: "warning" } as unknown as EventVisibility;
    case "plan.proposed":
      return { kind: "inbox", inbox_kind: "escalation", severity: "action_required" } as unknown as EventVisibility;
    case "plan.rejected":
      // QC-3: the user already drove the reject; no inbox card is
      // needed in degraded mode either. The mission's subsequent
      // mission.aborted event will surface the escalation card.
      return { kind: "internal" } as unknown as EventVisibility;
    case "arbiter.decided":
      // In degraded (IPC-down) mode, surface arbiter decisions as a
      // Warning escalation. Prefer false-positive escalations over
      // silent loss: a user-visible warning recovers cleanly when
      // the user reloads, whereas a missed Escalate is invisible.
      return { kind: "inbox", inbox_kind: "escalation", severity: "warning" } as unknown as EventVisibility;
    default:
      return { kind: "internal" } as unknown as EventVisibility;
  }
}

/**
 * Test-only: clear the in-process cache. Called between tests so
 * one test's cache hit doesn't leak into another.
 */
export function _resetVisibilityCache(): void {
  cache.clear();
}
