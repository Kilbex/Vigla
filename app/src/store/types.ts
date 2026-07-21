import type { Event, MissionEvent, Vendor, WorkerInfo, WorkerState } from "../bindings";
import type { ReplayState } from "../replay/state";

export const MAX_EVENTS_PER_WORKER = 500;

/// Closed set of squad accent colors. Hand-picked to be distinct from
/// vendor hues (claude/codex/gemini/opencode/mock) AND from
/// state accents (executing/planning/blocked/failed/done/reviewing) so
/// a station tile can carry vendor + state + squad cues without color
/// collision.
export type SquadColor =
  | "indigo"
  | "terracotta"
  | "sage"
  | "plum"
  | "sand"
  | "coral";

export const SQUAD_COLORS: readonly SquadColor[] = [
  "indigo",
  "terracotta",
  "sage",
  "plum",
  "sand",
  "coral",
];

/// Pure UI/store concept — squads live ABOVE the event boundary.
/// `applyEvent` never touches squads. Coordination (CLAUDE.md §4),
/// not runtime.
export interface Squad {
  id: string;
  name: string;
  color: SquadColor;
  workerIds: string[];
  /// Step 21 — squad lead. At most one worker per squad. The lead
  /// is just an ordinary member with an extra coordination role
  /// (executes a delegation/review playbook). Not a privileged
  /// runtime — "an employee with a different prompt," per the goal
  /// doc's framing.
  leadWorkerId: string | null;
  createdAt: number;
}

/// Live UI projection of a worker. Reduced from the event stream.
export interface WorkerSnapshot {
  id: string;
  shortId: string;
  vendor: Vendor;
  model: string | null;
  state: WorkerState;
  spawnedAt: number; // unix ms

  currentTaskId: string | null;
  currentTaskTitle: string | null;

  progress: number | null; // 0–100
  etaMs: number | null;
  progressNote: string | null;

  filesAdded: number;
  filesModified: number;
  filesDeleted: number;
  linesAdded: number;
  linesRemoved: number;

  testsPassed: number;
  testsFailed: number;
  testsSkipped: number;
  lastSuite: string | null;

  costUsd: number;
  inputTokens: number;
  outputTokens: number;

  recentLog: string | null;
  blockedOn: string[] | null;
  failureSummary: string | null;
  completionSummary: string | null;

  /// Set when the worker enters the lock-in phase after `done`.
  /// Read by Station to fade the celebration tint.
  flashUntil: number;

  eventCount: number;

  /// Mission workers are controlled by the mission runtime rather than the
  /// standalone worker command surface. The drawer uses this capability bit
  /// to hide unsupported stop/retry/follow-up/model/diff controls.
  missionScoped: boolean;
  missionId: string | null;
  missionTimeline: MissionWorkerTimelineEntry[];
}

export interface MissionWorkerTimelineEntry {
  ts: string;
  label: string;
  detail: string | null;
}

export interface TaskSnapshot {
  id: string;
  title: string;
  /// Worker currently handling this task (last-seen).
  workerId: string | null;
  state: WorkerState | null;
  completedAt: number | null;
}

export type AlertKind =
  | "completion"
  | "failure"
  | "blocked"
  | "unblocked"
  | "started"
  | "seq_gap";

export interface Alert {
  id: string;
  kind: AlertKind;
  workerId: string;
  workerShortId: string;
  title: string;
  detail: string | null;
  ts: number; // unix ms
}

/// One dependency edge between two worker tiles. Computed on every
/// event ingest into `OpsState.derivedDependencyEdges` with a content-
/// equality check so the array identity stays stable when nothing
/// edge-relevant changed (animations don't reset on every event).
export interface DependencyEdge {
  id: string;
  source: string;
  target: string;
  state: "pending" | "blocked" | "done";
}

export type ReviewStatus = "needs_review" | "accepted" | "rejected" | "parked";

export interface OpsState {
  workers: Record<string, WorkerSnapshot>;
  workerOrder: string[]; // insertion order — for stable layout
  tasks: Record<string, TaskSnapshot>;
  /// task_id → worker_id, used to lay out dependency edges.
  taskOwner: Record<string, string>;
  /// Per-worker bounded event history, used by the worker detail
  /// drawer (Step 8). Cap = `MAX_EVENTS_PER_WORKER`.
  workerEvents: Record<string, Event[]>;
  alerts: Alert[];
  totalEvents: number;
  totalSpendUsd: number;
  /// P3 — Derived O(1) counters maintained by the ingest reducer and
  /// `setReviewStatus`. `activeCount` = workers whose `state` is
  /// neither `done` nor `failed`. `needsInputCount` = done/failed
  /// workers whose `reviewStatus` is `undefined` or `needs_review`
  /// (i.e. would appear in `computeNeedsReview`). Selectors read
  /// these as scalars instead of recomputing on every notification.
  activeCount: number;
  needsInputCount: number;
  /// Worker currently open in the detail drawer; null = closed.
  selectedWorkerId: string | null;
  /// Derived list of dependency edges. Stable reference: replaced
  /// only when the computed content differs from the previous value
  /// (see ingest.ts).
  derivedDependencyEdges: DependencyEdge[];
  /// Last seq we observed per worker. Used by ingest to detect
  /// regressions and gaps (schema §6); the comparison is `> lastSeq+1`
  /// for gaps and `<= lastSeq` for regressions. Reset on `enterReplay`
  /// / `beginReplay` because replay is a fresh deterministic projection.
  lastSeqByWorker: Record<string, number>;
  /// Squads are a UI-only organizational layer. `applyEvent` never reads or
  /// writes these slices; manual actions and playbooks populate them, and one
  /// member may be designated as the visual lead.
  squads: Record<string, Squad>;
  /// Insertion order — drives the SquadPanel list.
  squadOrder: string[];
  /// `workerId` → `squadId`. Absence means unassigned. Mirrors
  /// `Squad.workerIds` for fast lookups in either direction.
  workerSquad: Record<string, string>;
  /// Batch 2 — Review status per worker. workerId → ReviewStatus.
  /// Absence means no explicit status (defaults to "needs_review" for
  /// done/failed workers). App-state only (not persisted to DB yet).
  reviewStatus: Record<string, ReviewStatus>;
  /// Batch 3 — Keyboard-navigation focus for the Review Queue.
  /// Independent from `selectedWorkerId` (which drives the Drawer);
  /// the focused card is what J/K/O/R/⇧R/A/X act on. Ephemeral UI
  /// focus — not replay-aware, not persisted.
  reviewFocusedWorkerId: string | null;
}

export type ApplyEvent = (event: Event) => void;

export interface OpsStore extends OpsState {
  ingest: (event: Event) => void;
  reset: () => void;
  selectWorker: (workerId: string | null) => void;

  replay: ReplayState;
  enterReplay: (sessions: WorkerInfo[]) => void;
  exitReplay: () => void;
  /// Reset replay state, set the target worker, and flip
  /// `replay.loading = true`. Subsequent `appendReplayPage` calls
  /// stream events in; `finishReplay` flips loading back off.
  beginReplay: (workerId: string) => void;
  /// Append a page of events to `replay.events` and project them
  /// into ops state via `applyEvent`. Safe to call repeatedly with
  /// disjoint pages; `replay.position` tracks `events.length`.
  appendReplayPage: (events: Event[]) => void;
  /// Mark the replay-page stream complete. Flips
  /// `replay.loading = false`. No state change otherwise.
  finishReplay: () => void;
  setReplayPlaying: (playing: boolean) => void;
  setReplaySpeed: (speed: ReplayState["speed"]) => void;
  setReplayPosition: (position: number) => void;
  stepReplay: (delta: number) => void;
  /// Like `stepReplay` but preserves `playing` (auto-stops only at
  /// end-of-stream) — used by the playback ticker so auto-play continues.
  advanceReplay: (delta: number) => void;

  liveSnapshot: OpsState | null;

  registerWorker: (
    id: string,
    vendor: Vendor,
    taskTitle: string | null,
    model?: string | null,
  ) => void;
  ingestMissionEvent: (event: MissionEvent) => void;
  registerMissionWorker: (
    id: string,
    vendor: Vendor,
    taskTitle: string,
    missionId: string,
  ) => void;
  updateMissionWorker: (
    id: string,
    update: {
      state?: WorkerState;
      note?: string;
      completionSummary?: string;
      failureSummary?: string;
      filesChanged?: number;
      timeline: MissionWorkerTimelineEntry;
    },
  ) => void;
  finishMissionWorkers: (
    missionId: string,
    state: "done" | "failed",
    timeline: MissionWorkerTimelineEntry,
  ) => void;
  setWorkerModel: (id: string, model: string | null) => void;

  createSquad: (name: string, color: SquadColor) => string;
  renameSquad: (squadId: string, name: string) => void;
  setSquadColor: (squadId: string, color: SquadColor) => void;
  deleteSquad: (squadId: string) => void;
  assignWorkerToSquad: (workerId: string, squadId: string | null) => void;
  setSquadLead: (squadId: string, workerId: string | null) => void;

  setReviewStatus: (workerId: string, status: ReviewStatus) => void;
  getReviewStatus: (workerId: string) => ReviewStatus | undefined;
  setReviewFocus: (workerId: string | null) => void;
}
