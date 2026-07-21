import { create } from "zustand";
import { subscribeWithSelector } from "zustand/middleware";
import { initialReplayState } from "../replay/state";
import { emptyState } from "./ingest";
import { createReplaySlice } from "./replay";
import { createReviewSlice } from "./review";
import { createSquadSlice } from "./squads";
import { createWorkerSlice } from "./workers";
import type { StoreSet, StoreSliceContext } from "./context";
import type {
  Alert,
  OpsStore,
  ReviewStatus,
  Squad,
  SquadColor,
  WorkerSnapshot,
} from "./types";

export type { WorkerSnapshot, Alert, Squad, SquadColor, ReviewStatus };
export * from "./selectors";

/// Zustand store for the operations room. The public API stays flat for
/// components, while business logic lives in domain slices to keep
/// ingestion, replay, squads, and review state independently maintainable.
export const useOpsStore = create<OpsStore>()(
  subscribeWithSelector((set, get) => {
    const ctx: StoreSliceContext = { set: set as StoreSet, get };
    return {
      ...emptyState(),
      liveSnapshot: null,
      replay: initialReplayState,
      ...createWorkerSlice(ctx),
      ...createSquadSlice(ctx),
      ...createReviewSlice(ctx),
      ...createReplaySlice(ctx),
    };
  }),
);
