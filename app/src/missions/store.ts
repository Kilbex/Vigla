// MSV Step 5 — Zustand store for the active mission. Sits alongside
// `useOpsStore` (workers/squads); the two stores do not share data.

import { create } from "zustand";
import { subscribeWithSelector } from "zustand/middleware";
import { commands } from "../bindings";
import type { MissionEvent } from "../bindings";
import { applyInboxAction } from "../inbox/InboxState";
import type { InboxCard } from "../inbox/types";
import {
  _setBannerEmitter,
  _setInboxAppender,
  applyMissionEvent,
} from "./ingest";
import { persistMissionTrustSnapshot } from "./trustSnapshot";
import type {
  ActiveMission,
  AttentionItem,
  MissionLifecycle,
  MissionsState,
  MissionTask,
  MissionWorker,
} from "./types";
import { emptyMissionsState, isTerminal } from "./types";

interface MissionsStore extends MissionsState {
  ingest: (event: MissionEvent) => void;
  reset: () => void;
  dismissTerminalOverlay: () => void;
  /**
   * A2: record the cwd used to start the most recent mission. Called
   * from the deploy panel right after `commands.startMission(...)`
   * returns ok. The value persists across mission lifecycle so memory
   * commands keep working after an accept.
   */
  setCurrentRepoCwd: (cwd: string) => void;
  /**
   * S3: replace the active mission's inbox slice. Called by
   * InboxOverview when the user resolves a card. The reducer
   * builds the slice incrementally from events; this mutator is
   * only for user-driven actions (resolve).
   *
   * No-op if the mission_id doesn't match the active mission
   * (defensive — guards against stale callbacks after a mission
   * transition).
   */
  setInboxForActive: (missionId: string, inbox: InboxCard[]) => void;
}

export const useMissionsStore = create<MissionsStore>()(
  subscribeWithSelector((set, get) => ({
    ...emptyMissionsState(),
    ingest: (event) => {
      set((state) => applyMissionEvent(state, event));
      const active = get().active;
      if (active) persistMissionTrustSnapshot(active);
    },
    reset: () => set(emptyMissionsState()),
    dismissTerminalOverlay: () => set({ terminalOverlayDismissed: true }),
    setCurrentRepoCwd: (cwd) => set({ currentRepoCwd: cwd }),
    setInboxForActive: (missionId, inbox) =>
      set((state) => {
        if (!state.active || state.active.id !== missionId) return state;
        return { ...state, active: { ...state.active, inbox } };
      }),
  })),
);

// S3 — wire the ingest module's inbox-append side-channel into
// this store. Called once at module load; safe because the store
// is a module-level singleton.
_setInboxAppender((missionId, card) => {
  useMissionsStore.setState((state) => {
    if (!state.active || state.active.id !== missionId) return state;
    const next = applyInboxAction(
      { cards: state.active.inbox },
      { type: "upsert", card },
    );
    return { ...state, active: { ...state.active, inbox: next.cards } };
  });
});

// S3 — wire the macOS banner emitter. Internal-only — surfaces
// only `ActionRequired` cards when the window is unfocused; the
// caller (ingest dispatch) guards on visibility.
_setBannerEmitter((missionId, title, body) => {
  // Guard like the inbox appender above: if a different mission has become
  // active since the async visibility lookup started, drop the banner so a
  // stale completion notification doesn't fire against the new mission (FE-5).
  if (useMissionsStore.getState().active?.id !== missionId) return;
  commands.surfaceInboxNotification(title, body).catch(() => {
    // Notification failed — silent: we don't alert about an
    // alerting failure.
  });
});

// ─────────────────────────────────────────────────────────────────────
// Selectors. Keep them trivial — the heavy lifting lives in the
// ingest reducer so derived fields are stored, not recomputed.
// ─────────────────────────────────────────────────────────────────────

export const selectActiveMission = (s: MissionsState): ActiveMission | null =>
  s.active;

export const selectMissionLifecycle = (
  s: MissionsState,
): MissionLifecycle | null => s.active?.lifecycle ?? null;

export const selectMissionTasks = (s: MissionsState): MissionTask[] =>
  s.active?.tasks ?? [];

export const selectMissionWorkers = (
  s: MissionsState,
): Record<string, MissionWorker> => s.active?.workers ?? {};

export const selectAttentionCount = (s: MissionsState): number =>
  s.active?.attention.length ?? 0;

export const selectAttentionItems = (s: MissionsState): AttentionItem[] =>
  s.active?.attention ?? [];

export const selectMissionStatusLine = (s: MissionsState): string | null =>
  s.active?.statusLine ?? null;

export const selectMissionProgress = (s: MissionsState): number | null =>
  s.active?.progressPercent ?? null;

export const selectMissionId = (s: MissionsState): string | null =>
  s.active?.id ?? null;

export const selectMissionOverlayVisible = (s: MissionsState): boolean =>
  s.active !== null && !s.terminalOverlayDismissed;

/**
 * A2: cwd of the current repository for memory IPC calls. Returns
 * `null` when no mission has been started this session — memory
 * commands key off this to know which per-repo kernel to address.
 */
export const selectCurrentRepoCwd = (s: MissionsState): string | null =>
  s.currentRepoCwd;

/**
 * True when the mission has reached a state where the user must (or
 * may) decide its disposition. Drives the Review-Outcome surface.
 */
export const selectAwaitingDisposition = (s: MissionsState): boolean =>
  s.active?.lifecycle === "complete_pending_merge" ||
  s.active?.lifecycle === "attention";

/** S3: the active mission's inbox cards. */
export const selectInboxCards = (s: MissionsState): InboxCard[] =>
  s.active?.inbox ?? [];

/** S3: true when at least one ActionRequired card is unresolved. */
export const selectHasActionRequired = (s: MissionsState): boolean =>
  (s.active?.inbox ?? []).some(
    (c) => c.severity === "action_required" && !c.resolved,
  );

/** QC-2: true when the mission is paused on the proposed-plan surface. */
export const selectIsPendingPlanApproval = (s: MissionsState): boolean =>
  s.active?.lifecycle === "pending_plan_approval";

/** QC-2: the proposed tasks the user is reviewing. */
export const selectProposedTasks = (s: MissionsState): MissionTask[] =>
  s.active?.lifecycle === "pending_plan_approval" ? s.active.tasks : [];

/** QC-2: the current plan generation (0 = first proposal). */
export const selectPlanGeneration = (s: MissionsState): number =>
  s.active?.planGeneration ?? 0;

/**
 * Phase 1 (G4 measurement clause — supervisor strip). Returns the
 * one-line "supervisor: <doing X>" string for the team-view strip,
 * or `null` when no applicable supervisor activity exists (mission
 * hasn't started, or never received any supervisor.* event).
 */
export const selectSupervisorActivity = (s: MissionsState): string | null =>
  s.active?.supervisorActivity ?? null;

/**
 * True when there is no active mission OR the active mission is in
 * a terminal state — i.e. the user can start a new mission.
 */
export const selectCanStartMission = (s: MissionsState): boolean => {
  if (!s.active) return true;
  return isTerminal(s.active.lifecycle);
};
