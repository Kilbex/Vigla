import type { Event } from "../bindings";
import { applyEvent } from "./ingest";
import type { OpsState } from "./types";

/// Internal helper shared by `ingest` (applies to visible state) and
/// the replay-mode shadow path (applies to `liveSnapshot`). Performs
/// the shallow-clone-and-mutate dance that Zustand needs for selector
/// triggers while preserving applyEvent's in-place semantics.
///
/// PRE-clone (not post-clone) the specific `workers[wid]` and
/// `tasks[tid]` entries that `applyEvent` will mutate: a post-clone
/// would only fix the outgoing `next` but leave prev's shared
/// per-entry ref already mutated, retroactively corrupting prior
/// Zustand snapshots that React selectors held via `Object.is`.
export function applyToOpsState(prev: OpsState, event: Event): OpsState {
  const next: OpsState = {
    workers: { ...prev.workers },
    workerOrder: prev.workerOrder,
    tasks: { ...prev.tasks },
    taskOwner: { ...prev.taskOwner },
    workerEvents: { ...prev.workerEvents },
    alerts: prev.alerts,
    totalEvents: prev.totalEvents,
    totalSpendUsd: prev.totalSpendUsd,
    activeCount: prev.activeCount,
    needsInputCount: prev.needsInputCount,
    selectedWorkerId: prev.selectedWorkerId,
    derivedDependencyEdges: prev.derivedDependencyEdges,
    lastSeqByWorker: { ...prev.lastSeqByWorker },
    // Squads are UI-only and untouched by applyEvent — carry refs
    // through unchanged. Squad actions update them on a separate path.
    squads: prev.squads,
    squadOrder: prev.squadOrder,
    workerSquad: prev.workerSquad,
    // Review status is UI-only and untouched by applyEvent — carry
    // through unchanged. Review actions update it on a separate path.
    reviewStatus: prev.reviewStatus,
    // Batch 3 — Review-queue focus is ephemeral UI focus, untouched
    // by applyEvent. Carry through unchanged.
    reviewFocusedWorkerId: prev.reviewFocusedWorkerId,
  };
  const wid = event.worker_id;
  if (next.workers[wid]) {
    next.workers[wid] = { ...next.workers[wid] };
  }
  // P6 — `applyEvent` now mutates `workerEvents[wid]` in place
  // (push + shift). Pre-clone the touched bucket so prior snapshots
  // held by React selectors stay byte-stable. Other buckets keep
  // their shared refs.
  if (next.workerEvents[wid]) {
    next.workerEvents[wid] = [...next.workerEvents[wid]];
  }
  const tid = event.task_id;
  if (tid && next.tasks[tid]) {
    next.tasks[tid] = { ...next.tasks[tid] };
  }
  applyEvent(next, event);
  return next;
}

/// Deep-enough clone for `liveSnapshot` capture at `enterReplay`.
/// `applyEvent` mutates `workers[wid]`, `tasks[tid]`, and (P6)
/// `workerEvents[wid]` in place, so the snapshot must own its own
/// copies of those entries — otherwise the next ingest-into-
/// liveSnapshot would mutate refs that React selectors might still
/// hold across the commit. Enumerated explicitly (no `...s` spread)
/// so callers can pass `OpsStore` without leaking actions into the
/// snapshot.
export function cloneOpsState(s: OpsState): OpsState {
  return {
    workers: Object.fromEntries(
      Object.entries(s.workers).map(([k, v]) => [
        k,
        { ...v, missionTimeline: [...v.missionTimeline] },
      ]),
    ),
    workerOrder: [...s.workerOrder],
    tasks: Object.fromEntries(
      Object.entries(s.tasks).map(([k, v]) => [k, { ...v }]),
    ),
    taskOwner: { ...s.taskOwner },
    workerEvents: Object.fromEntries(
      Object.entries(s.workerEvents).map(([k, v]) => [k, [...v]]),
    ),
    alerts: [...s.alerts],
    totalEvents: s.totalEvents,
    totalSpendUsd: s.totalSpendUsd,
    activeCount: s.activeCount,
    needsInputCount: s.needsInputCount,
    selectedWorkerId: s.selectedWorkerId,
    derivedDependencyEdges: [...s.derivedDependencyEdges],
    lastSeqByWorker: { ...s.lastSeqByWorker },
    // Step 19 — squad action paths use immutable updates (new arrays
    // for `workerIds`, new dicts for `squads`/`workerSquad`), so a
    // shallow per-entry clone matches the worker/task pattern. Each
    // Squad entry's `workerIds` is also spread to keep prior refs
    // immutable.
    squads: Object.fromEntries(
      Object.entries(s.squads).map(([k, v]) => [
        k,
        { ...v, workerIds: [...v.workerIds] },
      ]),
    ),
    squadOrder: [...s.squadOrder],
    workerSquad: { ...s.workerSquad },
    reviewStatus: { ...s.reviewStatus },
    reviewFocusedWorkerId: s.reviewFocusedWorkerId,
  };
}
