// MSV Step 5 — pure reducer that folds `MissionEvent`s into
// `MissionsState`. No I/O, no React; identically callable from
// tests and from the Zustand store.

import type { AcceptanceCriteria, AuthorityBound, MissionEvent } from "../bindings";
import { fetchVisibility } from "../inbox/visibility-client";
import type { InboxCard } from "../inbox/types";
import { getNotifyOnCompletion } from "../settings/preferences";
import type {
  ActiveMission,
  AttentionItem,
  MissionLifecycle,
  MissionsState,
  MissionTask,
  MissionWorker,
} from "./types";
import { emptyMissionsState } from "./types";

export function applyMissionEvent(
  state: MissionsState,
  event: MissionEvent,
): MissionsState {
  // mission.created always begins (or replaces) the active mission.
  // Note: currentRepoCwd is owned by the deploy panel and persists across
  // mission boundaries; the reducer must preserve it on every transition.
  if (event.type === "mission.created") {
    return {
      active: {
        id: event.mission_id,
        spec: event.payload.spec,
        lifecycle: "created",
        startedAt: event.ts,
        updatedAt: event.ts,
        statusLine: "Mission starting…",
        progressPercent: 0,
        tasks: [],
        workers: {},
        testsPassed: null,
        completionSummary: null,
        filesChanged: 0,
        resolution: null,
        abortReason: null,
        attention: [],
        planGeneration: 0,
        planOverview: null,
        planTechStack: null,
        planEnvelopeFit: null,
        supervisorActivity: null,
        lastExtensionDirective: null,
        lastExtensionAt: null,
        audit: null,
        auditPayloadJson: null,
        verdict: null,
        inbox: [],
      },
      terminalOverlayDismissed: false,
      currentRepoCwd: state.currentRepoCwd,
    };
  }

  // Every other event must target the currently-active mission.
  // Events for a different mission_id are ignored — they belong to
  // a stale mission whose `mission.created` we never saw.
  const current = state.active;
  if (!current || current.id !== event.mission_id) {
    return state;
  }

  const next: ActiveMission = {
    ...current,
    updatedAt: event.ts,
    tasks: current.tasks.map((t) => ({ ...t })),
    workers: { ...current.workers },
    attention: [...current.attention],
  };

  switch (event.type) {
    case "mission.execution_started": {
      next.lifecycle = "executing";
      next.statusLine = "Team starting…";
      break;
    }
    case "supervisor.decomposition": {
      next.tasks = event.payload.tasks.map<MissionTask>((t) => ({
        index: t.index,
        title: t.title,
        description: t.description ?? null,
        status: "pending",
        assignedWorkerId: null,
        integrationSha: null,
        snapshotTag: null,
        dependsOn: t.depends_on ?? [],
        role: t.role,
        criteriaSummary: summarizeTaskCriteria(t.criteria),
        scopePaths: (t.scope_paths ?? []).map((p) => String(p)),
      }));
      next.statusLine = `Team is planning ${next.tasks.length} task${
        next.tasks.length === 1 ? "" : "s"
      }`;
      break;
    }
    case "supervisor.decomposition_rejected": {
      // Cycle / orphan / duplicate-index / empty. Visibility routing adds the
      // actionable inbox card; this reducer keeps the mission status concise.
      next.statusLine = "Decomposition rejected by orchestrator";
      break;
    }
    case "plan.proposed": {
      // QC-2: replace the proposed task list and pause for user
      // approval. Tasks render the same shape as decomposition tasks
      // (read-only, no assigned worker yet). The
      // `supervisor.decomposition` event is emitted alongside
      // `plan.proposed` by the backend, so `next.tasks` may already
      // hold the same titles; we rewrite anyway to keep this branch
      // self-contained when events arrive in a different order.
      next.tasks = event.payload.tasks.map<MissionTask>((t) => ({
        index: t.index,
        title: t.title,
        description: t.description ?? null,
        status: "pending",
        assignedWorkerId: null,
        integrationSha: null,
        snapshotTag: null,
        dependsOn: t.depends_on ?? [],
        role: t.role,
        criteriaSummary: summarizeTaskCriteria(t.criteria),
        scopePaths: (t.scope_paths ?? []).map((p) => String(p)),
      }));
      next.lifecycle = "pending_plan_approval";
      next.planGeneration = event.payload.generation;
      // QC-3: persist the rich plan-preview context so MissionPlanPreview
      // and the drawer's Plan tab can render the mind map, envelope
      // panel, and tech-stack badges. Each field is independently
      // optional so legacy adapters (no envelope, no overview) still
      // populate the basic plan-approval card.
      next.planOverview = event.payload.overview ?? null;
      next.planTechStack = event.payload.tech_stack ?? null;
      next.planEnvelopeFit = event.payload.envelope_fit ?? null;
      next.statusLine = "Plan proposed — review and approve";
      break;
    }
    case "plan.confirmed": {
      // QC-2: user accepted the plan; workers are about to spawn.
      // `planGeneration` is intentionally left where it is so the
      // review surface can still display which generation was
      // confirmed if it cares to.
      next.lifecycle = "executing";
      next.statusLine = "Plan confirmed — workers starting";
      break;
    }
    case "plan.regeneration_requested": {
      // QC-2: user asked for a new plan. Lifecycle stays at
      // `pending_plan_approval` (the supervisor is about to emit
      // another `plan.proposed`); just update the status line so
      // the UI shows the in-flight state.
      next.statusLine = "Regenerating plan…";
      break;
    }
    case "worker.spawned": {
      const { worker_id, task_index, task_title } = event.payload;
      const worker: MissionWorker = {
        id: worker_id,
        taskIndex: task_index,
        taskTitle: task_title,
        status: "spawned",
        latestProgress: null,
        submittedFiles: [],
      };
      next.workers[worker_id] = worker;
      const task = next.tasks.find((t) => t.index === task_index);
      if (task) {
        task.status = "in_progress";
        task.assignedWorkerId = worker_id;
      }
      next.statusLine = `Spawned worker on "${task_title}"`;
      next.lifecycle = "executing";
      break;
    }
    case "worker.progress": {
      const worker = next.workers[event.payload.worker_id];
      if (worker) {
        worker.latestProgress = event.payload.note;
        worker.status = worker.status === "spawned" ? "working" : worker.status;
        if (next.lifecycle !== "attention") {
          next.statusLine = event.payload.note;
        }
      }
      break;
    }
    case "worker.result_submitted": {
      const worker = next.workers[event.payload.worker_id];
      if (worker) {
        worker.status = "submitted";
        worker.submittedFiles = [...event.payload.files];
      }
      if (next.lifecycle !== "attention") {
        next.statusLine = `Reviewing "${worker?.taskTitle ?? "submission"}"`;
      }
      break;
    }
    case "supervisor.review_started": {
      const worker = next.workers[event.payload.worker_id];
      if (worker) {
        worker.status = "under_review";
      }
      if (next.lifecycle !== "attention") {
        next.lifecycle = "reviewing";
      }
      break;
    }
    case "supervisor.integrated": {
      const { worker_id, integration_sha, snapshot_tag } = event.payload;
      const worker = next.workers[worker_id];
      if (worker) {
        worker.status = "integrated";
        const task = next.tasks.find((t) => t.index === worker.taskIndex);
        if (task) {
          task.status = "integrated";
          task.integrationSha = integration_sha;
          task.snapshotTag = snapshot_tag;
        }
      }
      // Recompute progress.
      const integrated = next.tasks.filter((t) => t.status === "integrated").length;
      next.progressPercent = next.tasks.length
        ? Math.round((integrated * 100) / next.tasks.length)
        : 0;
      if (next.lifecycle !== "attention") {
        next.statusLine = `Integrated ${integrated}/${next.tasks.length}`;
        next.lifecycle = "executing";
      }
      break;
    }
    case "supervisor.test_result": {
      next.testsPassed = event.payload.passed;
      // No status-line change; tests are background validation.
      break;
    }
    case "mission.completed": {
      next.lifecycle = "complete_pending_merge";
      next.completionSummary = event.payload.summary;
      next.filesChanged = event.payload.files_changed;
      next.progressPercent = 100;
      next.statusLine = "Mission complete — awaiting your decision";
      next.attention = upsertAttention(next.attention, {
        kind: "mission_complete",
        severity: "soft",
        summary: event.payload.summary,
        surfacedAt: event.ts,
      });
      break;
    }
    case "mission.merge_resolved": {
      next.resolution = event.payload.resolution;
      next.lifecycle = resolutionLifecycle(event.payload.resolution.type);
      next.statusLine = resolutionStatusLine(event.payload.resolution.type);
      // Mission is now in a terminal disposition (merged/discarded);
      // every attention item — `mission_complete`,
      // `arbiter_escalation`, `side_effect_logged`, etc. — represents
      // an unresolved decision that the user has now resolved by
      // choosing the merge outcome. Clear the entire array so the
      // attention badge collapses with the disposition.
      next.attention = [];
      break;
    }
    case "mission.aborted": {
      next.lifecycle = "aborted";
      next.abortReason = event.payload.reason;
      next.statusLine = `Aborted: ${event.payload.reason}`;
      next.attention = upsertAttention(next.attention, {
        kind: "mission_aborted",
        severity: "hard",
        summary: event.payload.reason,
        surfacedAt: event.ts,
      });
      break;
    }
    case "arbiter.decided": {
      const { worker_id, bound, decision_json } = event.payload;
      if (bound) {
        const worker = next.workers[worker_id];
        if (worker) {
          worker.status = "failed";
          const task = next.tasks.find((t) => t.index === worker.taskIndex);
          if (task && task.status !== "integrated") {
            task.status = "failed";
          }
        }
        const workerTitle = worker?.taskTitle ?? worker_id;
        const summary = `Escalation (${bound}) on "${workerTitle}"`;
        next.statusLine = summary;
        next.attention = upsertAttention(next.attention, {
          kind: "arbiter_escalation",
          severity: "hard",
          summary,
          surfacedAt: event.ts,
        });
      } else if (decision_json.includes('"kind":"scrub"')) {
        // Scrub: the worker's output is discarded and the mission
        // continues. Mark the worker/task failed so it doesn't hang in
        // working/submitted forever (FE-2). No escalation — the arbiter
        // has already decided; there's nothing for the user to act on.
        const worker = next.workers[worker_id];
        if (worker) {
          worker.status = "failed";
          const task = next.tasks.find((t) => t.index === worker.taskIndex);
          if (task && task.status !== "integrated") {
            task.status = "failed";
          }
        }
      }
      break;
    }
    case "mission.attention_ready": {
      next.lifecycle = "attention";
      next.statusLine =
        next.attention.find((item) => item.kind === "arbiter_escalation")?.summary ??
        "Mission needs your decision";
      break;
    }
    case "boundary.side_effect_logged": {
      // Phase 2: visible side-effect accounting. Discard removes
      // Vigla's branches/worktrees, but it cannot pretend external
      // package installs or network/API effects never happened.
      const { kind, summary, declared } = event.payload;
      next.attention = upsertAttention(next.attention, {
        kind: "side_effect_logged",
        severity: "soft",
        summary: `Side effect logged (${kind}${declared ? ", declared" : ", undeclared"}): ${summary}`,
        surfacedAt: event.ts,
      });
      break;
    }
    case "boundary.sub_supervisor_refused": {
      // Phase 1 (decisions.md entry 6): a soft attention item
      // documenting that the orchestrator refused a sub-supervisor
      // spawn. Mission keeps running with the original supervisor.
      const { requested_by_supervisor_id, requested_worker_id } = event.payload;
      next.attention = upsertAttention(next.attention, {
        kind: "sub_supervisor_refused",
        severity: "soft",
        summary: `Supervisor ${requested_by_supervisor_id} attempted to spawn sub-supervisor ${requested_worker_id} — refused per single-supervisor-per-mission boundary`,
        surfacedAt: event.ts,
      });
      break;
    }
    case "mission.extended": {
      // Historical replay compatibility. Earlier builds emitted this event
      // without scheduling a supervisor re-entry. Preserve the record and
      // collapse the old review surface without claiming work continued.
      next.lastExtensionDirective = event.payload.directive;
      next.lastExtensionAt = event.ts;
      next.lifecycle = "extended";
      // Treat the historical record as terminal for the prior round; clear
      // attention so a stale escalation/complete item doesn't keep the badge
      // lit when `mission.extended` arrives without a preceding
      // `mission.merge_resolved` (FE-4), mirroring the merge_resolved path.
      next.attention = [];
      break;
    }
    case "supervisor.audit_completed": {
      // Store tier + overall for lightweight gates and the inbox summary.
      next.audit = {
        tier: event.payload.tier,
        overall: event.payload.overall,
      };
      // Keep the full serialised payload so the inbox's
      // AuditBreakdown component can render per-scorer rows without
      // re-fetching the event log.
      next.auditPayloadJson = event.payload.payload_json;
      break;
    }
    case "mission.completion_verdict_rendered": {
      // A structured CompletionVerdict landed for this
      // mission. The wire carries the verdict as an opaque
      // payload_json string (typed bindings exist for the deserialised
      // shape). MissionInbox reads `verdict` directly.
      try {
        next.verdict = JSON.parse(event.payload.payload_json);
      } catch {
        // Malformed payload — leave verdict null. The legacy
        // mission.completed event will still fire as a fallback
        // user-facing signal.
        next.verdict = null;
      }
      break;
    }
    case "memory.context_bundle_composed": {
      // V1.3 (hybrid retrieval): a memory bundle was composed for a
      // worker. Pure telemetry — visibility is Internal, so this
      // does NOT surface in the inbox or comms feed. The supervisor
      // strip below picks it up to render a transient "memory
      // bundle composed (<source>, λ=<x>)" tooltip via
      // `deriveSupervisorActivity`.
      break;
    }
    case "mission.paused": {
      // Row 4 (R6): a vendor quota window closed and the wake-up task
      // will resume the mission automatically — no user input. But the
      // mission must not read as hung: surface a soft attention item
      // carrying the estimated resume time so the UI renders a live
      // countdown.
      const vendor = quotaVendorFromReason(event.payload.reason_json);
      const who = vendor ? `${vendor} quota` : "vendor quota";
      next.lifecycle = "paused";
      next.statusLine = `Paused — waiting on ${who}`;
      next.attention = upsertAttention(next.attention, {
        kind: "mission_paused",
        severity: "soft",
        summary: `Paused — waiting on ${who} to reopen`,
        surfacedAt: event.ts,
        resumeAtMs: event.payload.estimated_resume_at_ms,
      });
      break;
    }
    case "mission.resumed": {
      // Row 4 (R6): the quota window reopened and the wake-up task
      // resumed execution. Drop the paused attention item and return
      // the mission to the executing surface.
      next.lifecycle = "executing";
      next.statusLine = `Resumed — ${event.payload.vendor} quota reopened`;
      next.attention = next.attention.filter((a) => a.kind !== "mission_paused");
      break;
    }
    default: {
      // Forward-compat: variants the orchestrator may emit later but
      // that this MSV reducer doesn't surface yet. `updatedAt` has
      // already advanced above, so the mission card still ticks.
      break;
    }
  }

  // Phase 1 (G4 — supervisor strip): derive the one-line activity
  // string from the just-applied event and the resulting lifecycle.
  // The strip is peripheral; the derivation is intentionally tiny
  // and side-effect-free so it can be tested via the same reducer
  // pathway as everything else.
  next.supervisorActivity = deriveSupervisorActivity(
    event,
    next.supervisorActivity,
    next.lifecycle,
    next.lastExtensionDirective,
  );

  // S3 — fire-and-forget visibility lookup. The reducer is pure;
  // when the lookup resolves with an Inbox verdict, the store
  // appends a card via `appendInboxCard`. Internal / PowerUserOnly
  // events take no inbox action (PowerUserOnly is already in
  // `next.attention` from the legacy branches above).
  dispatchInboxLookup(event, next);

  const terminalOverlayDismissed =
    event.type === "mission.merge_resolved" ||
    event.type === "mission.aborted" ||
    event.type === "mission.extended"
      ? false
      : state.terminalOverlayDismissed;

  return {
    active: next,
    terminalOverlayDismissed,
    currentRepoCwd: state.currentRepoCwd,
  };
}

/**
 * Phase 1: pure derivation of the supervisor-strip activity string.
 * Reads only the event currently being applied, the (already-updated)
 * mission lifecycle, and the prior activity so unrelated events leave
 * the strip text unchanged. Returns `null` only when no applicable
 * event has ever landed.
 */
function deriveSupervisorActivity(
  event: MissionEvent,
  prior: string | null,
  lifecycle: MissionLifecycle,
  lastExtensionDirective: string | null,
): string | null {
  // Historical replay: report what was requested without implying the
  // unsupported continuation actually ran.
  if (event.type === "mission.extended") {
    const d = lastExtensionDirective?.trim();
    if (d && d.length > 0) {
      const truncated = d.length > 60 ? `${d.slice(0, 57)}…` : d;
      return `supervisor: legacy extension request — ${truncated}`;
    }
    return "supervisor: legacy extension request";
  }

  // Lifecycle-driven overrides — these always win because they map
  // to the explicit "paused" / "awaiting decision" states the user
  // most needs to see on the strip.
  if (lifecycle === "complete_pending_merge") {
    return "supervisor: awaiting your decision";
  }
  if (lifecycle === "attention") {
    return "supervisor: paused — see Attention";
  }
  if (lifecycle === "aborted") {
    return "supervisor: aborted";
  }

  switch (event.type) {
    // mission.created is short-circuited at the top of
    // applyMissionEvent (it resets the active mission with
    // supervisorActivity: null), so the strip stays hidden until the
    // first supervisor.* / lifecycle event lands.
    case "mission.execution_started":
      return "supervisor: starting up";
    case "supervisor.decomposition":
      return "supervisor: planning tasks";
    case "supervisor.decomposition_rejected":
      return "supervisor: decomposition rejected";
    case "plan.proposed":
      return "supervisor: proposing plan";
    case "plan.confirmed":
      return "supervisor: starting workers";
    case "plan.regeneration_requested":
      return "supervisor: regenerating plan";
    case "worker.spawned":
      return `supervisor: dispatched ${event.payload.worker_id}`;
    case "worker.progress":
      // Worker output drives the main status line; the strip
      // shouldn't churn with every line of worker progress.
      return prior;
    case "worker.result_submitted":
      return `supervisor: reviewing ${event.payload.worker_id}'s submission`;
    case "supervisor.review_started":
      return `supervisor: reviewing ${event.payload.worker_id}'s work`;
    case "supervisor.integrated":
      return `supervisor: integrated ${event.payload.worker_id}`;
    case "supervisor.test_result":
      return event.payload.passed
        ? `supervisor: tests passing — ${event.payload.summary}`
        : `supervisor: tests FAILING — ${event.payload.summary}`;
    case "mission.completed":
      return "supervisor: awaiting your decision";
    case "mission.merge_resolved":
      return prior;
    case "mission.aborted":
      return "supervisor: aborted";
    case "boundary.side_effect_logged":
      return prior;
    case "boundary.sub_supervisor_refused":
      return prior;
    case "memory.context_bundle_composed": {
      // V1.3 telemetry — transient strip update so the operator can
      // see retrieval firing in dev / power-user mode. λ shown only
      // for the retrieval path.
      const src = event.payload.source;
      const lam = event.payload.mmr_lambda;
      const suffix = lam !== null && lam !== undefined ? `, λ=${lam.toFixed(2)}` : "";
      return `supervisor: memory bundle composed (${src}${suffix})`;
    }
    default:
      return prior;
  }
}

function upsertAttention(
  current: AttentionItem[],
  item: AttentionItem,
): AttentionItem[] {
  // One attention item per kind; latest wins.
  const filtered = current.filter((a) => a.kind !== item.kind);
  return [...filtered, item];
}

/**
 * Best-effort vendor extraction from a `mission.paused` reason_json.
 * The wire form is the serialised Rust `PauseReason::WaitingForQuota
 * { vendor }` (externally tagged). Returns null on any parse miss —
 * the caller falls back to a generic "vendor quota" label rather than
 * throwing in the reducer path.
 */
function quotaVendorFromReason(reasonJson: string): string | null {
  try {
    const parsed = JSON.parse(reasonJson) as Record<string, unknown>;
    const wfq = parsed.WaitingForQuota as { vendor?: unknown } | undefined;
    if (wfq && typeof wfq.vendor === "string") return wfq.vendor;
    if (typeof parsed.vendor === "string") return parsed.vendor;
    return null;
  } catch {
    return null;
  }
}

function resolutionLifecycle(
  kind: "merged" | "discarded" | "extended",
): MissionLifecycle {
  switch (kind) {
    case "merged":
      return "merged";
    case "discarded":
      return "discarded";
    case "extended":
      return "extended";
  }
}

function resolutionStatusLine(kind: "merged" | "discarded" | "extended"): string {
  switch (kind) {
    case "merged":
      return "Merged into target branch";
    case "discarded":
      return "Discarded — no changes kept";
    case "extended":
      return "Extension requested (legacy record)";
  }
}

// ─────────────────────────────────────────────────────────────────────
// S3 — inbox dispatch. The reducer is pure; this helper kicks off
// the async visibility lookup and asks the store to upsert a card
// when an Inbox verdict comes back. The store mutator
// (`appendInboxCard`) is injected via a module-level setter so the
// reducer file stays React-free.
// ─────────────────────────────────────────────────────────────────────

let inboxAppender:
  | ((missionId: string, card: InboxCard) => void)
  | null = null;

let bannerEmitter:
  | ((missionId: string, title: string, body: string) => void)
  | null = null;

/** Wire up the runtime appender. Called once by the missions
 *  store on creation. Tests can re-call with their own stub. */
export function _setInboxAppender(
  fn: ((missionId: string, card: InboxCard) => void) | null,
): void {
  inboxAppender = fn;
}

/** Wire up the runtime macOS-banner emitter. Called once by the
 *  store on creation. */
export function _setBannerEmitter(
  fn: ((missionId: string, title: string, body: string) => void) | null,
): void {
  bannerEmitter = fn;
}

function dispatchInboxLookup(
  event: MissionEvent,
  active: ActiveMission,
): void {
  // Skip the lookup for `mission.created` — the active mission is
  // created from scratch; no event has a meaningful inbox card.
  if (event.type === "mission.created") return;
  if (!inboxAppender) return;

  fetchVisibility(event)
    .then((verdict) => {
      if (verdict.kind !== "inbox") return;
      const card = buildCard(event, active, verdict);
      inboxAppender?.(active.id, card);
      // S10 — gate the banner on either ActionRequired (existing
      // S3 surface) or terminal Completion verdict when the user
      // opted into notifyOnCompletion.
      const isCompletionVerdict =
        event.type === "mission.completion_verdict_rendered" ||
        event.type === "mission.completed" ||
        event.type === "mission.merge_resolved";
      const windowHidden =
        typeof document !== "undefined" && document.visibilityState !== "visible";
      const fireBanner =
        bannerEmitter !== null &&
        windowHidden &&
        (verdict.severity === "action_required" ||
          (isCompletionVerdict && getNotifyOnCompletion()));

      if (fireBanner && bannerEmitter) {
        // Pass the mission id so the store-side emitter can drop the banner
        // if a different mission has since become active — the completion
        // banner from mission N must not fire against mission N+1 (FE-5).
        bannerEmitter(active.id, card.title, card.detail ?? "");
      }
    })
    .catch(() => {
      // visibility-client already falls back; if even that throws
      // (impossible under the current type system), drop the event
      // rather than crash the reducer.
    });
}

function buildCard(
  event: MissionEvent,
  active: ActiveMission,
  verdict: { kind: "inbox"; inbox_kind: InboxCard["kind"]; severity: InboxCard["severity"] },
): InboxCard {
  const baseId = `${event.mission_id}:${event.seq}`;
  // Append worker_id to the id when present, so per-worker cards
  // dedupe across multi-task missions.
  const workerId =
    event.type === "worker.spawned"
      ? event.payload.worker_id
      : event.type === "worker.result_submitted"
      ? event.payload.worker_id
      : event.type === "supervisor.review_started"
      ? event.payload.worker_id
      : event.type === "supervisor.integrated"
      ? event.payload.worker_id
      : event.type === "arbiter.decided"
      ? event.payload.worker_id
      : event.type === "boundary.side_effect_logged"
      ? event.payload.worker_id
      : null;

  const id = workerId ? `${baseId}:${workerId}` : baseId;
  const { title, detail, bound } = describeEvent(event, active);

  // P1-7 safety net: replace any literal `"undefined"` substring that
  // slipped through a describe branch (e.g. an unhandled event variant
  // interpolating a missing field). Per-branch fixes in
  // `describeArbiterDecided` are the primary guard; this catches the
  // tail of unknown variants. Case-sensitive on purpose so genuine
  // user copy containing the word "Undefined" survives.
  const safeTitle = title.replace(/undefined/g, "unknown");
  const safeDetail =
    detail !== null ? detail.replace(/undefined/g, "unknown") : null;

  return {
    id,
    missionId: event.mission_id,
    seq: event.seq,
    surfacedAt: event.ts,
    kind: verdict.inbox_kind,
    severity: verdict.severity,
    title: safeTitle,
    detail: safeDetail,
    bound,
    resolved: false,
  };
}

function describeEvent(
  event: MissionEvent,
  active: ActiveMission,
): { title: string; detail: string | null; bound: AuthorityBound | null } {
  switch (event.type) {
    case "worker.result_submitted": {
      const worker = active.workers[event.payload.worker_id];
      return {
        title: `Submitted: "${worker?.taskTitle ?? event.payload.worker_id}"`,
        detail:
          event.payload.files.length > 0
            ? `${event.payload.files.length} file${
                event.payload.files.length === 1 ? "" : "s"
              } · ${event.payload.summary}`
            : event.payload.summary,
        bound: null,
      };
    }
    case "supervisor.integrated": {
      const worker = active.workers[event.payload.worker_id];
      const taskTitle = worker?.taskTitle ?? event.payload.worker_id;
      return {
        title: `Integrated: "${taskTitle}"`,
        detail: `sha ${event.payload.integration_sha.slice(0, 8)} · ${event.payload.snapshot_tag}`,
        bound: null,
      };
    }
    case "mission.completed":
      return {
        title: `Mission complete — ${event.payload.summary}`,
        detail: `${event.payload.files_changed} file${
          event.payload.files_changed === 1 ? "" : "s"
        } changed`,
        bound: null,
      };
    case "mission.aborted":
      return {
        title: `Mission aborted: ${event.payload.reason}`,
        detail: null,
        bound: null,
      };
    case "mission.merge_resolved":
      return {
        title:
          event.payload.resolution.type === "merged"
            ? "Merged"
            : event.payload.resolution.type === "discarded"
            ? "Discarded"
            : "Extended",
        detail: null,
        bound: null,
      };
    case "plan.proposed":
      return {
        title: `Plan proposed (${event.payload.tasks.length} task${
          event.payload.tasks.length === 1 ? "" : "s"
        })`,
        detail: "Review and approve or request a new plan",
        bound: null,
      };
    case "boundary.side_effect_logged":
      return {
        title: `Side effect: ${event.payload.kind}`,
        detail: event.payload.summary,
        bound: null,
      };
    case "boundary.sub_supervisor_refused":
      return {
        title: "Sub-supervisor spawn refused",
        detail: `Worker ${event.payload.requested_worker_id} attempted to act as a supervisor — refused per single-supervisor-per-mission boundary`,
        bound: null,
      };
    case "arbiter.decided":
      return describeArbiterDecided(event, active);
    default:
      return { title: active.statusLine ?? "Update", detail: null, bound: null };
  }
}

/**
 * Build an inbox-card description for an `arbiter.decided` event.
 *
 * Exported (additive, named export) so unit tests can drive this
 * branch directly without weaving through the async visibility
 * lookup. The runtime call site is unchanged — `describeEvent`
 * still routes here for the `arbiter.decided` switch arm.
 *
 * P1-7 hardening:
 *   - When the supervisor escalates before the `worker.spawned`
 *     event has been observed (or for mission-level escalations
 *     with an empty `worker_id`), `active.workers[worker_id]` is
 *     undefined and the prior `?? worker_id` fallback rendered the
 *     literal string `"undefined"` or an empty string in the
 *     card title.
 *   - When the upstream payload is malformed (`audit_overall` is
 *     `NaN`, `null`, or otherwise non-finite), `toFixed(2)` either
 *     throws or renders `"NaN"`.
 * Both cases are now coerced to safe, human-readable copy.
 */
export function describeArbiterDecided(
  event: Extract<MissionEvent, { type: "arbiter.decided" }>,
  active: ActiveMission,
): { title: string; detail: string | null; bound: AuthorityBound | null } {
  const { worker_id, decision_json, audit_overall, bound } = event.payload;
  const known = worker_id ? active.workers[worker_id] : null;
  // `||` (not `??`) so empty `taskTitle` and empty `worker_id`
  // both fall through to the next fallback rung.
  const workerTitle =
    known?.taskTitle ||
    (worker_id && worker_id.length > 0
      ? `worker ${worker_id.slice(-8)}`
      : "this mission");
  const auditDetail =
    typeof audit_overall === "number" && Number.isFinite(audit_overall)
      ? `Audit ${audit_overall.toFixed(2)}`
      : null;
  if (bound) {
    return {
      title: `Escalation: ${bound} bound on "${workerTitle}"`,
      detail:
        auditDetail !== null
          ? `${auditDetail}; see decision payload`
          : "see decision payload",
      bound,
    };
  }
  if (decision_json.includes('"kind":"scrub"')) {
    return {
      title: `Scrub: "${workerTitle}" discarded`,
      detail:
        auditDetail !== null
          ? `${auditDetail}; mission may proceed with remaining tasks`
          : "see decision payload",
      bound: null,
    };
  }
  // Accept (kind == "accept" or default).
  return {
    title: `Accepted: "${workerTitle}"`,
    detail: auditDetail !== null ? auditDetail : "see decision payload",
    bound: null,
  };
}

function summarizeTaskCriteria(
  criteria: AcceptanceCriteria | undefined,
): string | undefined {
  if (!criteria) return undefined;
  const explicit = criteria.summary?.trim();
  if (explicit) return explicit;
  const parts: string[] = [];
  if (typeof criteria.min_audit_overall === "number") {
    parts.push(`audit >= ${Math.round(criteria.min_audit_overall * 100)}%`);
  }
  if (criteria.require_tests_pass === true) {
    parts.push("tests must pass");
  }
  if (criteria.forbid_new_security_flags === true) {
    parts.push("no new security flags");
  }
  return parts.length > 0 ? parts.join(", ") : undefined;
}

export { emptyMissionsState };
