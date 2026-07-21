import type { Event, WorkerInfo } from "../bindings";

/// Step 14 — replay-mode state. When `mode === "replay"`, the
/// operations room renders projections of `playbackEvents.slice(0,
/// position)` instead of live events from the orchestrator.
export interface ReplayState {
  mode: "live" | "replay";
  /// All known sessions, fetched lazily on entering replay mode.
  sessions: WorkerInfo[];
  /// Worker currently being replayed; null when in live mode or
  /// before a session is selected.
  workerId: string | null;
  /// All canonical events for the chosen session, fetched once.
  events: Event[];
  /// Index into `events` — number of events applied so far.
  position: number;
  /// 0 = paused, 1x | 2x | 4x | inf are the supported speeds.
  speed: 1 | 2 | 4 | 16;
  playing: boolean;
  /// True between beginReplay and finishReplay — the replay event
  /// stream is still being paged in from the orchestrator. UI uses
  /// this for a progress indicator. Reset on enterReplay /
  /// exitReplay (handled by the slices themselves).
  loading: boolean;
}

export const initialReplayState: ReplayState = {
  mode: "live",
  sessions: [],
  workerId: null,
  events: [],
  position: 0,
  speed: 1,
  playing: false,
  loading: false,
};
