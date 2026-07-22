import type { StoreSliceContext } from "./context";
import type { OpsState, OpsStore } from "./types";

type ReviewActions = Pick<
  OpsStore,
  "setReviewStatus" | "getReviewStatus" | "setReviewFocus"
>;

export function createReviewSlice({
  set,
  get,
}: StoreSliceContext): ReviewActions {
  return {
    setReviewStatus: (workerId, status) =>
      set((prev) => {
        const isReplay = prev.replay.mode === "replay";
        const target: OpsState | null = isReplay ? prev.liveSnapshot : prev;
        if (!target) return prev;
        const reviewStatus = { ...target.reviewStatus, [workerId]: status };

        // P3 — keep needsInputCount aligned with the queue when the
        // status flips between {undefined | "needs_review"} and
        // {accepted | rejected | parked}. Only applies if the worker
        // is a standalone worker in a terminal state. Mission workers are
        // reviewed and integrated by their supervisor, not this queue.
        const worker = target.workers[workerId];
        const isTerminal =
          !worker?.missionScoped &&
          (worker?.state === "done" || worker?.state === "failed");
        const priorStatus = target.reviewStatus[workerId];
        const wasOnQueue =
          isTerminal &&
          (priorStatus === undefined || priorStatus === "needs_review");
        const nowOnQueue =
          isTerminal && status === "needs_review";
        let needsInputDelta = 0;
        if (wasOnQueue && !nowOnQueue) needsInputDelta = -1;
        else if (!wasOnQueue && nowOnQueue) needsInputDelta = 1;

        // Auto-move focus when the focused card leaves the queue.
        // §10.2 of the Batch-3 spec: if `workerId === focusedId` and
        // the new status is not `needs_review`, recompute focus from
        // the post-action queue. This only applies in live mode —
        // focus is not replay-aware.
        let focusUpdate: { reviewFocusedWorkerId?: string | null } = {};
        if (
          !isReplay &&
          prev.reviewFocusedWorkerId === workerId &&
          status !== "needs_review"
        ) {
          const nextWithStatus: OpsState = { ...prev, reviewStatus };
          const queue = computeNeedsReview(nextWithStatus);
          // Try to keep the relative position: if the prior queue
          // contained the worker at index i, pick queue[i] (clamped).
          const priorQueue = computeNeedsReview(prev);
          const priorIdx = priorQueue.indexOf(workerId);
          const nextIdx = Math.min(Math.max(priorIdx, 0), queue.length - 1);
          focusUpdate = {
            reviewFocusedWorkerId: queue.length === 0 ? null : queue[nextIdx],
          };
        }

        return isReplay
          ? {
              liveSnapshot: {
                ...prev.liveSnapshot!,
                reviewStatus,
                needsInputCount:
                  prev.liveSnapshot!.needsInputCount + needsInputDelta,
              },
            }
          : {
              reviewStatus,
              needsInputCount: prev.needsInputCount + needsInputDelta,
              ...focusUpdate,
            };
      }),

    getReviewStatus: (workerId) => {
      return get().reviewStatus[workerId];
    },

    setReviewFocus: (workerId) =>
      set((prev) => {
        // Focus is ephemeral live-mode UI state. In replay we still
        // allow setting it (browsing past sessions is a triage motion
        // per §4.5) but never write to liveSnapshot.
        if (prev.reviewFocusedWorkerId === workerId) return prev;
        return { reviewFocusedWorkerId: workerId };
      }),
  };
}

/// Shared implementation for the generic standalone-worker Review Queue.
/// Mission workers use supervisor review and never enter this queue.
export function computeNeedsReview(s: OpsState): string[] {
  const needsReview: string[] = [];
  for (const wid of s.workerOrder) {
    const worker = s.workers[wid];
    if (!worker) continue;
    if (worker.missionScoped) continue;
    if (worker.state !== "done" && worker.state !== "failed") continue;
    const status = s.reviewStatus[wid];
    if (status === undefined || status === "needs_review") {
      needsReview.push(wid);
    }
  }
  return needsReview;
}
