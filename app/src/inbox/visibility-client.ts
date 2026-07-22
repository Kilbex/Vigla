// S3 — thin client around the Rust `mission_event_visibility`
// Tauri command. Cached by a stable, payload-aware key so the same
// visibility shape only crosses the IPC boundary once per app session.
//
// Falls back to a small in-process table on IPC failure so the UI remains
// usable when the orchestrator is degraded. Fallback verdicts are not cached:
// the next event must retry the authoritative command after a transient
// transport failure. The fallback is intentionally conservative — unknown
// events return `Internal` rather than surfacing arbitrary events.

import type { MissionEvent, EventVisibility, MissionEventKindDto } from "../bindings";
import { commands } from "../bindings";

const cache = new Map<string, EventVisibility>();
const inFlight = new Map<string, Promise<EventVisibility>>();

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
export function fetchVisibility(
  event: MissionEvent,
): Promise<EventVisibility> {
  const key = cacheKey(event);
  const hit = cache.get(key);
  if (hit) return Promise.resolve(hit);

  const pending = inFlight.get(key);
  if (pending) return pending;

  // Do not cache degraded policy: a transient outage must not override
  // authoritative routing for the remainder of the app session.
  const degradedVerdict = () => fallbackVerdict(event);
  let request: Promise<EventVisibility>;
  try {
    const kind = unwrapToKind(event) as MissionEventKindDto;
    request = commands.missionEventVisibility(kind).then(
      (verdict) => {
        cache.set(key, verdict);
        return verdict;
      },
      degradedVerdict,
    );
  } catch {
    request = Promise.resolve(degradedVerdict());
  }

  inFlight.set(key, request);
  const release = () => {
    if (inFlight.get(key) === request) inFlight.delete(key);
  };
  void request.then(release, release);
  return request;
}

/**
 * Build a stable cache key for an event. Pure event-type for most
 * variants; for `arbiter.decided` the verdict varies along two
 * axes — the `bound` (only present for Escalate) and the decision
 * kind (Accept/Extend/Scrub/Escalate). Extend also varies for the
 * terminal `mark_unachievable` rework kind. These fields feed the key so
 * each distinct Rust verdict is cached independently.
 *
 * For `supervisor.recovery_decided` the verdict is `Internal` today
 * but the action kind (retry/pause/escalate/request_supervisor) is
 * a forward-compat discriminator: if a future visibility rule
 * surfaces Escalate actions as Inbox while Retry stays Internal,
 * a single shared cache slot would alias the two. The cache key
 * incorporates the parsed action kind so each variant caches
 * independently. Mirrors the `arbiter.decided` discriminator
 * pattern (see commit 9809437).
 *
 * `mission.completion_verdict_rendered` likewise routes by the
 * serialized `recommendation.kind`: Accept is a completion while
 * Extend/Scrub are escalations. Keep those outcomes in separate slots.
 */
function cacheKey(event: MissionEvent): string {
  if (event.type === "arbiter.decided") {
    const payload = (event as { payload?: { bound?: unknown; decision_json?: string } }).payload;
    const bound = payload?.bound ?? "none";
    const decisionKind = parseDecisionCacheDiscriminator(payload?.decision_json);
    return `arbiter.decided:${bound}:${decisionKind}`;
  }
  if (event.type === "supervisor.recovery_decided") {
    const payload = (event as { payload?: { action_json?: string } }).payload;
    const actionKind = parseRecoveryActionKind(payload?.action_json);
    return `supervisor.recovery_decided:${actionKind}`;
  }
  if (event.type === "mission.completion_verdict_rendered") {
    const payload = (event as { payload?: { payload_json?: string } }).payload;
    const recommendationKind = parseCompletionRecommendationKind(payload?.payload_json);
    return `mission.completion_verdict_rendered:${recommendationKind}`;
  }
  return event.type;
}

/**
 * Extract the visibility-relevant discriminator from a serialized
 * ArbiterDecision. Extend decisions also include the nested rework kind:
 * `mark_unachievable` is user-visible while ordinary rework stays internal.
 * Returns `"unknown"` when the payload is missing or malformed.
 */
function parseDecisionCacheDiscriminator(
  decisionJson: string | undefined,
): string {
  if (!decisionJson) return "unknown";
  try {
    const parsed = JSON.parse(decisionJson) as {
      kind?: unknown;
      rework_kind?: { kind?: unknown };
    };
    if (typeof parsed.kind !== "string") return "unknown";
    if (parsed.kind !== "extend") return parsed.kind;
    const reworkKind = parsed.rework_kind?.kind;
    return `extend:${typeof reworkKind === "string" ? reworkKind : "unknown"}`;
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

/** Extract `recommendation.kind` from a serialized CompletionVerdict. */
function parseCompletionRecommendationKind(payloadJson: string | undefined): string {
  if (!payloadJson) return "unknown";
  try {
    const parsed = JSON.parse(payloadJson) as {
      recommendation?: { kind?: unknown };
    };
    return typeof parsed.recommendation?.kind === "string"
      ? parsed.recommendation.kind
      : "unknown";
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
function fallbackVerdict(event: MissionEvent): EventVisibility {
  switch (event.type) {
    case "mission.completed":
    case "mission.merge_resolved":
    case "mission.reverted":
    case "mission.paused":
      return { kind: "inbox", inbox_kind: "completion", severity: "info" } as unknown as EventVisibility;
    case "mission.aborted":
    case "boundary.sub_supervisor_refused":
      return { kind: "inbox", inbox_kind: "escalation", severity: "warning" } as unknown as EventVisibility;
    case "boundary.side_effect_logged":
      return { kind: "inbox", inbox_kind: "side_effect", severity: "warning" } as unknown as EventVisibility;
    case "plan.proposed":
    case "supervisor.decomposition_rejected":
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
    case "mission.completion_verdict_rendered": {
      const payload = (event as { payload?: { payload_json?: string } }).payload;
      const recommendationKind = parseCompletionRecommendationKind(payload?.payload_json);
      if (recommendationKind === "extend" || recommendationKind === "escalate") {
        return { kind: "inbox", inbox_kind: "escalation", severity: "action_required" } as unknown as EventVisibility;
      }
      if (recommendationKind === "scrub") {
        return { kind: "inbox", inbox_kind: "escalation", severity: "warning" } as unknown as EventVisibility;
      }
      // Match Rust's fail-visible default: Accept and malformed payloads
      // remain a completion card instead of disappearing during IPC loss.
      return { kind: "inbox", inbox_kind: "completion", severity: "info" } as unknown as EventVisibility;
    }
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
  inFlight.clear();
}
