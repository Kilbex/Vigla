// MSV Step 5 — frontend mission state types.
//
// One mission at a time per the MSV constraint
// (`docs/proposals/msv-spec.md` §2.1). Lives alongside the existing
// `OpsStore` worker/squad state; the two stores don't share data.

import type { CompletionVerdict, MergeResolution, MissionSpec } from "../bindings";
import type { InboxCard } from "../inbox/types";

// QC-3 — re-export bindings types so missions/* code does not
// hard-depend on the auto-generated bindings.ts path. PlanProposed's
// new optional fields surface through these aliases.
export type {
  BoundFit,
  BoundFitKind,
  EnvelopeFit,
  TechChoice,
} from "../bindings";

export type MissionLifecycle =
  | "created"
  | "executing"
  | "pending_plan_approval"
  | "reviewing"
  | "attention"
  | "complete_pending_merge"
  | "merged"
  | "reverted"
  | "discarded"
  // Historical persisted-event compatibility. Current runtimes reject Extend.
  | "extended"
  | "aborted"
  // The mission is paused waiting for a self-healing condition
  // (e.g. a vendor quota window to reopen). Distinct from `attention`
  // because no user input is required — the orchestrator's wake-up
  // task resumes execution automatically; the inbox renders the countdown.
  | "paused";

export type TaskStatus =
  | "pending"
  | "in_progress"
  | "under_review"
  | "integrated"
  | "failed";

export type WorkerStatus =
  | "spawned"
  | "working"
  | "submitted"
  | "under_review"
  | "integrated"
  | "failed";

export interface MissionTask {
  index: number;
  title: string;
  description?: string | null;
  status: TaskStatus;
  assignedWorkerId: string | null;
  integrationSha: string | null;
  snapshotTag: string | null;
  // Optional for replay compatibility with events written before task-graph
  // metadata shipped. Current supervisor missions populate these fields.
  dependsOn?: number[];
  role?: "implementer" | "tester" | "reviewer";
  criteriaSummary?: string;
  scopePaths?: string[];
}

export interface MissionWorker {
  id: string;
  taskIndex: number;
  taskTitle: string;
  status: WorkerStatus;
  latestProgress: string | null;
  submittedFiles: string[];
}

export type AttentionKind =
  | "mission_complete"
  | "mission_aborted"
  | "arbiter_escalation"
  | "side_effect_logged"
  | "sub_supervisor_refused"
  // Row 4 (R6): a vendor quota window closed; the mission is paused
  // until the wake-up task resumes it. Self-healing — no user action.
  | "mission_paused";

/**
 * Severity guides visual treatment. `soft` = informational (the
 * mission is still running or otherwise actionable without blocking);
 * `hard` = the mission requires a user decision before it can
 * continue. Maps to existing color tokens: soft → `--accent-planning`,
 * hard → `--accent-failed` per `docs/visual-direction.md`.
 */
export type AttentionSeverity = "soft" | "hard";

export interface AttentionItem {
  kind: AttentionKind;
  severity: AttentionSeverity;
  summary: string;
  surfacedAt: string;
  /**
   * Row 4 (R6): for `mission_paused` items, the unix-ms timestamp the
   * vendor quota window is estimated to reopen. Drives the live
   * countdown so a paused mission reads as "resumes in mm:ss" instead
   * of looking hung. Unset for every other attention kind.
   */
  resumeAtMs?: number | null;
}

export interface ActiveMission {
  id: string;
  spec: MissionSpec;
  lifecycle: MissionLifecycle;
  startedAt: string;
  updatedAt: string;
  statusLine: string;
  /** 0..100 — integrated tasks / total tasks. */
  progressPercent: number;
  tasks: MissionTask[];
  workers: Record<string, MissionWorker>;
  testsPassed: boolean | null;
  completionSummary: string | null;
  filesChanged: number;
  resolution: MergeResolution | null;
  /** Revert commit emitted by `mission.reverted`; null before rollback. */
  restoredSha?: string | null;
  abortReason: string | null;
  attention: AttentionItem[];
  /**
   * Phase 1 (G4 measurement clause — supervisor strip). Latest
   * human-readable one-line description of what the supervisor is
   * currently doing, derived in `applyMissionEvent` from the most
   * recent `supervisor.*` / lifecycle / boundary event. `null` until
   * the first applicable event has landed; the team view hides the
   * strip while it is null.
   */
  supervisorActivity: string | null;
  /**
   * Most recent directive decoded from a historical `mission.extended`
   * event. Current runtimes do not emit this event.
   */
  lastExtensionDirective: string | null;
  /**
   * Timestamp of the most recent historical `mission.extended` event.
   */
  lastExtensionAt: string | null;
  /**
   * QC-2: monotonically-increasing per-mission counter that
   * disambiguates which proposed plan the user is looking at.
   * Starts at 0 on the first `plan.proposed`; increments on each
   * `plan.regeneration_requested`. Stays 0 for missions that never
   * pause (the default `confirm_plan: null` flow).
   */
  planGeneration: number;
  /**
   * QC-3: short prose summary of the proposed plan, rendered above
   * the task list in `MissionPlanPreview`. `null` for missions that
   * predate Mission Pre-Planning (no PlanProposed event) or for
   * adapters that don't emit it.
   */
  planOverview: string | null;
  /**
   * QC-3: typed tech-stack rows attached to the latest PlanProposed
   * event. `null` for missions whose supervisor adapter doesn't
   * emit them. `is_new` flagged rows render with a `[new]` badge.
   */
  planTechStack: import("../bindings").TechChoice[] | null;
  /**
   * QC-3: four-bound self-assessment from the supervisor on the
   * latest PlanProposed event. `null` for missions whose adapter
   * doesn't emit it (gating then collapses to QC-2 semantics).
   */
  planEnvelopeFit: import("../bindings").EnvelopeFit | null;
  /**
   * S1: tier + overall score from the most recent
   * `supervisor.audit_completed` event. `null` until the mission
   * loop emits the first audit result. The inbox renders the full
   * breakdown; lightweight UI gates read `overall` directly.
   */
  audit: { tier: string; overall: number } | null;
  /**
   * S1: full AuditReport payload (serialised) from the most recent
   * `supervisor.audit_completed` event. Kept alongside the
   * tier+overall summary above so MissionInbox's AuditBreakdown
   * has the full per-scorer payload without round-tripping through
   * the event log.
   */
  auditPayloadJson: string | null;
  /**
   * Structured completion verdict from
   * `mission.completion_verdict_rendered`. `null` until the verdict
   * is rendered. MissionInbox reads this slot to render the
   * residual-risk band, unresolved-issues list, and recommendation
   * surface.
   */
  verdict: CompletionVerdict | null;
  /**
   * S3: inbox cards derived from this mission's event stream.
   * Cards are produced by the ingest reducer when an event's
   * visibility verdict is `Inbox`. Resolved cards stay in the
   * list (dimmed); the list is sorted by seq ascending.
   */
  inbox: InboxCard[];
}

export interface MissionsState {
  /**
   * Latest mission. Stays populated through terminal states so the
   * user can see the outcome; replaced when a new mission starts.
   */
  active: ActiveMission | null;
  /**
   * Terminal overlay visibility is UI chrome, not mission data.
   * Dismissing the merged/discarded/aborted card must not clear
   * `active`, because the mission detail, verdict, and revert context
   * remain the operator's trust trail until a new mission replaces it.
   */
  terminalOverlayDismissed: boolean;
  /**
   * A2 (Tier-2G): canonical cwd of the most-recently-started mission
   * in this session. Used to scope memory commands to the right
   * per-repo kernel even after the active mission terminates — so a
   * user can pin a note after a mission accepts without losing the
   * repo context. `null` only at fresh app launch before the first
   * mission is started. Memory commands are disabled when null.
   */
  currentRepoCwd: string | null;
}

export const emptyMissionsState = (): MissionsState => ({
  active: null,
  terminalOverlayDismissed: false,
  currentRepoCwd: null,
});

export function isTerminal(lifecycle: MissionLifecycle): boolean {
  return (
    lifecycle === "merged" ||
    lifecycle === "reverted" ||
    lifecycle === "discarded" ||
    lifecycle === "extended" ||
    lifecycle === "aborted"
  );
}

export const emptyInbox = (): InboxCard[] => [];
