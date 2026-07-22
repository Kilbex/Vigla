import { initialReplayState } from "../replay/state";
import { applyEvent, emptyState } from "./ingest";
import { cloneOpsState } from "./ops-state";
import type { StoreSliceContext } from "./context";
import type { OpsState, OpsStore } from "./types";

type ReplayActions = Pick<
  OpsStore,
  | "enterReplay"
  | "exitReplay"
  | "beginReplay"
  | "appendReplayPage"
  | "finishReplay"
  | "setReplayPlaying"
  | "setReplaySpeed"
  | "setReplayPosition"
  | "stepReplay"
  | "advanceReplay"
>;

export function createReplaySlice({ set }: StoreSliceContext): ReplayActions {
  return {
    enterReplay: (sessions) =>
      set((prev) => {
        if (prev.replay.mode === "replay") {
          // Already in replay — only update session list. Preserve
          // liveSnapshot, visible, and replay state machine fields.
          // ReplayPanel.tsx calls enter(res.data) after entering
          // replay to populate sessions; if we re-snapshotted from
          // the now-empty visible state, we'd lose the live world.
          return { replay: { ...prev.replay, sessions } };
        }
        // Live → replay transition — snapshot live, reset visible.
        return {
          ...emptyState(),
          liveSnapshot: cloneOpsState(prev),
          replay: {
            ...prev.replay,
            mode: "replay",
            sessions,
            workerId: null,
            events: [],
            position: 0,
            playing: false,
            loading: false,
          },
        };
      }),

    exitReplay: () =>
      set((prev) => {
        if (prev.replay.mode !== "replay") return {};
        const restored = prev.liveSnapshot ?? emptyState();
        return {
          ...restored,
          liveSnapshot: null,
          replay: initialReplayState,
        };
      }),

    beginReplay: (workerId) =>
      set((prev) => {
        if (prev.replay.mode !== "replay") return {};
        return {
          ...emptyState(),
          selectedWorkerId: prev.selectedWorkerId,
          replay: {
            ...prev.replay,
            workerId,
            events: [],
            position: 0,
            playing: false,
            loading: true,
          },
        };
      }),

    appendReplayPage: (requestWorkerId, events) =>
      set((prev) => {
        if (events.length === 0) return {};
        const workerId = prev.replay.workerId;
        if (
          prev.replay.mode !== "replay" ||
          workerId === null ||
          requestWorkerId !== workerId
        ) {
          return {};
        }
        const allEvents = prev.replay.events.concat(events);
        let nextOps: OpsState;
        if (prev.replay.position === prev.replay.events.length) {
          nextOps = cloneOpsState(prev);
          for (const event of events) applyEvent(nextOps, event);
        } else {
          // A stale caller may still have moved the cursor while a page was
          // in flight. Rebuild the complete projection instead of applying
          // only the arriving suffix to that partial state.
          nextOps = emptyState();
          for (const event of allEvents) applyEvent(nextOps, event);
        }
        return {
          ...nextOps,
          replay: {
            ...prev.replay,
            events: allEvents,
            position: allEvents.length,
          },
        };
      }),

    finishReplay: () =>
      set((prev) =>
        prev.replay.mode === "replay"
          ? { replay: { ...prev.replay, loading: false } }
          : {},
      ),

    setReplayPlaying: (playing) =>
      set((prev) => {
        if (prev.replay.mode !== "replay") return {};
        return { replay: { ...prev.replay, playing } };
      }),

    setReplaySpeed: (speed) =>
      set((prev) =>
        prev.replay.mode === "replay"
          ? { replay: { ...prev.replay, speed } }
          : {},
      ),

    setReplayPosition: (position) =>
      set((prev) => {
        if (prev.replay.mode !== "replay") return {};
        const events = prev.replay.events;
        const clamped = Math.max(0, Math.min(events.length, position));
        const nextOps = projectReplay(prev, clamped);
        return {
          ...nextOps,
          replay: { ...prev.replay, position: clamped },
        };
      }),

    stepReplay: (delta) =>
      set((prev) => {
        if (prev.replay.mode !== "replay") return {};
        const events = prev.replay.events;
        const target = Math.max(
          0,
          Math.min(events.length, prev.replay.position + delta),
        );
        const nextOps = projectReplay(prev, target);
        return {
          ...nextOps,
          replay: { ...prev.replay, position: target, playing: false },
        };
      }),

    advanceReplay: (delta) =>
      set((prev) => {
        if (prev.replay.mode !== "replay") return {};
        const events = prev.replay.events;
        const target = Math.max(
          0,
          Math.min(events.length, prev.replay.position + delta),
        );
        const nextOps = projectReplay(prev, target);
        // Unlike stepReplay (a manual step, which pauses), preserve `playing`
        // so the playback ticker keeps advancing; auto-stop only when we reach
        // the end of the stream. (FE-1: stepReplay forced playing:false, which
        // killed auto-play after a single tick.)
        const playing = target < events.length ? prev.replay.playing : false;
        return {
          ...nextOps,
          replay: { ...prev.replay, position: target, playing },
        };
      }),
  };
}

/// Project the replay timeline up to `target` events. If `target` is
/// at or after the current replay position, apply only the delta —
/// otherwise rebuild from scratch. Avoids the O(N²) "replay all
/// events on every step" cost on long sessions.
function projectReplay(prev: OpsStore, target: number): OpsState {
  const events = prev.replay.events;
  const current = prev.replay.position;

  if (target >= current) {
    const next = cloneOpsState(prev);
    for (let i = current; i < target; i++) {
      applyEvent(next, events[i]);
    }
    return next;
  }
  const next = emptyState();
  for (let i = 0; i < target; i++) {
    applyEvent(next, events[i]);
  }
  return next;
}
