// S10 — surface router. Owns the "which top-level pane is showing
// in the right rail" decision: Inbox (default), Ops Room
// (power-user CommsFeed gated by Show-all-events), History
// (cross-mission table), or Mission Detail (per-mission deep
// view). Drives App.tsx's right-rail switch and the ⌘1/⌘2/⌘3
// keyboard handler.
//
// Lives in its own tiny Zustand store rather than the existing
// missions store so the surface choice survives mission
// transitions: starting a new mission must not auto-flip the
// user away from the History page they were inspecting.

import { create } from "zustand";
import type { MissionHistoryRow } from "./bindings-shim";

export type Surface =
  | "inbox" // default — InboxOverview right-rail
  | "ops_room" // legacy CommsFeed (only reachable when showAllEvents=true)
  | "history" // cross-mission MissionHistory table
  | "mission_detail"; // per-mission MissionInbox detail view

export interface MissionDetailAddress {
  /** Mission id being viewed. Stable across navigations; cleared
   *  when the surface transitions away from `mission_detail`. */
  missionId: string;
  /** Optional history-row payload carried alongside the id so the
   *  detail surface can render historical missions (those not in
   *  the active missions store) without a second backend fetch.
   *  `null`/absent when the address came from a code path that did
   *  not have a row in hand (e.g. inbox click-through on the active
   *  mission, or a wiring test). Consumers should narrow with
   *  `detail?.row` since both undefined and null are valid. */
  row?: MissionHistoryRow | null;
}

interface SurfaceStore {
  surface: Surface;
  /** Surface to restore when leaving a mission detail. */
  previousSurface: Exclude<Surface, "mission_detail">;
  /** Address payload for the mission_detail surface; null when the
   *  surface is anything else. */
  detail: MissionDetailAddress | null;
  /** Transition to a leaf surface. Use `openMission` for the
   *  mission_detail surface to ensure `detail` is populated
   *  consistently. */
  setSurface: (s: Exclude<Surface, "mission_detail">) => void;
  /** Open the mission_detail surface for a specific mission id.
   *  Pass the optional `row` from MissionHistory so the detail
   *  surface can render historical missions whose ids do not match
   *  the currently-active mission. */
  openMission: (missionId: string, row?: MissionHistoryRow | null) => void;
  /** Back-button: mission detail returns to its origin; other secondary
   *  surfaces return to inbox. From inbox, no-op. */
  back: () => void;
}

export const useSurfaceStore = create<SurfaceStore>((set) => ({
  surface: "inbox",
  previousSurface: "inbox",
  detail: null,
  setSurface: (s) => set({ surface: s, previousSurface: s, detail: null }),
  openMission: (missionId, row = null) =>
    set((state) => ({
      surface: "mission_detail",
      previousSurface:
        state.surface === "mission_detail" ? state.previousSurface : state.surface,
      detail: { missionId, row },
    })),
  back: () =>
    set((st) => {
      if (st.surface === "inbox") return st;
      if (st.surface === "mission_detail") {
        return { surface: st.previousSurface, detail: null };
      }
      return { surface: "inbox", previousSurface: "inbox", detail: null };
    }),
}));

/** Selector returning the current surface enum. Hooks call this to
 *  pin re-renders to surface flips (not unrelated state changes). */
export const selectSurface = (s: SurfaceStore): Surface => s.surface;

/** Selector returning the active mission-detail address, or null. */
export const selectMissionDetail = (
  s: SurfaceStore,
): MissionDetailAddress | null => s.detail;

/** Convenience hook for components that just want the enum. */
export function useSurface(): Surface {
  return useSurfaceStore(selectSurface);
}
