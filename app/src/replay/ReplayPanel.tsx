import { useEffect, useRef, useState } from "react";
import { commands, type WorkerInfo } from "../bindings";
import { selectReplay, useOpsStore } from "../store";

const SPEEDS = [1, 2, 4, 16] as const;
/// Page size for the replay IPC loop. Chosen to match the existing
/// `MAX_EVENTS_PER_WORKER` mental model (500 → 512 ≈ one drawer-feed
/// worth per round-trip). Smaller than the orchestrator's
/// MAX_REPLAY_PAGE = 10_000 so the user sees progress visibly fill in
/// even on slow links.
const REPLAY_PAGE_SIZE = 512;

function formatRelative(spawnedAt: string): string {
  const ms = Date.parse(spawnedAt);
  if (!Number.isFinite(ms)) return spawnedAt;
  const diff = Math.max(0, Date.now() - ms);
  const s = Math.floor(diff / 1000);
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86_400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86_400)}d ago`;
}

export default function ReplayPanel() {
  const replay = useOpsStore(selectReplay);
  const exit = useOpsStore((s) => s.exitReplay);
  const begin = useOpsStore((s) => s.beginReplay);
  const append = useOpsStore((s) => s.appendReplayPage);
  const finish = useOpsStore((s) => s.finishReplay);
  const setPlaying = useOpsStore((s) => s.setReplayPlaying);
  const setSpeed = useOpsStore((s) => s.setReplaySpeed);
  const setPosition = useOpsStore((s) => s.setReplayPosition);
  const step = useOpsStore((s) => s.stepReplay);
  const advance = useOpsStore((s) => s.advanceReplay);
  const enter = useOpsStore((s) => s.enterReplay);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  // Monotonic token for in-flight pickSession requests. Newest click
  // wins: any older response that resolves later is dropped, and any
  // in-flight paging loop bails at its next iteration.
  const pickTokenRef = useRef(0);
  const sessions = Array.isArray(replay.sessions) ? replay.sessions : [];
  const events = Array.isArray(replay.events) ? replay.events : [];
  const selectedSession = replay.workerId
    ? sessions.find((session) => session.id === replay.workerId)
    : undefined;

  // The panel is normally unmounted immediately after exitReplay. Invalidate
  // first so a page promise that resolves in the same turn cannot commit to
  // the restored live store before React runs the unmount cleanup.
  useEffect(
    () => () => {
      pickTokenRef.current += 1;
      finish();
    },
    [finish],
  );

  // Refresh the session list on entering replay mode.
  useEffect(() => {
    if (replay.mode !== "replay") return;
    if (sessions.length > 0) return;
    let cancelled = false;
    setLoading(true);
    commands
      .listRecentWorkers(50)
      .then((res) => {
        if (cancelled) return;
        if (res.status === "ok") {
          enter(Array.isArray(res.data) ? res.data : []);
        } else {
          setErr(res.error);
        }
      })
      .catch((e) => {
        if (!cancelled) setErr(String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [replay.mode, sessions.length, enter]);

  // Playback ticker. When playing, advance position based on speed
  // mapped to the gap between consecutive event timestamps.
  useEffect(() => {
    if (replay.mode !== "replay") return;
    if (!replay.playing) return;
    if (replay.position >= events.length) {
      setPlaying(false);
      return;
    }
    const current = events[replay.position - 1];
    const next = events[replay.position];
    const gapMs =
      current && next
        ? Math.max(0, Date.parse(next.ts) - Date.parse(current.ts))
        : 50;
    const wait = Math.min(2000, gapMs / replay.speed);
    // Use advanceReplay (preserves `playing`), NOT stepReplay (which pauses) —
    // otherwise auto-play stops after a single tick (FE-1).
    const timer = window.setTimeout(() => advance(1), Math.max(8, wait));
    return () => window.clearTimeout(timer);
  }, [
    replay.mode,
    replay.playing,
    replay.position,
    events,
    replay.speed,
    setPlaying,
    advance,
  ]);

  if (replay.mode !== "replay") return null;

  const pickSession = async (worker: WorkerInfo) => {
    const myToken = ++pickTokenRef.current;
    const isCurrentRequest = () => {
      const current = useOpsStore.getState().replay;
      return (
        myToken === pickTokenRef.current &&
        current.mode === "replay" &&
        current.workerId === worker.id
      );
    };
    setErr(null);
    setLoading(true);
    begin(worker.id);
    try {
      let afterSeq: number | null = null;
      while (true) {
        const res = await commands.replayWorkerEventsPage(
          worker.id,
          afterSeq,
          REPLAY_PAGE_SIZE,
        );
        // Drop the result if a newer click, replay exit, or unmount
        // superseded this request mid-flight.
        if (!isCurrentRequest()) return;
        if (res.status !== "ok") {
          setErr(res.error);
          return;
        }
        const page = Array.isArray(res.data) ? res.data : [];
        if (page.length === 0) break;
        append(worker.id, page);
        afterSeq = page[page.length - 1].seq;
        if (page.length < REPLAY_PAGE_SIZE) break; // exhausted
      }
    } catch (e) {
      if (isCurrentRequest()) setErr(String(e));
    } finally {
      // Run on every termination path (clean exhaustion, error, throw).
      // Skip when a newer pick has taken over — that owner will run its
      // own finish() / setLoading(false). Otherwise the loading banner
      // would hang on "…" after an error.
      if (isCurrentRequest()) {
        finish();
        setLoading(false);
      }
    }
  };

  const total = events.length;

  return (
    <>
      <div className="replay-banner">
        <span className="replay-label">REPLAY</span>
        {replay.workerId ? (
          <span className="replay-target">
            {replay.workerId.slice(-8)} · {replay.position} / {total}
            {replay.loading ? "…" : ""}
          </span>
        ) : null}
        <button
          className="replay-btn"
          onClick={() => {
            pickTokenRef.current += 1;
            setLoading(false);
            exit();
          }}
        >
          ← back to live
        </button>
      </div>

      {!replay.workerId ? (
        <aside className="replay-sessions" aria-label="Past sessions">
          <div className="replay-sessions-title">PAST SESSIONS</div>
          {loading ? <div className="replay-loading">loading…</div> : null}
          {err ? (
            <div className="replay-error" role="alert">
              {err}
            </div>
          ) : null}
          {sessions.length === 0 && !loading ? (
            <div className="replay-empty">no sessions yet</div>
          ) : null}
          <ul className="replay-session-list">
            {sessions.map((w) => (
              <li key={w.id}>
                <button
                  className="replay-session-row"
                  onClick={() => pickSession(w)}
                >
                  <span className="replay-session-name">{w.name}</span>
                  <span className="replay-session-vendor">{w.vendor}</span>
                  <span className="replay-session-time">
                    {formatRelative(w.spawned_at)}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </aside>
      ) : (
        <footer
          className={
            "replay-controls" + (err ? " replay-controls--error" : "")
          }
          aria-label="Replay controls"
        >
          {err ? (
            <div className="replay-page-error" role="alert">
              <span className="replay-page-error__message">
                <strong>Replay could not be loaded.</strong> {err}
              </span>
              {selectedSession ? (
                <button
                  type="button"
                  className="replay-btn"
                  onClick={() => pickSession(selectedSession)}
                >
                  Retry replay
                </button>
              ) : null}
            </div>
          ) : null}
          <button
            className="replay-btn replay-control--toggle"
            onClick={() => setPlaying(!replay.playing)}
            disabled={replay.loading || total === 0}
          >
            {replay.playing ? "❚❚ pause" : "▶ play"}
          </button>
          <button
            className="replay-btn replay-control--step"
            onClick={() => step(-1)}
            disabled={replay.loading}
          >
            ← step
          </button>
          <button
            className="replay-btn replay-control--step"
            onClick={() => step(1)}
            disabled={replay.loading}
          >
            step →
          </button>
          <button
            className="replay-btn replay-control--rewind"
            onClick={() => setPosition(0)}
            disabled={replay.loading}
          >
            ⤴ rewind
          </button>
          <button
            className="replay-btn replay-control--end"
            onClick={() => setPosition(total)}
            disabled={replay.loading}
          >
            end →
          </button>
          <input
            className="replay-scrubber"
            type="range"
            min={0}
            max={total}
            value={replay.position}
            style={total > 0 ? { "--_pct": `${(replay.position / total) * 100}%` } as React.CSSProperties : undefined}
            onChange={(e) => setPosition(Number(e.target.value))}
            aria-label="replay position"
            disabled={replay.loading}
          />
          <div className="replay-speed">
            {SPEEDS.map((s) => (
              <button
                key={s}
                className={
                  "replay-speed-btn" +
                  (replay.speed === s ? " replay-speed-btn--on" : "")
                }
                onClick={() => setSpeed(s)}
              >
                {s === 16 ? "max" : `${s}×`}
              </button>
            ))}
          </div>
        </footer>
      )}
    </>
  );
}
