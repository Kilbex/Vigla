import { useEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { commands, type HealthDto } from "../bindings";
import { requiresAction } from "../inbox/types";
import { useSurfaceStore } from "../inbox/router";
import MemoryDrawerButton from "../memory/MemoryDrawerButton";
import PinNoteButton from "../memory/PinNoteButton";
import { useMissionsStore } from "../missions/store";
import { isTerminal } from "../missions/types";
import {
  selectGlobalCounters,
  selectIsReplay,
  selectWorkersNeedingReview,
  useOpsStore,
} from "../store";

function formatUptime(ms: number): string {
  const total = Math.floor(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${pad(h)}:${pad(m)}:${pad(s)}`;
}

interface CommandPanelProps {
  onOpenSettings?: () => void;
}

export default function CommandPanel({ onOpenSettings }: CommandPanelProps) {
  const [health, setHealth] = useState<HealthDto | null>(null);
  const [healthErr, setHealthErr] = useState<string | null>(null);
  // useShallow: skip the re-render when no displayed counter changed
  // (e.g. selectWorker, exitReplay — store mutations that don't touch
  // any of these five values).
  const counters = useOpsStore(useShallow(selectGlobalCounters));
  const isReplay = useOpsStore(selectIsReplay);
  const exitReplay = useOpsStore((s) => s.exitReplay);
  const setReviewFocus = useOpsStore((s) => s.setReviewFocus);
  const setSurface = useSurfaceStore((s) => s.setSurface);
  const workerNeedsInput = useOpsStore((s) => selectWorkersNeedingReview(s).length);
  const missionAttentionCount = useMissionsStore((s) => {
    const mission = s.active;
    if (!mission || isTerminal(mission.lifecycle)) return 0;
    const cardCount = mission.inbox.filter(requiresAction).length;
    const decisionCount =
      mission.lifecycle === "complete_pending_merge" ||
      mission.lifecycle === "attention"
        ? 1
        : 0;
    return cardCount + decisionCount;
  });
  const terminalMissionVisible = useMissionsStore((s) =>
    s.active ? isTerminal(s.active.lifecycle) : false,
  );
  const attentionCount =
    missionAttentionCount + (terminalMissionVisible ? 0 : workerNeedsInput);
  // Batch 3 (B3.2) — clicking the needs-input chip scrolls the
  // review queue into view and primes keyboard focus on the first
  // card. Snapshot the queue at click-time so we don't subscribe
  // the panel to every queue mutation.
  const peekFirstQueueId = (): string | null => {
    const ids = selectWorkersNeedingReview(useOpsStore.getState());
    return ids.length > 0 ? ids[0] : null;
  };
  const focusFirstNeedsInput = () => {
    const wid = peekFirstQueueId();
    setSurface("inbox");
    if (wid) setReviewFocus(wid);
    window.requestAnimationFrame(() => {
      const panel =
        document.querySelector(".inbox-card--severity-action_required") ??
        document.querySelector(".review-queue-panel") ??
        document.querySelector(".inbox-overview");
      if (panel) panel.scrollIntoView({ behavior: "smooth", block: "nearest" });
    });
  };

  const [eventsPulse, setEventsPulse] = useState(false);
  const lastEventsRef = useRef(counters.totalEvents);
  const lastPulseAtRef = useRef(0);

  useEffect(() => {
    if (counters.totalEvents === lastEventsRef.current) return;
    const now = Date.now();
    const prev = lastEventsRef.current;
    lastEventsRef.current = counters.totalEvents;
    if (now - lastPulseAtRef.current > 500) {
      lastPulseAtRef.current = now;
      setEventsPulse(true);
      const id = window.setTimeout(() => setEventsPulse(false), 120);
      return () => window.clearTimeout(id);
    }
    void prev;
  }, [counters.totalEvents]);

  // Event rate sliding history (30 intervals of 2 seconds = last 60 seconds)
  const [eventHistory, setEventHistory] = useState<number[]>(() => Array(30).fill(0));
  const totalEvents = counters.totalEvents;
  const totalEventsRef = useRef(totalEvents);
  useEffect(() => {
    totalEventsRef.current = totalEvents;
  }, [totalEvents]);

  const lastTickEventsRef = useRef(totalEvents);

  useEffect(() => {
    const id = window.setInterval(() => {
      const current = totalEventsRef.current;
      const diff = Math.max(0, current - lastTickEventsRef.current);
      lastTickEventsRef.current = current;
      setEventHistory((prev) => [...prev.slice(1), diff]);
    }, 2000);
    return () => window.clearInterval(id);
  }, []);

  const maxVal = Math.max(...eventHistory, 1);
  const points = eventHistory
    .map((val, idx) => {
      const x = (idx / (eventHistory.length - 1)) * 40; // 40px wide sparkline
      const y = 10 - (val / maxVal) * 8; // 10px high range, inset
      return `${x},${y}`;
    })
    .join(" ");

  useEffect(() => {
    let cancelled = false;
    const tick = () => {
      commands
        .healthCheck()
        .then((h) => {
          if (cancelled) return;
          setHealth(h);
          setHealthErr(null);
        })
        .catch((e) => {
          if (cancelled) return;
          setHealthErr(String(e));
        });
    };
    tick();
    const id = window.setInterval(tick, 1000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);

  return (
    <header className="command-panel" data-tauri-drag-region>
      <svg
        className="command-panel-reticle"
        width="14"
        height="14"
        viewBox="0 0 14 14"
        aria-hidden
        focusable="false"
      >
        <circle cx="7" cy="7" r="5.5" fill="none" stroke="currentColor" strokeWidth="0.8" opacity="0.7" />
        <line x1="7" y1="1" x2="7" y2="3" stroke="currentColor" strokeWidth="0.8" />
        <line x1="7" y1="11" x2="7" y2="13" stroke="currentColor" strokeWidth="0.8" />
        <line x1="1" y1="7" x2="3" y2="7" stroke="currentColor" strokeWidth="0.8" />
        <line x1="11" y1="7" x2="13" y2="7" stroke="currentColor" strokeWidth="0.8" />
        <circle cx="7" cy="7" r="1.2" fill="currentColor" />
      </svg>
      <span className="brand">Vigla</span>
      <span className="sep">·</span>
      <span className="meta">
        active <strong>{counters.active}</strong>
        <span className="meta-faint"> / {counters.total}</span>
      </span>
      <span className="sep">·</span>
      <span className="meta command-panel-events-meta">
        <div className="command-panel-sparkline-wrapper">
          <svg
            className="command-panel-sparkline"
            width="40"
            height="12"
            viewBox="0 0 40 12"
            aria-hidden
          >
            <polyline
              fill="none"
              stroke="currentColor"
              strokeWidth="1.2"
              points={points}
            />
          </svg>
        </div>
        events <strong className={eventsPulse ? "hud-chroma" : ""}>{counters.totalEvents}</strong>
      </span>
      <span className="sep">·</span>
      <span className="meta">
        spend <strong>${counters.totalSpendUsd.toFixed(3)}</strong>
      </span>
      {attentionCount > 0 ? (
        <>
          <span className="sep">·</span>
          <button
            type="button"
            className="meta meta-needs-input"
            onClick={focusFirstNeedsInput}
            aria-label={`${attentionCount} item${attentionCount === 1 ? "" : "s"} need input`}
            title="Open Inbox attention queue"
          >
            ⚠ {attentionCount} needs input
          </button>
        </>
      ) : null}
      <span className="sep">·</span>
      <span className="meta">
        uptime{" "}
        {health ? (
          formatUptime(health.uptime_ms)
        ) : healthErr ? (
          <span className="mission-review__faint" title={healthErr}>
            n/a
          </span>
        ) : (
          <span className="hud-skeleton">···</span>
        )}
      </span>
      <span className="meta-spacer" />
      <MemoryDrawerButton />
      <PinNoteButton />
      <button
        className="command-panel-history-surface"
        onClick={() => setSurface("history")}
        title="Open recent mission history"
        aria-label="Open mission history"
      >
        History
      </button>
      {isReplay ? (
        <button
          className="command-panel-history command-panel-channel command-panel-channel--replay"
          onClick={exitReplay}
          title="Return to the live event stream"
          aria-label="History mode active; click to return to live"
        >
          <span className="command-panel-channel__dot" aria-hidden />
          <span className="command-panel-channel__label">History mode</span>
        </button>
      ) : (
        <span
          className="command-panel-history command-panel-channel command-panel-channel--live"
          title="Live event stream active"
          aria-label="Live event stream active"
          role="status"
        >
          <span className="command-panel-channel__dot" aria-hidden />
          <span className="command-panel-channel__label">Live</span>
        </span>
      )}
      <button
        type="button"
        className={
          "meta meta-faint command-panel-version" +
          (healthErr ? " command-panel-version--err" : "")
        }
        title={healthErr ?? "Open Settings for app and runtime details"}
        onClick={onOpenSettings}
        aria-label="Open app settings and version details"
        disabled={!onOpenSettings}
      >
        <span className="command-panel-version__dot" aria-hidden />
        version{" "}
        {health ? (
          health.version
        ) : healthErr ? (
          <span className="mission-review__faint" title={healthErr}>
            n/a
          </span>
        ) : (
          <span className="hud-skeleton">···</span>
        )}
      </button>
    </header>
  );
}
