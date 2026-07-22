import { lazy, Suspense, useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { commands, events } from "./bindings";
import CommandPanel from "./command-panel/CommandPanel";
import CommsFeed from "./comms/CommsFeed";
import ErrorBoundary from "./ErrorBoundary";
import InboxOverview from "./inbox/InboxOverview";
import MissionHistory from "./inbox/MissionHistory";
import MissionInbox from "./inbox/MissionInbox";
import { useSurface, useSurfaceStore } from "./inbox/router";
import { useShowAllEvents } from "./settings/preferences";
import { useGlobalKeyboard } from "./keyboard";
import { selectMissionOverlayVisible, useMissionsStore } from "./missions/store";
import OperationsRoom from "./operations/OperationsRoom";
import { selectReplay, selectSelectedWorkerId, useOpsStore } from "./store";

const IS_WEB_DEMO = import.meta.env.VITE_VIGLA_WEB_DEMO === "1";
const WebDemoBanner = IS_WEB_DEMO
  ? lazy(() => import("./demo/WebDemoBanner"))
  : null;

// Step 6 — Operations Room dashboard MVP.
//
// The app is now three live regions driven by the canonical
// `worker_event` stream from the orchestrator:
//   * CommandPanel  — global counters
//   * OperationsRoom — React Flow canvas of station tiles
//   * CommsFeed     — alert cards + spawn controls
//
// Worker tiles, dependency edges, and alert rendering are projections
// of `useOpsStore`. The store reduces canonical events into UI
// snapshots in `app/src/store/ingest.ts`. Replay-mode routing happens
// inside `ingest` itself — events arriving during replay accumulate
// in `liveSnapshot` so `exitReplay` can restore the live room.
//
// ## Bundle policy
//
// `CommandPanel`, `OperationsRoom`, and `CommsFeed` are the always-on
// chrome — eager imports keep first-paint deterministic. Everything
// below is conditional UI: the Drawer only matters when a worker is
// selected, ReplayPanel only when replay mode is on, MissionOverlay
// only when a mission is active, and Settings only when the modal is
// open. We lazy-load each and gate mounting on the relevant store
// slice, so neither their code nor their transitive deps (xterm,
// React Flow extras, etc.) ever enter the first-paint chunk.
const Drawer = lazy(() => import("./drawer/Drawer"));
const ReplayPanel = lazy(() => import("./replay/ReplayPanel"));
const Settings = lazy(() => import("./settings/Settings"));
const MissionOverlay = lazy(() => import("./missions/MissionOverlay"));

const EVENT_LISTENER_RETRY_DELAYS_MS = [
  250,
  500,
  1_000,
  2_000,
  4_000,
  5_000,
] as const;

function attachEventListenerWithRetry(
  attach: () => Promise<() => void>,
  onAttached?: () => void,
): () => void {
  let cancelled = false;
  let unlisten: (() => void) | null = null;
  let retryTimer: ReturnType<typeof setTimeout> | null = null;
  let retryIndex = 0;

  const scheduleRetry = () => {
    if (cancelled) return;
    const delay = EVENT_LISTENER_RETRY_DELAYS_MS[retryIndex];
    retryIndex = Math.min(
      retryIndex + 1,
      EVENT_LISTENER_RETRY_DELAYS_MS.length - 1,
    );
    retryTimer = window.setTimeout(() => {
      retryTimer = null;
      connect();
    }, delay);
  };

  const connect = () => {
    if (cancelled) return;
    try {
      attach().then(
        (detach) => {
          if (cancelled) detach();
          else {
            unlisten = detach;
            onAttached?.();
          }
        },
        () => scheduleRetry(),
      );
    } catch {
      scheduleRetry();
    }
  };

  connect();
  return () => {
    cancelled = true;
    if (retryTimer !== null) window.clearTimeout(retryTimer);
    if (unlisten) unlisten();
  };
}

export default function App() {
  // P4 — gate the main UI on async runtime init. The host emits
  // `vigla://startup-complete` once migrations + supervisor +
  // memory registry are ready; we also poll `startup_status` on
  // mount in case the event fires before the listener attaches.
  const [startupReady, setStartupReady] = useState(false);
  const [startupError, setStartupError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let pollTimer: ReturnType<typeof setTimeout> | null = null;

    const markReady = () => {
      if (cancelled) return;
      setStartupReady(true);
    };
    const markError = (msg: string) => {
      if (cancelled) return;
      setStartupError(msg);
    };

    const detachReadyListener = attachEventListenerWithRetry(() =>
      listen<unknown>("vigla://startup-complete", () => markReady()),
    );
    const detachErrorListener = attachEventListenerWithRetry(() =>
      listen<string>("vigla://startup-error", (e) =>
        markError(String(e.payload ?? "unknown error")),
      ),
    );

    const poll = async () => {
      try {
        const status = await commands.startupStatus();
        if (status.phase === "ready") {
          markReady();
          return;
        }
        if (status.phase === "failed") {
          markError(status.error ?? "unknown startup failure");
          return;
        }
      } catch {
        // Treat poll failure as not-ready; retry on the schedule
        // below. The startup-error listener catches genuine failures.
      }
      if (!cancelled) {
        pollTimer = setTimeout(poll, 200);
      }
    };
    poll();

    return () => {
      cancelled = true;
      detachReadyListener();
      detachErrorListener();
      if (pollTimer) clearTimeout(pollTimer);
    };
  }, []);

  if (startupError) {
    return <StartupSurface error={startupError} />;
  }
  if (!startupReady) {
    return <StartupSurface error={null} />;
  }
  return <OperationalApp />;
}

function StartupSurface({ error }: { error: string | null }) {
  return (
    <div className={IS_WEB_DEMO ? "web-demo-shell" : "app-shell"}>
      <div className="app-grid" inert={IS_WEB_DEMO ? true : undefined}>
        {error ? (
          <div
            className="startup-error"
            role="alert"
            data-testid="startup-error"
          >
            <h2>Vigla failed to start</h2>
            <p>{error}</p>
          </div>
        ) : (
          <div
            className="startup-splash"
            role="status"
            data-testid="startup-splash"
          >
            Initializing Vigla…
          </div>
        )}
      </div>
    </div>
  );
}

function OperationalApp() {
  const ingest = useOpsStore((s) => s.ingest);
  const ingestMissionWorker = useOpsStore((s) => s.ingestMissionEvent);
  const ingestMission = useMissionsStore((s) => s.ingest);
  const [workerEventsReady, setWorkerEventsReady] = useState(false);
  const [missionEventsReady, setMissionEventsReady] = useState(false);

  useEffect(() => {
    setWorkerEventsReady(false);
    return attachEventListenerWithRetry(
      () =>
        events.workerEvent.listen((e) => {
          ingest(e.payload);
        }),
      () => setWorkerEventsReady(true),
    );
  }, [ingest]);

  // MSV mission events. Separate listener so the existing
  // worker-event pipeline stays untouched.
  useEffect(() => {
    setMissionEventsReady(false);
    return attachEventListenerWithRetry(
      () =>
        events.missionEventDto.listen((e) => {
          ingestMission(e.payload);
          ingestMissionWorker(e.payload);
        }),
      () => setMissionEventsReady(true),
    );
  }, [ingestMission, ingestMissionWorker]);

  if (!workerEventsReady || !missionEventsReady) {
    return <StartupSurface error={null} />;
  }
  return <OperationalContents />;
}

function OperationalContents() {
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [showAllEvents] = useShowAllEvents();
  const surface = useSurface();
  const surfaceMissionId = useSurfaceStore((s) => s.detail?.missionId ?? null);

  const openSettings = useCallback(() => setSettingsOpen(true), []);
  useGlobalKeyboard({ onOpenSettings: openSettings });

  // Each conditional surface is its own zero-cost subscription: the
  // selector returns a primitive / shallow ref, so the App only
  // re-renders when activation actually flips. This keeps the always-on
  // chrome above untouched even while a Drawer/Settings is open.
  const workerSelected = useOpsStore(selectSelectedWorkerId) !== null;
  const replayActive = useOpsStore((s) => selectReplay(s).mode) === "replay";
  const missionOverlayVisible = useMissionsStore(selectMissionOverlayVisible);
  const rightRailKey = `${surface}:${surfaceMissionId ?? ""}:${
    showAllEvents ? "all-events" : "curated"
  }`;

  return (
    <div className={IS_WEB_DEMO ? "web-demo-shell" : "app-shell"}>
      {WebDemoBanner ? (
        <Suspense fallback={null}>
          <WebDemoBanner />
        </Suspense>
      ) : null}
      <div className="app-grid" inert={IS_WEB_DEMO ? true : undefined}>
      <ErrorBoundary label="Command panel">
        <CommandPanel onOpenSettings={openSettings} />
      </ErrorBoundary>
      <ErrorBoundary label="Operations room">
        <OperationsRoom />
      </ErrorBoundary>
      <ErrorBoundary
        key={rightRailKey}
        label="Right rail"
        resetKey={rightRailKey}
      >
        {(() => {
          // S10 — surface router: right-rail switches between the
          // four top-level surfaces. Default (inbox) preserves
          // existing behaviour; ops_room is gated by showAllEvents
          // so the legacy CommsFeed only renders when the user has
          // explicitly opted in.
          switch (surface) {
            case "mission_detail":
              return <MissionInbox />;
            case "history":
              return <MissionHistory />;
            case "ops_room":
              return showAllEvents ? <CommsFeed /> : <InboxOverview />;
            case "inbox":
            default:
              return <InboxOverview />;
          }
        })()}
      </ErrorBoundary>
      {/* fallback={null} — every lazy surface here is itself an
          overlay / modal, so a brief blank during chunk fetch is
          indistinguishable from "not yet open". A spinner would
          flash on every first-time open. */}
      {workerSelected && (
        <Suspense fallback={null}>
          <ErrorBoundary label="Worker drawer">
            <Drawer />
          </ErrorBoundary>
        </Suspense>
      )}
      {settingsOpen && (
        <Suspense fallback={null}>
          <ErrorBoundary label="Settings">
            <Settings open={settingsOpen} onClose={() => setSettingsOpen(false)} />
          </ErrorBoundary>
        </Suspense>
      )}
      {missionOverlayVisible && (
        <Suspense fallback={null}>
          <ErrorBoundary label="Mission overlay">
            <MissionOverlay />
          </ErrorBoundary>
        </Suspense>
      )}
      </div>
      {replayActive && (
        <Suspense fallback={null}>
          <ErrorBoundary label="Replay">
            <ReplayPanel />
          </ErrorBoundary>
        </Suspense>
      )}
    </div>
  );
}
