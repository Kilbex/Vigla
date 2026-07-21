import type { StoreSliceContext } from "./context";
import type { OpsState, OpsStore, Squad } from "./types";

type SquadActions = Pick<
  OpsStore,
  | "createSquad"
  | "renameSquad"
  | "setSquadColor"
  | "deleteSquad"
  | "assignWorkerToSquad"
  | "setSquadLead"
>;

/// Generate a squad ID. Random UUIDs are unique per session and survive
/// reset() (no recycling of dead IDs into freshly created squads).
function newSquadId(): string {
  // crypto.randomUUID is in jsdom + modern browsers + Node.
  return `sq-${crypto.randomUUID()}`;
}

export function createSquadSlice({ set }: StoreSliceContext): SquadActions {
  return {
    // Step 19 — squad actions are replay-aware: when in replay mode,
    // they write to `liveSnapshot` so the squad survives exitReplay.
    // The visible state in replay mode is the replay projection — we
    // don't pollute it with squad changes that belong to the live world.
    createSquad: (name, color) => {
      const squadId = newSquadId();
      set((prev) => {
        const isReplay = prev.replay.mode === "replay";
        const target: OpsState | null = isReplay ? prev.liveSnapshot : prev;
        if (!target) {
          console.warn(
            "[store] createSquad: replay mode but liveSnapshot is null — squad creation dropped",
            squadId,
          );
          return prev;
        }
        const squad: Squad = {
          id: squadId,
          name,
          color,
          workerIds: [],
          leadWorkerId: null,
          createdAt: Date.now(),
        };
        const update = {
          squads: { ...target.squads, [squadId]: squad },
          squadOrder: [...target.squadOrder, squadId],
        };
        return isReplay
          ? { liveSnapshot: { ...prev.liveSnapshot!, ...update } }
          : update;
      });
      return squadId;
    },

    renameSquad: (squadId, name) =>
      set((prev) => {
        const isReplay = prev.replay.mode === "replay";
        const target: OpsState | null = isReplay ? prev.liveSnapshot : prev;
        if (!target || !target.squads[squadId]) return prev;
        const next = { ...target.squads[squadId], name };
        const squads = { ...target.squads, [squadId]: next };
        return isReplay
          ? { liveSnapshot: { ...prev.liveSnapshot!, squads } }
          : { squads };
      }),

    setSquadColor: (squadId, color) =>
      set((prev) => {
        const isReplay = prev.replay.mode === "replay";
        const target: OpsState | null = isReplay ? prev.liveSnapshot : prev;
        if (!target || !target.squads[squadId]) return prev;
        const next = { ...target.squads[squadId], color };
        const squads = { ...target.squads, [squadId]: next };
        return isReplay
          ? { liveSnapshot: { ...prev.liveSnapshot!, squads } }
          : { squads };
      }),

    deleteSquad: (squadId) =>
      set((prev) => {
        const isReplay = prev.replay.mode === "replay";
        const target: OpsState | null = isReplay ? prev.liveSnapshot : prev;
        if (!target || !target.squads[squadId]) return prev;
        const { [squadId]: _gone, ...remaining } = target.squads;
        void _gone;
        const squadOrder = target.squadOrder.filter((id) => id !== squadId);
        const workerSquad: Record<string, string> = {};
        for (const [wid, sid] of Object.entries(target.workerSquad)) {
          if (sid !== squadId) workerSquad[wid] = sid;
        }
        const update = { squads: remaining, squadOrder, workerSquad };
        return isReplay
          ? { liveSnapshot: { ...prev.liveSnapshot!, ...update } }
          : update;
      }),

    assignWorkerToSquad: (workerId, squadId) =>
      set((prev) => {
        const isReplay = prev.replay.mode === "replay";
        const target: OpsState | null = isReplay ? prev.liveSnapshot : prev;
        if (!target) return prev;
        if (squadId !== null && !target.squads[squadId]) return prev;
        const prior = target.workerSquad[workerId];
        const squads = { ...target.squads };
        if (prior && prior !== squadId && squads[prior]) {
          squads[prior] = {
            ...squads[prior],
            workerIds: squads[prior].workerIds.filter((id) => id !== workerId),
            // If this worker was the prior squad's lead, clear the
            // lead pointer — leadership doesn't follow a worker out
            // of its squad.
            leadWorkerId:
              squads[prior].leadWorkerId === workerId
                ? null
                : squads[prior].leadWorkerId,
          };
        }
        if (squadId !== null) {
          const existingIds = squads[squadId].workerIds;
          if (!existingIds.includes(workerId)) {
            squads[squadId] = {
              ...squads[squadId],
              workerIds: [...existingIds, workerId],
            };
          }
        }
        const workerSquad: Record<string, string> = { ...target.workerSquad };
        if (squadId === null) {
          delete workerSquad[workerId];
        } else {
          workerSquad[workerId] = squadId;
        }
        const update = { squads, workerSquad };
        return isReplay
          ? { liveSnapshot: { ...prev.liveSnapshot!, ...update } }
          : update;
      }),

    setSquadLead: (squadId, workerId) =>
      set((prev) => {
        const isReplay = prev.replay.mode === "replay";
        const target: OpsState | null = isReplay ? prev.liveSnapshot : prev;
        if (!target || !target.squads[squadId]) return prev;
        // The lead must be an existing squad member (or null to
        // clear). Reject silently if the worker is not in this squad.
        if (
          workerId !== null &&
          !target.squads[squadId].workerIds.includes(workerId)
        ) {
          return prev;
        }
        const next = { ...target.squads[squadId], leadWorkerId: workerId };
        const squads = { ...target.squads, [squadId]: next };
        return isReplay
          ? { liveSnapshot: { ...prev.liveSnapshot!, squads } }
          : { squads };
      }),
  };
}
