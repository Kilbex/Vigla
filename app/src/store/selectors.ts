import type { Event } from "../bindings";
import { computeNeedsReview } from "./review";
import type { OpsStore, ReviewStatus, Squad } from "./types";

/// Selector helpers used by views. Stable reference equality where
/// possible to keep React Flow happy.
export const selectWorkerIds = (s: OpsStore): string[] => s.workerOrder;
export const selectWorker = (id: string) => (s: OpsStore) => s.workers[id];
export const selectAlerts = (s: OpsStore) => s.alerts;
export const selectGlobalCounters = (s: OpsStore) => ({
  totalEvents: s.totalEvents,
  totalSpendUsd: s.totalSpendUsd,
  active: s.activeCount,
  total: s.workerOrder.length,
  alerts: s.alerts.length,
  // P3 — scalar read of derived counter maintained by the ingest
  // reducer + `setReviewStatus`. `computeNeedsReview` still owns the
  // *list*; the *count* is shadowed here for cheap reads.
  needsInput: s.needsInputCount,
});

// Stable empty-array sentinel — Zustand uses Object.is equality to
// short-circuit re-renders, so a fresh `[]` per call (`?? []`) would
// loop `useSyncExternalStore` when a worker has no events yet. Mirrors
// the EMPTY_EDGES pattern in ingest.ts.
const EMPTY_EVENTS: Event[] = [];
export const selectWorkerEvents = (workerId: string) => (s: OpsStore) =>
  s.workerEvents[workerId] ?? EMPTY_EVENTS;
export const selectSelectedWorkerId = (s: OpsStore) => s.selectedWorkerId;

export const selectReplay = (s: OpsStore) => s.replay;
export const selectIsReplay = (s: OpsStore) => s.replay.mode === "replay";

// Step 19 — squad selectors. `selectSquadOf` returns the squad a
// given worker belongs to (or null), used by Station to render the
// squad-color bar. The empty-string fallback inside the selector
// keeps the function stable across calls.
export const selectSquadIds = (s: OpsStore): string[] => s.squadOrder;
export const selectSquad = (id: string) => (s: OpsStore): Squad | undefined =>
  s.squads[id];
export const selectSquadOf =
  (workerId: string) =>
  (s: OpsStore): Squad | null => {
    const squadId = s.workerSquad[workerId];
    return squadId ? s.squads[squadId] ?? null : null;
  };

// Step 21 — true when the given worker is its squad's designated lead.
export const selectIsLead =
  (workerId: string) =>
  (s: OpsStore): boolean => {
    const squadId = s.workerSquad[workerId];
    if (!squadId) return false;
    return s.squads[squadId]?.leadWorkerId === workerId;
  };

/// Returns the derived dependency-edges list. Edges are computed in
/// `ingest.applyEvent` with content-equality so this selector returns
/// a stable reference whenever no edge-relevant field changed —
/// React Flow won't re-route or reset edge animations on every event.
export const selectDependencyEdges = (s: OpsStore) => s.derivedDependencyEdges;

// Batch 2 — review status selectors.
export const selectReviewStatus =
  (workerId: string) =>
  (s: OpsStore): ReviewStatus | undefined =>
    s.reviewStatus[workerId];

/// Returns all done/failed workers that need review (for ReviewQueue panel).
/// Batch 3 fix: filters by `reviewStatus`. Workers that are accepted,
/// rejected, or parked drop out of the queue. Workers with no status
/// or explicit "needs_review" status remain. Sorts by most recently
/// spawned first.
export const selectWorkersNeedingReview = (s: OpsStore): string[] =>
  computeNeedsReview(s);

// Batch 3 — Review-queue focus selector.
export const selectReviewFocusedWorkerId = (s: OpsStore): string | null =>
  s.reviewFocusedWorkerId;
